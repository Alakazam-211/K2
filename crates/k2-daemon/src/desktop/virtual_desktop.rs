//! Virtual desktops on a Linux server (`prd-linux-virtual-desktop-v1`
//! §3, L3): the daemon runs them, `k2_core::desktop::virtual_desktop`
//! holds the rules.
//!
//! - **Start** (§3.3): Xvfb, then `dbus-run-session -- icewm-session`
//!   (openbox when IceWM is missing), each a transient `systemd --user`
//!   service in the desktop's slice `k2-desktops-<id>.slice` with its
//!   limits; the totals sit on `k2-desktops.slice`. Every program gets a
//!   clean environment (`env -i`) with `K2_NO_OWNER_FALLBACK=1`. K2 picks
//!   the display (:100 up, never one with a lock or a socket; Xvfb's own
//!   scan from :0 can take a live session's, L0 11-11), Xvfb reports it
//!   ready on `-displayfd`, and reads a fresh cookie per start; never
//!   `-ac`, no TCP, no abstract socket.
//! - **Sleep** (§3.4): release held keys, close K2 Screen's link to that
//!   display, then `systemctl --user freeze` both services. **Wake**: thaw.
//!   **Close**: thaw, then stop the slice (everything on it ends).
//! - **No user manager** (a compute job, a box without lingering): the
//!   two programs run as detached children, with no limits and no sleep;
//!   the desktop closes after 10 minutes with nobody viewing (LD-28). The
//!   freezer itself is behind a probe ([`Host::freezer`]) until L0
//!   confirms it on a fleet box.
//! - **Safe to repeat** (LD-32): a start of a running desktop returns it,
//!   of an asleep one wakes it; [`Manager::adopt`] reads every
//!   `~/.k2/desktop/<slug>/state.json` at boot and cleans up one whose
//!   programs are gone (a reboot, a crash), reporting it closed.
//!
//! Everything runs through [`Host`] and [`Clock`], so the tests use a fake
//! runner and a fake clock.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use k2_core::desktop::virtual_desktop::{self as vd, Action, Asker, BoxFacts, CloseReason, Closed, Desktop, Hold, Limits, Plan, Record, RunMode, Size, State, Waiter};
use serde_json::{json, Value};

pub use super::launch_agent::Output;

/// What the manager does to the machine (a fake in tests).
pub trait Host: Send + Sync {
    /// `systemctl` / `systemd-run` / `journalctl`, against this user's manager.
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output>;
    /// The no-user-manager fallback: a detached child in its own process
    /// group. Returns its pid.
    fn spawn_detached(&self, argv: &[String]) -> io::Result<u32>;
    fn alive(&self, pid: u32) -> bool;
    /// End a detached child's process group.
    fn kill_group(&self, pid: u32);
    fn facts(&self) -> BoxFacts;
    /// `/proc/sys/kernel/random/boot_id`.
    fn boot_id(&self) -> String;
    /// Is `name` on the system PATH (`/usr/local/bin`, `/usr/bin`, `/bin`)?
    fn has_program(&self, name: &str) -> bool;
    /// Which package manager installs things here (`apt`, `pacman`,
    /// `dnf`: the root helper's names), for the manual command.
    fn package_manager(&self) -> Option<&'static str>;
    /// Does this user have a systemd user manager (transient services)?
    fn systemd_user(&self) -> bool;
    /// Can that manager freeze a unit (cgroup v2, systemd 246+)?
    fn freezer(&self) -> bool;
    /// `/tmp/.X11-unix` is there and world-writable with the sticky bit:
    /// Xvfb run as a normal user won't create it (L0 11-11). `Err` says
    /// what is wrong.
    fn x11_socket_dir(&self) -> Result<(), String>;
    /// Display `n` is in use or claimed: `/tmp/.X<n>-lock` or
    /// `/tmp/.X11-unix/X<n>` exists. Only read, never touched.
    fn display_taken(&self, n: u32) -> bool;
    /// Ask the kernel to move up to `bytes` of a frozen cgroup's memory to
    /// swap (`memory.reclaim`; `cgroup` is its path under
    /// /sys/fs/cgroup). EAGAIN means it took what it could: that is `Ok`.
    fn reclaim(&self, cgroup: &str, bytes: u64) -> Result<(), String>;
    /// Where `at-spi-bus-launcher` is installed, if it is (L8: the
    /// accessibility bus agent-desktop reads).
    fn atspi_launcher(&self) -> Option<String>;
}

/// The session with the accessibility bus (L8): `$1` is the launcher, the
/// rest the window manager. The launcher runs on the desktop's own session
/// bus and publishes the accessibility bus on the display (AT_SPI_BUS), so
/// a program given only DISPLAY finds it.
pub const ATSPI_SH: &str = "l=$1; shift; \"$l\" --launch-immediately & exec \"$@\"";

/// Where distros put at-spi-bus-launcher (it is not on PATH).
pub const ATSPI_LAUNCHERS: [&str; 4] =
    ["/usr/libexec/at-spi-bus-launcher", "/usr/libexec/at-spi2-core/at-spi-bus-launcher", "/usr/lib/at-spi2-core/at-spi-bus-launcher", "/usr/lib/at-spi-bus-launcher"];

/// `memory.reclaim`'s answer (L0 11-11): EAGAIN after it reclaimed what it
/// could is normal, not a failure.
pub fn reclaim_result(r: io::Result<()>) -> Result<(), String> {
    match r {
        Ok(()) => Ok(()),
        // EAGAIN is `WouldBlock` on every Unix.
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// How many display numbers a start tries when Xvfb exits at once (a
/// number taken between the check and the start).
pub const DISPLAY_TRIES: u32 = 5;

/// Held from picking a display number until Xvfb reports it ready.
static START_LOCK: Mutex<()> = Mutex::new(());

/// [`Host::display_taken`] under `tmp` (`/tmp`). Pure reads.
pub fn display_taken_in(tmp: &Path, n: u32) -> bool {
    std::fs::symlink_metadata(tmp.join(format!(".X{n}-lock"))).is_ok() || std::fs::symlink_metadata(tmp.join(".X11-unix").join(format!("X{n}"))).is_ok()
}

/// Unix seconds.
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
}

/// K2 Screen's link to one desktop (installed by the routes' side).
pub trait Screen: Send + Sync {
    /// Release every held key and button on that display.
    fn release_keys(&self, desktop: &str);
    /// Close the link (K2 Screen stops capturing it); viewers are told.
    fn close(&self, desktop: &str, state: State);
}

/// No K2 Screen attached (boot, tests that don't watch it).
pub struct NoScreen;

impl Screen for NoScreen {
    fn release_keys(&self, _: &str) {}
    fn close(&self, _: &str, _: State) {}
}

/// Paths and programs.
pub struct Config {
    /// `~/.k2/desktop`.
    pub root: PathBuf,
    /// `HOME` for the programs on a desktop.
    pub home: PathBuf,
    /// The session command instead of the window manager found on the
    /// box (the Xvfb integration tests, where no window manager is
    /// installed).
    pub session: Option<Vec<String>>,
    /// How long Xvfb has to report its display.
    pub display_wait: Duration,
    /// The first display number tried (`vd::DISPLAY_BASE`; tests spread out).
    pub display_base: u32,
}

impl Config {
    pub fn for_home(home: &Path) -> Config {
        Config { root: home.join(".k2").join("desktop"), home: home.to_path_buf(), session: None, display_wait: Duration::from_secs(5), display_base: vd::DISPLAY_BASE }
    }
}

/// Why a start was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    BadId,
    /// Programs missing (names for the person), with the manual command.
    NotSetUp { missing: Vec<&'static str>, manual: Option<String> },
    TooMany { closable: Vec<String> },
    LowMemory,
    AgentPaused { until: i64 },
    /// `/tmp/.X11-unix` is missing or wrong, with what.
    NoSocketDir(String),
    Failed(String),
}

impl StartError {
    pub fn code(&self) -> &'static str {
        match self {
            StartError::BadId => "bad_desktop",
            StartError::NotSetUp { .. } => "not_set_up",
            StartError::TooMany { .. } => "too_many_desktops",
            StartError::LowMemory => "low_memory",
            StartError::AgentPaused { .. } => "agent_wake_paused",
            StartError::NoSocketDir(_) => "no_x11_socket_dir",
            StartError::Failed(_) => "start_failed",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            StartError::BadId => 400,
            StartError::LowMemory => 503,
            StartError::Failed(_) => 500,
            _ => 409,
        }
    }

    pub fn hint(&self, limits: &Limits) -> String {
        match self {
            StartError::BadId => "that isn't a virtual desktop id (virtual, or virtual:<workspace id>)".into(),
            StartError::NotSetUp { missing, .. } => format!("This server has no screen, and K2 needs {} to start a desktop on it (k2 desktop setup).", missing.join(", ")),
            StartError::TooMany { .. } => format!("This server already has {} desktops, its limit. Close one first (k2 desktop stop --desktop <id>).", limits.max_desktops),
            StartError::LowMemory => "This server has less than 1 GB of memory free, so K2 won't start a desktop on it now.".into(),
            StartError::AgentPaused { until } => format!("Agents kept this desktop awake for 4 hours with nobody watching, so an agent can't wake it again until {until} (Unix time). A person can."),
            StartError::NoSocketDir(why) => format!(
                "K2 can't start a desktop on this server: {why}. The system makes /tmp/.X11-unix at boot (systemd-tmpfiles); an administrator can run: sudo install -d -m 1777 /tmp/.X11-unix"
            ),
            StartError::Failed(why) => format!("The desktop didn't start: {why}"),
        }
    }
}

/// One desktop's status (the routes add who may do what).
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub id: String,
    pub state: State,
    pub closed: Option<Closed>,
    pub size: Option<Size>,
    pub display: Option<String>,
    pub memory_bytes: Option<u64>,
    pub present: Vec<(String, String)>,
    pub hold: Option<Hold>,
    pub next_sleep_at: Option<i64>,
    pub next_close_at: Option<i64>,
    pub can_sleep: bool,
    pub waiting: Option<usize>,
    pub limits: Limits,
    pub used_desktops: u32,
    pub used_awake: u32,
    pub missing: Vec<&'static str>,
    pub manual: Option<String>,
    /// Awake 7 days in all: status suggests closing and starting again.
    pub suggest_recycle: bool,
    /// A browser is installed (LD-20: else "Opens with no apps").
    pub has_browser: bool,
}

impl Status {
    /// `not_set_up` when closed with programs missing; else the state.
    pub fn wire_state(&self) -> &'static str {
        if self.state == State::Closed && !self.missing.is_empty() {
            "not_set_up"
        } else {
            self.state.as_wire()
        }
    }

    pub fn message(&self) -> String {
        if self.wire_state() == "not_set_up" {
            return "This server has no screen. K2 can start a desktop on it once three small packages are installed.".into();
        }
        match (&self.state, &self.closed) {
            (State::Closed, Some(c)) => c.reason.message(),
            (State::Closed, None) if !self.can_sleep => format!("{} {}", self.state.message(self.limits.max_awake), vd::NO_FREEZER),
            _ => self.state.message(self.limits.max_awake),
        }
    }

    /// The `virtual` object of `GET /cli/desktop/status`.
    pub fn virtual_json(&self, label: Option<&str>) -> Value {
        json!({
            "size": self.size.map(|s| json!({"width": s.width, "height": s.height})),
            "display": self.display,
            "memoryBytes": self.memory_bytes,
            "present": self.present.iter().map(|(v, u)| json!({"viewer": v, "user": u})).collect::<Vec<_>>(),
            "hold": self.hold.as_ref().map(|h| json!({"by": h.by, "until": h.until})),
            "nextSleepAt": self.next_sleep_at,
            "nextCloseAt": self.next_close_at,
            "canSleep": self.can_sleep,
            "waiting": self.waiting.map(|p| json!({"position": p, "awakeLimit": self.limits.max_awake})),
            "label": label,
            "limits": {
                "desktops": {"max": self.limits.max_desktops, "used": self.used_desktops},
                "awake": {"max": self.limits.max_awake, "used": self.used_awake},
                "memoryBytes": {"max": self.limits.total_memory, "each": self.limits.each_memory},
                "cpuCores": self.limits.cpu_cores,
            },
            "missing": self.missing,
            "manual": self.manual,
            "setupCard": if self.missing.is_empty() { None } else { Some(vd::SETUP_CARD) },
            "hasBrowser": self.has_browser,
            "suggestRecycle": self.suggest_recycle,
        })
    }

    pub fn reason_json(&self) -> Value {
        match (&self.state, &self.closed) {
            (State::Closed, Some(c)) => serde_json::to_value(c).unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }
}

/// The shell that hands Xvfb's `-displayfd 3` to a file: `$1` is the
/// file, the rest is Xvfb and its arguments.
pub const DISPLAYFD_SH: &str = "f=$1; shift; exec \"$@\" 3>\"$f\"";

#[derive(Default)]
struct Inner {
    desktops: BTreeMap<String, Desktop>,
    records: BTreeMap<String, Record>,
    queue: Vec<Waiter>,
    /// The size asked by a waiting start.
    pending_size: BTreeMap<String, Option<Size>>,
}

/// Told `(desktop, new state, why it closed)` on every change.
type OnChange = Arc<dyn Fn(&str, State, Option<&CloseReason>) + Send + Sync>;

/// Every virtual desktop on this daemon (a map by id, LD-21).
pub struct Manager {
    host: Arc<dyn Host>,
    clock: Arc<dyn Clock>,
    cfg: Config,
    screen: RwLock<Arc<dyn Screen>>,
    on_change: RwLock<Option<OnChange>>,
    inner: Mutex<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

impl Manager {
    pub fn new(host: Arc<dyn Host>, clock: Arc<dyn Clock>, cfg: Config) -> Arc<Manager> {
        Arc::new(Manager { host, clock, cfg, screen: RwLock::new(Arc::new(NoScreen)), on_change: RwLock::new(None), inner: Mutex::new(Inner::default()) })
    }

    /// K2 Screen's side (the routes install it at boot).
    pub fn set_screen(&self, s: Arc<dyn Screen>) {
        *self.screen.write().unwrap_or_else(|p| p.into_inner()) = s;
    }

    /// Told of every state change (the routes emit `desktop_state`).
    pub fn set_on_change(&self, f: OnChange) {
        *self.on_change.write().unwrap_or_else(|p| p.into_inner()) = Some(f);
    }

    fn screen(&self) -> Arc<dyn Screen> {
        self.screen.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn changed(&self, id: &str, state: State) {
        self.changed_why(id, state, None);
    }

    fn changed_why(&self, id: &str, state: State, why: Option<&CloseReason>) {
        let f = self.on_change.read().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(f) = f {
            f(id, state, why);
        }
    }

    pub fn now(&self) -> i64 {
        self.clock.now()
    }

    pub fn limits(&self) -> Limits {
        Limits::for_box(&self.host.facts())
    }

    pub fn mode(&self) -> RunMode {
        if self.host.systemd_user() {
            RunMode::Systemd
        } else {
            RunMode::Detached
        }
    }

    /// Sleep is possible: a user manager that can freeze.
    pub fn can_sleep(&self) -> bool {
        self.mode() == RunMode::Systemd && self.host.freezer()
    }

    /// The session command: the override, IceWM, else openbox, on its own
    /// session bus, with the accessibility bus when it is installed.
    fn window_manager(&self) -> Option<Vec<String>> {
        if let Some(s) = &self.cfg.session {
            return Some(s.clone());
        }
        let wm = if self.host.has_program("icewm-session") {
            "icewm-session"
        } else if self.host.has_program("openbox-session") {
            "openbox-session"
        } else {
            return None;
        };
        Some(match self.host.atspi_launcher() {
            Some(l) => {
                let mut v = s(&["dbus-run-session", "--", "/bin/sh", "-c", ATSPI_SH, "k2-session"]);
                v.push(l);
                v.push(wm.to_string());
                v
            }
            None => s(&["dbus-run-session", "--", wm]),
        })
    }

    /// The desktop's browser profile folder (LD-31): `<its folder>/profile`,
    /// 0700, made on first use, kept across closes. Never copied or linked
    /// from anywhere: each desktop's logins are its own.
    pub fn profile_dir(&self, id: &str) -> io::Result<PathBuf> {
        let p = self.dir(id).join("profile");
        make_private_dir(&self.dir(id))?;
        make_private_dir(&p)?;
        Ok(p)
    }

    /// What a start needs and the box lacks (empty: set up).
    pub fn missing(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if !self.host.has_program("Xvfb") {
            m.push("Xvfb");
        }
        if self.cfg.session.is_none() {
            if self.window_manager().is_none() {
                m.push("a window manager (IceWM)");
            }
            if !self.host.has_program("dbus-run-session") {
                m.push("dbus-run-session");
            }
        }
        m
    }

    /// The command a person runs to install the packages by hand (the
    /// root helper's own lists, `k2_root_proto::desktop`).
    pub fn manual(&self) -> Option<String> {
        Some(k2_root_proto::desktop::manual_command(self.host.package_manager()))
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.cfg.root.join(vd::folder_slug(id))
    }

    fn cookie_path(&self, id: &str) -> PathBuf {
        self.dir(id).join("Xauthority")
    }

    fn display_file(&self, id: &str) -> PathBuf {
        self.dir(id).join("display")
    }

    fn state_file(&self, id: &str) -> PathBuf {
        self.dir(id).join("state.json")
    }

    fn write_record(&self, r: &Record) {
        let path = self.state_file(&r.id);
        let body = match serde_json::to_vec_pretty(r) {
            Ok(b) => b,
            Err(e) => {
                k2_core::log_debug!("[vdesktop] state for {}: {e}", r.id);
                return;
            }
        };
        if let Err(e) = write_private(&path, &body) {
            k2_core::log_debug!("[vdesktop] writing {}: {e}", path.display());
        }
    }

    fn systemctl(&self, args: &[&str]) -> io::Result<Output> {
        let mut a = vec!["--user".to_string()];
        a.extend(args.iter().map(|x| x.to_string()));
        self.host.run("systemctl", &a)
    }

    fn systemctl_ok(&self, args: &[&str]) -> Result<(), String> {
        match self.systemctl(args) {
            Ok(o) if o.status == 0 => Ok(()),
            Ok(o) => Err(format!("systemctl {}: {}", args.join(" "), o.stderr.trim())),
            Err(e) => Err(format!("systemctl {}: {e}", args.join(" "))),
        }
    }

    // ── start ────────────────────────────────────────────────────────

    /// Start (or wake, or return) a desktop. `Ok` carries its state now:
    /// `ready`, `waiting` (queued for an awake slot) or `starting` (another
    /// request is starting it).
    pub fn start(&self, id: &str, asker: Asker, size: Option<Size>) -> Result<State, StartError> {
        if !vd::is_virtual(id) {
            return Err(StartError::BadId);
        }
        let missing = self.missing();
        if !missing.is_empty() {
            return Err(StartError::NotSetUp { missing, manual: self.manual() });
        }
        if self.state(id) == State::Closed {
            self.host.x11_socket_dir().map_err(StartError::NoSocketDir)?;
        }
        let (facts, now) = (self.host.facts(), self.now());
        let limits = Limits::for_box(&facts);
        let plan = {
            let mut g = lock(&self.inner);
            let inner = &mut *g;
            let plan = vd::plan_start(&inner.desktops, &mut inner.queue, id, asker, &limits, &facts, now);
            let d = inner.desktops.entry(id.to_string()).or_insert_with(|| Desktop::new(id));
            match &plan {
                Plan::Start => d.state = State::Starting,
                Plan::Wake => d.state = State::Waking,
                Plan::Wait { .. } => {
                    if d.state == State::Closed {
                        d.state = State::Waiting;
                    }
                    inner.pending_size.insert(id.to_string(), size);
                }
                _ => {}
            }
            plan
        };
        match plan {
            Plan::AsIs => Ok(lock(&self.inner).desktops.get(id).map_or(State::Closed, |d| d.state)),
            Plan::Wait { .. } => {
                let st = lock(&self.inner).desktops.get(id).map_or(State::Waiting, |d| d.state);
                self.changed(id, st);
                Ok(State::Waiting)
            }
            Plan::TooMany { closable } => {
                self.forget_if_closed(id);
                Err(StartError::TooMany { closable })
            }
            Plan::LowMemory => {
                self.forget_if_closed(id);
                Err(StartError::LowMemory)
            }
            Plan::AgentPaused { until } => Err(StartError::AgentPaused { until }),
            Plan::Start => {
                self.changed(id, State::Starting);
                self.do_start(id, size).map(|_| State::Ready)
            }
            Plan::Wake => {
                self.changed(id, State::Waking);
                self.do_wake(id).map(|_| State::Ready)
            }
        }
    }

    /// A refused first start leaves no entry behind.
    fn forget_if_closed(&self, id: &str) {
        let mut g = lock(&self.inner);
        if g.desktops.get(id).is_some_and(|d| d.state == State::Closed && d.closed.is_none() && !g.records.contains_key(id)) {
            g.desktops.remove(id);
        }
    }

    fn do_start(&self, id: &str, size: Option<Size>) -> Result<(), StartError> {
        let size = vd::clamp_size(size);
        let mode = self.mode();
        let units = vd::units(id);
        let started = self.launch(id, size, mode, &units);
        let now = self.now();
        match started {
            Ok((display, pids)) => {
                let rec = Record {
                    id: id.to_string(),
                    mode,
                    state: State::Ready,
                    closed: None,
                    display: Some(display),
                    size: Some(size),
                    units,
                    pids,
                    started_at: Some(now),
                    asleep_since: None,
                    boot_id: self.host.boot_id(),
                    holds: Default::default(),
                };
                self.write_record(&rec);
                {
                    let mut g = lock(&self.inner);
                    g.records.insert(id.to_string(), rec);
                    let d = g.desktops.entry(id.to_string()).or_insert_with(|| Desktop::new(id));
                    d.mark_awake(now);
                    d.size = Some(size);
                    d.started_at = Some(now);
                }
                self.changed(id, State::Ready);
                Ok(())
            }
            Err(why) => {
                k2_core::log_debug!("[vdesktop] {id} didn't start: {why}");
                self.close_with(id, CloseReason::Crashed { detail: why.clone() });
                Err(StartError::Failed(why))
            }
        }
    }

    /// Xvfb, its display, the session. Returns the display and (detached)
    /// the pids.
    fn launch(&self, id: &str, size: Size, mode: RunMode, units: &vd::Units) -> Result<(String, Vec<u32>), String> {
        let dir = self.dir(id);
        make_private_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let cookie_path = self.cookie_path(id);
        let mut cookie = [0u8; 16];
        getrandom::getrandom(&mut cookie).map_err(|e| format!("a random cookie: {e}"))?;
        write_private(&cookie_path, &vd::xauth_bytes(&cookie)).map_err(|e| format!("{}: {e}", cookie_path.display()))?;
        let display_file = self.display_file(id);
        let _ = std::fs::remove_file(&display_file);
        let home = self.cfg.home.display().to_string();
        let cookie_s = cookie_path.display().to_string();

        // K2 picks the display (never Xvfb's own scan, L0 11-11): from :100
        // up, skipping any number with a lock file or a socket. Xvfb's lock
        // refuses one taken in a race: it exits, and the next one is tried.
        // One start at a time picks a number: two Xvfbs racing for the same
        // free number can both get past its lock, and the second unlinks
        // the first one's socket (`-nolisten local`), so the first desktop's
        // display then answers with the second's server.
        let _picking = START_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut pids = Vec::new();
        let mut from = self.cfg.display_base;
        let mut tries = 0;
        let display = loop {
            tries += 1;
            let n = vd::pick_display(|n| self.host.display_taken(n), from).ok_or("no free X display number between :100 and :999")?;
            let mut x_cmd = vd::clean_env_prefix(&vd::session_env(&home, None, &cookie_s));
            x_cmd.extend(s(&["/bin/sh", "-c", DISPLAYFD_SH, "k2-desktop"]));
            x_cmd.push(display_file.display().to_string());
            x_cmd.push("Xvfb".into());
            x_cmd.extend(vd::xvfb_args(n, size, &cookie_s));
            pids.clear();
            match mode {
                RunMode::Systemd => self.run_unit(&units.x, &units.slice, &x_cmd)?,
                RunMode::Detached => pids.push(self.host.spawn_detached(&x_cmd).map_err(|e| format!("Xvfb: {e}"))?),
            }
            match self.wait_display(&display_file, mode, units, pids.first().copied()) {
                Ok(d) => break d,
                Err((e, exited)) => {
                    self.stop_programs(mode, units, &pids);
                    if !exited || tries >= DISPLAY_TRIES {
                        return Err(e);
                    }
                    k2_core::log_debug!("[vdesktop] {id}: Xvfb on :{n} exited ({e}); trying the next display");
                    from = n + 1;
                }
            }
        };
        let wm = self.window_manager().ok_or("no window manager")?;
        let mut session_cmd = vd::clean_env_prefix(&vd::session_env(&home, Some(&display), &cookie_s));
        session_cmd.extend(wm);
        let session = match mode {
            RunMode::Systemd => self.run_unit(&units.session, &units.slice, &session_cmd),
            RunMode::Detached => self.host.spawn_detached(&session_cmd).map(|p| pids.push(p)).map_err(|e| format!("the window manager: {e}")),
        };
        if let Err(e) = session {
            self.stop_programs(mode, units, &pids);
            return Err(e);
        }
        if mode == RunMode::Systemd {
            self.apply_limits(units);
        }
        Ok((display, pids))
    }

    fn run_unit(&self, unit: &str, slice: &str, cmd: &[String]) -> Result<(), String> {
        let mut a = s(&["--user", "--quiet", "--collect"]);
        a.push(format!("--unit={unit}"));
        a.push(format!("--slice={slice}"));
        a.push(format!("--property=TasksMax={}", vd::TASKS_MAX));
        a.push("--".into());
        a.extend(cmd.iter().cloned());
        match self.host.run("systemd-run", &a) {
            Ok(o) if o.status == 0 => Ok(()),
            Ok(o) => Err(format!("systemd-run {unit}: {}", o.stderr.trim())),
            Err(e) => Err(format!("systemd-run {unit}: {e}")),
        }
    }

    /// The desktop's limits on its slice and the totals on the parent.
    /// A box whose manager lacks a controller ignores that line; the
    /// failure is logged, never fatal.
    fn apply_limits(&self, units: &vd::Units) {
        let l = self.limits();
        let each = [
            format!("MemoryMax={}", l.each_memory),
            format!("TasksMax={}", vd::TASKS_MAX),
            "CPUQuota=200%".to_string(),
        ];
        let total = [format!("MemoryMax={}", l.total_memory), format!("CPUQuota={}%", l.cpu_cores * 100)];
        for (unit, props) in [(units.slice.as_str(), &each[..]), (vd::PARENT_SLICE, &total[..])] {
            let mut a = vec!["set-property", "--runtime", unit];
            a.extend(props.iter().map(String::as_str));
            if let Err(e) = self.systemctl_ok(&a) {
                k2_core::log_debug!("[vdesktop] limits on {unit}: {e}");
            }
        }
    }

    /// Xvfb writes its display to the file once it is ready. `Err` says
    /// why not, and whether Xvfb exited (a display taken in a race: try the
    /// next) rather than hung.
    fn wait_display(&self, file: &Path, mode: RunMode, units: &vd::Units, pid: Option<u32>) -> Result<String, (String, bool)> {
        let deadline = Instant::now() + self.cfg.display_wait;
        let mut polls = 0u32;
        loop {
            if let Ok(t) = std::fs::read_to_string(file) {
                if let Some(n) = t.lines().next().map(str::trim).filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
                    if t.ends_with('\n') {
                        return Ok(format!(":{n}"));
                    }
                }
            }
            polls += 1;
            let gone = match mode {
                RunMode::Detached => pid.is_some_and(|p| !self.host.alive(p)),
                // Ask the manager every 200 ms, not every poll.
                RunMode::Systemd => polls % 8 == 0 && !self.systemctl(&["is-active", "--quiet", &units.x]).is_ok_and(|o| o.status == 0),
            };
            if gone || Instant::now() >= deadline {
                let last = self.last_line(mode, units);
                let why = if last.is_empty() { format!("Xvfb reported no display within {} s", self.cfg.display_wait.as_secs()) } else { format!("Xvfb: {last}") };
                return Err((why, gone));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// The X server's last log line (journal; empty when detached).
    fn last_line(&self, mode: RunMode, units: &vd::Units) -> String {
        if mode != RunMode::Systemd {
            return String::new();
        }
        match self.host.run("journalctl", &s(&["--user", "-u", &units.x, "-n", "1", "-o", "cat", "--no-pager"])) {
            Ok(o) if o.status == 0 => o.stdout.lines().last().unwrap_or("").trim().to_string(),
            _ => String::new(),
        }
    }

    fn stop_programs(&self, mode: RunMode, units: &vd::Units, pids: &[u32]) {
        match mode {
            RunMode::Systemd => {
                // Thaw first: stopping a frozen unit can hang.
                let _ = self.systemctl(&["thaw", &units.session]);
                let _ = self.systemctl(&["thaw", &units.x]);
                if let Err(e) = self.systemctl_ok(&["stop", &units.slice]) {
                    k2_core::log_debug!("[vdesktop] stopping {}: {e}", units.slice);
                }
            }
            RunMode::Detached => {
                for p in pids.iter().rev() {
                    self.host.kill_group(*p);
                }
            }
        }
    }

    // ── sleep, wake, close ───────────────────────────────────────────

    fn do_sleep(&self, id: &str) {
        let Some(rec) = lock(&self.inner).records.get(id).cloned() else { return };
        // Order (§3.4): held keys, then K2 Screen's link, then freeze.
        let screen = self.screen();
        screen.release_keys(id);
        screen.close(id, State::Asleep);
        let frozen = self.systemctl_ok(&["freeze", &rec.units.session]).and_then(|_| self.systemctl_ok(&["freeze", &rec.units.x]));
        if let Err(e) = frozen {
            // Can't pause it: close instead, saying so (LD-28).
            k2_core::log_debug!("[vdesktop] freezing {id}: {e}");
            self.close_with(id, CloseReason::NobodyViewing);
            return;
        }
        // Frozen: let the kernel move what it holds to swap (on a box with
        // swap; elsewhere only page cache goes). Best effort.
        match self.systemctl(&["show", "-p", "ControlGroup", "--value", &rec.units.slice]) {
            Ok(o) if o.status == 0 && !o.stdout.trim().is_empty() => {
                if let Err(e) = self.host.reclaim(o.stdout.trim(), self.limits().each_memory) {
                    k2_core::log_debug!("[vdesktop] reclaiming {id}'s memory: {e}");
                }
            }
            _ => k2_core::log_debug!("[vdesktop] {id}: no control group to reclaim"),
        }
        let now = self.now();
        let rec = {
            let mut g = lock(&self.inner);
            if let Some(d) = g.desktops.get_mut(id) {
                d.mark_asleep(now);
            }
            g.records.get_mut(id).map(|r| {
                r.state = State::Asleep;
                r.asleep_since = Some(now);
                r.clone()
            })
        };
        if let Some(r) = rec {
            self.write_record(&r);
        }
        self.changed(id, State::Asleep);
    }

    fn do_wake(&self, id: &str) -> Result<(), StartError> {
        let Some(rec) = lock(&self.inner).records.get(id).cloned() else {
            return Err(StartError::Failed("no record of that desktop".into()));
        };
        if let Err(e) = self.systemctl_ok(&["thaw", &rec.units.x]).and_then(|_| self.systemctl_ok(&["thaw", &rec.units.session])) {
            let detail = format!("it couldn't be woken: {e}");
            self.close_with(id, CloseReason::Crashed { detail: detail.clone() });
            return Err(StartError::Failed(detail));
        }
        let now = self.now();
        let rec = {
            let mut g = lock(&self.inner);
            if let Some(d) = g.desktops.get_mut(id) {
                d.mark_awake(now);
            }
            g.records.get_mut(id).map(|r| {
                r.state = State::Ready;
                r.asleep_since = None;
                r.clone()
            })
        };
        if let Some(r) = rec {
            self.write_record(&r);
        }
        self.changed(id, State::Ready);
        Ok(())
    }

    /// Close a desktop (what was open on it is lost) with why.
    pub fn close_with(&self, id: &str, reason: CloseReason) -> bool {
        let why = reason.clone();
        let rec = lock(&self.inner).records.get(id).cloned();
        let was_running = rec.as_ref().is_some_and(|r| r.state != State::Closed);
        self.screen().close(id, State::Closed);
        if let Some(r) = rec.as_ref().filter(|r| r.state != State::Closed) {
            self.stop_programs(r.mode, &r.units, &r.pids);
        }
        let _ = std::fs::remove_file(self.cookie_path(id));
        let _ = std::fs::remove_file(self.display_file(id));
        let now = self.now();
        let rec = {
            let mut g = lock(&self.inner);
            g.queue.retain(|w| w.id != id);
            g.pending_size.remove(id);
            let d = g.desktops.entry(id.to_string()).or_insert_with(|| Desktop::new(id));
            d.mark_closed(Some(reason.clone()), now);
            let holds = vd::Holds::of(d);
            let r = g.records.entry(id.to_string()).or_insert_with(|| Record {
                id: id.to_string(),
                mode: RunMode::Systemd,
                state: State::Closed,
                closed: None,
                display: None,
                size: None,
                units: vd::units(id),
                pids: Vec::new(),
                started_at: None,
                asleep_since: None,
                boot_id: String::new(),
                holds: Default::default(),
            });
            r.state = State::Closed;
            r.closed = Some(Closed { reason, at: now });
            r.display = None;
            r.size = None;
            r.pids.clear();
            r.started_at = None;
            r.asleep_since = None;
            r.holds = holds;
            r.clone()
        };
        if self.dir(id).is_dir() {
            self.write_record(&rec);
        }
        self.changed_why(id, State::Closed, Some(&why));
        was_running
    }

    /// The Owner's switch went off: every virtual desktop closes.
    pub fn close_all(&self, reason: CloseReason) -> usize {
        let ids: Vec<String> = lock(&self.inner).desktops.values().filter(|d| d.state != State::Closed).map(|d| d.id.clone()).collect();
        ids.iter().filter(|id| self.close_with(id, reason.clone())).count()
    }

    // ── the timer ────────────────────────────────────────────────────

    /// One tick: a desktop whose programs died is closed with Xvfb's last
    /// line; then the rules ([`vd::step`]) and what they ask.
    pub fn tick(&self) {
        let awake: Vec<Record> = {
            let g = lock(&self.inner);
            g.desktops.values().filter(|d| d.state == State::Ready).filter_map(|d| g.records.get(&d.id).cloned()).collect()
        };
        for r in awake {
            if !self.running(&r) {
                let detail = self.last_line(r.mode, &r.units);
                self.close_with(&r.id, CloseReason::Crashed { detail });
            }
        }
        let (facts, now, can_sleep) = (self.host.facts(), self.now(), self.can_sleep());
        let limits = Limits::for_box(&facts);
        let actions = {
            let mut g = lock(&self.inner);
            let inner = &mut *g;
            vd::step(&mut inner.desktops, &mut inner.queue, &limits, &facts, can_sleep, now)
        };
        for a in actions {
            match a {
                Action::Sleep(id) => self.do_sleep(&id),
                Action::Close(id, why) => {
                    self.close_with(&id, why);
                }
                Action::Wake(id) => {
                    let _ = self.do_wake(&id);
                }
                Action::Proceed(id) => self.proceed(&id),
            }
        }
        // The hold cap's bookkeeping moves on the tick: keep it too.
        let ids: Vec<String> = lock(&self.inner).records.keys().cloned().collect();
        for id in ids {
            self.save_holds(&id);
        }
    }

    /// A queued request whose slot came free.
    fn proceed(&self, id: &str) {
        let (state, size) = {
            let mut g = lock(&self.inner);
            let size = g.pending_size.remove(id).flatten();
            let Some(d) = g.desktops.get_mut(id) else { return };
            let st = d.state;
            match st {
                State::Asleep => d.state = State::Waking,
                State::Closed | State::Waiting => d.state = State::Starting,
                _ => return,
            }
            (st, size)
        };
        let r = if state == State::Asleep {
            self.changed(id, State::Waking);
            self.do_wake(id)
        } else {
            self.changed(id, State::Starting);
            self.do_start(id, size)
        };
        if let Err(e) = r {
            k2_core::log_debug!("[vdesktop] queued start of {id}: {e:?}");
        }
    }

    /// Are a desktop's programs still there?
    fn running(&self, r: &Record) -> bool {
        match r.mode {
            RunMode::Systemd => self.systemctl(&["is-active", "--quiet", &r.units.x, &r.units.session]).is_ok_and(|o| o.status == 0),
            RunMode::Detached => !r.pids.is_empty() && r.pids.iter().all(|p| self.host.alive(*p)),
        }
    }

    fn frozen(&self, r: &Record) -> bool {
        r.mode == RunMode::Systemd
            && self.systemctl(&["show", "-p", "FreezerState", "--value", &r.units.x]).is_ok_and(|o| o.status == 0 && o.stdout.trim() == "frozen")
    }

    /// Boot: read every state file. Running desktops are taken back (the
    /// 10-minute count starts over); one whose programs are gone, or from
    /// before a reboot, is cleaned up and reported closed with why.
    pub fn adopt(&self) {
        let Ok(rd) = std::fs::read_dir(&self.cfg.root) else { return };
        let boot = self.host.boot_id();
        let now = self.now();
        for e in rd.flatten() {
            let path = e.path().join("state.json");
            let Ok(body) = std::fs::read(&path) else { continue };
            let rec: Record = match serde_json::from_slice(&body) {
                Ok(r) => r,
                Err(err) => {
                    k2_core::log_debug!("[vdesktop] {}: {err}", path.display());
                    continue;
                }
            };
            if !vd::is_virtual(&rec.id) || self.dir(&rec.id) != e.path() {
                continue;
            }
            let id = rec.id.clone();
            if rec.state == State::Closed {
                let mut g = lock(&self.inner);
                let d = g.desktops.entry(id.clone()).or_insert_with(|| Desktop::new(&id));
                d.state = State::Closed;
                d.closed = rec.closed.clone();
                g.records.insert(id, rec);
                continue;
            }
            lock(&self.inner).records.insert(id.clone(), rec.clone());
            if !rec.boot_id.is_empty() && rec.boot_id != boot {
                self.close_with(&id, CloseReason::Restarted);
                continue;
            }
            if !self.running(&rec) {
                let detail = self.last_line(rec.mode, &rec.units);
                let detail = if detail.is_empty() { "its programs had stopped".to_string() } else { detail };
                self.close_with(&id, CloseReason::Crashed { detail });
                continue;
            }
            let asleep = self.frozen(&rec);
            {
                let mut g = lock(&self.inner);
                let d = g.desktops.entry(id.clone()).or_insert_with(|| Desktop::new(&id));
                d.size = rec.size;
                d.started_at = rec.started_at;
                rec.holds.restore(d, now);
                if asleep {
                    d.state = State::Asleep;
                    d.asleep_since = Some(rec.asleep_since.unwrap_or(now));
                } else {
                    d.mark_awake(now);
                }
            }
            // K2 Screen starts again on an awake one (it died with the old
            // daemon, LD-9).
            if !asleep {
                self.changed(&id, State::Ready);
            }
        }
    }

    // ── viewers and holds ────────────────────────────────────────────

    fn with_desktop<R>(&self, id: &str, f: impl FnOnce(&mut Desktop, i64) -> R) -> Option<R> {
        let now = self.now();
        lock(&self.inner).desktops.get_mut(id).map(|d| f(d, now))
    }

    pub fn viewer_joined(&self, id: &str, viewer: &str, user: &str, may_wake: bool) {
        let now = self.now();
        lock(&self.inner).desktops.entry(id.to_string()).or_insert_with(|| Desktop::new(id)).viewer_joined(viewer, user, may_wake, now);
    }

    pub fn viewer_present(&self, id: &str, viewer: &str) {
        self.with_desktop(id, |d, now| d.viewer_present(viewer, now));
    }

    pub fn viewer_hidden(&self, id: &str, viewer: &str, hidden: bool) {
        self.with_desktop(id, |d, now| d.viewer_hidden(viewer, hidden, now));
    }

    pub fn viewer_left(&self, id: &str, viewer: &str) {
        self.with_desktop(id, |d, _| d.viewer_left(viewer));
    }

    /// An agent's hold (LD-29): `Err` names who holds it until when.
    pub fn hold(&self, id: &str, by: &str) -> Result<Hold, Hold> {
        let now = self.now();
        let r = lock(&self.inner).desktops.entry(id.to_string()).or_insert_with(|| Desktop::new(id)).take_hold(by, now);
        if r.is_ok() {
            self.save_holds(id);
        }
        r
    }

    pub fn release(&self, id: &str, by: &str) -> bool {
        let released = self.with_desktop(id, |d, _| d.release_hold(by)).unwrap_or(false);
        if released {
            self.save_holds(id);
        }
        released
    }

    /// Write the desktop's hold fields into its `state.json` when they
    /// changed (a desktop with no record yet has nothing to keep).
    fn save_holds(&self, id: &str) {
        let rec = {
            let mut g = lock(&self.inner);
            let Some(holds) = g.desktops.get(id).map(vd::Holds::of) else { return };
            let Some(r) = g.records.get_mut(id) else { return };
            if r.holds == holds {
                return;
            }
            r.holds = holds;
            r.clone()
        };
        if self.dir(id).is_dir() {
            self.write_record(&rec);
        }
    }

    // ── reads ────────────────────────────────────────────────────────

    pub fn state(&self, id: &str) -> State {
        lock(&self.inner).desktops.get(id).map_or(State::Closed, |d| d.state)
    }

    /// The desktops K2 knows (ever started, or asked for), with states.
    pub fn known(&self) -> Vec<(String, State)> {
        lock(&self.inner).desktops.values().map(|d| (d.id.clone(), d.state)).collect()
    }

    /// `DISPLAY` and `XAUTHORITY` for K2 Screen while the desktop is awake.
    pub fn screen_env(&self, id: &str) -> Option<Vec<(String, String)>> {
        let g = lock(&self.inner);
        if g.desktops.get(id).map(|d| d.state) != Some(State::Ready) {
            return None;
        }
        let display = g.records.get(id)?.display.clone()?;
        Some(vec![("DISPLAY".into(), display), ("XAUTHORITY".into(), self.cookie_path(id).display().to_string()), ("XDG_SESSION_TYPE".into(), "x11".into())])
    }

    /// The memory a desktop holds (its slice's `MemoryCurrent`).
    fn memory(&self, r: &Record) -> Option<u64> {
        if r.mode != RunMode::Systemd || r.state == State::Closed {
            return None;
        }
        let o = self.systemctl(&["show", "-p", "MemoryCurrent", "--value", &r.units.slice]).ok().filter(|o| o.status == 0)?;
        o.stdout.trim().parse().ok()
    }

    pub fn status(&self, id: &str) -> Status {
        let now = self.now();
        let limits = self.limits();
        let can_sleep = self.can_sleep();
        let (d, rec, waiting, used_desktops, used_awake) = {
            let g = lock(&self.inner);
            let d = g.desktops.get(id).cloned().unwrap_or_else(|| Desktop::new(id));
            let waiting = g.queue.iter().position(|w| w.id == id).map(|p| p + 1);
            let used_desktops = g.desktops.values().filter(|d| d.state.exists()).count() as u32;
            let used_awake = g.desktops.values().filter(|d| d.state.is_awake()).count() as u32;
            (d, g.records.get(id).cloned(), waiting, used_desktops, used_awake)
        };
        let memory_bytes = rec.as_ref().and_then(|r| self.memory(r));
        let missing = if d.state == State::Closed { self.missing() } else { Vec::new() };
        let manual = if missing.is_empty() { None } else { self.manual() };
        let state = if waiting.is_some() && d.state == State::Closed { State::Waiting } else { d.state };
        Status {
            id: id.to_string(),
            state,
            closed: d.closed.clone(),
            size: d.size,
            display: rec.as_ref().filter(|_| d.state != State::Closed).and_then(|r| r.display.clone()),
            memory_bytes,
            present: d.viewers.iter().filter(|(_, v)| v.present(now)).map(|(k, v)| (k.clone(), v.user.clone())).collect(),
            hold: d.hold_active(now).cloned(),
            next_sleep_at: d.next_sleep_at(now),
            next_close_at: d.next_close_at(),
            can_sleep,
            waiting,
            limits,
            used_desktops,
            used_awake,
            missing,
            manual,
            suggest_recycle: d.started_at.is_some_and(|t| d.state.exists() && now - t >= vd::SUGGEST_RECYCLE_AFTER),
            has_browser: BROWSERS.iter().any(|b| self.host.has_program(b)),
        }
    }
}

// ── files ────────────────────────────────────────────────────────────

fn make_private_dir(p: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p)?;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(p)
    }
}

/// Write `body` to `path` (0600), through a temp file and a rename.
fn write_private(path: &Path, body: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(body)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

// ── the real host ────────────────────────────────────────────────────

/// This machine.
#[derive(Default)]
pub struct RealHost {
    systemd_user: OnceLock<bool>,
    freezer: OnceLock<bool>,
}

/// Where K2 looks for programs (Ubuntu's snaps too, for a browser).
const SYSTEM_PATH: [&str; 4] = ["/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"];

/// A browser on the box (LD-20), by its usual program names.
pub const BROWSERS: [&str; 5] = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "firefox"];

/// Where X servers put their sockets.
pub const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// A folder every X server may make its socket in: there, a folder,
/// writable by all with the sticky bit (1777).
pub fn x11_dir_ok(p: &Path) -> Result<(), String> {
    let m = std::fs::metadata(p).map_err(|_| format!("{} doesn't exist", p.display()))?;
    if !m.is_dir() {
        return Err(format!("{} isn't a folder", p.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = m.permissions().mode() & 0o7777;
        if mode & 0o1777 != 0o1777 {
            return Err(format!("{} has mode {mode:o}, not 1777", p.display()));
        }
    }
    Ok(())
}

/// `systemctl --version`'s number (`systemd 255 (255.4-1ubuntu8)`).
pub fn systemd_version(out: &str) -> Option<u32> {
    out.lines().next()?.split_whitespace().nth(1)?.parse().ok()
}

/// `MemTotal` and `MemAvailable` (bytes) from `/proc/meminfo`.
pub fn parse_meminfo(s: &str) -> (u64, u64) {
    let get = |key: &str| {
        s.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok()).map_or(0, |kb| kb * 1024)
    };
    (get("MemTotal:"), get("MemAvailable:"))
}

impl Host for RealHost {
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
        let mut c = k2_core::bg_proc::command(program);
        c.args(args).stdin(std::process::Stdio::null());
        #[cfg(unix)]
        {
            for (k, v) in crate::publish_dev::user_manager_env() {
                if std::env::var_os(k).is_none() {
                    c.env(k, v);
                }
            }
        }
        let out = c.output()?;
        Ok(Output { status: out.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&out.stdout).into_owned(), stderr: String::from_utf8_lossy(&out.stderr).into_owned() })
    }

    fn spawn_detached(&self, argv: &[String]) -> io::Result<u32> {
        let (prog, rest) = argv.split_first().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
        let mut c = k2_core::bg_proc::command(prog);
        c.args(rest).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid in the child before exec; no allocation.
            unsafe {
                c.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = c.spawn()?;
        let pid = child.id();
        // Reap it whenever it ends (it is ours until then).
        let _ = std::thread::Builder::new().name("k2-vdesktop-reap".into()).spawn(move || {
            let _ = child.wait();
        });
        Ok(pid)
    }

    fn alive(&self, pid: u32) -> bool {
        #[cfg(unix)]
        {
            // A reaped child is gone; a zombie still answers kill 0, so
            // look at its state too.
            // SAFETY: signal 0 only checks.
            let exists = unsafe { libc::kill(pid as i32, 0) } == 0;
            exists && std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |s| !s.rsplit_once(')').is_some_and(|(_, r)| r.trim_start().starts_with('Z')))
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }

    fn kill_group(&self, pid: u32) {
        #[cfg(unix)]
        {
            // SAFETY: signalling a process group we started.
            unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
            let _ = std::thread::Builder::new().name("k2-vdesktop-kill".into()).spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                // SAFETY: as above.
                unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            });
        }
        #[cfg(not(unix))]
        let _ = pid;
    }

    fn facts(&self) -> BoxFacts {
        let (mem_total, mem_available) = std::fs::read_to_string("/proc/meminfo").map(|s| parse_meminfo(&s)).unwrap_or((0, 0));
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
        BoxFacts { mem_total, mem_available, cores }
    }

    fn boot_id(&self) -> String {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_string()).unwrap_or_default()
    }

    fn has_program(&self, name: &str) -> bool {
        SYSTEM_PATH.iter().any(|d| Path::new(d).join(name).is_file())
    }

    fn package_manager(&self) -> Option<&'static str> {
        [("apt-get", "apt"), ("pacman", "pacman"), ("dnf", "dnf")].into_iter().find(|(prog, _)| self.has_program(prog)).map(|(_, pm)| pm)
    }

    fn systemd_user(&self) -> bool {
        *self.systemd_user.get_or_init(|| {
            #[cfg(target_os = "linux")]
            {
                crate::publish_dev::user_scopes().is_ok()
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        })
    }

    fn x11_socket_dir(&self) -> Result<(), String> {
        x11_dir_ok(Path::new(X11_SOCKET_DIR))
    }

    fn display_taken(&self, n: u32) -> bool {
        display_taken_in(Path::new("/tmp"), n)
    }

    fn atspi_launcher(&self) -> Option<String> {
        ATSPI_LAUNCHERS.iter().find(|p| Path::new(p).is_file()).map(|p| p.to_string())
    }

    fn reclaim(&self, cgroup: &str, bytes: u64) -> Result<(), String> {
        use std::io::Write as _;
        let path = Path::new("/sys/fs/cgroup").join(cgroup.trim_start_matches('/')).join("memory.reclaim");
        reclaim_result(std::fs::OpenOptions::new().write(true).open(&path).and_then(|mut f| f.write_all(bytes.to_string().as_bytes())))
    }

    fn freezer(&self) -> bool {
        *self.freezer.get_or_init(|| {
            Path::new("/sys/fs/cgroup/cgroup.controllers").is_file()
                && self.run("systemctl", &s(&["--version"])).ok().filter(|o| o.status == 0).and_then(|o| systemd_version(&o.stdout)).is_some_and(|v| v >= 246)
        })
    }
}

// ── the one manager ──────────────────────────────────────────────────

fn slot() -> &'static RwLock<Option<Arc<Manager>>> {
    static S: OnceLock<RwLock<Option<Arc<Manager>>>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(None))
}

/// This daemon's manager (the real host, `~/.k2/desktop`).
pub fn manager() -> Arc<Manager> {
    if let Some(m) = slot().read().unwrap_or_else(|p| p.into_inner()).clone() {
        return m;
    }
    let mut w = slot().write().unwrap_or_else(|p| p.into_inner());
    w.get_or_insert_with(|| {
        let home = k2_core::paths::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Manager::new(Arc::new(RealHost::default()), Arc::new(SystemClock), Config::for_home(&home))
    })
    .clone()
}

/// Tests: put `m` in place until the guard drops.
pub fn set_for_test(m: Arc<Manager>) -> TestManager {
    let prev = std::mem::replace(&mut *slot().write().unwrap_or_else(|p| p.into_inner()), Some(m));
    TestManager { prev }
}

pub struct TestManager {
    prev: Option<Arc<Manager>>,
}

impl Drop for TestManager {
    fn drop(&mut self) {
        *slot().write().unwrap_or_else(|p| p.into_inner()) = self.prev.take();
    }
}

/// How often the rules run.
pub const TICK: Duration = Duration::from_secs(15);

/// Daemon boot (Linux): take back the desktops left running, then run the
/// rules every [`TICK`]. Never starts a desktop.
pub fn boot() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let _ = std::thread::Builder::new().name("k2-vdesktop".into()).spawn(|| {
        manager().adopt();
        loop {
            std::thread::sleep(TICK);
            manager().tick();
        }
    });
}

#[cfg(test)]
#[path = "virtual_desktop_tests.rs"]
pub(crate) mod tests;
