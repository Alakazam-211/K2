//! `/cli/approve/*` (prd-signed-approvals-v1 S1).
//!
//! | Route | Method | Who |
//! |---|---|---|
//! | `request` | POST | an agent (passport) with `approve.request` |
//! | `status` | GET | the asking agent (its own requests) |
//! | `wait` | GET | the asking agent: long-poll until final (≤ 50 s a call) |
//! | `cancel` | POST | the asking agent |
//! | `get` | GET | the asked person's own login: the bytes, to show and sign |
//! | `pending` | GET | a person's own login: what waits on them |
//! | `answer` | POST | the asked person's own login, never an agent or a K2 terminal |
//! | `rule` | POST | a person's own login: allow or block an agent asking them |
//! | `agents` | GET, POST | Owner or Admin: who holds `approve.request` |
//!
//! Every mutating route answers a GET with 405 first. Answers come only
//! from a login session (the phone signs in as its person); the owner
//! token and every passport are refused, so an agent can't answer its own
//! request, even through the owner token in `~/.k2`.

use std::collections::HashMap;

use k2_core::approve_requests::{self as store, codes, Answer, Answered, ApprovalRequest, NewRequest, Rule, StoreError};
use serde_json::{json, Map, Value};

use super::{card, people, shape, wait};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::permission::answer::err;
use k2_core::brand::{CLI_NAME, PRODUCT_NAME};
use k2_core::{cli_name, product_name};

pub(crate) const REQUEST: &str = "/cli/approve/request";
pub(crate) const STATUS: &str = "/cli/approve/status";
pub(crate) const WAIT: &str = "/cli/approve/wait";
pub(crate) const CANCEL: &str = "/cli/approve/cancel";
pub(crate) const GET: &str = "/cli/approve/get";
pub(crate) const PENDING: &str = "/cli/approve/pending";
pub(crate) const ANSWER: &str = "/cli/approve/answer";
pub(crate) const RULE: &str = "/cli/approve/rule";
pub(crate) const AGENTS: &str = "/cli/approve/agents";

pub(crate) const ROUTES: &[&str] = &[REQUEST, STATUS, WAIT, CANCEL, GET, PENDING, ANSWER, RULE, AGENTS];
/// POST only.
pub(crate) const POST_ROUTES: &[&str] = &[REQUEST, CANCEL, ANSWER, RULE];
/// GET to read, POST to change.
pub(crate) const BOTH_ROUTES: &[&str] = &[AGENTS];

/// A 16 KiB payload as base64 plus the JSON around it, or a record.
pub(crate) const MAX_BODY: usize = 64 * 1024;

/// The refusal an agent gets while its `approve.request` permission is off
/// (a permission-card wall: `k2 permission request approve.request`).
pub(crate) const CODE_OFF: &str = "approve_request_off";

pub(crate) fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub(crate) fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

pub(crate) fn too_large() -> CliResponse {
    err(413, "bad_request", format!("request bodies here are at most {} KiB", MAX_BODY / 1024))
}

pub(crate) fn bad(code: &str, hint: impl std::fmt::Display) -> CliResponse {
    err(400, code, hint)
}

/// `{"ok":true, …v}`.
fn ok(v: Value) -> CliResponse {
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(true));
    if let Value::Object(o) = v {
        m.extend(o);
    }
    CliResponse::ok_json(Value::Object(m).to_string())
}

fn store_err(e: StoreError) -> CliResponse {
    let status = match e.code {
        codes::DUPLICATE_NONCE | codes::NOT_WAITING | codes::EXPIRED => 409,
        codes::TOO_MANY_WAITING | codes::RATE_LIMITED => 429,
        codes::NOT_FOUND => 404,
        _ => 500,
    };
    err(status, e.code, e.message)
}

fn now() -> i64 {
    k2_core::overlay::now_secs()
}

/// The JSON body as a map; query parameters fill keys it lacks.
fn body_map(body: &[u8], params: &HashMap<String, String>) -> Map<String, Value> {
    let mut m: Map<String, Value> = serde_json::from_slice::<Value>(body).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default();
    for (k, v) in params {
        if k == "token" || k.starts_with("principal") || k == crate::caller_workspace::PRINCIPAL_BOUND_KEY {
            continue;
        }
        m.entry(k.clone()).or_insert_with(|| Value::String(v.clone()));
    }
    m
}

fn s<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())
}

/// A workspace by its id or handle (then name or path, as the vault reads
/// them) → `projects.id`.
fn workspace_of(token: &str) -> Option<String> {
    let direct: Option<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT id FROM projects WHERE id = ?1 OR handle = ?1 LIMIT 1", rusqlite::params![token], |r| r.get(0)).ok()
    };
    direct.or_else(|| crate::vault::workspace_id(token))
}

/// The family's door. `is_post` is the request method.
pub(crate) fn handle(path: &str, who: &Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if !is_route(path) {
        return CliResponse::not_found();
    }
    if is_post_route(path) && !is_post {
        return err(405, "method_not_allowed", "POST required");
    }
    if !is_post_route(path) && !BOTH_ROUTES.contains(&path) && is_post {
        return err(405, "method_not_allowed", "use GET");
    }
    let b = body_map(body, params);
    match path {
        REQUEST | STATUS | WAIT | CANCEL => {
            let Some((ws, session)) = who.passport() else {
                return err(
                    403,
                    "approval_agent_only",
                    concat!("an agent verb: agents ask with ", cli_name!(), " approve request; people answer on their phone in ", product_name!()),
                );
            };
            match path {
                REQUEST => request(ws, session, &b),
                STATUS => status(ws, &b),
                WAIT => wait_for(ws, &b),
                _ => cancel(ws, &b),
            }
        }
        GET | PENDING | ANSWER | RULE => {
            let Who::Login { username, .. } = who else {
                return err(
                    403,
                    "approval_person_only",
                    concat!("only the asked person answers an approval, signed in on their own device; an agent, a ", product_name!(), " terminal or the owner token never can"),
                );
            };
            let Some(person_id) = people::id_of_login(username) else {
                return err(403, "approval_person_only", "this login can't answer approvals");
            };
            match path {
                GET => get(&person_id, &b),
                PENDING => pending(&person_id),
                ANSWER => answer(username, &person_id, &b),
                _ => rule(&person_id, &b),
            }
        }
        _ => agents(who, is_post, &b),
    }
}

// ── views ───────────────────────────────────────────────────────────

/// What the asking agent sees. `recordText` is the record exactly as the
/// device posted it (what `--out` writes); `record` is the same, parsed.
fn agent_view(r: &ApprovalRequest) -> Value {
    let record = r.record.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok());
    json!({
        "id": r.id,
        "state": r.state.as_str(),
        "person": r.person_login,
        "kind": r.kind,
        "rp": r.rp,
        "nonce": r.nonce,
        "answerBy": r.answer_by,
        "createdAt": r.created_at,
        "answeredAt": r.answered_at,
        "device": r.device,
        "via": r.answered_via,
        "keyId": r.key_id,
        "alg": r.alg,
        "record": record,
        "recordText": r.record,
        "card": r.card_id.as_ref().map(|c| json!({ "conversationId": r.conversation_id, "id": c })),
    })
}

/// What the asked person's device gets: the bytes and the daemon's own
/// facts, nothing descriptive beside them (PRD §5). Who asked is the
/// daemon's report, marked as such (SA-11).
fn person_view(r: &ApprovalRequest) -> Value {
    json!({
        "id": r.id,
        "state": r.state.as_str(),
        "payloadB64": shape::encode(&r.payload),
        "payloadSha256": r.payload_sha256,
        "requestedBy": { "agent": card::agent_addr(&r.workspace_id), "reportedBy": "k2" },
        "answerBy": r.answer_by,
        "createdAt": r.created_at,
        "card": r.card_id.as_ref().map(|c| json!({ "conversationId": r.conversation_id, "id": c })),
    })
}

// ── agent verbs ─────────────────────────────────────────────────────

fn request(ws: &str, session: &str, b: &Map<String, Value>) -> CliResponse {
    let now = now();
    let allowed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::agent_may_request(&conn, ws)
    };
    match allowed {
        Ok(true) => {}
        Ok(false) => {
            return err(
                403,
                CODE_OFF,
                concat!("this agent may not ask people for signed approvals yet. An Owner or Admin turns it on with ", cli_name!(), " approve agents <workspace> on"),
            )
        }
        Err(e) => return store_err(e),
    }
    let (bytes, shape) = match shape::payload(s(b, "payload_b64"), now) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let person = match people::resolve(s(b, "to")) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let new = NewRequest {
        workspace_id: ws.to_string(),
        session_id: Some(session.to_string()).filter(|s| !s.is_empty()),
        person_id: person.id.clone(),
        person_login: person.login.clone(),
        payload: bytes,
        kind: shape.kind,
        rp: shape.rp,
        nonce: shape.nonce,
        answer_by: shape.answer_by,
    };
    let inserted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::may_ask(&conn, &person.id, ws, person.works_for) {
            Ok(true) => store::insert(&conn, &new, now),
            Ok(false) => {
                return err(
                    403,
                    "approval_not_allowed",
                    format!(
                        "{} hasn't allowed this agent to ask them for approvals. By default an agent asks only the person it works for; {} can allow it in {PRODUCT_NAME}",
                        person.login, person.login
                    ),
                )
            }
            Err(e) => Err(e),
        }
    };
    let mut req = match inserted {
        Ok(r) => r,
        Err(e) => return store_err(e),
    };
    if let Some((conv, id)) = card::post(&req) {
        req.conversation_id = Some(conv);
        req.card_id = Some(id);
    }
    ok(agent_view(&req))
}

/// The asking workspace's own request, after an expiry sweep.
fn own(ws: &str, id: &str) -> Result<ApprovalRequest, CliResponse> {
    super::expiry::sweep(now());
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::get(&conn, id) {
        Ok(Some(r)) if r.workspace_id == ws => Ok(r),
        Ok(_) => Err(err(404, codes::NOT_FOUND, format!("no approval {id} from this agent ({CLI_NAME} approve status lists yours)"))),
        Err(e) => Err(store_err(e)),
    }
}

fn status(ws: &str, b: &Map<String, Value>) -> CliResponse {
    if let Some(id) = s(b, "id") {
        return match own(ws, id) {
            Ok(r) => ok(agent_view(&r)),
            Err(r) => r,
        };
    }
    super::expiry::sweep(now());
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::for_workspace(&conn, ws, 20) {
        Ok(rows) => ok(json!({ "requests": rows.iter().map(agent_view).collect::<Vec<_>>() })),
        Err(e) => store_err(e),
    }
}

fn wait_for(ws: &str, b: &Map<String, Value>) -> CliResponse {
    let Some(id) = s(b, "id") else {
        return bad("bad_request", "which approval? wait?id=ap_…");
    };
    let until = std::time::Instant::now() + wait::clamp(s(b, "timeout"));
    let _held = wait::hold(id);
    loop {
        let seen = wait::counter();
        let r = match own(ws, id) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        if r.state.is_final() || std::time::Instant::now() >= until {
            return ok(agent_view(&r));
        }
        // Wake at answer_by at the latest, so expiry is seen on time.
        let by = std::time::Instant::now() + std::time::Duration::from_secs((r.answer_by - now()).max(0) as u64 + 1);
        wait::wait_past(seen, until.min(by));
    }
}

fn cancel(ws: &str, b: &Map<String, Value>) -> CliResponse {
    let Some(id) = s(b, "id") else {
        return bad("bad_request", "which approval? {\"id\":\"ap_…\"}");
    };
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::cancel(&conn, id, ws, now())
    };
    match r {
        Ok(r) => {
            card::close(&r);
            wait::notify();
            ok(agent_view(&r))
        }
        Err(e) => store_err(e),
    }
}

// ── the person's verbs ──────────────────────────────────────────────

/// The person's own request (anyone else's id is "not found").
fn mine(person_id: &str, id: &str) -> Result<ApprovalRequest, CliResponse> {
    super::expiry::sweep(now());
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::get(&conn, id) {
        Ok(Some(r)) if r.person_id == person_id => Ok(r),
        Ok(_) => Err(err(404, codes::NOT_FOUND, format!("no approval {id} for you"))),
        Err(e) => Err(store_err(e)),
    }
}

fn get(person_id: &str, b: &Map<String, Value>) -> CliResponse {
    let Some(id) = s(b, "id") else {
        return bad("bad_request", "which approval? get?id=ap_…");
    };
    match mine(person_id, id) {
        Ok(r) => ok(person_view(&r)),
        Err(r) => r,
    }
}

fn pending(person_id: &str) -> CliResponse {
    let now = now();
    super::expiry::sweep(now);
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::waiting_for_person(&conn, person_id, now) {
        Ok(rows) => ok(json!({ "requests": rows.iter().map(person_view).collect::<Vec<_>>() })),
        Err(e) => store_err(e),
    }
}

fn answer(login: &str, person_id: &str, b: &Map<String, Value>) -> CliResponse {
    let Some(id) = s(b, "id") else {
        return bad("bad_request", "which approval? {\"id\":\"ap_…\"}");
    };
    let req = match mine(person_id, id) {
        Ok(r) => r,
        Err(r) => return r,
    };
    if req.state.is_final() {
        return err(409, codes::NOT_WAITING, format!("approval {id} is {}, not waiting", req.state.as_str()));
    }
    let via = match s(b, "via").unwrap_or("phone") {
        v @ ("phone" | "desktop" | "web") => v.to_string(),
        other => return bad("bad_request", format!("via is phone, desktop or web, not {other:?}")),
    };
    let device = s(b, "device").map(|d| d.chars().filter(|c| !c.is_control()).take(80).collect::<String>());
    let a = match s(b, "decision") {
        Some("approve") => {
            // The record as the device wrote it (a JSON string), or an
            // object, which is written back as compact JSON.
            let text = match b.get("record") {
                Some(Value::String(t)) => t.clone(),
                Some(v @ Value::Object(_)) => v.to_string(),
                _ => return bad("approval_bad_record", "an approval carries the device's record: {\"v\":1,\"payload_b64\":…,\"key_id\":…,\"alg\":…,\"sig_b64\":…}"),
            };
            match shape::record(&text, &req.payload) {
                Ok((record, key_id, alg)) => Answer::Approve { record, key_id, alg },
                Err(r) => return r,
            }
        }
        Some("decline") => Answer::Decline,
        _ => return bad("bad_request", "decision is approve or decline"),
    };
    let who = Answered { by: login.to_string(), via, device };
    let done = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::answer(&conn, id, &a, &who, now())
    };
    match done {
        Ok(r) => {
            card::close(&r);
            wait::notify();
            card::tell_agent(&r);
            ok(json!({ "id": r.id, "state": r.state.as_str() }))
        }
        Err(e) => {
            if e.code == codes::EXPIRED {
                if let Ok(r) = mine(person_id, id) {
                    card::close(&r);
                    wait::notify();
                    card::tell_agent(&r);
                }
            }
            store_err(e)
        }
    }
}

fn rule(person_id: &str, b: &Map<String, Value>) -> CliResponse {
    let Some(target) = s(b, "workspace") else {
        return bad("bad_request", "which agent? {\"workspace\":\"<handle>\",\"rule\":\"allow|block|default\"}");
    };
    let Some(ws) = workspace_of(target) else {
        return err(404, "not_found", format!("no workspace named {target}"));
    };
    let r = match s(b, "rule") {
        Some("allow") => Some(Rule::Allow),
        Some("block") => Some(Rule::Block),
        Some("default") => None,
        _ => return bad("bad_request", "rule is allow, block or default"),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::set_rule(&conn, person_id, &ws, r, now()) {
        Ok(()) => ok(json!({ "workspace": card::agent_addr(&ws), "rule": r.map(Rule::as_str).unwrap_or("default") })),
        Err(e) => store_err(e),
    }
}

// ── the permission (Owner / Admin) ──────────────────────────────────

fn set_agent(ws: &str, on: bool, by: &str) -> CliResponse {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::set_agent(&conn, ws, on, by, now()) {
        Ok(()) => ok(json!({ "workspace": card::agent_addr(ws), "on": on })),
        Err(e) => store_err(e),
    }
}

/// The route's entry for the permission card (`approve.request`): the
/// route's own floor, checked again with the clicker's current role.
pub(crate) fn agents_entry(who: &crate::permission::actions::Answerer, ws: &str, on: bool) -> CliResponse {
    if crate::permission::actions::switches::route_floor(who, AGENTS).is_err() {
        return err(403, "owner_or_admin", OWNER_OR_ADMIN);
    }
    set_agent(ws, on, &who.stamp())
}

const OWNER_OR_ADMIN: &str = "only an Owner or Admin turns approve.request on or off for an agent";

fn agents(who: &Who, is_post: bool, b: &Map<String, Value>) -> CliResponse {
    if !who.is_admin() {
        return err(403, "owner_or_admin", OWNER_OR_ADMIN);
    }
    if is_post {
        let Some(target) = s(b, "workspace") else {
            return bad("bad_request", "which agent? {\"workspace\":\"<handle>\",\"on\":true}");
        };
        let Some(ws) = workspace_of(target) else {
            return err(404, "not_found", format!("no workspace named {target}"));
        };
        let on = match b.get("on") {
            Some(Value::Bool(v)) => *v,
            Some(Value::String(v)) if v == "on" || v == "true" => true,
            Some(Value::String(v)) if v == "off" || v == "false" => false,
            _ => return bad("bad_request", "on is true or false"),
        };
        return set_agent(&ws, on, &who.label());
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::agents(&conn) {
        Ok(rows) => ok(json!({
            "agents": rows.iter().map(|(ws, by, at)| json!({ "workspace": card::agent_addr(ws), "grantedBy": by, "grantedAt": at })).collect::<Vec<_>>()
        })),
        Err(e) => store_err(e),
    }
}
