//! `/cli/notify/*` — Thread push prefs (prd-thread-push-notifications-v1
//! §5, slice S2).
//!
//! | Method | Path | Floor | Acts on |
//! |---|---|---|---|
//! | GET  | `/cli/notify/prefs`     | Member | the caller's person key |
//! | POST | `/cli/notify/prefs`     | Member | the caller's person key |
//! | GET  | `/cli/notify/admin`     | Admin  | everyone + the workspace defaults |
//! | POST | `/cli/notify/admin/set` | Admin  | a workspace default (Admin+), or another person's row (Owner only) |
//! | GET  | `/cli/notify/email/address` | Member | the caller's notice address (`?person=`: Admin+), `email_notify::verify` |
//! | POST | `/cli/notify/email/address/set` | Member | the caller's own: sends a confirm link |
//! | POST | `/cli/notify/email/address/remove` | Member | the caller's own; `person` = someone else's (Admin+) |
//! | GET  | `/cli/people/color`     | Member | the palette, the caller's colour, everyone's effective colour |
//! | POST | `/cli/people/color`     | Member | the caller's own colour; `user` = someone else's (Admin+, see below) |
//! | GET  | `/cli/people/name`      | Member | the caller's (or `?person=`'s) names + everyone's |
//! | POST | `/cli/people/name`      | Member | the caller's own names; `person` = someone else's (Admin+, same floors as colour) |
//!
//! Identity comes from the dispatcher arm (owner token → `owner`, a live
//! Connect session → its username), never the body (TP20). Agent
//! passports and app passes fail that arm's `token_ok` and get a 403.
//! There are no `?user=` params: floors are per path (TP19).
//!
//! **Admins are read-only on other people (Rosson 2026-10-11):** an Admin
//! sees everyone on `GET /cli/notify/admin` and may set workspace
//! defaults, but only the Owner (owner token or an Owner-role login) may
//! change another person's switches.
//!
//! Every successful write emits the payload-free `notify_prefs_changed`
//! app event (TP21); clients refetch their own view.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::notify_prefs::{self, Effective, People, PrefsSnapshot, DEFAULT_PRINCIPAL, OWNER};
use rusqlite::{params, Connection};

use crate::cli_response::CliResponse;

pub const PREFS_PATH: &str = "/cli/notify/prefs";
pub const ADMIN_PATH: &str = "/cli/notify/admin";
pub const ADMIN_SET_PATH: &str = "/cli/notify/admin/set";
pub const OWNER_ACCOUNTS_PATH: &str = "/cli/owner/accounts";
pub const PEOPLE_COLOR_PATH: &str = "/cli/people/color";
pub const PEOPLE_NAME_PATH: &str = "/cli/people/name";
/// 0.46.0 "Your account" setup prompt (`k2_core::owner_account_prompt`).
pub const OWNER_ACCOUNT_PROMPT_PATH: &str = "/cli/owner/account-prompt";
pub const OWNER_ACCOUNT_PROMPT_DISMISS_PATH: &str = "/cli/owner/account-prompt/dismiss";

/// The acting login, resolved by the dispatcher from the request token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// `"owner"` for the owner daemon token, else the Connect username.
    pub login: String,
    /// The request carried the owner daemon token.
    pub owner_token: bool,
}

impl Actor {
    pub fn owner_token() -> Self {
        Self { login: OWNER.to_string(), owner_token: true }
    }

    pub fn login(username: &str) -> Self {
        Self { login: username.to_string(), owner_token: false }
    }
}

/// The caller's person key: the owner token is the owner; a login is the
/// owner only when the owner linked it, else itself. Never a username
/// match: a reserved-name login (`owner`, made before 0.46.6) is nobody
/// (`None`).
pub fn actor_person(people: &People, actor: &Actor) -> Option<String> {
    if actor.owner_token {
        Some(OWNER.to_string())
    } else {
        people.login_person_key(&actor.login)
    }
}

fn reserved_login_refusal() -> CliResponse {
    error(
        "403 Forbidden",
        "reserved_login",
        "this login's name is one K2 uses for the server's owner and its agents, so it can't use notices or profiles; the owner can remove it in Settings → Admin Access and add the person under another name",
    )
}

/// GET chain (`crate::cli::dispatch`): no identity reaches it, so the
/// reads refuse and the POST-only path answers 405. The real traffic
/// takes the dispatcher arm, which resolves the caller first.
pub fn dispatch(path: &str, _params: &HashMap<String, String>) -> Option<CliResponse> {
    match path {
        ADMIN_SET_PATH | OWNER_ACCOUNT_PROMPT_DISMISS_PATH => Some(CliResponse::method_not_allowed()),
        crate::email_notify::verify::ADDRESS_SET_PATH | crate::email_notify::verify::ADDRESS_REMOVE_PATH => {
            Some(CliResponse::method_not_allowed())
        }
        crate::email_notify::verify::ADDRESS_PATH => Some(error(
            "403 Forbidden",
            "forbidden",
            "a notice address belongs to a person: sign in (an agent cannot read or set it)",
        )),
        PREFS_PATH | ADMIN_PATH | OWNER_ACCOUNTS_PATH | PEOPLE_COLOR_PATH | PEOPLE_NAME_PATH
        | OWNER_ACCOUNT_PROMPT_PATH => Some(error(
            "403 Forbidden",
            "forbidden",
            "notification prefs belong to a person: sign in (an agent cannot read them)",
        )),
        _ => None,
    }
}

fn error(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } })
            .to_string(),
    }
}

fn usage(hint: impl std::fmt::Display) -> CliResponse {
    error("400 Bad Request", "usage", hint)
}

/// Handle one `/cli/notify/*` request for `actor`.
pub fn handle(is_post: bool, path: &str, body: &[u8], actor: &Actor) -> CliResponse {
    handle_with_params(is_post, path, body, actor, &HashMap::new())
}

/// [`handle`] with the request's query params (`GET /cli/people/name`
/// reads `?person=`; every other path ignores them).
pub fn handle_with_params(
    is_post: bool,
    path: &str,
    body: &[u8],
    actor: &Actor,
    params: &HashMap<String, String>,
) -> CliResponse {
    if path == OWNER_ACCOUNT_PROMPT_PATH || path == OWNER_ACCOUNT_PROMPT_DISMISS_PATH {
        return owner_account_prompt(is_post, path, actor);
    }
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => return error("500 Internal Server Error", "store", e),
    };
    let role = if actor.owner_token {
        Role::Owner
    } else {
        match k2_core::connect_users::role_for_user(&actor.login) {
            Some(r) => r,
            None => return error("403 Forbidden", "forbidden", "invalid or missing token"),
        }
    };
    let Some(person) = actor_person(&people, actor) else { return reserved_login_refusal() };
    if path == PEOPLE_COLOR_PATH {
        // Person colours need no DB: settings + connect-users.json only.
        return if is_post { people_color_write(&people, actor, role, body) } else { people_color_view(&people, actor) };
    }
    if path == PEOPLE_NAME_PATH {
        // Person names: settings + connect-users.json only, like colours.
        return if is_post {
            people_name_write(&people, actor, role, body)
        } else {
            let login = match params.get("person").map(|p| p.trim()).filter(|p| !p.is_empty()) {
                None => caller_login(actor),
                Some(p) => match known_login(&people, p) {
                    Ok(l) => l,
                    Err(r) => return r,
                },
            };
            people_name_view(&people, &login)
        };
    }
    if crate::email_notify::verify::is_person_route(path) {
        // Email Notifications part 2, slice V: a person's notice address
        // (it sends mail, so it runs without this handler's db lock).
        return crate::email_notify::verify::person_route(is_post, path, body, &person, role, params);
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !notify_prefs::tables_exist(&conn) {
        return error("503 Service Unavailable", "not_ready", "notification prefs are not migrated yet");
    }
    match (is_post, path) {
        (false, PREFS_PATH) => own_view(&conn, &people, &person),
        (true, PREFS_PATH) => own_write(&conn, &people, &person, body),
        (false, ADMIN_PATH) => {
            if role < Role::Admin {
                return role_required();
            }
            admin_view(&conn, &people)
        }
        (true, ADMIN_SET_PATH) => {
            if role < Role::Admin {
                return role_required();
            }
            admin_write(&conn, &people, &person, role, body)
        }
        (false, OWNER_ACCOUNTS_PATH) => {
            if role < Role::Admin {
                return role_required();
            }
            owner_accounts_view(&people)
        }
        (true, OWNER_ACCOUNTS_PATH) => owner_accounts_write(&people, actor, body),
        (false, ADMIN_SET_PATH) => CliResponse::method_not_allowed(),
        (true, ADMIN_PATH) => CliResponse::method_not_allowed(),
        _ => CliResponse::not_found(),
    }
}

/// The "Your account" setup prompt (0.46.0). The owner daemon token only:
/// the prompt is about who the owner is. `GET account-prompt` answers
/// whether to show it (and what to offer); `POST account-prompt/dismiss`
/// ("Not now") remembers this version and answers the new view. Linking
/// is the existing `POST /cli/owner/accounts`.
fn owner_account_prompt(is_post: bool, path: &str, actor: &Actor) -> CliResponse {
    match (is_post, path) {
        (false, OWNER_ACCOUNT_PROMPT_DISMISS_PATH) | (true, OWNER_ACCOUNT_PROMPT_PATH) => {
            return CliResponse::method_not_allowed();
        }
        _ => {}
    }
    if !actor.owner_token {
        return error(
            "403 Forbidden",
            "owner_token_required",
            "the Your account prompt is for the owner on the server's own computer",
        );
    }
    let version = env!("CARGO_PKG_VERSION");
    if is_post {
        if let Err(e) = k2_core::owner_account_prompt::dismiss(version) {
            return error("500 Internal Server Error", "store", e);
        }
    }
    match k2_core::owner_account_prompt::load_view(version) {
        Ok(v) => match serde_json::to_string(&v) {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => error("500 Internal Server Error", "serialize", e),
        },
        Err(e) => error("500 Internal Server Error", "store", e),
    }
}

fn role_required() -> CliResponse {
    error("403 Forbidden", "role_required", "this needs an admin login on this server")
}

#[derive(Debug, Clone)]
struct Workspace {
    id: String,
    name: String,
    handle: String,
}

fn workspaces(conn: &Connection) -> Result<Vec<Workspace>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, COALESCE(handle, '') FROM projects ORDER BY tab_order, name")
        .map_err(|e| format!("list workspaces: {e}"))?;
    let rows = stmt
        .query_map([], |r| Ok(Workspace { id: r.get(0)?, name: r.get(1)?, handle: r.get(2)? }))
        .map_err(|e| format!("list workspaces: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("list workspaces: {e}"))
}

fn project_exists(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", params![id], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// `{enabled, source}` plus `updatedBy`/`updatedAt` when the person's own
/// row exists.
fn switch_json(e: &Effective, on_key: &str) -> serde_json::Value {
    let mut v = serde_json::json!({ on_key: e.on, "source": e.source.as_wire() });
    if e.source == notify_prefs::Source::User {
        if let (Some(by), Some(at)) = (&e.updated_by, e.updated_at) {
            v["updatedBy"] = serde_json::json!(by);
            v["updatedAt"] = serde_json::json!(at);
        }
    }
    v
}

fn workspace_json(ws: &Workspace, e: &Effective) -> serde_json::Value {
    let mut v = switch_json(e, "threadPush");
    v["projectId"] = serde_json::json!(ws.id);
    v["name"] = serde_json::json!(ws.name);
    v["handle"] = serde_json::json!(ws.handle);
    v
}

fn devices() -> Vec<k2_core::push_devices::PushDevice> {
    k2_core::push_devices::list_devices().unwrap_or_default()
}

/// The `push` status (§5.6): ready | dormant | airgap, with a reason when
/// the gateway rejected this server.
fn push_status() -> k2_core::push::PushStatus {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    k2_core::push::PushStatus::of(
        crate::push_routes::current_config().as_ref(),
        crate::push_routes::airgap_blocks(),
        now,
    )
}

/// `GET /cli/notify/prefs` body (§5.1) for one person.
fn own_view(conn: &Connection, people: &People, person: &str) -> CliResponse {
    let prefs = match PrefsSnapshot::load(conn) {
        Ok(p) => p,
        Err(e) => return error("500 Internal Server Error", "store", e),
    };
    let list = match workspaces(conn) {
        Ok(l) => l,
        Err(e) => return error("500 Internal Server Error", "store", e),
    };
    // The owner's Connect logins that ARE the `owner` person: the
    // owner-linked accounts (the UI's "your
    // phone signs in as …" hint). Empty for everyone else.
    let aliases: Vec<String> = if person == OWNER { people.owner_aliases() } else { Vec::new() };
    let bell = prefs.bell(person);
    let status = push_status();
    let mut body = serde_json::json!({
        "ok": true,
        "principal": person,
        "aliases": aliases,
        "server": switch_json(&bell, "enabled"),
        "default": true,
        "devices": notify_prefs::device_count(people, &devices(), person),
        "push": status.as_wire(),
        "workspaces": list
            .iter()
            .map(|w| workspace_json(w, &prefs.effective(person, &w.id)))
            .collect::<Vec<_>>(),
    });
    if let Some(reason) = status.reason() {
        body["pushReason"] = serde_json::json!(reason);
    }
    CliResponse::ok_json(body.to_string())
}

/// Parsed `server` / `workspace` keys of a write body.
#[derive(Debug, Default)]
struct Change {
    server: Option<bool>,
    /// `(projectId, Some(on) | None = clear)`.
    workspace: Option<(String, Option<bool>)>,
}

fn parse_change(v: &serde_json::Value) -> Result<Change, CliResponse> {
    let mut c = Change::default();
    if let Some(s) = v.get("server") {
        let on = s
            .get("enabled")
            .and_then(|b| b.as_bool())
            .ok_or_else(|| usage("server.enabled must be true or false"))?;
        c.server = Some(on);
    }
    if let Some(w) = v.get("workspace") {
        c.workspace = Some(parse_workspace(w)?);
    }
    Ok(c)
}

fn parse_workspace(w: &serde_json::Value) -> Result<(String, Option<bool>), CliResponse> {
    let id = w
        .get("projectId")
        .and_then(|p| p.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| usage("workspace.projectId is required"))?;
    let tp = match w.get("threadPush") {
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(serde_json::Value::Null) => None,
        _ => return Err(usage("workspace.threadPush must be true, false or null")),
    };
    Ok((id.to_string(), tp))
}

fn parse_body(body: &[u8]) -> Result<serde_json::Value, CliResponse> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| usage(format!("invalid JSON body: {e}")))?;
    if !v.is_object() {
        return Err(usage("expected a JSON object"));
    }
    Ok(v)
}

/// Apply a change to `principal`'s rows. `Err` = nothing written.
fn apply(conn: &Connection, principal: &str, change: &Change, by: &str) -> Result<(), CliResponse> {
    if let Some((id, _)) = &change.workspace {
        if !project_exists(conn, id) {
            return Err(usage(format!("unknown projectId '{id}'")));
        }
    }
    if let Some(on) = change.server {
        notify_prefs::set_server(conn, principal, on, by).map_err(|e| error("500 Internal Server Error", "store", e))?;
    }
    if let Some((id, tp)) = &change.workspace {
        notify_prefs::set_workspace(conn, principal, id, *tp, by)
            .map_err(|e| error("500 Internal Server Error", "store", e))?;
    }
    Ok(())
}

fn emit_changed() {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::NotifyPrefsChanged {});
}

fn onoff(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "on",
        Some(false) => "off",
        None => "reset",
    }
}

/// One `notify.prefs.set` audit line per write: who, whom, which field,
/// on/off — never content.
fn audit(actor: &str, target: &str, change: &Change) {
    let mut fields = Vec::new();
    if let Some(on) = change.server {
        fields.push(format!("server={}", onoff(Some(on))));
    }
    if let Some((id, tp)) = &change.workspace {
        fields.push(format!("workspace:{id}={}", onoff(*tp)));
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "notify.prefs.set",
        actor,
        format!("target={target} {}", fields.join(" ")),
        "-",
        "-",
        "cli",
    ));
}

/// `POST /cli/notify/prefs` (§5.2): the caller's own rows only. A body
/// `principal` is ignored — identity is the token's.
fn own_write(conn: &Connection, people: &People, person: &str, body: &[u8]) -> CliResponse {
    let v = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let change = match parse_change(&v) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if change.server.is_none() && change.workspace.is_none() {
        return usage("send server and/or workspace");
    }
    if let Err(r) = apply(conn, person, &change, person) {
        return r;
    }
    audit(person, person, &change);
    emit_changed();
    own_view(conn, people, person)
}

/// `GET /cli/notify/admin` (§5.3).
fn admin_view(conn: &Connection, people: &People) -> CliResponse {
    let prefs = match PrefsSnapshot::load(conn) {
        Ok(p) => p,
        Err(e) => return error("500 Internal Server Error", "store", e),
    };
    let list = match workspaces(conn) {
        Ok(l) => l,
        Err(e) => return error("500 Internal Server Error", "store", e),
    };
    let devs = devices();
    let defaults: Vec<_> = list
        .iter()
        .map(|w| workspace_json(w, &prefs.default_for(&w.id)))
        .collect();
    let people_json: Vec<_> = people
        .roster()
        .iter()
        .map(|p| {
            serde_json::json!({
                "principal": p.principal,
                "role": p.role.as_wire(),
                "disabled": p.disabled,
                "logins": p.logins,
                "devices": notify_prefs::device_count(people, &devs, &p.principal),
                "server": switch_json(&prefs.bell(&p.principal), "enabled"),
                "workspaces": list
                    .iter()
                    .map(|w| workspace_json(w, &prefs.effective(&p.principal, &w.id)))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "defaults": defaults, "people": people_json }).to_string(),
    )
}

/// `POST /cli/notify/admin/set` (§5.4). A workspace default: Admin+.
/// Another person's row: the Owner only (Admins are read-only on people).
fn admin_write(conn: &Connection, people: &People, actor: &str, role: Role, body: &[u8]) -> CliResponse {
    let v = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Some(d) = v.get("default") {
        if v.get("principal").is_some() {
            return usage("send either default or principal, not both");
        }
        let (id, tp) = match parse_workspace(d) {
            Ok(x) => x,
            Err(r) => return r,
        };
        let change = Change { server: None, workspace: Some((id, tp)) };
        if let Err(r) = apply(conn, DEFAULT_PRINCIPAL, &change, actor) {
            return r;
        }
        audit(actor, DEFAULT_PRINCIPAL, &change);
        emit_changed();
        return admin_view(conn, people);
    }
    let Some(target) = v.get("principal").and_then(|p| p.as_str()).map(str::trim).filter(|p| !p.is_empty())
    else {
        return usage("send default, or principal with server and/or workspace");
    };
    if role != Role::Owner {
        return error(
            "403 Forbidden",
            "owner_only",
            "Admins can see other people's notification switches; only the Owner can change them",
        );
    }
    let target = people.person_key(target);
    let Some(target_role) = people.role_of(&target) else {
        return usage(format!("unknown person '{target}'"));
    };
    if !k2_core::connect_users::can_act_on(role, target_role) {
        return error("403 Forbidden", "forbidden", "you cannot change this person's switches");
    }
    let change = match parse_change(&v) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if change.server.is_none() && change.workspace.is_none() {
        return usage("send server and/or workspace");
    }
    if let Err(r) = apply(conn, &target, &change, actor) {
        return r;
    }
    audit(actor, &target, &change);
    emit_changed();
    admin_view(conn, people)
}

/// `GET /cli/owner/accounts` (Admin+): the Connect logins linked as the
/// owner, plus every login that currently resolves to `owner`.
fn owner_accounts_view(people: &People) -> CliResponse {
    let mut linked: Vec<String> = people.linked.iter().map(|l| l.trim().to_ascii_lowercase()).collect();
    linked.sort();
    linked.dedup();
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "accounts": linked, "aliases": people.owner_aliases() }).to_string(),
    )
}

/// `POST /cli/owner/accounts {"accounts": [...]}` — the OWNER DAEMON TOKEN
/// only: no Connect login (not even an Owner-role one, not a linked one)
/// and no agent passport may say who the owner is. Replaces the list;
/// every name must be an existing Connect user. Identity only: a linked
/// login keeps its own role and gains no owner power.
fn owner_accounts_write(people: &People, actor: &Actor, body: &[u8]) -> CliResponse {
    if !actor.owner_token {
        return error(
            "403 Forbidden",
            "owner_token_required",
            "linking accounts to the owner needs the owner token on the server",
        );
    }
    let v = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(list) = v.get("accounts").and_then(|a| a.as_array()) else {
        return usage("send {\"accounts\": [\"<connect username>\", ...]}");
    };
    let mut accounts = Vec::new();
    for item in list {
        let Some(name) = item.as_str().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()) else {
            return usage("accounts must be Connect usernames");
        };
        if name == OWNER || people.user(&name).is_none() {
            return usage(format!("'{name}' is not a Connect user on this server"));
        }
        if !accounts.contains(&name) {
            accounts.push(name);
        }
    }
    accounts.sort();
    if let Err(e) = k2_core::app_settings::update(serde_json::json!({ "ownerAccounts": accounts })) {
        return error("500 Internal Server Error", "store", e);
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "owner.accounts.set",
        OWNER,
        format!("accounts={}", accounts.join(",")),
        "-",
        "-",
        "cli",
    ));
    // 0.46.0: nothing may have been linked at boot — move the old
    // ownerDisplayName onto the account now (no-op once the flag is set).
    match k2_core::person_name::carry_over_owner_display_name() {
        Ok(k2_core::person_name::CarryOver::Carried { login }) => {
            eprintln!("[people] ownerDisplayName carried onto '{login}'")
        }
        Ok(_) => {}
        Err(e) => eprintln!("[people] ownerDisplayName carry-over failed: {e}"),
    }
    // The owner's names (and colour key) follow the linked account.
    crate::presence::rebroadcast();
    emit_changed();
    match People::load() {
        Ok(p) => owner_accounts_view(&p),
        Err(e) => error("500 Internal Server Error", "store", e),
    }
}

/// `GET /cli/people/color` (any login) →
/// `{ok, palette, person, color, chosen, colors}`: the eight hues, the
/// caller's person key, their effective colour, their chosen one (null =
/// Automatic), and name → effective colour for everyone (the map the
/// presence roster also carries).
fn people_color_view(people: &People, actor: &Actor) -> CliResponse {
    let settings = k2_core::app_settings::load();
    let owner_color = settings.owner_color.as_deref();
    let login = if actor.owner_token { OWNER } else { actor.login.as_str() };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "palette": k2_core::person_color::PALETTE,
            "person": people.person_key(login),
            "color": k2_core::person_color::effective(login, people, owner_color),
            "chosen": k2_core::person_color::chosen(login, people, owner_color),
            "colors": k2_core::person_color::color_map(people, owner_color),
        })
        .to_string(),
    )
}

/// `POST /cli/people/color {"color": "#2563eb" | null, "user"?: "<login>"}`.
///
/// - No `user`: the caller's own person. For the owner token, an
///   owner-linked login (person key `owner`)
///   that is the `ownerColor` setting, which the linked logins show too;
///   for anyone else, their own `connect-users.json` row.
/// - `user`: someone else (an admin override). Naming yourself is the
///   no-`user` case. The owner person (`owner` or a login that keys to it)
///   needs the Owner role; another login needs Admin+ and `can_act_on`.
///   A Member may set only their own.
///
/// `color: null` (or `"auto"`) clears it back to Automatic (the hash).
/// Any `#RRGGBB` is accepted (stored lowercase); anything else is a 400. Re-broadcasts the presence
/// roster so every client repaints.
fn people_color_write(people: &People, actor: &Actor, role: Role, body: &[u8]) -> CliResponse {
    let v = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let color: Option<String> = match v.get("color") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if s.trim().eq_ignore_ascii_case("auto") => None,
        Some(serde_json::Value::String(s)) => match k2_core::person_color::normalize(s) {
            Ok(c) => Some(c),
            Err(e) => return usage(e),
        },
        _ => return usage("send {\"color\": \"#RRGGBB\" | null}"),
    };
    let Some(me) = actor_person(people, actor) else { return reserved_login_refusal() };
    let target_login: String = match v.get("user") {
        None | Some(serde_json::Value::Null) => {
            if actor.owner_token {
                OWNER.to_string()
            } else {
                actor.login.trim().to_ascii_lowercase()
            }
        }
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.trim().to_ascii_lowercase(),
        _ => return usage("user must be a Connect username"),
    };
    let target_person = people.person_key(&target_login);
    let is_self = target_person == me;
    if target_person == OWNER {
        if !is_self && role < Role::Owner {
            return error("403 Forbidden", "role_required", "only the owner can change the owner's colour");
        }
        if let Err(e) = k2_core::app_settings::update(serde_json::json!({ "ownerColor": color })) {
            return error("500 Internal Server Error", "store", e);
        }
    } else {
        let Some(target) = people.user(&target_login) else {
            return usage(format!("'{target_login}' is not a Connect user on this server"));
        };
        if !is_self && (role < Role::Admin || !k2_core::connect_users::can_act_on(role, target.role)) {
            return error("403 Forbidden", "role_required", "you can change only your own colour");
        }
        if let Err(e) = k2_core::connect_users::set_color(&target.username, color.as_deref()) {
            return error("500 Internal Server Error", "store", e);
        }
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "people.color.set",
        &actor.login,
        format!("person={target_person} color={}", color.as_deref().unwrap_or("auto")),
        "-",
        "-",
        "cli",
    ));
    crate::presence::rebroadcast();
    match People::load() {
        Ok(p) => people_color_view(&p, actor),
        Err(e) => error("500 Internal Server Error", "store", e),
    }
}

/// The caller's own login: `owner` for the owner token, else the
/// lowercased Connect username.
fn caller_login(actor: &Actor) -> String {
    if actor.owner_token {
        OWNER.to_string()
    } else {
        actor.login.trim().to_ascii_lowercase()
    }
}

/// `raw` as a login this server knows (`owner` or a Connect username),
/// else a 400.
fn known_login(people: &People, raw: &str) -> Result<String, CliResponse> {
    let login = raw.trim().to_ascii_lowercase();
    if login == OWNER {
        return Ok(login);
    }
    match people.user(&login) {
        Some(u) => Ok(u.username.clone()),
        None => Err(usage(format!("'{login}' is not a Connect user on this server"))),
    }
}

/// `GET /cli/people/name[?person=<login>]` (any login; names are not
/// secret, like colours) → `{ok, person, firstName, lastName,
/// displayName, names}`: the person key `login` stands for (`owner` for
/// the owner token or an owner-linked login), their effective names
/// (`null` when unset; `displayName` falls back to the username — for
/// `owner`, the linked account's names, else its username, else
/// `Owner`), and login → `{firstName, lastName, displayName}` for `owner`
/// and every login (the map the presence roster also carries).
fn people_name_view(people: &People, login: &str) -> CliResponse {
    let me = k2_core::person_name::effective(login, people);
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "person": people.person_key(login),
            "firstName": me.first_name,
            "lastName": me.last_name,
            "displayName": me.display_name,
            "names": k2_core::person_name::name_map(people),
        })
        .to_string(),
    )
}

/// One name field of a `POST /cli/people/name` body: absent → `None`
/// (unchanged); `null` / `""` → `Some(None)` (clear); a string →
/// normalized (plain text, ≤ 64 chars); anything else → 400.
fn name_field(v: &serde_json::Value, key: &str) -> Result<Option<Option<String>>, CliResponse> {
    match v.get(key) {
        None => Ok(None),
        Some(serde_json::Value::Null) => Ok(Some(None)),
        Some(serde_json::Value::String(s)) => match k2_core::person_name::normalize(s) {
            Ok(n) => Ok(Some(n)),
            Err(e) => Err(usage(format!("{key}: {e}"))),
        },
        Some(_) => Err(usage(format!("{key} must be a string or null"))),
    }
}

/// `POST /cli/people/name {"person"?: "<login>", "firstName"?: str|null,
/// "lastName"?: str|null}`. A key that is absent is left as it is;
/// `null` or `""` clears it. At least one of the two is required. Names
/// live on login rows only:
///
/// - No `person`: the caller. A login edits its own row (an owner-linked
///   login's row IS the owner's names). The owner token edits the owner's
///   linked account ([`k2_core::person_name::owner_login`]); with none
///   linked it is a 409 `no_owner_account` ("pick your account first").
/// - `person: "owner"`: the linked account's row (409 when none). Anyone
///   but the owner person needs the Owner role.
/// - `person: "<login>"` = someone else: a login that keys to the owner
///   person needs the Owner role; any other needs Admin+ and
///   `can_act_on`. A Member may set only their own.
///
/// Both values are validated before anything is written. Names are
/// display only: no identity, routing or principal changes. Re-broadcasts
/// the presence roster so every client shows the new names. Returns the
/// GET body for the target person.
fn people_name_write(people: &People, actor: &Actor, role: Role, body: &[u8]) -> CliResponse {
    let v = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let first = match name_field(&v, "firstName") {
        Ok(f) => f,
        Err(r) => return r,
    };
    let last = match name_field(&v, "lastName") {
        Ok(l) => l,
        Err(r) => return r,
    };
    if first.is_none() && last.is_none() {
        return usage("send {\"firstName\"?: string|null, \"lastName\"?: string|null}");
    }
    let Some(me) = actor_person(people, actor) else { return reserved_login_refusal() };
    let caller = caller_login(actor);
    let target_login: String = match v.get("person") {
        None | Some(serde_json::Value::Null) => caller.clone(),
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.trim().to_ascii_lowercase(),
        _ => return usage("person must be a Connect username or \"owner\""),
    };
    let target_person = people.person_key(&target_login);
    // The row the names go on.
    let row = if target_login == OWNER {
        if me != OWNER && role < Role::Owner {
            return error("403 Forbidden", "role_required", "only the owner can change the owner's name");
        }
        match k2_core::person_name::owner_login(people) {
            Some(login) => login,
            None => {
                return error(
                    "409 Conflict",
                    "no_owner_account",
                    "pick your account first: the owner's name is its linked account's (Settings > General > Your account)",
                )
            }
        }
    } else {
        let Some(target) = people.user(&target_login) else {
            return usage(format!("'{target_login}' is not a Connect user on this server"));
        };
        let is_self = target.username == caller;
        if !is_self {
            if target_person == OWNER {
                if role < Role::Owner {
                    return error("403 Forbidden", "role_required", "only the owner can change the owner's name");
                }
            } else if role < Role::Admin || !k2_core::connect_users::can_act_on(role, target.role) {
                return error("403 Forbidden", "role_required", "you can change only your own name");
            }
        }
        target.username.clone()
    };
    // Already normalized: a clear is "", a value re-normalizes to itself.
    let as_arg = |part: &Option<Option<String>>| part.as_ref().map(|p| p.clone().unwrap_or_default());
    let (f, l) = (as_arg(&first), as_arg(&last));
    if let Err(e) = k2_core::connect_users::set_names(&row, f.as_deref(), l.as_deref()) {
        return error("500 Internal Server Error", "store", e);
    }
    let changed: Vec<&str> = [("firstName", first.is_some()), ("lastName", last.is_some())]
        .into_iter()
        .filter_map(|(k, on)| on.then_some(k))
        .collect();
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "people.name.set",
        &actor.login,
        format!("person={target_person} login={row} fields={}", changed.join(",")),
        "-",
        "-",
        "cli",
    ));
    crate::presence::rebroadcast();
    match People::load() {
        Ok(p) => people_name_view(&p, &target_login),
        Err(e) => error("500 Internal Server Error", "store", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempHome;
    use k2_core::connect_users;

    struct Fixture {
        _home: TempHome,
        _db: k2_core::db::TestDbGuard,
        alpha: String,
        beta: String,
    }

    fn fixture() -> Fixture {
        let home = TempHome::new();
        let db = k2_core::db::scoped_for_test();
        let alpha = uuid::Uuid::new_v4().to_string();
        let beta = uuid::Uuid::new_v4().to_string();
        {
            let h = db.handle();
            let conn = h.lock();
            for (id, name) in [(&alpha, "alpha"), (&beta, "beta")] {
                conn.execute(
                    "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                    params![id, name, format!("/tmp/notify-{id}")],
                )
                .expect("seed project");
            }
        }
        Fixture { _home: home, _db: db, alpha, beta }
    }

    fn add(name: &str, role: Role) {
        connect_users::add_user(name, "password123").expect("add_user");
        if role != Role::Member {
            connect_users::set_role(name, role).expect("set_role");
        }
    }

    fn get(path: &str, actor: &Actor) -> CliResponse {
        handle(false, path, b"", actor)
    }

    fn post(path: &str, body: serde_json::Value, actor: &Actor) -> CliResponse {
        handle(true, path, body.to_string().as_bytes(), actor)
    }

    fn ok(resp: CliResponse) -> serde_json::Value {
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], true, "{v}");
        v
    }

    fn ws<'a>(v: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        v["workspaces"]
            .as_array()
            .expect("workspaces")
            .iter()
            .find(|w| w["projectId"] == id)
            .unwrap_or_else(|| panic!("workspace {id} missing: {v}"))
    }

    fn drain_notify_events(rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>) -> usize {
        let mut n = 0;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, crate::session_events::SessionEvent::NotifyPrefsChanged {}) {
                n += 1;
            }
        }
        n
    }

    #[test]
    fn own_prefs_get_lists_every_workspace_with_sources() {
        let f = fixture();
        add("rosson", Role::Owner);
        let v = ok(get(PREFS_PATH, &Actor::owner_token()));
        assert_eq!(v["principal"], "owner");
        assert_eq!(v["aliases"], serde_json::json!([]), "an unlinked Owner-role login is not the owner (0.46.0)");
        assert_eq!(v["server"], serde_json::json!({ "enabled": true, "source": "fallback" }));
        assert_eq!(v["default"], true);
        assert_eq!(v["devices"], 0);
        assert_eq!(v["push"], "dormant", "no gateway in tests");
        let a = ws(&v, &f.alpha);
        assert_eq!(a["threadPush"], true);
        assert_eq!(a["source"], "fallback");
        assert_eq!(a["name"], "alpha");
        assert!(a.get("updatedBy").is_none(), "updatedBy only for source=user");
        // Unlinked, the Owner-role phone login is its own person (0.46.0)…
        let phone = ok(get(PREFS_PATH, &Actor::login("rosson")));
        assert_eq!(phone["principal"], "rosson");
        // …and once linked it is the same person as the owner token.
        ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["rosson"] }), &Actor::owner_token()));
        let phone = ok(get(PREFS_PATH, &Actor::login("rosson")));
        assert_eq!(phone["principal"], "owner");
        assert_eq!(ok(get(PREFS_PATH, &Actor::owner_token()))["aliases"], serde_json::json!(["rosson"]));
    }

    #[test]
    fn own_write_sets_and_clears_rows_and_emits_once() {
        let f = fixture();
        add("mo", Role::Member);
        let mo = Actor::login("mo");
        let mut rx = crate::session_events::subscribe();
        let v = ok(post(
            PREFS_PATH,
            serde_json::json!({ "server": { "enabled": false }, "workspace": { "projectId": f.alpha, "threadPush": false } }),
            &mo,
        ));
        assert_eq!(drain_notify_events(&mut rx), 1, "one event per write");
        assert_eq!(v["principal"], "mo");
        assert_eq!(v["server"]["enabled"], false);
        assert_eq!(v["server"]["source"], "user");
        let a = ws(&v, &f.alpha);
        assert_eq!((a["threadPush"].clone(), a["source"].clone()), (serde_json::json!(false), serde_json::json!("user")));
        assert_eq!(a["updatedBy"], "mo");
        assert!(a["updatedAt"].as_i64().expect("updatedAt") > 0);
        assert_eq!(ws(&v, &f.beta)["source"], "fallback");

        // null clears the row → the workspace follows its default again.
        let v = ok(post(PREFS_PATH, serde_json::json!({ "workspace": { "projectId": f.alpha, "threadPush": null } }), &mo));
        assert_eq!(ws(&v, &f.alpha)["source"], "fallback");
        assert_eq!(drain_notify_events(&mut rx), 1);

        // The owner's view is untouched by mo's rows.
        let owner = ok(get(PREFS_PATH, &Actor::owner_token()));
        assert_eq!(owner["server"]["source"], "fallback");
    }

    #[test]
    fn own_write_ignores_a_body_principal() {
        let f = fixture();
        add("mo", Role::Member);
        add("gina", Role::Admin);
        let v = ok(post(
            PREFS_PATH,
            serde_json::json!({ "principal": "gina", "workspace": { "projectId": f.alpha, "threadPush": false } }),
            &Actor::login("mo"),
        ));
        assert_eq!(v["principal"], "mo", "identity is the token's");
        let gina = ok(get(PREFS_PATH, &Actor::login("gina")));
        assert_eq!(ws(&gina, &f.alpha)["source"], "fallback", "gina's row was not written");
    }

    #[test]
    fn bad_bodies_are_usage_errors_and_write_nothing() {
        let f = fixture();
        add("mo", Role::Member);
        let mo = Actor::login("mo");
        let mut rx = crate::session_events::subscribe();
        for (body, why) in [
            (serde_json::json!({ "workspace": { "projectId": "nope", "threadPush": true } }), "unknown projectId"),
            (serde_json::json!({ "workspace": { "projectId": f.alpha, "threadPush": "yes" } }), "non-boolean"),
            (serde_json::json!({ "server": { "enabled": 1 } }), "non-boolean bell"),
            (serde_json::json!({}), "nothing to do"),
            (serde_json::json!([1]), "not an object"),
        ] {
            let r = post(PREFS_PATH, body, &mo);
            assert_eq!(r.status, "400 Bad Request", "{why}: {}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
            assert_eq!(v["error"]["code"], "usage", "{why}");
        }
        assert_eq!(drain_notify_events(&mut rx), 0, "no event for a refused write");
        let v = ok(get(PREFS_PATH, &mo));
        assert_eq!(ws(&v, &f.alpha)["source"], "fallback");
        assert_eq!(v["server"]["source"], "fallback");
    }

    #[test]
    fn admin_routes_need_admin_and_admins_are_read_only_on_people() {
        let f = fixture();
        add("gina", Role::Admin);
        add("mo", Role::Member);
        add("rosson", Role::Owner);
        // rosson is the owner's phone: linked (0.46.0 — only linked logins
        // fold into the owner person).
        ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["rosson"] }), &Actor::owner_token()));
        // Member: role_required on both admin paths.
        for r in [get(ADMIN_PATH, &Actor::login("mo")), post(ADMIN_SET_PATH, serde_json::json!({}), &Actor::login("mo"))] {
            assert_eq!(r.status, "403 Forbidden", "{}", r.body);
            assert!(r.body.contains("role_required"), "{}", r.body);
        }
        let gina = Actor::login("gina");
        // An Admin sees everyone.
        let v = ok(get(ADMIN_PATH, &gina));
        let people: Vec<&str> = v["people"].as_array().expect("people").iter().map(|p| p["principal"].as_str().expect("p")).collect();
        assert_eq!(people, vec!["owner", "gina", "mo"]);
        assert_eq!(v["people"][0]["logins"], serde_json::json!(["owner", "rosson"]));
        // …and may set a workspace default…
        let v = ok(post(ADMIN_SET_PATH, serde_json::json!({ "default": { "projectId": f.alpha, "threadPush": false } }), &gina));
        let d = v["defaults"].as_array().expect("defaults").iter().find(|d| d["projectId"] == f.alpha.as_str()).expect("alpha");
        assert_eq!((d["threadPush"].clone(), d["source"].clone()), (serde_json::json!(false), serde_json::json!("default")));
        assert_eq!(ws(&ok(get(PREFS_PATH, &Actor::login("mo"))), &f.alpha)["source"], "default");
        // …but may NOT change another person's switches — not a Member's,
        // not the owner's, not another Admin's.
        let mut rx = crate::session_events::subscribe();
        for target in ["mo", "owner", "rosson"] {
            let r = post(ADMIN_SET_PATH, serde_json::json!({ "principal": target, "server": { "enabled": false } }), &gina);
            assert_eq!(r.status, "403 Forbidden", "{target}: {}", r.body);
            assert!(r.body.contains("owner_only"), "{}", r.body);
        }
        assert_eq!(drain_notify_events(&mut rx), 0);
        let mo = ok(get(PREFS_PATH, &Actor::login("mo")));
        assert_eq!(mo["server"]["source"], "fallback", "nothing written");
        assert_eq!(ok(get(PREFS_PATH, &Actor::owner_token()))["server"]["source"], "fallback");
        let _ = f.beta;
    }

    #[test]
    fn the_owner_sets_other_peoples_rows_and_they_see_who() {
        let f = fixture();
        add("gina", Role::Admin);
        add("rosson", Role::Owner);
        let mut rx = crate::session_events::subscribe();
        // Owner token sets an Admin's workspace switch.
        ok(post(
            ADMIN_SET_PATH,
            serde_json::json!({ "principal": "gina", "workspace": { "projectId": f.alpha, "threadPush": false } }),
            &Actor::owner_token(),
        ));
        // An Owner-role login has the Owner's power too, but unlinked it is
        // its own person (0.46.0), so it signs as itself.
        ok(post(ADMIN_SET_PATH, serde_json::json!({ "principal": "gina", "server": { "enabled": false } }), &Actor::login("rosson")));
        assert_eq!(drain_notify_events(&mut rx), 2);
        let g = ok(get(PREFS_PATH, &Actor::login("gina")));
        assert_eq!(ws(&g, &f.alpha)["updatedBy"], "owner", "gina sees who set it");
        assert_eq!(g["server"]["updatedBy"], "rosson");
        // gina can Reset her row herself.
        let g = ok(post(PREFS_PATH, serde_json::json!({ "workspace": { "projectId": f.alpha, "threadPush": null } }), &Actor::login("gina")));
        assert_eq!(ws(&g, &f.alpha)["source"], "fallback");
        // Unknown people are usage errors.
        let r = post(ADMIN_SET_PATH, serde_json::json!({ "principal": "ghost", "server": { "enabled": false } }), &Actor::owner_token());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    /// Owner-linked accounts (Rosson 2026-10-11): a linked Admin login and
    /// the owner token share ONE prefs row; an unlinked Admin is separate;
    /// the link grants no owner power; only the owner token sets links.
    #[test]
    fn linked_login_shares_the_owner_person_but_not_owner_power() {
        let f = fixture();
        add("z3thon", Role::Admin);
        add("gina", Role::Admin);
        add("wes", Role::Owner);
        add("rosson", Role::Owner);
        let z = Actor::login("z3thon");
        // Before linking: z3thon is its own person.
        assert_eq!(ok(get(PREFS_PATH, &z))["principal"], "z3thon");
        // Only the owner token may link: not an Owner-role login, not the
        // Admin itself.
        for who in [Actor::login("rosson"), z.clone()] {
            let r = post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["z3thon"] }), &who);
            assert_eq!(r.status, "403 Forbidden", "{who:?}: {}", r.body);
            assert!(r.body.contains("owner_token_required"), "{}", r.body);
        }
        let r = post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["ghost"] }), &Actor::owner_token());
        assert_eq!(r.status, "400 Bad Request", "unknown login refused: {}", r.body);
        let mut rx = crate::session_events::subscribe();
        let v = ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["Z3thon"] }), &Actor::owner_token()));
        assert_eq!(v["accounts"], serde_json::json!(["z3thon"]));
        assert_eq!(drain_notify_events(&mut rx), 1);
        assert_eq!(k2_core::app_settings::load().owner_accounts, vec!["z3thon".to_string()]);
        // An Admin can see the links.
        assert_eq!(ok(get(OWNER_ACCOUNTS_PATH, &Actor::login("gina")))["accounts"], serde_json::json!(["z3thon"]));

        // The phone login and the desktop owner token: ONE row.
        let v = ok(post(PREFS_PATH, serde_json::json!({ "workspace": { "projectId": f.alpha, "threadPush": false } }), &z));
        assert_eq!(v["principal"], "owner");
        assert_eq!(v["aliases"], serde_json::json!(["z3thon"]));
        let desk = ok(get(PREFS_PATH, &Actor::owner_token()));
        assert_eq!(ws(&desk, &f.alpha)["threadPush"], false, "the desktop sees the phone's switch");
        assert_eq!(ws(&desk, &f.alpha)["updatedBy"], "owner");
        // An unlinked Admin stays separate.
        assert_eq!(ws(&ok(get(PREFS_PATH, &Actor::login("gina"))), &f.alpha)["source"], "fallback");

        // Identity, not privilege: the linked Admin still cannot change
        // other people's rows (owner-only) or the links themselves.
        let r = post(ADMIN_SET_PATH, serde_json::json!({ "principal": "gina", "server": { "enabled": false } }), &z);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert!(r.body.contains("owner_only"), "{}", r.body);
        let r = post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": [] }), &z);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(k2_core::connect_users::role_for_user("z3thon"), Some(Role::Admin), "role unchanged");
        // settings/update can't write the key at all.
        let r = crate::settings_routes::handle_settings_update(
            br#"{"ownerAccounts":["gina"]}"#,
            crate::settings_routes::SettingsActor::Owner,
        );
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(k2_core::app_settings::load().owner_accounts, vec!["z3thon".to_string()]);
    }

    #[test]
    fn get_chain_refuses_reads_and_405s_the_post_only_path() {
        let params = HashMap::new();
        assert_eq!(dispatch(ADMIN_SET_PATH, &params).expect("claimed").status, "405 Method Not Allowed");
        assert_eq!(crate::cli::dispatch(ADMIN_SET_PATH, &params).status, "405 Method Not Allowed");
        for p in [PREFS_PATH, ADMIN_PATH] {
            let r = crate::cli::dispatch(p, &params);
            assert_eq!(r.status, "403 Forbidden", "{p} via the identity-less GET chain: {}", r.body);
        }
        assert!(dispatch("/cli/notify/other", &params).is_none());
        let _f = fixture();
        let r = handle(false, ADMIN_SET_PATH, b"", &Actor::owner_token());
        assert_eq!(r.status, "405 Method Not Allowed");
        let r = handle(true, ADMIN_PATH, b"{}", &Actor::owner_token());
        assert_eq!(r.status, "405 Method Not Allowed");
    }

    // ── Person colours (0.46.0) ──────────────────────────────────────

    fn color_of(v: &serde_json::Value, name: &str) -> String {
        v["colors"][name].as_str().unwrap_or_else(|| panic!("no colour for {name}: {v}")).to_string()
    }

    #[test]
    fn people_color_set_and_get_round_trip() {
        let _f = fixture();
        add("anna", Role::Member);
        let anna = Actor::login("anna");
        let v = ok(get(PEOPLE_COLOR_PATH, &anna));
        assert_eq!(v["person"], "anna");
        assert_eq!(v["chosen"], serde_json::Value::Null);
        assert_eq!(v["color"], k2_core::person_color::hash_color("anna"), "Automatic = the hash");
        assert_eq!(v["palette"].as_array().expect("palette").len(), 8);
        assert_eq!(color_of(&v, "owner"), k2_core::person_color::hash_color("owner"));

        let mut rx = crate::session_events::subscribe();
        let v = ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#0D9488" }), &anna));
        assert_eq!(v["chosen"], "#0d9488", "stored lowercase");
        assert_eq!(v["color"], "#0d9488");
        assert_eq!(color_of(&v, "anna"), "#0d9488");
        assert_eq!(
            k2_core::connect_users::list_users().expect("users").iter().find(|u| u.username == "anna").expect("anna").color.as_deref(),
            Some("#0d9488"),
            "persisted on the user row"
        );
        let mut presence = 0;
        while let Ok(ev) = rx.try_recv() {
            if let crate::session_events::SessionEvent::PresenceChanged { colors, .. } = ev {
                assert_eq!(colors.get("anna").map(String::as_str), Some("#0d9488"));
                presence += 1;
            }
        }
        assert_eq!(presence, 1, "the roster is re-broadcast so clients repaint");

        // Back to Automatic.
        let v = ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": null }), &anna));
        assert_eq!(v["chosen"], serde_json::Value::Null);
        assert_eq!(v["color"], k2_core::person_color::hash_color("anna"));
    }

    #[test]
    fn people_color_accepts_any_rrggbb_and_refuses_the_rest() {
        let _f = fixture();
        add("anna", Role::Member);
        for bad in ["#abc", "red", "ff0000", "#ff00000", "#gg0000", ""] {
            let r = post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": bad }), &Actor::login("anna"));
            assert_eq!(r.status, "400 Bad Request", "{bad:?}: {}", r.body);
        }
        let r = post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": 3 }), &Actor::login("anna"));
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let anna_row = || connect_users::list_users().expect("users").into_iter().find(|u| u.username == "anna").expect("anna");
        assert_eq!(anna_row().color, None, "nothing written on a 400");
        // Any #RRGGBB, not just the Automatic palette (stored lowercase).
        let v = ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#123ABC" }), &Actor::login("anna")));
        assert_eq!(v["chosen"], "#123abc");
        assert_eq!(color_of(&v, "anna"), "#123abc");
        assert_eq!(anna_row().color.as_deref(), Some("#123abc"));
        let r = post(PEOPLE_COLOR_PATH, serde_json::json!({}), &Actor::owner_token());
        assert_eq!(r.status, "400 Bad Request", "color is required: {}", r.body);
        let r = post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#abc" }), &Actor::owner_token());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert_eq!(k2_core::app_settings::load().owner_color, None);
        let v = ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#FF0000" }), &Actor::owner_token()));
        assert_eq!(color_of(&v, "owner"), "#ff0000", "off the palette is fine");
        assert_eq!(k2_core::app_settings::load().owner_color.as_deref(), Some("#ff0000"));
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": null }), &Actor::owner_token()));
        assert_eq!(k2_core::app_settings::load().owner_color, None);
        // settings/update can't write the owner's colour around the check.
        let r = crate::settings_routes::handle_settings_update(
            br##"{"ownerColor":"#ff0000"}"##,
            crate::settings_routes::SettingsActor::Owner,
        );
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(k2_core::app_settings::load().owner_color, None);
    }

    #[test]
    fn people_color_linked_accounts_show_the_owner_colour() {
        let _f = fixture();
        add("z3thon", Role::Admin);
        add("gina", Role::Admin);
        add("rosson", Role::Owner);
        add("wes", Role::Owner);
        ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["z3thon"] }), &Actor::owner_token()));
        // The owner token sets the owner's colour …
        let v = ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#4f46e5" }), &Actor::owner_token()));
        assert_eq!(v["person"], "owner");
        assert_eq!(k2_core::app_settings::load().owner_color.as_deref(), Some("#4f46e5"));
        assert_eq!(color_of(&v, "owner"), "#4f46e5");
        assert_eq!(color_of(&v, "z3thon"), "#4f46e5", "the linked login shows the owner's colour");
        assert_eq!(color_of(&v, "gina"), k2_core::person_color::hash_color("gina"));
        // … and the linked login's own pick sets the OWNER's colour too.
        let z = Actor::login("z3thon");
        let v = ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#c026d3" }), &z));
        assert_eq!(v["person"], "owner");
        assert_eq!(v["chosen"], "#c026d3");
        assert_eq!(k2_core::app_settings::load().owner_color.as_deref(), Some("#c026d3"));
        assert_eq!(color_of(&v, "owner"), "#c026d3");
        let z_row = k2_core::connect_users::list_users().expect("users").into_iter().find(|u| u.username == "z3thon").expect("z3thon");
        assert_eq!(z_row.color, None, "the linked login's own row is untouched");
        // 0.46.0: the old ownerDisplayName is no longer a colour key (the
        // owner's identity key is the linked username, already above).
        k2_core::app_settings::update(serde_json::json!({ "ownerDisplayName": "Rosson L" })).expect("name");
        let v = ok(get(PEOPLE_COLOR_PATH, &Actor::login("gina")));
        assert!(v["colors"].get("Rosson L").is_none(), "{v}");
    }

    #[test]
    fn people_color_permission_floors() {
        let _f = fixture();
        add("anna", Role::Member);
        add("bob", Role::Member);
        add("gina", Role::Admin);
        add("hal", Role::Admin);
        add("rosson", Role::Owner);
        add("wes", Role::Owner);
        // A Member sets only their own.
        let r = post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#64748b", "user": "bob" }), &Actor::login("anna"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let r = post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#64748b", "user": "owner" }), &Actor::login("anna"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        // Naming yourself is fine.
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#64748b", "user": "anna" }), &Actor::login("anna")));
        // An Admin may change a Member's or another Admin's, not an Owner's
        // and not the owner person's.
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#0891b2", "user": "bob" }), &Actor::login("gina")));
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#0891b2", "user": "hal" }), &Actor::login("gina")));
        let r = post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#0891b2", "user": "wes" }), &Actor::login("gina"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let r = post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#0891b2", "user": "owner" }), &Actor::login("gina"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(k2_core::app_settings::load().owner_color, None);
        // The owner token (and an Owner-role login) may change anyone's.
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#2563eb", "user": "wes" }), &Actor::owner_token()));
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#7c3aed", "user": "owner" }), &Actor::login("rosson")));
        assert_eq!(k2_core::app_settings::load().owner_color.as_deref(), Some("#7c3aed"));
        let r = post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#7c3aed", "user": "ghost" }), &Actor::owner_token());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let colors = ok(get(PEOPLE_COLOR_PATH, &Actor::login("anna")));
        assert_eq!(color_of(&colors, "anna"), "#64748b");
        assert_eq!(color_of(&colors, "bob"), "#0891b2");
        assert_eq!(color_of(&colors, "wes"), "#2563eb");
        // The GET chain (no identity) refuses.
        assert_eq!(dispatch(PEOPLE_COLOR_PATH, &HashMap::new()).expect("claimed").status, "403 Forbidden");
    }

    // ── Person names (0.46.0) ────────────────────────────────────────

    fn get_person(person: &str, actor: &Actor) -> CliResponse {
        let mut params = HashMap::new();
        params.insert("person".to_string(), person.to_string());
        handle_with_params(false, PEOPLE_NAME_PATH, b"", actor, &params)
    }

    fn display_of(v: &serde_json::Value, login: &str) -> String {
        v["names"][login]["displayName"].as_str().unwrap_or_else(|| panic!("no names entry for {login}: {v}")).to_string()
    }

    #[test]
    fn people_name_self_set_and_get_round_trip() {
        let _f = fixture();
        add("anna", Role::Member);
        let anna = Actor::login("anna");
        let v = ok(get(PEOPLE_NAME_PATH, &anna));
        assert_eq!(v["person"], "anna");
        assert_eq!(v["firstName"], serde_json::Value::Null);
        assert_eq!(v["lastName"], serde_json::Value::Null);
        assert_eq!(v["displayName"], "anna", "no name → the username");
        assert_eq!(display_of(&v, "owner"), "Owner", "no linked account");

        let mut rx = crate::session_events::subscribe();
        let v = ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": " Anna ", "lastName": "Smith" }), &anna));
        assert_eq!(v["firstName"], "Anna", "trimmed");
        assert_eq!(v["lastName"], "Smith");
        assert_eq!(v["displayName"], "Anna Smith");
        assert_eq!(display_of(&v, "anna"), "Anna Smith");
        let row = connect_users::list_users().expect("users").into_iter().find(|u| u.username == "anna").expect("anna");
        assert_eq!((row.first_name.as_deref(), row.last_name.as_deref()), (Some("Anna"), Some("Smith")), "persisted on the row");
        let mut presence = 0;
        while let Ok(ev) = rx.try_recv() {
            if let crate::session_events::SessionEvent::PresenceChanged { names, .. } = ev {
                let e = names.get("anna").expect("anna in the names map");
                assert_eq!(e.display_name, "Anna Smith");
                presence += 1;
            }
        }
        assert_eq!(presence, 1, "the roster is re-broadcast so clients repaint");

        // An absent key is left alone; null clears; "" clears.
        let v = ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "lastName": null }), &anna));
        assert_eq!(v["firstName"], "Anna");
        assert_eq!(v["lastName"], serde_json::Value::Null);
        assert_eq!(v["displayName"], "Anna");
        let v = ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "" }), &anna));
        assert_eq!(v["firstName"], serde_json::Value::Null);
        assert_eq!(v["displayName"], "anna");

        // Any login reads anyone's (names are not secret).
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Anna" }), &anna));
        add("bob", Role::Member);
        let v = ok(get_person("ANNA", &Actor::login("bob")));
        assert_eq!(v["person"], "anna");
        assert_eq!(v["displayName"], "Anna");
        let r = get_person("ghost", &Actor::login("bob"));
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn people_name_refuses_bad_values() {
        let _f = fixture();
        add("anna", Role::Member);
        let anna = Actor::login("anna");
        for bad in [
            serde_json::json!({ "firstName": "a".repeat(65) }),
            serde_json::json!({ "lastName": "é".repeat(65) }),
            serde_json::json!({ "firstName": "Line\nBreak" }),
            serde_json::json!({ "firstName": "Tab\there" }),
            serde_json::json!({ "lastName": "<b>Bold</b>" }),
            serde_json::json!({ "firstName": 3 }),
            serde_json::json!({ "firstName": "Ok", "lastName": "x".repeat(65) }),
            serde_json::json!({}),
            serde_json::json!({ "person": 7, "firstName": "Ok" }),
        ] {
            let r = post(PEOPLE_NAME_PATH, bad.clone(), &anna);
            assert_eq!(r.status, "400 Bad Request", "{bad}: {}", r.body);
        }
        let row = connect_users::list_users().expect("users").into_iter().find(|u| u.username == "anna").expect("anna");
        assert_eq!((row.first_name, row.last_name), (None, None), "nothing written on a 400");
        // 64 characters is fine (multi-byte counted as characters).
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "é".repeat(64) }), &anna));
        // settings/update can't write the daemon-owned carry-over flag.
        let r = crate::settings_routes::handle_settings_update(
            br#"{"ownerDisplayNameCarried":true}"#,
            crate::settings_routes::SettingsActor::Owner,
        );
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert!(!k2_core::app_settings::load().owner_display_name_carried);
    }

    #[test]
    fn people_name_owner_is_the_linked_account() {
        let _f = fixture();
        add("z3thon", Role::Admin);
        add("gina", Role::Admin);
        add("rosson", Role::Owner);
        add("wes", Role::Owner);
        // Nothing linked: no names, displayName "Owner"; a write is a 409.
        k2_core::app_settings::update(serde_json::json!({ "ownerDisplayName": "Rosson L" })).expect("display");
        let v = ok(get(PEOPLE_NAME_PATH, &Actor::owner_token()));
        assert_eq!(v["person"], "owner");
        assert_eq!(v["firstName"], serde_json::Value::Null);
        assert_eq!(v["lastName"], serde_json::Value::Null);
        assert_eq!(v["displayName"], "Owner", "ownerDisplayName is not read for display");
        let r = post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Rosson" }), &Actor::owner_token());
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert!(r.body.contains("no_owner_account") && r.body.contains("pick your account first"), "{}", r.body);
        let r = post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Rosson", "person": "owner" }), &Actor::login("rosson"));
        assert_eq!(r.status, "409 Conflict", "{}", r.body);

        // Linking z3thon (no names yet) carries the old display name over once.
        ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["z3thon"] }), &Actor::owner_token()));
        let settings = k2_core::app_settings::load();
        assert!(settings.owner_display_name_carried);
        assert_eq!(settings.owner_display_name.as_deref(), Some("Rosson L"), "kept on disk");
        let v = ok(get(PEOPLE_NAME_PATH, &Actor::owner_token()));
        assert_eq!(v["firstName"], "Rosson");
        assert_eq!(v["lastName"], "L");
        assert_eq!(v["displayName"], "Rosson L");

        // The owner token writes the linked account's row …
        let v = ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Rosson", "lastName": "Long" }), &Actor::owner_token()));
        assert_eq!(v["person"], "owner");
        assert_eq!(v["displayName"], "Rosson Long");
        let z = connect_users::list_users().expect("users").into_iter().find(|u| u.username == "z3thon").expect("z3thon");
        assert_eq!((z.first_name.as_deref(), z.last_name.as_deref()), (Some("Rosson"), Some("Long")), "on the linked row");
        assert_eq!(display_of(&v, "owner"), "Rosson Long");
        assert_eq!(display_of(&v, "z3thon"), "Rosson Long");
        assert!(v["names"].get("Rosson L").is_none(), "no display-name key: {v}");
        assert_eq!(display_of(&v, "gina"), "gina");
        // … and the linked login editing itself edits its own row (the owner's names).
        let v = ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "lastName": "L." }), &Actor::login("z3thon")));
        assert_eq!(v["person"], "owner");
        assert_eq!(v["displayName"], "Rosson L.");
        assert_eq!(ok(get(PEOPLE_NAME_PATH, &Actor::owner_token()))["displayName"], "Rosson L.");
        // Cleared: the owner shows the linked username.
        let v = ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": null, "lastName": null }), &Actor::owner_token()));
        assert_eq!(v["firstName"], serde_json::Value::Null);
        assert_eq!(v["displayName"], "z3thon");
        // The carry-over never runs again.
        ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["z3thon"] }), &Actor::owner_token()));
        assert_eq!(ok(get(PEOPLE_NAME_PATH, &Actor::owner_token()))["displayName"], "z3thon");
        // An Admin can't rename the owner person's login.
        let r = post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "X", "person": "z3thon" }), &Actor::login("gina"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    }

    #[test]
    fn people_name_permission_floors() {
        let _f = fixture();
        add("anna", Role::Member);
        add("bob", Role::Member);
        add("gina", Role::Admin);
        add("hal", Role::Admin);
        add("rosson", Role::Owner);
        add("wes", Role::Owner);
        add("z3thon", Role::Admin);
        ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["z3thon"] }), &Actor::owner_token()));
        // A Member sets only their own.
        let r = post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Bob", "person": "bob" }), &Actor::login("anna"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let r = post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "X", "person": "owner" }), &Actor::login("anna"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        // Naming yourself is fine.
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Anna", "person": "anna" }), &Actor::login("anna")));
        // An Admin may change a Member's or another Admin's, not an Owner's
        // and not the owner person's.
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Bob", "person": "bob" }), &Actor::login("gina")));
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Hal", "person": "hal" }), &Actor::login("gina")));
        let r = post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Wes", "person": "wes" }), &Actor::login("gina"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let r = post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "X", "person": "owner" }), &Actor::login("gina"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let z = connect_users::list_users().expect("users").into_iter().find(|u| u.username == "z3thon").expect("z3thon");
        assert_eq!(z.first_name, None, "nothing written on a 403");
        // The owner token (and an Owner-role login) may change anyone's.
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Wes", "person": "wes" }), &Actor::owner_token()));
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Rosson", "person": "owner" }), &Actor::login("rosson")));
        let z = connect_users::list_users().expect("users").into_iter().find(|u| u.username == "z3thon").expect("z3thon");
        assert_eq!(z.first_name.as_deref(), Some("Rosson"), "the owner person's names live on the linked row");
        let r = post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Ghost", "person": "ghost" }), &Actor::owner_token());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let v = ok(get(PEOPLE_NAME_PATH, &Actor::login("anna")));
        assert_eq!(display_of(&v, "anna"), "Anna");
        assert_eq!(display_of(&v, "bob"), "Bob");
        assert_eq!(display_of(&v, "hal"), "Hal");
        assert_eq!(display_of(&v, "wes"), "Wes");
        assert_eq!(display_of(&v, "owner"), "Rosson");
        // The GET chain (no identity) refuses.
        assert_eq!(dispatch(PEOPLE_NAME_PATH, &HashMap::new()).expect("claimed").status, "403 Forbidden");
    }

    // ── Only linked accounts are the owner person (0.46.0) ────────────

    /// julie is the server's only Owner-role login but is NOT linked: she
    /// keeps her own name and colour after z3thon is linked, and z3thon
    /// (linked) shows the owner's colour and names everywhere.
    #[test]
    fn an_unlinked_sole_owner_role_login_stays_itself() {
        let _f = fixture();
        add("julie", Role::Owner);
        add("z3thon", Role::Admin);
        let owner = Actor::owner_token();
        let julie = Actor::login("julie");
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Julie", "lastName": "Ray" }), &julie));
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#aa3355" }), &julie));
        ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["z3thon"] }), &owner));
        ok(post(PEOPLE_NAME_PATH, serde_json::json!({ "firstName": "Rosson", "lastName": "Long" }), &owner));
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#12ab9f" }), &owner));

        // julie: her own person, name and colour.
        let v = ok(get(PEOPLE_NAME_PATH, &julie));
        assert_eq!((v["person"].clone(), v["displayName"].clone()), ("julie".into(), "Julie Ray".into()));
        let c = ok(get(PEOPLE_COLOR_PATH, &julie));
        assert_eq!((c["person"].clone(), c["color"].clone()), ("julie".into(), "#aa3355".into()));
        assert_eq!(c["colors"]["julie"], "#aa3355");
        assert_eq!(v["names"]["julie"]["displayName"], "Julie Ray");

        // z3thon (linked): the owner's settings.
        assert_eq!(c["colors"]["z3thon"], "#12ab9f");
        assert_eq!(c["colors"]["owner"], "#12ab9f");
        assert_eq!(v["names"]["z3thon"]["displayName"], "Rosson Long");
        assert_eq!(v["names"]["owner"]["displayName"], "Rosson Long");
        let z = ok(get(PEOPLE_COLOR_PATH, &Actor::login("z3thon")));
        assert_eq!((z["person"].clone(), z["color"].clone()), ("owner".into(), "#12ab9f".into()));

        // The owner's aliases name only the linked login.
        assert_eq!(ok(get(OWNER_ACCOUNTS_PATH, &owner))["aliases"], serde_json::json!(["z3thon"]));
    }

    // ── "Your account" setup prompt (0.46.0) ──────────────────────────

    #[test]
    fn account_prompt_pick_then_not_now_then_link() {
        let _f = fixture();
        add("z3thon", Role::Admin);
        add("julie", Role::Owner);
        add("anna", Role::Member);
        let owner = Actor::owner_token();
        let v = ok(get(OWNER_ACCOUNT_PROMPT_PATH, &owner));
        assert_eq!((v["show"].clone(), v["mode"].clone(), v["reason"].clone()), (true.into(), "pick".into(), "pick_account".into()));
        let names: Vec<&str> = v["candidates"].as_array().expect("candidates").iter().map(|c| c["username"].as_str().expect("u")).collect();
        assert_eq!(names, vec!["julie", "z3thon"], "Owner/Admin only, sorted");
        assert_eq!(v["preselect"], serde_json::Value::Null, "two candidates, no name match");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));

        // Not now: hidden for this version, stored on the daemon.
        let v = ok(post(OWNER_ACCOUNT_PROMPT_DISMISS_PATH, serde_json::json!({}), &owner));
        assert_eq!((v["show"].clone(), v["reason"].clone()), (false.into(), "dismissed".into()));
        assert_eq!(
            k2_core::app_settings::load().owner_account_prompt.dismissed_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(ok(get(OWNER_ACCOUNT_PROMPT_PATH, &owner))["show"], false);

        // A link (the existing route) stops it for good.
        ok(post(OWNER_ACCOUNTS_PATH, serde_json::json!({ "accounts": ["z3thon"] }), &owner));
        let v = ok(get(OWNER_ACCOUNT_PROMPT_PATH, &owner));
        assert_eq!((v["show"].clone(), v["reason"].clone(), v["linked"].clone()), (false.into(), "linked".into(), "z3thon".into()));
    }

    #[test]
    fn account_prompt_with_no_admins_is_create_and_ignores_not_now() {
        let _f = fixture();
        add("anna", Role::Member);
        let owner = Actor::owner_token();
        ok(post(OWNER_ACCOUNT_PROMPT_DISMISS_PATH, serde_json::json!({}), &owner));
        let v = ok(get(OWNER_ACCOUNT_PROMPT_PATH, &owner));
        assert_eq!((v["show"].clone(), v["mode"].clone(), v["reason"].clone()), (true.into(), "create".into(), "create_account".into()));
        assert_eq!(v["candidates"], serde_json::json!([]));
    }

    #[test]
    fn account_prompt_is_the_owner_tokens_only_and_dismiss_is_post_only() {
        let _f = fixture();
        add("z3thon", Role::Owner);
        let login = Actor::login("z3thon");
        for (is_post, path) in [(false, OWNER_ACCOUNT_PROMPT_PATH), (true, OWNER_ACCOUNT_PROMPT_DISMISS_PATH)] {
            let r = handle(is_post, path, b"{}", &login);
            assert_eq!(r.status, "403 Forbidden", "{path}: {}", r.body);
            assert!(r.body.contains("owner_token_required"), "{}", r.body);
        }
        let owner = Actor::owner_token();
        assert_eq!(get(OWNER_ACCOUNT_PROMPT_DISMISS_PATH, &owner).status, "405 Method Not Allowed");
        assert_eq!(post(OWNER_ACCOUNT_PROMPT_PATH, serde_json::json!({}), &owner).status, "405 Method Not Allowed");
        assert_eq!(k2_core::app_settings::load().owner_account_prompt.dismissed_version, None, "nothing written");
        // settings/update can't write it either (the dismiss route owns it).
        let r = crate::settings_routes::handle_settings_update(
            br#"{"ownerAccountPrompt":{"dismissedVersion":"9.9.9"}}"#,
            crate::settings_routes::SettingsActor::Owner,
        );
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(k2_core::app_settings::load().owner_account_prompt.dismissed_version, None);
        // The GET chain (no identity): the read refuses, the dismiss is 405.
        assert_eq!(dispatch(OWNER_ACCOUNT_PROMPT_PATH, &HashMap::new()).expect("claimed").status, "403 Forbidden");
        assert_eq!(dispatch(OWNER_ACCOUNT_PROMPT_DISMISS_PATH, &HashMap::new()).expect("claimed").status, "405 Method Not Allowed");
    }

    /// A Connect login named `owner` (made before 0.46.6, even as an
    /// Owner-role login, even linked) is never the owner person on notices,
    /// colours or names; the owner token still is.
    #[test]
    fn a_login_named_owner_is_never_the_owner_person_here() {
        let _f = fixture();
        connect_users::test_add_legacy_reserved_user("owner", "password123").expect("legacy owner login");
        connect_users::set_role("owner", Role::Owner).expect("owner role");
        k2_core::app_settings::update(serde_json::json!({ "ownerAccounts": ["owner"] })).expect("link");
        let named = Actor::login("owner");
        for (path, body) in [
            (PREFS_PATH, None),
            (PEOPLE_COLOR_PATH, Some(serde_json::json!({ "color": "#112233" }))),
            (PEOPLE_NAME_PATH, Some(serde_json::json!({ "firstName": "Not" }))),
        ] {
            let r = match body {
                Some(b) => post(path, b, &named),
                None => get(path, &named),
            };
            assert_eq!(r.status, "403 Forbidden", "{path}: {}", r.body);
            assert!(r.body.contains("reserved_login"), "{path}: {}", r.body);
        }
        assert_eq!(k2_core::app_settings::load().owner_color, None, "the owner's colour is untouched");
        let people = People::load().expect("people");
        assert_eq!(actor_person(&people, &named), None);
        assert_eq!(actor_person(&people, &Actor::owner_token()).as_deref(), Some(OWNER));
        // The real owner token still works.
        ok(get(PREFS_PATH, &Actor::owner_token()));
        ok(post(PEOPLE_COLOR_PATH, serde_json::json!({ "color": "#112233" }), &Actor::owner_token()));
        assert_eq!(k2_core::app_settings::load().owner_color.as_deref(), Some("#112233"));
    }
}
