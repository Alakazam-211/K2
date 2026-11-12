//! Virtual desktops on a Linux server (`prd-linux-virtual-desktop-v1`
//! §3, §5): the ids, the limits, the start arguments and the awake /
//! asleep / closed rules. Pure: no processes, no clock of its own (every
//! rule takes `now` in Unix seconds), so it runs anywhere and is tested
//! with a fake clock. The daemon's `desktop::virtual_desktop` runs the
//! processes (Xvfb and the window manager as `systemd --user` services)
//! and calls [`step`] on a timer.
//!
//! Ids (LD-21, LD-33): [`VIRTUAL`] is "This server" on a Linux server
//! with no screen, `virtual:<workspace id>` is an agent's own desktop.
//! Everything is keyed by id: folder, cookie, units, display number,
//! limits.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The shared desktop for people on a server with no screen.
pub const VIRTUAL: &str = "virtual";
/// The prefix of an agent's desktop (`virtual:<workspace id>`).
pub const AGENT_PREFIX: &str = "virtual:";
/// The longest desktop id.
pub const MAX_ID_LEN: usize = 64;
/// What the tab and `k2 study desktop` call an agent's desktop until
/// agent isolation (0.49).
pub const SEPARATE_LABEL: &str = "Separate, not private";
/// The environment guard every desktop session gets (LD-10): `k2` there
/// refuses to read the owner token.
pub const NO_OWNER_FALLBACK: &str = "K2_NO_OWNER_FALLBACK";

/// Sizes (LD-11): the first viewer's pane, clamped; 1280×720 without one.
pub const MIN_SIZE: Size = Size { width: 1280, height: 720 };
pub const MAX_SIZE: Size = Size { width: 1920, height: 1080 };

/// Times, in seconds (§3.4, §3.9).
pub const SLEEP_AFTER: i64 = 10 * 60;
pub const PRESENT_WINDOW: i64 = 90;
pub const PRESENT_EVERY: i64 = 30;
pub const HOLD_FOR: i64 = 30 * 60;
pub const HOLD_CAP: i64 = 4 * 3600;
pub const AGENT_PAUSE: i64 = 15 * 60;
pub const CLOSE_ASLEEP_AFTER: i64 = 24 * 3600;
pub const SUGGEST_RECYCLE_AFTER: i64 = 7 * 24 * 3600;

/// Refuse a start with less memory available than this (§5).
pub const MEMORY_FLOOR: u64 = 1 << 30;
/// Per-desktop task cap (§5).
pub const TASKS_MAX: u32 = 512;
const GIB: u64 = 1 << 30;

// ── ids ──────────────────────────────────────────────────────────────

/// `[a-z0-9:_-]`, 1–64 characters.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b':' | b'_' | b'-'))
}

/// A virtual desktop's id: `virtual` or `virtual:<workspace>`.
pub fn is_virtual(id: &str) -> bool {
    id == VIRTUAL || workspace_of(id).is_some()
}

/// The workspace of an agent's desktop.
pub fn workspace_of(id: &str) -> Option<&str> {
    id.strip_prefix(AGENT_PREFIX).filter(|w| !w.is_empty() && !w.contains(':') && valid_id(id))
}

/// An agent's desktop id from its workspace's project id: `virtual:<id>`.
/// Project ids are lowercase UUIDs (36 characters), which always fit the
/// grammar. One that doesn't (an old or imported id with capitals or
/// other characters) gets a stable form instead: `virtual:x` and 24 hex
/// characters of the id's SHA-256, never a lossy rewrite that two ids
/// could share. `None` only for an empty id. The daemon maps an id back
/// to its workspace by computing this for each workspace.
pub fn agent_desktop(workspace: &str) -> Option<String> {
    if workspace.is_empty() {
        return None;
    }
    let id = format!("{AGENT_PREFIX}{workspace}");
    if workspace_of(&id).is_some() {
        return Some(id);
    }
    use sha2::{Digest, Sha256};
    let h: String = Sha256::digest(workspace.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect();
    Some(format!("{AGENT_PREFIX}x{h}"))
}

/// The folder name under `~/.k2/desktop/` (§3.1: `:` turned into `-`).
pub fn folder_slug(id: &str) -> String {
    id.replace(':', "-")
}

/// The id as it goes into a unit name: `-` is a slice separator in
/// systemd, so it is escaped as `systemd-escape` does (`\x2d`); `:` and
/// `_` are allowed as they are. One id, one name.
pub fn unit_slug(id: &str) -> String {
    id.replace('-', "\\x2d")
}

/// The parent slice holding every desktop's slice (the totals, §5).
pub const PARENT_SLICE: &str = "k2-desktops.slice";

/// One desktop's systemd names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Units {
    pub slice: String,
    pub x: String,
    pub session: String,
}

pub fn units(id: &str) -> Units {
    let s = unit_slug(id);
    // `k2-desktops-<s>.slice` sits under `k2-desktops.slice` by systemd's
    // dash rule; `<s>` holds no bare dash, so it nests no deeper.
    Units { slice: format!("k2-desktops-{s}.slice"), x: format!("k2-desktop-{s}-x.service"), session: format!("k2-desktop-{s}-session.service") }
}

// ── sizes ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub fn parse(s: &str) -> Option<Size> {
        let (w, h) = s.trim().split_once(['x', 'X'])?;
        Some(Size { width: w.trim().parse().ok()?, height: h.trim().parse().ok()? })
    }
}

impl std::fmt::Display for Size {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// The pane's size clamped to 1280×720 … 1920×1080, even numbers (LD-11).
pub fn clamp_size(asked: Option<Size>) -> Size {
    let Some(a) = asked else { return MIN_SIZE };
    let even = |v: u32| v & !1;
    Size { width: even(a.width.clamp(MIN_SIZE.width, MAX_SIZE.width)), height: even(a.height.clamp(MIN_SIZE.height, MAX_SIZE.height)) }
}

// ── limits ───────────────────────────────────────────────────────────

/// What the box has (`/proc/meminfo`, the CPU count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxFacts {
    pub mem_total: u64,
    pub mem_available: u64,
    pub cores: u32,
}

/// §3.2's table, computed from the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// Desktops that may exist (not closed): one per 2 GB, 2 to 16.
    pub max_desktops: u32,
    /// Awake at once: half the cores, 1 to 8.
    pub max_awake: u32,
    /// Memory for all desktops: a quarter of RAM, at least 2 GB.
    pub total_memory: u64,
    /// Memory for one: 2 GB, never more than the total.
    pub each_memory: u64,
    /// CPU for all: half the cores (at least one).
    pub cpu_cores: u32,
}

impl Limits {
    pub fn for_box(f: &BoxFacts) -> Limits {
        let gb = (f.mem_total / GIB) as u32;
        let total_memory = (f.mem_total / 4).max(2 * GIB);
        Limits {
            max_desktops: (gb / 2).clamp(2, 16),
            max_awake: (f.cores / 2).clamp(1, 8),
            total_memory,
            each_memory: (2 * GIB).min(total_memory),
            cpu_cores: (f.cores / 2).max(1),
        }
    }
}

/// Short of memory (§3.4): available under 10% of RAM or 512 MB,
/// whichever is larger.
pub fn low_memory(f: &BoxFacts) -> bool {
    f.mem_available < (f.mem_total / 10).max(512 << 20)
}

/// The Owner switch that covers a desktop: `virtual` for every virtual
/// desktop (one switch for the server, §4 item 2), else the id itself.
pub fn switch_id(id: &str) -> &str {
    if is_virtual(id) {
        VIRTUAL
    } else {
        id
    }
}

// ── start arguments ──────────────────────────────────────────────────

/// The first display number K2 tries, and how far up it looks (L0 11-11):
/// far above a session's :0–:9.
pub const DISPLAY_BASE: u32 = 100;
pub const DISPLAY_SPAN: u32 = 900;

/// The display K2 asks Xvfb for: the first from `from` that `taken` says
/// is free (no `/tmp/.X<n>-lock`, no `/tmp/.X11-unix/X<n>`). Never left
/// to Xvfb: with `-nolisten local` its own scan starts at :0 and can take
/// a live session's display, unlinking its socket (L0 11-11, z13flow).
pub fn pick_display(taken: impl Fn(u32) -> bool, from: u32) -> Option<u32> {
    (from.max(DISPLAY_BASE)..DISPLAY_BASE + DISPLAY_SPAN).find(|n| !taken(*n))
}

/// Xvfb's arguments after the program (§3.3): the display K2 picked,
/// explicitly. Xvfb writes that number to fd 3 once it is ready
/// (`-displayfd`, about 50 ms), which is how K2 knows it is up. Never
/// `-ac`; no TCP and no abstract socket; the desktop's own cookie file.
/// No GLX (L0 11-11): with the NVIDIA driver Xvfb crashes in GLX init, and
/// without GLX it holds 15 MB instead of 37–64.
pub fn xvfb_args(display: u32, size: Size, cookie: &str) -> Vec<String> {
    [
        &format!(":{display}"),
        "-displayfd",
        "3",
        "-screen",
        "0",
        &format!("{}x{}x24", size.width, size.height),
        "-nolisten",
        "tcp",
        "-nolisten",
        "local",
        "-extension",
        "GLX",
        "-auth",
        cookie,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The clean environment of everything on a desktop (§3.3, §4 item 3):
/// no K2 token, nothing of the daemon's, plus the owner-fallback guard.
pub fn session_env(home: &str, display: Option<&str>, cookie: &str) -> Vec<(String, String)> {
    let mut v = vec![
        ("PATH".to_string(), "/usr/local/bin:/usr/bin:/bin".to_string()),
        ("HOME".to_string(), home.to_string()),
        ("XAUTHORITY".to_string(), cookie.to_string()),
        ("XDG_SESSION_TYPE".to_string(), "x11".to_string()),
        (NO_OWNER_FALLBACK.to_string(), "1".to_string()),
    ];
    if let Some(d) = display {
        v.push(("DISPLAY".to_string(), d.to_string()));
    }
    v
}

/// `env -i K=V …`: the prefix that runs a program with only `vars`.
pub fn clean_env_prefix(vars: &[(String, String)]) -> Vec<String> {
    let mut v = vec!["/usr/bin/env".to_string(), "-i".to_string()];
    v.extend(vars.iter().map(|(k, val)| format!("{k}={val}")));
    v
}

/// An Xauthority file holding one MIT-MAGIC-COOKIE-1 for any display
/// (FamilyWild): Xvfb reads it with `-auth`, clients with `XAUTHORITY`.
/// The display number isn't known until Xvfb picks one, and FamilyWild
/// matches whatever it picks.
pub fn xauth_bytes(cookie: &[u8; 16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    let field = |out: &mut Vec<u8>, b: &[u8]| {
        out.extend_from_slice(&(b.len() as u16).to_be_bytes());
        out.extend_from_slice(b);
    };
    out.extend_from_slice(&0xFFFFu16.to_be_bytes());
    field(&mut out, b"");
    field(&mut out, b"");
    field(&mut out, b"MIT-MAGIC-COOKIE-1");
    field(&mut out, cookie);
    out
}

// ── states ───────────────────────────────────────────────────────────

/// A virtual desktop's lifecycle state (§3.4, and `waiting` from §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Closed,
    Waiting,
    Starting,
    Ready,
    Asleep,
    Waking,
}

impl State {
    pub fn as_wire(self) -> &'static str {
        match self {
            State::Closed => "closed",
            State::Waiting => "waiting",
            State::Starting => "starting",
            State::Ready => "ready",
            State::Asleep => "asleep",
            State::Waking => "waking",
        }
    }

    /// Takes an awake slot.
    pub fn is_awake(self) -> bool {
        matches!(self, State::Starting | State::Ready | State::Waking)
    }

    /// Counts against the "may exist" limit.
    pub fn exists(self) -> bool {
        !matches!(self, State::Closed | State::Waiting)
    }

    /// The tab's sentence (§6). `reason` covers `closed`.
    pub fn message(self, awake_limit: u32) -> String {
        match self {
            State::Closed => "This server has no screen. K2 can start a desktop on it.".into(),
            State::Waiting => format!("Waiting for another desktop to go to sleep. This server runs {awake_limit} at a time."),
            State::Starting => "Starting the desktop…".into(),
            State::Ready => "Ready.".into(),
            State::Asleep => "The desktop is asleep. Everything on it is kept.".into(),
            State::Waking => "Waking the desktop…".into(),
        }
    }
}

/// Why K2 closed a desktop (§3.4's table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CloseReason {
    AsleepTooLong,
    LowMemory,
    Restarted,
    ClosedBy { by: String },
    SwitchedOff,
    Crashed { detail: String },
    /// No freezer here: closed after 10 minutes with nobody viewing.
    NobodyViewing,
}

impl CloseReason {
    pub fn code(&self) -> &'static str {
        match self {
            CloseReason::AsleepTooLong => "asleep_too_long",
            CloseReason::LowMemory => "low_memory",
            CloseReason::Restarted => "restarted",
            CloseReason::ClosedBy { .. } => "closed_by",
            CloseReason::SwitchedOff => "switched_off",
            CloseReason::Crashed { .. } => "crashed",
            CloseReason::NobodyViewing => "nobody_viewing",
        }
    }

    pub fn message(&self) -> String {
        match self {
            CloseReason::AsleepTooLong => "Your desktop was closed after a day asleep. Start a new one?".into(),
            CloseReason::LowMemory => "Your desktop was closed because this server was low on memory.".into(),
            CloseReason::Restarted => "Your desktop was closed when this server restarted.".into(),
            CloseReason::ClosedBy { by } => format!("The desktop was closed by {by}."),
            CloseReason::SwitchedOff => "The desktop was closed when Remote desktop was turned off.".into(),
            CloseReason::Crashed { detail } if detail.is_empty() => "The desktop stopped unexpectedly.".into(),
            CloseReason::Crashed { detail } => format!("The desktop stopped unexpectedly: {detail}"),
            CloseReason::NobodyViewing => format!("{} The desktop was closed after 10 minutes with nobody viewing it.", NO_FREEZER),
        }
    }
}

/// What the Set up card says before the Owner's yes (L0 11-11).
pub const SETUP_CARD: &str = "Installs Xvfb, IceWM and a font from your distro: about 300 MB on a server that has no graphics libraries yet, about 70 MB on one that has.";

/// Said by status and the Start card where sleep isn't possible (LD-28).
pub const NO_FREEZER: &str = "This server can't pause a desktop, so it closes when nobody is viewing it.";

/// A closed desktop's reason with when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closed {
    #[serde(flatten)]
    pub reason: CloseReason,
    pub at: i64,
}

// ── who wants it ─────────────────────────────────────────────────────

/// Who asked for a start or a wake: a person goes ahead of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Asker {
    Person,
    Agent,
}

/// One viewer of a desktop (LD-24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub user: String,
    /// An Owner or Admin: their presence wakes an asleep desktop.
    pub may_wake: bool,
    pub hidden: bool,
    pub last_present: i64,
}

impl Viewer {
    pub fn present(&self, now: i64) -> bool {
        !self.hidden && now - self.last_present <= PRESENT_WINDOW
    }
}

/// An agent's hold (§3.9): one session at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub by: String,
    pub until: i64,
}

/// One desktop's lifecycle bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desktop {
    pub id: String,
    pub state: State,
    pub closed: Option<Closed>,
    pub size: Option<Size>,
    /// When it last started (awake, after a close).
    pub started_at: Option<i64>,
    pub asleep_since: Option<i64>,
    /// The last moment something kept it awake (a present viewer, a hold,
    /// a start or a wake): it sleeps [`SLEEP_AFTER`] later.
    pub kept_at: i64,
    pub hold: Option<Hold>,
    /// Holds alone have kept it awake since (reset by a present person).
    pub held_alone_since: Option<i64>,
    /// After the hold cap: agents can't wake it before this.
    pub agent_paused_until: Option<i64>,
    pub viewers: BTreeMap<String, Viewer>,
}

impl Desktop {
    pub fn new(id: &str) -> Desktop {
        Desktop {
            id: id.to_string(),
            state: State::Closed,
            closed: None,
            size: None,
            started_at: None,
            asleep_since: None,
            kept_at: 0,
            hold: None,
            held_alone_since: None,
            agent_paused_until: None,
            viewers: BTreeMap::new(),
        }
    }

    pub fn any_present(&self, now: i64) -> bool {
        self.viewers.values().any(|v| v.present(now))
    }

    fn present_waker(&self, now: i64) -> bool {
        self.viewers.values().any(|v| v.may_wake && v.present(now))
    }

    pub fn hold_active(&self, now: i64) -> Option<&Hold> {
        self.hold.as_ref().filter(|h| h.until > now)
    }

    /// A viewer's socket opened (counts as present now).
    pub fn viewer_joined(&mut self, viewer: &str, user: &str, may_wake: bool, now: i64) {
        self.viewers.insert(viewer.to_string(), Viewer { user: user.to_string(), may_wake, hidden: false, last_present: now });
    }

    /// The viewer's `present` heartbeat.
    pub fn viewer_present(&mut self, viewer: &str, now: i64) {
        if let Some(v) = self.viewers.get_mut(viewer) {
            v.hidden = false;
            v.last_present = now;
        }
    }

    /// `fps 0` (hidden) or a rate again (shown).
    pub fn viewer_hidden(&mut self, viewer: &str, hidden: bool, now: i64) {
        if let Some(v) = self.viewers.get_mut(viewer) {
            v.hidden = hidden;
            if !hidden {
                v.last_present = now;
            }
        }
    }

    pub fn viewer_left(&mut self, viewer: &str) {
        self.viewers.remove(viewer);
    }

    /// Take or renew the hold for `by` (LD-29): refused while another
    /// session holds it, with who and until when.
    pub fn take_hold(&mut self, by: &str, now: i64) -> Result<Hold, Hold> {
        if let Some(h) = self.hold_active(now) {
            if h.by != by {
                return Err(h.clone());
            }
        }
        let h = Hold { by: by.to_string(), until: now + HOLD_FOR };
        self.hold = Some(h.clone());
        Ok(h)
    }

    /// `k2 desktop env --release`: drop `by`'s hold (another's stays).
    pub fn release_hold(&mut self, by: &str) -> bool {
        if self.hold.as_ref().is_some_and(|h| h.by == by) {
            self.hold = None;
            return true;
        }
        false
    }

    /// May an agent wake or start it now (the 15-minute pause)?
    pub fn agent_may_wake(&self, now: i64) -> bool {
        self.agent_paused_until.is_none_or(|t| now >= t)
    }

    /// Bookkeeping when it becomes awake (a start or a wake).
    pub fn mark_awake(&mut self, now: i64) {
        self.state = State::Ready;
        self.closed = None;
        self.asleep_since = None;
        self.kept_at = now;
    }

    pub fn mark_asleep(&mut self, now: i64) {
        self.state = State::Asleep;
        self.asleep_since = Some(now);
        self.held_alone_since = None;
    }

    pub fn mark_closed(&mut self, reason: Option<CloseReason>, now: i64) {
        self.state = State::Closed;
        self.closed = reason.map(|reason| Closed { reason, at: now });
        self.size = None;
        self.started_at = None;
        self.asleep_since = None;
        self.held_alone_since = None;
        self.hold = None;
    }

    /// When it will sleep on its own, if nothing changes.
    pub fn next_sleep_at(&self, now: i64) -> Option<i64> {
        if self.state != State::Ready || self.any_present(now) {
            return None;
        }
        if let Some(h) = self.hold_active(now) {
            let cap = self.held_alone_since.unwrap_or(now) + HOLD_CAP;
            return Some(cap.min(h.until + SLEEP_AFTER));
        }
        Some(self.kept_at + SLEEP_AFTER)
    }

    /// When an asleep desktop will be closed (24 h).
    pub fn next_close_at(&self) -> Option<i64> {
        (self.state == State::Asleep).then_some(self.asleep_since? + CLOSE_ASLEEP_AFTER)
    }
}

/// What [`step`] asks the daemon to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Release keys, close K2 Screen's link, freeze.
    Sleep(String),
    /// Thaw and reconnect.
    Wake(String),
    Close(String, CloseReason),
    /// A queued request whose slot came free: start or wake it.
    Proceed(String),
}

/// A request waiting for an awake slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiter {
    pub id: String,
    pub asker: Asker,
    pub at: i64,
}

/// The rules on one tick. Updates each desktop's bookkeeping (presence
/// keeps it awake, the hold cap) and returns what to do; the daemon does
/// it and sets the states. `can_sleep` is false where there's no freezer
/// (LD-28): it closes instead.
pub fn step(desktops: &mut BTreeMap<String, Desktop>, queue: &mut Vec<Waiter>, limits: &Limits, facts: &BoxFacts, can_sleep: bool, now: i64) -> Vec<Action> {
    let mut out = Vec::new();
    for d in desktops.values_mut() {
        match d.state {
            State::Ready => {
                if d.any_present(now) {
                    d.kept_at = now;
                    d.held_alone_since = None;
                    d.agent_paused_until = None;
                    continue;
                }
                if d.hold_active(now).is_some() {
                    let since = *d.held_alone_since.get_or_insert(now);
                    if now - since >= HOLD_CAP {
                        d.agent_paused_until = Some(now + AGENT_PAUSE);
                        d.hold = None;
                        out.push(if can_sleep { Action::Sleep(d.id.clone()) } else { Action::Close(d.id.clone(), CloseReason::NobodyViewing) });
                    } else {
                        d.kept_at = now;
                    }
                    continue;
                }
                d.held_alone_since = None;
                if now - d.kept_at >= SLEEP_AFTER {
                    out.push(if can_sleep { Action::Sleep(d.id.clone()) } else { Action::Close(d.id.clone(), CloseReason::NobodyViewing) });
                }
            }
            State::Asleep => {
                if d.present_waker(now) {
                    d.agent_paused_until = None;
                    if !queue.iter().any(|w| w.id == d.id) {
                        queue.push(Waiter { id: d.id.clone(), asker: Asker::Person, at: now });
                    }
                } else if d.asleep_since.is_some_and(|t| now - t >= CLOSE_ASLEEP_AFTER) {
                    out.push(Action::Close(d.id.clone(), CloseReason::AsleepTooLong));
                }
            }
            _ => {}
        }
    }
    // Short of memory: the longest-asleep desktop closes (one a tick; the
    // next tick reads memory again). Awake desktops never close for this.
    if low_memory(facts) {
        let closing: Vec<&str> = out.iter().filter_map(|a| if let Action::Close(id, _) = a { Some(id.as_str()) } else { None }).collect();
        let oldest = desktops
            .values()
            .filter(|d| d.state == State::Asleep && !closing.contains(&d.id.as_str()))
            .min_by_key(|d| (d.asleep_since.unwrap_or(i64::MAX), d.id.clone()))
            .map(|d| d.id.clone());
        if let Some(id) = oldest {
            queue.retain(|w| w.id != id);
            out.push(Action::Close(id, CloseReason::LowMemory));
        }
    }
    out.extend(drain_queue(desktops, queue, limits, &out));
    out
}

/// Awake desktops now, less those this tick puts to sleep or closes.
fn awake_after(desktops: &BTreeMap<String, Desktop>, pending: &[Action]) -> u32 {
    desktops
        .values()
        .filter(|d| d.state.is_awake())
        .filter(|d| !pending.iter().any(|a| matches!(a, Action::Sleep(id) | Action::Close(id, _) if *id == d.id)))
        .count() as u32
}

/// Waiters that may go now: people first, then agents, each in order.
fn drain_queue(desktops: &BTreeMap<String, Desktop>, queue: &mut Vec<Waiter>, limits: &Limits, pending: &[Action]) -> Vec<Action> {
    queue.retain(|w| desktops.get(&w.id).is_some_and(|d| matches!(d.state, State::Closed | State::Waiting | State::Asleep)));
    queue.sort_by_key(|w| (w.asker, w.at));
    let free = limits.max_awake.saturating_sub(awake_after(desktops, pending)) as usize;
    let go: Vec<Waiter> = queue.drain(..free.min(queue.len())).collect();
    go.into_iter().map(|w| Action::Proceed(w.id)).collect()
}

/// What a start (or wake) request should do now (LD-32: safe to repeat).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Already awake (or on its way): return it as it is.
    AsIs,
    Start,
    Wake,
    /// No awake slot: queued, with its place.
    Wait { position: usize },
    /// Past the "may exist" limit: these could be closed.
    TooMany { closable: Vec<String> },
    /// Under the memory floor.
    LowMemory,
    /// An agent within its 15-minute pause.
    AgentPaused { until: i64 },
}

/// Plan a start of `id` (which may be new: then it's `Closed`).
pub fn plan_start(desktops: &BTreeMap<String, Desktop>, queue: &mut Vec<Waiter>, id: &str, asker: Asker, limits: &Limits, facts: &BoxFacts, now: i64) -> Plan {
    let state = desktops.get(id).map_or(State::Closed, |d| d.state);
    if state.is_awake() {
        return Plan::AsIs;
    }
    if asker == Asker::Agent {
        if let Some(t) = desktops.get(id).and_then(|d| d.agent_paused_until).filter(|t| now < *t) {
            return Plan::AgentPaused { until: t };
        }
    }
    if state == State::Closed || state == State::Waiting {
        let existing: Vec<&Desktop> = desktops.values().filter(|d| d.state.exists() && d.id != id).collect();
        if existing.len() as u32 >= limits.max_desktops {
            let mut closable: Vec<&Desktop> = existing;
            closable.sort_by_key(|d| (d.state != State::Asleep, d.asleep_since.unwrap_or(i64::MAX), d.id.clone()));
            return Plan::TooMany { closable: closable.iter().map(|d| d.id.clone()).collect() };
        }
        if facts.mem_available < MEMORY_FLOOR {
            return Plan::LowMemory;
        }
    }
    let awake = desktops.values().filter(|d| d.state.is_awake()).count() as u32;
    let queued_ahead = |q: &Vec<Waiter>| q.iter().filter(|w| w.id != id && (w.asker < asker || w.asker == asker)).count();
    if awake >= limits.max_awake || queued_ahead(queue) > 0 && awake + queued_ahead(queue) as u32 >= limits.max_awake {
        if let Some(w) = queue.iter_mut().find(|w| w.id == id) {
            // A person asking again moves an agent's request up.
            if asker < w.asker {
                w.asker = asker;
            }
        } else {
            queue.push(Waiter { id: id.to_string(), asker, at: now });
        }
        queue.sort_by_key(|w| (w.asker, w.at));
        let position = queue.iter().position(|w| w.id == id).map_or(1, |p| p + 1);
        return Plan::Wait { position };
    }
    queue.retain(|w| w.id != id);
    if state == State::Asleep {
        Plan::Wake
    } else {
        Plan::Start
    }
}

// ── the state file ───────────────────────────────────────────────────

/// How a desktop's processes run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// `systemd --user` services in the desktop's slice (limits, sleep).
    Systemd,
    /// No user manager: detached children, no limits, no sleep (LD-28).
    Detached,
}

/// `~/.k2/desktop/<slug>/state.json`: what a restarted daemon adopts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub mode: RunMode,
    pub state: State,
    #[serde(default)]
    pub closed: Option<Closed>,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub size: Option<Size>,
    pub units: Units,
    #[serde(default)]
    pub pids: Vec<u32>,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub asleep_since: Option<i64>,
    /// `/proc/sys/kernel/random/boot_id` at start: another means a reboot.
    #[serde(default)]
    pub boot_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn facts(gb: u64, cores: u32) -> BoxFacts {
        BoxFacts { mem_total: gb * GB, mem_available: gb * GB / 2, cores }
    }

    fn limits(max_desktops: u32, max_awake: u32) -> Limits {
        Limits { max_desktops, max_awake, total_memory: 2 * GB, each_memory: 2 * GB, cpu_cores: 1 }
    }

    fn ready(id: &str, now: i64) -> Desktop {
        let mut d = Desktop::new(id);
        d.mark_awake(now);
        d.size = Some(MIN_SIZE);
        d.started_at = Some(now);
        d
    }

    fn map(ds: Vec<Desktop>) -> BTreeMap<String, Desktop> {
        ds.into_iter().map(|d| (d.id.clone(), d)).collect()
    }

    fn tick(ds: &mut BTreeMap<String, Desktop>, q: &mut Vec<Waiter>, now: i64) -> Vec<Action> {
        step(ds, q, &limits(16, 8), &facts(64, 16), true, now)
    }

    #[test]
    fn ids_follow_the_grammar() {
        assert!(valid_id("virtual") && valid_id("console") && valid_id("virtual:3f2a-11"));
        assert!(!valid_id("") && !valid_id("Virtual") && !valid_id("virtual:a b") && !valid_id(&"a".repeat(65)));
        assert!(is_virtual("virtual") && is_virtual("virtual:abc") && !is_virtual("console") && !is_virtual("virtual:") && !is_virtual("virtual:a:b"));
        assert_eq!(workspace_of("virtual:1a2b"), Some("1a2b"));
        assert_eq!(agent_desktop("9c1e-44"), Some("virtual:9c1e-44".into()));
        // Every project id K2 mints today fits as it is.
        let uuid = uuid::Uuid::new_v4().to_string();
        assert_eq!(agent_desktop(&uuid), Some(format!("virtual:{uuid}")));
        assert!(valid_id(&agent_desktop(&uuid).unwrap()));
        // One that doesn't gets a stable, valid form of its own.
        let odd = agent_desktop("Bad Id").unwrap();
        assert!(valid_id(&odd) && odd.starts_with("virtual:x") && odd.len() == "virtual:x".len() + 24, "{odd}");
        assert_eq!(agent_desktop("Bad Id"), Some(odd.clone()), "stable");
        assert_ne!(agent_desktop("bad id"), Some(odd), "no two ids share one");
        assert_eq!(agent_desktop(""), None);
        assert_eq!(folder_slug("virtual:ab-1"), "virtual-ab-1");
    }

    #[test]
    fn two_ids_get_their_own_units_and_no_slice_nests_in_another() {
        let a = units("virtual");
        let b = units("virtual:1a-2b");
        assert_eq!(a.slice, "k2-desktops-virtual.slice");
        assert_eq!(b.slice, "k2-desktops-virtual:1a\\x2d2b.slice");
        assert_ne!(a.x, b.x);
        assert_ne!(a.session, b.session);
        // systemd nests slices on '-': the agent's slice must not sit under
        // the shared one (its limits would bound the agent's).
        let parent = |s: &str| s.trim_end_matches(".slice").rsplit_once('-').map(|(p, _)| p.to_string());
        assert_eq!(parent(&a.slice).as_deref(), Some("k2-desktops"));
        assert_eq!(parent(&b.slice).as_deref(), Some("k2-desktops"));
        assert_ne!(units("virtual:a-b").slice, units("virtual:a_b").slice);
    }

    #[test]
    fn sizes_are_clamped_and_even() {
        assert_eq!(clamp_size(None), Size { width: 1280, height: 720 });
        assert_eq!(clamp_size(Some(Size { width: 800, height: 600 })), Size { width: 1280, height: 720 });
        assert_eq!(clamp_size(Some(Size { width: 1601, height: 901 })), Size { width: 1600, height: 900 });
        assert_eq!(clamp_size(Some(Size { width: 3840, height: 2160 })), Size { width: 1920, height: 1080 });
        assert_eq!(Size::parse("1600x900"), Some(Size { width: 1600, height: 900 }));
        assert_eq!(Size::parse("big"), None);
    }

    #[test]
    fn limits_for_4_8_and_64_gb_boxes_are_as_the_table_says() {
        let small = Limits::for_box(&facts(4, 2));
        assert_eq!((small.max_desktops, small.max_awake, small.total_memory, small.each_memory, small.cpu_cores), (2, 1, 2 * GB, 2 * GB, 1));
        let mid = Limits::for_box(&facts(8, 4));
        assert_eq!((mid.max_desktops, mid.max_awake, mid.total_memory, mid.each_memory, mid.cpu_cores), (4, 2, 2 * GB, 2 * GB, 2));
        let big = Limits::for_box(&facts(64, 16));
        assert_eq!((big.max_desktops, big.max_awake, big.total_memory, big.each_memory, big.cpu_cores), (16, 8, 16 * GB, 2 * GB, 8));
    }

    #[test]
    fn xvfb_never_listens_beyond_its_cookie() {
        let a = xvfb_args(104, Size { width: 1600, height: 900 }, "/h/.k2/desktop/virtual/Xauthority");
        assert_eq!(a[0], ":104", "the display K2 picked, never Xvfb's own scan");
        assert!(!a.iter().any(|x| x == "-ac"), "never -ac");
        let pairs: Vec<String> = a.windows(2).map(|w| format!("{} {}", w[0], w[1])).collect();
        for want in ["-nolisten tcp", "-nolisten local", "-displayfd 3", "-extension GLX", "-auth /h/.k2/desktop/virtual/Xauthority"] {
            assert!(pairs.iter().any(|p| p == want), "{want} in {a:?}");
        }
        assert!(a.iter().any(|x| x == "1600x900x24"));
    }

    #[test]
    fn the_display_is_picked_from_100_and_never_a_taken_one() {
        assert_eq!(pick_display(|_| false, 0), Some(100), "never :0, even when it looks free");
        let taken = [0u32, 100, 101, 103];
        assert_eq!(pick_display(|n| taken.contains(&n), 0), Some(102));
        assert_eq!(pick_display(|n| taken.contains(&n), 103), Some(104), "a retry goes on from the next one");
        assert_eq!(pick_display(|_| true, 0), None, "all taken: none");
    }

    #[test]
    fn the_session_env_is_clean_and_carries_the_guard() {
        let env = session_env("/home/k2", Some(":3"), "/c");
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(env.contains(&(NO_OWNER_FALLBACK.to_string(), "1".to_string())));
        assert!(keys.iter().filter(|k| k.starts_with("K2_")).all(|k| *k == NO_OWNER_FALLBACK), "{keys:?}");
        let pre = clean_env_prefix(&env);
        assert_eq!(&pre[..2], &["/usr/bin/env".to_string(), "-i".to_string()]);
        assert!(pre.contains(&"DISPLAY=:3".to_string()));
    }

    #[test]
    fn the_xauthority_file_is_one_wild_cookie() {
        let b = xauth_bytes(&[7u8; 16]);
        assert_eq!(&b[..2], &[0xFF, 0xFF]);
        assert_eq!(&b[2..6], &[0, 0, 0, 0], "no address, no display number");
        assert_eq!(&b[6..8], &[0, 18]);
        assert_eq!(&b[8..26], b"MIT-MAGIC-COOKIE-1");
        assert_eq!(&b[26..28], &[0, 16]);
        assert_eq!(&b[28..], &[7u8; 16]);
    }

    #[test]
    fn no_sleep_while_a_viewer_is_present_then_sleep_ten_minutes_after() {
        let mut ds = map(vec![ready("virtual", 0)]);
        let mut q = Vec::new();
        ds.get_mut("virtual").unwrap().viewer_joined("dv1", "rosson", true, 0);
        // Present every 30 s for an hour: never sleeps.
        let mut t = 0;
        while t <= 3600 {
            ds.get_mut("virtual").unwrap().viewer_present("dv1", t);
            assert!(tick(&mut ds, &mut q, t).is_empty(), "at {t}");
            t += 30;
        }
        ds.get_mut("virtual").unwrap().viewer_left("dv1");
        assert!(tick(&mut ds, &mut q, 3600 + SLEEP_AFTER - 1).is_empty());
        assert_eq!(ds["virtual"].next_sleep_at(3600 + 1), Some(3600 + SLEEP_AFTER));
        assert_eq!(tick(&mut ds, &mut q, 3600 + SLEEP_AFTER), vec![Action::Sleep("virtual".into())]);
    }

    #[test]
    fn a_hidden_viewer_does_not_count() {
        let mut ds = map(vec![ready("virtual", 0)]);
        let mut q = Vec::new();
        let d = ds.get_mut("virtual").unwrap();
        d.viewer_joined("dv1", "rosson", true, 0);
        d.viewer_hidden("dv1", true, 0);
        assert_eq!(tick(&mut ds, &mut q, SLEEP_AFTER), vec![Action::Sleep("virtual".into())]);
    }

    #[test]
    fn a_viewer_with_no_present_for_90_s_does_not_count() {
        let mut ds = map(vec![ready("virtual", 0)]);
        let mut q = Vec::new();
        ds.get_mut("virtual").unwrap().viewer_joined("dv1", "rosson", true, 0);
        assert!(tick(&mut ds, &mut q, 90).is_empty(), "90 s: still present");
        // Silent since 0 (a hung client): it stopped counting at 91 s, so
        // the 10 minutes run from the last tick that saw it present.
        assert!(tick(&mut ds, &mut q, 91 + SLEEP_AFTER - 2).is_empty());
        assert_eq!(tick(&mut ds, &mut q, 90 + SLEEP_AFTER), vec![Action::Sleep("virtual".into())]);
    }

    #[test]
    fn an_agents_window_does_not_count_but_its_hold_does_until_4_h() {
        // An agent's window isn't a viewer: with no viewer and no hold the
        // desktop sleeps on time, whatever is drawn on it.
        let mut ds = map(vec![ready("virtual:w1", 0)]);
        let mut q = Vec::new();
        assert_eq!(tick(&mut ds, &mut q, SLEEP_AFTER), vec![Action::Sleep("virtual:w1".into())]);

        let mut ds = map(vec![ready("virtual:w1", 0)]);
        let mut t = 0;
        ds.get_mut("virtual:w1").unwrap().take_hold("k2/w1", t).expect("hold");
        while t < HOLD_CAP {
            if t % 1200 == 0 {
                ds.get_mut("virtual:w1").unwrap().take_hold("k2/w1", t).expect("renew");
            }
            assert!(tick(&mut ds, &mut q, t).is_empty(), "held at {t}");
            t += 60;
        }
        assert_eq!(tick(&mut ds, &mut q, HOLD_CAP), vec![Action::Sleep("virtual:w1".into())]);
        let d = &ds["virtual:w1"];
        assert!(!d.agent_may_wake(HOLD_CAP + AGENT_PAUSE - 1), "the agent can't wake it for 15 min");
        assert!(d.agent_may_wake(HOLD_CAP + AGENT_PAUSE));
    }

    #[test]
    fn a_person_present_resets_the_hold_cap() {
        let mut ds = map(vec![ready("virtual:w1", 0)]);
        let mut q = Vec::new();
        let held = |ds: &mut BTreeMap<String, Desktop>, q: &mut Vec<Waiter>, from: i64, to: i64| {
            let mut t = from;
            while t <= to {
                ds.get_mut("virtual:w1").unwrap().take_hold("k2/w1", t).expect("renew");
                assert!(tick(ds, q, t).is_empty(), "held at {t}");
                t += 600;
            }
        };
        held(&mut ds, &mut q, 0, 3 * 3600);
        ds.get_mut("virtual:w1").unwrap().viewer_joined("dv1", "rosson", true, 3 * 3600 + 60);
        assert!(tick(&mut ds, &mut q, 3 * 3600 + 60).is_empty());
        ds.get_mut("virtual:w1").unwrap().viewer_left("dv1");
        // The hold alone again, counted from here: past the old 4 h mark
        // it is still awake.
        held(&mut ds, &mut q, 3 * 3600 + 120, 5 * 3600);
    }

    #[test]
    fn one_holder_at_a_time() {
        let mut d = ready("virtual:w1", 0);
        let h = d.take_hold("k2/w1", 0).unwrap();
        assert_eq!(h.until, HOLD_FOR);
        let other = d.take_hold("k2/w1-sidecar", 10).unwrap_err();
        assert_eq!(other, Hold { by: "k2/w1".into(), until: HOLD_FOR });
        assert!(!d.release_hold("k2/w1-sidecar"));
        assert!(d.release_hold("k2/w1"));
        assert!(d.take_hold("k2/w1-sidecar", 20).is_ok());
        // An expired hold doesn't block.
        assert!(d.take_hold("k2/w1", 20 + HOLD_FOR).is_ok());
    }

    #[test]
    fn closed_after_24_h_asleep() {
        let mut d = ready("virtual", 0);
        d.mark_asleep(100);
        let mut ds = map(vec![d]);
        let mut q = Vec::new();
        assert_eq!(ds["virtual"].next_close_at(), Some(100 + CLOSE_ASLEEP_AFTER));
        assert!(tick(&mut ds, &mut q, 100 + CLOSE_ASLEEP_AFTER - 1).is_empty());
        assert_eq!(tick(&mut ds, &mut q, 100 + CLOSE_ASLEEP_AFTER), vec![Action::Close("virtual".into(), CloseReason::AsleepTooLong)]);
    }

    #[test]
    fn under_memory_pressure_the_longest_asleep_closes_first_and_awake_never() {
        let mut a = ready("virtual:a", 0);
        a.mark_asleep(500);
        let mut b = ready("virtual:b", 0);
        b.mark_asleep(200);
        let awake = ready("virtual", 0);
        let mut ds = map(vec![a, b, awake]);
        ds.get_mut("virtual").unwrap().viewer_joined("dv", "r", true, 600);
        let mut q = Vec::new();
        let tight = BoxFacts { mem_total: 8 * GB, mem_available: 100 << 20, cores: 4 };
        let acts = step(&mut ds, &mut q, &limits(4, 2), &tight, true, 600);
        assert_eq!(acts, vec![Action::Close("virtual:b".into(), CloseReason::LowMemory)]);
        ds.get_mut("virtual:b").unwrap().mark_closed(Some(CloseReason::LowMemory), 600);
        let acts = step(&mut ds, &mut q, &limits(4, 2), &tight, true, 601);
        assert_eq!(acts, vec![Action::Close("virtual:a".into(), CloseReason::LowMemory)]);
        ds.get_mut("virtual:a").unwrap().mark_closed(Some(CloseReason::LowMemory), 601);
        assert!(step(&mut ds, &mut q, &limits(4, 2), &tight, true, 602).is_empty(), "the awake one stays");
        assert!(low_memory(&BoxFacts { mem_total: 8 * GB, mem_available: 800 << 20, cores: 4 }), "10% of 8 GB is above 512 MB");
        assert!(!low_memory(&BoxFacts { mem_total: 2 * GB, mem_available: 600 << 20, cores: 4 }));
    }

    #[test]
    fn without_a_freezer_it_closes_instead_of_sleeping() {
        let mut ds = map(vec![ready("virtual", 0)]);
        let mut q = Vec::new();
        let acts = step(&mut ds, &mut q, &limits(4, 2), &facts(8, 4), false, SLEEP_AFTER);
        assert_eq!(acts, vec![Action::Close("virtual".into(), CloseReason::NobodyViewing)]);
        assert!(CloseReason::NobodyViewing.message().contains("can't pause a desktop"));
    }

    #[test]
    fn a_desktop_past_the_exist_limit_is_refused_with_what_could_close() {
        let mut a = ready("virtual:a", 0);
        a.mark_asleep(10);
        let ds = map(vec![a, ready("virtual:b", 0)]);
        let mut q = Vec::new();
        let p = plan_start(&ds, &mut q, "virtual:c", Asker::Person, &limits(2, 2), &facts(8, 4), 20);
        assert_eq!(p, Plan::TooMany { closable: vec!["virtual:a".into(), "virtual:b".into()] });
        // A start of one that exists is not a new desktop.
        assert_eq!(plan_start(&ds, &mut q, "virtual:b", Asker::Person, &limits(2, 2), &facts(8, 4), 20), Plan::AsIs);
        assert_eq!(plan_start(&ds, &mut q, "virtual:a", Asker::Person, &limits(2, 2), &facts(8, 4), 20), Plan::Wake);
    }

    #[test]
    fn a_start_under_the_memory_floor_is_refused() {
        let ds = BTreeMap::new();
        let mut q = Vec::new();
        let f = BoxFacts { mem_total: 8 * GB, mem_available: MEMORY_FLOOR - 1, cores: 4 };
        assert_eq!(plan_start(&ds, &mut q, "virtual", Asker::Person, &limits(4, 2), &f, 0), Plan::LowMemory);
    }

    #[test]
    fn past_the_awake_limit_a_start_waits_and_a_person_goes_before_an_agent() {
        let mut ds = map(vec![ready("virtual:a", 0)]);
        let mut q = Vec::new();
        let l = limits(4, 1);
        let f = facts(8, 4);
        assert_eq!(plan_start(&ds, &mut q, "virtual:agent", Asker::Agent, &l, &f, 10), Plan::Wait { position: 1 });
        ds.insert("virtual:agent".into(), Desktop { state: State::Waiting, ..Desktop::new("virtual:agent") });
        assert_eq!(plan_start(&ds, &mut q, "virtual", Asker::Person, &l, &f, 20), Plan::Wait { position: 1 }, "the person goes first");
        ds.insert("virtual".into(), Desktop { state: State::Waiting, ..Desktop::new("virtual") });
        // Asked again: still waiting, same place (safe to repeat).
        assert_eq!(plan_start(&ds, &mut q, "virtual", Asker::Person, &l, &f, 25), Plan::Wait { position: 1 });
        assert_eq!(q.len(), 2);
        // Nothing frees: nobody goes.
        ds.get_mut("virtual:a").unwrap().viewer_joined("dv", "r", true, 30);
        assert_eq!(step(&mut ds, &mut q, &l, &f, true, 30), vec![]);
        ds.get_mut("virtual:a").unwrap().viewer_left("dv");
        // a sleeps 10 min later; its slot goes to the person.
        let acts = step(&mut ds, &mut q, &l, &f, true, 30 + SLEEP_AFTER);
        assert_eq!(acts, vec![Action::Sleep("virtual:a".into()), Action::Proceed("virtual".into())]);
        assert_eq!(q, vec![Waiter { id: "virtual:agent".into(), asker: Asker::Agent, at: 10 }]);
    }

    #[test]
    fn a_present_owner_wakes_an_asleep_desktop_and_a_member_does_not() {
        let mut d = ready("virtual", 0);
        d.mark_asleep(100);
        let mut ds = map(vec![d]);
        let mut q = Vec::new();
        ds.get_mut("virtual").unwrap().viewer_joined("dv_m", "member", false, 200);
        assert!(tick(&mut ds, &mut q, 200).is_empty());
        ds.get_mut("virtual").unwrap().viewer_joined("dv_o", "owner", true, 210);
        assert_eq!(tick(&mut ds, &mut q, 210), vec![Action::Proceed("virtual".into())]);
    }

    #[test]
    fn an_agent_in_its_pause_is_refused_and_a_person_is_not() {
        let mut d = ready("virtual:w", 0);
        d.mark_asleep(10);
        d.agent_paused_until = Some(900);
        let ds = map(vec![d]);
        let mut q = Vec::new();
        assert_eq!(plan_start(&ds, &mut q, "virtual:w", Asker::Agent, &limits(4, 2), &facts(8, 4), 20), Plan::AgentPaused { until: 900 });
        assert_eq!(plan_start(&ds, &mut q, "virtual:w", Asker::Person, &limits(4, 2), &facts(8, 4), 20), Plan::Wake);
    }

    #[test]
    fn every_state_and_reason_has_its_sentence() {
        for s in [State::Closed, State::Waiting, State::Starting, State::Ready, State::Asleep, State::Waking] {
            assert!(!s.message(2).is_empty(), "{s:?}");
        }
        assert!(State::Waiting.message(2).contains("runs 2 at a time"));
        let all = [
            CloseReason::AsleepTooLong,
            CloseReason::LowMemory,
            CloseReason::Restarted,
            CloseReason::ClosedBy { by: "rosson".into() },
            CloseReason::SwitchedOff,
            CloseReason::Crashed { detail: "Fatal server error".into() },
            CloseReason::NobodyViewing,
        ];
        for r in all {
            let j = serde_json::to_value(Closed { reason: r.clone(), at: 5 }).expect("json");
            assert_eq!(j["code"], r.code());
            assert_eq!(j["at"], 5);
            assert!(!r.message().is_empty());
        }
        assert_eq!(CloseReason::ClosedBy { by: "rosson".into() }.message(), "The desktop was closed by rosson.");
    }

    #[test]
    fn the_state_file_round_trips() {
        let r = Record {
            id: "virtual:w".into(),
            mode: RunMode::Systemd,
            state: State::Asleep,
            closed: None,
            display: Some(":4".into()),
            size: Some(MIN_SIZE),
            units: units("virtual:w"),
            pids: vec![],
            started_at: Some(1),
            asleep_since: Some(2),
            boot_id: "b".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"state\":\"asleep\"") && s.contains("\"mode\":\"systemd\""), "{s}");
        assert_eq!(serde_json::from_str::<Record>(&s).unwrap(), r);
    }
}
