//! A test login's rooms (0.46.17, a user's report): an agent's `k2 app user
//! add <app> <login> --test --role R` works, but signing in is refused
//! (`skin_room`) in every room of R but the App's own. By design an
//! agent-made login reaches only its own room until the person chooses their
//! own password (`skin::ties`), and a test login has no person, so testing
//! the role's other rooms was blocked.
//!
//! ```text
//! POST /cli/app/test-guest/rooms-ok  {app, username, reason?}
//! ```
//!
//! - **An agent** (its own workspace's test login, one it made): K2 reads the
//!   login's role now and files the `apps.test-guest-rooms` card on its
//!   Thread ("Let <ws>'s test login <login> into <rooms> with its role <R>"),
//!   for an owner or admin, `--assign <login>` for one of them. Asking
//!   changes nothing.
//! - **An owner or admin** (from their own terminal): opens them at once.
//! - **Allow** opens exactly the rooms on the card for that login and that
//!   role (`skin::test_guests::open_rooms`): the role's rights there, never a
//!   person-only one; the login still ends when it ends. A different role or
//!   a new room needs a new card: a card whose role or rooms changed since the
//!   ask is stale, and so is one for a login made again since.
//! - **Only a test login.** Any other agent-made login keeps today's rule.
//!
//! Why a separate verb and not `--rooms` on `add`: the login usually exists
//! already (the add answered 0 and works in the App's own room), the card
//! reads the role the login has at ask time, and the add stays one
//! made-now step that never waits on a person.

use std::collections::HashMap;

use k2_core::skin::{self, test_guests as tg};
use serde_json::{json, Value as J};

use super::routes::{actor, agent_ws, json_body, need_credential, no, s, ws_handle, Who, R};
use super::test_guests::{guest_app_for, GuestApp, NOT_A_TEST_GUEST};
use k2_core::brand::CLI_NAME;

/// The card.
pub(crate) const ACTION: &str = "apps.test-guest-rooms";
/// `k2 app user rooms-ok <app> <login>`.
pub const ROOMS_OK: &str = "/cli/app/test-guest/rooms-ok";
/// The login is not tied to an agent: it already reaches every room.
pub const NOT_TIED: &str = "test_guest_not_tied";
/// The login has no role on its App.
pub const NO_ROLE: &str = "test_guest_no_role";
/// The role names no room but the agent's own.
pub const NOTHING_TO_OPEN: &str = "test_guest_nothing_to_open";

/// One test login's rooms, as the card and the route read them now.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub app: GuestApp,
    pub principal_id: String,
    pub username: String,
    pub role_id: String,
    pub role: String,
    /// Room ids past the login's own, sorted.
    pub rooms: Vec<String>,
    /// Their handles, in the same order.
    pub handles: Vec<String>,
    /// Those already opened to it.
    pub opened: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

impl Plan {
    /// Every room is open already.
    pub fn done(&self) -> bool {
        self.rooms.iter().all(|r| self.opened.contains(r))
    }

    /// The card's values.
    pub fn params(&self) -> serde_json::Map<String, J> {
        let mut m = serde_json::Map::new();
        m.insert("app".into(), json!(self.app.app_id));
        m.insert("login".into(), json!(self.username));
        m.insert("role".into(), json!(self.role));
        m.insert("rooms".into(), json!(self.handles));
        m
    }
}

fn store_err(e: String) -> super::routes::Refusal {
    no("db_error", 500, e)
}

/// Read `login` of `app` now. `asker`: the asking agent's workspace (the
/// login must be one it made); `None` for a person.
pub(crate) fn plan(who: &Who, asker: Option<&str>, app_raw: Option<&str>, login: Option<&str>) -> R<Plan> {
    let app = guest_app_for(who, app_raw)?;
    let login = login.map(str::trim).filter(|l| !l.is_empty()).ok_or_else(|| no("bad_request", 400, "name the test login"))?;
    let lower = login.to_ascii_lowercase();
    let not_test = || {
        no(
            NOT_A_TEST_GUEST,
            409,
            format!("{login} isn't a test login of {}: only a --test login can be let into its role's rooms on a card ({CLI_NAME} app test-guests {})", app.app_id, app.app_id),
        )
    };
    let p = skin::list_principals().map_err(store_err)?.into_iter().find(|p| p.username == lower).ok_or_else(not_test)?;
    let t = tg::get(&p.id).map_err(store_err)?.filter(|t| t.app == app.service_id).ok_or_else(not_test)?;
    if t.expired_at(k2_core::deploy::now()) {
        return Err(no("test_guest_expired", 409, format!("{} has ended: make a new test login", p.username)));
    }
    let tie = skin::ties::get(&p.id).map_err(store_err)?.ok_or_else(|| {
        no(NOT_TIED, 409, format!("{} was made by a person, not an agent: it already reaches every room its role names", p.username))
    })?;
    if let Some(ws) = asker {
        if tie.workspace_id != ws {
            return Err(not_test());
        }
    }
    let grant = skin::list_grants()
        .map_err(store_err)?
        .into_iter()
        .find(|g| g.enabled && g.kind == "app" && g.subject_kind == "principal" && g.subject_id == p.id && g.target_id == app.service_id);
    let role_id = grant.and_then(|g| g.role_id).ok_or_else(|| {
        no(NO_ROLE, 409, format!("{} has no role on {}: only a role's rooms can be opened (give it one of the App's roles)", p.username, app.app_id))
    })?;
    let role = skin::list_roles().map_err(store_err)?.into_iter().find(|r| r.id == role_id).ok_or_else(|| {
        no(NO_ROLE, 409, format!("{}'s role on {} is gone", p.username, app.app_id))
    })?;
    let rooms: Vec<String> = role.room_policy.keys().filter(|r| **r != tie.workspace_id).cloned().collect();
    if rooms.is_empty() {
        return Err(no(
            NOTHING_TO_OPEN,
            409,
            format!("role {} names no room but {}'s own, which {} reaches already", role.name, ws_handle(&tie.workspace_id), p.username),
        ));
    }
    let handles = rooms.iter().map(|r| ws_handle(r)).collect();
    let opened = tie.rooms_ok.iter().filter(|(a, _)| *a == app.service_id).map(|(_, r)| r.clone()).collect();
    Ok(Plan {
        app,
        principal_id: p.id,
        username: p.username,
        role_id,
        role: role.name,
        rooms,
        handles,
        opened,
        created_at: t.created_at,
        expires_at: t.expires_at,
    })
}

/// Does the card (its role and rooms) still match `p`? `Err(why)`: stale.
pub(crate) fn matches(p: &Plan, role: &str, rooms: &[String]) -> Result<(), String> {
    if p.role != role {
        return Err(format!("{}'s role on {} is {} now, not {}: ask again", p.username, p.app.app_id, p.role, role));
    }
    let mut want: Vec<&str> = rooms.iter().map(String::as_str).collect();
    let mut have: Vec<&str> = p.handles.iter().map(String::as_str).collect();
    want.sort_unstable();
    have.sort_unstable();
    if want != have {
        return Err(format!("role {} names {} now, not {}: ask again", p.role, have.join(", "), want.join(", ")));
    }
    Ok(())
}

/// Open `p`'s rooms, as `by`, with the audit lines.
pub(crate) fn open(p: &Plan, by: &str, via: &str) -> R<J> {
    tg::open_rooms(&p.principal_id, &p.app.service_id, &p.role_id, &p.rooms, by).map_err(|e| no(NOT_A_TEST_GUEST, 409, e))?;
    let rooms = p.handles.join(", ");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let payload = json!({ "username": p.username, "role": p.role, "rooms": p.handles, "via": via, "expiresAt": p.expires_at });
        if let Err(e) = k2_core::deploy::audit::append(&conn, k2_core::deploy::audit::SUBJECT_APP, &p.app.app_id, "test_guest_rooms_opened", by, None, &payload) {
            k2_core::log_debug!("[apps] audit {} test_guest_rooms_opened: {e}", p.app.app_id);
        }
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "app-test-guest-rooms",
        by,
        format!("opened {rooms} to test login {} on {} (role {}, via {via}, until {})", p.username, p.app.app_id, p.role, p.expires_at),
        "cli",
        "-",
        "apps",
    ));
    Ok(json!({
        "ok": true,
        "status": "opened",
        "app": p.app.app_id,
        "username": p.username,
        "role": p.role,
        "rooms": p.handles,
        "expiresAt": p.expires_at,
        "note": format!(
            "{} can now use {rooms} on {} with its role {} (never a person-only right), until it ends. A new role or room needs a new card.",
            p.username, p.app.app_id, p.role
        ),
    }))
}

/// POST [`ROOMS_OK`] `{app, username, reason?}`.
pub(crate) fn handle(who: &Who, body: &[u8], _params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    match who {
        Who::Agent { project_id, session_id } => {
            let p = plan(who, agent_ws(who), s(&v, "app"), s(&v, "username"))?;
            if p.done() {
                return Ok(json!({ "ok": true, "status": "already_open", "username": p.username, "rooms": p.handles,
                    "note": format!("{} already uses {} on {}", p.username, p.handles.join(", "), p.app.app_id) }));
            }
            let why = s(&v, "reason").map(str::to_string).unwrap_or_else(|| format!("To test {} as {} with its role {}", p.app.app_id, p.username, p.role));
            let assign = super::held_cards::assigned();
            let (status, out) =
                crate::permission::routes::raise_card(project_id, session_id, ACTION, J::Object(p.params()), &why, assign.as_deref());
            let st = out.get("status").and_then(J::as_str).unwrap_or("");
            if status != 200 || !(st == "pending" || st == "already_pending") {
                let code = out.pointer("/error/code").and_then(J::as_str).unwrap_or("card_not_raised").to_string();
                let hint = out.pointer("/error/hint").and_then(J::as_str).unwrap_or("the card couldn't go up").to_string();
                return Err(super::routes::Refusal { code: leak(code), status: if status == 200 { 409 } else { status }, hint, extra: None });
            }
            let routed: Vec<String> = out
                .get("routedTo")
                .and_then(J::as_array)
                .map(|a| a.iter().filter_map(|x| x.get("login").and_then(J::as_str).map(str::to_string)).collect())
                .unwrap_or_default();
            let id = out.get("id").and_then(J::as_str).unwrap_or("").to_string();
            let thread = out.get("addr").and_then(J::as_str).unwrap_or("").to_string();
            Ok(json!({
                "ok": true,
                "status": "asked",
                "username": p.username,
                "role": p.role,
                "rooms": p.handles,
                "card": { "id": id, "card": out.get("card"), "thread": thread, "routedTo": routed, "new": st == "pending" },
                "where": format!(
                    "{} permission card {id} is on {thread}'s Thread for {}: Allow lets {} into {} with its role {}. Check with {CLI_NAME} permission status {id}.",
                    if st == "pending" { "a" } else { "the" },
                    if routed.is_empty() { "the owner".to_string() } else { routed.join(", ") },
                    p.username,
                    p.handles.join(", "),
                    p.role
                ),
            }))
        }
        Who::Shell { .. } => Err(no(
            "owner_only",
            403,
            format!("an owner or admin opens these from their own terminal ({CLI_NAME} app user rooms-ok <app> <login>), or answers the agent's card"),
        )),
        _ if who.is_admin() => {
            let p = plan(who, None, s(&v, "app"), s(&v, "username"))?;
            if p.done() {
                return Ok(json!({ "ok": true, "status": "already_open", "username": p.username, "rooms": p.handles }));
            }
            open(&p, &actor(who), "rooms-ok")
        }
        _ => Err(no("owner_only", 403, "an owner or admin lets a test login into its role's rooms")),
    }
}

/// A refusal code from the card path, as the `&'static str` a refusal
/// carries (the codes are a small fixed set: the permission request's own).
pub(crate) fn leak(code: String) -> &'static str {
    const KNOWN: &[&str] = &["rate_limited", "no_thread", "bad_params", "unknown_approver", "approver_not_eligible", "bad_approvers", "ask_limit", "denied_recently"];
    KNOWN.iter().find(|k| **k == code).copied().unwrap_or("card_not_raised")
}
