//! The virtual desktop routes (`prd-linux-virtual-desktop-v1` L4):
//! `GET /cli/desktop/list`, `POST /cli/desktop/start|stop|setup`, the
//! `virtual` part of status, and the glue between the manager
//! ([`super::virtual_desktop`]) and K2 Screen, the stream and the events.
//! `routes::handle` has already classified the caller (people only) and
//! resolved the desktop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use k2_core::connect_users::Role;
use k2_core::desktop::audit::{self, AuditLine, Event};
use k2_core::desktop::policy::Principal;
use k2_core::desktop::virtual_desktop::{self as vd, Asker, CloseReason, Size, State};
use k2_core::desktop::{grants, store, viewers, wire};
use k2_deskd_proto::msg::ReleaseReason;
use serde_json::{json, Value};

use super::deskd;
use super::routes::{audit_first, current_desktop, line, linux_here, linux_only, may_start, ok, person, refuse, refuse_with, Req};
use super::virtual_desktop::{manager, Screen};
use crate::cli_response::CliResponse;

/// "This desktop no longer exists." (its agent's workspace was removed).
pub const GONE: &str = "This desktop no longer exists.";

/// The workspace behind an agent's desktop: `(project id, name)`, `None`
/// when it is gone (or the id isn't an agent's).
fn agent_workspace(id: &str) -> Option<(String, String)> {
    vd::workspace_of(id)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::desktop_workspace(&conn, id)
}

fn agent_name(id: &str) -> Option<String> {
    agent_workspace(id).map(|(_, name)| name)
}

fn agent_switch(id: &str) -> bool {
    let Some((ws, _)) = agent_workspace(id) else { return false };
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::agents_can_use_desktop(&conn, &ws)
}

/// A hold's holder for people (LD-30): the agent's address
/// (`<agent>` or `<agent>/<sidecar>`) as "<agent>" or "<agent> (<sidecar>)".
pub fn hold_name(by: &str) -> String {
    match by.split_once('/') {
        Some((agent, sidecar)) if !sidecar.is_empty() => format!("{agent} ({sidecar})"),
        _ => by.trim_end_matches('/').to_string(),
    }
}

/// May this person open that desktop (the tab's picker, LD-36)?
fn may_open(p: &Principal, id: &str) -> bool {
    match p {
        Principal::OwnerToken | Principal::Shell => true,
        Principal::Login { role: Role::Owner | Role::Admin, .. } => true,
        Principal::Login { role: Role::Member, username } => grants::live_grant(id, username).ok().flatten().is_some(),
        Principal::Login { role: Role::VoiceChat, .. } => false,
    }
}

/// A desktop's state on the wire, and its sentence.
pub(crate) fn state_of(id: &str) -> (String, String) {
    if vd::is_virtual(id) {
        if vd::workspace_of(id).is_some() && agent_name(id).is_none() {
            return ("gone".into(), GONE.into());
        }
        if setting_up() {
            return ("setting_up".into(), SETTING_UP_MESSAGE.into());
        }
        let st = manager().status(id);
        let state = st.wire_state();
        if state == "not_set_up" {
            if let Some(why) = setup_failed() {
                return (state.to_string(), failed_message(&why, st.manual.as_deref()));
            }
        }
        (state.to_string(), st.message())
    } else {
        let s = deskd::current().state();
        (s.as_wire().to_string(), s.message())
    }
}

/// `GET /cli/desktop/list[?workspace=<id>]`: what this person may open.
pub fn list(p: &Principal, req: &Req<'_>) -> CliResponse {
    let server = current_desktop();
    let mut out = Vec::new();
    if may_open(p, &server) {
        let (state, _) = state_of(&server);
        out.push(json!({"id": server, "kind": "server", "name": "This server", "state": state}));
    }
    if linux_here() && store::is_enabled(vd::VIRTUAL).unwrap_or(false) {
        let mut agents: Vec<(String, String)> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            match store::desktop_agents(&conn) {
                Ok(a) => a,
                Err(e) => return refuse(500, "store_error", e),
            }
        };
        // A desktop that exists stays listed after its switch goes off.
        for (id, st) in manager().known() {
            if st == State::Closed {
                continue;
            }
            let Some((ws, name)) = agent_workspace(&id) else { continue };
            if !agents.iter().any(|(w, _)| *w == ws) {
                agents.push((ws, name));
            }
        }
        for (ws, name) in agents {
            let Some(id) = vd::agent_desktop(&ws) else { continue };
            if !may_open(p, &id) {
                continue;
            }
            let (state, _) = state_of(&id);
            out.push(json!({"id": id, "kind": "agent", "name": name, "workspaceId": ws, "state": state, "label": vd::SEPARATE_LABEL}));
        }
    }
    let default = req
        .params
        .get("workspace")
        .and_then(|w| vd::agent_desktop(w.trim()))
        .filter(|id| agent_switch(id) && out.iter().any(|d| d["id"] == json!(id)))
        .unwrap_or(server);
    ok(json!({"default": default, "desktops": out}))
}

/// The `virtual` part of `GET /cli/desktop/status` (and the state).
pub fn fill_status(v: &mut Value, id: &str, screen: &str) {
    let st = manager().status(id);
    let (state, message) = state_of(id);
    v["state"] = json!(state);
    v["message"] = json!(message);
    v["reason"] = st.reason_json();
    let mut label = None;
    if vd::workspace_of(id).is_some() {
        let ws = agent_workspace(id);
        v["kind"] = json!("agent");
        v["name"] = json!(ws.as_ref().map(|(_, n)| n.clone()).unwrap_or_default());
        v["workspaceId"] = json!(ws.as_ref().map(|(w, _)| w.clone()));
        v["agentsAllowed"] = json!(agent_switch(id));
        label = Some(vd::SEPARATE_LABEL);
    }
    let mut virt = st.virtual_json(label);
    virt["screen"] = json!(screen);
    virt["setup"] = setup_json();
    if let Some(h) = virt.get_mut("hold").filter(|h| h.is_object()) {
        let name = h["by"].as_str().map(hold_name).unwrap_or_default();
        h["name"] = json!(name);
    }
    v["virtual"] = virt;
}

/// The status object for a desktop (start answers with it).
fn status_reply(p: &Principal, req: &Req<'_>, id: &str) -> CliResponse {
    let params = std::collections::HashMap::from([("desktop".to_string(), id.to_string())]);
    let r = Req { path: "/cli/desktop/status", is_post: false, body: b"", params: &params, ingress: req.ingress, ip: req.ip, token: req.token };
    let who = match p {
        Principal::OwnerToken => crate::compute_routes::Who::Owner,
        Principal::Shell => crate::compute_routes::Who::Shell { project_id: String::new(), session_id: String::new() },
        Principal::Login { username, role } => crate::compute_routes::Who::Login { username: username.clone(), role: *role },
    };
    super::routes::handle(&who, &r)
}

fn size_of(body: &Value) -> Result<Option<Size>, CliResponse> {
    let n = |k: &str| match body.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().filter(|n| *n <= 100_000).map(|n| Some(n as u32)).ok_or_else(|| refuse(400, "bad_request", format!("{k} is the pane's size in pixels"))),
    };
    match (n("width")?, n("height")?) {
        (Some(width), Some(height)) => Ok(Some(Size { width, height })),
        (None, None) => Ok(None),
        _ => Err(refuse(400, "bad_request", "send width and height together (the pane's size), or neither")),
    }
}

/// Start (or wake) needs: Linux, a person who may, a virtual id, the
/// switch on, and for an agent's desktop its workspace and switch.
fn may_run(p: &Principal, id: &str, what: &str) -> Result<(), CliResponse> {
    if !linux_here() {
        return Err(linux_only());
    }
    if !may_start(p) {
        return Err(refuse(403, "owner_or_admin", format!("{what} a desktop is for Owners and Admins. You can watch one an Owner or Admin started.")));
    }
    if !vd::is_virtual(id) {
        return Err(refuse(409, "not_virtual", "This server's desktop is a real screen session (console): K2 doesn't start or close it."));
    }
    Ok(())
}

/// `POST /cli/desktop/start {desktop?, width?, height?}`.
pub fn start(p: &Principal, body: &Value, req: &Req<'_>, id: &str) -> CliResponse {
    if let Err(r) = may_run(p, id, "Starting") {
        return r;
    }
    if setting_up() {
        return refuse(409, "setting_up", "The desktop packages are being installed: start it when that ends (k2 desktop status).");
    }
    let size = match size_of(body) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match store::is_enabled(vd::VIRTUAL) {
        Ok(true) => {}
        Ok(false) => return refuse(409, "desktop_off", "Remote desktop is off on this server: an Owner turns it on (k2 desktop enable)"),
        Err(e) => return refuse(500, "store_error", e),
    }
    let m = manager();
    if vd::workspace_of(id).is_some() {
        let Some(name) = agent_name(id) else { return refuse(404, "desktop_gone", GONE) };
        if m.state(id) == State::Closed && !agent_switch(id) {
            return refuse(
                409,
                "agent_desktop_off",
                format!("{name} isn't allowed a virtual desktop: a person turns on \"Allow a virtual desktop\" in Settings → Workspaces → {name} → Agent."),
            );
        }
    }
    if let Err(r) = audit_first(&line(Event::DesktopStart, id, p, req).outcome("requested")) {
        return r;
    }
    match m.start(id, Asker::Person, size) {
        Ok(_) => status_reply(p, req, id),
        Err(e) => {
            let extra = match &e {
                super::virtual_desktop::StartError::TooMany { closable } => json!({"closable": closable}),
                super::virtual_desktop::StartError::NotSetUp { missing, manual } => json!({"missing": missing, "manual": manual}),
                super::virtual_desktop::StartError::Failed(d) => json!({"detail": d}),
                _ => json!({}),
            };
            refuse_with(e.status(), e.code(), e.hint(&m.limits()), extra)
        }
    }
}

/// `POST /cli/desktop/stop {desktop?}`: close it (what was open is lost).
pub fn stop(p: &Principal, req: &Req<'_>, id: &str) -> CliResponse {
    if let Err(r) = may_run(p, id, "Closing") {
        return r;
    }
    if let Err(r) = audit_first(&line(Event::DesktopClose, id, p, req).outcome("closed_by")) {
        return r;
    }
    let reason = CloseReason::ClosedBy { by: person(p) };
    let message = reason.message();
    let closed = manager().close_with(id, reason);
    ok(json!({"ok": true, "desktop": id, "closed": closed, "state": "closed", "message": message}))
}

// ── setup: the packages (L5's root-helper verb) ──────────────────────

/// The root helper's `desktop packages install`, behind a seam for tests.
pub trait SetupDoor: Send + Sync {
    /// Protocol 16 or later with `namespaces.desktop`.
    fn helper_ok(&self) -> bool;
    fn install(&self) -> Result<String, String>;
}

struct RealDoor;

impl SetupDoor for RealDoor {
    fn helper_ok(&self) -> bool {
        let st = crate::root_helper::state();
        let ns = st.version_json.as_ref().and_then(|v| v.pointer("/namespaces/desktop")).and_then(Value::as_bool) == Some(true);
        st.usable(k2_root_proto::version::DESKTOP_MIN_PROTOCOL) && ns
    }
    fn install(&self) -> Result<String, String> {
        crate::root_helper::call_text(&["desktop".to_string(), "packages".to_string(), "install".to_string()], None)
    }
}

static DOOR: RwLock<Option<Arc<dyn SetupDoor>>> = RwLock::new(None);
static SETTING_UP: AtomicBool = AtomicBool::new(false);
/// The last install's failure (cleared when another starts).
static SETUP_FAILED: Mutex<Option<String>> = Mutex::new(None);

/// What `setting_up` says.
pub const SETTING_UP_MESSAGE: &str = "Installing the desktop packages…";

fn door() -> Arc<dyn SetupDoor> {
    DOOR.read().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| Arc::new(RealDoor))
}

/// Tests: a fake root helper until the guard drops.
pub fn door_for_test(d: Arc<dyn SetupDoor>) -> DoorGuard {
    *DOOR.write().unwrap_or_else(|p| p.into_inner()) = Some(d);
    DoorGuard
}

pub struct DoorGuard;

impl Drop for DoorGuard {
    fn drop(&mut self) {
        *DOOR.write().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// The packages are being installed now.
pub(crate) fn setting_up() -> bool {
    SETTING_UP.load(Ordering::SeqCst)
}

fn setup_failed() -> Option<String> {
    SETUP_FAILED.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// The `not_set_up` sentence after a failed install.
fn failed_message(why: &str, manual: Option<&str>) -> String {
    match manual {
        Some(m) => format!("The desktop packages didn't install: {why}. Set up again, or run this by hand: {m}"),
        None => format!("The desktop packages didn't install: {why}. Set up again."),
    }
}

/// `virtual.setup` in status: whether an install runs, and the last one's
/// failure (`null` when it didn't fail or another runs).
pub fn setup_json() -> Value {
    json!({"running": setting_up(), "failed": setup_failed()})
}

/// `desktop_state` for every virtual desktop K2 knows (the packages are
/// the server's, so every one's state moves with them).
fn emit_every_virtual() {
    let mut ids = vec![vd::VIRTUAL.to_string()];
    for (id, _) in manager().known() {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    for id in ids {
        super::routes::emit_state_for(&id);
    }
}

/// `POST /cli/desktop/setup {}` (Owner): install the packages through the
/// root helper, or print the command when it can't. Answers
/// `{"ok": true, "state": "setting_up"}` at once and installs in the
/// background; `desktop_state` events follow it until the state is
/// `closed` (installed) or `not_set_up` with why (contract note 11-11).
pub fn setup(p: &Principal, req: &Req<'_>) -> CliResponse {
    if !linux_here() {
        return linux_only();
    }
    if !matches!(p, Principal::OwnerToken | Principal::Shell | Principal::Login { role: Role::Owner, .. }) {
        return refuse(403, "owner_only", "Installing the desktop packages is the Owner's: an Owner login, or the owner at this server");
    }
    if setting_up() {
        return refuse(409, "setting_up", "The desktop packages are already being installed.");
    }
    let m = manager();
    if m.missing().is_empty() {
        return ok(json!({"ok": true, "state": "closed", "installed": [], "message": "The desktop packages are already installed."}));
    }
    let manual = m.manual();
    let d = door();
    if !d.helper_ok() {
        return refuse_with(
            409,
            "setup_manual",
            format!(
                "K2 can't install packages on this server itself (no root helper, or one older than protocol {}). Run this, then try again: {}",
                k2_root_proto::version::DESKTOP_MIN_PROTOCOL,
                manual.as_deref().unwrap_or("")
            ),
            json!({"manual": manual}),
        );
    }
    if SETTING_UP.swap(true, Ordering::SeqCst) {
        return refuse(409, "setting_up", "The desktop packages are already being installed.");
    }
    if let Err(r) = audit_first(&line(Event::DesktopSetup, vd::VIRTUAL, p, req).outcome("started")) {
        SETTING_UP.store(false, Ordering::SeqCst);
        return r;
    }
    *SETUP_FAILED.lock().unwrap_or_else(|p| p.into_inner()) = None;
    emit_every_virtual();
    let done_line = line(Event::DesktopSetup, vd::VIRTUAL, p, req);
    let spawned = std::thread::Builder::new().name("k2-vdesktop-setup".into()).spawn(move || {
        let r = d.install();
        let failed = match &r {
            Ok(_) if !manager().missing().is_empty() => Some(format!("still missing {}", manager().missing().join(", "))),
            Ok(_) => None,
            Err(e) => Some(e.trim().to_string()),
        };
        if let Err(e) = audit::write(&done_line.outcome(if failed.is_none() { "ok" } else { "failed" })) {
            k2_core::log_debug!("[desktop] audit write failed (setup): {e}");
        }
        match &r {
            Ok(text) => k2_core::log_debug!("[desktop] setup: {}", text.trim()),
            Err(e) => k2_core::log_debug!("[desktop] setup failed: {e}"),
        }
        *SETUP_FAILED.lock().unwrap_or_else(|p| p.into_inner()) = failed;
        SETTING_UP.store(false, Ordering::SeqCst);
        emit_every_virtual();
    });
    if let Err(e) = spawned {
        SETTING_UP.store(false, Ordering::SeqCst);
        emit_every_virtual();
        return refuse_with(500, "setup_failed", format!("K2 couldn't start the install: {e}"), json!({"manual": manual}));
    }
    ok(json!({"ok": true, "state": "setting_up", "message": SETTING_UP_MESSAGE}))
}

// ── the glue: K2 Screen, the stream, events, the access log ──────────

/// The manager's [`Screen`]: K2 Screen per desktop, and its viewers.
struct VirtualScreen;

impl Screen for VirtualScreen {
    fn release_keys(&self, id: &str) {
        if let Some(s) = deskd::for_desktop(id).stream() {
            s.release_all(ReleaseReason::Other);
        }
    }

    fn close(&self, id: &str, state: State) {
        let (reason, message) = match state {
            State::Asleep => (wire::reason::DESKTOP_ASLEEP, State::Asleep.message(0)),
            _ => (wire::reason::DESKTOP_CLOSED, "The desktop was closed.".to_string()),
        };
        super::stream::tell_state(id, state.as_wire(), &message);
        viewers::close_all(id, wire::CLOSE_DESKTOP_OFF, reason);
        if let Err(e) = deskd::for_desktop(id).disable() {
            k2_core::log_debug!("[desktop] K2 Screen for {id}: {e}");
        }
    }
}

/// Every state change: the event to clients, the access log for what K2
/// did on its own (sleep, wake, close), and K2 Screen on an awake desktop.
fn on_change(id: &str, state: State, why: Option<&CloseReason>) {
    let ev = match state {
        State::Asleep => Some(AuditLine::new(Event::DesktopSleep, id, "k2")),
        State::Waking => Some(AuditLine::new(Event::DesktopWake, id, "k2")),
        State::Closed => {
            let mut l = AuditLine::new(Event::DesktopClose, id, "k2").outcome(why.map_or("closed", |w| w.code()));
            if let Some(CloseReason::ClosedBy { by }) = why {
                l = l.target(by);
            }
            Some(l)
        }
        _ => None,
    };
    if let Some(l) = ev {
        if let Err(e) = audit::write(&l) {
            k2_core::log_debug!("[desktop] audit write failed ({}): {e}", l.event.as_str());
        }
    }
    if state == State::Ready {
        let id = id.to_string();
        let _ = std::thread::Builder::new().name("k2-vdesktop-screen".into()).spawn(move || {
            if let Err(e) = deskd::for_desktop(&id).enable() {
                k2_core::log_debug!("[desktop] K2 Screen for {id}: {e}");
            }
            super::routes::emit_state_for(&id);
        });
    }
    super::routes::emit_state_for(id);
}

/// Boot (and tests): the manager talks to K2 Screen and the clients.
pub fn install_hooks(m: &super::virtual_desktop::Manager) {
    m.set_screen(Arc::new(VirtualScreen));
    m.set_on_change(Arc::new(on_change));
}
