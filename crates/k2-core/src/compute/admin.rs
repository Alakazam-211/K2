//! Node admin, controller side (`prd-k2-node-admin-v1` §7, §10, §14).
//!
//! **Agents propose, owners approve, the node checks.** This module holds
//! the SQLite and pure parts (migration 0156): held admin requests, the
//! controller's argument checks and plain-words summaries, the latest
//! health facts, the alert rules and the operate switch. The routes, the
//! owner confirm, the approval key (a vault engine key) and the live
//! sends are `k2-daemon`'s `compute_admin`.
//!
//! The wire (signed command, plans, verb catalog, health facts) is
//! `k2_node_proto::admin` / `::health`, owned by the node side; this
//! module only stores and checks what the controller holds.
//!
//! A request is decided exactly once: [`decide`] is a single
//! `UPDATE … WHERE state = 'pending' AND expires_at > now`, so a second
//! approve (a replay) or an approve after expiry finds nothing to update.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::proto::admin::{self as wire, Class, SignedCommand};
use super::proto::crypto;
use super::proto::health::Health;

/// Pending requests one proposer (workspace, or human) may hold (R2).
pub const MAX_PENDING_PER_PROPOSER: i64 = 10;
/// One alert per kind per node per 6 h (§10.2).
pub const ALERT_REPEAT_SECS: i64 = 6 * 3600;
/// Offline alert defaults (§10.2): 10 min, at least 2.
pub const DEFAULT_ALERT_AFTER_SECS: i64 = 600;
pub const MIN_ALERT_AFTER_SECS: i64 = 120;
/// Pending request lifetime (Q15: 24 h).
pub const REQUEST_TTL_SECS: i64 = 24 * 3600;
/// Plans: at most this many steps (the node's cap).
pub const MAX_PLAN_STEPS: usize = wire::MAX_PLAN_STEPS;
/// Security updates pending this long raise an alert (§10.2).
pub const SECURITY_UPDATES_AFTER_SECS: i64 = 7 * 86400;
/// The vault engine key that signs owner approvals (§7.4).
pub const APPROVAL_ENGINE: &str = "compute";
pub const APPROVAL_KEY_NAME: &str = "node-admin";
/// The vault confirm action an approval consumes (§7.4).
pub const CONFIRM_ACTION: &str = "node-admin";

/// Stable error codes (exit 3 with teaching text, or exit 2 for held).
pub mod codes {
    pub const OPERATE_OFF: &str = "operate_off";
    pub const APPROVAL_REQUIRED: &str = "approval_required";
    pub const VAULT_CONFIRM_REQUIRED: &str = "vault_confirm_required";
    pub const REQUEST_NOT_FOUND: &str = "request_not_found";
    pub const NOT_PENDING: &str = "not_pending";
    pub const APPROVAL_EXPIRED: &str = "approval_expired";
    pub const TOO_MANY_PENDING: &str = "too_many_pending";
    pub const VERB_NOT_ALLOWED: &str = "verb_not_allowed";
    pub const BAD_ARGS: &str = "bad_args";
    pub const NODE_LACKS_ADMIN: &str = "node_lacks_admin";
    pub const REMOTE_ADMIN_OFF: &str = "remote_admin_off";
    pub const NO_UPDATE: &str = "no_update";
    pub const VAULT_CLOSED: &str = "vault_closed";
}

// ── requests ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRow {
    pub id: String,
    pub node_id: String,
    pub kind: String,
    pub class: String,
    pub body: Value,
    pub digest: String,
    pub summary: String,
    pub reason: Option<String>,
    pub proposed_by: String,
    pub proposer_workspace: Option<String>,
    #[serde(skip)]
    pub proposer_session: Option<String>,
    pub proposed_at: i64,
    pub state: String,
    pub decided_by: Option<String>,
    pub decided_via: Option<String>,
    pub decided_at: Option<i64>,
    pub deny_reason: Option<String>,
    pub expires_at: i64,
    pub cmd_ids: Vec<String>,
    #[serde(skip)]
    pub commands: Vec<SignedCommand>,
    pub next_step: i64,
    pub result: Option<Value>,
}

impl RequestRow {
    /// Steps of this request (`command`/`service`: one; `plan`: many).
    pub fn steps(&self) -> Result<Vec<Step>, String> {
        steps_of(&self.kind, &self.body)
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state.as_str(), "pending" | "approved" | "sent")
    }
}

const REQ_COLS: &str = "id, node_id, kind, class, body_json, digest, summary, reason, proposed_by, proposer_workspace, \
    proposer_session, proposed_at, state, decided_by, decided_via, decided_at, deny_reason, expires_at, cmd_ids_json, \
    commands_json, next_step, result_json";

fn req_from_row(r: &Row<'_>) -> rusqlite::Result<RequestRow> {
    let body: String = r.get(4)?;
    let cmd_ids: String = r.get(18)?;
    let commands: Option<String> = r.get(19)?;
    let result: Option<String> = r.get(21)?;
    Ok(RequestRow {
        id: r.get(0)?,
        node_id: r.get(1)?,
        kind: r.get(2)?,
        class: r.get(3)?,
        body: serde_json::from_str(&body).unwrap_or(Value::Null),
        digest: r.get(5)?,
        summary: r.get(6)?,
        reason: r.get(7)?,
        proposed_by: r.get(8)?,
        proposer_workspace: r.get(9)?,
        proposer_session: r.get(10)?,
        proposed_at: r.get(11)?,
        state: r.get(12)?,
        decided_by: r.get(13)?,
        decided_via: r.get(14)?,
        decided_at: r.get(15)?,
        deny_reason: r.get(16)?,
        expires_at: r.get(17)?,
        cmd_ids: serde_json::from_str(&cmd_ids).unwrap_or_default(),
        commands: commands.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default(),
        next_step: r.get(20)?,
        result: result.and_then(|x| serde_json::from_str(&x).ok()),
    })
}

/// One step: a verb and its args.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    pub verb: String,
    pub args: Value,
}

/// Body shapes: `command`/`service` = `{verb, args}`; `plan` =
/// `{steps: [{verb, args}, …]}`.
pub fn steps_of(kind: &str, body: &Value) -> Result<Vec<Step>, String> {
    let one = |v: &Value| -> Result<Step, String> {
        let verb = v.get("verb").and_then(Value::as_str).ok_or("each step needs a verb")?;
        let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return Err(format!("{verb}: args must be an object"));
        }
        Ok(Step { verb: verb.to_string(), args })
    };
    match kind {
        "command" | "service" => Ok(vec![one(body)?]),
        "plan" => {
            let steps = body.get("steps").and_then(Value::as_array).ok_or("a plan needs steps")?;
            if steps.is_empty() || steps.len() > MAX_PLAN_STEPS {
                return Err(format!("a plan has 1–{MAX_PLAN_STEPS} steps"));
            }
            steps.iter().map(one).collect()
        }
        k => Err(format!("unknown request kind {k:?}")),
    }
}

pub struct NewRequest<'a> {
    pub node_id: &'a str,
    pub kind: &'a str,
    pub class: Class,
    pub body: &'a Value,
    pub summary: &'a str,
    pub reason: Option<&'a str>,
    pub proposed_by: &'a str,
    pub proposer_workspace: Option<&'a str>,
    pub proposer_session: Option<&'a str>,
    pub now: i64,
}

/// `sha256:<hex>` of the canonical JSON of what the owner approves.
pub fn request_digest(body: &Value) -> Result<String, String> {
    Ok(format!("sha256:{}", crypto::sha256_hex(&super::proto::canonical::to_vec(body)?)))
}

/// `car_<32 hex>`.
pub fn new_request_id() -> Result<String, String> {
    Ok(format!("car_{}", crypto::random_id()?))
}

/// Pending requests one proposer holds (its workspace, else its label).
pub fn pending_count(conn: &Connection, proposer_workspace: Option<&str>, proposed_by: &str) -> i64 {
    let r = match proposer_workspace {
        Some(ws) => conn.query_row(
            "SELECT COUNT(*) FROM compute_admin_requests WHERE state = 'pending' AND proposer_workspace = ?1",
            params![ws],
            |r| r.get(0),
        ),
        None => conn.query_row(
            "SELECT COUNT(*) FROM compute_admin_requests WHERE state = 'pending' AND proposer_workspace IS NULL AND proposed_by = ?1",
            params![proposed_by],
            |r| r.get(0),
        ),
    };
    r.unwrap_or(0)
}

/// Store a held request. Refuses a proposer over [`MAX_PENDING_PER_PROPOSER`]
/// (`too_many_pending`) and anything the request digest can't cover.
pub fn insert_request(conn: &Connection, n: &NewRequest<'_>) -> Result<RequestRow, (&'static str, String)> {
    if n.class == Class::Read {
        return Err((codes::BAD_ARGS, "read verbs are never requests".into()));
    }
    if pending_count(conn, n.proposer_workspace, n.proposed_by) >= MAX_PENDING_PER_PROPOSER {
        return Err((
            codes::TOO_MANY_PENDING,
            format!("{MAX_PENDING_PER_PROPOSER} requests are already waiting for the owner; withdraw some first"),
        ));
    }
    let digest = request_digest(&json!({"kind": n.kind, "node": n.node_id, "body": n.body})).map_err(|e| (codes::BAD_ARGS, e))?;
    let id = new_request_id().map_err(|e| (codes::BAD_ARGS, e))?;
    // A `servers.add` carries the new server's enrol-code binding, a
    // credential for 10 minutes (V79): the request lives no longer.
    let ttl = if carries_code_binding(n.kind, n.body) { super::proto::share::PENDING_SECS } else { REQUEST_TTL_SECS };
    conn.execute(
        "INSERT INTO compute_admin_requests (id, node_id, kind, class, body_json, digest, summary, reason, proposed_by, \
         proposer_workspace, proposer_session, proposed_at, state, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending', ?13)",
        params![
            id,
            n.node_id,
            n.kind,
            n.class.as_str(),
            n.body.to_string(),
            digest,
            n.summary,
            n.reason,
            n.proposed_by,
            n.proposer_workspace,
            n.proposer_session,
            n.now,
            n.now + ttl
        ],
    )
    .map_err(|e| (codes::BAD_ARGS, e.to_string()))?;
    request_by_id(conn, &id).ok_or((codes::REQUEST_NOT_FOUND, "request vanished after insert".into()))
}

/// Does a request body hold a `servers.add` step (and so an enrol-code
/// binding)?
pub fn carries_code_binding(kind: &str, body: &Value) -> bool {
    steps_of(kind, body).is_ok_and(|steps| steps.iter().any(|s| s.args.get("code_binding").is_some()))
}

/// `[redacted]` in place of every step's `code_binding` (V79: the binding
/// is the credential; it never shows in a listing, audit or message).
pub fn redact_body(body: &Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    if k == "code_binding" {
                        *x = Value::String("[redacted]".into());
                    } else {
                        walk(x);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let mut b = body.clone();
    walk(&mut b);
    b
}

pub fn request_by_id(conn: &Connection, id: &str) -> Option<RequestRow> {
    conn.query_row(&format!("SELECT {REQ_COLS} FROM compute_admin_requests WHERE id = ?1"), params![id], req_from_row)
        .optional()
        .ok()
        .flatten()
}

/// By id, or a unique prefix of at least 8 characters (`car_1234…`).
pub fn find_request(conn: &Connection, token: &str) -> Option<RequestRow> {
    let t = token.trim();
    if let Some(r) = request_by_id(conn, t) {
        return Some(r);
    }
    if t.len() < 8 || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let mut stmt = conn
        .prepare(&format!("SELECT {REQ_COLS} FROM compute_admin_requests WHERE id LIKE ?1 || '%' LIMIT 2"))
        .ok()?;
    let rows: Vec<RequestRow> = stmt.query_map(params![t], req_from_row).ok()?.filter_map(Result::ok).collect();
    (rows.len() == 1).then(|| rows.into_iter().next()).flatten()
}

#[derive(Debug, Default, Clone)]
pub struct ReqFilter<'a> {
    pub node_id: Option<&'a str>,
    /// `open` = pending|approved|sent.
    pub state: Option<&'a str>,
    pub proposer_workspace: Option<&'a str>,
    pub limit: i64,
}

pub fn list_requests(conn: &Connection, f: &ReqFilter<'_>) -> Vec<RequestRow> {
    let mut sql = format!("SELECT {REQ_COLS} FROM compute_admin_requests WHERE 1=1");
    let mut args: Vec<String> = Vec::new();
    if let Some(n) = f.node_id {
        args.push(n.to_string());
        sql.push_str(&format!(" AND node_id = ?{}", args.len()));
    }
    match f.state {
        Some("open") => sql.push_str(" AND state IN ('pending','approved','sent')"),
        Some(s) => {
            args.push(s.to_string());
            sql.push_str(&format!(" AND state = ?{}", args.len()));
        }
        None => {}
    }
    if let Some(w) = f.proposer_workspace {
        args.push(w.to_string());
        sql.push_str(&format!(" AND proposer_workspace = ?{}", args.len()));
    }
    let limit = if f.limit <= 0 { 50 } else { f.limit.min(500) };
    sql.push_str(&format!(" ORDER BY proposed_at DESC, id DESC LIMIT {limit}"));
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map(rusqlite::params_from_iter(args.iter()), req_from_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Pending past `expires_at` → `expired`. Returns what expired.
pub fn expire_due(conn: &Connection, now: i64) -> Vec<RequestRow> {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM compute_admin_requests WHERE state = 'pending' AND expires_at <= ?1")
        .and_then(|mut s| s.query_map(params![now], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let mut out = Vec::new();
    for id in ids {
        let n = conn
            .execute(
                "UPDATE compute_admin_requests SET state = 'expired', decided_at = ?2 WHERE id = ?1 AND state = 'pending'",
                params![id, now],
            )
            .unwrap_or(0);
        if n == 1 {
            if let Some(r) = request_by_id(conn, &id) {
                out.push(r);
            }
        }
    }
    out
}

/// Approved commands whose signatures ran out before they could be sent
/// (the node stayed offline): `approved` with nothing sent and every
/// command's `exp` past → `expired`.
pub fn expire_unsent(conn: &Connection, now: i64) -> Vec<RequestRow> {
    let rows = list_requests(conn, &ReqFilter { state: Some("approved"), limit: 500, ..Default::default() });
    let mut out = Vec::new();
    for r in rows {
        let exp = r.commands.first().and_then(command_exp);
        if r.next_step == 0 && exp.is_some_and(|e| e + wire::CLOCK_SKEW_SECS < now) {
            let _ = set_result(conn, &r.id, "expired", &json!({"code": wire::code::EXPIRED, "message": "the node didn't come online before the approval ran out"}));
            if let Some(x) = request_by_id(conn, &r.id) {
                out.push(x);
            }
        }
    }
    out
}

/// `exp` of a sealed command (the body is ours; parsing it is fine here).
pub fn command_exp(c: &SignedCommand) -> Option<i64> {
    let body = crypto::unb64(&c.body_b64).ok()?;
    serde_json::from_slice::<wire::Command>(&body).ok().map(|c| c.exp)
}

/// Decide a pending request exactly once. `to` is `approved`, `denied`
/// or `withdrawn`. Err codes: `request_not_found`, `approval_expired`
/// (pending but past its lifetime: also marks it expired), `not_pending`.
#[allow(clippy::too_many_arguments)]
pub fn decide(
    conn: &Connection,
    id: &str,
    to: &str,
    by: &str,
    via: Option<&str>,
    deny_reason: Option<&str>,
    at: i64,
) -> Result<RequestRow, &'static str> {
    if !matches!(to, "approved" | "denied" | "withdrawn") {
        return Err(codes::NOT_PENDING);
    }
    let n = conn
        .execute(
            "UPDATE compute_admin_requests SET state = ?2, decided_by = ?3, decided_via = ?4, decided_at = ?5, deny_reason = ?6 \
             WHERE id = ?1 AND state = 'pending' AND expires_at > ?5",
            params![id, to, by, via, at, deny_reason],
        )
        .unwrap_or(0);
    if n == 1 {
        return request_by_id(conn, id).ok_or(codes::REQUEST_NOT_FOUND);
    }
    match request_by_id(conn, id) {
        None => Err(codes::REQUEST_NOT_FOUND),
        Some(r) if r.state == "pending" => {
            let _ = conn.execute(
                "UPDATE compute_admin_requests SET state = 'expired', decided_at = ?2 WHERE id = ?1 AND state = 'pending'",
                params![id, at],
            );
            Err(codes::APPROVAL_EXPIRED)
        }
        Some(r) if r.state == "expired" => Err(codes::APPROVAL_EXPIRED),
        Some(_) => Err(codes::NOT_PENDING),
    }
}

/// Store the sealed commands of an approved request.
pub fn set_commands(conn: &Connection, id: &str, commands: &[SignedCommand], cmd_ids: &[String]) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE compute_admin_requests SET commands_json = ?2, cmd_ids_json = ?3, next_step = 0 WHERE id = ?1",
            params![
                id,
                serde_json::to_string(commands).map_err(|e| e.to_string())?,
                serde_json::to_string(cmd_ids).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("request not found".into());
    }
    Ok(())
}

/// Step `step` (0-based) went to the node.
pub fn mark_sent(conn: &Connection, id: &str, step: i64) {
    let _ = conn.execute(
        "UPDATE compute_admin_requests SET state = 'sent', next_step = ?2 WHERE id = ?1 AND state IN ('approved','sent')",
        params![id, step + 1],
    );
}

/// A plan step finished `done` and more remain: back to `approved` so
/// the next step is sendable.
pub fn reopen_for_next(conn: &Connection, id: &str) {
    let _ = conn.execute("UPDATE compute_admin_requests SET state = 'approved' WHERE id = ?1 AND state = 'sent'", params![id]);
}

/// Final state (`done` | `failed` | `expired`) and the result facts.
pub fn set_result(conn: &Connection, id: &str, state: &str, result: &Value) -> Result<(), String> {
    conn.execute(
        "UPDATE compute_admin_requests SET state = ?2, result_json = ?3, commands_json = NULL WHERE id = ?1",
        params![id, state, result.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Record a step's result without ending the request.
pub fn set_step_result(conn: &Connection, id: &str, result: &Value) {
    let _ = conn.execute("UPDATE compute_admin_requests SET result_json = ?2 WHERE id = ?1", params![id, result.to_string()]);
}

/// The request a sent command belongs to.
pub fn request_by_cmd_id(conn: &Connection, cmd_id: &str) -> Option<RequestRow> {
    if cmd_id.is_empty() || cmd_id.contains('"') {
        return None;
    }
    let pat = format!("\"{cmd_id}\"");
    conn.query_row(
        &format!(
            "SELECT {REQ_COLS} FROM compute_admin_requests WHERE instr(cmd_ids_json, ?1) > 0 AND state IN ('approved','sent') LIMIT 1"
        ),
        params![pat],
        req_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// Approved/sent requests on `node_id` with a step still to send.
pub fn sendable(conn: &Connection, node_id: &str) -> Vec<RequestRow> {
    let mut rows = list_requests(conn, &ReqFilter { node_id: Some(node_id), state: Some("open"), limit: 500, ..Default::default() });
    rows.retain(|r| r.state == "approved" && (r.next_step as usize) < r.commands.len());
    rows.sort_by_key(|r| r.decided_at);
    rows
}

// ── argument checks and plain words ──────────────────────────────────

fn obj(args: &Value) -> Result<&Map<String, Value>, String> {
    args.as_object().ok_or_else(|| "args must be an object".to_string())
}

fn int_in(a: &Map<String, Value>, k: &str, lo: i64, hi: i64) -> Result<Option<i64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_i64().ok_or_else(|| format!("{k} must be a whole number"))?;
            if n < lo || n > hi {
                return Err(format!("{k} must be {lo}..{hi}"));
            }
            Ok(Some(n))
        }
    }
}

fn only_keys(a: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match a.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("unknown argument {k:?}")),
        None => Ok(()),
    }
}

fn has_float(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_f64(),
        Value::Array(a) => a.iter().any(has_float),
        Value::Object(m) => m.values().any(has_float),
        _ => false,
    }
}

/// Every key `policy.set` accepts (k2-node `PolicyFile`).
pub const POLICY_KEYS: &[&str] = &[
    "cpu_cap_percent",
    "mem_cap_gb",
    "disk_budget_gb",
    "disk_floor_gb",
    "availability",
    "window",
    "on_owner_return",
    "max_parallel",
    "workspaces",
    "foreign_locks",
    "write_smoke_lock",
    "network",
    "repo_toolchains",
];
/// Policy keys where a bigger number gives jobs more.
pub const POLICY_BIGGER_IS_WIDER: &[&str] = &["cpu_cap_percent", "mem_cap_gb", "disk_budget_gb", "max_parallel"];
/// Policy keys where a smaller number gives jobs more.
pub const POLICY_SMALLER_IS_WIDER: &[&str] = &["disk_floor_gb"];

/// The controller's guess whether a `policy.set` narrows, against the
/// policy the node last reported (`policy.show`). Conservative: only a
/// numeric key known in `current` moving toward less is `Operate`;
/// anything else is `Privileged` (held for the owner). The node decides
/// for real from its own file and refuses an unapproved widening.
pub fn classify_policy_change(current: &Map<String, Value>, changes: &Map<String, Value>) -> Class {
    let mut any = false;
    for (k, new) in changes {
        if current.get(k) == Some(new) {
            continue;
        }
        any = true;
        let bigger_wider = POLICY_BIGGER_IS_WIDER.contains(&k.as_str());
        let smaller_wider = POLICY_SMALLER_IS_WIDER.contains(&k.as_str());
        if !bigger_wider && !smaller_wider {
            return Class::Privileged;
        }
        let (Some(cur), Some(new)) = (current.get(k).and_then(Value::as_u64), new.as_u64()) else {
            return Class::Privileged;
        };
        let narrows = if bigger_wider { new < cur } else { new > cur };
        if !narrows {
            return Class::Privileged;
        }
    }
    if any {
        Class::Operate
    } else {
        Class::Read
    }
}

/// The class a request step carries when the owner approves it: the
/// node's base class, except `policy.set` and `servers.limits`
/// (re-classed by the node; a proposed one is treated as a widening, so it
/// carries the owner signature).
pub fn step_class(verb: &str) -> Option<Class> {
    let v = wire::verb(verb)?;
    Some(if v.name == "policy.set" || v.name == super::proto::share::VERB_LIMITS { Class::Privileged } else { v.base })
}

/// `servers.limits` against the share the node last listed (§4.3, §9):
/// `Operate` (sent at once) when every change narrows, `Privileged` (held
/// for the owner's approval) when any field widens or the end date moves
/// out or is cleared, `Read` when nothing changes. The node classifies
/// again with the same rule ([`ShareLimits::widens`]).
///
/// [`ShareLimits::widens`]: super::proto::share::ShareLimits::widens
pub fn classify_servers_limits(
    current: &super::proto::share::ShareLimits,
    current_expires_at: Option<i64>,
    args: &super::proto::share::LimitsArgs,
    now: i64,
) -> Class {
    let next = args.set.apply(current);
    let widens = !next.widens(current).is_empty();
    let end_wider = match args.for_secs {
        None => false,
        // Clearing the end date gives more time.
        Some(0) => current_expires_at.is_some(),
        // No end date today = forever: any end date narrows.
        Some(s) => current_expires_at.is_some_and(|e| now + s > e),
    };
    if widens || end_wider {
        Class::Privileged
    } else if next == *current && args.for_secs.is_none() {
        Class::Read
    } else {
        Class::Operate
    }
}

/// The controller's argument check against the node's catalog
/// (`k2_node_proto::admin::VERBS`, wire §3); the node checks again.
/// Read verbs (`health`, `policy.show`, `ps`) are never requests.
/// Returns the step's class ([`step_class`]).
pub fn validate_step(verb: &str, args: &Value) -> Result<Class, (&'static str, String)> {
    let class = step_class(verb).ok_or((
        codes::VERB_NOT_ALLOWED,
        format!("{verb:?} is not an admin verb this node understands (k2 study compute)"),
    ))?;
    if class == Class::Read {
        return Err((codes::BAD_ARGS, format!("{verb} is a read verb: no request needed")));
    }
    let a = obj(args).map_err(|e| (codes::BAD_ARGS, e))?;
    if has_float(args) {
        return Err((codes::BAD_ARGS, format!("{verb}: whole numbers only")));
    }
    let bad = |e: String| (codes::BAD_ARGS, format!("{verb}: {e}"));
    match verb {
        "policy.set" => {
            if a.is_empty() {
                return Err(bad("name at least one key=value".into()));
            }
            only_keys(a, POLICY_KEYS).map_err(bad)?;
            // `network = "none"` passes here: a walled node (0.46.3+)
            // enforces it, and an older node's executor refuses it itself
            // (`network_none_not_enforced_yet`), so a mixed fleet stays honest.
        }
        "policy.narrow" => {
            only_keys(a, &["cpu_millis", "mem_bytes", "max_parallel"]).map_err(bad)?;
            int_in(a, "cpu_millis", 1, i64::MAX).map_err(bad)?;
            int_in(a, "mem_bytes", 1, i64::MAX).map_err(bad)?;
            int_in(a, "max_parallel", 1, 64).map_err(bad)?;
        }
        "admin.level.lower" => {
            only_keys(a, &["level"]).map_err(bad)?;
            let l = a.get("level").and_then(Value::as_str).unwrap_or("");
            if !matches!(l, "off" | "policy") {
                return Err(bad("level is off or policy (raising needs a person at the machine)".into()));
            }
        }
        "control.pause" | "control.drain" | "control.resume" => only_keys(a, &[]).map_err(bad)?,
        "node.update" => return Err(bad("updates go through k2 compute node update (a signed build offer)".into())),
        "node.rollback" => {
            only_keys(a, &["reason", wire::ARG_NOW]).map_err(bad)?;
            wire::now_arg(args).map_err(|r| bad(r.message))?;
        }
        "gc" => {
            only_keys(a, &["older_days", "toolchains"]).map_err(bad)?;
            int_in(a, "older_days", 1, 365).map_err(bad)?;
            if a.get("toolchains").is_some_and(|t| !t.is_boolean()) {
                return Err(bad("toolchains is true or false".into()));
            }
        }
        "jobs.kill" => {
            only_keys(a, &["job_id"]).map_err(bad)?;
            if a.get("job_id").and_then(Value::as_str).is_none_or(str::is_empty) {
                return Err(bad("job_id is required".into()));
            }
        }
        "unit.restart" => {
            // `now`: on a shared node, skip the drain (V55).
            only_keys(a, &["unit", wire::ARG_NOW]).map_err(bad)?;
            wire::now_arg(args).map_err(|r| bad(r.message))?;
            let u = a.get("unit").and_then(Value::as_str).unwrap_or("");
            if !matches!(u, "k2-node" | "k2-node-packages") {
                return Err(bad("unit is k2-node or k2-node-packages".into()));
            }
        }
        "wall.enable" => only_keys(a, &[]).map_err(bad)?,
        // Node sharing (protocol 3): one checker on both ends.
        v if v.starts_with("servers.") => super::proto::share::check_verb_args(v, args).map_err(|e| (codes::BAD_ARGS, e))?,
        "homes.private" => {
            // Absent = every open home the node reports; else named ones.
            only_keys(a, &["homes"]).map_err(bad)?;
            if let Some(h) = a.get("homes") {
                let list = h.as_array().ok_or_else(|| bad("homes is a list of paths".into()))?;
                if list.is_empty() || !list.iter().all(|p| p.as_str().is_some_and(|s| s.starts_with('/'))) {
                    return Err(bad("homes is a non-empty list of absolute paths".into()));
                }
            }
        }
        _ => {}
    }
    Ok(class)
}

fn arg_list(a: &Map<String, Value>) -> String {
    let mut parts: Vec<String> = a
        .iter()
        .map(|(k, v)| match v {
            Value::String(s) => format!("{k}={s}"),
            other => format!("{k}={other}"),
        })
        .collect();
    parts.sort();
    parts.join(", ")
}

/// What the owner reads before approving (§4.1 "plain words").
pub fn summarize(node: &str, verb: &str, args: &Value) -> String {
    let empty = Map::new();
    let a = args.as_object().unwrap_or(&empty);
    let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match verb {
        "policy.set" => format!("change job limits on {node}: {}", arg_list(a)),
        "policy.narrow" => format!("narrow what {node} offers this server: {}", arg_list(a)),
        "admin.level.lower" => format!("lower {node}'s remote-admin level to {}", s("level")),
        "control.pause" => format!("pause {node} (no new jobs)"),
        "control.drain" => format!("drain {node} (finish running jobs, take no new ones)"),
        "control.resume" => format!("resume {node}"),
        "gc" => format!("clean old job folders on {node}{}", a.get("older_days").map(|d| format!(" (older than {d} days)")).unwrap_or_default()),
        "jobs.kill" => format!("kill job {} on {node}", s("job_id")),
        "unit.restart" => format!("restart {} on {node} (as root)", s("unit")),
        "wall.enable" => format!(
            "turn on the job wall on {node} (as root): create the k2job user, let it into the node home, run jobs as k2job; takes effect when k2-node restarts"
        ),
        "homes.private" => match a.get("homes").and_then(Value::as_array) {
            Some(h) => format!(
                "make home folders on {node} private (chmod o-rwx, as root): {}",
                h.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
            ),
            None => format!("make every home folder on {node} that others can read private (chmod o-rwx, as root)"),
        },
        "power.set" if a.contains_key("preset") => format!("set {node} to the {} power preset (never sleep, power back on after a cut)", s("preset")),
        "power.set" => format!("change power settings on {node}: {}", arg_list(a)),
        "power.reboot" => format!("reboot {node}{}", a.get("in_secs").and_then(Value::as_i64).map(|n| format!(" in {n}s")).unwrap_or_default()),
        "power.shutdown" => format!("shut down {node} (someone must power it back on)"),
        "os.updates.install" => format!("install {} OS updates on {node}", s("scope").replace("labels", "the named")),
        "pkg.install" => format!("install packages on {node}: {}", names_str(a)),
        "pkg.remove" => format!("remove packages on {node}: {}", names_str(a)),
        "pkg.allow" => format!("allow jobs on {node} to request packages: {}", names_str(a)),
        "pkg.disallow" => format!("stop allowing packages on {node}: {}", names_str(a)),
        "user.create-service" => format!("create hidden service user {} on {node}", s("name")),
        "user.delete-service" => format!("delete service user {} on {node}", s("name")),
        "proc.kill" => format!("kill process {} ({}) on {node}", a.get("pid").map(Value::to_string).unwrap_or_default(), s("comm")),
        "file.read" => format!("read the end of {} on {node}", s("path")),
        "script" => {
            let t = s("text");
            format!("run a root script on {node} ({} lines, {} bytes) — read it in full before approving", t.lines().count(), t.len())
        }
        "svc.apply" => format!("create or change service {} on {node}", a.get("spec").and_then(|x| x.get("name")).and_then(Value::as_str).unwrap_or("?")),
        "svc.adopt" => format!("adopt the existing {} as a managed service on {node}", s("label")),
        "svc.remove" => format!("remove service {} from {node}", s("name")),
        "servers.add" => {
            let trust = s("trust");
            let until = a.get("for_secs").and_then(Value::as_i64).map(|f| format!(" for {}", super::share::human_secs(f.max(0) as u64))).unwrap_or_default();
            let lim = a.get("limits").and_then(Value::as_object).filter(|m| !m.is_empty()).map(|m| format!(" ({})", arg_list(m))).unwrap_or_default();
            format!(
                "share {node} with the server at {} as {trust}{until}{lim}: it gets its own folders{} and a share of the machine",
                s("url"),
                if trust == "team" { ", user" } else { "" }
            )
        }
        "servers.limits" => {
            let set = a.get("set").and_then(Value::as_object).map(arg_list).unwrap_or_default();
            let until = match a.get("for_secs").and_then(Value::as_i64) {
                Some(0) => ", no end date".to_string(),
                Some(f) => format!(", ends in {}", super::share::human_secs(f as u64)),
                None => String::new(),
            };
            format!("change {}'s share of {node}: {set}{until}", s("server"))
        }
        "servers.pause" => format!("pause {} on {node} (its queued jobs wait; running ones finish)", s("server")),
        "servers.resume" => format!("resume {} on {node}", s("server")),
        "servers.revoke" => format!(
            "revoke {} on {node}: cancel its queued jobs{}{}",
            s("server"),
            if a.get("now").and_then(Value::as_bool) == Some(true) { ", stop its running jobs" } else { ", let running jobs finish" },
            if a.get("wipe").and_then(Value::as_bool) == Some(true) { ", wipe its files now" } else { ", wipe its files in 24 h" }
        ),
        "node.update" => format!("update k2-node on {node} to {}", s("version")),
        "node.rollback" => format!("roll k2-node on {node} back to its previous build"),
        other => format!("{other} on {node}: {}", arg_list(a)),
    }
}

fn names_str(a: &Map<String, Value>) -> String {
    a.get("names")
        .and_then(Value::as_array)
        .map(|v| v.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

// ── node admin columns, health, settings ─────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminCols {
    pub remote_admin: Option<String>,
    pub approval_key_fp: Option<String>,
    pub alert_after_secs: Option<i64>,
    pub update_ring: String,
    pub update: Option<Value>,
    pub offline_alerted_at: Option<i64>,
    /// The policy the node last reported (`policy.show`), PolicyFile keys.
    pub policy: Option<Map<String, Value>>,
    pub policy_at: Option<i64>,
}

pub fn admin_cols(conn: &Connection, node_id: &str) -> AdminCols {
    conn.query_row(
        "SELECT remote_admin, approval_key_fp, alert_after_secs, update_ring, update_json, offline_alerted_at, \
         policy_json, policy_at FROM compute_nodes WHERE id = ?1",
        params![node_id],
        |r| {
            let u: Option<String> = r.get(4)?;
            let p: Option<String> = r.get(6)?;
            Ok(AdminCols {
                remote_admin: r.get(0)?,
                approval_key_fp: r.get(1)?,
                alert_after_secs: r.get(2)?,
                update_ring: r.get(3)?,
                update: u.and_then(|x| serde_json::from_str(&x).ok()),
                offline_alerted_at: r.get(5)?,
                policy: p.and_then(|x| serde_json::from_str(&x).ok()),
                policy_at: r.get(7)?,
            })
        },
    )
    .unwrap_or_default()
}

pub fn set_reported_admin(conn: &Connection, node_id: &str, remote_admin: Option<&str>, approval_key_fp: Option<&str>) {
    let _ = conn.execute(
        "UPDATE compute_nodes SET remote_admin = ?2, approval_key_fp = ?3 WHERE id = ?1",
        params![node_id, remote_admin, approval_key_fp],
    );
}

pub fn set_alert_after(conn: &Connection, node_id: &str, secs: Option<i64>) -> Result<(), String> {
    if let Some(s) = secs {
        if s < MIN_ALERT_AFTER_SECS {
            return Err(format!("offline alerts wait at least {}m", MIN_ALERT_AFTER_SECS / 60));
        }
    }
    conn.execute("UPDATE compute_nodes SET alert_after_secs = ?2 WHERE id = ?1", params![node_id, secs])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn set_update_ring(conn: &Connection, node_id: &str, ring: &str) -> Result<(), String> {
    if !matches!(ring, "canary" | "normal" | "hold") {
        return Err("ring is canary, normal or hold".into());
    }
    conn.execute("UPDATE compute_nodes SET update_ring = ?2 WHERE id = ?1", params![node_id, ring]).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn set_update(conn: &Connection, node_id: &str, update: &Value) {
    let _ = conn.execute("UPDATE compute_nodes SET update_json = ?2 WHERE id = ?1", params![node_id, update.to_string()]);
}

/// Cache the policy a `policy.show` returned.
pub fn set_policy_cache(conn: &Connection, node_id: &str, policy: &Map<String, Value>, at: i64) {
    let _ = conn.execute(
        "UPDATE compute_nodes SET policy_json = ?2, policy_at = ?3 WHERE id = ?1",
        params![node_id, Value::Object(policy.clone()).to_string(), at],
    );
}

pub fn set_offline_alerted(conn: &Connection, node_id: &str, at: Option<i64>) {
    let _ = conn.execute("UPDATE compute_nodes SET offline_alerted_at = ?2 WHERE id = ?1", params![node_id, at]);
}

/// Latest health (no history in v1).
pub fn store_health(conn: &Connection, node_id: &str, at: i64, facts: &Health) -> Result<(), String> {
    conn.execute(
        "INSERT INTO compute_node_health (node_id, at, facts_json) VALUES (?1, ?2, ?3) \
         ON CONFLICT(node_id) DO UPDATE SET at = excluded.at, facts_json = excluded.facts_json",
        params![node_id, at, serde_json::to_string(facts).map_err(|e| e.to_string())?],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn health(conn: &Connection, node_id: &str) -> Option<(i64, Health)> {
    conn.query_row("SELECT at, facts_json FROM compute_node_health WHERE node_id = ?1", params![node_id], |r| {
        let f: String = r.get(1)?;
        Ok((r.get::<_, i64>(0)?, f))
    })
    .optional()
    .ok()
    .flatten()
    .and_then(|(at, f)| serde_json::from_str(&f).ok().map(|f| (at, f)))
}

pub fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM compute_admin_settings WHERE key = ?1", params![key], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

pub fn set_setting(conn: &Connection, key: &str, value: Option<&str>, by: &str, at: i64) -> Result<(), String> {
    match value {
        Some(v) => conn.execute(
            "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
            params![key, v, by, at],
        ),
        None => conn.execute("DELETE FROM compute_admin_settings WHERE key = ?1", params![key]),
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The setting key for "alerts go to" (a workspace id).
pub const ALERTS_TO: &str = "alerts_to";

/// What a person at the node runs once when its executor has no (or
/// another) controller key pinned (wire §6: nodes enrolled after setup).
pub const PIN_COMMAND: &str = "sudo k2-node admin pin";

fn needs_pin_key(node_id: &str) -> String {
    format!("needs_pin:{node_id}")
}

/// Does this result say the node's executor hasn't pinned this
/// controller (`not_configured` naming the controller key, or
/// `wrong_controller`)?
pub fn result_needs_pin(code: Option<&str>, message: Option<&str>) -> bool {
    match code {
        Some(c) if c == wire::code::WRONG_CONTROLLER => true,
        Some(c) if c == wire::code::NOT_CONFIGURED => message.is_some_and(|m| m.contains("controller")),
        _ => false,
    }
}

/// Record (or clear) "needs one-time pin" for a node.
pub fn set_needs_pin(conn: &Connection, node_id: &str, on: bool, at: i64) {
    let _ = set_setting(conn, &needs_pin_key(node_id), on.then_some("1"), "node", at);
}

pub fn needs_pin(conn: &Connection, node_id: &str) -> bool {
    setting(conn, &needs_pin_key(node_id)).is_some()
}

// ── the operate switch (Q14) ─────────────────────────────────────────

/// `projects.agents_can_operate_compute` (fail closed).
pub fn agents_can_operate_compute(conn: &Connection, project_id: &str) -> bool {
    conn.query_row(
        "SELECT agents_can_operate_compute FROM projects WHERE id = ?1",
        params![project_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

/// Dedicated writer for `/cli/agent-access/set` toggle `compute_operate`.
pub fn set_agents_can_operate_compute(conn: &Connection, project_id: &str, on: bool) -> Result<(), String> {
    let n = conn
        .execute("UPDATE projects SET agents_can_operate_compute = ?1 WHERE id = ?2", params![on as i64, project_id])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("Project not found: {project_id}"));
    }
    Ok(())
}

// ── alerts (§10.2) ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub kind: &'static str,
    pub text: String,
}

const GIB: u64 = 1 << 30;

/// Disk floor when the node's policy isn't known (k2-node's default floor
/// is the larger of 10 % of the disk and 30 GB).
pub const DEFAULT_DISK_FLOOR_BYTES: u64 = 30 * GIB;

/// Threshold alerts from one health report (§10.2). `floor_bytes` = the
/// policy's disk floor; `security_since` = when security updates were
/// first seen pending. Facts only, never log text.
pub fn threshold_alerts(node: &str, f: &Health, floor_bytes: u64, security_since: Option<i64>, now: i64) -> Vec<Alert> {
    let mut out = Vec::new();
    let limit = floor_bytes + floor_bytes / 10;
    if let Some(d) = f.disks.iter().find(|d| d.total_bytes > 0 && d.free_bytes < limit) {
        out.push(Alert {
            kind: "disk_low",
            text: format!("{node}: {} GB free on {} (floor {} GB)", d.free_bytes / GIB, d.path, floor_bytes / GIB),
        });
    }
    if f.memory_pressure.as_deref() == Some("critical") {
        out.push(Alert { kind: "memory_pressure", text: format!("{node}: memory pressure is critical") });
    }
    if let Some(p) = f.thermal.as_ref().and_then(|t| t.cpu_speed_limit).filter(|p| *p < 80) {
        out.push(Alert { kind: "thermal", text: format!("{node}: CPU held to {p}% by heat") });
    }
    if let (Some(n), Some(since)) = (f.updates.as_ref().and_then(|u| u.security).filter(|n| *n > 0), security_since) {
        if now - since > SECURITY_UPDATES_AFTER_SECS {
            out.push(Alert {
                kind: "security_updates",
                text: format!("{node}: {n} security update(s) waiting for {} days", (now - since) / 86400),
            });
        }
    }
    if f.updates.as_ref().and_then(|u| u.reboot_required) == Some(true) {
        out.push(Alert { kind: "reboot_required", text: format!("{node}: an update needs a reboot") });
    }
    if !f.executor.reachable {
        out.push(Alert { kind: "executor_unreachable", text: format!("{node}: the admin executor isn't answering") });
    }
    if let Some(j) = f.stuck_jobs.first() {
        out.push(Alert {
            kind: "stuck_job",
            text: format!(
                "{node}: job {} has shown no output and no CPU for {}{}",
                super::message::short(&j.job_id),
                super::message::duration(j.quiet_secs as i64),
                if f.stuck_jobs.len() > 1 { format!(" ({} jobs look stuck)", f.stuck_jobs.len()) } else { String::new() }
            ),
        });
    }
    out
}

/// Was an alert of `kind` for `node` sent in the last [`ALERT_REPEAT_SECS`]?
pub fn alert_recent(conn: &Connection, node_id: &str, kind: &str, now: i64) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM compute_events WHERE kind = 'alert_sent' AND node_id = ?1 AND at > ?2 \
         AND json_extract(detail_json, '$.kind') = ?3",
        params![node_id, now - ALERT_REPEAT_SECS, kind],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Is `node` due an offline alert? Offline (not connected), confirmed,
/// alerts on, silent for longer than its threshold, and not already
/// alerted since it went quiet.
pub fn offline_due(state: &str, online: bool, last_seen: Option<i64>, cols: &AdminCols, now: i64) -> bool {
    let Some(after) = cols.alert_after_secs else { return false };
    if state != "active" || online {
        return false;
    }
    let Some(seen) = last_seen else { return false };
    if now - seen < after {
        return false;
    }
    cols.offline_alerted_at.is_none_or(|a| a < seen)
}

fn clock(at: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| at.to_string())
}

/// The offline alert (§4.5): last route, last health, the FileVault caveat.
pub fn offline_text(node: &str, since: i64, last_route: Option<&str>, h: Option<&Health>) -> String {
    let mut t = format!("{node} offline since {}", clock(since));
    if let Some(r) = last_route {
        t.push_str(&format!(" (last seen via {r})"));
    }
    t.push('.');
    if let Some(f) = h {
        let mut bits = Vec::new();
        if let Some(u) = f.uptime_secs {
            bits.push(format!("uptime {}", super::message::duration(u as i64)));
        }
        if f.power.has_battery {
            bits.push((if f.power.on_battery { "on battery" } else { "on AC" }).to_string());
        }
        if let Some(fv) = &f.disk_encryption {
            bits.push(format!("FileVault {fv}"));
        }
        if let Some(a) = f.sleep.as_ref().and_then(|s| s.autorestart) {
            bits.push(format!("autorestart {}", if a == 1 { "on" } else { "off" }));
        }
        if !bits.is_empty() {
            t.push_str(&format!(" Last health: {}.", bits.join(", ")));
        }
        if f.disk_encryption.as_deref() == Some("on") {
            t.push_str(" If the power went out, this Mac waits at the FileVault screen until someone types the password.");
        }
    }
    t
}

/// The back-online alert: downtime and whether it rebooted (boot id).
pub fn back_online_text(node: &str, down_secs: i64, old_boot: Option<&str>, new_boot: Option<&str>) -> String {
    let mut t = format!("{node} back after {}", super::message::duration(down_secs));
    match (old_boot.filter(|b| !b.is_empty()), new_boot.filter(|b| !b.is_empty())) {
        (Some(a), Some(b)) if a != b => t.push_str("; it rebooted (new boot id)"),
        (Some(_), Some(_)) => t.push_str("; no reboot"),
        _ => {}
    }
    t.push('.');
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn conn_with_node() -> Connection {
        let c = crate::db::isolated_test_connection();
        let labels = BTreeMap::from([("os".to_string(), "macos".to_string())]);
        super::super::store::insert_pending_node(&c, "n1", "mini-1", "fp1", "pem", &labels, "owner", "123456", 5).unwrap();
        super::super::store::confirm_node(&c, "n1", "owner", 6).unwrap();
        c
    }

    fn new<'a>(body: &'a Value, ws: Option<&'a str>, now: i64) -> NewRequest<'a> {
        NewRequest {
            node_id: "n1",
            kind: "command",
            class: Class::Privileged,
            body,
            summary: "s",
            reason: Some("why"),
            proposed_by: "agent:it",
            proposer_workspace: ws,
            proposer_session: None,
            now,
        }
    }

    #[test]
    fn request_decided_once_and_expiry_refuses() {
        let c = conn_with_node();
        let body = json!({"verb": "unit.restart", "args": {"unit": "k2-node"}});
        let r = insert_request(&c, &new(&body, Some("ws1"), 100)).unwrap();
        assert_eq!(r.state, "pending");
        assert!(r.digest.starts_with("sha256:"));
        assert_eq!(r.expires_at, 100 + REQUEST_TTL_SECS);
        let a = decide(&c, &r.id, "approved", "owner", Some("local-auth"), None, 200).unwrap();
        assert_eq!(a.state, "approved");
        assert_eq!(decide(&c, &r.id, "approved", "owner", Some("local-auth"), None, 201).unwrap_err(), codes::NOT_PENDING, "replayed approval");
        assert_eq!(decide(&c, &r.id, "denied", "owner", None, None, 201).unwrap_err(), codes::NOT_PENDING);

        let r2 = insert_request(&c, &new(&body, Some("ws1"), 100)).unwrap();
        let late = 100 + REQUEST_TTL_SECS + 1;
        assert_eq!(decide(&c, &r2.id, "approved", "owner", None, None, late).unwrap_err(), codes::APPROVAL_EXPIRED);
        assert_eq!(request_by_id(&c, &r2.id).unwrap().state, "expired");
        assert_eq!(decide(&c, "car_nope", "approved", "owner", None, None, 1).unwrap_err(), codes::REQUEST_NOT_FOUND);
    }

    #[test]
    fn pending_cap_per_proposer() {
        let c = conn_with_node();
        let body = json!({"verb": "unit.restart", "args": {"unit": "k2-node"}});
        for i in 0..MAX_PENDING_PER_PROPOSER {
            insert_request(&c, &new(&body, Some("ws1"), 100 + i)).unwrap();
        }
        assert_eq!(insert_request(&c, &new(&body, Some("ws1"), 200)).unwrap_err().0, codes::TOO_MANY_PENDING);
        // Another workspace isn't blocked.
        insert_request(&c, &new(&body, Some("ws2"), 200)).unwrap();
        assert_eq!(expire_due(&c, 200 + REQUEST_TTL_SECS).len(), MAX_PENDING_PER_PROPOSER as usize + 1);
    }

    #[test]
    fn commands_and_lookup_by_cmd_id() {
        let c = conn_with_node();
        let body = json!({"steps": [{"verb": "policy.set", "args": {"mem_cap_gb": 12}}, {"verb": "unit.restart", "args": {"unit": "k2-node"}}]});
        let mut n = new(&body, None, 100);
        n.kind = "plan";
        let r = insert_request(&c, &n).unwrap();
        assert_eq!(r.steps().unwrap().len(), 2);
        decide(&c, &r.id, "approved", "owner", Some("vault-password"), None, 150).unwrap();
        let cmds = vec![
            SignedCommand { body_b64: "a".into(), sig_c: "b".into(), sig_o: None, plan_b64: None },
            SignedCommand { body_b64: "c".into(), sig_c: "d".into(), sig_o: None, plan_b64: None },
        ];
        set_commands(&c, &r.id, &cmds, &["cmd-a".into(), "cmd-b".into()]).unwrap();
        assert_eq!(sendable(&c, "n1").len(), 1);
        mark_sent(&c, &r.id, 0);
        assert!(sendable(&c, "n1").is_empty(), "sent requests wait for the step result");
        assert_eq!(request_by_cmd_id(&c, "cmd-b").unwrap().id, r.id);
        assert!(request_by_cmd_id(&c, "cmd").is_none());
        set_result(&c, &r.id, "done", &json!({"ok": true})).unwrap();
        let done = request_by_id(&c, &r.id).unwrap();
        assert_eq!(done.state, "done");
        assert!(done.commands.is_empty(), "signed envelopes are dropped once finished");
        assert!(request_by_cmd_id(&c, "cmd-b").is_none());
    }

    #[test]
    fn argument_checks_follow_the_node_catalog() {
        assert_eq!(validate_step("unit.restart", &json!({"unit": "k2-node"})).unwrap(), Class::Privileged);
        assert_eq!(validate_step("policy.set", &json!({"mem_cap_gb": 10})).unwrap(), Class::Privileged, "a proposed policy.set carries the owner signature");
        assert_eq!(validate_step("gc", &json!({"older_days": 3})).unwrap(), Class::Operate);
        // A walled node enforces network = none; an older one refuses it
        // itself, so the controller lets the step through.
        assert_eq!(validate_step("policy.set", &json!({"network": "none", "mem_cap_gb": 4})).unwrap(), Class::Privileged);
        assert_eq!(validate_step("policy.set", &json!({"network": "full"})).unwrap(), Class::Privileged);
        assert_eq!(validate_step("homes.private", &json!({})).unwrap(), Class::Privileged, "root on people's files: always the owner");
        assert_eq!(validate_step("homes.private", &json!({"homes": ["/Users/rosson"]})).unwrap(), Class::Privileged);
        for bad in [json!({"homes": []}), json!({"homes": ["rel"]}), json!({"homes": "/Users/x"}), json!({"evil": 1})] {
            assert_eq!(validate_step("homes.private", &bad).unwrap_err().0, codes::BAD_ARGS, "{bad}");
        }
        assert_eq!(validate_step("wall.enable", &json!({})).unwrap(), Class::Privileged);
        assert_eq!(validate_step("wall.enable", &json!({"user": "root"})).unwrap_err().0, codes::BAD_ARGS, "the job user isn't the caller's to pick");
        assert!(summarize("k2n2-m1", "wall.enable", &json!({})).contains("k2job"));
        assert!(summarize("k2n2-m1", "homes.private", &json!({})).contains("private"));
        assert_eq!(validate_step("control.pause", &json!({})).unwrap(), Class::Operate);
        // V55: `now` skips a shared node's drain (bool only).
        assert!(validate_step("node.rollback", &json!({"now": true, "reason": "bad build"})).is_ok());
        assert!(validate_step("unit.restart", &json!({"unit": "k2-node", "now": false})).is_ok());
        assert_eq!(validate_step("unit.restart", &json!({"unit": "k2-node", "now": "yes"})).unwrap_err().0, codes::BAD_ARGS);
        assert_eq!(validate_step("node.rollback", &json!({"now": 1})).unwrap_err().0, codes::BAD_ARGS);
        for v in ["sudo", "script", "power.set", "pkg.install"] {
            assert_eq!(validate_step(v, &json!({})).unwrap_err().0, codes::VERB_NOT_ALLOWED, "{v} isn't in the node's v1 catalog");
        }
        for (verb, args) in [
            ("policy.show", json!({})),
            ("policy.set", json!({"evil": 1})),
            ("policy.set", json!({})),
            ("policy.set", json!({"mem_cap_gb": 1.5})),
            ("unit.restart", json!({"unit": "sshd"})),
            ("admin.level.lower", json!({"level": "full"})),
            ("gc", json!({"older_days": 0})),
            ("jobs.kill", json!({})),
            ("node.update", json!({"version": "9.9.9"})),
            ("control.pause", json!({"now": true})),
        ] {
            assert_eq!(validate_step(verb, &args).unwrap_err().0, codes::BAD_ARGS, "{verb} {args}");
        }
    }

    #[test]
    fn policy_change_classification() {
        let cur = json!({"mem_cap_gb": 8, "max_parallel": 2, "disk_floor_gb": 30, "availability": "always"});
        let cur = cur.as_object().unwrap();
        let ch = |v: Value| classify_policy_change(cur, v.as_object().unwrap());
        assert_eq!(ch(json!({"mem_cap_gb": 6})), Class::Operate);
        assert_eq!(ch(json!({"mem_cap_gb": 10})), Class::Privileged);
        assert_eq!(ch(json!({"disk_floor_gb": 40})), Class::Operate);
        assert_eq!(ch(json!({"disk_floor_gb": 20})), Class::Privileged);
        assert_eq!(ch(json!({"mem_cap_gb": 6, "max_parallel": 3})), Class::Privileged);
        assert_eq!(ch(json!({"availability": "plugged_in"})), Class::Privileged);
        assert_eq!(ch(json!({"cpu_cap_percent": 50})), Class::Privileged, "unknown current is not provably narrower");
        assert_eq!(ch(json!({"mem_cap_gb": 8})), Class::Read, "no change");
    }

    /// §4.3, §9: narrowing a share goes at once; any widening (a bigger
    /// limit, a class, a later or cleared end date) waits for the owner.
    #[test]
    fn servers_limits_classification() {
        use crate::compute::proto::share::{LimitsArgs, ShareLimits, ShareOverrides, Trust};
        let cur = ShareLimits::new_server(Trust::Team, false);
        let args = |set: ShareOverrides, for_secs: Option<i64>| LimitsArgs { server: "baden.k2.dev".into(), set, for_secs };
        let c = |a: LimitsArgs, exp: Option<i64>| classify_servers_limits(&cur, exp, &a, 1000);
        assert_eq!(c(args(ShareOverrides { max_queued: Some(5), ..Default::default() }, None), None), Class::Operate);
        assert_eq!(c(args(ShareOverrides { max_running: Some(2), ..Default::default() }, None), None), Class::Privileged);
        assert_eq!(c(args(ShareOverrides { max_queued: Some(5), cpu_hours_day: Some(12), ..Default::default() }, None), None), Class::Privileged, "one widening holds the whole change");
        assert_eq!(c(args(ShareOverrides { max_queued: Some(10), ..Default::default() }, None), None), Class::Read, "no change");
        assert_eq!(c(args(ShareOverrides::default(), Some(3600)), Some(10_000)), Class::Operate, "an earlier end date narrows");
        assert_eq!(c(args(ShareOverrides::default(), Some(86_400)), Some(10_000)), Class::Privileged, "a later end date widens");
        assert_eq!(c(args(ShareOverrides::default(), Some(0)), Some(10_000)), Class::Privileged, "clearing the end date widens");
        assert_eq!(c(args(ShareOverrides::default(), Some(3600)), None), Class::Operate, "adding an end date narrows");
        assert_eq!(step_class("servers.limits"), Some(Class::Privileged), "a held limits change carries the owner signature");
        assert_eq!(step_class("servers.pause"), Some(Class::Operate));
        assert_eq!(step_class("servers.add"), Some(Class::Privileged));
        assert_eq!(validate_step("servers.add", &json!({"url": "https://b.k2.dev", "code_binding": "ab", "fp_prefix": "0123456789abcdef", "trust": "team"})).unwrap(), Class::Privileged);
        assert_eq!(validate_step("servers.add", &json!({"url": "https://b.k2.dev"})).unwrap_err().0, codes::BAD_ARGS);
    }

    /// V79: the enrol-code binding is a credential: the request expires in
    /// 10 minutes and no listing shows it.
    #[test]
    fn a_servers_add_request_expires_in_ten_minutes_and_hides_the_binding() {
        let c = crate::db::isolated_test_connection();
        crate::compute::store::insert_pending_node(&c, "n1", "m", "fp", "pem", &Default::default(), "o", "1", 1).unwrap();
        let body = json!({"verb": "servers.add", "args": {"url": "https://b.k2.dev", "code_binding": "SECRETBINDING", "fp_prefix": "0123456789abcdef", "trust": "own"}});
        let n = NewRequest {
            node_id: "n1",
            kind: "command",
            class: Class::Privileged,
            body: &body,
            summary: "s",
            reason: None,
            proposed_by: "owner",
            proposer_workspace: None,
            proposer_session: None,
            now: 100,
        };
        let r = insert_request(&c, &n).unwrap();
        assert_eq!(r.expires_at, 100 + crate::compute::proto::share::PENDING_SECS);
        assert!(carries_code_binding(&r.kind, &r.body));
        let shown = redact_body(&r.body).to_string();
        assert!(!shown.contains("SECRETBINDING") && shown.contains("[redacted]"), "{shown}");
        let other = json!({"verb": "gc", "args": {}});
        let r2 = insert_request(&c, &NewRequest { body: &other, ..n }).unwrap();
        assert_eq!(r2.expires_at, 100 + REQUEST_TTL_SECS);
        assert!(summarize("k2n2-m1", "servers.add", &body["args"]).contains("https://b.k2.dev"));
        assert!(!summarize("k2n2-m1", "servers.add", &body["args"]).contains("SECRET"));
        assert!(summarize("k2n2-m1", "servers.revoke", &json!({"server": "b", "now": true})).contains("stop its running jobs"));
    }

    #[test]
    fn summaries_are_plain_words() {
        assert_eq!(summarize("k2n2-m1", "unit.restart", &json!({"unit": "k2-node"})), "restart k2-node on k2n2-m1 (as root)");
        assert_eq!(summarize("m", "policy.set", &json!({"mem_cap_gb": 10})), "change job limits on m: mem_cap_gb=10");
        assert_eq!(summarize("m", "gc", &json!({"older_days": 3})), "clean old job folders on m (older than 3 days)");
    }

    #[test]
    fn operate_switch_fails_closed() {
        let c = crate::db::isolated_test_connection();
        assert!(!agents_can_operate_compute(&c, "nope"));
        assert!(set_agents_can_operate_compute(&c, "nope", true).is_err());
    }

    #[test]
    fn operate_switch_rides_the_projects_list() {
        // The Agent tab reads `agentsCanOperateCompute` off the projects list.
        let c = crate::db::isolated_test_connection();
        c.execute("INSERT INTO projects (id, name, path) VALUES ('w1', 'it', '/w/it')", []).unwrap();
        let p = crate::db::schema::Project::get(&c, "w1").unwrap();
        assert_eq!(p.agents_can_operate_compute, 0);
        set_agents_can_operate_compute(&c, "w1", true).unwrap();
        let listed = crate::db::schema::Project::list(&c).unwrap();
        let p = listed.iter().find(|p| p.id == "w1").expect("w1 listed");
        assert_eq!(p.agents_can_operate_compute, 1);
        let v = serde_json::to_value(p).unwrap();
        assert_eq!(v["agentsCanOperateCompute"], 1);
        assert_eq!(crate::db::schema::Project::get(&c, "w1").unwrap().agents_can_operate_compute, 1);
    }

    #[test]
    fn health_alerts_and_offline_rules() {
        use super::super::proto::health::{DiskFact, ExecutorFact, Thermal, Updates};
        let c = conn_with_node();
        let f = Health {
            boot_id: "b1".into(),
            disks: vec![DiskFact { name: "root".into(), path: "/".into(), total_bytes: 500 * GIB, free_bytes: 9 * GIB }],
            thermal: Some(Thermal { cpu_speed_limit: Some(70), max_zone_milli_c: None }),
            executor: ExecutorFact { reachable: false, ..Default::default() },
            disk_encryption: Some("on".into()),
            updates: Some(Updates { pending: Some(3), security: Some(2), reboot_required: Some(true), checked_at: 1, source: "softwareupdate".into() }),
            ..Default::default()
        };
        store_health(&c, "n1", 50, &f).unwrap();
        assert_eq!(health(&c, "n1").unwrap().1, f);
        let kinds: Vec<_> = threshold_alerts("mini-1", &f, 30 * GIB, None, 100).iter().map(|a| a.kind).collect();
        assert_eq!(kinds, vec!["disk_low", "thermal", "reboot_required", "executor_unreachable"]);
        let kinds: Vec<_> = threshold_alerts("mini-1", &f, 5 * GIB, Some(100), 100 + 8 * 86400).iter().map(|a| a.kind).collect();
        assert_eq!(kinds, vec!["thermal", "security_updates", "reboot_required", "executor_unreachable"], "a lower floor, and security updates 8 days old");
        let quiet = Health { executor: ExecutorFact { reachable: true, ..Default::default() }, ..Default::default() };
        assert!(threshold_alerts("m", &quiet, DEFAULT_DISK_FLOOR_BYTES, None, 100).is_empty());

        assert!(!alert_recent(&c, "n1", "disk_low", 1000));
        super::super::store::record_event(&c, "controller", "alert_sent", Some("n1"), None, None, json!({"kind": "disk_low"}));
        let now = super::super::now();
        assert!(alert_recent(&c, "n1", "disk_low", now));
        assert!(!alert_recent(&c, "n1", "thermal", now));

        let mut cols = AdminCols { alert_after_secs: Some(600), ..Default::default() };
        assert!(!offline_due("active", true, Some(0), &cols, 10_000), "online");
        assert!(!offline_due("active", false, Some(9_500), &cols, 10_000), "not long enough");
        assert!(offline_due("active", false, Some(9_000), &cols, 10_000));
        cols.offline_alerted_at = Some(9_700);
        assert!(!offline_due("active", false, Some(9_000), &cols, 10_000), "alerted once");
        cols.alert_after_secs = None;
        assert!(!offline_due("active", false, Some(0), &cols, 10_000), "alerts off");
        assert!(set_alert_after(&c, "n1", Some(60)).is_err());

        let t = offline_text("k2n1-m4", 0, Some("relay"), Some(&f));
        assert!(t.contains("FileVault screen"), "{t}");
        assert_eq!(back_online_text("k2n1-m4", 47 * 60, Some("b1"), Some("b2")), "k2n1-m4 back after 47m00s; it rebooted (new boot id).");
        assert_eq!(back_online_text("k2n1-m4", 60, Some("b1"), Some("b1")), "k2n1-m4 back after 1m00s; no reboot.");

        let p = json!({"mem_cap_gb": 8}).as_object().cloned().unwrap();
        set_policy_cache(&c, "n1", &p, 77);
        let cols = admin_cols(&c, "n1");
        assert_eq!((cols.policy, cols.policy_at), (Some(p), Some(77)));
    }

    #[test]
    fn needs_pin_state() {
        let c = conn_with_node();
        assert!(result_needs_pin(Some("not_configured"), Some("no controller key pinned (sudo k2-node admin pin)")));
        assert!(result_needs_pin(Some("wrong_controller"), None));
        assert!(!result_needs_pin(Some("not_configured"), Some("node key unreadable")));
        assert!(!result_needs_pin(None, None));
        assert!(!needs_pin(&c, "n1"));
        set_needs_pin(&c, "n1", true, 1);
        assert!(needs_pin(&c, "n1"));
        set_needs_pin(&c, "n1", false, 2);
        assert!(!needs_pin(&c, "n1"));
    }

    #[test]
    fn settings_round_trip() {
        let c = crate::db::isolated_test_connection();
        assert_eq!(setting(&c, ALERTS_TO), None);
        set_setting(&c, ALERTS_TO, Some("ws-it"), "owner", 1).unwrap();
        assert_eq!(setting(&c, ALERTS_TO).as_deref(), Some("ws-it"));
        set_setting(&c, ALERTS_TO, None, "owner", 2).unwrap();
        assert_eq!(setting(&c, ALERTS_TO), None);
    }
}
