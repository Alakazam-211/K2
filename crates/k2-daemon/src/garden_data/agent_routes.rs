//! Garden data routes: get, put, delete, list, changes, batch, grants
//! (contract §4.3, §4.5, §5). Owner: B1.
//!
//! The gates in `zen_data_routes::handle` already ran (credential kind,
//! person-only rows, POST-only, body cap). Here: the space, the caller's
//! access (the owner token is full; an agent or K2 shell tab gets its
//! effective grants, re-read on every call), budgets, the store, the push,
//! and the counted audit. Reads of a missing space answer empty, never 404.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::garden_data::backend::{Change, Cursor, Entry, Op};
use k2_core::garden_data::grants::{self, Access};
use k2_core::garden_data::{
    keys, DataError, Space, StoreBackend, DEFAULT_LIST_LIMIT, MAX_BATCH_BYTES, MAX_BATCH_OPS, MAX_LIST_LIMIT,
    MAX_WAIT_SECS,
};

use super::audit::{self, Counted};
use super::budget::{self, BudgetKey, Spend};
use super::{generation, notify_changed, store, wait_for_change};
use crate::zen_data_routes::DataWho;

// ── shared helpers (grant and export routes use these too) ───────────

/// A JSON body, strictly (`deny_unknown_fields` on the target type).
pub(crate) fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, DataError> {
    serde_json::from_slice(body).map_err(|e| DataError::BadRequest(format!("bad request body: {e}")))
}

/// A widget's data space. A built-in's `@version` is dropped (versions
/// share data).
pub(crate) fn data_space(widget: &str) -> Result<Space, DataError> {
    let w = widget.trim();
    if w.is_empty() {
        return Err(DataError::BadRequest("widget is required".into()));
    }
    let w = if w.starts_with("k2:") { w.split('@').next().unwrap_or(w) } else { w };
    Space::data(w)
}

pub(crate) fn param<'a>(params: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    params.get(k).map(String::as_str).map(str::trim).filter(|s| !s.is_empty())
}

pub(crate) fn int_param(params: &HashMap<String, String>, k: &str) -> Result<Option<i64>, DataError> {
    param(params, k)
        .map(|v| v.parse::<i64>().map_err(|_| DataError::BadRequest(format!("{k} is a whole number"))))
        .transpose()
}

/// The caller's access to `space`, read now. An agent with the
/// `gardens_manage` switch that made the widget's newest version reaches its
/// own widget's data in full (0.46.1, gap 7; `rights::maker_owns_widget_data`),
/// with one Access Audit line per agent and space per daemon run.
pub(crate) fn access(who: &DataWho, space: &Space) -> Result<Access, DataError> {
    match who.project_id() {
        None if who.is_owner() => Ok(Access::Full),
        None => Err(DataError::NotGranted { key: None }),
        Some(p) if k2_core::zen::garden_access::enabled() => {
            // Garden access (G15, §1.7): listed on every Garden that places
            // the widget → its data in full; else the grants as today. The
            // maker's own reach (`maker_owns_widget_data`) is retired.
            let acc = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                grants::access_for_agent(&conn, space, p)?
            };
            if acc != Access::Full && crate::zen_access_routes::agent_edits_widget_data(space, p)? {
                audit_listed_access(p, space);
                return Ok(Access::Full);
            }
            Ok(acc)
        }
        Some(p) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let acc = grants::access_for_agent(&conn, space, p)?;
            if acc != Access::Full && k2_core::garden_data::rights::maker_owns_widget_data(&conn, space, p)? {
                drop(conn);
                audit_maker_access(p, space);
                return Ok(Access::Full);
            }
            Ok(acc)
        }
    }
}

/// One Access Audit line the first time (per daemon run) an agent reaches
/// its own widget's data under the `gardens_manage` switch.
fn audit_maker_access(project_id: &str, space: &Space) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<(String, String)>>> = Mutex::new(None);
    let first = {
        let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashSet::new).insert((project_id.to_string(), space.as_str().to_string()))
    };
    if first {
        let user = match crate::compute_routes::ws_by_id(project_id) {
            Some(ws) => format!("agent:{}", ws.handle),
            None => format!("agent:{project_id}"),
        };
        crate::gardens_manage::audit("zen-data-maker", &user, space.as_str(), None, None);
    }
}

/// One Access Audit line the first time (per daemon run) an agent reaches a
/// widget's data as an editor of every Garden that places it (garden access).
fn audit_listed_access(project_id: &str, space: &Space) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<(String, String)>>> = Mutex::new(None);
    let first = {
        let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashSet::new).insert((project_id.to_string(), space.as_str().to_string()))
    };
    if first {
        let user = match crate::compute_routes::ws_by_id(project_id) {
            Some(ws) => format!("agent:{}", ws.handle),
            None => format!("agent:{project_id}"),
        };
        crate::gardens_manage::audit("zen-data-editor", &user, space.as_str(), None, None);
    }
}

/// The stored writer → the wire `by` (contract §5.3).
pub(crate) struct ByNames(HashMap<String, String>);

impl ByNames {
    pub(crate) fn new() -> ByNames {
        ByNames(HashMap::new())
    }

    pub(crate) fn wire(&mut self, stored: &str) -> String {
        if let Some(w) = self.0.get(stored) {
            return w.clone();
        }
        let w = match stored.split_once(':') {
            None if stored == "owner" => "you".to_string(),
            Some(("agent", id)) => match crate::compute_routes::ws_by_id(id) {
                Some(ws) => format!("agent:{}", ws.handle),
                None => "agent:gone".to_string(),
            },
            Some(("widget", w)) => format!("you via {w}"),
            _ => stored.to_string(),
        };
        self.0.insert(stored.to_string(), w.clone());
        w
    }
}

pub(crate) fn entry_json(e: Entry, names: &mut ByNames) -> Value {
    json!({ "key": e.key, "rev": e.rev, "value": e.value, "updatedAt": e.updated_at, "by": names.wire(&e.by) })
}

pub(crate) fn change_json(c: Change, names: &mut ByNames) -> Value {
    json!({ "key": c.key, "rev": c.rev, "deleted": c.deleted, "value": c.value, "by": names.wire(&c.by), "at": c.at })
}

fn need_write(acc: &Access, key: &str) -> Result<(), DataError> {
    if acc.can_write(key) {
        Ok(())
    } else {
        Err(DataError::NotGranted { key: Some(key.to_string()) })
    }
}

fn spend(who: &DataWho, space: &Space, what: Spend) -> Result<(), DataError> {
    match who.project_id() {
        Some(p) => budget::spend(&BudgetKey::Agent { project_id: p.to_string(), space: space.as_str().to_string() }, what),
        None => Ok(()),
    }
}

/// Count a refusal or a conflict, then pass it on.
fn counted<T>(space: &Space, who: &DataWho, r: Result<T, DataError>) -> Result<T, DataError> {
    if let Err(e) = &r {
        match e {
            DataError::Conflict { .. } => audit::count(space.as_str(), &who.audit_label(), None, Counted::Conflict),
            DataError::NotGranted { .. } => {
                audit::line("zen.data.refused", &who.audit_label(), &format!("space={space} code=not_granted"))
            }
            _ => {}
        }
    }
    r
}

// ── bodies ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutBody {
    widget: String,
    key: String,
    value: Value,
    #[serde(default, rename = "ifRev")]
    if_rev: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteBody {
    widget: String,
    key: String,
    #[serde(default, rename = "ifRev")]
    if_rev: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchBody {
    widget: String,
    ops: Vec<Op>,
}

// ── handlers ──────────────────────────────────────────────────────────

pub fn handle(
    path: &str,
    who: &DataWho,
    body: &[u8],
    params: &HashMap<String, String>,
) -> Result<Value, DataError> {
    match path {
        "/cli/zen/data/get" => get(who, params),
        "/cli/zen/data/put" => put(who, body),
        "/cli/zen/data/delete" => delete(who, body),
        "/cli/zen/data/list" => list(who, params),
        "/cli/zen/data/changes" => changes(who, params),
        "/cli/zen/data/batch" => batch(who, body),
        "/cli/zen/data/grants" => my_grants(who, params),
        _ => Err(DataError::NotFound("route_not_found")),
    }
}

fn get(who: &DataWho, params: &HashMap<String, String>) -> Result<Value, DataError> {
    let space = data_space(param(params, "widget").unwrap_or(""))?;
    let key = param(params, "key").ok_or_else(|| DataError::BadRequest("key is required".into()))?;
    keys::validate_key(key)?;
    let acc = access(who, &space)?;
    if !acc.can_read(key) {
        return counted(&space, who, Err(DataError::NotGranted { key: Some(key.to_string()) }));
    }
    let e = store().get(&space, key)?;
    audit::count(space.as_str(), &who.audit_label(), None, Counted::Read);
    let mut names = ByNames::new();
    Ok(json!({ "entry": e.map(|e| entry_json(e, &mut names)) }))
}

fn put(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: PutBody = parse(body)?;
    let space = data_space(&b.widget)?;
    keys::validate_key(&b.key)?;
    let acc = access(who, &space)?;
    counted(&space, who, need_write(&acc, &b.key))?;
    spend(who, &space, Spend::Write)?;
    let actor = who.actor().ok_or(DataError::NotGranted { key: None })?;
    let bytes = serde_json::to_string(&b.value).map(|s| s.len()).unwrap_or(0);
    let rev = counted(&space, who, store().put(&space, &b.key, &b.value, b.if_rev, &actor))?;
    notify_changed(space.as_str(), rev);
    audit::count(space.as_str(), &who.audit_label(), None, Counted::Write { ops: 1, bytes });
    Ok(json!({ "rev": rev }))
}

fn delete(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: DeleteBody = parse(body)?;
    let space = data_space(&b.widget)?;
    keys::validate_key(&b.key)?;
    let acc = access(who, &space)?;
    counted(&space, who, need_write(&acc, &b.key))?;
    spend(who, &space, Spend::Write)?;
    let actor = who.actor().ok_or(DataError::NotGranted { key: None })?;
    match counted(&space, who, store().delete(&space, &b.key, b.if_rev, &actor))? {
        Some(rev) => {
            notify_changed(space.as_str(), rev);
            audit::count(space.as_str(), &who.audit_label(), None, Counted::Delete);
            Ok(json!({ "rev": rev, "deleted": true }))
        }
        None => Ok(json!({ "rev": 0, "deleted": false })),
    }
}

fn list(who: &DataWho, params: &HashMap<String, String>) -> Result<Value, DataError> {
    let space = data_space(param(params, "widget").unwrap_or(""))?;
    let prefix = param(params, "prefix").unwrap_or("");
    let limit = match int_param(params, "limit")? {
        None => DEFAULT_LIST_LIMIT,
        Some(n) if n >= 1 && n as usize <= MAX_LIST_LIMIT => n as usize,
        Some(_) => return Err(DataError::BadRequest(format!("limit is 1–{MAX_LIST_LIMIT}"))),
    };
    let acc = access(who, &space)?;
    if who.project_id().is_some() && acc == Access::None {
        return counted(&space, who, Err(DataError::NotGranted { key: None }));
    }
    let filter = acc.read_prefixes();
    let page = store().list(&space, prefix, param(params, "after"), limit, filter.as_deref())?;
    audit::count(space.as_str(), &who.audit_label(), None, Counted::Read);
    let mut names = ByNames::new();
    let items: Vec<Value> = page.items.into_iter().map(|e| entry_json(e, &mut names)).collect();
    Ok(json!({ "items": items, "next": page.next }))
}

fn changes(who: &DataWho, params: &HashMap<String, String>) -> Result<Value, DataError> {
    let space = data_space(param(params, "widget").unwrap_or(""))?;
    let prefix = param(params, "prefix").unwrap_or("");
    let since = int_param(params, "since")?.unwrap_or(0);
    if since < 0 {
        return Err(DataError::BadRequest("since is 0 or more".into()));
    }
    let wait = match int_param(params, "wait")? {
        None => 0,
        Some(w) if (0..=i64::from(MAX_WAIT_SECS)).contains(&w) => w as u32,
        Some(_) => return Err(DataError::BadRequest(format!("wait is 0–{MAX_WAIT_SECS} seconds"))),
    };
    let from = Cursor { since, after: param(params, "after").map(str::to_string) };
    let seen = generation(space.as_str());
    let mut acc = access(who, &space)?;
    let mut page = store().changes(&space, &from, prefix, acc.read_prefixes().as_deref())?;
    if page.items.is_empty() && wait > 0 && wait_for_change(space.as_str(), seen, wait) {
        // Grants may have changed while we waited: check again first.
        acc = access(who, &space)?;
        page = store().changes(&space, &from, prefix, acc.read_prefixes().as_deref())?;
    }
    if who.project_id().is_some() && acc == Access::None {
        return counted(&space, who, Err(DataError::NotGranted { key: None }));
    }
    audit::count(space.as_str(), &who.audit_label(), None, Counted::Read);
    let mut names = ByNames::new();
    let items: Vec<Value> = page.items.into_iter().map(|c| change_json(c, &mut names)).collect();
    Ok(json!({ "head": page.head, "items": items, "next": page.next, "partial": page.partial }))
}

fn batch(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: BatchBody = parse(body)?;
    let space = data_space(&b.widget)?;
    if b.ops.is_empty() || b.ops.len() > MAX_BATCH_OPS {
        return Err(DataError::TooLarge(format!(
            "a batch is 1–{MAX_BATCH_OPS} ops; use a transaction (k2 store batch does) for more"
        )));
    }
    let bytes: usize = b.ops.iter().map(Op::value_bytes).sum();
    if bytes > MAX_BATCH_BYTES {
        return Err(DataError::TooLarge(format!("a batch carries at most {} KiB of values", MAX_BATCH_BYTES / 1024)));
    }
    let acc = access(who, &space)?;
    for op in &b.ops {
        keys::validate_key(op.key())?;
        counted(&space, who, need_write(&acc, op.key()))?;
    }
    spend(who, &space, Spend::Commit { ops: b.ops.len(), bytes })?;
    let actor = who.actor().ok_or(DataError::NotGranted { key: None })?;
    let rev = counted(&space, who, store().apply(&space, &b.ops, &actor))?;
    notify_changed(space.as_str(), rev);
    audit::count(space.as_str(), &who.audit_label(), None, Counted::Write { ops: b.ops.len(), bytes });
    Ok(json!({ "rev": rev, "ops": b.ops.len() }))
}

/// "What can I access?" (BG9): an agent's effective grants plus the grants
/// it made under its delegations; the owner sees every row.
fn my_grants(who: &DataWho, params: &HashMap<String, String>) -> Result<Value, DataError> {
    let space = data_space(param(params, "widget").unwrap_or(""))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    match who.project_id() {
        None => {
            let rows = grants::list_space(&conn, &space)?;
            Ok(json!({ "space": space.as_str(), "grants": rows, "made": [] }))
        }
        Some(p) => {
            let eff = grants::effective_grants(&conn, &space, p)?;
            let mut made = Vec::new();
            for d in grants::delegations_of(&conn, &space, p)? {
                made.extend(grants::list_made_under(&conn, d.id)?);
            }
            Ok(json!({ "space": space.as_str(), "grants": eff, "made": made }))
        }
    }
}

#[cfg(test)]
#[path = "agent_routes_tests.rs"]
pub(crate) mod tests;
