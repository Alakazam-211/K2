//! `/cli/llm/accounts/*` — the LLM login wallet (Settings → LLMs,
//! `k2 llm accounts`).
//!
//! Callers:
//! - the owner token and Connect logins (Member+, the `/cli/agents/*`
//!   floor) may do everything;
//! - a session passport — an agent tab **and** a K2 shell tab (Rosson:
//!   a K2 terminal has its workspace agent's permissions) — is read-only:
//!   `list`, `status`, `usage`, and sees no email / org;
//! - app passes and anything else: refused.
//!
//! No route ever returns a token, a refresh token or a credential file.
//! Mutations are POST-only (`require_post` in the dispatcher arm and
//! `route_policy` rows).

use std::collections::HashMap;

use k2_core::llm_accounts::{self as wallet_core, state, wallet, Entry, Tool, WalletError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::llm_accounts_runtime as rt;
use crate::sidecar_routes::Caller;

pub const LIST: &str = "/cli/llm/accounts/list";
pub const STATUS: &str = "/cli/llm/accounts/status";
pub const USAGE: &str = "/cli/llm/accounts/usage";
pub const LOGIN_STATUS: &str = "/cli/llm/accounts/login/status";
pub const ADD: &str = "/cli/llm/accounts/add";
pub const LOGIN: &str = "/cli/llm/accounts/login";
pub const LOGIN_INPUT: &str = "/cli/llm/accounts/login/input";
pub const LOGIN_CANCEL: &str = "/cli/llm/accounts/login/cancel";
pub const SWITCH: &str = "/cli/llm/accounts/switch";
pub const NEXT: &str = "/cli/llm/accounts/next";
pub const RENAME: &str = "/cli/llm/accounts/rename";
pub const REMOVE: &str = "/cli/llm/accounts/remove";
pub const REFRESH: &str = "/cli/llm/accounts/refresh";
pub const PINS: &str = "/cli/llm/accounts/pins";
pub const PIN: &str = "/cli/llm/accounts/pin";
pub const UNPIN: &str = "/cli/llm/accounts/unpin";
pub const ADD_KEY: &str = "/cli/llm/accounts/add-key";

/// Read routes (passports allowed).
pub const READ_ROUTES: &[&str] = &[LIST, STATUS, USAGE, LOGIN_STATUS, PINS];
/// POST-only routes (humans only).
pub const POST_ROUTES: &[&str] = &[
    ADD, LOGIN, LOGIN_INPUT, LOGIN_CANCEL, SWITCH, NEXT, RENAME, REMOVE, REFRESH, PIN, UNPIN, ADD_KEY,
];

/// Request bodies here are small JSON objects.
pub const MAX_BODY: usize = 16 * 1024;

pub fn is_route(path: &str) -> bool {
    READ_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

enum Who {
    /// Owner token or a Connect login: full access. The key identifies
    /// the starter of a login terminal.
    Human { key: String },
    /// Agent or K2 shell tab passport: read-only.
    Passport,
}

fn who(caller: &Caller) -> Option<Who> {
    match caller {
        Caller::Owner => Some(Who::Human { key: "owner-token".into() }),
        Caller::Login { username } => Some(Who::Human { key: format!("login:{username}") }),
        Caller::Shell { .. } | Caller::Agent(_) => Some(Who::Passport),
        Caller::WrongCredential | Caller::Invalid => None,
    }
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        422 => "422 Unprocessable Entity",
        _ => "500 Internal Server Error",
    }
}

fn refuse(code: u16, err_code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status: status_line(code),
        content_type: "application/json",
        body: json!({"error": {"code": err_code, "hint": hint.into()}}).to_string(),
    }
}

fn wallet_err(e: WalletError) -> CliResponse {
    let code: u16 = e.http_status()[..3].parse().unwrap_or(500);
    refuse(code, e.code(), e.to_string())
}

pub fn too_large() -> CliResponse {
    refuse(413, "body_too_large", "request bodies here are at most 16 KiB")
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn audit(event: &str, user: &str, outcome: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        user,
        outcome.to_string(),
        ingress,
        "-",
        "cli",
    ));
}

// ── JSON shapes ─────────────────────────────────────────────────────

fn usage_for(e: &Entry, active: bool) -> Value {
    if active {
        return crate::subscription_usage::cached_row(&e.tool)
            .and_then(|r| serde_json::to_value(r).ok())
            .unwrap_or(Value::Null);
    }
    e.usage_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null)
}

fn pin_json(conn: &rusqlite::Connection, p: &k2_core::llm_accounts::pins::Pin) -> Value {
    let label = if p.scope_kind == "workspace" {
        conn.query_row(
            "SELECT COALESCE(NULLIF(handle, ''), name) FROM projects WHERE id = ?1",
            rusqlite::params![p.scope_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| p.scope_id.clone())
    } else {
        p.scope_id.clone()
    };
    json!({
        "scopeKind": p.scope_kind,
        "scopeId": p.scope_id,
        "tool": p.tool,
        "accountId": p.account_id,
        "label": label,
    })
}

fn pinned_to(e: &Entry) -> Value {
    with_conn(|conn| {
        let pins = k2_core::llm_accounts::pins::pins_for(conn, &e.id).unwrap_or_default();
        Value::Array(pins.iter().map(|p| pin_json(conn, p)).collect())
    })
}

fn account_json(e: &Entry, active: bool, human: bool) -> Value {
    let api = e.is_api_key();
    json!({
        "id": e.id,
        "tool": e.tool,
        "label": e.label,
        "kind": e.kind,
        "billedPerToken": api,
        "pinnedTo": pinned_to(e),
        "inUse": !api && rt::slot_in_use(&e.id),
        "active": active,
        "state": e.state,
        "detail": e.detail,
        "email": if human { json!(e.email) } else { Value::Null },
        "org": if human { json!(e.org) } else { Value::Null },
        "plan": e.plan,
        "expiresAt": e.expires_at,
        "refreshedAt": e.refreshed_at,
        "lastUsedAt": e.last_used_at,
        "createdAt": e.created_at,
        "createdBy": e.created_by,
        "usage": usage_for(e, active),
        "usageCheckedAt": if active { Value::Null } else { json!(e.usage_checked_at) },
    })
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn)
}

fn list_doc(human: bool, who_key: Option<&str>) -> Result<Value, WalletError> {
    let mut tools = Vec::new();
    let all_pins = with_conn(|conn| -> Result<Vec<Value>, WalletError> {
        Ok(k2_core::llm_accounts::pins::list_pins(conn)?.iter().map(|p| pin_json(conn, p)).collect())
    })?;
    let accounts_by_tool = with_conn(|conn| -> Result<Vec<(Tool, Option<String>, Option<String>, Vec<Entry>)>, WalletError> {
        let mut v = Vec::new();
        for tool in Tool::ALL {
            v.push((
                tool,
                wallet_core::active_id(conn, tool)?,
                wallet_core::live_owner(conn, tool)?,
                wallet_core::list_for(conn, tool)?,
            ));
        }
        Ok(v)
    })?;
    for (tool, active, live, entries) in accounts_by_tool {
        let accounts: Vec<Value> = entries
            .iter()
            .map(|e| account_json(e, active.as_deref() == Some(e.id.as_str()), human))
            .collect();
        let pins: Vec<Value> = all_pins.iter().filter(|p| p["tool"] == tool.as_str()).cloned().collect();
        tools.push(json!({
            "tool": tool.as_str(),
            "display": tool.display(),
            "supported": true,
            "subscription": tool.subscription_supported(),
            "apiKeys": true,
            "activeId": active,
            "liveAccountId": live,
            "loginMethod": if tool.subscription_supported() { json!(wallet::login_method(tool)) } else { Value::Null },
            "accounts": accounts,
            "pins": pins,
        }));
    }
    for (id, display) in wallet_core::NOT_YET {
        tools.push(json!({
            "tool": id, "display": display, "supported": false, "subscription": false, "apiKeys": false,
            "activeId": Value::Null, "liveAccountId": Value::Null, "loginMethod": Value::Null, "accounts": [], "pins": [],
        }));
    }
    let logins: Vec<Value> = match who_key {
        Some(k) if human => rt::views(&k.to_string()).iter().map(rt::login_json).collect(),
        _ => Vec::new(),
    };
    Ok(json!({
        "tools": tools,
        "logins": logins,
        "airgap": k2_core::airgap::enabled(),
        "switchNote": rt::SWITCH_NOTE,
    }))
}

fn entry_json(id: &str, human: bool) -> Result<Value, WalletError> {
    with_conn(|conn| {
        let e = wallet_core::lookup(conn, None, id)?;
        let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
        let active = wallet_core::active_id(conn, tool)?.as_deref() == Some(e.id.as_str());
        Ok(account_json(&e, active, human))
    })
}

// ── Bodies ──────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Body {
    tool: Option<String>,
    label: Option<String>,
    id: Option<String>,
    mode: Option<String>,
    login_id: Option<String>,
    text: Option<String>,
    usage: Option<bool>,
    scope: Option<String>,
    scope_id: Option<String>,
    workspace: Option<String>,
    session: Option<String>,
    key: Option<String>,
}

/// A workspace by projects.id, path, handle or name.
fn resolve_workspace(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM projects WHERE id = ?1 OR path = ?1 OR handle = ?1 OR name = ?1 \
         ORDER BY CASE WHEN id = ?1 THEN 0 WHEN path = ?1 THEN 1 WHEN handle = ?1 THEN 2 ELSE 3 END LIMIT 1",
        rusqlite::params![key.trim().trim_end_matches('/')],
        |r| r.get(0),
    )
    .ok()
}

/// The pin scope from a body: `{scope:"workspace", scopeId|workspace}` or
/// `{scope:"session", scopeId|session}` (a session's v2 key; the
/// canonical chat's key is its workspace id).
fn scope_of(b: &Body) -> Result<(k2_core::llm_accounts::pins::ScopeKind, String), CliResponse> {
    use k2_core::llm_accounts::pins::ScopeKind;
    let kind = match (b.scope.as_deref(), &b.workspace, &b.session) {
        (Some(s), _, _) => ScopeKind::parse(s)
            .ok_or_else(|| refuse(400, "bad_request", "scope is workspace or session"))?,
        (None, Some(_), _) => ScopeKind::Workspace,
        (None, None, Some(_)) => ScopeKind::Session,
        _ => return Err(refuse(400, "bad_request", "scope (workspace or session) is required")),
    };
    let raw = b
        .scope_id
        .clone()
        .or_else(|| if kind == ScopeKind::Workspace { b.workspace.clone() } else { b.session.clone() })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| refuse(400, "bad_request", "scopeId is required"))?;
    if kind == ScopeKind::Workspace {
        let id = with_conn(|conn| resolve_workspace(conn, &raw))
            .ok_or_else(|| refuse(404, "not_found", format!("no workspace {raw}")))?;
        return Ok((kind, id));
    }
    Ok((kind, raw))
}

fn parse_body(body: &[u8]) -> Result<Body, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(Body::default());
    }
    serde_json::from_slice::<Body>(body).map_err(|_| refuse(400, "bad_request", "the body must be a JSON object"))
}

fn need<'a>(v: &'a Option<String>, name: &str) -> Result<&'a str, CliResponse> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| refuse(400, "bad_request", format!("{name} is required")))
}

fn tool_of(v: &Option<String>) -> Result<Tool, CliResponse> {
    let t = need(v, "tool")?;
    Tool::parse(t).ok_or_else(|| wallet_err(WalletError::UnknownTool(t.to_string())))
}

fn opt_tool(v: &Option<String>) -> Result<Option<Tool>, CliResponse> {
    match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(t) => Tool::parse(t)
            .map(Some)
            .ok_or_else(|| wallet_err(WalletError::UnknownTool(t.to_string()))),
    }
}

// ── Entry ───────────────────────────────────────────────────────────

/// Route a wallet request. Blocking (DB, file IO, PTY spawn): call from
/// `spawn_blocking`. `params` are the query params.
pub fn handle(
    path: &str,
    caller: Caller,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
) -> CliResponse {
    let Some(w) = who(&caller) else {
        return refuse(403, "forbidden", "Invalid or missing auth token");
    };
    let (human, key) = match &w {
        Who::Human { key } => (true, Some(key.clone())),
        Who::Passport => (false, None),
    };
    if is_post_route(path) && !human {
        audit("llm_accounts.refused", "passport", path, ingress);
        return refuse(
            403,
            "owner_only",
            "Agents and K2 terminals can only read logins. Change logins on Settings → LLMs, or run k2 from a terminal outside K2.",
        );
    }
    match path {
        LIST => match list_doc(human, key.as_deref()) {
            Ok(v) => ok(v),
            Err(e) => wallet_err(e),
        },
        STATUS => {
            let id = params.get("id").map(String::as_str).unwrap_or("").trim();
            if id.is_empty() {
                return refuse(400, "bad_request", "id is required");
            }
            let tool = match opt_tool(&params.get("tool").cloned()) {
                Ok(t) => t,
                Err(r) => return r,
            };
            let res = with_conn(|conn| -> Result<Value, WalletError> {
                let e = wallet_core::lookup(conn, tool, id)?;
                let t = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
                let active = wallet_core::active_id(conn, t)?.as_deref() == Some(e.id.as_str());
                Ok(account_json(&e, active, human))
            });
            match res {
                Ok(a) => ok(json!({"account": a})),
                Err(e) => wallet_err(e),
            }
        }
        USAGE => {
            let res = with_conn(|conn| -> Result<Vec<Value>, WalletError> {
                let mut rows = Vec::new();
                for tool in Tool::ALL {
                    let active = wallet_core::active_id(conn, tool)?;
                    for e in wallet_core::list_for(conn, tool)? {
                        let is_active = active.as_deref() == Some(e.id.as_str());
                        rows.push(json!({
                            "accountId": e.id, "tool": e.tool, "label": e.label,
                            "active": is_active, "usage": usage_for(&e, is_active),
                        }));
                    }
                }
                Ok(rows)
            });
            match res {
                Ok(rows) => ok(json!({"rows": rows})),
                Err(e) => wallet_err(e),
            }
        }
        PINS => {
            let res = with_conn(|conn| -> Result<Vec<Value>, WalletError> {
                Ok(k2_core::llm_accounts::pins::list_pins(conn)?.iter().map(|p| pin_json(conn, p)).collect())
            });
            match res {
                Ok(p) => ok(json!({"pins": p})),
                Err(e) => wallet_err(e),
            }
        }
        LOGIN_STATUS => {
            if !human {
                return refuse(403, "owner_only", "sign-in terminals are visible to people only");
            }
            let id = params.get("loginId").map(String::as_str).unwrap_or("");
            match rt::view(id, key.as_ref().expect("human key")) {
                Some(v) => ok(json!({"login": rt::login_json(&v)})),
                None => refuse(404, "not_found", "no such sign-in (it may have finished more than 10 minutes ago)"),
            }
        }
        _ => handle_post(path, body, key.expect("human key"), ingress),
    }
}

fn handle_post(path: &str, body: &[u8], key: String, ingress: &str) -> CliResponse {
    let b = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match path {
        ADD => {
            let tool = match tool_of(&b.tool) {
                Ok(t) => t,
                Err(r) => return r,
            };
            let label = match need(&b.label, "label") {
                Ok(l) => l.to_string(),
                Err(r) => return r,
            };
            if k2_core::airgap::enabled() {
                return wallet_err(WalletError::AirGap);
            }
            let entry = match with_conn(|conn| wallet::begin_new(conn, tool, &label, Some(&key))) {
                Ok(e) => e,
                Err(e) => return wallet_err(e),
            };
            let mode = rt::LoginMode::parse(b.mode.as_deref());
            match rt::start_login(&entry, true, state::NOT_SET_UP, mode, &key) {
                Ok(v) => {
                    audit("llm_accounts.add", &key, tool.as_str(), ingress);
                    let account = entry_json(&entry.id, true).unwrap_or(Value::Null);
                    ok(json!({"account": account, "login": rt::login_json(&v)}))
                }
                Err(e) => {
                    let _ = with_conn(|conn| wallet::discard_unfinished(conn, &entry.id));
                    wallet_err(e)
                }
            }
        }
        LOGIN => {
            let id = match need(&b.id, "id") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            if k2_core::airgap::enabled() {
                return wallet_err(WalletError::AirGap);
            }
            if let Some(running) = rt::running_for(&id) {
                if let Some(v) = rt::view(&running, &key) {
                    let account = entry_json(&id, true).unwrap_or(Value::Null);
                    return ok(json!({"account": account, "login": rt::login_json(&v)}));
                }
            }
            let prev = match with_conn(|conn| wallet_core::lookup(conn, None, &id)) {
                Ok(e) => e.state,
                Err(e) => return wallet_err(e),
            };
            let entry = match with_conn(|conn| wallet::begin_relogin(conn, &id)) {
                Ok(e) => e,
                Err(e) => return wallet_err(e),
            };
            let mode = rt::LoginMode::parse(b.mode.as_deref());
            match rt::start_login(&entry, false, &prev, mode, &key) {
                Ok(v) => {
                    audit("llm_accounts.login", &key, &entry.tool, ingress);
                    let account = entry_json(&entry.id, true).unwrap_or(Value::Null);
                    ok(json!({"account": account, "login": rt::login_json(&v)}))
                }
                Err(e) => {
                    let _ = with_conn(|conn| {
                        wallet_core::update_meta(conn, &entry.id, &wallet_core::MetaUpdate { state: Some(prev.clone()), ..Default::default() })
                    });
                    wallet_err(e)
                }
            }
        }
        LOGIN_INPUT => {
            let id = match need(&b.login_id, "loginId") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            let text = b.text.unwrap_or_default();
            // The pasted text is never logged or stored.
            match rt::input(&id, &text, &key) {
                Ok(()) => ok(json!({"ok": true})),
                Err((code, c, hint)) => refuse(code, c, hint),
            }
        }
        LOGIN_CANCEL => {
            let id = match need(&b.login_id, "loginId") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            match rt::cancel(&id, &key) {
                Ok(v) => ok(json!({"login": rt::login_json(&v)})),
                Err((code, c, hint)) => refuse(code, c, hint),
            }
        }
        SWITCH | NEXT => {
            let res = with_conn(|conn| -> Result<(wallet::SwitchOutcome, Tool), WalletError> {
                let (tool, target) = if path == NEXT {
                    let tool = Tool::parse(b.tool.as_deref().unwrap_or(""))
                        .ok_or_else(|| WalletError::UnknownTool(b.tool.clone().unwrap_or_default()))?;
                    let next = wallet_core::next_signed_in(conn, tool)?
                        .ok_or_else(|| WalletError::NotFound("no_next".into()))?;
                    (tool, next.id)
                } else {
                    let id = b.id.clone().unwrap_or_default();
                    let tool_hint = match b.tool.as_deref() {
                        Some(t) if !t.trim().is_empty() => {
                            Some(Tool::parse(t).ok_or_else(|| WalletError::UnknownTool(t.to_string()))?)
                        }
                        _ => None,
                    };
                    let e = wallet_core::lookup(conn, tool_hint, &id)?;
                    let tool = e.tool().ok_or_else(|| WalletError::UnknownTool(e.tool.clone()))?;
                    (tool, e.id)
                };
                let out = wallet::switch(conn, tool, &target, Some(&key), Some(&rt::DaemonRefresher))?;
                Ok((out, tool))
            });
            match res {
                Ok((out, tool)) => {
                    audit("llm_accounts.switch", &key, tool.as_str(), ingress);
                    crate::session_events::emit_llm_accounts_changed(Some(tool.as_str()));
                    let account = entry_json(&out.to, true).unwrap_or(Value::Null);
                    ok(json!({"switch": out, "account": account, "note": rt::SWITCH_NOTE}))
                }
                Err(WalletError::NotFound(k)) if k == "no_next" => refuse(
                    404,
                    "no_next",
                    "there is no other signed-in login for that tool; add one on Settings → LLMs",
                ),
                Err(e) => wallet_err(e),
            }
        }
        ADD_KEY => {
            let tool = match tool_of(&b.tool) {
                Ok(t) => t,
                Err(r) => return r,
            };
            let label = match need(&b.label, "label") {
                Ok(l) => l.to_string(),
                Err(r) => return r,
            };
            // The key is never logged, echoed or returned.
            let key_text = b.key.clone().unwrap_or_default();
            match with_conn(|conn| k2_core::llm_accounts::pins::add_api_key(conn, tool, &label, &key_text, Some(&key))) {
                Ok(e) => {
                    audit("llm_accounts.add_key", &key, tool.as_str(), ingress);
                    crate::session_events::emit_llm_accounts_changed(Some(tool.as_str()));
                    ok(json!({"account": entry_json(&e.id, true).unwrap_or(Value::Null)}))
                }
                Err(e) => wallet_err(e),
            }
        }
        PIN => {
            let (kind, scope_id) = match scope_of(&b) {
                Ok(s) => s,
                Err(r) => return r,
            };
            let id = match need(&b.id, "id") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            let tool = match opt_tool(&b.tool) {
                Ok(t) => t,
                Err(r) => return r,
            };
            let res = with_conn(|conn| {
                k2_core::llm_accounts::pins::pin(conn, kind, &scope_id, tool, &id, Some(&key))
                    .map(|p| pin_json(conn, &p))
            });
            match res {
                Ok(p) => {
                    audit("llm_accounts.pin", &key, &format!("{}:{}", kind.as_str(), p["tool"].as_str().unwrap_or("")), ingress);
                    crate::session_events::emit_llm_accounts_changed(p["tool"].as_str());
                    let note = if p["tool"] == "claude" {
                        "This agent's Claude history will live with this login. New sessions use it; a resumed conversation keeps the login it started on."
                    } else {
                        "New sessions use this login; a resumed conversation keeps the login it started on."
                    };
                    ok(json!({"pin": p, "note": note}))
                }
                Err(e) => wallet_err(e),
            }
        }
        UNPIN => {
            let (kind, scope_id) = match scope_of(&b) {
                Ok(s) => s,
                Err(r) => return r,
            };
            let tool = match tool_of(&b.tool) {
                Ok(t) => t,
                Err(r) => return r,
            };
            match with_conn(|conn| k2_core::llm_accounts::pins::unpin(conn, kind, &scope_id, tool)) {
                Ok(removed) => {
                    audit("llm_accounts.unpin", &key, tool.as_str(), ingress);
                    crate::session_events::emit_llm_accounts_changed(Some(tool.as_str()));
                    ok(json!({"unpinned": removed}))
                }
                Err(e) => wallet_err(e),
            }
        }
        RENAME => {
            let id = match need(&b.id, "id") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            let label = match need(&b.label, "label") {
                Ok(l) => l.to_string(),
                Err(r) => return r,
            };
            match with_conn(|conn| wallet_core::rename(conn, &id, &label)) {
                Ok(e) => {
                    crate::session_events::emit_llm_accounts_changed(Some(&e.tool));
                    ok(json!({"account": entry_json(&e.id, true).unwrap_or(Value::Null)}))
                }
                Err(e) => wallet_err(e),
            }
        }
        REMOVE => {
            let id = match need(&b.id, "id") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            if rt::running_for(&id).is_some() {
                return refuse(409, "busy", "a sign-in is running for that login; cancel it first");
            }
            match with_conn(|conn| wallet::remove(conn, &id)) {
                Ok(out) => {
                    audit("llm_accounts.remove", &key, &out.id, ingress);
                    crate::session_events::emit_llm_accounts_changed(None);
                    ok(json!({"removed": out}))
                }
                Err(e) => wallet_err(e),
            }
        }
        REFRESH => {
            let ids: Vec<String> = match b.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(id) => match with_conn(|conn| wallet_core::lookup(conn, opt_tool(&b.tool).ok().flatten(), id)) {
                    Ok(e) => vec![e.id],
                    Err(e) => return wallet_err(e),
                },
                None => with_conn(|conn| wallet_core::list(conn))
                    .map(|v| v.into_iter().map(|e| e.id).collect())
                    .unwrap_or_default(),
            };
            let mut out = Vec::new();
            for id in &ids {
                if rt::running_for(id).is_some() {
                    continue;
                }
                let e = match with_conn(|conn| wallet::recheck(conn, id)) {
                    Ok(e) => e,
                    Err(e) => return wallet_err(e),
                };
                if b.usage.unwrap_or(false) && e.state == state::SIGNED_IN {
                    if let Err(err) = rt::probe_idle_usage(&e) {
                        k2_core::log_debug!("[llm-accounts] usage {}: {err}", e.id);
                    }
                }
            }
            if b.usage.unwrap_or(false) && !k2_core::airgap::enabled() {
                // The active logins' row is the shared cache (top-bar chip).
                let _ = crate::subscription_usage::handle_refresh();
            }
            for id in &ids {
                if let Ok(v) = entry_json(id, true) {
                    out.push(v);
                }
            }
            crate::session_events::emit_llm_accounts_changed(None);
            ok(json!({"accounts": out}))
        }
        _ => CliResponse::not_found(),
    }
}
