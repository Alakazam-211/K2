//! Garden data history, restore and snapshots (prd-garden-board-needs-v1
//! §6, GH5–GH11), on both doors:
//!
//! - **Widget door:** `store.history` / `store.restore` / `store.snapshot`
//!   through `POST /cli/zen/call` (`call.rs` hands them here with the
//!   placement's access, its maker's rights, and the manifest's
//!   `restoreSkips`).
//! - **Agent and person door:** `GET /cli/zen/data/history`,
//!   `POST /cli/zen/data/restore`, `POST /cli/zen/data/snapshot`
//!   (`k2 store history|restore|snapshot --widget`, `k2 zen data restore`).
//!
//! Rights (GH6): a restore only touches keys the caller may write, and a
//! snapshot only covers keys it may read (whole, never half). An agent's
//! restore always names `prefixes`: a whole-space restore is the person's
//! (403 `person_only`; the permission wall says "It can't be requested").
//! Every restore takes a pre-op snapshot of `gardens.sqlite` first
//! (`PreOp::Restore`), runs once per 10 s per placement or agent, is
//! charged like a commit of its ops, and writes one `zen.data.restore`
//! audit line with its counts.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::Deserialize;
use serde_json::{json, Map, Value as J};

use k2_core::garden_data::history::{
    self, HistoryItem, HistoryQuery, KeySet, RestoreTarget, DEFAULT_HISTORY_LIMIT, MAX_HISTORY_LIMIT, MAX_LABEL_CHARS,
    RESTORE_EVERY_SECS,
};
use k2_core::garden_data::snapshots::PreOp;
use k2_core::garden_data::{blobs, keys, Access, Actor, DataError, Space};

use super::agent_routes::{access, data_space, int_param, param, parse, ByNames};
use super::audit::{self, Counted};
use super::budget::{self, BudgetKey, Spend};
use super::{notify_changed, store};
use crate::zen_data_routes::DataWho;

pub const AGENT_HISTORY: &str = "/cli/zen/data/history";
pub const AGENT_RESTORE: &str = "/cli/zen/data/restore";
pub const AGENT_SNAPSHOT: &str = "/cli/zen/data/snapshot";

pub fn is_agent_route(p: &str) -> bool {
    matches!(p, AGENT_HISTORY | AGENT_RESTORE | AGENT_SNAPSHOT)
}

/// Who is asking, as both doors resolved it.
pub(crate) struct Caller {
    /// `placement:<g>/<p>`, `owner`, `agent:<handle>`, `shell:<handle>`.
    pub label: String,
    /// The stored writer (`by` on every change a restore writes).
    pub actor: Actor,
    pub acc: Access,
    /// The budget key (none for the person outside K2).
    pub budget: Option<BudgetKey>,
    /// An agent or shell tab (GH6: its restore always names prefixes).
    pub agent: bool,
    /// The widget manifest's `restoreSkips` (GH11).
    pub skips: Vec<String>,
    /// The widget's maker for the counted audit (widget door only).
    pub made: Option<String>,
}

fn spend(c: &Caller, what: Spend) -> Result<(), DataError> {
    match &c.budget {
        Some(k) => budget::spend(k, what),
        None => Ok(()),
    }
}

// ── options ───────────────────────────────────────────────────────────

fn field<T: serde::de::DeserializeOwned>(o: &Map<String, J>, name: &str, verb: &str) -> Result<Option<T>, DataError> {
    match o.get(name) {
        None | Some(J::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|_| DataError::BadRequest(format!("{verb}: {name} has the wrong type"))),
    }
}

fn only_known(o: &Map<String, J>, allowed: &[&str], verb: &str) -> Result<(), DataError> {
    match o.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(DataError::BadRequest(format!("{verb}: unknown option '{k}'"))),
        None => Ok(()),
    }
}

/// `store.history`'s options (the agent door passes its query the same way).
pub(crate) fn history_query(o: &Map<String, J>, verb: &str) -> Result<HistoryQuery, DataError> {
    only_known(o, &["key", "prefix", "before", "after", "limit", "from"], verb)?;
    let limit: u64 = field(o, "limit", verb)?.unwrap_or(DEFAULT_HISTORY_LIMIT as u64);
    if limit == 0 || limit > MAX_HISTORY_LIMIT as u64 {
        return Err(DataError::BadRequest(format!("{verb}: limit is 1..={MAX_HISTORY_LIMIT}")));
    }
    let before: Option<i64> = field(o, "before", verb)?;
    if before.is_some_and(|b| b < 1) {
        return Err(DataError::BadRequest(format!("{verb}: before is a rev, 1 or more")));
    }
    let after: Option<String> = field(o, "after", verb)?;
    if after.is_some() && before.is_none() {
        return Err(DataError::BadRequest(format!("{verb}: after goes with before (next from an earlier page)")));
    }
    let key: Option<String> = field(o, "key", verb)?;
    let prefix: String = field(o, "prefix", verb)?.unwrap_or_default();
    if key.is_some() && !prefix.is_empty() {
        return Err(DataError::BadRequest(format!("{verb}: one of key or prefix")));
    }
    Ok(HistoryQuery { key, prefix, before, after, limit: limit as usize })
}

fn item_json(i: HistoryItem, names: &mut ByNames) -> J {
    let mut v = json!({
        "key": i.key,
        "rev": i.rev,
        "deleted": i.deleted,
        "value": i.value,
        "prev": i.prev,
        "prevKind": i.prev_kind,
        "by": names.wire(&i.by),
        "at": i.at,
    });
    if let (Some(id), Some(o)) = (i.restore, v.as_object_mut()) {
        o.insert("restore".into(), J::String(id));
    }
    v
}

/// `StoreHistory` for the caller's read prefixes.
pub(crate) fn history(space: &Space, c: &Caller, q: &HistoryQuery) -> Result<J, DataError> {
    spend(c, Spend::Read)?;
    if matches!(c.acc, Access::None) {
        return Err(DataError::NotGranted { key: None });
    }
    if let Some(k) = &q.key {
        if !c.acc.can_read(k) {
            return Err(DataError::NotGranted { key: None });
        }
    }
    let filter = c.acc.read_prefixes();
    let page = store().history(space, q, filter.as_deref())?;
    audit::count(space.as_str(), &c.label, c.made.as_deref(), Counted::Read);
    let mut names = ByNames::new();
    let items: Vec<J> = page.items.into_iter().map(|i| item_json(i, &mut names)).collect();
    let next = page.next.map(|(before, after)| match after {
        Some(a) => json!({ "before": before, "after": a }),
        None => json!({ "before": before }),
    });
    Ok(json!({ "head": page.head, "oldest": page.oldest, "items": items, "next": next }))
}

fn string_list(o: &Map<String, J>, name: &str, verb: &str) -> Result<Vec<String>, DataError> {
    Ok(field::<Vec<String>>(o, name, verb)?.unwrap_or_default())
}

/// `YYYY-MM-DDTHH:MM[:SS]` in this computer's local time (the person's
/// `k2 zen data restore --at`).
pub(crate) fn local_time(s: &str) -> Result<i64, DataError> {
    use chrono::{NaiveDateTime, TimeZone};
    let t = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .map_err(|_| DataError::BadRequest("a time looks like 2026-10-26T17:30 (local time)".into()))?;
    chrono::Local
        .from_local_datetime(&t)
        .earliest()
        .map(|t| t.timestamp())
        .ok_or_else(|| DataError::BadRequest(format!("{s} doesn't exist in this computer's time zone")))
}

/// The one target a restore names, and its key set.
fn restore_args(space: &Space, c: &Caller, o: &Map<String, J>, verb: &str) -> Result<(RestoreTarget, KeySet, bool), DataError> {
    let to_rev: Option<i64> = field(o, "toRev", verb)?;
    let at: Option<i64> = field(o, "at", verb)?;
    let at_local: Option<String> = field(o, "atLocal", verb)?;
    let snapshot: Option<String> = field(o, "snapshot", verb)?;
    let dry: bool = field(o, "dryRun", verb)?.unwrap_or(false);
    let named = [to_rev.is_some(), at.is_some(), at_local.is_some(), snapshot.is_some()].iter().filter(|x| **x).count();
    if named != 1 {
        return Err(DataError::BadRequest(format!("{verb}: name exactly one of toRev, at or snapshot")));
    }
    if to_rev.is_some_and(|r| r < 0) {
        return Err(DataError::BadRequest(format!("{verb}: toRev is 0 or more")));
    }
    let set = KeySet::new(string_list(o, "prefixes", verb)?, string_list(o, "except", verb)?, c.skips.clone())?;
    let target = if let Some(r) = to_rev {
        RestoreTarget::Rev(r)
    } else if let Some(t) = at {
        RestoreTarget::At(t)
    } else if let Some(s) = at_local {
        RestoreTarget::At(local_time(&s)?)
    } else {
        let r = snapshot.unwrap_or_default();
        let hash = blobs::parse_ref(&r)?;
        // The snapshot is a file this widget's data holds, readable by the
        // caller like any blob (GB13 rights).
        super::blob_routes::readable_row(space, &c.acc, &c.actor, &hash)?;
        let bytes = blobs::read_all(&store().blob_dir(), &hash)?;
        RestoreTarget::Snapshot(history::parse_snapshot(&hash, &bytes, space)?)
    };
    // GH6 / R2: an agent's restore names prefixes; the whole space (or a
    // snapshot of the whole space) is the person's.
    let whole = match &target {
        RestoreTarget::Snapshot(d) => d.prefixes.is_empty(),
        _ => set.whole(),
    };
    if c.agent && whole {
        return Err(DataError::PersonOnly);
    }
    Ok((target, set, dry))
}

/// The last restore per placement or agent (GH5: one per 10 s).
fn last_restore() -> &'static Mutex<HashMap<String, Instant>> {
    static L: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per store (each test runs under its own home) and caller.
fn slot_key(label: &str) -> String {
    format!("{}|{label}", store().path().display())
}

/// Tests: let this store's callers restore again at once.
#[cfg(test)]
pub(crate) fn forget_restore_slots() {
    let mine = format!("{}|", store().path().display());
    last_restore().lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| !k.starts_with(&mine));
}

fn restore_slot(label: &str) -> Result<(), DataError> {
    let mut g = last_restore().lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|_, t| t.elapsed().as_secs() < RESTORE_EVERY_SECS);
    if let Some(t) = g.get(label) {
        let left = RESTORE_EVERY_SECS.saturating_sub(t.elapsed().as_secs()).max(1);
        return Err(DataError::RateLimited { retry_after_secs: left as u32 });
    }
    g.insert(label.to_string(), Instant::now());
    Ok(())
}

/// `StoreRestored`.
pub(crate) fn restore(space: &Space, c: &Caller, o: &Map<String, J>, verb: &str) -> Result<J, DataError> {
    only_known(o, &["toRev", "at", "atLocal", "snapshot", "prefixes", "except", "dryRun"], verb)?;
    spend(c, Spend::Read)?;
    let (target, set, dry) = restore_args(space, c, o, verb)?;
    let can_write = |k: &str| c.acc.can_write(k);
    if dry {
        let plan = store().restore_plan(space, &target, &set, &can_write)?;
        return Ok(json!({
            "id": J::Null,
            "rev": J::Null,
            "written": plan.written,
            "deleted": plan.deleted,
            "families": plan.families,
            "keys": plan.keys,
        }));
    }
    // Charged like a commit of its ops, before anything is written.
    let plan = store().restore_plan(space, &target, &set, &can_write)?;
    if plan.ops.is_empty() {
        return Ok(json!({ "id": J::Null, "rev": J::Null, "written": 0, "deleted": 0, "families": [], "keys": [] }));
    }
    let bytes = k2_core::garden_data::tx::ops_bytes(&plan.ops);
    spend(c, Spend::Restore { ops: plan.ops.len() })?;
    restore_slot(&slot_key(&c.label))?;
    let snapshot = super::export_routes::pre_op_snapshot(PreOp::Restore)?;
    let done = store().restore(space, &target, &set, &can_write, &c.actor)?;
    if let Some(rev) = done.rev {
        notify_changed(space.as_str(), rev);
    }
    audit::line(
        "zen.data.restore",
        &c.label,
        &format!(
            "space={space} id={} rev={} written={} deleted={} families={} snapshot={}",
            done.id.as_deref().unwrap_or("-"),
            done.rev.map_or("-".to_string(), |r| r.to_string()),
            done.plan.written,
            done.plan.deleted,
            done.plan.families.join(","),
            snapshot.as_deref().unwrap_or("-"),
        ),
    );
    audit::count(
        space.as_str(),
        &c.label,
        c.made.as_deref(),
        Counted::Write { ops: done.plan.ops.len(), bytes },
    );
    Ok(json!({
        "id": done.id,
        "rev": done.rev,
        "written": done.plan.written,
        "deleted": done.plan.deleted,
        "families": done.plan.families,
        "keys": done.plan.keys,
    }))
}

/// A snapshot label: plain text, control characters out, at most 120.
fn clean_label(s: &str) -> Option<String> {
    let t: String = s.chars().filter(|ch| !ch.is_control() && !blobs::is_format_char(*ch)).collect();
    let t = t.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.chars().take(MAX_LABEL_CHARS).collect())
    }
}

/// `StoreSnapshot`: the keys as they are now, kept as a pending blob in
/// this widget's data (restoreSkips don't apply: a snapshot keeps all it
/// names, a restore decides).
pub(crate) fn snapshot(space: &Space, c: &Caller, o: &Map<String, J>, verb: &str) -> Result<J, DataError> {
    only_known(o, &["prefixes", "except", "label"], verb)?;
    spend(c, Spend::Read)?;
    if !c.acc.can_write_somewhere() {
        return Err(DataError::NotGranted { key: None });
    }
    let set = KeySet::new(string_list(o, "prefixes", verb)?, string_list(o, "except", verb)?, Vec::new())?;
    let label: Option<String> = field(o, "label", verb)?;
    let label = label.as_deref().and_then(clean_label);
    let can_read = |k: &str| c.acc.can_read(k);
    let (bytes, rev, n) = store().snapshot_doc(space, &set, label.as_deref(), &can_read)?;
    spend(c, Spend::UploadStart)?;
    spend(c, Spend::UploadBytes { bytes: bytes.len() })?;
    let info = store().blob_put(space, &bytes, Some(&format!("snapshot-{rev}.json")), &c.actor)?;
    audit::count(space.as_str(), &c.label, c.made.as_deref(), Counted::Write { ops: 1, bytes: bytes.len() });
    Ok(json!({ "ref": info.reference, "rev": rev, "keys": n, "bytes": bytes.len() }))
}

// ── the agent and person door ─────────────────────────────────────────

/// The widget manifest's `restoreSkips` (none for a built-in or a widget
/// that isn't on this computer).
pub(crate) fn skips_of(widget: &str) -> Vec<String> {
    let f = k2_core::zen::ZenFiles::local();
    if !f.is_set_up() || widget.starts_with("k2:") {
        return Vec::new();
    }
    f.widget_status(widget).manifest.map(|m| m.restore_skips).unwrap_or_default()
}

fn agent_caller(who: &DataWho, space: &Space) -> Result<Caller, DataError> {
    let actor = who.actor().ok_or(DataError::NotGranted { key: None })?;
    let acc = access(who, space)?;
    Ok(Caller {
        label: who.audit_label(),
        actor,
        acc,
        budget: who.project_id().map(|p| BudgetKey::Agent { project_id: p.to_string(), space: space.as_str().to_string() }),
        agent: who.project_id().is_some(),
        skips: skips_of(space.widget()),
        made: None,
    })
}

#[derive(Deserialize)]
struct WidgetBody {
    widget: String,
    #[serde(flatten)]
    rest: Map<String, J>,
}

/// `/cli/zen/data/history|restore|snapshot`. The gates in
/// `zen_data_routes::handle` already ran.
pub fn agent(path: &str, who: &DataWho, body: &[u8], params: &HashMap<String, String>) -> Result<J, DataError> {
    match path {
        AGENT_HISTORY => {
            let space = data_space(param(params, "widget").unwrap_or(""))?;
            let mut o = Map::new();
            for k in ["key", "prefix", "after"] {
                if let Some(v) = param(params, k) {
                    o.insert(k.into(), J::String(v.to_string()));
                }
            }
            for k in ["before", "limit"] {
                if let Some(v) = int_param(params, k)? {
                    o.insert(k.into(), json!(v));
                }
            }
            let q = history_query(&o, "history")?;
            if let Some(k) = &q.key {
                keys::validate_key(k)?;
            }
            let c = agent_caller(who, &space)?;
            history(&space, &c, &q).inspect_err(|e| refused(who, &space, e))
        }
        AGENT_RESTORE | AGENT_SNAPSHOT => {
            let b: WidgetBody = parse(body)?;
            let space = data_space(&b.widget)?;
            let c = agent_caller(who, &space)?;
            let r = if path == AGENT_RESTORE {
                restore(&space, &c, &b.rest, "restore")
            } else {
                snapshot(&space, &c, &b.rest, "snapshot")
            };
            r.inspect_err(|e| refused(who, &space, e))
        }
        _ => Err(DataError::NotFound("route_not_found")),
    }
}

fn refused(who: &DataWho, space: &Space, e: &DataError) {
    if matches!(e, DataError::NotGranted { .. } | DataError::PersonOnly) {
        audit::line("zen.data.refused", &who.audit_label(), &format!("space={space} code={}", e.code()));
    }
}

#[cfg(test)]
#[path = "history_routes_tests.rs"]
mod tests;
