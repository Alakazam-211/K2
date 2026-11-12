//! `/cli/desktop/*` (`prd-remote-desktop-tab-v1` §5, D2; virtual
//! desktops `prd-linux-virtual-desktop-v1` L4).
//!
//! ```text
//! GET  /cli/desktop/status     ?desktop=   the switch, the desktop's state, what you may do
//! GET  /cli/desktop/list       ?workspace= the desktops you may open (the tab's picker)
//! GET  /cli/desktop/displays   ?desktop=   the display list
//! POST /cli/desktop/enable     {desktop?}                 Owner
//! POST /cli/desktop/disable    {desktop?}                 Owner; closes every viewer 4410
//! POST /cli/desktop/policy     {hideApps: [bundle id]}    Owner
//! POST /cli/desktop/grant      {user, mode: "view", desktop?}  Owner; Members only, view only
//! POST /cli/desktop/revoke     {user, desktop?}           Owner; closes that login's viewers 4401
//! GET  /cli/desktop/grants     ?desktop=                  Owner
//! GET  /cli/desktop/viewers    ?desktop=                  Owner
//! POST /cli/desktop/kick       {viewer} | {user}, desktop? Owner; closes 4403 kicked
//! GET  /cli/desktop/audit      ?since=1d&limit=50         Owner
//! POST /cli/desktop/open       {display?, desktop?}       who may watch, or the Owner: open_desktop
//! POST /cli/desktop/ticket     {display?, desktop?}       a login who may watch: a 60 s stream ticket
//! POST /cli/desktop/start      {desktop?, width?, height?} Owner, Admin (Linux): start or wake
//! POST /cli/desktop/stop       {desktop?}                 Owner, Admin (Linux): close it
//! POST /cli/desktop/setup      {}                         Owner (Linux): the packages
//! ```
//!
//! **Which desktop.** Every route takes an optional `desktop` (the query on
//! GET, the body on POST). Left out it is This server: [`current_desktop`],
//! `console` where a real screen session exists, else `virtual` (Linux).
//! `virtual:<workspace id>` is an agent's own desktop. The Owner switch for
//! every virtual desktop is the `virtual` row; grants are per id. A macOS
//! server never has a virtual desktop: its status stays as it was, and a
//! virtual id, `start` and `setup` answer `linux_only`.
//!
//! **Who.** People only. An agent passport, an App pass, an API key or an
//! API session gets `403 desktop_people_only`, and the permission-card
//! wrapper ends it with "It can't be requested." "Owner" above means an
//! Owner-role login, the owner daemon token, or a K2 shell tab (a person
//! typing in K2); never an Admin (S1). Watching: Owners and Admins; a
//! Member only with a grant, view only (`k2_core::desktop::policy`). The
//! stream (D6) takes logins only, through a ticket.
//!
//! **Every write** is POST-only (405 on GET, here and at the dispatcher),
//! writes its access-log line FIRST (a failed write refuses the change,
//! 503 `audit_unavailable`), emits `desktop_state`, and, for enable,
//! disable, a policy change and a new grant, pushes a notice to every
//! paired phone of an Owner login (P3).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::desktop::audit::{self, AuditLine, Event};
use k2_core::desktop::policy::{self, Access, Principal};
use k2_core::desktop::virtual_desktop as vd;
use k2_core::desktop::wire::{self, Mode};
use k2_core::desktop::{grants, store, ticket, viewers, CONSOLE, ENABLE_CARD, VIRTUAL_ENABLE_CARD};
use serde_json::{json, Value};

use super::deskd;
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const ROUTES: &[&str] = &[
    "/cli/desktop/audit",
    "/cli/desktop/disable",
    "/cli/desktop/displays",
    "/cli/desktop/enable",
    "/cli/desktop/grant",
    "/cli/desktop/grants",
    "/cli/desktop/kick",
    "/cli/desktop/list",
    "/cli/desktop/open",
    "/cli/desktop/policy",
    "/cli/desktop/revoke",
    "/cli/desktop/setup",
    "/cli/desktop/start",
    "/cli/desktop/status",
    "/cli/desktop/stop",
    "/cli/desktop/ticket",
    "/cli/desktop/viewers",
];

pub const POST_ROUTES: &[&str] = &[
    "/cli/desktop/disable",
    "/cli/desktop/enable",
    "/cli/desktop/grant",
    "/cli/desktop/kick",
    "/cli/desktop/open",
    "/cli/desktop/policy",
    "/cli/desktop/revoke",
    "/cli/desktop/setup",
    "/cli/desktop/start",
    "/cli/desktop/stop",
    "/cli/desktop/ticket",
];

/// Request bodies here are small.
pub const MAX_BODY: usize = 16 * 1024;

/// The refusal code for every non-person credential.
pub const PEOPLE_ONLY: &str = "desktop_people_only";

/// What a macOS server answers for a virtual desktop (LD-15).
pub const LINUX_ONLY: &str = "Virtual desktops are for Linux servers only. This computer's own screen is the Desktop tab's only desktop.";

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// One request, as the dispatcher (or the cell socket) saw it.
pub struct Req<'a> {
    pub path: &'a str,
    pub is_post: bool,
    pub body: &'a [u8],
    pub params: &'a HashMap<String, String>,
    /// `loopback` | `lan` | `tunnel` | `app` | `cell`.
    pub ingress: &'a str,
    pub ip: &'a str,
    /// The credential presented (a ticket binds a login's session to it).
    pub token: &'a str,
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
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

pub(crate) fn refuse(status: u16, code: &str, hint: impl Into<String>) -> CliResponse {
    refuse_with(status, code, hint, json!({}))
}

/// A refusal with extra top-level fields (`closable`, `manual`, …).
pub(crate) fn refuse_with(status: u16, code: &str, hint: impl Into<String>, extra: Value) -> CliResponse {
    let mut v = json!({"ok": false, "error": {"code": code, "hint": hint.into()}});
    if let (Value::Object(o), Value::Object(x)) = (&mut v, extra) {
        o.extend(x);
    }
    CliResponse { status: status_line(status), content_type: "application/json", body: v.to_string() }
}

pub(crate) fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

pub fn too_large() -> CliResponse {
    refuse(413, "too_large", format!("request bodies here are at most {} KiB", MAX_BODY / 1024))
}

/// `403 desktop_people_only`.
pub fn people_only() -> CliResponse {
    refuse(
        403,
        PEOPLE_ONLY,
        "Remote desktop is for people only: the owner and the logins they allow. Agents can't see or drive this computer's screen with K2 in v1.",
    )
}

fn owner_only(what: &str) -> CliResponse {
    refuse(
        403,
        "owner_only",
        format!("{what} is the Owner's: an Owner login, or the owner at this computer (k2 desktop, or Settings → Remote Desktop)"),
    )
}

pub(crate) fn linux_only() -> CliResponse {
    refuse(409, "linux_only", LINUX_ONLY)
}

fn principal(who: &Who) -> Result<Principal, CliResponse> {
    match who {
        Who::Owner => Ok(Principal::OwnerToken),
        Who::Shell { .. } => Ok(Principal::Shell),
        Who::Login { username, role } => Ok(Principal::Login { username: username.clone(), role: *role }),
        Who::Agent { .. } | Who::Wrong => Err(people_only()),
        Who::Invalid => Err(CliResponse::forbidden()),
    }
}

fn access_for(p: &Principal, ingress: &str, desktop: &str) -> Result<Access, CliResponse> {
    let grant = match p {
        Principal::Login { username, role: Role::Member } => {
            grants::live_grant(desktop, username).map_err(|e| refuse(500, "store_error", e))?
        }
        _ => None,
    };
    let same_machine = matches!(ingress, "loopback" | "cell");
    Ok(policy::access(p, grant.as_ref(), same_machine))
}

/// Owners and Admins (logins), the owner token and a K2 shell tab: they
/// start, wake and close virtual desktops (§3.3). Never a Member.
pub(crate) fn may_start(p: &Principal) -> bool {
    matches!(p, Principal::OwnerToken | Principal::Shell | Principal::Login { role: Role::Owner | Role::Admin, .. })
}

/// The person's name for a notice line.
pub(crate) fn person(p: &Principal) -> String {
    match p {
        Principal::OwnerToken => "the owner at this computer".into(),
        Principal::Shell => "a K2 terminal on this computer".into(),
        Principal::Login { username, .. } => username.clone(),
    }
}

pub(crate) fn machine() -> String {
    crate::connect_device_routes::server_display_name(None)
}

pub(crate) fn line(event: Event, desktop: &str, p: &Principal, req: &Req<'_>) -> AuditLine {
    AuditLine::new(event, desktop, &p.actor()).role(p.role_wire()).ingress(req.ingress).ip(req.ip)
}

/// Write the access-log line before the change; refuse when it can't be.
pub(crate) fn audit_first(l: &AuditLine) -> Result<(), CliResponse> {
    audit::write(l).map_err(|e| {
        k2_core::log_debug!("[desktop] audit write failed: {e}");
        refuse(503, "audit_unavailable", "K2 couldn't write the remote desktop access log (~/.k2/desktop-audit.jsonl), so nothing changed")
    })
}

// ── which desktop ────────────────────────────────────────────────────

/// Test overrides: is this a Linux server, and is there a real screen
/// session (`None` = ask the machine).
static LINUX_FOR_TEST: std::sync::RwLock<Option<bool>> = std::sync::RwLock::new(None);
static SESSION_FOR_TEST: std::sync::RwLock<Option<bool>> = std::sync::RwLock::new(None);

/// Virtual desktops exist here (Linux).
pub fn linux_here() -> bool {
    LINUX_FOR_TEST.read().unwrap_or_else(|p| p.into_inner()).unwrap_or(cfg!(target_os = "linux"))
}

/// Tests: pretend to be (or not be) a Linux server, and whether someone
/// is logged in at its screen, until the guard drops.
pub fn platform_for_test(linux: bool, real_session: bool) -> PlatformGuard {
    *LINUX_FOR_TEST.write().unwrap_or_else(|p| p.into_inner()) = Some(linux);
    *SESSION_FOR_TEST.write().unwrap_or_else(|p| p.into_inner()) = Some(real_session);
    PlatformGuard
}

pub struct PlatformGuard;

impl Drop for PlatformGuard {
    fn drop(&mut self) {
        *LINUX_FOR_TEST.write().unwrap_or_else(|p| p.into_inner()) = None;
        *SESSION_FOR_TEST.write().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// A real graphical session on this Linux machine (cached 10 s).
fn real_session() -> bool {
    if let Some(v) = *SESSION_FOR_TEST.read().unwrap_or_else(|p| p.into_inner()) {
        return v;
    }
    #[cfg(target_os = "linux")]
    {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
        let mut c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, v)) = *c {
            if at.elapsed() < Duration::from_secs(10) {
                return v;
            }
        }
        // SAFETY: plain call.
        let uid = unsafe { libc::getuid() };
        let home = k2_core::paths::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let v = super::deskd_link::discover_session_env(&super::launch_agent::SystemRunner, uid, &home, &|p| p.exists()).is_some_and(|e| e.has_display());
        *c = Some((Instant::now(), v));
        v
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// This server's desktop (LD-2): `console` with a real screen session (and
/// always on macOS), else `virtual`.
pub fn current_desktop() -> String {
    if linux_here() && !real_session() {
        vd::VIRTUAL.to_string()
    } else {
        CONSOLE.to_string()
    }
}

/// The desktop a request names, checked; or [`current_desktop`].
fn resolve_desktop(req: &Req<'_>, body: &Value) -> Result<String, CliResponse> {
    let asked = if req.is_post { str_field(body, "desktop").map(str::to_string) } else { req.params.get("desktop").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) };
    let Some(id) = asked else { return Ok(current_desktop()) };
    if !vd::valid_id(&id) || !(id == CONSOLE || vd::is_virtual(&id)) {
        return Err(refuse(400, "bad_desktop", "desktop is console, virtual, or virtual:<workspace id>"));
    }
    if vd::is_virtual(&id) && !linux_here() {
        return Err(linux_only());
    }
    Ok(id)
}

/// `desktop_state` for one desktop to every client.
pub fn emit_state_for(desktop: &str) {
    let enabled = store::is_enabled(vd::switch_id(desktop)).unwrap_or(false);
    let (state, message) = if vd::is_virtual(desktop) {
        super::virtual_routes::state_of(desktop)
    } else {
        let s = deskd::current().state();
        (s.as_wire().to_string(), s.message())
    };
    crate::session_events::emit_desktop(
        "desktop_state",
        json!({"desktop": desktop, "enabled": enabled, "state": state, "message": message, "viewers": viewers::count(desktop)}),
    );
}

/// `desktop_state` for This server.
pub fn emit_state() {
    emit_state_for(&current_desktop());
}

/// The Owner logins' names (enabled ones), for the push.
pub(crate) fn owner_logins() -> Vec<String> {
    grants::all_user_facts()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.role == Role::Owner && !f.disabled)
        .map(|f| f.username)
        .collect()
}

/// A phone push to every paired Owner (P3). `kind_id`: enable | disable |
/// policy | grant | viewer.
pub fn notify(kind_id: &str, text: String) {
    crate::push_routes::notify_desktop(kind_id, &text, owner_logins());
}

/// D6 calls this when a viewer connects (a new viewer is a P3 notice).
pub fn notify_new_viewer(user: &str, ingress: &str) {
    notify("viewer", format!("{user} started watching {}'s screen ({ingress}).", machine()));
}

fn body_json(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(_) => Err(refuse(400, "bad_request", "the body must be a JSON object")),
        Err(e) => Err(refuse(400, "bad_request", format!("invalid JSON body: {e}"))),
    }
}

pub(crate) fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// `?since=`: RFC 3339, or `30m` / `2h` / `1d` / `7d` back from now.
pub fn parse_since(s: &str) -> Result<String, String> {
    let s = s.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    }
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num.parse().map_err(|_| format!("since '{s}': use 30m, 2h, 1d or an RFC 3339 time"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return Err(format!("since '{s}': use 30m, 2h, 1d or an RFC 3339 time")),
    };
    Ok((chrono::Utc::now() - chrono::Duration::seconds(secs)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// Answer one `/cli/desktop/*` request.
pub fn handle(who: &Who, req: &Req<'_>) -> CliResponse {
    if !is_route(req.path) {
        return CliResponse::not_found();
    }
    if is_post_route(req.path) && !req.is_post {
        return CliResponse::method_not_allowed();
    }
    if !is_post_route(req.path) && req.is_post {
        return CliResponse::method_not_allowed();
    }
    let p = match principal(who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    // A Voice & Chat only login never reaches here over TCP (Member floor
    // in route_policy); this is the belt for any other door.
    if matches!(p, Principal::Login { role: Role::VoiceChat, .. }) {
        return refuse(403, "role_required", "Remote desktop isn't for a Voice & Chat only login");
    }
    let body = if req.is_post {
        match body_json(req.body) {
            Ok(b) => b,
            Err(r) => return r,
        }
    } else {
        json!({})
    };
    // The list and the packages are about the server, not one desktop.
    match req.path {
        "/cli/desktop/list" => return super::virtual_routes::list(&p, req),
        "/cli/desktop/setup" => return super::virtual_routes::setup(&p, req),
        _ => {}
    }
    let desktop = match resolve_desktop(req, &body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let a = match access_for(&p, req.ingress, &desktop) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match req.path {
        "/cli/desktop/status" => status(&p, &a, &desktop),
        "/cli/desktop/displays" => {
            if !a.may_view && !a.may_manage {
                return refuse(403, "not_allowed", "you don't have access to this computer's screen (the owner grants it: k2 desktop grant)");
            }
            ok(json!({"displays": deskd::for_desktop(&desktop).displays()}))
        }
        "/cli/desktop/open" => open(&p, &a, &body, req, &desktop),
        "/cli/desktop/ticket" => mint_ticket(&p, &a, &body, req, &desktop),
        "/cli/desktop/start" => super::virtual_routes::start(&p, &body, req, &desktop),
        "/cli/desktop/stop" => super::virtual_routes::stop(&p, req, &desktop),
        _ if !a.may_manage => owner_only(match req.path {
            "/cli/desktop/enable" | "/cli/desktop/disable" => "Turning Remote desktop on or off",
            "/cli/desktop/policy" => "Remote desktop's policy",
            "/cli/desktop/grant" | "/cli/desktop/revoke" | "/cli/desktop/grants" => "Remote desktop grants",
            "/cli/desktop/kick" | "/cli/desktop/viewers" => "Seeing and closing viewers",
            _ => "The Remote desktop access log",
        }),
        "/cli/desktop/enable" => set_switch(&p, true, req, &desktop),
        "/cli/desktop/disable" => set_switch(&p, false, req, &desktop),
        "/cli/desktop/policy" => set_policy(&p, &body, req),
        "/cli/desktop/grant" => grant(&p, &body, req, &desktop),
        "/cli/desktop/revoke" => revoke(&p, &body, req, &desktop),
        "/cli/desktop/grants" => match grants::list(&desktop) {
            Ok(g) => ok(json!({"desktop": desktop, "grants": g})),
            Err(e) => refuse(500, "store_error", e),
        },
        "/cli/desktop/viewers" => ok(json!({"desktop": desktop, "viewers": viewers::list(&desktop)})),
        "/cli/desktop/kick" => kick(&p, &body, req, &desktop),
        "/cli/desktop/audit" => read_audit(req),
        _ => CliResponse::not_found(),
    }
}

fn status(p: &Principal, a: &Access, desktop: &str) -> CliResponse {
    let s = match store::settings(vd::switch_id(desktop)) {
        Ok(s) => s,
        Err(e) => return refuse(500, "store_error", e),
    };
    let d = deskd::for_desktop(desktop);
    let state = d.state();
    let mut v = json!({
        "desktop": desktop,
        "kind": "server",
        "name": "This server",
        "machine": machine(),
        "enabled": s.enabled,
        "backend": d.backend(),
        "state": state.as_wire(),
        "message": state.message(),
        "reason": null,
        "lane": "main",
        "you": {
            "role": p.role_wire(),
            "mayView": a.may_view,
            "mayControl": a.may_control,
            "mayManage": a.may_manage,
            "sameMachine": a.same_machine,
            "mayStart": linux_here() && may_start(p),
        },
    });
    if vd::is_virtual(desktop) {
        super::virtual_routes::fill_status(&mut v, desktop, state.as_wire());
    }
    if a.may_manage {
        if desktop == CONSOLE {
            v["hideApps"] = json!(s.hide_apps);
        }
        v["grants"] = match grants::list(desktop) {
            Ok(g) => json!(g),
            Err(e) => return refuse(500, "store_error", e),
        };
        v["viewers"] = json!(viewers::list(desktop));
    }
    ok(v)
}

fn set_switch(p: &Principal, on: bool, req: &Req<'_>, desktop: &str) -> CliResponse {
    let switch = vd::switch_id(desktop);
    let virtual_switch = vd::is_virtual(desktop);
    let was = match store::is_enabled(switch) {
        Ok(b) => b,
        Err(e) => return refuse(500, "store_error", e),
    };
    let ev = if on { Event::Enable } else { Event::Disable };
    if let Err(r) = audit_first(&line(ev, switch, p, req).outcome(if was == on { "unchanged" } else { "ok" })) {
        return r;
    }
    if let Err(e) = store::enable(switch, on, &p.actor()) {
        return refuse(500, "store_error", e);
    }
    let mut closed = 0;
    if virtual_switch {
        // The virtual desktops start only when asked; off closes them all.
        if !on {
            let m = super::virtual_desktop::manager();
            for (id, _) in m.known() {
                closed += viewers::close_all(&id, wire::CLOSE_DESKTOP_OFF, wire::reason::DESKTOP_OFF);
            }
            m.close_all(vd::CloseReason::SwitchedOff);
        }
    } else {
        let d = deskd::current();
        if on {
            if let Err(e) = d.enable() {
                // deskd couldn't start: the switch goes back off, loudly.
                let _ = store::enable(switch, false, &p.actor());
                emit_state_for(switch);
                return refuse(500, "deskd_enable_failed", format!("K2 Screen didn't start: {e}"));
            }
        } else {
            closed = viewers::close_all(CONSOLE, wire::CLOSE_DESKTOP_OFF, wire::reason::DESKTOP_OFF);
            if let Err(e) = d.disable() {
                k2_core::log_debug!("[desktop] deskd disable: {e}");
            }
        }
    }
    emit_state_for(switch);
    if was != on {
        let verb = if on { "turned on" } else { "turned off" };
        notify(if on { "enable" } else { "disable" }, format!("Remote desktop on {} was {verb} by {} ({}).", machine(), person(p), req.ingress));
    }
    let (state, message) = if virtual_switch {
        let st = super::virtual_desktop::manager().status(switch);
        (st.wire_state().to_string(), st.message())
    } else {
        let s = deskd::current().state();
        (s.as_wire().to_string(), s.message())
    };
    let mut v = json!({"ok": true, "desktop": switch, "enabled": on, "changed": was != on, "state": state, "message": message});
    if on {
        v["card"] = json!(if virtual_switch { VIRTUAL_ENABLE_CARD } else { ENABLE_CARD });
    } else {
        v["closed"] = json!(closed);
    }
    ok(v)
}

fn set_policy(p: &Principal, body: &Value, req: &Req<'_>) -> CliResponse {
    let Some(list) = body.get("hideApps").and_then(Value::as_array) else {
        return refuse(400, "bad_request", "send {\"hideApps\": [\"com.apple.Passwords\", …]} (bundle ids)");
    };
    let mut raw = Vec::with_capacity(list.len());
    for x in list {
        match x.as_str() {
            Some(s) => raw.push(s.to_string()),
            None => return refuse(400, "bad_request", "hideApps holds bundle ids (strings)"),
        }
    }
    if let Err(e) = store::clean_hide_apps(&raw) {
        return refuse(400, "bad_hide_app", e);
    }
    if let Err(r) = audit_first(&line(Event::Policy, CONSOLE, p, req)) {
        return r;
    }
    let stored = match store::hide_apps(CONSOLE, &raw, &p.actor()) {
        Ok(l) => l,
        Err(e) => return refuse(500, "store_error", e),
    };
    emit_state_for(CONSOLE);
    notify("policy", format!("Remote desktop's hidden apps on {} were changed by {} ({} hidden).", machine(), person(p), stored.len()));
    ok(json!({"ok": true, "hideApps": stored}))
}

fn grant_error(e: &grants::GrantError) -> CliResponse {
    refuse(e.status(), e.code(), e.hint())
}

fn grant(p: &Principal, body: &Value, req: &Req<'_>, desktop: &str) -> CliResponse {
    let Some(user) = str_field(body, "user") else {
        return refuse(400, "bad_request", "send {\"user\": \"<login>\", \"mode\": \"view\"}");
    };
    let mode = match body.get("mode").map(|m| m.as_str().and_then(Mode::parse)) {
        None => Mode::View,
        Some(Some(m)) => m,
        Some(None) => return refuse(400, "bad_request", "mode is view (Members only ever watch)"),
    };
    let facts = match grants::user_facts(user) {
        Ok(Some(f)) => f,
        Ok(None) => return grant_error(&grants::GrantError::NoSuchUser(user.to_string())),
        Err(e) => return refuse(500, "store_error", e),
    };
    if let Err(e) = grants::check(&facts, mode) {
        return grant_error(&e);
    }
    if let Err(r) = audit_first(&line(Event::Grant, desktop, p, req).target(&facts.username).mode(mode.as_str())) {
        return r;
    }
    let g = match grants::grant(desktop, &facts.username, mode, &p.actor()) {
        Ok(g) => g,
        Err(e) => return grant_error(&e),
    };
    emit_state_for(desktop);
    notify("grant", format!("{} let {} watch {}'s screen.", person(p), g.user, machine()));
    ok(json!({"ok": true, "desktop": desktop, "grant": g}))
}

fn revoke(p: &Principal, body: &Value, req: &Req<'_>, desktop: &str) -> CliResponse {
    let Some(user) = str_field(body, "user") else {
        return refuse(400, "bad_request", "send {\"user\": \"<login>\"}");
    };
    if let Err(r) = audit_first(&line(Event::Revoke, desktop, p, req).target(user)) {
        return r;
    }
    let id = match grants::revoke(desktop, user) {
        Ok(id) => id,
        Err(e) => return refuse(500, "store_error", e),
    };
    let closed = match &id {
        Some(id) => viewers::close_user(desktop, id, "", wire::CLOSE_AUTH_REVOKED, wire::reason::GRANT_GONE),
        None => 0,
    };
    emit_state_for(desktop);
    ok(json!({"ok": true, "desktop": desktop, "revoked": id.is_some(), "closed": closed}))
}

fn kick(p: &Principal, body: &Value, req: &Req<'_>, desktop: &str) -> CliResponse {
    let (viewer, user) = (str_field(body, "viewer"), str_field(body, "user"));
    if viewer.is_none() && user.is_none() {
        return refuse(400, "bad_request", "send {\"viewer\": \"dv_…\"} or {\"user\": \"<login>\"} (k2 desktop viewers lists them)");
    }
    if let Err(r) = audit_first(&line(Event::Kick, desktop, p, req).viewer(viewer.unwrap_or("")).target(user.unwrap_or(""))) {
        return r;
    }
    let mut kicked = 0;
    if let Some(v) = viewer {
        kicked += viewers::close_viewer(desktop, v, wire::CLOSE_FORBIDDEN, wire::reason::KICKED);
    }
    if let Some(u) = user {
        let id = grants::user_facts(u).ok().flatten().map(|f| f.id).unwrap_or_default();
        kicked += viewers::close_user(desktop, &id, u, wire::CLOSE_FORBIDDEN, wire::reason::KICKED);
    }
    emit_state_for(desktop);
    ok(json!({"ok": true, "kicked": kicked}))
}

fn read_audit(req: &Req<'_>) -> CliResponse {
    let since = match req.params.get("since").map(String::as_str).filter(|s| !s.trim().is_empty()) {
        None => None,
        Some(s) => match parse_since(s) {
            Ok(t) => Some(t),
            Err(e) => return refuse(400, "bad_request", e),
        },
    };
    let limit = match req.params.get("limit").map(|l| l.trim().parse::<usize>()) {
        None => audit::clamp_limit(None),
        Some(Ok(n)) => audit::clamp_limit(Some(n)),
        Some(Err(_)) => return refuse(400, "bad_request", "limit is a number"),
    };
    match audit::read(since.as_deref(), limit) {
        Ok(lines) => ok(json!({"lines": lines})),
        Err(e) => refuse(500, "audit_unreadable", e),
    }
}

fn open(p: &Principal, a: &Access, body: &Value, req: &Req<'_>, desktop: &str) -> CliResponse {
    if !a.may_view && !a.may_manage {
        return refuse(403, "not_allowed", "you don't have access to this computer's screen (the owner grants it: k2 desktop grant)");
    }
    let display = str_field(body, "display").map(str::to_string);
    if let Err(r) = audit_first(&line(Event::Open, desktop, p, req).display(display.as_deref().unwrap_or(""))) {
        return r;
    }
    // A login: its own clients. The owner token or a K2 shell tab: this
    // computer's own app, and the Owner logins (the person typing in a
    // shell tab usually watches it from another device, signed in).
    let (for_whom, owners) = match p {
        Principal::Login { username, .. } => (username.clone(), Vec::new()),
        Principal::OwnerToken | Principal::Shell => ("owner".to_string(), owner_logins()),
    };
    let mut ev = json!({"desktop": desktop, "display": display, "for": for_whom});
    if !owners.is_empty() {
        ev["owners"] = json!(owners);
    }
    crate::session_events::emit_desktop("open_desktop", ev);
    ok(json!({"ok": true, "opened": true, "desktop": desktop, "for": for_whom}))
}

fn mint_ticket(p: &Principal, a: &Access, body: &Value, req: &Req<'_>, desktop: &str) -> CliResponse {
    let Principal::Login { username, .. } = p else {
        return refuse(
            403,
            "desktop_logins_only",
            "the desktop stream takes a Connect login (sign in from another device); the owner token and K2 terminals manage it but never watch",
        );
    };
    if a.same_machine {
        return refuse(403, "same_machine", "You're on this computer: there's no use watching its own screen");
    }
    if !a.may_view {
        return refuse(403, "not_allowed", "you don't have access to this computer's screen (the owner grants it: k2 desktop grant)");
    }
    match store::is_enabled(vd::switch_id(desktop)) {
        Ok(true) => {}
        Ok(false) => return refuse(409, "desktop_off", "Remote desktop is off on this computer"),
        Err(e) => return refuse(500, "store_error", e),
    }
    let display = str_field(body, "display").map(str::to_string);
    if let Err(r) = audit_first(&line(Event::Ticket, desktop, p, req).display(display.as_deref().unwrap_or(""))) {
        return r;
    }
    let t = ticket::mint(ticket::Claim {
        desktop: desktop.to_string(),
        username: username.clone(),
        session_token: req.token.to_string(),
        display,
    });
    ok(json!({"ok": true, "desktop": desktop, "ticket": t, "expiresIn": ticket::TTL.as_secs()}))
}

#[cfg(test)]
#[path = "routes_tests.rs"]
mod tests;
