//! App users' personal wallets: the routes (prd-app-user-llm-logins-v1
//! rev 2 §7).
//!
//! **The person's own verbs** (`/cli/skin/wallet/*`, catalog `wallet.*`,
//! cap `wallet:self`): only an App user's own **login session pass**
//! (`k2skn_`, `session=1`) reaches them, and every call keys on that
//! session's principal (there is no username argument). Refused with
//! `person_pass_required`: a Connect login (Rosson #9: wallets belong to
//! App users), the owner token, an agent passport and an App's own pass.
//! The session's App must have its wallet on (`wallet_off`).
//!
//! **The owner's verbs** (`/cli/app/wallet/*`, `k2 app wallet …`): the
//! owner token and Admin or Owner logins see each item's existence,
//! label, provider, type and status (never a secret, URL, code, email,
//! org or usage), force-remove, set personal agents and the per-App
//! settings. An agent (or K2 terminal) of an App's own workspace with
//! switch 2 may read and set that App's **fallback** only.
//!
//! No route here returns a credential. Mutations are POST-only.

use std::collections::HashMap;

use k2_core::llm_accounts::{state, Tool, WalletError};
use k2_core::person_wallet::{self as pw, Item};
use k2_core::skin::SkinPass;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::person_wallet_runtime::{self as prt, audit, person_key, VaultSealer};
use crate::sidecar_routes::Caller;

pub const LIST: &str = "/cli/skin/wallet/list";
pub const STATUS: &str = "/cli/skin/wallet/status";
pub const ADD: &str = "/cli/skin/wallet/add";
pub const LOGIN: &str = "/cli/skin/wallet/login";
pub const LOGIN_STATUS: &str = "/cli/skin/wallet/login/status";
pub const LOGIN_INPUT: &str = "/cli/skin/wallet/login/input";
pub const LOGIN_CANCEL: &str = "/cli/skin/wallet/login/cancel";
pub const USE: &str = "/cli/skin/wallet/use";
pub const REMOVE: &str = "/cli/skin/wallet/remove";

pub const SKIN_GET: &[&str] = &[LIST, STATUS, LOGIN_STATUS];
pub const SKIN_POST: &[&str] = &[ADD, LOGIN, LOGIN_INPUT, LOGIN_CANCEL, USE, REMOVE];

pub const O_SETTINGS: &str = "/cli/app/wallet/settings";
pub const O_PEOPLE: &str = "/cli/app/wallet/people";
pub const O_REMOVE: &str = "/cli/app/wallet/remove";
pub const O_AGENT: &str = "/cli/app/wallet/personal-agent";
pub const O_AGENTS: &str = "/cli/app/wallet/personal-agents";

pub const OWNER_GET: &[&str] = &[O_SETTINGS, O_PEOPLE, O_AGENTS];
pub const OWNER_POST: &[&str] = &[O_SETTINGS, O_REMOVE, O_AGENT];

pub const MAX_BODY: usize = 16 * 1024;

pub fn is_skin_route(p: &str) -> bool {
    SKIN_GET.contains(&p) || SKIN_POST.contains(&p)
}

pub fn is_owner_route(p: &str) -> bool {
    OWNER_GET.contains(&p) || OWNER_POST.contains(&p)
}

/// Shown to a Connect login (and anything else that isn't a person).
pub const PERSON_PASS_HINT: &str = "Wallets belong to App users: sign in to the App as yourself. A Connect login (even an admin) has no wallet; to have one, make yourself an App user account.";

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

fn refuse(code: u16, err: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status: status_line(code),
        content_type: "application/json",
        body: json!({"ok": false, "error": {"code": err, "hint": hint.into()}}).to_string(),
    }
}

fn wallet_err(e: WalletError) -> CliResponse {
    let code: u16 = e.http_status()[..3].parse().unwrap_or(500);
    refuse(code, e.code(), e.to_string())
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

pub fn too_large() -> CliResponse {
    refuse(413, "body_too_large", "request bodies here are at most 16 KiB")
}

pub fn method_not_allowed() -> CliResponse {
    refuse(405, "method_not_allowed", "use POST")
}

pub fn person_pass_required() -> CliResponse {
    refuse(403, "person_pass_required", PERSON_PASS_HINT)
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn)
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Body {
    #[serde(rename = "type")]
    item_type: Option<String>,
    provider: Option<String>,
    label: Option<String>,
    key: Option<String>,
    id: Option<String>,
    login_id: Option<String>,
    text: Option<String>,
    app: Option<String>,
    user: Option<String>,
    workspace: Option<String>,
    clear: Option<bool>,
    enabled: Option<bool>,
    fallback: Option<bool>,
    #[serde(rename = "use")]
    use_id: Option<Value>,
}

fn parse_body(body: &[u8]) -> Result<Body, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(Body::default());
    }
    serde_json::from_slice(body).map_err(|_| refuse(400, "bad_request", "the body must be a JSON object"))
}

fn need<'a>(v: &'a Option<String>, name: &str) -> Result<&'a str, CliResponse> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| refuse(400, "bad_request", format!("{name} is required")))
}

/// The handles of a person's personal agents that run on `item` (its
/// provider's person pin is this item).
fn used_by(conn: &rusqlite::Connection, item: &Item) -> Vec<String> {
    let pinned = pw::use_for(conn, &item.principal_id, &item.provider).ok().flatten().as_deref() == Some(item.id.as_str());
    if !pinned {
        return Vec::new();
    }
    pw::agents_of(conn, &item.principal_id).unwrap_or_default().into_iter().map(|a| a.project_id).collect()
}

fn handles(ids: Vec<String>) -> Vec<String> {
    if ids.is_empty() {
        return Vec::new();
    }
    k2_core::skin::handles_for_project_ids(&ids)
}

/// The person's own view of one item.
fn item_json(item: &Item, in_use: bool, used_by: Vec<String>) -> Value {
    json!({
        "id": item.id,
        "type": item.item_type,
        "provider": item.provider,
        "label": item.label,
        "state": item.state,
        "plan": item.plan,
        "expiresAt": item.expires_at,
        "sealed": item.sealed,
        "addedVia": item.added_via,
        "createdAt": item.created_at,
        "lastUsedAt": item.last_used_at,
        "dormantSince": item.dormant_since,
        "inUse": in_use,
        "usedBy": used_by,
    })
}

/// The owner's view of one item: existence, label, status. Never a
/// secret, URL, code, email, org or usage.
fn owner_json(item: &Item, username: &str, used_by: Vec<String>) -> Value {
    json!({
        "id": item.id,
        "username": username,
        "type": item.item_type,
        "provider": item.provider,
        "label": item.label,
        "state": item.state,
        "sealed": item.sealed,
        "addedVia": item.added_via,
        "dormantSince": item.dormant_since,
        "usedBy": used_by,
    })
}

/// The person's view of one item (handles resolve outside the DB lock).
fn one_item(item: &Item) -> Value {
    let (in_use, ids) = with_conn(|c| {
        let in_use = pw::use_for(c, &item.principal_id, &item.provider).ok().flatten().as_deref() == Some(item.id.as_str());
        (in_use, used_by(c, item))
    });
    item_json(item, in_use, handles(ids))
}

// ── The person's own verbs ─────────────────────────────────────────

/// Who is calling a wallet verb: the person behind a login session pass,
/// on an App whose wallet is on.
struct Me {
    principal_id: String,
    app: String,
    first_name: String,
}

fn me(pass: Option<&SkinPass>) -> Result<Me, CliResponse> {
    let Some(pass) = pass else { return Err(person_pass_required()) };
    let Some(pid) = pass.principal_id.as_deref().filter(|_| pass.session) else {
        return Err(person_pass_required());
    };
    let person = match k2_core::skin::wallet_access::person(pid) {
        Ok(Some(p)) => p,
        Ok(None) => return Err(person_pass_required()),
        Err(e) => return Err(refuse(500, "skin", e)),
    };
    if person.is_agent_login {
        return Err(refuse(403, "not_a_person", "this login belongs to an agent; wallets are for people"));
    }
    let settings = with_conn(|c| pw::app_settings(c, &pass.app)).map_err(wallet_err)?;
    if !settings.enabled {
        return Err(refuse(
            403,
            "wallet_off",
            "this App doesn't use personal wallets; the App's owner can turn it on (k2 app wallet settings <app> --on)",
        ));
    }
    Ok(Me { principal_id: pid.to_string(), app: pass.app.clone(), first_name: person.first_name() })
}

/// Route a person's wallet verb. Blocking (DB, PTY spawn): call from
/// `spawn_blocking`. `pass`: the resolved `k2skn_` pass, `None` for any
/// other credential (Connect login, owner token, passport).
pub fn handle_skin(
    path: &str,
    pass: Option<SkinPass>,
    is_post: bool,
    body: &[u8],
    params: &HashMap<String, String>,
) -> CliResponse {
    if SKIN_POST.contains(&path) && !is_post {
        return method_not_allowed();
    }
    let me = match me(pass.as_ref()) {
        Ok(m) => m,
        Err(r) => {
            audit("wallet.refused", pass.as_ref().map(|p| p.username.as_str()).unwrap_or("-"), path, "app");
            return r;
        }
    };
    let who = person_key(&me.principal_id);
    match path {
        LIST => {
            let res = with_conn(|c| -> Result<_, WalletError> { Ok((pw::list_for(c, &me.principal_id)?, pw::uses_of(c, &me.principal_id)?)) });
            match res {
                Ok((items, uses)) => {
                    let uses: serde_json::Map<String, Value> = uses.into_iter().map(|(p, i)| (p, Value::String(i))).collect();
                    ok(json!({
                        "ok": true,
                        "items": items.iter().map(one_item).collect::<Vec<_>>(),
                        "uses": uses,
                        "providers": {
                            "subscription": Tool::SUBSCRIPTION.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                            "apiKey": Tool::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                        },
                    }))
                }
                Err(e) => wallet_err(e),
            }
        }
        STATUS => {
            let id = params.get("id").map(String::as_str).unwrap_or("").trim().to_string();
            if id.is_empty() {
                return refuse(400, "bad_request", "id is required");
            }
            match with_conn(|c| pw::get_own(c, &me.principal_id, &id)) {
                Ok(item) => ok(json!({"ok": true, "item": one_item(&item)})),
                Err(e) => wallet_err(e),
            }
        }
        LOGIN_STATUS => {
            let id = params.get("loginId").map(String::as_str).unwrap_or("");
            match prt::view(id, &me.principal_id) {
                Some(v) => ok(json!({"ok": true, "login": v})),
                None => refuse(404, "not_found", "no such sign-in (it may have finished more than 10 minutes ago)"),
            }
        }
        _ => handle_skin_post(path, body, &me, &who),
    }
}

/// "<First name> <Tool>" ("Patty Claude"): labels allow letters, digits,
/// spaces and `. _ - @ +` only (no apostrophe), at most 40 characters.
fn default_label(first_name: &str, tool: Tool) -> String {
    let name: String = first_name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '+'))
        .take(24)
        .collect();
    if name.is_empty() {
        format!("My {}", tool.display())
    } else {
        format!("{name} {}", tool.display())
    }
}

fn handle_skin_post(path: &str, body: &[u8], me: &Me, who: &str) -> CliResponse {
    let b = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match path {
        ADD => {
            let provider = match need(&b.provider, "provider") {
                Ok(p) => p.to_string(),
                Err(r) => return r,
            };
            let item_type = b.item_type.clone().unwrap_or_else(|| pw::item_type::LLM_SUBSCRIPTION.to_string());
            let tool = match Tool::parse(&provider) {
                Some(t) => t,
                None => return wallet_err(WalletError::UnknownTool(provider)),
            };
            let label = b
                .label
                .clone()
                .filter(|l| !l.trim().is_empty())
                .unwrap_or_else(|| default_label(&me.first_name, tool));
            if item_type == pw::item_type::LLM_API_KEY {
                let key = match need(&b.key, "key") {
                    Ok(k) => k.to_string(),
                    Err(r) => return r,
                };
                // The key is never logged or echoed.
                return match with_conn(|c| pw::add_api_key(c, &VaultSealer, &me.principal_id, tool.as_str(), &label, &key, Some(&me.app))) {
                    Ok(item) => {
                        audit("wallet.add", who, &format!("{} api_key {}", item.provider, item.id), "app");
                        ok(json!({"ok": true, "item": one_item(&item)}))
                    }
                    Err(e) => wallet_err(e),
                };
            }
            if k2_core::airgap::enabled() {
                return wallet_err(WalletError::AirGap);
            }
            let item = match with_conn(|c| pw::create(c, &me.principal_id, &item_type, tool.as_str(), &label, Some(&me.app))) {
                Ok(i) => i,
                Err(e) => return wallet_err(e),
            };
            match prt::start(&item, true, state::NOT_SET_UP) {
                Ok(v) => {
                    audit("wallet.add", who, &format!("{} subscription {}", item.provider, item.id), "app");
                    let fresh = with_conn(|c| pw::get(c, &item.id)).ok().flatten().unwrap_or(item);
                    ok(json!({"ok": true, "item": one_item(&fresh), "login": v}))
                }
                Err(e) => {
                    let _ = with_conn(|c| pw::discard_unfinished(c, &VaultSealer, &item.id));
                    wallet_err(e)
                }
            }
        }
        LOGIN => {
            let id = match need(&b.id, "id") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            let item = match with_conn(|c| pw::get_own(c, &me.principal_id, &id)) {
                Ok(i) => i,
                Err(e) => return wallet_err(e),
            };
            if k2_core::airgap::enabled() {
                return wallet_err(WalletError::AirGap);
            }
            let prev = item.state.clone();
            match prt::start(&item, false, &prev) {
                Ok(v) => {
                    audit("wallet.login", who, &format!("{} {}", item.provider, item.id), "app");
                    ok(json!({"ok": true, "item": one_item(&item), "login": v}))
                }
                Err(e) => wallet_err(e),
            }
        }
        LOGIN_INPUT => {
            let id = match need(&b.login_id, "loginId") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            match prt::input(&id, b.text.as_deref().unwrap_or(""), &me.principal_id) {
                Ok(()) => ok(json!({"ok": true})),
                Err((c, code, hint)) => refuse(c, code, hint),
            }
        }
        LOGIN_CANCEL => {
            let id = match need(&b.login_id, "loginId") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            match prt::cancel(&id, &me.principal_id) {
                Ok(v) => ok(json!({"ok": true, "login": v})),
                Err((c, code, hint)) => refuse(c, code, hint),
            }
        }
        USE => {
            let provider = match need(&b.provider, "provider") {
                Ok(p) => p.to_string(),
                Err(r) => return r,
            };
            let id = b.id.clone().filter(|s| !s.trim().is_empty());
            match with_conn(|c| pw::set_use(c, &me.principal_id, &provider, id.as_deref())) {
                Ok(()) => {
                    audit("wallet.use", who, &format!("{provider} {}", id.as_deref().unwrap_or("(none)")), "app");
                    ok(json!({"ok": true, "provider": provider, "id": id}))
                }
                Err(e) => wallet_err(e),
            }
        }
        REMOVE => {
            let id = match need(&b.id, "id") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            let item = match with_conn(|c| pw::get_own(c, &me.principal_id, &id)) {
                Ok(i) => i,
                Err(e) => return wallet_err(e),
            };
            match prt::delete_now(&item, "wallet.remove", who, "app") {
                Ok(()) => ok(json!({
                    "ok": true,
                    "removed": item.id,
                    "note": "Deleted from this server. Also sign K2 out on the vendor's website if you want that session gone there too.",
                })),
                Err(e) => wallet_err(e),
            }
        }
        _ => refuse(404, "not_found", "no such wallet route"),
    }
}

// ── The owner's verbs ──────────────────────────────────────────────

enum OwnerWho {
    /// Owner token or an Admin/Owner login: everything.
    Admin(String),
    /// An agent (or K2 terminal) of `project_id`: an App's fallback only,
    /// for Apps of its own workspace with switch 2 on.
    Agent { project_id: String, key: String },
}

fn owner_who(caller: &Caller) -> Result<OwnerWho, CliResponse> {
    use k2_core::connect_users::Role;
    match caller {
        Caller::Owner => Ok(OwnerWho::Admin("owner-token".into())),
        Caller::Login { username } => match k2_core::connect_users::role_for_user(username) {
            Some(r) if r >= Role::Admin => Ok(OwnerWho::Admin(format!("login:{username}"))),
            _ => Err(refuse(403, "admin_required", "people's wallets are managed by an Admin or the Owner")),
        },
        Caller::Agent(a) => Ok(OwnerWho::Agent { project_id: a.project_id.clone(), key: format!("agent:{}", a.project_id) }),
        Caller::Shell { project_id } => Ok(OwnerWho::Agent { project_id: project_id.clone(), key: format!("shell:{project_id}") }),
        Caller::WrongCredential | Caller::Invalid => Err(refuse(403, "forbidden", "Invalid or missing auth token")),
    }
}

/// May an agent of `project_id` manage `service_id` end to end (switch 1
/// and switch 2 on, and the App is its workspace's own)?
pub fn agent_manages_app(project_id: &str, service_id: &str) -> bool {
    with_conn(|c| {
        c.query_row(
            "SELECT 1 FROM apps a JOIN projects p ON p.id = a.owner_workspace \
             WHERE a.service_id = ?1 AND a.owner_workspace = ?2 \
             AND p.agents_can_manage_skin = 1 AND p.agents_can_manage_apps_full = 1",
            rusqlite::params![service_id, project_id],
            |_| Ok(()),
        )
        .is_ok()
    })
}

/// Route an owner wallet verb. Blocking: call from `spawn_blocking`.
pub fn handle_owner(
    path: &str,
    caller: Caller,
    is_post: bool,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
) -> CliResponse {
    if is_post && !OWNER_POST.contains(&path) || !is_post && !OWNER_GET.contains(&path) {
        return method_not_allowed();
    }
    let who = match owner_who(&caller) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let b = if is_post {
        match parse_body(body) {
            Ok(b) => b,
            Err(r) => return r,
        }
    } else {
        Body::default()
    };
    if let OwnerWho::Agent { project_id, key } = &who {
        // Agents: an App's settings only (read; fallback write), for their
        // own Apps with switch 2.
        if path != O_SETTINGS {
            audit("wallet.refused", key, path, ingress);
            return refuse(403, "owner_only", "Agents can only read and set their own App's wallet fallback (with \"…and manage them end to end\" on).");
        }
        let app = if is_post { b.app.clone() } else { params.get("app").cloned() };
        let Some(app) = app.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()) else {
            return refuse(400, "bad_request", "app is required");
        };
        if !agent_manages_app(project_id, &app) {
            audit("wallet.refused", key, &format!("{path} {app}"), ingress);
            return refuse(403, "switch2_required", "Only an agent whose workspace owns this App, with \"…and manage them end to end\" on, may set its wallet fallback.");
        }
        if is_post && b.enabled.is_some() {
            return refuse(403, "owner_only", "Turning an App's wallet on or off is the Owner's or an Admin's; agents set the fallback only.");
        }
        return settings(&app, is_post, None, b.fallback, key, ingress);
    }
    let OwnerWho::Admin(key) = who else { unreachable!() };
    match path {
        O_SETTINGS => {
            let app = if is_post { b.app.clone() } else { params.get("app").cloned() };
            let Some(app) = app.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()) else {
                return refuse(400, "bad_request", "app is required");
            };
            settings(&app, is_post, b.enabled, b.fallback, &key, ingress)
        }
        O_PEOPLE => people(params.get("app").map(String::as_str)),
        O_AGENTS => {
            let marks = match with_conn(|c| pw::person_agents(c)) {
                Ok(m) => m,
                Err(e) => return wallet_err(e),
            };
            let rows: Vec<Value> = marks
                .iter()
                .map(|m| {
                    let user = k2_core::skin::wallet_access::person(&m.principal_id).ok().flatten().map(|p| p.username);
                    json!({
                        "workspace": m.project_id,
                        "handle": handles(vec![m.project_id.clone()]).into_iter().next(),
                        "username": user,
                        "app": m.service_id,
                        "setBy": m.set_by,
                        "setAt": m.set_at,
                    })
                })
                .collect();
            ok(json!({"ok": true, "agents": rows}))
        }
        O_REMOVE => {
            let id = match need(&b.id, "id") {
                Ok(i) => i.to_string(),
                Err(r) => return r,
            };
            let item = match with_conn(|c| pw::get(c, &id)) {
                Ok(Some(i)) => i,
                Ok(None) => return wallet_err(WalletError::NotFound(id)),
                Err(e) => return wallet_err(e),
            };
            match prt::delete_now(&item, "wallet.force_remove", &key, ingress) {
                Ok(()) => ok(json!({"ok": true, "removed": item.id})),
                Err(e) => wallet_err(e),
            }
        }
        O_AGENT => personal_agent(&b, &key, ingress),
        _ => refuse(404, "not_found", "no such wallet route"),
    }
}

fn settings(app: &str, is_post: bool, enabled: Option<bool>, fallback: Option<bool>, key: &str, ingress: &str) -> CliResponse {
    if !is_post {
        return match with_conn(|c| pw::app_settings(c, app)) {
            Ok(s) => ok(json!({"ok": true, "settings": s})),
            Err(e) => wallet_err(e),
        };
    }
    if enabled.is_none() && fallback.is_none() {
        return refuse(400, "bad_request", "set enabled and/or fallback");
    }
    match with_conn(|c| pw::set_app_settings(c, app, enabled, fallback, key)) {
        Ok((before, after)) => {
            audit(
                "wallet.settings",
                key,
                &format!("{app} enabled {}→{} fallback {}→{}", before.enabled, after.enabled, before.fallback, after.fallback),
                ingress,
            );
            ok(json!({"ok": true, "settings": after}))
        }
        Err(e) => wallet_err(e),
    }
}

fn people(app: Option<&str>) -> CliResponse {
    let items = match with_conn(|c| pw::list_all(c)) {
        Ok(i) => i,
        Err(e) => return wallet_err(e),
    };
    let mut rows = Vec::new();
    let mut cache: HashMap<String, Option<k2_core::skin::wallet_access::Person>> = HashMap::new();
    for item in &items {
        let person = cache
            .entry(item.principal_id.clone())
            .or_insert_with(|| k2_core::skin::wallet_access::person(&item.principal_id).ok().flatten())
            .clone();
        if let Some(a) = app.map(str::trim).filter(|a| !a.is_empty()) {
            let via = item.added_via.as_deref() == Some(a);
            if !via && !person.as_ref().is_some_and(|p| p.has_app(a)) {
                continue;
            }
        }
        let ids = with_conn(|c| used_by(c, item));
        let username = person.as_ref().map(|p| p.username.clone()).unwrap_or_else(|| "(gone)".into());
        rows.push(owner_json(item, &username, handles(ids)));
    }
    ok(json!({"ok": true, "items": rows}))
}

#[cfg(test)]
#[path = "person_wallet_tests.rs"]
mod tests;

fn personal_agent(b: &Body, key: &str, ingress: &str) -> CliResponse {
    let ws = match need(&b.workspace, "workspace") {
        Ok(w) => w.to_string(),
        Err(r) => return r,
    };
    let project_id = match k2_core::skin::resolve_room_tokens(&[ws.clone()]) {
        Ok(v) if v.len() == 1 => v[0].clone(),
        _ => return refuse(404, "not_found", format!("no workspace {ws:?}")),
    };
    if b.clear == Some(true) {
        return match with_conn(|c| pw::set_person_agent(c, &project_id, None, Some(key))) {
            Ok(changed) => {
                audit("wallet.personal_agent", key, &format!("clear {project_id}"), ingress);
                ok(json!({"ok": true, "cleared": changed, "workspace": project_id}))
            }
            Err(e) => wallet_err(e),
        };
    }
    let app = match need(&b.app, "app") {
        Ok(a) => a.to_string(),
        Err(r) => return r,
    };
    let user = match need(&b.user, "user") {
        Ok(u) => u.to_string(),
        Err(r) => return r,
    };
    let person = match k2_core::skin::wallet_access::person_by_username(&user) {
        Ok(Some(p)) if !p.is_agent_login => p,
        Ok(Some(_)) => return refuse(409, "not_a_person", format!("{user} is an agent's login, not a person")),
        _ => return refuse(404, "not_found", format!("no App user {user:?}")),
    };
    if !person.holds_room_via(&app, &project_id) {
        return refuse(
            409,
            "room_not_granted",
            format!("{user} doesn't have this agent's room in {app}; grant it first (k2 app guest rooms {app} {user} …)"),
        );
    }
    match with_conn(|c| pw::set_person_agent(c, &project_id, Some((&person.principal_id, &app)), Some(key))) {
        Ok(_) => {
            audit("wallet.personal_agent", key, &format!("{project_id} → {} via {app}", person_key(&person.principal_id)), ingress);
            ok(json!({"ok": true, "workspace": project_id, "username": person.username, "app": app}))
        }
        Err(e) => wallet_err(e),
    }
}
