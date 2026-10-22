//! A1 — `/cli/permission/*` handlers: `request`, `update`, `status`,
//! `cancel`, `list`, `enabled` (PRD rev 3.1 §3.2, §11.1, §11.2) and the
//! dispatcher arms (TCP `routes/dispatcher.rs`, the cell
//! `cell_server.rs`), reached only while the gate is on (§2.4):
//!
//! | server switch | agent switch | an agent passport gets |
//! |---|---|---|
//! | off | — | the arm is skipped: today's unknown-route answer |
//! | on | off | **today's body for its transport and method** ([`today`]) |
//! | on | on | served |
//!
//! People (the owner token, a login) are served while the server switch
//! is on; the agent verbs refuse them (`forbidden`), and they answer cards
//! through `POST /cli/thread/answer`. `GET /cli/permission/pending` is lane
//! A2's (`waiting.rs`).

use std::collections::HashMap;

use k2_core::permission::catalog::{ActionSpec, NotCardable, ParamKind};
use k2_core::permission::request::{code, limits as lim, params_hash, PermissionRequest, RequestId, RequestState};
use k2_core::permission::gate;
use serde_json::{json, Map, Value};

use super::answer::{self, err};
use super::{batch, limits, present, store};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub(crate) const REQUEST: &str = "/cli/permission/request";
pub(crate) const UPDATE: &str = "/cli/permission/update";
pub(crate) const STATUS: &str = "/cli/permission/status";
pub(crate) const CANCEL: &str = "/cli/permission/cancel";
pub(crate) const LIST: &str = "/cli/permission/list";
pub(crate) const ENABLED: &str = "/cli/permission/enabled";
/// People only (Owner/Admin): every agent's "Can ask for permission cards"
/// switch and the recent requests (the Settings page, the Agent tab; D2).
pub(crate) const AGENTS: &str = "/cli/permission/agents";

/// Every route A1 serves (rows for lane B's tables: [`POST_ROUTES`] are
/// POST, the rest GET; all are agent verbs).
pub(crate) const ROUTES: &[&str] = &[REQUEST, UPDATE, STATUS, CANCEL, LIST, ENABLED, AGENTS];
pub(crate) const POST_ROUTES: &[&str] = &[REQUEST, UPDATE, CANCEL];

/// Request bodies are small: an action, a few values and one line of why.
pub(crate) const MAX_BODY: usize = 16 * 1024;

pub(crate) fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub(crate) fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

/// Which door a request came through (today's bodies differ, §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    Tcp,
    Cell,
}

pub(crate) fn too_large() -> CliResponse {
    err(413, "bad_request", format!("request bodies here are at most {} KiB", MAX_BODY / 1024))
}

/// Today's answer for an unknown `/cli` route, byte for byte (the
/// off-goldens): what a caller the family doesn't serve gets while the
/// server switch is on (an agent whose own switch is off, a bad token).
/// (Lane B's `route_policy::permission_agent_off_response`.)
pub(crate) fn today(transport: Transport, is_post: bool) -> CliResponse {
    crate::routes::route_policy::permission_agent_off_response(transport == Transport::Cell, if is_post { "POST" } else { "GET" })
}

/// The family's door: the gate, then the verb. `is_post` is the request
/// method. Callers skip this entirely while the server switch is off.
pub(crate) fn handle(path: &str, who: Who, is_post: bool, body: &[u8], params: &HashMap<String, String>, transport: Transport) -> CliResponse {
    if !gate::server_on() {
        return today(transport, is_post);
    }
    match &who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } if gate::agent_switch_on(project_id) => {}
        Who::Owner | Who::Login { .. } => {}
        _ => return today(transport, is_post),
    }
    handle_gated(path, &who, is_post, body, params)
}

/// The verbs, with the gate already passed for `who`.
pub(crate) fn handle_gated(path: &str, who: &Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if !is_route(path) {
        return CliResponse::not_found();
    }
    if is_post_route(path) != is_post {
        return if is_post_route(path) {
            err(405, "method_not_allowed", "POST required")
        } else {
            err(405, "method_not_allowed", "use GET")
        };
    }
    if path == AGENTS {
        return agents(who, &body_map(body, params));
    }
    if path == ENABLED {
        return enabled(who);
    }
    let Some((workspace_id, session_id)) = who.passport().map(|(w, s)| (w.to_string(), s.to_string())) else {
        return err(
            403,
            "forbidden",
            "an agent verb: agents ask with k2 permission request; people answer the card in the Thread (or k2 permission approve|deny)",
        );
    };
    let body = body_map(body, params);
    match path {
        REQUEST => request(&workspace_id, &session_id, &body),
        UPDATE => update(&workspace_id, &body),
        STATUS => status(&workspace_id, &body),
        CANCEL => cancel(&workspace_id, &body),
        LIST => list(&workspace_id, str_field(&body, "area"), str_field(&body, "search")),
        _ => CliResponse::not_found(),
    }
}

/// The JSON body as a map; the query/form params fill keys it lacks.
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

fn str_field<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn bool_field(m: &Map<String, Value>, k: &str) -> bool {
    match m.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim(), "1" | "true" | "yes"),
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        _ => false,
    }
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn enabled(who: &Who) -> CliResponse {
    match who.passport() {
        Some((ws, _)) => ok(json!({ "enabled": true, "agent": present::workspace_handle(ws), "cards": k2_core::permission::CARDS_CONTRACT })),
        None => ok(json!({ "enabled": true, "server": true, "cards": k2_core::permission::CARDS_CONTRACT })),
    }
}

// ── checks on what the agent sends (§3.2, §6.3) ─────────────────────

/// Token-shaped text (keys, passes, bearer strings) never goes on a card.
pub(crate) fn secret_shaped(s: &str) -> bool {
    if k2_core::llm_accounts::looks_like_secret(s) {
        return true;
    }
    const PREFIXES: &[&str] = &["k2skn_", "k2sk_", "k2rs_", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "sk-", "AKIA", "-----BEGIN"];
    s.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'' | '=' | ':'))
        .any(|w| PREFIXES.iter().any(|p| w.starts_with(p) && w.len() >= p.len() + 8))
}

fn check_why(why: Option<&str>) -> Result<String, CliResponse> {
    let Some(w) = why else {
        return Err(err(400, code::BAD_WHY, "say what you need it for: --why \"<one line>\""));
    };
    if w.chars().count() > lim::WHY_MAX_CHARS {
        return Err(err(400, code::BAD_WHY, format!("--why is at most {} characters", lim::WHY_MAX_CHARS)));
    }
    if w.contains(['\n', '\r']) || w.chars().any(char::is_control) {
        return Err(err(400, code::BAD_WHY, "--why is one line"));
    }
    if secret_shaped(w) {
        return Err(err(400, code::BAD_WHY, "--why looks like it holds a secret; never put one on a card"));
    }
    Ok(w.to_string())
}

/// A content hash: exactly 64 lowercase hex characters (a sha256, as a
/// widget version or a data-source spec carries). The one long-token shape
/// a card value may hold: `secret_shaped` flags any 40+ character token, and
/// these name daemon-known content, never a credential.
pub(crate) fn is_content_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn check_value(key: &str, s: &str) -> Result<String, CliResponse> {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > 200 || t.chars().any(char::is_control) {
        return Err(err(400, code::BAD_PARAMS, format!("{key}= takes one value of 1-200 characters")));
    }
    if !is_content_hash(t) && secret_shaped(t) {
        return Err(err(400, code::BAD_PARAMS, format!("{key}= looks like a secret; never put one on a card")));
    }
    Ok(t.to_string())
}

/// The action's values, typed: known keys only, required ones present,
/// choices checked, lists split (`a,b`; `\,` is a literal comma).
fn check_params(spec: &ActionSpec, raw: Option<&Value>) -> Result<Map<String, Value>, CliResponse> {
    let raw = match raw {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(err(400, code::BAD_PARAMS, "params is an object of key=value pairs")),
    };
    let mut out = Map::new();
    for (k, v) in &raw {
        let Some(p) = spec.params.iter().find(|p| p.key == k) else {
            let known: Vec<&str> = spec.params.iter().map(|p| p.key).collect();
            let hint = if k == "workspace" {
                "the workspace is always yours (from your passport), never a value".to_string()
            } else if known.is_empty() {
                format!("{} takes no values", spec.id)
            } else {
                format!("{} takes {}", spec.id, known.iter().map(|k| format!("{k}=")).collect::<Vec<_>>().join(", "))
            };
            return Err(err(400, code::BAD_PARAMS, format!("unknown value {k}=: {hint}")));
        };
        let texts: Vec<String> = match v {
            Value::String(s) if p.list => k2_core::overlay::split_options_csv(s),
            Value::String(s) => vec![s.clone()],
            Value::Number(n) => vec![n.to_string()],
            Value::Bool(b) => vec![b.to_string()],
            Value::Array(a) if p.list => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect(),
            _ => return Err(err(400, code::BAD_PARAMS, format!("{k}= takes {}", if p.list { "a comma list" } else { "one value" }))),
        };
        let mut vals = Vec::new();
        for t in &texts {
            let t = check_value(k, t)?;
            if p.kind == ParamKind::Choice && !p.choices.contains(&t.as_str()) {
                return Err(err(400, code::BAD_PARAMS, format!("{k}= is one of {}", p.choices.join(", "))));
            }
            vals.push(t);
        }
        if vals.is_empty() {
            return Err(err(400, code::BAD_PARAMS, format!("{k}= is empty")));
        }
        out.insert(k.clone(), if p.list { json!(vals) } else { json!(vals[0]) });
    }
    for p in spec.params.iter().filter(|p| p.required) {
        if !out.contains_key(p.key) {
            return Err(err(400, code::BAD_PARAMS, format!("{} needs {}=", spec.id, p.key)));
        }
    }
    Ok(out)
}

/// The conversation the card goes on: `addr` (the agent's own Thread
/// address, e.g. a sidecar's) when it is this workspace's, else the
/// workspace's pinned Chat. Returns `(conversation, address)`.
fn asking_conversation(workspace_id: &str, addr: Option<&str>) -> Result<(String, String), CliResponse> {
    let handle = present::workspace_handle(workspace_id);
    let pinned = crate::overlay_routes::pinned_chat_of(workspace_id);
    let Some(addr) = addr.filter(|a| *a != handle) else {
        return pinned.map(|c| (c, handle)).ok_or_else(|| err(409, "no_thread", "this agent has no Thread a person answers on yet"));
    };
    let Some(conv) = crate::overlay_routes::conversation_for_addr(addr) else {
        return Err(err(400, code::BAD_PARAMS, format!("addr {addr} is not a Thread of yours")));
    };
    if pinned.as_deref() == Some(conv.as_str()) {
        return Ok((conv, handle));
    }
    let owner: Option<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT project_id FROM overlay_conversations WHERE conversation_id = ?1",
            rusqlite::params![conv],
            |r| r.get(0),
        )
        .ok()
    };
    let mine = owner.as_deref() == Some(workspace_id)
        || k2_core::workspace_session_handles::project_id_for_session_id(&k2_core::db::shared().lock(), &conv)
            .ok()
            .flatten()
            .as_deref()
            == Some(workspace_id);
    if !mine {
        return Err(err(403, "forbidden", format!("addr {addr} is not a Thread of yours")));
    }
    Ok((conv, addr.to_string()))
}

/// The login the agent was working with: the last person who posted to
/// this conversation within 30 minutes (best effort, a label only).
fn asked_for(conversation_id: &str, now: i64) -> Option<String> {
    let page = k2_core::overlay::read_thread_page(conversation_id, None, None, 50).ok()?;
    page.items
        .iter()
        .rev()
        .take_while(|i| now - i.doc.created_at <= lim::ASKED_FOR_WINDOW_SECS)
        .find_map(|i| {
            let a = i.doc.author.as_ref()?;
            (a.kind == k2_core::overlay::doc::AUTHOR_USER || a.kind == k2_core::overlay::doc::AUTHOR_OWNER).then(|| a.login.clone())
        })
}

fn not_cardable(id: &str) -> Option<CliResponse> {
    let n = present::find_not_cardable(id)?;
    let (hint, covered) = match n.why {
        NotCardable::Covered { by } => (format!("{id} is covered by {by}: Ask with: k2 permission request {by} --why \"<what you need it for>\""), Some(by)),
        NotCardable::Never { .. } => (format!("{id} is never a card; a person does it in {}. It can't be requested.", n.r#where), None),
        NotCardable::Later | NotCardable::Dropped => (format!("{id} isn't a card yet; a person does it in {}. It can't be requested.", n.r#where), None),
    };
    Some(CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": { "code": code::NOT_CARDABLE, "hint": hint },
            "permission": match covered {
                Some(by) => json!({ "action": by, "params": {}, "command": format!("k2 permission request {by} --why \"<what you need it for>\"") }),
                None => json!({ "requestable": false, "where": n.r#where }),
            },
        })
        .to_string(),
    })
}

// ── POST /cli/permission/request ────────────────────────────────────

fn request(workspace_id: &str, session_id: &str, body: &Map<String, Value>) -> CliResponse {
    let now = k2_core::overlay::now_secs();
    let Some(action_id) = str_field(body, "action") else {
        return err(400, code::BAD_ACTION, "name the action: k2 permission request <action> (see k2 permission list)");
    };
    let Some(registered) = present::find_action(action_id) else {
        if let Some(r) = not_cardable(action_id) {
            return r;
        }
        return err(400, code::BAD_ACTION, format!("no action {action_id}; k2 permission list shows what can be asked"));
    };
    let spec = registered.spec;
    let values = match check_params(spec, body.get("params")) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let why = match check_why(str_field(body, "why")) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let ctx = super::actions::Ctx {
        workspace_id: workspace_id.to_string(),
        workspace_handle: present::workspace_handle(workspace_id),
        params: values.clone(),
    };
    let w = present::words(spec, &ctx);
    match (registered.check)(&ctx) {
        super::actions::Check::AlreadyDone => {
            return ok(json!({
                "status": code::ALREADY_DONE,
                "action": spec.id,
                "text": format!("{} is already {} for {}. Nothing to ask; run your command again.", w.setting, present::on_word(spec.direction), w.scope),
            }))
        }
        super::actions::Check::Moved(why) => return err(400, code::BAD_PARAMS, why),
        super::actions::Check::Needed => {}
    }
    let hash = params_hash(&values);
    if let Ok(Some(same)) = store::open_same(workspace_id, spec.id, &hash) {
        return ok(json!({
            "status": code::ALREADY_PENDING,
            "id": same.display_id(),
            "expiresAt": same.expires_at,
            "card": same.card_id,
            "addr": same.addr,
        }));
    }
    if let Err(l) = limits::check_ask(workspace_id, spec.id, &hash, now) {
        let mut v = json!({ "ok": false, "error": { "code": l.code, "hint": l.hint } });
        if let (Some(o), Some(extra)) = (v.as_object_mut(), l.extra.as_object()) {
            for (k, x) in extra {
                o.insert(k.clone(), x.clone());
            }
        }
        return CliResponse { status: answer_status(l.status), content_type: "application/json", body: v.to_string() };
    }
    let floor = spec.floor;
    let routed = routed_to(floor);
    let expires_at = now + spec.expiry.default_secs;
    if bool_field(body, "dryRun") || bool_field(body, "dry_run") {
        return ok(json!({
            "status": "dry_run",
            "action": spec.id,
            "text": w.what_changes,
            "setting": w.setting,
            "consequences": w.consequences,
            "approvers": floor.label(),
            "routedTo": routed,
            "expiresIn": spec.expiry.default_secs,
        }));
    }
    let (conversation_id, addr) = match asking_conversation(workspace_id, str_field(body, "addr")) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let held_ref = spec
        .params
        .iter()
        .find(|p| p.kind == ParamKind::HeldRequest)
        .and_then(|p| values.get(p.key))
        .and_then(Value::as_str)
        .map(str::to_string);
    let req = PermissionRequest {
        id: RequestId::new(),
        workspace_id: workspace_id.to_string(),
        session_id: Some(session_id.to_string()).filter(|s| !s.is_empty()),
        conversation_id: None,
        addr: Some(addr.clone()),
        card_id: None,
        action: spec.id.to_string(),
        params: Value::Object(values),
        params_hash: hash.clone(),
        text_hash: present::words_hash(&w),
        why,
        state: RequestState::Pending,
        floor: present::floor_str(floor).to_string(),
        held_ref,
        batch_id: None,
        ask_number: limits::ask_number(workspace_id, spec.id, &hash, now),
        asked_for: asked_for(&conversation_id, now),
        created_at: now,
        expires_at,
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    if let Err(e) = store::insert(&req) {
        return err(500, "db", e);
    }
    let placed = match batch::place(&req, &conversation_id) {
        Ok(p) => p,
        Err(e) => {
            let _ = store::cas_open_to(req.id.as_str(), RequestState::Failed);
            let _ = store::decide(
                req.id.as_str(),
                &store::Decided { state: RequestState::Failed, by: None, via: None, reason: None, message: Some(&e), at: now },
            );
            return err(409, "no_thread", format!("the card could not be posted: {e}"));
        }
    };
    let stored = store::get(req.id.as_str()).ok().flatten().unwrap_or(req);
    answer::audit(k2_core::permission::audit_event::REQUEST, &format!("agent:{}", present::workspace_handle(workspace_id)), "passport", &stored);
    ok(json!({
        "id": stored.display_id(),
        "status": "pending",
        "action": spec.id,
        "expiresAt": placed.expires_at,
        "routedTo": routed,
        "approvers": floor.label(),
        "text": w.what_changes,
        "setting": w.setting,
        "consequences": w.consequences,
        "duration": w.duration,
        "card": placed.card_id,
        "addr": addr,
        "batch": placed.batch,
        "ask": stored.ask_number,
        "asksAllowed": lim::ASKS_PER_WINDOW,
    }))
}

/// The people a card is routed to: `[{login, role}]` (the eligible logins).
fn routed_to(floor: k2_core::permission::catalog::Floor) -> Value {
    let roles: std::collections::HashMap<String, String> = k2_core::connect_users::list_users()
        .unwrap_or_default()
        .into_iter()
        .map(|u| (u.username, format!("{:?}", u.role).to_ascii_lowercase()))
        .collect();
    json!(present::eligible_logins(floor)
        .into_iter()
        .map(|l| json!({ "role": roles.get(&l).cloned().unwrap_or_default(), "login": l }))
        .collect::<Vec<_>>())
}

fn answer_status(code: u16) -> &'static str {
    match code {
        409 => "409 Conflict",
        429 => "429 Too Many Requests",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        _ => "500 Internal Server Error",
    }
}

/// The agent's own request by `pr_…` or held id, else 404.
fn own(workspace_id: &str, body: &Map<String, Value>) -> Result<PermissionRequest, CliResponse> {
    let Some(id) = str_field(body, "id") else {
        return Err(err(400, "usage", "name the request: <pr_… id>"));
    };
    match store::get_by_display_id(id) {
        Ok(Some(r)) if r.workspace_id == workspace_id => Ok(r),
        Ok(_) => Err(err(404, "not_found", format!("no request {id} of yours"))),
        Err(e) => Err(err(500, "db", e)),
    }
}

// ── GET /cli/permission/agents (people, Admin floor) ────────────────

/// A workspace by id, handle or path (the Settings page sends the id).
fn workspace_id_of(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let db = k2_core::db::shared();
    if k2_core::db::schema::Project::get(&db.lock(), token).is_ok() {
        return Some(token.to_string());
    }
    let path = crate::workspace_msg::resolve_workspace(token)?;
    let conn = db.lock();
    k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
}

/// `{agents:[{workspace, handle, name, on}], recent:[summary + agent,
/// workspace, setting]}`: every agent's switch (name order) and the 20
/// newest requests server-wide, or one workspace's with `workspace=`.
fn agents(who: &Who, body: &Map<String, Value>) -> CliResponse {
    match who {
        Who::Owner => {}
        Who::Login { role, .. } if *role >= k2_core::connect_users::Role::Admin => {}
        Who::Login { .. } => return err(403, "role_required", "an owner or an admin sees the agents' permission-card switches"),
        _ => return err(403, "forbidden", "a people route: the Settings page's agent list (an agent asks with k2 permission request)"),
    }
    let only = match str_field(body, "workspace") {
        Some(t) => match workspace_id_of(t) {
            Some(id) => Some(id),
            None => return err(404, "workspace_not_found", format!("no workspace '{t}'")),
        },
        None => None,
    };
    let names: std::collections::HashMap<String, String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut out = std::collections::HashMap::new();
        if let Ok(mut st) = conn.prepare("SELECT id, name FROM projects") {
            if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
                out.extend(rows.filter_map(Result::ok));
            }
        }
        out
    };
    let agents: Vec<Value> = k2_core::workspace::settings::agents_permission_switches()
        .into_iter()
        .filter(|(id, _)| only.as_deref().map_or(true, |o| o == id))
        .map(|(id, on)| {
            json!({
                "workspace": id,
                "handle": present::workspace_handle(&id),
                "name": names.get(&id).cloned().unwrap_or_default(),
                "on": on,
            })
        })
        .collect();
    let rows = match only.as_deref() {
        Some(ws) => store::for_workspace(ws, 20),
        None => store::recent(20),
    };
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return err(500, "db", e),
    };
    let recent: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut v = summary(r);
            v["agent"] = json!(present::workspace_handle(&r.workspace_id));
            v["workspace"] = json!(r.workspace_id);
            v["setting"] = json!(present::find_action(&r.action).map(|a| a.spec.words.setting).unwrap_or(r.action.as_str()));
            v
        })
        .collect();
    ok(json!({ "agents": agents, "recent": recent }))
}

fn summary(r: &PermissionRequest) -> Value {
    json!({
        "id": r.display_id(),
        "action": r.action,
        "params": r.params,
        "state": r.state.as_str(),
        "askedAt": r.created_at,
        "expiresAt": r.expires_at,
        "answeredBy": r.answered_by,
        "answeredAt": r.answered_at,
        "reason": r.reason,
        "message": r.result_message,
        "card": r.card_id,
        "addr": r.addr,
        "ask": r.ask_number,
    })
}

// ── GET /cli/permission/status, POST cancel, POST update ────────────

fn status(workspace_id: &str, body: &Map<String, Value>) -> CliResponse {
    if str_field(body, "id").is_none() {
        let recent: Vec<Value> = store::for_workspace(workspace_id, 20).unwrap_or_default().iter().map(summary).collect();
        return ok(json!({ "requests": recent }));
    }
    match own(workspace_id, body) {
        Ok(r) => ok(summary(&r)),
        Err(e) => e,
    }
}

fn cancel(workspace_id: &str, body: &Map<String, Value>) -> CliResponse {
    let r = match own(workspace_id, body) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let msg = format!("Withdrawn by {}.", present::workspace_handle(workspace_id));
    match answer::close_open(std::slice::from_ref(&r), RequestState::Cancelled, &msg).into_iter().next() {
        Some(done) => {
            let mut v = summary(&done);
            v["message"] = json!(format!("Withdrawn {}. Nothing was applied.", done.display_id()));
            ok(v)
        }
        None => err(409, code::REQUEST_NOT_PENDING, format!("{} is {}; only a waiting request can be withdrawn", r.display_id(), r.state.as_str())),
    }
}

/// Once, on a denied request within 7 days (§3.2): a new `why`, the card
/// posted again for the person who denied it.
fn update(workspace_id: &str, body: &Map<String, Value>) -> CliResponse {
    let now = k2_core::overlay::now_secs();
    let r = match own(workspace_id, body) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let why = match check_why(str_field(body, "why")) {
        Ok(w) => w,
        Err(e) => return e,
    };
    if r.updated_at.is_some() {
        return err(409, code::ALREADY_UPDATED, format!("{} was already updated once; ask your human the usual way", r.display_id()));
    }
    let Some(conversation_id) = r.conversation_id.clone() else {
        return err(409, code::REQUEST_NOT_PENDING, "this request has no card");
    };
    let expires_at = now + present::find_action(&r.action).map(|a| a.spec.expiry.default_secs).unwrap_or(k2_core::permission::catalog::DEFAULT_EXPIRY_SECS);
    match store::reopen_denied(r.id.as_str(), &why, now, now - lim::UPDATE_WINDOW_SECS, expires_at) {
        Ok(true) => {}
        Ok(false) => {
            return err(409, code::REQUEST_NOT_PENDING, format!("only a request denied in the last 7 days can be updated; {} is {}", r.display_id(), r.state.as_str()))
        }
        Err(e) => return err(500, "db", e),
    }
    let reopened = match store::get(r.id.as_str()) {
        Ok(Some(x)) => x,
        _ => return err(500, "db", "the request vanished"),
    };
    match batch::place(&reopened, &conversation_id) {
        Ok(placed) => ok(json!({
            "id": reopened.display_id(),
            "status": "pending",
            "card": placed.card_id,
            "expiresAt": placed.expires_at,
            "deniedBy": r.answered_by,
            "message": format!("{} is waiting again with your new reason; {} denied it before.", reopened.display_id(), r.answered_by.clone().unwrap_or_default()),
        })),
        Err(e) => err(409, "no_thread", format!("the card could not be posted: {e}")),
    }
}

// ── GET /cli/permission/list ────────────────────────────────────────

fn list(workspace_id: &str, area: Option<&str>, search: Option<&str>) -> CliResponse {
    let ws = present::workspace_handle(workspace_id);
    let ctx = super::actions::Ctx { workspace_id: workspace_id.to_string(), workspace_handle: ws.clone(), params: Map::new() };
    let mut actions = Vec::new();
    let search = search.map(str::to_ascii_lowercase);
    for r in present::all_actions() {
        let s = r.spec;
        if area.is_some_and(|a| a != s.area.as_str()) {
            continue;
        }
        let w = present::words(s, &ctx);
        if let Some(q) = &search {
            let hay = format!("{} {} {}", s.id, w.setting, w.what_changes).to_ascii_lowercase();
            if !hay.contains(q.as_str()) {
                continue;
            }
        }
        let now_state = if s.params.iter().any(|p| p.required) {
            Value::Null
        } else {
            json!(match (r.check)(&ctx) {
                super::actions::Check::AlreadyDone => present::on_word(s.direction),
                _ => "needed",
            })
        };
        actions.push(json!({
            "id": s.id,
            "area": s.area.as_str(),
            "setting": w.setting,
            "whatChanges": w.what_changes,
            "floor": s.floor.as_str(),
            "approvers": s.floor.label(),
            "proof": match s.proof { k2_core::permission::catalog::Proof::Click => "click", _ => "confirm" },
            "grant": s.grant,
            "direction": s.direction,
            "params": s.params.iter().map(|p| json!({ "key": p.key, "kind": p.kind, "required": p.required, "list": p.list, "choices": p.choices })).collect::<Vec<_>>(),
            "now": now_state,
            "command": format!("k2 permission request {}{} --why \"<what you need it for>\"", s.id, s.params.iter().filter(|p| p.required).map(|p| format!(" {}=…", p.key)).collect::<String>()),
        }));
    }
    let recent: Vec<Value> = store::for_workspace(workspace_id, 20).unwrap_or_default().iter().map(summary).collect();
    ok(json!({ "agent": ws, "actions": actions, "requests": recent }))
}

#[cfg(test)]
mod value_tests {
    use super::{check_value, is_content_hash};

    fn refused(v: &str) -> bool {
        match check_value("spec", v) {
            Ok(_) => false,
            Err(r) => {
                assert_eq!(r.status, "400 Bad Request", "{}", r.body);
                assert!(r.body.contains("looks like a secret"), "{}", r.body);
                true
            }
        }
    }

    /// A 64-hex content hash passes the secret check (widget versions,
    /// source specs); every other 40+ character token is still refused.
    #[test]
    fn only_a_64_hex_content_hash_skips_the_secret_check() {
        let hash = "0123456789abcdef".repeat(4);
        assert!(is_content_hash(&hash));
        assert_eq!(check_value("version", &hash).unwrap_or_else(|r| panic!("{}", r.body)), hash);
        for v in [
            "0123456789ABCDEF".repeat(4),                         // uppercase hex
            format!("{}0", &hash),                                // 65 hex
            hash[..63].to_string() + "g",                         // 64, not all hex
            "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8s9T0".to_string(), // 40 alphanumeric
            "Zm9vYmFyYmF6cXV4cXV1eGNvcmdlZ3JhdWx0Z2FycGx5".to_string(),
            format!("{}xyz", "0123456789abcdef".repeat(4)),       // 67, hex + tail
        ] {
            assert!(!is_content_hash(&v), "{v}");
            assert!(refused(&v), "{v} must still be refused");
        }
        assert!(refused("k2sk_0123456789abcdef"), "a token prefix is refused whatever its length");
        assert!(!refused("0123456789ab"), "a 12-hex short form was never secret-shaped");
    }
}
