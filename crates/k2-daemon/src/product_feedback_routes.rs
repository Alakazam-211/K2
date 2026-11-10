//! `/cli/product-feedback/*` — Send feedback to K2
//! (prd-k2-product-feedback-v1 rev 3 PF17, 0.46.11). Not the tickets
//! (`/cli/feedback/*`, whose POST prefix arm would catch a route under it).
//!
//! Routes (every mutating one POST-only: a GET is 405):
//! - `POST send` `{category?, text, contactEmail?, client?, k2Version?,
//!   locale?, diagnostics?: {draftId, drop?, edited?}}` → `{id, state}`
//! - `GET  list?state=&limit=` → own rows; the owner and admins see all
//! - `GET  show?id=` → one row (own; the owner and admins any)
//! - `POST retry {id}` (`failed` → `queued`), `POST cancel {id}`
//!   (`queued` → `cancelled`)
//! - `GET  settings` → `{enabled, agents, agentDiagnostics,
//!   rememberDiagnostics, canEdit, canSend, why?, message?, endpoint,
//!   contactAddress, pending, service}`; `POST settings` `{enabled?,
//!   agents?, agentDiagnostics?}` (the owner or an admin, a person) and
//!   `{rememberDiagnostics?}` (any person, for themselves)
//!
//! Who: the owner token, a Connect login (Member floor in `route_policy`),
//! a K2 terminal's passport (a person typing) and an agent's passport. App
//! passes and API keys: 403 `product_feedback_wrong_credential`. An agent
//! sends only while `agents` is on, within the box limits (1 a minute and
//! 3 a day per workspace, 10 a day for all agents), with no contact email.
//! Only `senderKind` leaves the box; the sender's name stays here.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::connect_users::Role;
use k2_core::product_feedback::store::{self, Row, State};
use k2_core::product_feedback::{self as pf, now_ms, settings};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::product_feedback_drain as drain;
use crate::session_events::{emit, SessionEvent};

pub const MAX_BODY: usize = 64 * 1024;

pub const SEND: &str = "/cli/product-feedback/send";
pub const LIST: &str = "/cli/product-feedback/list";
pub const SHOW: &str = "/cli/product-feedback/show";
pub const RETRY: &str = "/cli/product-feedback/retry";
pub const CANCEL: &str = "/cli/product-feedback/cancel";
pub const SETTINGS: &str = "/cli/product-feedback/settings";

pub const GET_ROUTES: &[&str] = &[LIST, SHOW, SETTINGS];
pub const POST_ROUTES: &[&str] = &[SEND, RETRY, CANCEL, SETTINGS];

pub fn is_route(path: &str) -> bool {
    GET_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

/// POST-only rows (a GET on them is 405).
pub fn is_post_only(path: &str) -> bool {
    POST_ROUTES.contains(&path) && !GET_ROUTES.contains(&path)
}

pub fn who_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Who {
    crate::compute_routes::who_from_tcp(query, bearer, owner_token)
}

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

fn ok(mut body: J) -> CliResponse {
    if let Some(o) = body.as_object_mut() {
        o.insert("ok".into(), json!(true));
    }
    resp("200 OK", body)
}

fn refuse(status: &'static str, code: &str, message: impl Into<String>) -> CliResponse {
    resp(status, json!({ "ok": false, "error": code, "message": message.into() }))
}

pub fn too_large() -> CliResponse {
    refuse("413 Payload Too Large", "body_too_large", format!("Bodies here are at most {} KiB.", MAX_BODY / 1024))
}

fn bad(code: &str, message: impl Into<String>) -> CliResponse {
    refuse("400 Bad Request", code, message)
}

fn db_err(e: impl std::fmt::Display) -> CliResponse {
    CliResponse::internal_error(format!("product feedback store: {e}"))
}

/// The caller, as this module sees it.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    /// `person` or `agent` (what leaves the box).
    kind: &'static str,
    /// Local only: `owner`, `login:<user>`, `agent:<pid>`, `shell:<pid>`.
    sender: String,
    /// A person's own key for the remembered choice (owner token: `owner`).
    username: Option<String>,
    /// Owner token or an Owner/Admin login.
    admin: bool,
    /// A passport (agent or K2 terminal): always `client: cli`.
    passport: bool,
}

impl Caller {
    fn is_agent(&self) -> bool {
        self.kind == "agent"
    }
}

pub(crate) fn caller(who: &Who) -> Result<Caller, CliResponse> {
    Ok(match who {
        Who::Owner => Caller { kind: "person", sender: "owner".into(), username: Some("owner".into()), admin: true, passport: false },
        Who::Login { username, role } => Caller {
            kind: "person",
            sender: format!("login:{username}"),
            username: Some(username.clone()),
            admin: *role >= Role::Admin,
            passport: false,
        },
        // A K2 terminal tab: a person typing (PF9: only an agent passport
        // is an agent).
        Who::Shell { project_id, .. } => {
            Caller { kind: "person", sender: format!("shell:{project_id}"), username: None, admin: false, passport: true }
        }
        Who::Agent { project_id, .. } => {
            Caller { kind: "agent", sender: format!("agent:{project_id}"), username: None, admin: false, passport: true }
        }
        Who::Wrong => {
            return Err(refuse(
                "403 Forbidden",
                "product_feedback_wrong_credential",
                "App passes and API keys can't send feedback to K2. People use the button next to Help; agents use k2 send-feedback.",
            ))
        }
        Who::Invalid => return Err(refuse("401 Unauthorized", "unauthorized", "Sign in to this server first.")),
    })
}

/// Whether `c` may see `row`.
fn visible(c: &Caller, row: &Row) -> bool {
    c.admin || row.sender == c.sender
}

fn project_name(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT name FROM projects WHERE id = ?1", [id], |r| r.get::<_, String>(0)).ok()
}

/// How the Sent tab names the sender (local only).
fn sender_label(sender: &str) -> String {
    if sender == "owner" {
        return "the owner".into();
    }
    if let Some(u) = sender.strip_prefix("login:") {
        return u.to_string();
    }
    if let Some(p) = sender.strip_prefix("agent:") {
        return format!("agent in {}", project_name(p).unwrap_or_else(|| "a removed workspace".into()));
    }
    if let Some(p) = sender.strip_prefix("shell:") {
        return format!("K2 terminal in {}", project_name(p).unwrap_or_else(|| "a removed workspace".into()));
    }
    sender.to_string()
}

/// `code: detail` → (code, detail).
fn split_error(e: &str) -> (String, String) {
    match e.split_once(": ") {
        Some((c, d)) if c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') => (c.to_string(), d.to_string()),
        _ => (String::new(), e.to_string()),
    }
}

/// The plain line the Sent tab and the CLI show for a row's reason.
pub fn reason_line(code: &str, detail: &str) -> String {
    match code {
        k2_core::product_feedback::wire::E_UNREACHABLE | k2_core::product_feedback::wire::E_RELAY_NOT_READY => {
            "K2's feedback service isn't reachable yet. Your feedback is saved on this server and sends when it is.".into()
        }
        "" => detail.to_string(),
        _ => detail.to_string(),
    }
}

fn row_json(c: &Caller, r: &Row, full: bool) -> J {
    let (code, detail) = r.last_error.as_deref().map(split_error).unwrap_or_default();
    let mut v = json!({
        "id": r.id,
        "createdAt": r.created_at,
        "updatedAt": r.updated_at,
        "category": r.category,
        "text": r.body,
        "senderKind": r.sender_kind,
        "client": r.client,
        "sender": sender_label(&r.sender),
        "mine": r.sender == c.sender,
        "state": r.state.as_str(),
        "attempts": r.attempts,
        "nextAttemptAt": if r.state == State::Queued { json!(r.next_attempt_at) } else { J::Null },
        "error": if code.is_empty() { J::Null } else { json!(code) },
        "errorMessage": r.last_error.as_ref().map(|_| reason_line(&code, &detail)),
        "remoteId": r.remote_id,
        "sentAt": r.sent_at,
        "contactEmail": r.contact_email,
        "hasDiagnostics": r.diagnostics_json.is_some(),
    });
    if full {
        let sections: J = r
            .diagnostics_json
            .as_deref()
            .and_then(|d| serde_json::from_str(d).ok())
            .unwrap_or(J::Null);
        v["diagnostics"] = sections;
        v["meta"] = serde_json::from_str(&r.meta_json).unwrap_or(J::Null);
    }
    v
}

fn emit_row(id: &str, st: State, error: Option<&str>) {
    let _ = emit(SessionEvent::ProductFeedbackStatus {
        id: id.to_string(),
        state: st.as_str().to_string(),
        error: error.map(str::to_string),
    });
}

fn service_json() -> J {
    match drain::service_seen() {
        None => json!({ "state": "unknown" }),
        Some(s) => {
            let message = match s.state {
                "ok" => "K2's feedback service is reachable.".to_string(),
                "unreachable" => reason_line(k2_core::product_feedback::wire::E_UNREACHABLE, ""),
                "paused" => "Alakazam paused the feedback service for now. Your feedback is saved and sends when it opens.".into(),
                "limited" => "The feedback service asked this server to wait. Your feedback is saved and sends later.".into(),
                _ => s.detail.clone().unwrap_or_default(),
            };
            json!({ "state": s.state, "code": s.code, "message": message, "at": s.at_ms })
        }
    }
}

pub fn handle(path: &str, is_post: bool, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let c = match caller(&who) {
        Ok(c) => c,
        Err(r) => return r,
    };
    match (path, is_post) {
        (SEND, true) => send(&c, body),
        (LIST, false) => list(&c, params),
        (SHOW, false) => show(&c, params),
        (RETRY, true) => transition(&c, body, true),
        (CANCEL, true) => transition(&c, body, false),
        (SETTINGS, false) => settings_get(&c),
        (SETTINGS, true) => settings_post(&c, body),
        _ => CliResponse::method_not_allowed(),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    category: Option<String>,
    text: Option<String>,
    contact_email: Option<String>,
    client: Option<String>,
    k2_version: Option<String>,
    locale: Option<String>,
}

fn parse<T: for<'de> Deserialize<'de> + Default>(body: &[u8]) -> Result<T, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| bad("bad_json", format!("The body isn't the JSON this route takes: {e}")))
}

/// Why `c` may not send now (PF7, PF8, PF14, PF19), as the refusal.
fn send_gate(c: &Caller) -> Option<CliResponse> {
    if let Some(h) = drain::hold() {
        let status = if h == drain::Hold::NeedsConnect { "409 Conflict" } else { "403 Forbidden" };
        return Some(refuse(status, h.code(), h.message()));
    }
    if c.is_agent() && !settings::load().agents {
        return Some(refuse(
            "403 Forbidden",
            "agents_off",
            "The owner turned off feedback from agents on this server. Ask your owner to turn it on (k2 send-feedback settings --agents on), or ask them to send it.",
        ));
    }
    None
}

fn meta(c: &Caller, client: &str, b: &SendBody) -> J {
    let daemon = env!("CARGO_PKG_VERSION");
    let k2 = if client == "desktop" && !c.passport {
        b.k2_version.as_deref().and_then(|v| pf::short_field(v, 32)).unwrap_or_else(|| daemon.to_string())
    } else {
        daemon.to_string()
    };
    let locale = b
        .locale
        .as_deref()
        .and_then(|l| pf::short_field(l, 35))
        .or_else(|| {
            std::env::var("LANG").ok().and_then(|l| pf::short_field(l.split('.').next().unwrap_or(""), 35))
        })
        .unwrap_or_default();
    json!({
        "k2Version": k2,
        "daemonVersion": daemon,
        "installKind": crate::boot_status::install_kind(),
        "os": std::env::consts::OS,
        "osVersion": pf::os_version(),
        "arch": std::env::consts::ARCH,
        "locale": locale,
    })
}

fn send(c: &Caller, body: &[u8]) -> CliResponse {
    let b: SendBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if let Some(r) = send_gate(c) {
        return r;
    }
    let category = b.category.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("other");
    if !pf::CATEGORIES.contains(&category) {
        return bad("bad_category", "category is one of bug, idea, praise, other.");
    }
    let client = if c.passport {
        "cli"
    } else {
        match b.client.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("cli") {
            x if pf::CLIENTS.contains(&x) => x,
            _ => return bad("bad_client", "client is one of desktop, web, cli."),
        }
    };
    let text = match pf::clean_text(b.text.as_deref().unwrap_or("")) {
        Ok(t) => t,
        Err((code, msg)) => return bad(code, msg),
    };
    let contact = match b.contact_email.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(_) if c.is_agent() => {
            return bad("contact_people_only", "Only a person can ask Alakazam to email them; leave contactEmail out.")
        }
        Some(e) if pf::valid_email(e) => Some(e.to_string()),
        Some(_) => return bad("bad_email", "contactEmail is one email address."),
    };
    let now = now_ms();
    let db = k2_core::db::shared();
    let conn = db.lock();
    if c.is_agent() {
        match store::agent_limit(&conn, &c.sender, now) {
            Ok(None) => {}
            Ok(Some(l)) => {
                return resp(
                    "429 Too Many Requests",
                    json!({ "ok": false, "error": "agent_limit", "message": l.message(), "retryAfterSecs": l.retry_after_secs() }),
                )
            }
            Err(e) => return db_err(e),
        }
    }
    let n = store::NewRow {
        id: pf::new_client_id(),
        sender_kind: c.kind.to_string(),
        sender: c.sender.clone(),
        client: client.to_string(),
        category: category.to_string(),
        body: text,
        contact_email: contact,
        meta_json: meta(c, client, &b).to_string(),
        diagnostics_json: None,
    };
    let row = match store::insert(&conn, &n, now) {
        Ok(r) => r,
        Err(e) => return db_err(e),
    };
    drop(conn);
    emit_row(&row.id, State::Queued, None);
    drain::kick();
    ok(json!({
        "id": row.id,
        "state": row.state.as_str(),
        "service": service_json(),
        "message": "Saved on this server; it sends to Alakazam Labs as soon as it can.",
    }))
}

fn list(c: &Caller, params: &HashMap<String, String>) -> CliResponse {
    let state = match params.get("state").map(String::as_str).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => match State::parse(s) {
            Some(st) => Some(st),
            None => return bad("bad_state", "state is one of queued, sending, sent, failed, refused, cancelled."),
        },
    };
    let limit = match params.get("limit").filter(|s| !s.is_empty()) {
        None => 50,
        Some(l) => match l.parse::<i64>() {
            Ok(n) if (1..=200).contains(&n) => n,
            _ => return bad("bad_limit", "limit is 1 to 200."),
        },
    };
    let db = k2_core::db::shared();
    let rows = {
        let conn = db.lock();
        let sender = if c.admin { None } else { Some(c.sender.as_str()) };
        match store::list(&conn, sender, state, limit) {
            Ok(r) => r,
            Err(e) => return db_err(e),
        }
    };
    let items: Vec<J> = rows.iter().map(|r| row_json(c, r, false)).collect();
    ok(json!({ "items": items, "all": c.admin }))
}

fn find(c: &Caller, id: &str) -> Result<Row, CliResponse> {
    let not_found = || refuse("404 Not Found", "not_found", format!("No feedback {id} you can see on this server."));
    if id.is_empty() {
        return Err(bad("id_required", "Name the feedback id (pf_…)."));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::get(&conn, id) {
        Ok(Some(r)) if visible(c, &r) => Ok(r),
        Ok(_) => Err(not_found()),
        Err(e) => Err(db_err(e)),
    }
}

fn show(c: &Caller, params: &HashMap<String, String>) -> CliResponse {
    let id = params.get("id").map(String::as_str).unwrap_or("").trim();
    match find(c, id) {
        Ok(r) => ok(json!({ "item": row_json(c, &r, true) })),
        Err(r) => r,
    }
}

#[derive(Debug, Default, Deserialize)]
struct IdBody {
    id: Option<String>,
}

fn transition(c: &Caller, body: &[u8], is_retry: bool) -> CliResponse {
    let b: IdBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let id = b.id.as_deref().unwrap_or("").trim().to_string();
    let row = match find(c, &id) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let now = now_ms();
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = if is_retry { store::retry(&conn, &id, now) } else { store::cancel(&conn, &id, now) };
        match r {
            Ok(c) => c,
            Err(e) => return db_err(e),
        }
    };
    if !changed {
        let (want, verb) = if is_retry { ("failed", "retry") } else { ("queued", "cancel") };
        return refuse(
            "409 Conflict",
            "wrong_state",
            format!("{id} is {}; you can {verb} only a {want} one.", row.state.as_str()),
        );
    }
    let st = if is_retry { State::Queued } else { State::Cancelled };
    emit_row(&id, st, None);
    if is_retry {
        drain::kick();
    }
    ok(json!({ "id": id, "state": st.as_str() }))
}

fn settings_get(c: &Caller) -> CliResponse {
    let s = settings::load();
    let hold = drain::hold();
    let agent_off = c.is_agent() && !s.agents;
    let (can_send, why, message) = match (&hold, agent_off) {
        (Some(h), _) => (false, Some(h.code()), Some(h.message())),
        (None, true) => (false, Some("agents_off"), Some("The owner turned off feedback from agents on this server.".to_string())),
        (None, false) => (true, None, None),
    };
    let remember = match &c.username {
        Some(u) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            match store::remember_diagnostics(&conn, u) {
                Ok(v) => json!(v),
                Err(e) => return db_err(e),
            }
        }
        None => J::Null,
    };
    let pending = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::pending_count(&conn).unwrap_or(0)
    };
    ok(json!({
        "enabled": s.enabled && !k2_core::airgap::enabled(),
        "agents": s.agents,
        "agentDiagnostics": s.agent_diagnostics,
        "rememberDiagnostics": remember,
        "canEdit": c.admin && !c.passport,
        "canSend": can_send,
        "why": why,
        "message": message,
        "endpoint": pf::endpoint(),
        "contactAddress": pf::CONTACT_ADDRESS,
        "senderKind": c.kind,
        "pending": pending,
        "service": service_json(),
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsBody {
    enabled: Option<bool>,
    agents: Option<bool>,
    agent_diagnostics: Option<bool>,
    remember_diagnostics: Option<bool>,
}

fn settings_post(c: &Caller, body: &[u8]) -> CliResponse {
    let b: SettingsBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if c.passport {
        return refuse(
            "403 Forbidden",
            "settings_people_only",
            "Only a person signed in to this server changes these settings (the owner or an admin in Settings → General → Send feedback).",
        );
    }
    let server_keys = b.enabled.is_some() || b.agents.is_some() || b.agent_diagnostics.is_some();
    if server_keys && !c.admin {
        return refuse("403 Forbidden", "admin_only", "Only the owner or an admin can change this server's feedback settings.");
    }
    if let Some(on) = b.remember_diagnostics {
        let Some(u) = c.username.as_deref() else {
            return refuse("403 Forbidden", "settings_people_only", "Only a person can remember this choice.");
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = store::set_remember_diagnostics(&conn, u, on) {
            return db_err(e);
        }
    }
    if server_keys {
        let mut s = settings::load();
        if let Some(v) = b.enabled {
            s.enabled = v;
        }
        if let Some(v) = b.agents {
            s.agents = v;
        }
        if let Some(v) = b.agent_diagnostics {
            s.agent_diagnostics = v;
        }
        if let Err(e) = settings::save(&s) {
            return CliResponse::internal_error(format!("save feedback settings: {e}"));
        }
        if s.enabled {
            // Rows held while it was off may go now.
            drain::kick();
        }
    }
    settings_get(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_sets() {
        for p in [SEND, RETRY, CANCEL] {
            assert!(is_post_only(p), "{p}");
        }
        for p in [LIST, SHOW, SETTINGS] {
            assert!(!is_post_only(p), "{p}");
        }
        assert!(!is_route("/cli/feedback/create"));
        assert!(GET_ROUTES.iter().chain(POST_ROUTES).all(|p| p.starts_with("/cli/product-feedback/")));
    }

    fn must(w: Who) -> Caller {
        match caller(&w) {
            Ok(c) => c,
            Err(r) => panic!("refused: {}", r.body),
        }
    }

    #[test]
    fn app_passes_and_api_keys_are_refused() {
        let r = caller(&Who::Wrong).unwrap_err();
        assert_eq!(r.status, "403 Forbidden");
        assert!(r.body.contains("product_feedback_wrong_credential"));
        let a = must(Who::Agent { project_id: "p".into(), session_id: "s".into() });
        assert_eq!((a.kind, a.sender.as_str(), a.passport, a.admin), ("agent", "agent:p", true, false));
        let s = must(Who::Shell { project_id: "p".into(), session_id: "s".into() });
        assert_eq!((s.kind, s.passport), ("person", true));
        let m = must(Who::Login { username: "al".into(), role: Role::Member });
        assert!(!m.admin);
        let a = must(Who::Login { username: "ad".into(), role: Role::Admin });
        assert!(a.admin);
    }
}
