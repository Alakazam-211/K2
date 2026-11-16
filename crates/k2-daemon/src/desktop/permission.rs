//! Turning Remote desktop on at a Mac in one step (Rosson 2026-11-15: "When
//! they click the toggle … we should make all of that happen
//! automatically").
//!
//! K2 Screen (`k2-deskd`) holds Screen Recording and Accessibility; the
//! daemon holds neither (§3.2). Switching Remote desktop on used to start K2
//! Screen and stop there: nobody asked macOS, so K2 Screen wasn't in either
//! System Settings list, and after someone allowed it by hand the running
//! K2 Screen still saw no permission (macOS applies a new Screen Recording
//! grant to a fresh process only). This module is the daemon's half of the
//! fix:
//!
//! 1. **Ask.** When the switch goes on (or the Owner presses Ask at the
//!    Mac), K2 Screen calls `CGRequestScreenCaptureAccess()` and the
//!    Accessibility prompt. macOS adds K2 Screen to both lists and shows its
//!    own prompts. macOS shows each prompt once per app, so when K2 asked
//!    before ([`Asked`], kept in `~/.k2/desktop-permission.json`), K2 also
//!    opens System Settings at the first pane still missing, and the row
//!    says "Allow K2 Screen in the window that just opened."
//! 2. **Notice.** While something is missing, the daemon asks K2 Screen
//!    every [`POLL`] for its two flags (`tcc_status`: the preflight calls,
//!    plus K2 Screen's tiny `--tcc-probe` copy while Screen Recording reads
//!    "no", because a process's own preflight can keep its first answer).
//!    For [`WATCH_FOR`] at most; then it waits for the next toggle, Ask at
//!    the Mac, or Check again.
//! 3. **Restart.** When a grant flips to allowed, K2 Screen is restarted
//!    once (`launchctl kickstart -k`) so the grant applies, and every open
//!    view hears `desktop_state`. Screen Recording allowed and Accessibility
//!    still missing: System Settings opens at Accessibility next.
//!
//! [`Flow`] is the state machine, pure and on a clock the caller passes.
//! [`Watcher`] drives it over a [`Host`] (the real one talks to K2 Screen
//! and runs `/usr/bin/open`; tests use a fake). The macOS prompts can't run
//! in CI.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_deskd_proto::msg::Tcc;
use serde::{Deserialize, Serialize};

/// How often the daemon asks K2 Screen for its flags while one is missing.
pub const POLL: Duration = Duration::from_secs(2);

/// How long it keeps asking after the toggle, Ask at the Mac or Check again.
pub const WATCH_FOR: Duration = Duration::from_secs(10 * 60);

/// The row's line when K2 opened System Settings for the person.
pub const LINE_OPENED: &str = "Allow K2 Screen in the window that just opened.";
/// The row's line when macOS is showing its own prompts.
pub const LINE_PROMPTED: &str = "macOS is asking on this Mac's screen: allow K2 Screen there.";
/// The row's line before K2 Screen has answered.
pub const LINE_ASKING: &str = "Starting K2 Screen to ask macOS…";
/// The row's line once K2 stopped checking by itself.
pub const LINE_STOPPED: &str = "Still waiting. Allow K2 Screen in System Settings, then press Check again.";
/// The row's line while K2 checks without having asked (boot, Check again).
pub const LINE_WAITING: &str = "Allow K2 Screen in System Settings on this Mac.";
pub const LINE_READY: &str = "K2 Screen is allowed.";

/// A System Settings pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pane {
    Screen,
    Accessibility,
}

impl Pane {
    /// The pane's URL (`/usr/bin/open` takes it).
    pub fn url(self) -> &'static str {
        match self {
            Pane::Screen => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            Pane::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        }
    }

    pub fn parse(s: &str) -> Option<Pane> {
        match s {
            "screen" | "screen_recording" => Some(Pane::Screen),
            "accessibility" => Some(Pane::Accessibility),
            _ => None,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Pane::Screen => "screen",
            Pane::Accessibility => "accessibility",
        }
    }
}

/// Which prompts K2 has fired at some point (macOS shows each once).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Asked {
    #[serde(default)]
    pub screen: bool,
    #[serde(default)]
    pub accessibility: bool,
}

impl Asked {
    fn has(self, p: Pane) -> bool {
        match p {
            Pane::Screen => self.screen,
            Pane::Accessibility => self.accessibility,
        }
    }
}

/// Where the flow is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// The switch is off.
    Off,
    /// K2 Screen is starting, or hasn't fired the prompts yet.
    Asking,
    WaitingScreen,
    WaitingAccessibility,
    Ready,
}

/// What the [`Watcher`] does for the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    OpenPane(Pane),
    /// Restart K2 Screen so a new grant applies to a fresh process.
    RestartHelper,
    /// Remember what was asked (persist [`Asked`]).
    SaveAsked(Asked),
}

/// The first grant still missing.
fn missing(t: &Tcc) -> Option<Pane> {
    if t.granted() {
        None
    } else if !t.screen {
        Some(Pane::Screen)
    } else {
        Some(Pane::Accessibility)
    }
}

/// The state machine. Every method takes `now`; nothing here sleeps, spawns
/// or calls macOS.
#[derive(Debug, Clone)]
pub struct Flow {
    /// The switch is on.
    active: bool,
    /// The prompts are still to fire (K2 Screen hadn't answered yet).
    need_request: bool,
    /// The prompts fired in this round of asking.
    requested: bool,
    tcc: Option<Tcc>,
    asked: Asked,
    opened: Option<Pane>,
    /// Checking by itself until then.
    until: Option<Instant>,
    /// K2 Screen was restarted for this grant (once per flip).
    restarted: Asked,
}

impl Flow {
    pub fn new(asked: Asked) -> Self {
        Flow { active: false, need_request: false, requested: false, tcc: None, asked, opened: None, until: None, restarted: Asked::default() }
    }

    /// The switch went on (or was already on: the toggle asks again).
    pub fn turned_on(&mut self, now: Instant) {
        self.active = true;
        self.ask(now);
    }

    /// The switch went off: nothing more is asked or checked.
    pub fn turned_off(&mut self) {
        *self = Flow::new(self.asked);
    }

    /// Ask at the Mac: fire the prompts again and check for [`WATCH_FOR`].
    pub fn ask(&mut self, now: Instant) {
        if !self.active {
            return;
        }
        self.need_request = true;
        self.requested = false;
        self.opened = None;
        self.restarted = Asked::default();
        self.until = Some(now + WATCH_FOR);
    }

    /// Check again: no prompts, no windows; check for [`WATCH_FOR`].
    pub fn check_again(&mut self, now: Instant) {
        if self.active {
            self.until = Some(now + WATCH_FOR);
        }
    }

    /// The Owner pressed a step's button: open that pane and check.
    pub fn open(&mut self, now: Instant, pane: Pane) -> Vec<Action> {
        if !self.active {
            return Vec::new();
        }
        self.opened = Some(pane);
        self.until = Some(now + WATCH_FOR);
        vec![Action::OpenPane(pane)]
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn needs_request(&self) -> bool {
        self.active && self.need_request
    }

    /// Checking by itself right now.
    pub fn checking(&self, now: Instant) -> bool {
        self.active && self.until.is_some_and(|u| now < u) && !self.tcc.is_some_and(|t| t.granted())
    }

    /// K2 Screen fired the prompts; `tcc` is right after (usually still
    /// missing: nobody has clicked yet).
    pub fn requested(&mut self, now: Instant, tcc: Tcc) -> Vec<Action> {
        self.need_request = false;
        self.requested = true;
        let mut out = self.note(now, tcc);
        if tcc.granted() {
            return out;
        }
        // macOS already showed this prompt once and won't again: open the
        // pane instead.
        if let Some(p) = missing(&tcc).filter(|p| self.asked.has(*p) && self.opened != Some(*p)) {
            self.opened = Some(p);
            out.push(Action::OpenPane(p));
        }
        let asked = Asked { screen: self.asked.screen || !tcc.screen, accessibility: self.asked.accessibility || !tcc.accessibility };
        if asked != self.asked {
            self.asked = asked;
            out.push(Action::SaveAsked(asked));
        }
        if self.until.is_none_or(|u| u <= now) {
            self.until = Some(now + WATCH_FOR);
        }
        out
    }

    /// One check's answer.
    pub fn checked(&mut self, now: Instant, tcc: Tcc) -> Vec<Action> {
        self.note(now, tcc)
    }

    /// What K2 Screen said outside a check: its hello (a restart, the
    /// switch going on) or its own `tcc_changed`.
    pub fn saw(&mut self, now: Instant, tcc: Tcc) -> Vec<Action> {
        self.note(now, tcc)
    }

    /// K2 Screen's flags, from wherever. A grant going from missing to
    /// allowed restarts K2 Screen once (macOS applies a new Screen
    /// Recording grant to a fresh process only); Screen Recording coming
    /// through while Accessibility is still missing opens Accessibility
    /// next.
    fn note(&mut self, now: Instant, tcc: Tcc) -> Vec<Action> {
        let mut out = Vec::new();
        let before = self.tcc;
        self.tcc = Some(tcc);
        if !tcc.screen {
            self.restarted.screen = false;
        }
        if !tcc.accessibility {
            self.restarted.accessibility = false;
        }
        if !self.active {
            return out;
        }
        // No earlier answer (a first hello): that process started with
        // what it has, so there's nothing to apply.
        let Some(before) = before else {
            if tcc.granted() {
                self.until = None;
                self.opened = None;
            }
            return out;
        };
        let screen_flip = tcc.screen && !before.screen && !self.restarted.screen;
        let ax_flip = tcc.accessibility && !before.accessibility && !self.restarted.accessibility;
        if screen_flip || ax_flip {
            self.restarted.screen |= tcc.screen;
            self.restarted.accessibility |= tcc.accessibility;
            out.push(Action::RestartHelper);
        }
        if tcc.granted() {
            self.until = None;
            self.opened = None;
            return out;
        }
        let watching = self.until.is_some_and(|u| now < u);
        if screen_flip && !tcc.accessibility && watching && self.opened != Some(Pane::Accessibility) {
            self.opened = Some(Pane::Accessibility);
            self.until = Some(now + WATCH_FOR);
            out.push(Action::OpenPane(Pane::Accessibility));
        }
        out
    }

    pub fn step(&self) -> Step {
        self.step_with(None)
    }

    /// [`Flow::step`], with what K2 Screen last said when no check ran yet.
    fn step_with(&self, fallback: Option<Tcc>) -> Step {
        if !self.active {
            return Step::Off;
        }
        if self.need_request {
            return Step::Asking;
        }
        match self.tcc.or(fallback) {
            None => Step::Asking,
            Some(t) if t.granted() => Step::Ready,
            Some(t) if !t.screen => Step::WaitingScreen,
            Some(_) => Step::WaitingAccessibility,
        }
    }

    pub fn tcc(&self) -> Option<Tcc> {
        self.tcc
    }

    /// The row: each grant, the pane K2 opened, whether K2 is still
    /// checking, and one plain line. `None` with the switch off, and where
    /// K2 Screen needs no permission (Linux).
    pub fn view(&self, now: Instant, fallback: Option<Tcc>) -> Option<Permission> {
        if !self.active {
            return None;
        }
        let tcc = self.tcc.or(fallback);
        if tcc.is_some_and(|t| !t.required) {
            return None;
        }
        let step = self.step_with(fallback);
        let checking = step != Step::Ready && (self.checking(now) || self.needs_request());
        let line = match step {
            Step::Ready => LINE_READY,
            Step::Off | Step::Asking => LINE_ASKING,
            _ if !checking => LINE_STOPPED,
            _ if self.opened.is_some() => LINE_OPENED,
            _ if self.requested => LINE_PROMPTED,
            _ => LINE_WAITING,
        };
        let word = |b: Option<bool>| if b == Some(true) { "allowed" } else { "waiting" };
        Some(Permission {
            step,
            screen: word(tcc.map(|t| t.screen)),
            accessibility: word(tcc.map(|t| t.accessibility)),
            opened: if step == Step::Ready { None } else { self.opened },
            checking,
            line,
        })
    }
}

/// `status.macPermission` (macOS only): the Settings row's steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permission {
    pub step: Step,
    /// `allowed` | `waiting`.
    pub screen: &'static str,
    /// `allowed` | `waiting` (needed to type and click; K2 waits for both).
    pub accessibility: &'static str,
    /// The pane K2 opened for the person, if any.
    pub opened: Option<Pane>,
    /// K2 is checking by itself (else: Check again).
    pub checking: bool,
    pub line: &'static str,
}

/// What the Owner asked for (`POST /cli/desktop/ask`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Fire macOS's prompts again (Ask at the Mac).
    Prompt,
    /// Open System Settings at a pane.
    Open(Pane),
    /// Check again, no windows.
    Check,
}

impl Ask {
    pub fn as_wire(self) -> &'static str {
        match self {
            Ask::Prompt => "prompt",
            Ask::Open(Pane::Screen) => "open_screen",
            Ask::Open(Pane::Accessibility) => "open_accessibility",
            Ask::Check => "check",
        }
    }
}

/// The machine end: K2 Screen and System Settings.
pub trait Host: Send + Sync {
    /// K2 Screen fires the prompts for what's missing (`tcc_request`).
    fn request(&self) -> Result<Tcc, String>;
    /// K2 Screen's two flags now (`tcc_status`).
    fn check(&self) -> Result<Tcc, String>;
    /// Restart K2 Screen (a new grant applies to a fresh process).
    fn restart_helper(&self) -> Result<(), String>;
    /// Open System Settings at `pane` on this Mac.
    fn open_pane(&self, pane: Pane) -> Result<(), String>;
    fn load_asked(&self) -> Asked;
    fn save_asked(&self, a: Asked) -> Result<(), String>;
}

/// Drives a [`Flow`] over a [`Host`]: one round per [`POLL`] on its own
/// thread while the flow wants it (never two threads).
pub struct Watcher {
    host: Arc<dyn Host>,
    flow: Mutex<Flow>,
    on_change: Box<dyn Fn() + Send + Sync>,
    /// A poll thread is running.
    running: AtomicBool,
    /// Tests drive [`Watcher::round`] themselves.
    spawn: bool,
}

impl Watcher {
    /// `on_change` runs after a round that changed the row (the daemon
    /// emits `desktop_state`).
    pub fn new(host: Arc<dyn Host>, on_change: Box<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Self::build(host, on_change, true)
    }

    /// No poll thread: the test calls [`Watcher::round`].
    pub fn manual(host: Arc<dyn Host>, on_change: Box<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Self::build(host, on_change, false)
    }

    fn build(host: Arc<dyn Host>, on_change: Box<dyn Fn() + Send + Sync>, spawn: bool) -> Arc<Self> {
        let asked = host.load_asked();
        Arc::new(Watcher { host, flow: Mutex::new(Flow::new(asked)), on_change, running: AtomicBool::new(false), spawn })
    }

    fn flow(&self) -> std::sync::MutexGuard<'_, Flow> {
        self.flow.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn turned_on(self: &Arc<Self>) {
        self.flow().turned_on(Instant::now());
        self.kick();
    }

    pub fn turned_off(&self) {
        self.flow().turned_off();
    }

    /// The switch is on with a permission flow (boot, or the toggle).
    pub fn active(&self) -> bool {
        self.flow().active()
    }

    /// The boot path: the switch was left on. Nothing is asked at boot;
    /// the row shows what K2 Screen reports, and Check again / Ask at the
    /// Mac work.
    pub fn resume(&self) {
        let mut f = self.flow();
        if !f.active() {
            f.active = true;
        }
    }

    /// The Owner's ask. `false`: the switch is off here.
    pub fn ask(self: &Arc<Self>, what: Ask) -> bool {
        let now = Instant::now();
        let actions = {
            let mut f = self.flow();
            if !f.active() {
                return false;
            }
            match what {
                Ask::Prompt => {
                    f.ask(now);
                    Vec::new()
                }
                Ask::Open(p) => f.open(now, p),
                Ask::Check => {
                    f.check_again(now);
                    Vec::new()
                }
            }
        };
        self.run(actions);
        self.kick();
        true
    }

    /// K2 Screen said something outside a check (hello, its own change
    /// event).
    pub fn saw(&self, tcc: Tcc) {
        let actions = self.flow().saw(Instant::now(), tcc);
        self.run(actions);
    }

    pub fn view(&self, fallback: Option<Tcc>) -> Option<Permission> {
        self.flow().view(Instant::now(), fallback)
    }

    /// One round at `now`: fire pending prompts, else check. Returns
    /// whether another round is wanted.
    pub fn round(&self, now: Instant) -> bool {
        let (need_request, checking, before) = {
            let f = self.flow();
            if !f.active() {
                return false;
            }
            (f.needs_request(), f.checking(now), f.view(now, None))
        };
        let actions = if need_request {
            match self.host.request() {
                Ok(tcc) => self.flow().requested(now, tcc),
                Err(e) => {
                    // K2 Screen isn't up yet: the next round tries again,
                    // until the window ends.
                    k2_core::log_debug!("[desktop] asking K2 Screen for its permission: {e}");
                    if !checking {
                        self.flow().need_request = false;
                    }
                    Vec::new()
                }
            }
        } else if checking {
            match self.host.check() {
                Ok(tcc) => self.flow().checked(now, tcc),
                Err(e) => {
                    k2_core::log_debug!("[desktop] checking K2 Screen's permission: {e}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        self.run(actions);
        let (again, after) = {
            let f = self.flow();
            (f.needs_request() || f.checking(now), f.view(now, None))
        };
        if before != after {
            (self.on_change)();
        }
        again
    }

    fn run(&self, actions: Vec<Action>) {
        for a in actions {
            let r = match a {
                Action::OpenPane(p) => self.host.open_pane(p),
                Action::RestartHelper => self.host.restart_helper(),
                Action::SaveAsked(x) => self.host.save_asked(x),
            };
            if let Err(e) = r {
                k2_core::log_debug!("[desktop] permission step {a:?}: {e}");
            }
        }
    }

    /// Starts the poll thread unless one runs (or this is a manual one).
    fn kick(self: &Arc<Self>) {
        if !self.spawn || self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("k2-desktop-permission".into()).spawn(move || {
            loop {
                if !me.round(Instant::now()) {
                    break;
                }
                std::thread::sleep(POLL);
            }
            me.running.store(false, Ordering::SeqCst);
            // An ask that landed between the last round and the flag:
            // pick it up.
            let again = {
                let f = me.flow();
                f.needs_request() || f.checking(Instant::now())
            };
            if again {
                me.kick();
            }
        });
        if spawned.is_err() {
            self.running.store(false, Ordering::SeqCst);
        }
    }
}

// ── the real host ────────────────────────────────────────────────────

/// `~/.k2/desktop-permission.json`: which prompts K2 has fired.
pub fn asked_path() -> PathBuf {
    k2_core::paths::k2_home().join("desktop-permission.json")
}

pub fn read_asked() -> Asked {
    k2_core::guard_test_store_home("desktop-permission.json");
    std::fs::read(asked_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn write_asked(a: Asked) -> Result<(), String> {
    k2_core::guard_test_store_home("desktop-permission.json");
    let p = asked_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = p.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(&a).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
}

/// K2 Screen over the daemon's link, and `/usr/bin/open` for the panes.
pub struct DeskdHost {
    pub m: Arc<super::deskd_link::DeskdManager>,
}

impl Host for DeskdHost {
    fn request(&self) -> Result<Tcc, String> {
        self.m.request_permission().map_err(|e| e.to_string())
    }
    fn check(&self) -> Result<Tcc, String> {
        self.m.check_permission().map_err(|e| e.to_string())
    }
    fn restart_helper(&self) -> Result<(), String> {
        self.m.restart_helper().map_err(|e| e.to_string())
    }
    fn open_pane(&self, pane: Pane) -> Result<(), String> {
        if !cfg!(target_os = "macos") {
            return Err("System Settings is on a Mac only".into());
        }
        let st = k2_core::bg_proc::command("/usr/bin/open")
            .arg(pane.url())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("open {}: {e}", pane.url()))?;
        if st.success() {
            Ok(())
        } else {
            Err(format!("open {} exited {st}", pane.url()))
        }
    }
    fn load_asked(&self) -> Asked {
        read_asked()
    }
    fn save_asked(&self, a: Asked) -> Result<(), String> {
        write_asked(a)
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
