//! `GET /cli/desktop/stream` (`prd-remote-desktop-tab-v1` §4–§6, D6): this
//! machine's screen over a WebSocket, on K2 Render's stream engine
//! ([`crate::stream::serve`]), and the controller's keyboard and mouse.
//!
//! ```text
//! GET /cli/desktop/stream?ticket=<k2dt_…>&codecs=h264,vp8,jpeg&win=3[&display=<id>][&fps=n][&view=1][&stats=1][&pw=<px>]
//! ```
//!
//! **The gate, before the upgrade** ([`admit`], the §4 table with Rosson's
//! answers of 2026-11-10). Only a single-use ticket opens it (vs-live R20:
//! no session token in the URL); the ticket names a login and its session,
//! and that session must still be live. Then `k2_core::desktop::policy`:
//! Owners and Admins watch and drive; a Member only with a grant, and only
//! watches; Voice & Chat only never. The owner token, a K2 shell tab, an
//! agent passport, an App pass, an API key or a `k2rs_` grant has no
//! ticket, so it never upgrades. A caller on this machine (loopback) is
//! refused `same_machine` with no frame (vs-live R19: viewing your own
//! screen has no use, and refusing it closes a local path). The access-log
//! line is written before the viewer connects, and a viewer whose line
//! can't be written is refused (R33).
//!
//! **While open** ([`Auth`]):
//! - every [`AUTH_TICK`] (5 s): the session, the role and the grant again
//!   (a revoke, a sign-out, a grant gone, an Owner moved to Member with no
//!   grant: 4401), the switch (off: 4410), and S8's timeouts: 30 min with no
//!   input while the tab is hidden (`fps 0`), 8 h in all (4403); a
//!   controller who stops having the right to drive is handed back;
//! - every client frame: [`crate::socket_access::IncomingCheck`] (the
//!   security review's L7 guard, as the sim route does);
//! - every input message (R17): may this login drive now (re-read at most
//!   every 250 ms), does this viewer hold the control lease, and is someone
//!   at the machine using it (local input wins for 3 s; input then is
//!   dropped, never queued). A refused input gets an `error` and the first
//!   in each 10 s goes in the access log.
//!
//! **The lease** (`k2_core::desktop::lease`): one controller at a time.
//! A viewer that opens in control mode takes it when nobody drives;
//! `take-control` / `view {on:false}` take it, `hand-back` / `view {on:true}`
//! give it back, a controller idle for 10 min loses it, and a controller's
//! socket closing frees it. Every loss asks deskd to let go of every held
//! key and button (vs-live R3). Each change goes to every viewer as
//! `control`, and to the access log.
//!
//! **Displays.** One capture per desktop (the v1 rule): the hub is keyed
//! by desktop and the display is a property of its source. `display=` on
//! the URL is honoured only when no one is watching yet; a viewer joining
//! a running desktop gets the current display. Only the controller may
//! switch (`display {id}`), and a switch moves every viewer.
//!
//! **Rates and skips** are the engine's: the desktop row of the rate table
//! (`crate::stream::rates`, PRD §3.5) and, with K2 Render's S4, the
//! skip-to-keyframe thresholds (max(2 × srtt, 250 ms) of unacked age, or a
//! send over 100 ms), for every route on `serve`.
//!
//! **Consent at the screen** was dropped by Rosson (2026-11-10: no
//! per-session Allow click), so a viewer is never held waiting.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use k2_core::connect_users::{self, Role};
use k2_core::desktop::audit::{self, AuditLine, Event};
use k2_core::desktop::input::{check_input, InputBudget};
use k2_core::desktop::lease::{LeaseChange, TakeRefused};
use k2_core::desktop::policy::{self, Principal};
use k2_core::desktop::viewers::{self, ViewerHandle, ViewerInfo};
use k2_core::desktop::wire::{self, Button, Display, Encoder, HostOs, Lane, Mode, ServerMsg, ViewerMsg, ViewerName, WheelMode};
use k2_core::desktop::{grants, store, ticket, MachineState};
use k2_core::sim::stream::{Codec, ViewerPath};
use k2_core::stream::{EncodedSource, SourceKey, SourceKind};
use k2_deskd_proto::msg::{Button as DeskdButton, Display as DeskdDisplay, InputEvent, ReleaseReason, WheelMode as DeskdWheel};

use crate::stream::viewer::WsFrame;
use crate::stream::{Auth, DesktopRule, EngineMsg, Hooks, InputSink, JoinError, Negotiated, Offer, PointerWheel, Push, Refusal, ServeError, Source, StreamMsg, Verdict, Wheel, AUTH_TICK};

pub const ROUTE: &str = "/cli/desktop/stream";

/// S8: a viewer with no input while its tab is hidden closes after this.
pub const VIEWER_IDLE: Duration = Duration::from_secs(30 * 60);
/// S8: every viewer closes after this.
pub const VIEWER_MAX: Duration = Duration::from_secs(8 * 3600);
/// How long a "may this login drive" answer is reused (R17: ≤ 250 ms).
pub const CONTROL_CACHE: Duration = Duration::from_millis(250);
/// At most one access-log line for refused input per viewer per this.
pub const REFUSAL_LOG_GAP: Duration = Duration::from_secs(10);

// ── what the stream needs from deskd ─────────────────────────────────

/// What deskd tells the stream on its own.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Someone at the machine is using it (`hold`), or stopped for 3 s.
    LocalInput { hold: bool },
    /// Disconnect on the banner: every viewer closes.
    BannerDisconnect,
    /// A cursor shape (once per shape).
    Cursor { id: u32, hot_x: u32, hot_y: u32, png: String },
    CursorPos { x: f32, y: f32, visible: bool },
    DisplaysChanged(Vec<DeskdDisplay>),
    Locked(bool),
    /// deskd can't run a layer (no encoder for its codec, a session that
    /// won't open).
    LayerFailed { layer: u8, why: String },
    /// deskd's capture stopped or couldn't start (no Screen Recording, the
    /// display went away); deskd tries again every 2 s.
    CaptureError { code: String, why: String },
}

/// The stream's side of k2-deskd (the routes' side is
/// [`super::deskd::Deskd`]). The daemon's real one is
/// [`super::managed::ManagedDeskd`]; tests give a fake.
pub trait StreamDeskd: Send + Sync {
    fn os(&self) -> HostOs;
    /// The H.264 encoder deskd opened (hello).
    fn encoder(&self) -> Encoder;
    /// The video codecs deskd encodes (protocol-4 hello `codecs`).
    fn codecs(&self) -> Vec<Codec>;
    /// Does deskd make JPEG too? (Not on a Mac: H.264 only. A viewer that
    /// can't decode its codecs is refused, and a failed layer has nowhere
    /// to go.)
    fn jpeg(&self) -> bool;
    /// deskd failed `layer`: hand it to the hub (a video layer's viewers
    /// move to JPEG; a failed JPEG layer, the Mac's case, ends its viewers
    /// with the reason, shown as the "picture stopped" card). False when
    /// no frames pipe is open to take it: the stream ends every viewer.
    fn fail_layer(&self, layer: u8, why: &str) -> bool;
    /// Can deskd tell the person's input from ours (not on Wayland)?
    fn local_wins(&self) -> bool;
    fn displays(&self) -> Result<Vec<DeskdDisplay>, String>;
    /// A protocol-4 source for a new hub, capturing `display`.
    fn source(&self, display: u32, displays: &[DeskdDisplay]) -> Result<Box<dyn EncodedSource>, String>;
    fn select_display(&self, display: u32) -> Result<(), String>;
    fn inject(&self, ev: InputEvent) -> Result<(), String>;
    fn release_all(&self, why: ReleaseReason);
    /// `(text, reason)`: no text with a reason when deskd refused
    /// (`concealed`: a password manager's copy).
    fn clipboard_get(&self) -> Result<(Option<String>, Option<String>), String>;
    fn clipboard_set(&self, text: &str) -> Result<(), String>;
    fn banner(&self, text: Option<String>);
    fn events(&self) -> std::sync::mpsc::Receiver<StreamEvent>;
}

/// A deskd display as the wire has it.
pub fn wire_display(d: &DeskdDisplay) -> Display {
    Display {
        id: d.id.to_string(),
        name: d.name.clone(),
        width_pt: d.width_pt.round().max(0.0) as u32,
        height_pt: d.height_pt.round().max(0.0) as u32,
        width_px: d.width_px,
        height_px: d.height_px,
        scale: d.scale as f32,
        main: d.main,
    }
}

fn first_display(list: &[DeskdDisplay]) -> u32 {
    list.iter().find(|d| d.main).or(list.first()).map_or(0, |d| d.id)
}

// ── one desktop's viewers ────────────────────────────────────────────

struct Member {
    name: String,
    owner: bool,
    push: mpsc::UnboundedSender<Push>,
}

#[derive(Default)]
struct Session {
    lease: k2_core::desktop::lease::Lease,
    members: HashMap<String, Member>,
    display: Option<u32>,
    displays: Vec<DeskdDisplay>,
    pump: bool,
    /// The last cursor shape, for a joiner.
    cursor: Option<String>,
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static S: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_session<R>(desktop: &str, f: impl FnOnce(&mut Session) -> R) -> R {
    let mut m = sessions().lock().unwrap_or_else(|p| p.into_inner());
    f(m.entry(desktop.to_string()).or_default())
}

fn text(m: &ServerMsg) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

fn control_text(s: &Session, viewer: &str, now: Instant) -> String {
    text(&ServerMsg::Control { controller: s.lease.controller().map(str::to_string), you: s.lease.holds(viewer), local_hold: s.lease.local_hold(now) })
}

fn displays_text(s: &Session) -> String {
    text(&ServerMsg::Displays { displays: s.displays.iter().map(wire_display).collect(), display: s.display.map(|d| d.to_string()) })
}

/// `control` to every viewer.
fn broadcast_control(desktop: &str) {
    let now = Instant::now();
    with_session(desktop, |s| {
        for (id, m) in &s.members {
            let _ = m.push.send(Push::Text(control_text(s, id, now)));
        }
    });
}

/// `viewers` to the Owners watching.
fn broadcast_viewers(desktop: &str) {
    let list: Vec<ViewerName> = viewers::list(desktop).into_iter().map(|v| ViewerName { name: v.user, mode: v.mode }).collect();
    let t = text(&ServerMsg::Viewers { viewers: list });
    with_session(desktop, |s| {
        for m in s.members.values().filter(|m| m.owner) {
            let _ = m.push.send(Push::Text(t.clone()));
        }
    });
}

fn broadcast_text(desktop: &str, t: String) {
    with_session(desktop, |s| {
        for m in s.members.values() {
            let _ = m.push.send(Push::Text(t.clone()));
        }
    });
}

/// The in-use banner's words: who watches and who drives.
pub fn banner_text(entries: &[(String, Mode)]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut driving: Vec<&str> = entries.iter().filter(|(_, m)| *m == Mode::Control).map(|(n, _)| n.as_str()).collect();
    let mut watching: Vec<&str> = entries.iter().filter(|(_, m)| *m == Mode::View).map(|(n, _)| n.as_str()).collect();
    driving.dedup();
    watching.sort_unstable();
    watching.dedup();
    let mut parts = Vec::new();
    if !driving.is_empty() {
        parts.push(format!("{} is controlling", driving.join(", ")));
    }
    if !watching.is_empty() {
        let verb = if watching.len() == 1 { "is" } else { "are" };
        parts.push(format!("{} {verb} viewing", watching.join(", ")));
    }
    Some(format!("{} this screen through K2", parts.join(" and ")))
}

fn refresh_banner(desktop: &str, deskd: &Arc<dyn StreamDeskd>) {
    let entries: Vec<(String, Mode)> = viewers::list(desktop).into_iter().map(|v| (v.user, v.mode)).collect();
    let d = deskd.clone();
    let _ = std::thread::Builder::new().name("k2-desktop-banner".into()).spawn(move || d.banner(banner_text(&entries)));
}

fn release_in_background(deskd: &Arc<dyn StreamDeskd>, why: ReleaseReason) {
    let d = deskd.clone();
    let _ = std::thread::Builder::new().name("k2-desktop-release".into()).spawn(move || d.release_all(why));
}

/// One lease change: modes, the access log, held input, every viewer.
fn lease_changed(desktop: &str, deskd: &Arc<dyn StreamDeskd>, change: LeaseChange, why: ReleaseReason) {
    let (event, viewer, outcome) = match &change {
        LeaseChange::Taken { viewer } => (Event::ControlTaken, viewer.clone(), "ok"),
        LeaseChange::HandedBack { viewer } => (Event::ControlHandedBack, viewer.clone(), "ok"),
        LeaseChange::TimedOut { viewer } => (Event::ControlTimedOut, viewer.clone(), "idle"),
        LeaseChange::Dropped { viewer } => (Event::ControlHandedBack, viewer.clone(), "closed"),
    };
    let taken = matches!(change, LeaseChange::Taken { .. });
    viewers::set_mode(&viewer, if taken { Mode::Control } else { Mode::View });
    let who = with_session(desktop, |s| s.members.get(&viewer).map(|m| m.name.clone())).unwrap_or_default();
    if let Err(e) = audit::write(&AuditLine::new(event, desktop, &format!("login:{who}")).viewer(&viewer).outcome(outcome)) {
        k2_core::log_debug!("[desktop] audit write failed ({}): {e}", event.as_str());
    }
    if !taken {
        release_in_background(deskd, why);
    }
    broadcast_control(desktop);
    broadcast_viewers(desktop);
    refresh_banner(desktop, deskd);
}

/// deskd's events to the viewers, while anyone watches.
fn ensure_pump(desktop: &str, deskd: &Arc<dyn StreamDeskd>) {
    let start = with_session(desktop, |s| !std::mem::replace(&mut s.pump, true));
    if !start {
        return;
    }
    let rx = deskd.events();
    let (name, d) = (desktop.to_string(), deskd.clone());
    let spawned = std::thread::Builder::new().name("k2-desktop-events".into()).spawn(move || loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => on_event(&name, &d, ev),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if with_session(&name, |s| {
                    let idle = s.members.is_empty();
                    if idle {
                        s.pump = false;
                    }
                    idle
                }) {
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                with_session(&name, |s| s.pump = false);
                return;
            }
        }
    });
    if spawned.is_err() {
        with_session(desktop, |s| s.pump = false);
    }
}

fn on_event(desktop: &str, deskd: &Arc<dyn StreamDeskd>, ev: StreamEvent) {
    match ev {
        StreamEvent::LocalInput { hold } => {
            let started = hold && with_session(desktop, |s| s.lease.local_input(Instant::now()));
            if started {
                let _ = audit::write(&AuditLine::new(Event::LocalHold, desktop, "local"));
            }
            broadcast_control(desktop);
        }
        StreamEvent::BannerDisconnect => {
            let n = viewers::close_all(desktop, wire::CLOSE_FORBIDDEN, wire::reason::DISCONNECTED_AT_SCREEN);
            k2_core::log_debug!("[desktop] Disconnect at the screen closed {n} viewer(s)");
            refresh_banner(desktop, deskd);
        }
        StreamEvent::Cursor { id, hot_x, hot_y, png } => {
            let t = text(&ServerMsg::Cursor { id, hot_x, hot_y, png });
            with_session(desktop, |s| s.cursor = Some(t.clone()));
            broadcast_text(desktop, t);
        }
        StreamEvent::CursorPos { x, y, visible } => broadcast_text(desktop, text(&ServerMsg::CursorPos { x, y, visible })),
        StreamEvent::DisplaysChanged(list) => {
            let t = with_session(desktop, |s| {
                s.displays = list;
                displays_text(s)
            });
            broadcast_text(desktop, t);
        }
        StreamEvent::LayerFailed { layer, why } => {
            if deskd.fail_layer(layer, &why) {
                k2_core::log_debug!("[desktop] layer {layer} failed ({why}); the hub moves or ends its viewers");
                return;
            }
            k2_core::log_debug!("[desktop] layer {layer} failed ({why}) with no frames pipe; closing the viewers");
            broadcast_text(desktop, text(&ServerMsg::State { state: "unavailable".into(), message: picture_failed_text(&why) }));
            viewers::close_all(desktop, wire::CLOSE_DESKTOP_OFF, wire::reason::PICTURE_FAILED);
        }
        StreamEvent::CaptureError { code, why } => {
            let st = if code == "no_screen_permission" { MachineState::NeedsPermission } else { MachineState::Unavailable(String::new()) };
            let message = if code == "no_screen_permission" { st.message() } else { format!("K2 Screen's capture stopped ({why}); it tries again in a moment.") };
            broadcast_text(desktop, text(&ServerMsg::State { state: st.as_wire().into(), message }));
        }
        StreamEvent::Locked(on) => {
            let st = if on { MachineState::Locked } else { MachineState::Ready };
            broadcast_text(desktop, text(&ServerMsg::State { state: st.as_wire().into(), message: st.message() }));
        }
    }
}

/// The card a viewer ends on when the picture can't go on.
pub fn picture_failed_text(why: &str) -> String {
    let why = why.trim().trim_end_matches('.');
    if why.is_empty() {
        "The picture stopped. Try again.".into()
    } else {
        format!("The picture stopped: {why}. Try again.")
    }
}

// ── the gate ─────────────────────────────────────────────────────────

/// Why a viewer wasn't let in (an HTTP answer before the upgrade).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub code: &'static str,
    pub hint: String,
    /// The machine's state, for `desktop_unavailable`.
    pub state: Option<&'static str>,
}

impl Refused {
    fn new(status: u16, code: &'static str, hint: impl Into<String>) -> Self {
        Refused { status, code, hint: hint.into(), state: None }
    }

    pub fn body(&self) -> String {
        let mut e = json!({"code": self.code, "hint": self.hint});
        if let Some(s) = self.state {
            e["state"] = json!(s);
        }
        json!({"ok": false, "error": e, "code": self.code}).to_string()
    }

    fn status_line(&self) -> &'static str {
        match self.status {
            401 => "401 Unauthorized",
            403 => "403 Forbidden",
            409 => "409 Conflict",
            503 => "503 Service Unavailable",
            _ => "500 Internal Server Error",
        }
    }
}

/// Who the gate let in.
pub struct Admitted {
    pub desktop: String,
    pub username: String,
    pub user_id: String,
    pub role: Role,
    /// The ticket's session: re-checked every tick (never logged).
    pub session: String,
    pub may_control: bool,
    /// The display the ticket or the URL asked for.
    pub display: Option<u32>,
    pub deskd: Arc<dyn StreamDeskd>,
    pub displays: Vec<DeskdDisplay>,
}

/// The §4 gate (blocking: it reads the login store and the database).
/// `ingress` is the dispatcher's label (`loopback`, `lan`, `tunnel`, `app`).
pub fn admit(params: &HashMap<String, String>, ingress: &str) -> Result<Admitted, Refused> {
    if ingress == "app" {
        return Err(Refused::new(403, super::routes::PEOPLE_ONLY, "Remote desktop isn't served on an App's address"));
    }
    let Some(t) = params.get("ticket").map(|t| t.trim()).filter(|t| !t.is_empty()) else {
        return Err(Refused::new(
            403,
            "desktop_logins_only",
            "the desktop stream opens with a ticket from POST /cli/desktop/ticket, which takes a Connect login (sign in from another device); the owner token, K2 terminals and agents never watch",
        ));
    };
    let Some(claim) = ticket::redeem(t) else {
        return Err(Refused::new(401, "ticket_invalid", "that ticket is spent, expired or unknown: ask POST /cli/desktop/ticket for a new one (each opens one stream, within 60 s)"));
    };
    let Some((username, role)) = connect_users::validate_session_role(&claim.session_token) else {
        return Err(Refused::new(401, wire::reason::AUTH_REVOKED, "the login behind that ticket isn't signed in any more"));
    };
    if username != claim.username {
        return Err(Refused::new(401, wire::reason::AUTH_REVOKED, "the login behind that ticket changed"));
    }
    let facts = match grants::user_facts(&username) {
        Ok(Some(f)) if !f.disabled => f,
        Ok(_) => return Err(Refused::new(401, wire::reason::AUTH_REVOKED, "that login is gone or disabled")),
        Err(e) => return Err(Refused::new(500, "store_error", e)),
    };
    let grant = match role {
        Role::Member => grants::live_grant(&claim.desktop, &username).map_err(|e| Refused::new(500, "store_error", e))?,
        _ => None,
    };
    let same_machine = matches!(ingress, "loopback" | "cell");
    let access = policy::access(&Principal::Login { username: username.clone(), role }, grant.as_ref(), same_machine);
    if access.same_machine {
        return Err(Refused::new(403, wire::reason::SAME_MACHINE, "You're on this computer: there's no use watching its own screen"));
    }
    if !access.may_view {
        return Err(Refused::new(403, "not_allowed", "you don't have access to this computer's screen (the owner grants it: k2 desktop grant)"));
    }
    match store::is_enabled(k2_core::desktop::virtual_desktop::switch_id(&claim.desktop)) {
        Ok(true) => {}
        Ok(false) => return Err(Refused::new(409, wire::reason::DESKTOP_OFF, "Remote desktop is off on this computer")),
        Err(e) => return Err(Refused::new(500, "store_error", e)),
    }
    let backend = super::deskd::for_desktop(&claim.desktop);
    let state = backend.state();
    let unavailable = |state: &MachineState, hint: String| Refused { status: 409, code: "desktop_unavailable", hint, state: Some(state.as_wire()) };
    let Some(deskd) = backend.stream() else {
        return Err(unavailable(&state, state.message()));
    };
    if state != MachineState::Ready {
        return Err(unavailable(&state, state.message()));
    }
    let displays = deskd.displays().map_err(|e| unavailable(&MachineState::NoDisplay, format!("K2 Screen couldn't list the displays: {e}")))?;
    if displays.is_empty() {
        return Err(unavailable(&MachineState::NoDisplay, MachineState::NoDisplay.message()));
    }
    let asked = params.get("display").or(claim.display.as_ref()).and_then(|d| d.trim().parse::<u32>().ok());
    Ok(Admitted {
        desktop: claim.desktop.clone(),
        username,
        user_id: facts.id,
        role,
        session: claim.session_token.clone(),
        may_control: access.may_control,
        display: asked.filter(|a| displays.iter().any(|d| d.id == *a)),
        deskd,
        displays,
    })
}

// ── one viewer ───────────────────────────────────────────────────────

/// S8 and the tick, overridable by tests.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub tick: Duration,
    pub idle: Duration,
    pub max: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { tick: AUTH_TICK, idle: VIEWER_IDLE, max: VIEWER_MAX }
    }
}

/// What the dispatcher knows of the request.
pub struct StreamReq {
    pub params: HashMap<String, String>,
    /// The dispatcher's ingress label.
    pub ingress: String,
    pub path: ViewerPath,
    pub ip: String,
    pub owner_token: String,
}

struct Clock {
    started: Instant,
    last_input: Instant,
    /// The tab went hidden (`fps 0`) at; `None` while shown.
    hidden_since: Option<Instant>,
    control: (bool, Instant),
    last_refusal_log: Option<Instant>,
}

/// One viewer, shared by its hooks.
struct Ctx {
    desktop: String,
    viewer: String,
    username: String,
    user_id: String,
    session: String,
    deskd: Arc<dyn StreamDeskd>,
    ingress: String,
    clock: Mutex<Clock>,
}

impl Ctx {
    fn clock(&self) -> std::sync::MutexGuard<'_, Clock> {
        self.clock.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// May this login drive now (Owner or Admin, still signed in)? Re-read
    /// at most every [`CONTROL_CACHE`].
    fn may_control_now(&self) -> bool {
        let now = Instant::now();
        {
            let c = self.clock();
            if now.saturating_duration_since(c.control.1) < CONTROL_CACHE {
                return c.control.0;
            }
        }
        let ok = matches!(connect_users::validate_session_role(&self.session), Some((u, Role::Owner | Role::Admin)) if u == self.username);
        self.clock().control = (ok, now);
        ok
    }

    /// The 5 s re-check of the login, role, grant and switch.
    fn still_may_view(&self) -> Result<(), (u16, &'static str)> {
        let Some((user, role)) = connect_users::validate_session_role(&self.session) else {
            return Err((wire::CLOSE_AUTH_REVOKED, wire::reason::AUTH_REVOKED));
        };
        if user != self.username {
            return Err((wire::CLOSE_AUTH_REVOKED, wire::reason::AUTH_REVOKED));
        }
        match role {
            Role::Owner | Role::Admin => {}
            Role::Member => {
                if !grants::still_granted(&self.desktop, &self.user_id) {
                    return Err((wire::CLOSE_AUTH_REVOKED, wire::reason::GRANT_GONE));
                }
            }
            Role::VoiceChat => return Err((wire::CLOSE_AUTH_REVOKED, wire::reason::AUTH_REVOKED)),
        }
        if !store::is_enabled(k2_core::desktop::virtual_desktop::switch_id(&self.desktop)).unwrap_or(false) {
            return Err((wire::CLOSE_DESKTOP_OFF, wire::reason::DESKTOP_OFF));
        }
        Ok(())
    }

    /// A refused input: logged (the first per [`REFUSAL_LOG_GAP`]).
    fn refused(&self, r: Refusal) -> Refusal {
        let now = Instant::now();
        let log = {
            let mut c = self.clock();
            let due = c.last_refusal_log.is_none_or(|t| now.saturating_duration_since(t) >= REFUSAL_LOG_GAP);
            if due {
                c.last_refusal_log = Some(now);
            }
            due
        };
        if log {
            k2_core::log_debug!("[desktop] input from {} refused: {}", self.viewer, r.code);
            let line = AuditLine::new(Event::Refused, &self.desktop, &format!("login:{}", self.username)).viewer(&self.viewer).ingress(&self.ingress).outcome(&r.code);
            if let Err(e) = audit::write(&line) {
                k2_core::log_debug!("[desktop] audit write failed (refused): {e}");
            }
        }
        r
    }
}

fn watch_only() -> Refusal {
    Refusal::new("desktop_watch_only", "You can watch this computer's screen but not use it.")
}

/// The viewer's messages on the engine: `ack`, `fps`, `keyframe` are the
/// engine's (`fps 0`, a hidden tab, takes the table's floor).
impl StreamMsg for ViewerMsg {
    fn engine(&self) -> Option<EngineMsg> {
        match self {
            ViewerMsg::Ack { seq } => Some(EngineMsg::Ack { seq: *seq }),
            ViewerMsg::Fps { fps } => Some(EngineMsg::Fps { fps: *fps }),
            ViewerMsg::Keyframe => Some(EngineMsg::Keyframe),
            _ => None,
        }
    }
}

impl PointerWheel for ViewerMsg {
    fn is_pointer(&self) -> bool {
        matches!(self, ViewerMsg::Pointer { .. })
    }

    fn wheel(&self) -> Option<Wheel> {
        match self {
            ViewerMsg::Wheel { dx, dy, mode } => Some(Wheel { dx: *dx as f64, dy: *dy as f64, lines: *mode == WheelMode::Line }),
            _ => None,
        }
    }

    fn set_wheel(&mut self, w: Wheel) {
        if let ViewerMsg::Wheel { dx, dy, .. } = self {
            *dx = w.dx as f32;
            *dy = w.dy as f32;
        }
    }
}

fn deskd_button(b: Button) -> DeskdButton {
    match b {
        Button::Left => DeskdButton::Left,
        Button::Right => DeskdButton::Right,
        Button::Middle => DeskdButton::Middle,
        Button::Back => DeskdButton::Back,
        Button::Forward => DeskdButton::Forward,
    }
}

/// A viewer input → deskd's (pointer on the current display).
pub fn to_event(m: &ViewerMsg, display: u32) -> Option<InputEvent> {
    Some(match m {
        ViewerMsg::Pointer { x, y } => InputEvent::Pointer { display, x: *x as f64, y: *y as f64 },
        ViewerMsg::Button { button, down } => InputEvent::Button { button: deskd_button(*button), down: *down },
        ViewerMsg::Wheel { dx, dy, mode } => InputEvent::Wheel {
            dx: *dx as f64,
            dy: *dy as f64,
            mode: match mode {
                WheelMode::Pixel => DeskdWheel::Pixel,
                WheelMode::Line => DeskdWheel::Line,
            },
        },
        ViewerMsg::Key { code, down } => InputEvent::Key { code: code.clone(), down: *down },
        ViewerMsg::Text { text } => InputEvent::Text { text: text.clone() },
        _ => return None,
    })
}

/// Per message, on the socket task (R17): control and lease checks, and
/// the lease's own messages (taken here, so input right after a take
/// isn't refused by a queue that hasn't caught up).
fn may_input(ctx: &Arc<Ctx>, m: &ViewerMsg) -> Result<(), Refusal> {
    let take = matches!(m, ViewerMsg::TakeControl | ViewerMsg::View { on: false });
    let give = matches!(m, ViewerMsg::HandBack | ViewerMsg::View { on: true });
    if take {
        if !ctx.may_control_now() {
            return Err(ctx.refused(watch_only()));
        }
        let r = with_session(&ctx.desktop, |s| s.lease.take(&ctx.viewer, &ctx.username, Instant::now()));
        return match r {
            Ok(Some(change)) => {
                lease_changed(&ctx.desktop, &ctx.deskd, change, ReleaseReason::Other);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(TakeRefused::HeldBy(who)) => Err(Refusal::new("desktop_controlled", format!("{who} is controlling this screen; ask them to hand back control."))),
        };
    }
    if give {
        let why = if matches!(m, ViewerMsg::View { .. }) { ReleaseReason::ViewOnly } else { ReleaseReason::HandBack };
        if let Some(change) = with_session(&ctx.desktop, |s| s.lease.hand_back(&ctx.viewer)) {
            lease_changed(&ctx.desktop, &ctx.deskd, change, why);
        }
        return Ok(());
    }
    let drives = m.is_input() || matches!(m, ViewerMsg::Display { .. });
    if !drives {
        return Ok(());
    }
    if !ctx.may_control_now() {
        return Err(ctx.refused(watch_only()));
    }
    let now = Instant::now();
    let (holds, by, held) = with_session(&ctx.desktop, |s| (s.lease.holds(&ctx.viewer), s.lease.controller().map(str::to_string), s.lease.local_hold(now)));
    if !holds {
        return Err(ctx.refused(match by {
            Some(who) => Refusal::new("desktop_controlled", format!("{who} is controlling this screen; ask them to hand back control.")),
            None => Refusal::new("desktop_not_controller", "Take control first."),
        }));
    }
    if held && m.is_input() {
        return Err(Refusal::new("desktop_local_hold", "Someone at this computer is using it; your input waits until they stop for 3 seconds."));
    }
    Ok(())
}

/// The viewer's input into deskd, in order, on its input thread.
struct DesktopSink {
    ctx: Arc<Ctx>,
    push: mpsc::UnboundedSender<Push>,
}

/// One access-log line for a clipboard read or write: who, which viewer,
/// the outcome. The line has no field for the text, and none is passed.
fn audit_clipboard(ctx: &Ctx, event: Event, outcome: &str) {
    let line = AuditLine::new(event, &ctx.desktop, &format!("login:{}", ctx.username)).viewer(&ctx.viewer).outcome(outcome);
    if let Err(e) = audit::write(&line) {
        k2_core::log_debug!("[desktop] audit write failed ({}): {e}", event.as_str());
    }
}

impl DesktopSink {
    fn deskd_err(e: String) -> Refusal {
        Refusal::new("desktop_input_failed", format!("K2 Screen couldn't do that: {e}"))
    }
}

impl InputSink<ViewerMsg> for DesktopSink {
    fn send(&mut self, m: ViewerMsg) -> Result<(), Refusal> {
        let ctx = &self.ctx;
        let now = Instant::now();
        match &m {
            ViewerMsg::TakeControl | ViewerMsg::HandBack | ViewerMsg::View { .. } => return Ok(()),
            ViewerMsg::Ack { .. } | ViewerMsg::Fps { .. } | ViewerMsg::Keyframe | ViewerMsg::Present => return Ok(()),
            _ => {}
        }
        // Again at the moment it goes: a hand-back or a lost lease since it
        // was queued drops it.
        let (holds, held, display) = with_session(&ctx.desktop, |s| {
            let holds = s.lease.holds(&ctx.viewer);
            let held = s.lease.local_hold(now);
            if holds && !held && m.is_input() {
                s.lease.admit_input(&ctx.viewer, now);
            }
            (holds, held, s.display.unwrap_or(0))
        });
        if !holds {
            return Err(Refusal::new("desktop_not_controller", "You don't control this screen any more."));
        }
        ctx.clock().last_input = now;
        match m {
            ViewerMsg::Display { id } => {
                let Some(want) = id.trim().parse::<u32>().ok().filter(|d| with_session(&ctx.desktop, |s| s.displays.iter().any(|x| x.id == *d))) else {
                    return Err(Refusal::new("desktop_no_such_display", format!("There's no display {id} on this computer.")));
                };
                ctx.deskd.release_all(ReleaseReason::DisplaySwitch);
                ctx.deskd.select_display(want).map_err(Self::deskd_err)?;
                let t = with_session(&ctx.desktop, |s| {
                    s.display = Some(want);
                    displays_text(s)
                });
                broadcast_text(&ctx.desktop, t);
                Ok(())
            }
            ViewerMsg::ClipboardGet => {
                let got = ctx.deskd.clipboard_get();
                let outcome = match &got {
                    Ok((_, Some(reason))) => reason.as_str(),
                    Ok(_) => "ok",
                    Err(_) => "failed",
                };
                audit_clipboard(ctx, Event::ClipboardRead, outcome);
                let (text_v, reason) = got.map_err(Self::deskd_err)?;
                let _ = self.push.send(Push::Text(text(&ServerMsg::Clipboard { text: text_v, reason })));
                Ok(())
            }
            ViewerMsg::ClipboardSet { text: t } => {
                let r = ctx.deskd.clipboard_set(&t);
                audit_clipboard(ctx, Event::ClipboardWrite, if r.is_ok() { "ok" } else { "failed" });
                r.map_err(Self::deskd_err)
            }
            other => {
                if held {
                    return Err(Refusal::new("desktop_local_hold", "Someone at this computer is using it; that input was dropped."));
                }
                match to_event(&other, display) {
                    Some(ev) => ctx.deskd.inject(ev).map_err(Self::deskd_err),
                    None => Ok(()),
                }
            }
        }
    }
}

/// Closes one viewer through its push channel (`disable`, `revoke`, `kick`).
struct PushClose(mpsc::UnboundedSender<Push>);

impl ViewerHandle for PushClose {
    fn close(&self, code: u16, reason: &str) {
        let _ = self.0.send(Push::Close { code, reason: reason.to_string() });
    }
}

fn new_viewer_id() -> String {
    format!("dv_{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

async fn refuse_http(stream: &mut TcpStream, r: &Refused) {
    let mut buf = vec![0u8; 16 * 1024];
    let _ = tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buf)).await;
    crate::routes::http::send_response(stream, r.status_line(), "application/json", &r.body()).await;
}

fn wire_codec(c: Codec) -> wire::Codec {
    match c {
        Codec::Jpeg => wire::Codec::Jpeg,
        Codec::H264 => wire::Codec::H264,
        Codec::Vp8 => wire::Codec::Vp8,
    }
}

fn wire_path(p: ViewerPath) -> wire::ViewerPath {
    match p {
        ViewerPath::Local => wire::ViewerPath::Local,
        ViewerPath::Lan => wire::ViewerPath::Lan,
        ViewerPath::Tunnel => wire::ViewerPath::Tunnel,
    }
}

/// Serve one viewer. The dispatcher sends every `/cli/desktop/stream`
/// request here; the gate is [`admit`].
pub async fn serve(stream: &mut TcpStream, req: StreamReq) {
    serve_with(stream, req, Limits::default()).await
}

pub(crate) async fn serve_with(stream: &mut TcpStream, req: StreamReq, limits: Limits) {
    let StreamReq { params, ingress, path, ip, owner_token } = req;
    let admitted = {
        let (p, i) = (params.clone(), ingress.clone());
        match tokio::task::spawn_blocking(move || admit(&p, &i)).await {
            Ok(r) => r,
            Err(e) => Err(Refused::new(500, "worker_failed", e.to_string())),
        }
    };
    let a = match admitted {
        Ok(a) => a,
        Err(r) => {
            k2_core::log_debug!("[desktop] stream refused ({}): {}", ingress, r.code);
            refuse_http(stream, &r).await;
            return;
        }
    };
    // A Mac's K2 Screen sends H.264 only: a viewer that can't decode it
    // would get a JPEG layer nothing makes. Refuse it with why.
    let offer = Offer::from_params(&params);
    let gives = a.deskd.codecs();
    let neg = crate::stream::viewer::negotiate(SourceKind::Desktop, &offer, &gives, path);
    if neg.codec == Codec::Jpeg && !a.deskd.jpeg() {
        let have = gives.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ");
        let r = Refused::new(409, "desktop_codec", format!("this viewer can't decode what K2 Screen sends here ({have}); use the K2 desktop app or a browser that decodes H.264"));
        refuse_http(stream, &r).await;
        return;
    }
    let viewer = new_viewer_id();
    let want_view = params.get("view").map(String::as_str) == Some("1");
    let start_mode = if a.may_control && !want_view { Mode::Control } else { Mode::View };
    // The access-log line before the viewer connects (R33).
    let connect = AuditLine::new(Event::Connect, &a.desktop, &format!("login:{}", a.username))
        .role(a.role.as_wire())
        .ingress(&ingress)
        .ip(&ip)
        .viewer(&viewer)
        .mode(start_mode.as_str());
    let logged = tokio::task::spawn_blocking(move || audit::write(&connect)).await.unwrap_or_else(|e| Err(e.to_string()));
    if let Err(e) = logged {
        k2_core::log_debug!("[desktop] audit write failed, viewer refused: {e}");
        refuse_http(stream, &Refused::new(503, wire::reason::AUDIT_UNAVAILABLE, "K2 couldn't write the remote desktop access log (~/.k2/desktop-audit.jsonl), so the screen stays closed")).await;
        return;
    }
    // One capture per desktop: a joiner gets the display the hub shows.
    let running = crate::stream::hub_for(&SourceKey::Desktop { desktop: a.desktop.clone() }).is_some();
    let (display, switch_to) = with_session(&a.desktop, |s| {
        s.displays = a.displays.clone();
        let cur = s.display.filter(|d| a.displays.iter().any(|x| x.id == *d)).unwrap_or_else(|| first_display(&a.displays));
        let pick = if running { cur } else { a.display.unwrap_or(cur) };
        let switch = (!running && pick != cur).then_some(pick);
        s.display = Some(pick);
        (pick, switch)
    });
    if let Some(d) = switch_to {
        let deskd = a.deskd.clone();
        if let Ok(Err(e)) = tokio::task::spawn_blocking(move || deskd.select_display(d)).await {
            k2_core::log_debug!("[desktop] starting on display {d}: {e}");
        }
    }
    let (push_tx, push_rx) = mpsc::unbounded_channel::<Push>();
    let info = ViewerInfo {
        id: viewer.clone(),
        desktop: a.desktop.clone(),
        user: a.username.clone(),
        user_id: a.user_id.clone(),
        role: a.role.as_wire().to_string(),
        mode: Mode::View,
        ingress: ingress.clone(),
        ip: ip.clone(),
        device: String::new(),
        display: display.to_string(),
        since: 0,
    };
    viewers::register(info, Arc::new(PushClose(push_tx.clone())));
    // A virtual desktop counts its viewers' presence (LD-24): joining is
    // present; an Owner's or Admin's presence wakes it.
    if k2_core::desktop::virtual_desktop::is_virtual(&a.desktop) {
        super::virtual_desktop::manager().viewer_joined(&a.desktop, &viewer, &a.username, matches!(a.role, Role::Owner | Role::Admin));
    }
    let now = Instant::now();
    let took = with_session(&a.desktop, |s| {
        s.members.insert(viewer.clone(), Member { name: a.username.clone(), owner: a.role == Role::Owner, push: push_tx.clone() });
        if start_mode == Mode::Control {
            s.lease.take(&viewer, &a.username, now).ok().flatten()
        } else {
            None
        }
    });
    let mode = if took.is_some() { Mode::Control } else { Mode::View };
    if let Some(change) = took {
        lease_changed(&a.desktop, &a.deskd, change, ReleaseReason::Other);
    } else {
        broadcast_viewers(&a.desktop);
        refresh_banner(&a.desktop, &a.deskd);
    }
    // After hello: who drives, and the last cursor shape.
    with_session(&a.desktop, |s| {
        let _ = push_tx.send(Push::Text(control_text(s, &viewer, now)));
        if let Some(c) = &s.cursor {
            let _ = push_tx.send(Push::Text(c.clone()));
        }
    });
    ensure_pump(&a.desktop, &a.deskd);
    {
        let (u, i, d) = (a.username.clone(), ingress.clone(), a.desktop.clone());
        let _ = tokio::task::spawn_blocking(move || {
            super::routes::notify_new_viewer(&u, &i);
            super::routes::emit_state_for(&d);
        })
        .await;
    }

    let ctx = Arc::new(Ctx {
        desktop: a.desktop.clone(),
        viewer: viewer.clone(),
        username: a.username.clone(),
        user_id: a.user_id.clone(),
        session: a.session.clone(),
        deskd: a.deskd.clone(),
        ingress: ingress.clone(),
        clock: Mutex::new(Clock { started: now, last_input: now, hidden_since: None, control: (a.may_control, now), last_refusal_log: None }),
    });
    let hooks = hooks(&ctx, &a, display, mode, push_tx.clone(), push_rx, &owner_token, limits);
    let source = {
        let (deskd, list) = (a.deskd.clone(), a.displays.clone());
        Source::<String> {
            key: SourceKey::Desktop { desktop: a.desktop.clone() },
            kind: SourceKind::Desktop,
            open: Box::new(move || deskd.source(display, &list)),
            gives,
        }
    };
    let served = crate::stream::serve(stream, source, offer, path, hooks).await;
    leave(&ctx);
    match served {
        Ok(()) => {}
        Err(ServeError::Join(JoinError::Open(e))) | Err(ServeError::Join(JoinError::Thread(e))) | Err(ServeError::Task(e)) => {
            let r = Refused { status: 409, code: "desktop_unavailable", hint: format!("K2 Screen isn't streaming: {e}"), state: None };
            crate::routes::http::send_response(stream, r.status_line(), "application/json", &r.body()).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn hooks(
    ctx: &Arc<Ctx>,
    a: &Admitted,
    display: u32,
    mode: Mode,
    push_tx: mpsc::UnboundedSender<Push>,
    push_rx: mpsc::UnboundedReceiver<Push>,
    owner_token: &str,
    limits: Limits,
) -> Hooks<ViewerMsg> {
    let mut incoming = crate::socket_access::IncomingCheck::new(&a.session, owner_token);
    let mut budget = InputBudget::new(Instant::now());
    let (c_tick, c_frame, c_input) = (ctx.clone(), ctx.clone(), ctx.clone());
    let hello_fields = {
        let (desktop, list, os, encoder, local_wins, controller) = (
            a.desktop.clone(),
            a.displays.iter().map(wire_display).collect::<Vec<_>>(),
            a.deskd.os(),
            a.deskd.encoder(),
            a.deskd.local_wins(),
            with_session(&a.desktop, |s| s.lease.controller().map(str::to_string)),
        );
        move |n: &Negotiated| -> Map<String, Value> {
            let m = ServerMsg::Hello {
                desktop,
                display: display.to_string(),
                displays: list,
                codec: wire_codec(n.codec),
                window: n.window,
                fps: n.fps,
                path: wire_path(n.path),
                lane: Lane::Main,
                encoder,
                mode,
                controller,
                os,
                local_wins,
                same_machine: false,
            };
            match serde_json::to_value(&m) {
                Ok(Value::Object(o)) => o,
                _ => Map::new(),
            }
        }
    };
    Hooks {
        auth: Auth {
            every: limits.tick,
            recheck: Box::new(move || recheck(&c_tick, limits)),
            // Security review L7: a client frame from a login is acted on
            // only while the login still clears this socket's floor.
            frame: Box::new(move |f: &WsFrame| {
                if incoming.frame_ok(f) {
                    note_frame(&c_frame, f);
                    Verdict::Keep
                } else {
                    Verdict::Close { code: crate::presence::WS_CLOSE_ACCESS_LOST, reason: "access_lost".into() }
                }
            }),
            may_input: Box::new(move |m: &ViewerMsg| may_input(&c_input, m)),
        },
        limits: Box::new(move |m: &ViewerMsg, now: Instant| {
            check_input(m).and_then(|_| budget.take(m, now)).map_err(|r| Refusal::new(r.code, r.message))
        }),
        sink: Box::new(DesktopSink { ctx: ctx.clone(), push: push_tx }),
        coalesce: Box::new(DesktopRule),
        hello: Box::new(hello_fields),
        push: Some(push_rx),
        busy: Refusal::new("desktop_busy", "Too much input at once; slow down."),
        ended: Box::new(|why: &str| Some(text(&ServerMsg::State { state: MachineState::Unavailable(String::new()).as_wire().into(), message: picture_failed_text(why) }))),
        ended_reason: wire::reason::DESKD_ENDED.into(),
        tag: "desktop",
    }
}

/// The tab's shown / hidden state, from its `fps` (0 = hidden), and its
/// `present` heartbeat (a virtual desktop's presence, LD-24).
fn note_frame(ctx: &Ctx, f: &WsFrame) {
    let Some(Ok(Message::Text(t))) = f else { return };
    if !t.contains("\"fps\"") && !t.contains("\"present\"") {
        return;
    }
    let virtual_desktop = k2_core::desktop::virtual_desktop::is_virtual(&ctx.desktop);
    match serde_json::from_str::<ViewerMsg>(t) {
        Ok(ViewerMsg::Fps { fps }) => {
            {
                let mut c = ctx.clock();
                if fps == 0 {
                    if c.hidden_since.is_none() {
                        c.hidden_since = Some(Instant::now());
                    }
                } else {
                    c.hidden_since = None;
                }
            }
            if virtual_desktop {
                super::virtual_desktop::manager().viewer_hidden(&ctx.desktop, &ctx.viewer, fps == 0);
            }
        }
        Ok(ViewerMsg::Present) if virtual_desktop => {
            super::virtual_desktop::manager().viewer_present(&ctx.desktop, &ctx.viewer);
        }
        _ => {}
    }
}

/// Tell every viewer of a desktop its new state (a virtual desktop going
/// to sleep or closing), before their sockets close.
pub fn tell_state(desktop: &str, state: &str, message: &str) {
    let t = text(&ServerMsg::State { state: state.into(), message: message.into() });
    broadcast_text(desktop, t);
}

fn recheck(ctx: &Arc<Ctx>, limits: Limits) -> Verdict {
    if let Err((code, reason)) = ctx.still_may_view() {
        return Verdict::Close { code, reason: reason.into() };
    }
    let now = Instant::now();
    {
        let c = ctx.clock();
        if now.saturating_duration_since(c.started) >= limits.max {
            return Verdict::Close { code: wire::CLOSE_FORBIDDEN, reason: wire::reason::SESSION_LIMIT.into() };
        }
        if let Some(h) = c.hidden_since {
            if now.saturating_duration_since(c.last_input.max(h)) >= limits.idle {
                return Verdict::Close { code: wire::CLOSE_FORBIDDEN, reason: wire::reason::IDLE.into() };
            }
        }
    }
    // The controller's 10 min idle rule, and a controller who lost the
    // right to drive (Owner → Member) is handed back.
    let holds = with_session(&ctx.desktop, |s| s.lease.holds(&ctx.viewer));
    let lost = holds && !ctx.may_control_now();
    let change = with_session(&ctx.desktop, |s| {
        if lost {
            s.lease.hand_back(&ctx.viewer).map(|c| (c, ReleaseReason::LeaseLost))
        } else {
            s.lease.tick(now).map(|c| (c, ReleaseReason::LeaseLost))
        }
    });
    if let Some((c, why)) = change {
        lease_changed(&ctx.desktop, &ctx.deskd, c, why);
    }
    Verdict::Keep
}

/// The socket ended: out of the registry and the session, the lease freed
/// (held input released), the access-log line, every viewer told.
fn leave(ctx: &Arc<Ctx>) {
    viewers::unregister(&ctx.viewer);
    if k2_core::desktop::virtual_desktop::is_virtual(&ctx.desktop) {
        super::virtual_desktop::manager().viewer_left(&ctx.desktop, &ctx.viewer);
    }
    let change = with_session(&ctx.desktop, |s| {
        s.members.remove(&ctx.viewer);
        s.lease.drop_viewer(&ctx.viewer)
    });
    if let Some(c) = change {
        lease_changed(&ctx.desktop, &ctx.deskd, c, ReleaseReason::ViewerClosed);
    } else {
        broadcast_viewers(&ctx.desktop);
        refresh_banner(&ctx.desktop, &ctx.deskd);
    }
    let ms = ctx.clock().started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let line = AuditLine::new(Event::Disconnect, &ctx.desktop, &format!("login:{}", ctx.username)).viewer(&ctx.viewer).ingress(&ctx.ingress).duration_ms(ms);
    if let Err(e) = audit::write(&line) {
        k2_core::log_debug!("[desktop] audit write failed (disconnect): {e}");
    }
    super::routes::emit_state_for(&ctx.desktop);
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
