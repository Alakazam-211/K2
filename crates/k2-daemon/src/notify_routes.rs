//! `/cli/notify/*` — Thread push prefs (prd-thread-push-notifications-v1
//! §5, slice S2).
//!
//! | Method | Path | Floor | Acts on |
//! |---|---|---|---|
//! | GET  | `/cli/notify/prefs`     | Member | the caller's person key |
//! | POST | `/cli/notify/prefs`     | Member | the caller's person key |
//! | GET  | `/cli/notify/admin`     | Admin  | everyone + the workspace defaults |
//! | POST | `/cli/notify/admin/set` | Admin  | a workspace default (Admin+), or another person's row (Owner only) |
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

/// GET chain (`crate::cli::dispatch`): no identity reaches it, so the
/// reads refuse and the POST-only path answers 405. The real traffic
/// takes the dispatcher arm, which resolves the caller first.
pub fn dispatch(path: &str, _params: &HashMap<String, String>) -> Option<CliResponse> {
    match path {
        ADMIN_SET_PATH => Some(CliResponse::method_not_allowed()),
        PREFS_PATH | ADMIN_PATH | OWNER_ACCOUNTS_PATH => Some(error(
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
    let person = if actor.owner_token { OWNER.to_string() } else { people.person_key(&actor.login) };
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
    // owner-linked accounts plus a sole Owner-role login (the UI's "your
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
    emit_changed();
    match People::load() {
        Ok(p) => owner_accounts_view(&p),
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
        assert_eq!(v["aliases"], serde_json::json!(["rosson"]), "the sole Owner-role login maps to owner");
        assert_eq!(v["server"], serde_json::json!({ "enabled": true, "source": "fallback" }));
        assert_eq!(v["default"], true);
        assert_eq!(v["devices"], 0);
        assert_eq!(v["push"], "dormant", "no gateway in tests");
        let a = ws(&v, &f.alpha);
        assert_eq!(a["threadPush"], true);
        assert_eq!(a["source"], "fallback");
        assert_eq!(a["name"], "alpha");
        assert!(a.get("updatedBy").is_none(), "updatedBy only for source=user");
        // The phone login is the same person.
        let phone = ok(get(PREFS_PATH, &Actor::login("rosson")));
        assert_eq!(phone["principal"], "owner");
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
        // An Owner-role login is the Owner too (and maps to `owner`).
        ok(post(ADMIN_SET_PATH, serde_json::json!({ "principal": "gina", "server": { "enabled": false } }), &Actor::login("rosson")));
        assert_eq!(drain_notify_events(&mut rx), 2);
        let g = ok(get(PREFS_PATH, &Actor::login("gina")));
        assert_eq!(ws(&g, &f.alpha)["updatedBy"], "owner", "gina sees who set it");
        assert_eq!(g["server"]["updatedBy"], "owner");
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
}
