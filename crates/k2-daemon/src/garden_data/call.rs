//! `POST /cli/zen/call`: the widget door (contract §7.1; PRD §6.1–§6.3).
//! C0 → B2.
//!
//! Checks, in order: owner token only (`owner_only`); the Origin gate
//! (Tauri origins only; `Origin: null` or several Origin headers →
//! `origin_refused`; none = a local owner-token process, audited); the
//! placement → widget → live hash (`stale_widget`); the verb is a
//! `garden-local` catalog row whose cap is in the manifest's effective caps
//! after the Sources taint (`cap_not_granted`); the placement isn't paused
//! (`paused`); the maker's rights for `store.*` (`author_unknown`,
//! `not_granted`); budgets; then the store. Writes store
//! `Actor::Widget { widget }`.
//!
//! The daemon can't see which widget is calling (PRD §4.1): the app's
//! renderer asserts the placement and the hash, and the daemon checks them
//! against the Garden's live page. Every rule here is a guardrail against
//! mistakes and injected prompts, not a wall against a same-user process.

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::contract::{catalog, Effect, Exposure};
use k2_core::garden_data::backend::{Cursor, Op, StoreBackend};
use k2_core::garden_data::tx::{self, TxOwner};
use k2_core::garden_data::{
    authors, keys, rights, Access, Actor, DataError, MadeBy, Space, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT,
};
use k2_core::zen::widget_access::{effective_caps, WidgetOrigin};
use k2_core::zen::ZenFiles;

use crate::cli_response::CliResponse;
use crate::garden_data::agent_routes::{change_json, data_space, entry_json, ByNames};
use crate::garden_data::audit::{self, Counted};
use crate::garden_data::budget::{self, BudgetKey, Spend};
use crate::garden_data::tx_routes;
use crate::garden_data::{refused, response, store};
use crate::zen_data_routes::DataWho;

/// The request body (≤ 64 KiB).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallBody {
    pub garden: String,
    pub placement: String,
    /// The live bundle hash the frame runs.
    pub hash: String,
    /// A `garden-local` catalog verb (`store.put`, `save.get`, `viewer.get`, …).
    pub verb: String,
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
}

/// Origins the app's renderer calls from (`routes::http::TAURI_ORIGINS`),
/// plus the Vite dev server in debug builds.
fn origin_ok(origin: &str) -> bool {
    let o = origin.trim().to_ascii_lowercase();
    crate::routes::http::TAURI_ORIGINS.contains(&o.as_str()) || (cfg!(debug_assertions) && o == "http://localhost:5173")
}

/// Every `Origin` header value in the request head (the request line and
/// anything after the blank line are skipped).
fn origin_values(head: &str) -> Vec<&str> {
    head.lines()
        .skip(1)
        .map(|l| l.trim_end_matches('\r'))
        .take_while(|l| !l.is_empty())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case("origin").then(|| value.trim())
        })
        .collect()
}

/// How the caller reached the door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingress {
    /// The app's renderer (a Tauri origin).
    App,
    /// No Origin: a local owner-token process (a script).
    LocalProcess,
}

/// The Origin gate (PRD §6.2, GL3).
pub fn origin_gate(head: &str) -> Result<Ingress, DataError> {
    match origin_values(head).as_slice() {
        [] => Ok(Ingress::LocalProcess),
        [one] if origin_ok(one) => Ok(Ingress::App),
        [_] => Err(refused("origin_refused", 403, "this door answers only the K2 app on this computer")),
        _ => Err(refused("origin_refused", 403, "one Origin header, please")),
    }
}

/// One widget call. Answers `{"ok": true, "result": …}` or the envelope.
pub fn handle(who: &DataWho, headers: &str, body: &[u8]) -> CliResponse {
    if !who.is_owner() {
        return response(&refused(
            "owner_only",
            403,
            "only the K2 app on this computer calls widget data; agents use `k2 store --widget`",
        ));
    }
    let ingress = match origin_gate(headers) {
        Ok(i) => i,
        Err(e) => {
            audit::line("zen.data.refused", "owner", "code=origin_refused");
            return response(&e);
        }
    };
    let b: CallBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return response(&DataError::BadRequest(format!(
                "zen/call takes {{garden, placement, hash, verb, args}}: {e}"
            )))
        }
    };
    if ingress == Ingress::LocalProcess {
        audit::line("zen.data.call", "local-process", &format!("{}/{} verb={}", b.garden, b.placement, b.verb));
    }
    match run(&b) {
        Ok(v) => crate::garden_data::ok(json!({ "ok": true, "result": v })),
        Err(e) => response(&e),
    }
}

/// The placement as the daemon resolved it.
struct Placed {
    garden: String,
    placement: String,
    /// Folder name or `k2:<name>@<n>`.
    widget: String,
    hash: String,
    caps: Vec<String>,
}

impl Placed {
    /// The widget's keyspace name: a folder, or a built-in without `@<n>`.
    fn space_widget(&self) -> &str {
        self.widget.split('@').next().unwrap_or(&self.widget)
    }

    fn label(&self) -> String {
        format!("placement:{}/{}", self.garden, self.placement)
    }

    fn actor(&self) -> Actor {
        Actor::Widget { widget: self.space_widget().to_string() }
    }
}

fn stale(why: &str) -> DataError {
    refused("stale_widget", 409, format!("{why}; K2 reloads the widget"))
}

/// Placement → widget → live hash (a locked Garden's frozen copy) → caps.
fn resolve(b: &CallBody) -> Result<Placed, DataError> {
    let f = ZenFiles::local();
    if !f.is_set_up() {
        return Err(DataError::NotFound("widget_not_found"));
    }
    let (_, g) = f.garden(&b.garden).map_err(|_| DataError::NotFound("widget_not_found"))?;
    let p = f
        .custom_placements()
        .into_iter()
        .find(|p| p.garden == g.id && p.placement == b.placement)
        .ok_or(DataError::NotFound("widget_not_found"))?;
    let st = f.garden_widget_status(&g.id, &p.widget);
    let live = st.hash();
    if live.is_empty() {
        return Err(stale("this widget has no good version now"));
    }
    if live != b.hash {
        return Err(stale("the widget changed since this frame loaded"));
    }
    let caps = effective_caps(WidgetOrigin::of(&p.widget), &st.requested());
    Ok(Placed { garden: g.id, placement: b.placement.clone(), widget: p.widget, hash: live.to_string(), caps })
}

fn run(b: &CallBody) -> Result<J, DataError> {
    let c = catalog();
    let row = match c.verb(&b.verb) {
        Some(r) if r.exposed_to(Exposure::GardenLocal) => r,
        Some(_) => return Err(refused("not_exposed", 403, format!("{} isn't a widget data verb", b.verb))),
        None => return Err(refused("unknown_verb", 400, format!("no verb {}", b.verb))),
    };
    let w = resolve(b)?;
    if let Some(cap) = row.cap.as_deref() {
        if !w.caps.iter().any(|x| x == cap) {
            let why = if cap == "store:write" && w.caps.iter().any(|x| x == "sources:read") {
                format!("{} needs {cap}; a widget that reads data sources runs without it", b.verb)
            } else {
                format!("{} needs {cap}; add it to the widget's manifest caps", b.verb)
            };
            return Err(refused("cap_not_granted", 403, why));
        }
    }
    if row.effect == Effect::Write {
        if let Some(p) = crate::zen_routes::placement_pause(&w.garden, &w.placement) {
            let _ = p;
            return Err(refused("paused", 423, "this widget is paused: too many writes. The person can Resume it on the widget"));
        }
    }
    let key = BudgetKey::Placement { garden: w.garden.clone(), placement: w.placement.clone() };
    let args = &b.args;
    match b.verb.as_str() {
        "viewer.get" => {
            budget::spend(&key, Spend::Read)?;
            Ok(viewer())
        }
        v if v.starts_with("save.") => {
            let spend = if row.effect == Effect::Write { Spend::Save } else { Spend::Read };
            budget::spend(&key, spend)?;
            save_verb(&w, v, args)
        }
        v => {
            let space = data_space(&w.widget)?;
            // The maker's rights, read now (a revoke wins on the next call).
            let (maker, acc) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let maker = authors::maker_of(&conn, &w.widget, &w.hash)?;
                let acc = match rights::rights_for_widget(&conn, &space, &maker) {
                    Ok(a) => a,
                    Err(DataError::AuthorUnknown) => {
                        audit::line("zen.data.refused", &w.label(), &format!("{} code=author_unknown", space.as_str()));
                        return Err(author_unknown(&w));
                    }
                    Err(e) => return Err(e),
                };
                (maker, acc)
            };
            let made = made_by_label(&maker);
            store_verb(&w, &key, &space, &acc, &made, v, args).map_err(|e| {
                if matches!(e, DataError::NotGranted { .. } | DataError::BadTx) {
                    audit::line("zen.data.refused", &w.label(), &format!("{} code={}", space.as_str(), e.code()));
                }
                e
            })
        }
    }
}

fn author_unknown(w: &Placed) -> DataError {
    refused(
        "author_unknown",
        403,
        format!(
            "nobody signed this version of {0}. Run `k2 zen validate --widget {0}`, or click It's mine in Settings \u{2192} Gardens",
            w.widget
        ),
    )
}

// ── arguments ────────────────────────────────────────────────────────

fn arg_str<'a>(args: &'a [J], i: usize, verb: &str, what: &str) -> Result<&'a str, DataError> {
    args.get(i).and_then(J::as_str).ok_or_else(|| DataError::BadRequest(format!("{verb} needs {what}")))
}

fn arg_obj<'a>(args: &'a [J], i: usize, verb: &str) -> Result<Option<&'a serde_json::Map<String, J>>, DataError> {
    match args.get(i) {
        None | Some(J::Null) => Ok(None),
        Some(J::Object(o)) => Ok(Some(o)),
        Some(_) => Err(DataError::BadRequest(format!("{verb}: options are an object"))),
    }
}

/// An optional field of an options object, typed.
fn opt_field<T: serde::de::DeserializeOwned>(
    o: Option<&serde_json::Map<String, J>>,
    name: &str,
    verb: &str,
) -> Result<Option<T>, DataError> {
    match o.and_then(|o| o.get(name)) {
        None | Some(J::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|_| DataError::BadRequest(format!("{verb}: {name} has the wrong type"))),
    }
}

/// Only the named options; anything else is a mistake worth saying.
fn known_opts(o: Option<&serde_json::Map<String, J>>, allowed: &[&str], verb: &str) -> Result<(), DataError> {
    match o.and_then(|o| o.keys().find(|k| !allowed.contains(&k.as_str()))) {
        Some(k) => Err(DataError::BadRequest(format!("{verb}: unknown option '{k}'"))),
        None => Ok(()),
    }
}

fn if_rev(o: Option<&serde_json::Map<String, J>>, verb: &str) -> Result<Option<i64>, DataError> {
    known_opts(o, &["ifRev"], verb)?;
    let r: Option<i64> = opt_field(o, "ifRev", verb)?;
    if r.is_some_and(|r| r < 0) {
        return Err(DataError::BadRequest(format!("{verb}: ifRev is 0 or more")));
    }
    Ok(r)
}

fn ops_arg(args: &[J], i: usize, verb: &str) -> Result<Vec<Op>, DataError> {
    let v = args.get(i).cloned().ok_or_else(|| DataError::BadRequest(format!("{verb} needs ops")))?;
    serde_json::from_value(v).map_err(|e| DataError::BadRequest(format!("{verb}: ops are [{{op, key, value?, ifRev?}}]: {e}")))
}

// ── store.* ──────────────────────────────────────────────────────────

fn read_check(acc: &Access, key: &str) -> Result<(), DataError> {
    keys::validate_key(key)?;
    if acc.can_read(key) {
        Ok(())
    } else {
        Err(DataError::NotGranted { key: Some(key.to_string()) })
    }
}

fn write_check(acc: &Access, key: &str) -> Result<(), DataError> {
    keys::validate_key(key)?;
    if acc.can_write(key) {
        Ok(())
    } else {
        Err(DataError::NotGranted { key: Some(key.to_string()) })
    }
}

/// The runaway guard (PRD §6.3): the same key and value, over and over,
/// pauses the placement (`PauseReason::Writes`).
fn runaway_check(w: &Placed, ops: &[Op]) -> Result<(), DataError> {
    for op in ops {
        let value = match op {
            Op::Put { value, .. } => serde_json::to_string(value).unwrap_or_default(),
            Op::Delete { .. } => String::new(),
        };
        let hash = k2_core::zen::bundle::sha256_hex(value.as_bytes());
        if budget::runaway(&w.garden, &w.placement, op.key(), &hash) {
            crate::zen_routes::pause_for_writes(&w.garden, &w.placement);
            return Err(refused("paused", 423, "this widget is paused: too many identical writes. The person can Resume it on the widget"));
        }
    }
    Ok(())
}

fn store_verb(
    w: &Placed,
    key: &BudgetKey,
    space: &Space,
    acc: &Access,
    made: &str,
    verb: &str,
    args: &[J],
) -> Result<J, DataError> {
    let s = store();
    let label = w.label();
    let mut names = ByNames::new();
    match verb {
        "store.get" => {
            budget::spend(key, Spend::Read)?;
            let k = arg_str(args, 0, verb, "a key")?;
            read_check(acc, k)?;
            let e = s.get(space, k)?;
            audit::count(space.as_str(), &label, Some(made), Counted::Read);
            Ok(e.map(|e| entry_json(e, &mut names)).unwrap_or(J::Null))
        }
        "store.put" => {
            budget::spend(key, Spend::Write)?;
            let k = arg_str(args, 0, verb, "a key")?;
            let value = args.get(1).cloned().ok_or_else(|| DataError::BadRequest(format!("{verb} needs a value")))?;
            let rev_if = if_rev(arg_obj(args, 2, verb)?, verb)?;
            write_check(acc, k)?;
            let op = Op::Put { key: k.to_string(), value, if_rev: rev_if };
            runaway_check(w, std::slice::from_ref(&op))?;
            let Op::Put { value, .. } = &op else { unreachable!("built as a put") };
            match s.put(space, k, value, rev_if, &w.actor()) {
                Ok(rev) => {
                    crate::garden_data::notify_changed(space.as_str(), rev);
                    audit::count(space.as_str(), &label, Some(made), Counted::Write { ops: 1, bytes: op.value_bytes() });
                    Ok(json!({ "rev": rev }))
                }
                Err(e) => Err(conflict_counted(space, &label, made, e)),
            }
        }
        "store.delete" => {
            budget::spend(key, Spend::Write)?;
            let k = arg_str(args, 0, verb, "a key")?;
            let rev_if = if_rev(arg_obj(args, 1, verb)?, verb)?;
            write_check(acc, k)?;
            match s.delete(space, k, rev_if, &w.actor()) {
                Ok(Some(rev)) => {
                    crate::garden_data::notify_changed(space.as_str(), rev);
                    audit::count(space.as_str(), &label, Some(made), Counted::Delete);
                    Ok(json!({ "rev": rev, "deleted": true }))
                }
                Ok(None) => Ok(json!({ "rev": 0, "deleted": false })),
                Err(e) => Err(conflict_counted(space, &label, made, e)),
            }
        }
        "store.list" => {
            budget::spend(key, Spend::Read)?;
            let o = arg_obj(args, 0, verb)?;
            known_opts(o, &["prefix", "after", "limit"], verb)?;
            let prefix: String = opt_field(o, "prefix", verb)?.unwrap_or_default();
            let after: Option<String> = opt_field(o, "after", verb)?;
            let limit: usize = opt_field::<u64>(o, "limit", verb)?.map_or(DEFAULT_LIST_LIMIT, |l| l as usize);
            if limit == 0 || limit > MAX_LIST_LIMIT {
                return Err(DataError::BadRequest(format!("{verb}: limit is 1..={MAX_LIST_LIMIT}")));
            }
            keys::validate_prefix(&prefix)?;
            let filter = acc.read_prefixes();
            let page = s.list(space, &prefix, after.as_deref(), limit, filter.as_deref())?;
            audit::count(space.as_str(), &label, Some(made), Counted::Read);
            let items: Vec<J> = page.items.into_iter().map(|e| entry_json(e, &mut names)).collect();
            Ok(json!({ "items": items, "next": page.next }))
        }
        "store.changes" => {
            budget::spend(key, Spend::Read)?;
            let o = arg_obj(args, 0, verb)?;
            known_opts(o, &["since", "after", "prefix"], verb)?;
            let since: i64 = opt_field(o, "since", verb)?
                .ok_or_else(|| DataError::BadRequest(format!("{verb} needs {{since}}")))?;
            if since < 0 {
                return Err(DataError::BadRequest(format!("{verb}: since is 0 or more")));
            }
            let after: Option<String> = opt_field(o, "after", verb)?;
            let prefix: String = opt_field(o, "prefix", verb)?.unwrap_or_default();
            keys::validate_prefix(&prefix)?;
            let filter = acc.read_prefixes();
            let page = s.changes(space, &Cursor { since, after }, &prefix, filter.as_deref())?;
            audit::count(space.as_str(), &label, Some(made), Counted::Read);
            let items: Vec<J> = page.items.into_iter().map(|c| change_json(c, &mut names)).collect();
            let next = page.next.map(|n| match n.after {
                Some(a) => json!({ "since": n.since, "after": a }),
                None => json!({ "since": n.since }),
            });
            Ok(json!({ "head": page.head, "items": items, "next": next, "partial": page.partial }))
        }
        "store.batch" => {
            budget::spend(key, Spend::Write)?;
            let ops = ops_arg(args, 0, verb)?;
            tx::check_part(&ops, acc)?;
            runaway_check(w, &ops)?;
            let rev = tx_routes::apply(space, &ops, &w.actor(), &label, Some(made))?;
            Ok(json!({ "rev": rev, "ops": ops.len() }))
        }
        "store.txBegin" => {
            budget::spend(key, Spend::Read)?;
            if !acc.can_write_somewhere() {
                return Err(DataError::NotGranted { key: None });
            }
            let (tx_id, expires_at) = tx_routes::table().begin(tx_owner(w), space.clone(), tx_routes::now())?;
            Ok(json!({ "txId": tx_id, "expiresAt": expires_at }))
        }
        "store.txAdd" => {
            budget::spend(key, Spend::Read)?;
            let tx_id = arg_str(args, 0, verb, "a txId")?;
            let ops = ops_arg(args, 1, verb)?;
            let owner = tx_owner(w);
            tx_routes::table().space_of(tx_id, &owner, tx_routes::now())?;
            if let Err(e) = tx::check_part(&ops, acc) {
                tx_routes::table().abort(tx_id, &owner);
                return Err(e);
            }
            let st = tx_routes::table().add(tx_id, &owner, ops, tx_routes::now())?;
            Ok(json!({ "ops": st.ops, "bytes": st.bytes, "expiresAt": st.expires_at }))
        }
        "store.txCommit" => {
            let tx_id = arg_str(args, 0, verb, "a txId")?;
            let (tx_space, ops) = tx_routes::table().take(tx_id, &tx_owner(w), tx_routes::now())?;
            if &tx_space != space {
                return Err(DataError::BadTx);
            }
            // `acc` was read for this call: the maker's grants now.
            tx::check_writable(&ops, acc)?;
            budget::spend(key, Spend::Commit { ops: ops.len(), bytes: tx::ops_bytes(&ops) })?;
            runaway_check(w, &ops)?;
            let rev = tx_routes::apply(space, &ops, &w.actor(), &label, Some(made))?;
            Ok(json!({ "rev": rev, "ops": ops.len() }))
        }
        "store.txAbort" => {
            let tx_id = arg_str(args, 0, verb, "a txId")?;
            Ok(json!({ "aborted": tx_routes::table().abort(tx_id, &tx_owner(w)) }))
        }
        _ => Err(refused("unknown_verb", 400, format!("no verb {verb}"))),
    }
}

fn tx_owner(w: &Placed) -> TxOwner {
    TxOwner::Placement { garden: w.garden.clone(), placement: w.placement.clone(), hash: w.hash.clone() }
}

fn conflict_counted(space: &Space, label: &str, made: &str, e: DataError) -> DataError {
    if matches!(e, DataError::Conflict { .. }) {
        audit::count(space.as_str(), label, Some(made), Counted::Conflict);
    }
    e
}

// ── save.* (private to the widget; never granted, never pushed) ──────

fn save_verb(w: &Placed, verb: &str, args: &[J]) -> Result<J, DataError> {
    let space = Space::save(w.space_widget())?;
    let s = store();
    match verb {
        "save.get" => {
            let k = arg_str(args, 0, verb, "a key")?;
            keys::validate_key(k)?;
            Ok(s.get(&space, k)?.map(|e| e.value).unwrap_or(J::Null))
        }
        "save.set" => {
            let k = arg_str(args, 0, verb, "a key")?;
            keys::validate_key(k)?;
            let value = args.get(1).ok_or_else(|| DataError::BadRequest(format!("{verb} needs a value")))?;
            s.put(&space, k, value, None, &w.actor())?;
            Ok(J::Null)
        }
        "save.remove" => {
            let k = arg_str(args, 0, verb, "a key")?;
            keys::validate_key(k)?;
            s.delete(&space, k, None, &w.actor())?;
            Ok(J::Null)
        }
        "save.keys" => {
            let o = arg_obj(args, 0, verb)?;
            known_opts(o, &["prefix"], verb)?;
            let prefix: String = opt_field(o, "prefix", verb)?.unwrap_or_default();
            keys::validate_prefix(&prefix)?;
            let mut out = Vec::new();
            let mut after: Option<String> = None;
            loop {
                let page = s.list(&space, &prefix, after.as_deref(), MAX_LIST_LIMIT, None)?;
                out.extend(page.items.into_iter().map(|e| J::String(e.key)));
                match page.next {
                    Some(n) => after = Some(n),
                    None => break,
                }
            }
            Ok(J::Array(out))
        }
        _ => Err(refused("unknown_verb", 400, format!("no verb {verb}"))),
    }
}

// ── viewer.get, wire names ───────────────────────────────────────────

/// `{kind: "owner", name}`: Settings' name for the person, else "You".
/// Read per call (contract §7.5).
pub fn viewer() -> J {
    let name = k2_core::app_settings::load()
        .owner_display_name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "You".to_string());
    json!({ "kind": "owner", "name": name })
}

/// The wire form of a maker (sign/claim responses, `spaces`):
/// `{kind: "owner"|"k2"|"unclaimed"}` or `{kind: "agent", handle, projectId}`.
pub fn made_by_wire(m: &MadeBy) -> J {
    match m {
        MadeBy::Owner => json!({ "kind": "owner" }),
        MadeBy::K2 => json!({ "kind": "k2" }),
        MadeBy::Unclaimed => json!({ "kind": "unclaimed" }),
        MadeBy::Agent { project_id } => {
            json!({ "kind": "agent", "handle": crate::compute_routes::ws_handle(project_id), "projectId": project_id })
        }
    }
}

/// `owner` | `k2` | `unclaimed` | `agent:<handle>` for the counted audit.
fn made_by_label(m: &MadeBy) -> String {
    match m {
        MadeBy::Owner => "owner".into(),
        MadeBy::K2 => "k2".into(),
        MadeBy::Unclaimed => "unclaimed".into(),
        MadeBy::Agent { project_id } => format!("agent:{}", crate::compute_routes::ws_handle(project_id)),
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod door_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn head(origins: &[&str]) -> String {
        let mut s = String::from("POST /cli/zen/call HTTP/1.1\r\nHost: 127.0.0.1:38471\r\n");
        for o in origins {
            s.push_str(&format!("Origin: {o}\r\n"));
        }
        s.push_str("Content-Type: application/json\r\n\r\n{\"origin\": \"x\"}\r\nOrigin: tauri://localhost");
        s
    }

    #[test]
    fn the_origin_gate_takes_only_the_app() {
        assert_eq!(origin_gate(&head(&["tauri://localhost"])), Ok(Ingress::App));
        assert_eq!(origin_gate(&head(&["http://tauri.localhost"])), Ok(Ingress::App));
        assert_eq!(origin_gate(&head(&["https://tauri.localhost"])), Ok(Ingress::App));
        assert_eq!(origin_gate(&head(&[])), Ok(Ingress::LocalProcess), "no Origin: a local process (the body never counts)");
        let code = |h: &str| origin_gate(h).unwrap_err().code();
        assert_eq!(code(&head(&["null"])), "origin_refused");
        assert_eq!(code(&head(&["https://evil.k2.dev"])), "origin_refused");
        assert_eq!(code(&head(&["tauri://localhost", "tauri://localhost"])), "origin_refused", "several Origin headers");
        assert_eq!(code(&head(&["http://localhost:1234"])), "origin_refused");
        assert_eq!(origin_gate(&head(&["http://localhost:5173"])).is_ok(), cfg!(debug_assertions));
    }

    #[test]
    fn only_the_owner_token_reaches_the_door() {
        let code = |r: CliResponse| -> String {
            let v: J = serde_json::from_str(&r.body).expect("json");
            v["error"]["code"].as_str().unwrap_or("").to_string()
        };
        let body = br#"{"garden":"g","placement":"p","hash":"h","verb":"viewer.get"}"#;
        for who in [
            DataWho::Agent { project_id: "w".into() },
            DataWho::Shell { project_id: "w".into() },
            DataWho::Login,
            DataWho::Wrong,
            DataWho::Invalid,
        ] {
            assert_eq!(code(handle(&who, &head(&["tauri://localhost"]), body)), "owner_only", "{who:?}");
        }
        assert_eq!(code(handle(&DataWho::Owner, &head(&["null"]), body)), "origin_refused");
        assert_eq!(code(handle(&DataWho::Owner, &head(&["tauri://localhost"]), b"{\"x\":1}")), "bad_request");
    }

    #[test]
    fn options_are_typed_and_closed() {
        let o = json!({"ifRev": 3});
        assert_eq!(if_rev(o.as_object(), "store.put").expect("ok"), Some(3));
        let bad = json!({"ifrev": 3});
        assert_eq!(if_rev(bad.as_object(), "store.put").unwrap_err().code(), "bad_request");
        let neg = json!({"ifRev": -1});
        assert_eq!(if_rev(neg.as_object(), "store.put").unwrap_err().code(), "bad_request");
        assert_eq!(if_rev(None, "store.put").expect("none"), None);
        assert_eq!(arg_obj(&[json!(1)], 0, "store.list").unwrap_err().code(), "bad_request");
    }

    #[test]
    fn makers_have_a_wire_form() {
        assert_eq!(made_by_wire(&MadeBy::Owner), json!({"kind": "owner"}));
        assert_eq!(made_by_wire(&MadeBy::K2), json!({"kind": "k2"}));
        assert_eq!(made_by_wire(&MadeBy::Unclaimed), json!({"kind": "unclaimed"}));
    }
}
