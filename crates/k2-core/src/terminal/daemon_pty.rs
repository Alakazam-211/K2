//! Kessel daemon-hosted terminal session.
//!
//! Minimum viable terminal: PTY + alacritty Term on the daemon,
//! driven by alacritty_terminal's built-in `EventLoop::spawn()`.
//! No LineMux, no byte broadcast, no ring, no APC coordination —
//! single-subscriber by design.
//!
//! Conceptually this is Alacritty_v1 with the PTY and Term moved
//! from the Tauri process into the daemon so that:
//!
//!   - Sessions survive Tauri quit (daemon owns the master FD).
//!   - Heartbeats can target the session by `agent_name` via
//!     `session_map` registration (caller's responsibility).
//!   - One Tauri-side grid-snapshot/delta client can render it.
//!
//! Follows Zed's `TerminalBuilder` pattern — uses alacritty's
//! built-in event loop instead of a custom reader thread. See
//! `.k2/prds/alacritty-v2.md` for the product context.
//!
//! **Deliberately NOT used by** `session_stream_pty.rs` or any
//! Kessel-T0 path. Those stay on their own fork. When v2 ships and
//! retires v1, this becomes the single daemon-side terminal type.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{
    EventListener, Notify, OnResize, WindowSize,
};

/// Re-exported so downstream crates (the daemon, tests) can pattern-
/// match on alacritty's lifecycle events without needing their own
/// direct `alacritty_terminal` dependency. The daemon crate only
/// depends on k2-core; this keeps that surface honest.
pub use alacritty_terminal::event::Event as AlacEvent;

use alacritty_terminal::event_loop::{EventLoop, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::Term;
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::session::SessionId;
use crate::terminal::login_path;
use crate::terminal::sandbox::{
    SandboxSpec, ShellSpec, SpawnRequest, SpawnedChild, WorkspaceMountSpec,
};

/// Scrollback depth (in rows) retained by the daemon-side Term.
/// Matches `session_stream_pty.rs`'s value so v2 sessions inherit
/// the same "how much history can I scroll back through" UX as
/// v1 sessions.
pub const SCROLLBACK_CAP: usize = 5000;

/// Thin `Dimensions` implementation for the Term. `total_lines`
/// returns rows + SCROLLBACK_CAP so the Term sizes its scrollback
/// buffer correctly at construction time.
#[derive(Clone, Copy, Debug)]
pub struct TermSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows + SCROLLBACK_CAP
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// One visible grid row as plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRow {
    /// Styles dropped. Trailing blanks trimmed, except on a wrapped
    /// row: content reached the last column there, so a trailing
    /// space is real text on the wrap boundary.
    pub text: String,
    /// alacritty's WRAPLINE on the row's last cell: the terminal
    /// soft-wrapped and the logical line continues on the next row
    /// (no newline was written between them).
    pub wrapped: bool,
}

/// Plain-text projection of a Term's visible rows with the soft-wrap
/// flag per row. A free function over the Term so it is testable
/// with an in-memory Term fed through the parser.
pub fn visible_rows_of<T>(term: &Term<T>) -> Vec<VisibleRow> {
    use alacritty_terminal::index::{Column, Line, Point};
    use alacritty_terminal::term::cell::Flags;
    let cols = term.columns();
    let rows = term.screen_lines();
    let grid = term.grid();
    let mut out = Vec::with_capacity(rows);
    for r in 0..(rows as i32) {
        let mut line = String::with_capacity(cols);
        for c in 0..cols {
            let cell = &grid[Point::new(Line(r), Column(c))];
            line.push(if cell.c == '\0' { ' ' } else { cell.c });
        }
        let wrapped = cols > 0
            && grid[Point::new(Line(r), Column(cols - 1))]
                .flags
                .contains(Flags::WRAPLINE);
        if !wrapped {
            let trimmed = line.trim_end();
            line.truncate(trimmed.len());
        }
        out.push(VisibleRow { text: line, wrapped });
    }
    out
}

/// Test helper: feed `bytes` through a real alacritty Term of the
/// given size (no PTY) and return its visible rows. Lets callers test
/// screen scraping against the terminal's own wrapping.
#[cfg(any(test, feature = "test-util"))]
pub fn visible_rows_from_bytes(cols: usize, rows: usize, bytes: &[u8]) -> Vec<VisibleRow> {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;
    let size = TermSize { cols, rows };
    let mut term = Term::new(TermConfig::default(), &size, VoidListener);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, bytes);
    visible_rows_of(&term)
}

/// One grid row for screen scraping: the text, the soft-wrap flag, the
/// OSC 8 hyperlink targets on the row, and whether it is still on the
/// visible screen (false = scrollback history).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRow {
    /// Same rules as [`VisibleRow::text`].
    pub text: String,
    /// Same rules as [`VisibleRow::wrapped`].
    pub wrapped: bool,
    /// Distinct OSC 8 link targets (`ESC ] 8 ; ; <uri> BEL`) on the
    /// row's cells, in column order. The terminal keeps the whole URI
    /// per cell, so a target is never cut by layout or wrapping.
    pub links: Vec<String>,
    /// On the visible screen (not scrollback).
    pub visible: bool,
}

/// Like [`visible_rows_of`], plus up to `history` rows of scrollback
/// above the screen and each row's OSC 8 link targets. Oldest row
/// first. Used by the LLM sign-in watcher so a sign-in URL that scrolled
/// off the screen, or that the tool sent as a hyperlink, is never lost.
pub fn scan_rows_of<T>(term: &Term<T>, history: usize) -> Vec<ScanRow> {
    use alacritty_terminal::index::{Column, Line, Point};
    use alacritty_terminal::term::cell::Flags;
    let cols = term.columns();
    let rows = term.screen_lines() as i32;
    let grid = term.grid();
    let back = history.min(grid.history_size()) as i32;
    let mut out = Vec::with_capacity((rows + back) as usize);
    for r in -back..rows {
        let mut line = String::with_capacity(cols);
        let mut links: Vec<String> = Vec::new();
        for c in 0..cols {
            let cell = &grid[Point::new(Line(r), Column(c))];
            line.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(h) = cell.hyperlink() {
                let uri = h.uri();
                if !links.iter().any(|l| l == uri) {
                    links.push(uri.to_string());
                }
            }
        }
        let wrapped = cols > 0
            && grid[Point::new(Line(r), Column(cols - 1))]
                .flags
                .contains(Flags::WRAPLINE);
        if !wrapped {
            let trimmed = line.trim_end();
            line.truncate(trimmed.len());
        }
        out.push(ScanRow { text: line, wrapped, links, visible: r >= 0 });
    }
    out
}

/// Test helper: [`scan_rows_of`] over a Term of the given size fed
/// `bytes` (no PTY).
#[cfg(any(test, feature = "test-util"))]
pub fn scan_rows_from_bytes(cols: usize, rows: usize, history: usize, bytes: &[u8]) -> Vec<ScanRow> {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;
    let size = TermSize { cols, rows };
    let mut term = Term::new(TermConfig::default(), &size, VoidListener);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, bytes);
    scan_rows_of(&term, history)
}

/// Minimal `EventListener` that broadcasts alacritty lifecycle events
/// to any number of subscribers via `tokio::sync::broadcast`. The one
/// exception is `PtyWrite` — terminal-query answers — which routes back
/// into the PTY input channel instead (see `send_event`).
/// Consumers (A3's WS handler) subscribe fresh on attach and pull
/// from their own receiver; there's no ownership transfer, so a
/// subscriber disconnecting + reconnecting works cleanly.
///
/// `send_event` is invoked by alacritty's IO thread, which is a
/// plain `std::thread` (not a tokio context). `broadcast::Sender::send`
/// is thread-safe and non-blocking, so cross-context use is fine.
///
/// Channel capacity. Each `Wakeup` event drives one `build_emit`
/// snapshot/delta on the subscriber side; if the channel fills before
/// a briefly-slow subscriber drains it, that subscriber gets
/// `RecvError::Lagged` and we flush a fresh full snapshot — which is
/// itself more traffic. Issue #8: a long-lived window that accumulated
/// many mounted panes could fire a burst of resize/claim churn that,
/// combined with full-screen TUI redraws, briefly overran the old
/// 256-slot bound → `lagged` → snapshot flush → more traffic → the
/// visible "stall then recover". The renderer-side fix (claim only the
/// visible+focused pane) removes the churn at the source; this larger
/// bound is the defense-in-depth backstop so a single full-screen
/// redraw burst can't tip a momentarily-slow subscriber into the
/// lag/flush cycle. 4096 ≈ tens of seconds of headroom at Alacritty's
/// typical ~10-100 events/sec, while staying bounded (each slot is a
/// small `AlacEvent`, so the ceiling is a few hundred KB worst case).
pub const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// Sink for terminal-query answers (`AlacEvent::PtyWrite`). A boxed
/// closure rather than the concrete `Notifier` so Term-level tests can
/// capture replies without standing up a real alacritty event loop
/// (`EventLoopSender` is unconstructible outside `EventLoop::channel`).
type QueryReplySink = Box<dyn Fn(String) + Send + Sync>;

#[derive(Clone)]
pub struct DaemonEventListener {
    tx: broadcast::Sender<AlacEvent>,
    /// Where `PtyWrite` query answers go: the session's PTY input
    /// channel. Set exactly once by `spawn()` — after the event loop
    /// exists, before its IO thread starts — so no query can race an
    /// empty slot (queries only arrive by parsing PTY output, which
    /// begins after the IO thread spawns). `OnceLock` keeps reads on
    /// the hot event path lock-free.
    reply_tx: Arc<std::sync::OnceLock<QueryReplySink>>,
    /// k2-ptyd P75 (V1): while true, terminal-query answers are dropped
    /// and window-system queries are not broadcast. Set only while bytes
    /// that were already answered once (adoption replay) go through the
    /// Term. Always false on the live path, so behaviour is unchanged.
    muted: Arc<std::sync::atomic::AtomicBool>,
}

impl DaemonEventListener {
    /// P75: mute (or unmute) query replies for a replay.
    pub fn set_replies_muted(&self, on: bool) {
        self.muted.store(on, std::sync::atomic::Ordering::SeqCst);
    }
}

impl EventListener for DaemonEventListener {
    fn send_event(&self, event: AlacEvent) {
        if self.muted.load(std::sync::atomic::Ordering::SeqCst)
            && matches!(
                event,
                AlacEvent::PtyWrite(_)
                    | AlacEvent::ColorRequest(..)
                    | AlacEvent::TextAreaSizeRequest(..)
                    | AlacEvent::ClipboardLoad(..)
            )
        {
            return;
        }
        // Terminal-query ANSWERS (DA `\x1b[c`, DSR/CPR `\x1b[6n`, DECRQM,
        // kitty-keyboard negotiation, XTWINOPS…) arrive as `PtyWrite` and
        // flow INTO the PTY on the same channel as user keystrokes — the
        // exact routing alacritty's own event loop performs (plain FIFO
        // enqueue onto the IO thread's write list, no priority). They are
        // never broadcast: a query answer is transport back to the child,
        // not session output, so viewers must not observe it (and
        // `subscriber_count()` stays a pure client-attachment signal).
        //
        // INVARIANT — no DCS replies (slice 5, grok gotcha): grok echoes
        // any unconsumed DCS reply (`ESC P … ST`, e.g. an XTVERSION
        // answer) into its composer as literal text. K2's emulator can
        // never trigger that: every `PtyWrite` alacritty_terminal 0.26
        // produces is CSI-shaped (`ESC [ …` — DA1/DA2, DSR/CPR, DECRPM,
        // XTWINOPS, kitty flags); it implements no DECRQSS/XTVERSION
        // responder, and the OSC-shaped ColorRequest/ClipboardLoad
        // events are never answered by the daemon (sessions_grid_ws
        // ignores them by policy). Pinned by
        // `query_replies_are_never_dcs_shaped` below — if a dependency
        // bump ever adds a DCS responder, that test fails and DCS must
        // be gated away from grok panes before shipping.
        if let AlacEvent::PtyWrite(text) = event {
            if let Some(reply) = self.reply_tx.get() {
                reply(text);
            }
            return;
        }
        // Fire-and-forget. If no subscribers, send returns `Err`
        // and we ignore it — the daemon keeps advancing Term state
        // regardless. Subscribers that reconnect later will get the
        // current grid via an initial snapshot + subsequent live
        // events from that point forward.
        let _ = self.tx.send(event);
    }
}

/// Source-of-truth marker for `DaemonPtySession::label`. Drives
/// the title-event-vs-locked-label state machine that lives on the
/// daemon side (Phase B / `session-label-daemon-owned.md` PRD).
///
/// - `Pty` — default. PTY `WindowTitleChanged` events freely
///   overwrite the label. Right answer for ad-hoc Cmd+T tabs where
///   vim's filename or Claude's progress glyph is informative.
/// - `Seed` — caller (renderer/CLI/heartbeat fire) supplied a
///   meaningful label at spawn time but didn't lock it. PTY can
///   still update the label as the session evolves; the seed is
///   just the initial value subscribers see on connect.
/// - `Locked` — caller wants the label PINNED. PTY title events
///   are observed (still drive activity-marker detection) but
///   never mutate the label. Used by the canonical workspace+agent
///   session, heartbeat-fire sessions, and explicit user renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSource {
    Pty,
    Seed,
    Locked,
}

impl Default for LabelSource {
    fn default() -> Self {
        LabelSource::Pty
    }
}

/// Configuration for `DaemonPtySession::spawn`. Construct via
/// `DaemonPtyConfig::default()` and mutate fields, or use the
/// struct literal with explicit values.
#[derive(Debug, Clone)]
pub struct DaemonPtyConfig {
    pub session_id: SessionId,
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<PathBuf>,
    /// Shell program to run. `None` = alacritty's default
    /// (user's login shell).
    pub program: Option<String>,
    /// Arguments passed to `program`. Ignored if `program` is None.
    /// This is the **ephemeral exec** argv (may include a fire-once
    /// host-session launch prompt). What is stored on the session /
    /// `args_json` is [`Self::durable_args`] when set, else this vec.
    pub args: Vec<String>,
    /// Identity-only args persisted on [`DaemonPtySession::args`] and
    /// `workspace_tab_sessions.args_json`. When `Some`, overrides
    /// [`Self::args`] for durable storage so a request-scoped launch
    /// prompt is never replayed by recovery / re-exec (fire-once S1).
    pub durable_args: Option<Vec<String>>,
    pub env: HashMap<String, String>,
    /// If true, drain the child's pending output before tearing
    /// down the PTY on child exit. Matches Zed's default.
    pub drain_on_exit: bool,
    /// Initial label for the session (Phase B). Empty string ⇒
    /// caller had no preference; PTY title events will fill it in.
    /// Populated values are visible via `LabelInitial` to the first
    /// WS subscriber.
    pub label: String,
    /// Initial label source (Phase B). Defaults to `Pty` when the
    /// caller doesn't care; set to `Seed` when supplying an initial
    /// `label` but allowing PTY updates; set to `Locked` to pin the
    /// label so PTY events never overwrite it.
    pub label_source: LabelSource,
    /// Which spawn backend opens this session's PTY (Sandbox P1 seam).
    /// Defaults to [`SandboxSpec::Passthrough`] (host-direct, byte-identical
    /// to pre-seam behavior); the real microVM backend (P2) lives behind a
    /// default-OFF env and is unconstructible in P1.
    pub sandbox: SandboxSpec,
    /// P4-H6: the daemon-allocated per-session uid the microVM worker drops to
    /// (instead of the shared `k2cell`). `None` for EVERY non-microVM spawn
    /// (Passthrough ignores it) → default-OFF parity. The daemon's spawn door
    /// allocates it from [`crate::cell_uid_pool`](the `k2-daemon` allocator) and
    /// stamps it here; [`crate::terminal::sandbox::build_worker_invocation`]
    /// emits it as `--cell-uid <n>` so the worker drops to THIS uid.
    pub cell_uid: Option<u32>,
    /// Sandbox v2 — the WORKSPACE-SCOPED **MIRROR** mount spec, threaded from the
    /// daemon's policy-resolver to `build_worker_invocation`. `Some` ONLY for a
    /// workspace-scoped API session; `None` for every other spawn (default-OFF
    /// parity). When `Some`, [`DaemonPtySession::spawn`] sets the terminal
    /// [`SpawnRequest::workspace_root`] to `None` (the mirror spec supersedes the
    /// single-dir RW mount) and hands this spec through instead. (Field name kept
    /// as `overlay` for plumbing stability.)
    pub overlay: Option<WorkspaceMountSpec>,
    /// k2-ptyd (PRD prd-k2-ptyd-v1 §7.5, P76): this session MAY be hosted
    /// by k2-ptyd so it can outlive the daemon. Set only by the two
    /// production spawn sites that register into `v2_session_map`
    /// (`v2_spawn.rs`, `spawn.rs`), never for `/v1` host sessions (N8),
    /// the LLM login PTY, tests or other owners (N9). Even when true the
    /// session stays in-process unless the switch is on (0.46.2: dev-only
    /// `K2_PTYD=1`) and ptyd is usable — see `terminal::ptyd`.
    pub survive: bool,
    /// k2-ptyd S3 (PRD §7.5 meta, P28): identity a restarted daemon needs
    /// to adopt this session — the `v2_session_map` key, workspace uuid,
    /// conversation id, `canonical`, `hb_terminal`, `llm_slot`,
    /// `secrets_dir`, `spawn_site`. Merged into the ptyd meta at spawn;
    /// ignored for in-process sessions. Identity only: never env values,
    /// tokens or keys (P72).
    pub ptyd_meta: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Default for DaemonPtyConfig {
    fn default() -> Self {
        Self {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: None,
            program: None,
            args: Vec::new(),
            durable_args: None,
            env: HashMap::new(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
            sandbox: SandboxSpec::default(),
            cell_uid: None,
            overlay: None,
            survive: false,
            ptyd_meta: None,
        }
    }
}

/// The identity half of a ptyd session (spawn or adoption).
#[cfg(unix)]
struct PtydParts {
    session_id: SessionId,
    sid: String,
    pid: i32,
    cwd: Option<PathBuf>,
    program: Option<String>,
    args: Vec<String>,
    label: String,
    label_source: LabelSource,
    cols: u16,
    rows: u16,
    meta: serde_json::Value,
}

/// P33: the error code a spawn refused by the `--resume` chokepoint
/// carries (the conversation is running in an adopted session).
pub const DUP_RESUME_REFUSED: &str = "dup_resume_refused";

/// How a session's PTY bytes move. `Alac` is alacritty's own `EventLoop`
/// (every session before k2-ptyd, and every session with the switch off).
/// `Ptyd` is a k2-ptyd-hosted session driven by K2's own IO loop
/// (`terminal::ptyd::io`, S0 decision A1).
enum PtyIo {
    Alac(Mutex<Notifier>),
    #[cfg(unix)]
    Ptyd(PtydSession),
}

#[cfg(unix)]
struct PtydSession {
    sid: String,
    io: crate::terminal::ptyd::io::PtydIo,
    owner: Arc<crate::terminal::ptyd::client::Owner>,
    /// The meta last sent to ptyd (P28); label and pin fields are kept
    /// current, the rest is fixed at spawn.
    meta: Mutex<serde_json::Value>,
    /// S3: this daemon let go of the session (§7.6 `KeepAgents`, hand-off
    /// quiesce): the IO loop is stopped and the agent belongs to the next
    /// daemon. `kill()` and `Drop` must never end it from here.
    detached: std::sync::atomic::AtomicBool,
    /// This daemon adopted the session from ptyd after a restart (it was
    /// spawned by an earlier daemon), rather than spawning it.
    adopted: bool,
}

/// Trailing-debounce window for [`DaemonPtySession::request_resize`].
/// Sized to swallow a mobile keyboard show/hide or two clients racing
/// claims (which fire well inside 120ms of each other) without adding
/// perceptible latency to a deliberate window resize.
pub const RESIZE_DEBOUNCE_MS: u64 = 120;

/// Last-declared viewport of one live grid-WS subscriber (orca study
/// `.k2/notes/orca-pty-mobile-study.md`, "Relevance to K2" #1). The WS
/// handler upserts this from `SetActive` claims carrying dims and from
/// `Resize` frames (even ones dropped for being non-active — the
/// subscriber still declared its viewport); the record is what lets
/// the daemon restore a sensible PTY size when the active viewer
/// disconnects instead of leaving the grid stuck at, say, phone dims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriberViewport {
    pub cols: u16,
    pub rows: u16,
    /// Monotonic recency stamp from the session's viewport clock. A
    /// plain counter rather than wall time so the successor election
    /// is deterministic and unit-testable.
    pub last_seen: u64,
}

/// State for the resize debouncer (`request_resize`). Guarded by one
/// Mutex; the lock is held across the underlying `resize()` call so a
/// leading apply and a trailing flush can never land out of order.
struct ResizeGate {
    /// Newest coalesced target awaiting the trailing flush. Only the
    /// final geometry of a burst is ever applied from here.
    pending: Option<(u16, u16)>,
    /// When the pending target may be applied. Extended by every
    /// request that lands inside the window (trailing debounce).
    deadline: Instant,
    /// A flusher thread is currently parked waiting on `deadline`.
    flusher_live: bool,
    /// Instant of the last resize actually applied. `None` until the
    /// first request — the initial fit of a fresh session must stay
    /// instant, never debounced.
    last_applied: Option<Instant>,
}

/// Successor election on active-viewer detach (orca study takeaway #1:
/// re-elect the most-recent surviving actor and re-fit to *their*
/// viewport). Pure so the policy is unit-testable without a PTY.
///
/// Returns `Some((subscriber_id, viewport))` — the survivor to promote
/// and the dims the PTY should restore to — when the departing
/// connection held the active claim AND a surviving subscriber with
/// known (nonzero) dims exists. Ties on recency break toward the
/// higher subscriber id (later-connected). `None` means "no takeover":
/// either the departing viewer wasn't active (nothing to restore) or
/// nobody eligible survives (leave dims as-is; a reattaching viewer
/// claims + resizes anyway).
fn elect_on_detach(
    viewports: &HashMap<u64, SubscriberViewport>,
    departing: u64,
    current_active: u64,
) -> Option<(u64, SubscriberViewport)> {
    if current_active != departing {
        return None;
    }
    viewports
        .iter()
        // Defensive: skip the departing id even if the caller hasn't
        // removed it yet, and skip records with unknown dims (a claim
        // that never carried cols/rows) — promoting one would resize
        // the PTY to 0x0-clamped nonsense.
        .filter(|(id, vp)| **id != departing && vp.cols > 0 && vp.rows > 0)
        .max_by_key(|(id, vp)| (vp.last_seen, **id))
        .map(|(id, vp)| (*id, *vp))
}

/// A daemon-hosted terminal session.
///
/// Holds the PTY, the alacritty Term, and the PTY writer handle.
/// Typically stored inside an `Arc` so multiple subsystems
/// (session_map, registry, WS handler) can share one handle.
///
/// The PTY **master fd lives in alacritty's IO thread** (the
/// `EventLoop` owns the `Pty`; `spawn()` discards the JoinHandle), so
/// the fd's lifetime is the IO thread's lifetime — NOT this struct's.
/// The thread exits on exactly two triggers: it observes the child's
/// exit itself (its own `try_wait` wins the reap), or it receives an
/// explicit `Msg::Shutdown`. Dropping the last Arc does NEITHER —
/// alacritty's `drain_recv_channel` treats a disconnected channel as
/// "no messages" (keeps looping) and a dropped `Sender` never calls
/// `poller.notify()`, so the thread just stays parked. That is why
/// `kill()` sends `Msg::Shutdown` explicitly (2026-07-02 PTY-master
/// leak fix); Drop then rides on `kill()`'s idempotent gate.
pub struct DaemonPtySession {
    pub session_id: SessionId,
    pub cwd: Option<PathBuf>,
    pub program: Option<String>,
    /// A2/B2: the backend tier this session was spawned under (SSOT).
    /// Read by the daemon's reuse-echo (to report the resolved backend
    /// name on a reattach) and by the ChildExit cleanup gate (to run the
    /// authoritative microVM cgroup/NEWROOT teardown only for microVM
    /// cells). `SandboxSpec` is `Copy`; for the default [`SandboxSpec::Passthrough`]
    /// path this field is inert — zero behavior change.
    pub sandbox: SandboxSpec,
    /// PID of the direct child process this PTY spawned. Captured at
    /// spawn time via alacritty's `Pty::child().id()` before the Pty
    /// is consumed by `EventLoop::new`. `None` only if capture is
    /// unavailable on a given platform.
    ///
    /// The child is launched with `setsid()` by alacritty
    /// (`tty/unix.rs`), so it is its own session/process-group leader:
    /// `getpgid(pid) == pid`, a group distinct from the daemon's. That
    /// makes `killpg(pgid, …)` in `kill()` SAFE — it can only reach the
    /// child + its descendants, never the daemon.
    ///
    /// Used by `kill()` to forcefully terminate + reap the child on
    /// teardown. Without this, dropping `pty_notifier` only delivers a
    /// single SIGHUP to the direct child via alacritty's `Pty::Drop`
    /// (no killpg, no SIGKILL, no waitpid) — agent CLIs (claude/codex)
    /// that ignore or outlive SIGHUP orphan and accumulate (~200MB
    /// each → multi-GB leak). v1 reaped correctly; v2 dropped it.
    pid: Option<i32>,
    /// Guards `kill()` idempotency. Flipped to `true` the first time
    /// `kill()` actually runs its kill+reap sequence so a second call
    /// (e.g. explicit unregister + then Drop) is a cheap no-op and we
    /// never `waitpid` a PID that may have been recycled.
    killed: std::sync::atomic::AtomicBool,
    /// Args the child was spawned with. Persisted on the session so
    /// post-spawn callers (e.g. heartbeat smart-launch's "is there a
    /// live PTY running --resume <session_id>?" check) can match by
    /// arg contents without keeping a parallel map. Empty when the
    /// shell was spawned with the user's default login args only.
    pub args: Vec<String>,

    /// The daemon-side alacritty Term. Locked briefly by the WS
    /// handler to snapshot the grid or by `resize()` to reshape it.
    /// Alacritty's `FairMutex` prevents writer starvation under
    /// heavy IO-thread contention.
    term: Arc<FairMutex<Term<DaemonEventListener>>>,

    /// Notifier for writing input bytes + signaling resize, and — via
    /// `kill()` — for the explicit `Msg::Shutdown` that makes the IO
    /// thread exit and drop the PTY (closing the master fd). NOTE:
    /// merely dropping this does NOT shut the IO thread down (see the
    /// struct doc); teardown must go through `kill()`.
    /// Guarded by a `Mutex` so concurrent `write()` + `resize()`
    /// calls serialize (Notifier::notify needs `&self` but
    /// on_resize needs `&mut self`).
    ///
    /// k2-ptyd: a ptyd-hosted session holds `PtyIo::Ptyd` instead (its
    /// "Shutdown" is stopping K2's IO loop; the child belongs to ptyd).
    pty_io: PtyIo,

    /// Broadcast sender for alacritty lifecycle events (Wakeup,
    /// Title, Bell, ChildExit, etc.). Subscribers call
    /// `subscribe_events()` to get a fresh receiver — any number
    /// of subscribers can exist, and reconnects just subscribe
    /// again (no ownership handoff).
    events_tx: broadcast::Sender<AlacEvent>,

    /// Flipped to `true` when the alacritty `ChildExit` event has
    /// been observed. Read by `is_child_alive()` for
    /// idempotency-check liveness probes (the daemon's
    /// `agents running` reaping pass and the spawn helper's
    /// existing-session check both consult it). Set inside the
    /// event listener — fires synchronously when alacritty's IO
    /// thread sees the child exit, so the bool flips before the
    /// broadcast subscribers are woken.
    child_exited: std::sync::atomic::AtomicBool,

    /// 0.37.11 — id of the subscriber currently claiming "active
    /// viewer" status. The active viewer is the one whose grid
    /// dimensions drive the PTY's resize. Other subscribers see
    /// the daemon's snapshot/delta stream at whatever size the
    /// active viewer dictates and don't push their own resize
    /// frames.
    ///
    /// 0 means "no active claim yet" — first resize from any
    /// subscriber wins (matches pre-claim behavior for single-
    /// viewer sessions). Non-zero values are
    /// monotonically-incrementing subscriber ids generated by the
    /// WS accept handler.
    ///
    /// Implemented as AtomicU64 so claim / release / lookup are
    /// lock-free; contention is minimal (claims fire on window
    /// focus change, not per-frame).
    pub active_subscriber: std::sync::atomic::AtomicU64,

    /// 2026-07-02 PTY-leak breaker — latched true (never cleared) by the
    /// grid-WS handler the first time ANY subscriber attaches to this
    /// session. Distinguishes a session a client actually looked at from
    /// one that was spawned and abandoned: the split-pane restore
    /// re-mint loop (fixed client-side in b339c70) minted a fresh
    /// `tab-<uuid>` bare-shell spawn every cycle that nothing ever
    /// attached to, until the box ran out of PTYs. `v2_spawn`'s
    /// per-workspace cap counts only never-attached bare shells, so
    /// real (once-viewed) tabs never count against it.
    pub ever_attached: std::sync::atomic::AtomicBool,

    /// 2026-07-02 PTY-leak incident (Bug 2) — the number of REAL
    /// viewers currently attached, i.e. live [`ViewerRegistration`]
    /// guards handed out by [`Self::attach_viewer`] (the grid-WS
    /// handler acquires one per connection and holds it until ANY
    /// disconnect path returns).
    ///
    /// This deliberately does NOT piggyback on
    /// `events_tx.receiver_count()`: the daemon holds its own
    /// internal receivers on the same broadcast channel — the
    /// child-exit observer (`v2_spawn::spawn_child_exit_observer`)
    /// for the session's whole life, plus the shared grid emitter
    /// (`grid_emitter::run`) from first attach until child exit — so
    /// the receiver count never returns to its true zero. That
    /// permanently defeated every "no viewers attached" consumer:
    /// the GH#22 un-forced close guard saw ≥1 forever and refused
    /// every reap. Counting explicit registrations instead means a
    /// future internal `subscribe_events()` caller can NEVER drift
    /// back into being miscounted as a viewer.
    viewer_count: std::sync::atomic::AtomicUsize,

    /// 0.39.43 (PRD `daemon-multi-client-arbitration.md` Issue A) —
    /// the active viewer's viewport dimensions, captured on the
    /// `SetActive { active:true, cols, rows }` claim. When a viewer
    /// becomes active, the WS handler stores their cols/rows here AND
    /// immediately `resize()`s the PTY to them, so the grid snaps to
    /// the active viewer's size on claim instead of waiting for a
    /// follow-up `Resize` frame. `0` means "no dims captured yet"
    /// (an older client claimed without dims, or no claim has carried
    /// dims) — in that case the claim leaves the PTY size untouched,
    /// matching pre-0.39.43 behavior.
    pub active_cols: std::sync::atomic::AtomicU16,
    pub active_rows: std::sync::atomic::AtomicU16,

    /// S7a pin-to-size (prd-presence-multiplayer-v1 §5.5) — the
    /// daemon-canonical pinned grid geometry. `0` = unpinned (the
    /// same sentinel convention as `active_cols`/`active_rows`).
    /// While BOTH are nonzero, [`Self::request_resize`] (and the
    /// underlying [`Self::resize`], belt-and-suspenders) clamp every
    /// incoming target to these dims — which makes all four resize
    /// paths (grid-WS Resize, typing input-claim snap, SetActive
    /// claim, detach-promotion) natural no-ops via the existing
    /// same-dims skip. Set via [`Self::set_pinned`] only.
    pinned_cols: std::sync::atomic::AtomicU16,
    pinned_rows: std::sync::atomic::AtomicU16,
    /// Who pinned ("owner" | connect-user username), for display.
    /// `None` when unpinned.
    pinned_set_by: Mutex<Option<String>>,
    /// Broadcast channel for pin changes (the `label_tx` pattern):
    /// [`Self::set_pinned`] pushes the new state — `Some((cols,
    /// rows))` on pin, `None` on clear — so every grid-WS
    /// subscriber's select loop wakes and emits a `pin_changed`
    /// frame to its client without polling.
    pin_tx: broadcast::Sender<Option<(u16, u16)>>,
    /// Companion T0 — ephemeral "claim session" pin ownership. When
    /// nonzero, the current pin was set by THIS grid-WS subscriber id
    /// (the socket's `claim_pin` action) and auto-clears when that
    /// subscriber detaches ([`Self::detach_subscriber`]). `0` = the
    /// pin (if any) is a normal, manually-cleared one. Every plain
    /// [`Self::set_pinned`] resets this to `0` — a normal pin replaces
    /// an ephemeral one (last write wins) and an unpin clears it;
    /// [`Self::set_pinned_ephemeral`] re-stamps the owner afterwards.
    ephemeral_pin_owner: std::sync::atomic::AtomicU64,

    /// Per-subscriber viewport registry (orca study, "Relevance to
    /// K2" #1/#8). Keyed by the WS handler's `subscriber_id`; the
    /// handler upserts via [`Self::note_viewport`] and removes via
    /// [`Self::detach_subscriber`] on ANY disconnect. All the
    /// size-arbitration POLICY — who inherits the PTY size when the
    /// active viewer leaves — lives here next to the
    /// `active_subscriber` atomics, not scattered through WS handlers.
    viewports: Mutex<HashMap<u64, SubscriberViewport>>,
    /// Monotonic recency clock for `viewports` stamps.
    viewport_clock: std::sync::atomic::AtomicU64,
    /// Debounce state for [`Self::request_resize`] (orca study
    /// takeaway #3 — coalesce resize bursts instead of thrashing the
    /// PTY through rapid resize+reflow cycles).
    resize_gate: Mutex<ResizeGate>,
    /// Count of resizes actually APPLIED through the debounced front
    /// door. Diagnostic, and the test surface for the same-dims skip
    /// (Kessel postmortem — see `request_resize`): a skipped no-op
    /// must NOT bump this.
    resizes_applied: std::sync::atomic::AtomicU64,
    /// Bumped by [`Self::resize`] on every REAL dims change, from
    /// inside the same term-lock critical section as the pre-reflow
    /// viewport clear. The grid emitter samples it (under the same
    /// lock) to open its blank-frame suppression window: between the
    /// clear and the child's SIGWINCH repaint the Term is blank, and
    /// broadcasting that intermediate is the user-visible resize
    /// "black flash". An atomic on the session rather than a new
    /// event variant because `AlacEvent` is alacritty's own enum (not
    /// ours to extend), and a side-channel message would reintroduce
    /// ordering questions the lock-coupled atomic doesn't have.
    resize_generation: std::sync::atomic::AtomicU64,

    /// Authoritative human-friendly label (Phase B / PRD
    /// `session-label-daemon-owned.md`). The daemon owns this
    /// string; every consumer (Tauri tabs, mobile companion, CLI
    /// `agents running`, future MCP) reads it from here. PTY title
    /// events are absorbed here according to `label_source` — they
    /// don't round-trip through the renderer back to the tab.
    label: std::sync::RwLock<String>,
    /// Drives the PTY-title-vs-locked-label state machine. See
    /// [`LabelSource`]. Held under its own `RwLock` so callers can
    /// flip `Pty → Locked` (user explicit rename) and have new PTY
    /// events instantly start ignoring the title surface.
    label_source: std::sync::RwLock<LabelSource>,
    /// Broadcast channel for label changes (Phase B). Out-of-band
    /// label sets — from `/cli/sessions/<id>/label` or from the
    /// agent-display-name change hook — push the new label here so
    /// every WS subscriber's select loop wakes and emits a
    /// `LabelChanged` to its client. PTY-driven label updates
    /// don't need this channel since the WS subscriber that's
    /// already processing the `Title` AlacEvent emits
    /// `LabelChanged` directly; the broadcast still fires there
    /// too so OTHER subscribers (multi-window) converge.
    label_tx: broadcast::Sender<String>,
}

impl DaemonPtySession {
    /// Spawn a child process attached to a PTY + Term pair.
    ///
    /// Synchronous to the caller. Internally starts alacritty's IO
    /// thread in the background; that thread lives until the
    /// session is dropped.
    ///
    /// Returns an `Arc<Self>` because typical use has multiple
    /// owners (e.g. `session_map` + the WS handler). Returning Arc
    /// eagerly saves every caller from having to re-wrap.
    pub fn spawn(cfg: DaemonPtyConfig) -> Result<Arc<Self>, io::Error> {
        // Runtime stage H: a daemon that handed off to its successor starts
        // no PTY — the successor resumes the sessions (two would double-
        // resume one conversation on the same tree).
        crate::handoff_state::guard().map_err(io::Error::other)?;
        // k2-ptyd P33 (V9): the ONE `--resume` duplicate chokepoint. A
        // conversation an adopted (restart-surviving) session still runs
        // must not be resumed a second time by any path — restart
        // recovery, canonical resolve, refresh, ensure-pinned-chat,
        // heartbeat fires. The map is empty unless sessions were adopted,
        // so with the switch off this is a lookup in an empty map.
        #[cfg(unix)]
        if let Err(holder) = crate::terminal::ptyd::adopt::check_resume(cfg.program.as_deref(), &cfg.args) {
            log_debug!(
                "[daemon_pty] {} session={} — the conversation is held by adopted session {}",
                DUP_RESUME_REFUSED,
                cfg.session_id,
                holder
            );
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{DUP_RESUME_REFUSED}: the conversation is running in live session {holder}"),
            ));
        }
        let cols = cfg.cols.max(1);
        let rows = cfg.rows.max(1);

        let window_size = WindowSize {
            num_cols: cols,
            num_lines: rows,
            // Cell-size hints. The daemon is headless so we don't
            // have real pixel metrics; these fields are only used
            // by programs that query cell pixel size (e.g. Sixel
            // image renderers). 10x20 is a safe stand-in.
            cell_width: 10,
            cell_height: 20,
        };

        // Build alacritty's `tty::Options`. None-shell gets the
        // user's login shell, same as opening a terminal.app window
        // with no command override.
        // Exec argv (may include a fire-once launch prompt). Durable
        // identity-only args are stored on the session separately when
        // `durable_args` is set so recovery never re-fires the prompt.
        let spawn_args = cfg.args.clone();
        let store_args = cfg
            .durable_args
            .clone()
            .unwrap_or_else(|| spawn_args.clone());

        // Build the env we hand to alacritty's tty::new. Without an
        // explicit TERM/COLORTERM, child processes inherit alacritty's
        // default (TERM=dumb on this version), which makes Claude Code,
        // bash prompts, ls --color, vim — basically every TUI — turn
        // OFF colors. Mirror what `alacritty_backend.rs` does for the
        // legacy renderer (TERM=xterm-256color + COLORTERM=truecolor +
        // TERM_PROGRAM=K2) so v2 children render the same colors as
        // legacy children.
        let mut child_env = cfg.env.clone();

        // Issue #15: PATH enrichment. The daemon runs under macOS
        // launchd with a bare PATH (/usr/bin:/bin:/usr/sbin:/sbin),
        // and under Windows services with a similarly incomplete
        // Path, so children spawned by bare name (`claude`, `cursor`,
        // `gemini`) can't find binaries in ~/.local/bin (the Claude
        // native installer default), /opt/homebrew/bin, nvm shims,
        // %APPDATA%\npm, etc. — they ENOENT. Augment the child PATH
        // with the union of the user's login-shell / User+Machine
        // PATH, known install dirs, and the daemon's inherited PATH.
        // Respect a caller-provided PATH/`Path`: if `cfg.env` already
        // set one explicitly, leave it untouched — only fill in the
        // enriched value when the caller passed none (which is the
        // common case: spawn.rs hands an empty env, so agents get the
        // enriched PATH). On Windows, `PATH` and `Path` are equivalent.
        // Shared with [`preflight_program`], so a preflight finds exactly
        // what this spawn will exec.
        let dropped = prepare_child_path(&mut child_env);
        if dropped > 0 {
            log_debug!(
                "[daemon_pty] dropped {} unusable PATH entr{} for session={}",
                dropped,
                if dropped == 1 { "y" } else { "ies" },
                cfg.session_id
            );
        }

        // Windows: CreateProcess does not apply PATHEXT. Resolve
        // `claude` → `%APPDATA%\npm\claude.cmd` and wrap .cmd/.bat
        // through COMSPEC. Unix is a no-op. Uses the PATH we just
        // put on the child so known npm dirs are visible.
        // Sandbox P2a: ShellSpec (alacritty Shell fields are pub(crate)).
        let path_for_resolve = login_path::env_path_entry(&child_env)
            .map(|(_, v)| v)
            .unwrap_or("");

        // Test guard — the ONE choke point every agent spawn crosses
        // (`spawn_agent_session_v2_blocking` → here: heartbeat wake,
        // pinned chat, host/remote sessions, skin Thread wakes). With
        // `K2_TEST_AGENT_SHIM_DIR` set the program's basename resolves
        // ONLY inside those dirs (never the login-shell PATH that let the
        // real `~/.local/bin/claude` beat a test's shim and open a browser
        // OAuth login under a temp HOME — 2026-09-12). Under a temp HOME
        // without a shim dir, a real agent path is refused outright.
        // Production (no env, real HOME) gets `cfg.program` back verbatim.
        let guarded_program: Option<String> = match cfg.program.as_deref() {
            Some(prog) => {
                let guard = crate::terminal::agent_spawn_guard::GuardEnv::from_process();
                match crate::terminal::agent_spawn_guard::resolve_program(
                    prog,
                    path_for_resolve,
                    &guard,
                ) {
                    Ok(p) => Some(p),
                    Err(msg) => {
                        log_debug!("{} session={}", msg, cfg.session_id);
                        return Err(io::Error::new(io::ErrorKind::PermissionDenied, msg));
                    }
                }
            }
            None => None,
        };
        // Codex 0.155+ attaches every TUI to ONE shared, detached
        // app-server per CODEX_HOME, and tool commands run under it with
        // the env of whichever session started it — another tab's (or a
        // dead) K2 passport. `--no-daemon` keeps the app-server in this
        // PTY so tool commands inherit THIS session's env. Exec argv only
        // (durable `store_args` above stay clean); skipped for microVM
        // cells, whose program runs in their own guest.
        let mut spawn_args = spawn_args;
        if let Some(prog) = guarded_program.as_deref() {
            if !matches!(cfg.sandbox, crate::terminal::SandboxSpec::Microvm) {
                let outcome = crate::terminal::codex_no_daemon::apply(
                    prog,
                    &mut spawn_args,
                    path_for_resolve,
                    crate::terminal::codex_no_daemon::Policy::from_process(),
                );
                match outcome {
                    crate::terminal::codex_no_daemon::Outcome::NotCodex
                    | crate::terminal::codex_no_daemon::Outcome::NotTui => {}
                    other => log_debug!(
                        "[daemon_pty] codex shared app-server: {:?} session={}",
                        other,
                        cfg.session_id
                    ),
                }
            }
        }
        // The isolated-tab marker the `k2` CLI checks before a Codex tool
        // call may use this tab's credentials: the passport's session id
        // when the exec argv has `--no-daemon` or the tab is a microVM cell
        // (its Codex runs in its own guest). Set here, by the spawner only:
        // a marker the caller's env or the daemon's own env carried is
        // overwritten or blanked, for every program.
        {
            use crate::terminal::codex_no_daemon as cnd;
            let cell = matches!(cfg.sandbox, crate::terminal::SandboxSpec::Microvm);
            let value = cnd::tab_marker(guarded_program.as_deref().unwrap_or(""), &spawn_args, &child_env, cell);
            cnd::set_tab_marker(&mut child_env, value, cnd::marker_in_process_env());
        }
        // Owner switch `agentProcessNames` (off by default): an agent's
        // Claude Code runs from `~/.k2/bin/agents/<agent>`, a hard link to
        // the version PATH resolves to now, so Activity Monitor shows the
        // agent's name instead of `2.1.322`. Exec argv only: the session's
        // stored `program` stays `claude`. Any miss keeps the program as is.
        let guarded_program = match guarded_program {
            Some(prog) if !matches!(cfg.sandbox, crate::terminal::SandboxSpec::Microvm) => {
                let (linked, outcome) =
                    crate::terminal::agent_process_name::apply(&prog, &child_env, path_for_resolve);
                if !matches!(
                    outcome,
                    crate::terminal::agent_process_name::Outcome::Off
                        | crate::terminal::agent_process_name::Outcome::NotVerified
                ) {
                    log_debug!("[daemon_pty] agent process name: {:?} session={}", outcome, cfg.session_id);
                }
                Some(linked.unwrap_or(prog))
            }
            other => other,
        };
        // Windows: a batch file or cmd.exe built-in that can't take an
        // argument safely is refused here, before anything starts.
        let shell = match guarded_program.as_deref() {
            Some(prog) => {
                let (program, args) = crate::terminal::win_cmd::resolve_spawn(prog, &spawn_args, path_for_resolve)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
                Some(ShellSpec { program, args })
            }
            None => None,
        };

        child_env
            .entry("TERM".to_string())
            .or_insert_with(|| "xterm-256color".to_string());
        child_env
            .entry("COLORTERM".to_string())
            .or_insert_with(|| "truecolor".to_string());
        child_env
            .entry("TERM_PROGRAM".to_string())
            .or_insert_with(|| "K2".to_string());
        // Claude Code turns synchronized output (DEC mode 2026) on only for
        // terminals it recognises by XTVERSION or TERM_PROGRAM, and K2 is
        // neither, so its redraws went out unbracketed and a frame could
        // catch one half-parsed ("jelly" scrolling, worst on remote boxes).
        // K2 honours 2026 (grid_emitter), so say so. A caller's value wins.
        child_env
            .entry("CLAUDE_CODE_FORCE_SYNC_OUTPUT".to_string())
            .or_insert_with(|| "1".to_string());

        // Locale: launchd/systemd hand the daemon NO LANG/LC_*, so
        // children land in the C locale — under which `pbcopy`
        // MacRoman-mangles UTF-8 (TUI copy-on-select puts `‚îå‚îÄ`
        // mojibake on the pasteboard while the screen looks fine).
        // Default a UTF-8 LANG the way real terminal emulators do;
        // caller- or daemon-env-provided locales win.
        crate::terminal::locale_env::ensure_utf8_locale(&mut child_env);

        // 0.40.34 browser-open shim: children that run `xdg-open <url>`
        // or honor `$BROWSER` must surface the URL in the CONNECTED K2
        // app (as an `open_url` session event) instead of launching a
        // browser on the daemon's — possibly headless — machine. The
        // daemon staged `~/.k2/bin/k2-open` at boot (`open_shim::stage`);
        // here we wire it into the child env. Runs AFTER the PATH
        // enrichment above and PREPENDS `~/.k2/bin`, so our staged
        // `xdg-open` (Linux) is FIRST in execvp lookup order and wins
        // over any system one. `BROWSER` uses `.entry().or_insert_with`
        // so a caller-supplied BROWSER always wins. Gated on the staged
        // file actually existing — a failed staging never points
        // children at a dangling path.
        if let Some(shim) = crate::open_shim::staged_shim_path() {
            apply_browser_shim_env(&mut child_env, &shim);
        }

        // 0.46.4 first run without git (`crate::git_tools`): when git runs,
        // do any git steps this folder still waits for; while it doesn't
        // (a Mac without Apple's Command Line Tools), put the git-wait
        // shim first on PATH so the agent's `git` prints one clear line
        // instead of Apple's install dialog and xcrun error.
        if !matches!(cfg.sandbox, crate::terminal::SandboxSpec::Microvm) {
            if let Some(dir) = crate::git_tools::before_spawn(cfg.cwd.as_deref()) {
                let current = login_path::env_path_entry(&child_env).map(|(_, v)| v.to_string()).unwrap_or_default();
                #[cfg(windows)]
                child_env.retain(|k, _| !k.eq_ignore_ascii_case("PATH"));
                child_env.insert("PATH".to_string(), crate::terminal::path_env::prepend(&current, &dir));
            }
        }

        // Capture spawn-failure diagnostics before `req` takes ownership
        // of `shell` / `child_env`. PATH entry count helps distinguish
        // bare-name ENOENT (PATH too thin) from other spawn errors.
        let spawn_prog = shell
            .as_ref()
            .map(|s| s.program.clone())
            .unwrap_or_else(|| "(login-shell)".to_string());
        let spawn_path_entries = login_path::env_path_entry(&child_env)
            .map(|(_, v)| crate::terminal::path_env::entry_count(v))
            .unwrap_or(0);

        // Sandbox P1 seam: the single backend-specific step — build
        // `tty::Options`, open the PTY, capture the child PID — now lives
        // behind `SandboxBackend::spawn`. The default `Passthrough` backend is
        // a VERBATIM extraction of the original body, so this is byte-identical
        // on every existing path. `cell_socket` / `workspace_root` are plumbed
        // here but only consumed by the P2 microVM backend.
        let req = SpawnRequest {
            shell,
            cwd: cfg.cwd.clone(),
            env: child_env,
            window_size,
            drain_on_exit: cfg.drain_on_exit,
            cell_socket: cfg.env.get("K2_HOOK_SOCK").map(PathBuf::from),
            // Sandbox v2 (PRD §B): a workspace-scoped overlay session carries its
            // RW mount in `overlay` (RO base + persistent upper/work), so the
            // legacy single-dir RW `workspace_root` is suppressed — the two must
            // never both appear (see `build_worker_invocation`). Every other
            // spawn keeps the historical `workspace_root = cwd` behavior.
            workspace_root: if cfg.overlay.is_some() {
                None
            } else {
                cfg.cwd.clone()
            },
            // P2a: ro tool mounts are a P2b concern; none plumbed here yet.
            tool_roots: Vec::new(),
            // A2/B1: thread the daemon's own session id so the microVM backend
            // names the cell deterministically (UUID = `[A-Za-z0-9-]`, satisfies
            // the worker's `[A-Za-z0-9._-]+` path-component gate). Passthrough
            // ignores this field, so the default path stays byte-identical.
            session_id: Some(cfg.session_id.to_string()),
            // P4-H6: the per-session worker uid the daemon allocated (microVM
            // only). Passthrough ignores it; the microVM backend emits it as
            // `--cell-uid` so the worker drops to THIS uid, not shared `k2cell`.
            cell_uid: cfg.cell_uid,
            // Sandbox v2 (PRD §B): the workspace-scoped overlay mount spec.
            // SLICE 3: the microVM backend consumes it; Passthrough ignores it.
            overlay: cfg.overlay.clone(),
        };
        // k2-ptyd backend selection (PRD §7.5). With the switch off (the
        // default) this returns InProcess before evaluating anything else —
        // ptyd is never contacted — and the code below is the unchanged
        // in-process path.
        {
            use crate::terminal::ptyd::{select_backend, Switch};
            #[cfg(unix)]
            use crate::terminal::ptyd::Backend;
            let choice = select_backend(
                cfg.survive,
                // Non-survive spawns never read the switch (it may read
                // settings.json); select_backend ignores it for them.
                if cfg.survive { Switch::resolve() } else { Switch::Off },
                matches!(cfg.sandbox, SandboxSpec::Passthrough),
                crate::terminal::ptyd::current_killmode,
                || {
                    #[cfg(unix)]
                    {
                        crate::terminal::ptyd::client::global().is_available()
                    }
                    #[cfg(not(unix))]
                    {
                        Err("unsupported_os".to_string())
                    }
                },
            );
            crate::terminal::ptyd::note_selection(&choice);
            #[cfg(unix)]
            if choice == Backend::Ptyd {
                return Self::spawn_ptyd(cfg, req, store_args);
            }
        }

        let backend = cfg.sandbox.backend();

        // Window ID is used on macOS/Windows to associate the PTY
        // with a specific OS window for controlling-terminal
        // semantics. The daemon has no window, so the backend passes 0.
        let __t_pty = std::time::Instant::now();
        let SpawnedChild { pty, child_pid } = match backend.spawn(req) {
            Ok(child) => child,
            Err(e) => {
                log_debug!(
                    "[daemon_pty] spawn failed session={} program={} path_entries={} err={}",
                    cfg.session_id,
                    spawn_prog,
                    spawn_path_entries,
                    e
                );
                return Err(e);
            }
        };
        // S17 / AR18: the session's child joins the agents' memory budget
        // (a no-op unless the daemon's unit is delegated; agents_leaf).
        if let Some(pid) = child_pid.and_then(|p| u32::try_from(p).ok()) {
            crate::agents_leaf::join(pid);
        }

        let pty_ms = __t_pty.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon stage=pty_open ms={:.3} session={}",
            pty_ms,
            cfg.session_id
        );

        // Event listener + broadcast channel for alacritty's
        // lifecycle events. Subscribers attach lazily via
        // `subscribe_events()`; we keep a clone of the sender here
        // so they can all tap the same stream.
        let (events_tx, _initial_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        // Drop the initial receiver — we don't keep one ourselves;
        // each subscriber calls `subscribe_events()` to get theirs.
        drop(_initial_rx);
        let query_reply = Arc::new(std::sync::OnceLock::new());
        let listener = DaemonEventListener {
            tx: events_tx.clone(),
            reply_tx: Arc::clone(&query_reply),
            muted: Default::default(),
        };

        // Term config — scrollback + cursor + colors. Start from
        // defaults (which match Zed's behavior) and override only
        // the scrollback depth to our SCROLLBACK_CAP constant.
        let term_config = TermConfig {
            scrolling_history: SCROLLBACK_CAP,
            ..TermConfig::default()
        };

        let term_size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };

        let __t_term = std::time::Instant::now();
        let term = Term::new(term_config, &term_size, listener.clone());
        let term_ms = __t_term.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon stage=term_new ms={:.3} session={}",
            term_ms,
            cfg.session_id
        );
        let term = Arc::new(FairMutex::new(term));

        // Alacritty's built-in event loop drives the PTY reader +
        // Term feeding + input writer. This replaces the custom
        // reader thread the Kessel-T0 path hand-rolls.
        let __t_loop = std::time::Instant::now();
        let event_loop = EventLoop::new(
            Arc::clone(&term),
            listener,
            pty,
            cfg.drain_on_exit,
            false, // ref_test — only true for alacritty's own test harness
        )?;

        let pty_sender = event_loop.channel();

        // Route terminal-query answers back into the PTY through the
        // same `Notifier` user input takes (`Msg::Input` FIFO — matches
        // alacritty's own `PtyWrite` handling). Installed before
        // `event_loop.spawn()`: the child's first output bytes can carry
        // a query, and the sink must exist when the parser answers it.
        let reply_notifier = Notifier(event_loop.channel());
        assert!(
            query_reply
                .set(Box::new(move |text: String| {
                    reply_notifier.notify(text.into_bytes());
                }) as QueryReplySink)
                .is_ok(),
            "query-reply sink is set exactly once, here"
        );

        // Spawn the IO thread. The handle is `JoinHandle<(EventLoop, State)>`
        // — we intentionally don't store it. The thread's return value
        // owns the EventLoop (and therefore the PTY master fd); when the
        // thread finishes, the detached JoinHandle's packet drops it and
        // the fd closes. The thread finishes when it observes the child
        // exit itself OR when `kill()` sends the explicit `Msg::Shutdown`
        // (a dropped sender alone never wakes it — see the struct doc).
        // Not joining means thread cleanup happens implicitly via
        // OS reaping; acceptable for a daemon.
        let _io_thread = event_loop.spawn();
        let event_loop_ms = __t_loop.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon stage=event_loop_spawn ms={:.3} session={}",
            event_loop_ms,
            cfg.session_id
        );

        // Phase B: small broadcast for out-of-band label updates.
        // Cap of 16 is plenty — we only push on actual changes and
        // multi-window subscribers consume immediately.
        let (label_tx, _label_rx_drop) = broadcast::channel::<String>(16);
        drop(_label_rx_drop);

        // S7a: same shape for pin changes (rare, human-triggered).
        let (pin_tx, _pin_rx_drop) = broadcast::channel::<Option<(u16, u16)>>(16);
        drop(_pin_rx_drop);

        Ok(Arc::new(Self {
            session_id: cfg.session_id,
            cwd: cfg.cwd,
            program: cfg.program,
            sandbox: cfg.sandbox,
            pid: child_pid,
            killed: std::sync::atomic::AtomicBool::new(false),
            args: store_args,
            term,
            pty_io: PtyIo::Alac(Mutex::new(Notifier(pty_sender))),
            events_tx,
            child_exited: std::sync::atomic::AtomicBool::new(false),
            active_subscriber: std::sync::atomic::AtomicU64::new(0),
            ever_attached: std::sync::atomic::AtomicBool::new(false),
            viewer_count: std::sync::atomic::AtomicUsize::new(0),
            active_cols: std::sync::atomic::AtomicU16::new(0),
            active_rows: std::sync::atomic::AtomicU16::new(0),
            pinned_cols: std::sync::atomic::AtomicU16::new(0),
            pinned_rows: std::sync::atomic::AtomicU16::new(0),
            pinned_set_by: Mutex::new(None),
            pin_tx,
            ephemeral_pin_owner: std::sync::atomic::AtomicU64::new(0),
            viewports: Mutex::new(HashMap::new()),
            viewport_clock: std::sync::atomic::AtomicU64::new(0),
            resize_gate: Mutex::new(ResizeGate {
                pending: None,
                deadline: Instant::now(),
                flusher_live: false,
                last_applied: None,
            }),
            resizes_applied: std::sync::atomic::AtomicU64::new(0),
            resize_generation: std::sync::atomic::AtomicU64::new(0),
            label: std::sync::RwLock::new(cfg.label),
            label_source: std::sync::RwLock::new(cfg.label_source),
            label_tx,
        }))
    }

    /// k2-ptyd path of [`Self::spawn`] (PRD §7.5). Everything up to the PTY
    /// open — env enrichment, the agent-spawn guard, codex `--no-daemon`,
    /// TERM/locale/browser shim — already ran in `spawn` and is in `req`.
    /// Here ptyd opens the PTY and forks the child instead of alacritty,
    /// and K2's own IO loop replaces alacritty's `EventLoop`. The Term,
    /// listener, broadcast channels and every other field are built
    /// exactly as on the in-process path.
    #[cfg(unix)]
    fn spawn_ptyd(cfg: DaemonPtyConfig, req: SpawnRequest, store_args: Vec<String>) -> Result<Arc<Self>, io::Error> {
        use crate::terminal::ptyd::{client, env as penv};
        use k2_ptyd::proto::{Reply, Request};

        let owner = client::global().owner().map_err(io::Error::other)?;
        let user = penv::ShellUser::from_env()?;
        let argv = match &req.shell {
            Some(s) => std::iter::once(s.program.clone()).chain(s.args.iter().cloned()).collect(),
            None => penv::default_shell_argv(&user),
        };
        // P15: the same env set alacritty's Command would give the child.
        let child_env = penv::child_env(std::env::vars_os(), &user, &req.env);
        // alacritty ignores an invalid working directory; so do we.
        let cwd = req.cwd.as_ref().filter(|p| p.is_dir()).map(|p| p.to_string_lossy().into_owned());
        let sid = cfg.session_id.to_string();
        // P28/P72: identity only — never env values, tokens or keys.
        let mut meta = serde_json::json!({
            "session_id": sid,
            "cwd": cfg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "program": cfg.program,
            "durable_args": store_args,
            "label": cfg.label,
            "label_source": format!("{:?}", cfg.label_source),
            "sandbox": "passthrough",
            "spawned_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        });
        // S3: the daemon's identity fields (key, workspace, conversation…)
        // so a restarted daemon can adopt the session. The fields above win.
        if let (Some(extra), Some(obj)) = (cfg.ptyd_meta.as_ref(), meta.as_object_mut()) {
            for (k, v) in extra {
                obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let events = owner.watch(&sid);
        let pid = match owner.request(Request::Spawn {
            sid: sid.clone(),
            argv,
            env: child_env,
            cwd,
            cols: req.window_size.num_cols,
            rows: req.window_size.num_lines,
            // Debug builds only (tests): a small ring makes overruns certain.
            ring_bytes: if cfg!(debug_assertions) {
                std::env::var("K2_PTYD_TEST_RING_BYTES").ok().and_then(|v| v.parse().ok())
            } else {
                None
            },
            meta: meta.clone(),
        }) {
            Ok(Reply::Spawned { pid, .. }) => pid,
            Ok(Reply::Error { code, detail }) => {
                owner.unwatch(&sid);
                log_debug!("[daemon_pty] ptyd spawn failed session={} code={}", cfg.session_id, code);
                let kind = if code == "enoent" { io::ErrorKind::NotFound } else { io::ErrorKind::Other };
                return Err(io::Error::new(kind, format!("ptyd spawn: {code}: {detail}")));
            }
            Ok(other) => {
                owner.unwatch(&sid);
                return Err(io::Error::other(format!("ptyd spawn: unexpected {other:?}")));
            }
            Err(e) => {
                owner.unwatch(&sid);
                return Err(e);
            }
        };
        // S17 / AR18 parity with the in-process path: the agent joins the
        // agents' memory budget (a no-op unless the daemon's unit is
        // delegated). ptyd is the child's parent; the child is still in the
        // daemon unit's cgroup tree and owned by the same user. What the
        // child forked before this move, and the sessions an older ptyd
        // kept across a restart, follow on agents_leaf's sweep.
        if let Ok(p) = u32::try_from(pid) {
            crate::agents_leaf::join(p);
        }
        // The vault binds this session's agent socket record to this ptyd
        // instance and process.
        crate::terminal::ptyd::note_spawned(&sid, pid, &owner.boot_id);
        // Debug builds only (tests): widen the window between `spawned` and
        // the first attach (as on a loaded box), so a flooding program has
        // filled the ring and is still writing when the attach lands.
        if cfg!(debug_assertions) {
            if let Some(ms) = std::env::var("K2_PTYD_TEST_FIRST_ATTACH_DELAY_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
        // The first attach can lose its `attached` reply to an overrun when
        // the program writes more than the ring before it lands (ptyd then
        // closes the connection). The program is fine: retry the way the
        // IO loop's reattach does, and end the session only if it really
        // ended or no attach held.
        let stream = match crate::terminal::ptyd::io::attach_retrying(&owner, &sid, &events, 0) {
            Ok(s) => s,
            Err(gave_up) => {
                use crate::terminal::ptyd::{client::SessionEvent, io::AttachGaveUp};
                let msg = match &gave_up {
                    AttachGaveUp::Ended(SessionEvent::Exited { code, signal }) => {
                        format!("ptyd: session {sid} ended before its first attach held (code={code:?} signal={signal:?})")
                    }
                    AttachGaveUp::Ended(_) => format!("ptyd: lost ptyd (or session {sid}) before the session's first attach held"),
                    AttachGaveUp::Exhausted => format!(
                        "ptyd: session {sid}'s first attach overran {} times (the program writes faster than it can be attached); stopped it",
                        crate::terminal::ptyd::io::ATTACH_TRIES
                    ),
                };
                log_debug!("[daemon_pty] {msg}");
                // A program that already exited needs no kill.
                if !matches!(gave_up, AttachGaveUp::Ended(SessionEvent::Exited { .. })) {
                    let _ = owner.request(Request::Kill { sid: sid.clone(), grace_ms: Some(100) });
                }
                let _ = owner.request(Request::AckExit { sid: sid.clone() });
                owner.unwatch(&sid);
                return Err(io::Error::other(msg));
            }
        };

        log_debug!("[daemon_pty] ptyd session={} pid={}", cfg.session_id, pid);
        Self::from_ptyd_stream(
            PtydParts {
                session_id: cfg.session_id,
                sid,
                pid,
                cwd: cfg.cwd,
                program: cfg.program,
                args: store_args,
                label: cfg.label,
                label_source: cfg.label_source,
                cols: cfg.cols,
                rows: cfg.rows,
                meta,
            },
            owner,
            events,
            stream,
            false,
        )
    }

    /// Build a ptyd-backed session around an attached data stream: the
    /// Term, listener, broadcast channels and K2's IO loop. Shared by a
    /// fresh spawn and an adoption (`replay`: bytes below `stream.end` are
    /// muted, P75, and the listener starts muted).
    #[cfg(unix)]
    fn from_ptyd_stream(
        parts: PtydParts,
        owner: Arc<crate::terminal::ptyd::client::Owner>,
        events: std::sync::mpsc::Receiver<crate::terminal::ptyd::client::SessionEvent>,
        stream: k2_ptyd::client::DataStream,
        replay: bool,
    ) -> Result<Arc<Self>, io::Error> {
        Self::from_ptyd_stream_restoring(parts, owner, events, stream, replay, None)
    }

    /// [`Self::from_ptyd_stream`] that first rebuilds the Term from a saved
    /// screen (S4, P42; `stream` then starts at its offset). The prelude
    /// runs with query replies muted (P75), like the replay after it.
    #[cfg(unix)]
    fn from_ptyd_stream_restoring(
        parts: PtydParts,
        owner: Arc<crate::terminal::ptyd::client::Owner>,
        events: std::sync::mpsc::Receiver<crate::terminal::ptyd::client::SessionEvent>,
        stream: k2_ptyd::client::DataStream,
        replay: bool,
        restore: Option<crate::terminal::ptyd::snapshot::Snapshot>,
    ) -> Result<Arc<Self>, io::Error> {
        use crate::terminal::ptyd::io as pio;
        let (events_tx, _initial_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        drop(_initial_rx);
        let query_reply = Arc::new(std::sync::OnceLock::new());
        let listener = DaemonEventListener { tx: events_tx.clone(), reply_tx: Arc::clone(&query_reply), muted: Default::default() };
        let term_config = TermConfig { scrolling_history: SCROLLBACK_CAP, ..TermConfig::default() };
        let live_cols = parts.cols.max(1) as usize;
        let live_rows = parts.rows.max(1) as usize;
        let term = match &restore {
            Some(snap) => {
                // Amendment A: the snapshot's size first, then resize.
                listener.set_replies_muted(true);
                let mut t = Term::new(term_config, &TermSize { cols: snap.cols.max(1), rows: snap.rows.max(1) }, listener.clone());
                snap.restore_into(&mut t, live_cols, live_rows);
                t
            }
            None => Term::new(term_config, &TermSize { cols: live_cols, rows: live_rows }, listener.clone()),
        };
        let term = Arc::new(FairMutex::new(term));

        // Only adoption replays the ring (a fresh spawn has none).
        let adopted = replay;
        let replay = if replay {
            listener.set_replies_muted(true);
            let l = listener.clone();
            Some(pio::Replay { until: stream.end, on_live: Box::new(move || l.set_replies_muted(false)) })
        } else {
            None
        };
        let io = pio::start_with_replay(
            parts.sid.clone(),
            stream,
            Arc::clone(&term),
            listener,
            Arc::clone(&owner),
            events,
            |sender| {
                assert!(
                    query_reply
                        .set(Box::new(move |text: String| sender.send(pio::IoMsg::Input(text.into_bytes().into()))) as QueryReplySink)
                        .is_ok(),
                    "query-reply sink is set exactly once, here"
                );
            },
            replay,
        )?;

        let (label_tx, _label_rx_drop) = broadcast::channel::<String>(16);
        drop(_label_rx_drop);
        let (pin_tx, _pin_rx_drop) = broadcast::channel::<Option<(u16, u16)>>(16);
        drop(_pin_rx_drop);

        Ok(Arc::new(Self {
            session_id: parts.session_id,
            cwd: parts.cwd,
            program: parts.program,
            sandbox: SandboxSpec::Passthrough,
            pid: Some(parts.pid),
            killed: std::sync::atomic::AtomicBool::new(false),
            args: parts.args,
            term,
            pty_io: PtyIo::Ptyd(PtydSession {
                sid: parts.sid,
                io,
                owner,
                meta: Mutex::new(parts.meta),
                detached: std::sync::atomic::AtomicBool::new(false),
                adopted,
            }),
            events_tx,
            child_exited: std::sync::atomic::AtomicBool::new(false),
            active_subscriber: std::sync::atomic::AtomicU64::new(0),
            ever_attached: std::sync::atomic::AtomicBool::new(false),
            viewer_count: std::sync::atomic::AtomicUsize::new(0),
            active_cols: std::sync::atomic::AtomicU16::new(0),
            active_rows: std::sync::atomic::AtomicU16::new(0),
            pinned_cols: std::sync::atomic::AtomicU16::new(0),
            pinned_rows: std::sync::atomic::AtomicU16::new(0),
            pinned_set_by: Mutex::new(None),
            pin_tx,
            ephemeral_pin_owner: std::sync::atomic::AtomicU64::new(0),
            viewports: Mutex::new(HashMap::new()),
            viewport_clock: std::sync::atomic::AtomicU64::new(0),
            resize_gate: Mutex::new(ResizeGate {
                pending: None,
                deadline: Instant::now(),
                flusher_live: false,
                last_applied: None,
            }),
            resizes_applied: std::sync::atomic::AtomicU64::new(0),
            resize_generation: std::sync::atomic::AtomicU64::new(0),
            label: std::sync::RwLock::new(parts.label),
            label_source: std::sync::RwLock::new(parts.label_source),
            label_tx,
        }))
    }

    /// k2-ptyd S3 (PRD §7.7, §7.9 B1, P34, P42 replay-only, P75): rebuild
    /// a session a previous daemon left running in ptyd. The agent process
    /// is untouched: this attaches from the ring's start, rebuilds the Term
    /// from the replayed bytes with terminal-query replies muted, and
    /// returns once the replay reached the live offset (bounded by
    /// `replay_wait`), so no subscriber ever sees replayed events. The
    /// session keeps its original `SessionId`, pid and durable args; the
    /// caller registers it under the key in `info.meta`.
    ///
    /// Full screen restore from a saved grid (S4) is not here: replay-only
    /// is the first ship (R3).
    #[cfg(unix)]
    pub fn adopt(
        info: &k2_ptyd::proto::SessionInfo,
        owner: Arc<crate::terminal::ptyd::client::Owner>,
        replay_wait: std::time::Duration,
    ) -> Result<Arc<Self>, io::Error> {
        let m = &info.meta;
        let session_id = SessionId::parse(&info.sid)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("adopt: sid {} is not a session id", info.sid)))?;
        let str_of = |k: &str| m.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let args: Vec<String> = m
            .get("durable_args")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let label_source = match str_of("label_source").as_deref() {
            Some("Locked") => LabelSource::Locked,
            Some("Seed") => LabelSource::Seed,
            _ => LabelSource::Pty,
        };
        let events = owner.watch(&info.sid);
        // S4 (P42): a saved screen whose offset the ring still holds →
        // restore it and replay only the tail; otherwise replay the ring.
        let snap = crate::terminal::ptyd::snapshot::read(&info.sid, info.cols.max(1) as usize, info.rows.max(1) as usize).filter(|s| s.ring_offset >= info.ring_start && s.ring_offset <= info.ring_end);
        let attach = |from: u64| match owner.attach(&info.sid, from) {
            Ok(s) => Ok(s),
            Err(e) => {
                owner.unwatch(&info.sid);
                Err(e)
            }
        };
        let (stream, snap) = match snap {
            Some(s) => {
                let stream = attach(s.ring_offset)?;
                if stream.start == s.ring_offset {
                    (stream, Some(s))
                } else {
                    // The ring moved past the snapshot between list and
                    // attach: replay what it holds instead.
                    log_debug!("[ptyd] adopt session={}: ring passed the saved screen ({} > {}); replaying", info.sid, stream.start, s.ring_offset);
                    drop(stream);
                    (attach(0)?, None)
                }
            }
            None => (attach(0)?, None),
        };
        let restored = snap.is_some();
        let session = Self::from_ptyd_stream_restoring(
            PtydParts {
                session_id,
                sid: info.sid.clone(),
                pid: info.pid,
                cwd: str_of("cwd").map(PathBuf::from),
                program: str_of("program"),
                args,
                label: str_of("label").unwrap_or_default(),
                label_source,
                cols: info.cols,
                rows: info.rows,
                meta: m.clone(),
            },
            owner,
            events,
            stream,
            true,
            snap,
        )?;
        // P38: the grid-size pin comes back from meta.
        if let Some(pin) = m.get("pin").filter(|p| !p.is_null()) {
            let c = pin.get("cols").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            let r = pin.get("rows").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            if c > 0 && r > 0 {
                let by = pin.get("set_by").and_then(|v| v.as_str()).map(str::to_string);
                Self::set_pinned(&session, Some((c, r)), by);
            }
        }
        // Wait for the replay to reach the live offset (P75: replay runs
        // before `register`). A session still replaying after the bound is
        // registered anyway; its replies stay muted until the offset.
        if let PtyIo::Ptyd(p) = &session.pty_io {
            let t = Instant::now();
            while !p.io.replayed() && t.elapsed() < replay_wait {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        log_debug!(
            "[ptyd] adopted session={} pid={} ring={}..{} screen={}",
            info.sid,
            info.pid,
            info.ring_start,
            info.ring_end,
            if restored { "restored" } else { "replayed" }
        );
        Ok(session)
    }

    /// k2-ptyd S3 (§7.6 `KeepAgents`, P30 hand-off quiesce): let go of a
    /// ptyd session WITHOUT ending it — stop K2's IO loop (joined) and stop
    /// hearing its events. The agent keeps running in ptyd for the next
    /// daemon to adopt; after this `kill()` and `Drop` leave it alone.
    /// Returns `false` (and does nothing) for an in-process session.
    pub fn detach(&self) -> bool {
        #[cfg(unix)]
        if let PtyIo::Ptyd(p) = &self.pty_io {
            if p.detached.swap(true, std::sync::atomic::Ordering::AcqRel) {
                return true;
            }
            p.owner.unwatch(&p.sid);
            // S4 (P41, P78): the loop is stopped and joined and its sync
            // flushed; `safe_offset` is the first byte the grid hasn't seen.
            // The next daemon restores this Term and replays from there.
            if let Some(end) = p.io.stop().filter(|e| !e.exited) {
                let snap = crate::terminal::ptyd::snapshot::Snapshot::capture(&*self.term.lock(), end.safe_offset);
                match crate::terminal::ptyd::snapshot::write(&p.sid, &snap) {
                    Ok(()) => log_debug!("[ptyd] detach session={}: screen saved at offset {}", p.sid, end.safe_offset),
                    Err(e) => log_debug!("[ptyd] detach session={}: screen not saved: {e}", p.sid),
                }
            }
            return true;
        }
        false
    }

    /// S3: whether [`Self::detach`] ran.
    pub fn is_detached(&self) -> bool {
        #[cfg(unix)]
        if let PtyIo::Ptyd(p) = &self.pty_io {
            return p.detached.load(std::sync::atomic::Ordering::Acquire);
        }
        false
    }

    /// True when this daemon adopted the session from k2-ptyd after a
    /// restart (it survived the previous daemon); false for a session this
    /// daemon spawned, and always false in-process.
    pub fn is_ptyd_adopted(&self) -> bool {
        #[cfg(unix)]
        if let PtyIo::Ptyd(p) = &self.pty_io {
            return p.adopted;
        }
        false
    }

    /// The meta this session last sent to ptyd (`None` in-process).
    pub fn ptyd_meta(&self) -> Option<serde_json::Value> {
        #[cfg(unix)]
        if let PtyIo::Ptyd(p) = &self.pty_io {
            return Some(p.meta.lock().clone());
        }
        None
    }

    /// Subscribe to out-of-band label change events. Receivers see
    /// every label set that happened after they subscribed.
    /// Subscribed by the WS handler so multi-window sync works:
    /// when one window sets the label, other windows' subscribers
    /// wake and emit `LabelChanged` to their clients.
    pub fn subscribe_labels(&self) -> broadcast::Receiver<String> {
        self.label_tx.subscribe()
    }

    /// S7a — subscribe to pin-state changes. Receivers see every
    /// [`Self::set_pinned`] that happened after they subscribed:
    /// `Some((cols, rows))` on pin, `None` on clear. Subscribed by
    /// the grid-WS handler so every window's pin UI converges,
    /// mirroring [`Self::subscribe_labels`].
    pub fn subscribe_pins(&self) -> broadcast::Receiver<Option<(u16, u16)>> {
        self.pin_tx.subscribe()
    }

    /// S7a — current pin state. `Some((cols, rows))` while pinned,
    /// `None` otherwise. Lock-free (two atomic loads).
    pub fn pinned(&self) -> Option<(u16, u16)> {
        use std::sync::atomic::Ordering;
        let cols = self.pinned_cols.load(Ordering::Relaxed);
        let rows = self.pinned_rows.load(Ordering::Relaxed);
        if cols > 0 && rows > 0 {
            Some((cols, rows))
        } else {
            None
        }
    }

    /// S7a — who set the current pin ("owner" | connect-user
    /// username). `None` when unpinned.
    pub fn pinned_set_by(&self) -> Option<String> {
        self.pinned_set_by.lock().clone()
    }

    /// S7a — pin (or unpin) this session's PTY to fixed dims.
    ///
    /// - `Some((cols, rows))`: records the pin, broadcasts on
    ///   `pin_tx`, and applies the dims immediately through
    ///   [`Self::request_resize`] (whose clamp then holds the PTY
    ///   there no matter what any window does). Idempotent: a re-pin
    ///   to the CURRENT dims refreshes attribution only — no
    ///   broadcast, no resize window (companion T0 — the mobile
    ///   claim-pin re-fires on every keyboard transition).
    /// - `None`: clears the pin, broadcasts, and re-resizes to the
    ///   current active viewer's declared viewport if one exists —
    ///   falling back to the most-recently-seen viewport with known
    ///   dims (the same eligibility + recency policy as
    ///   [`elect_on_detach`]) — so unpinning "clears back to the
    ///   juggling" UX cleanly. With no eligible viewer the dims are
    ///   left as-is (a reattaching viewer claims + resizes anyway).
    ///
    /// Takes `&Arc<Self>` because the resize goes through the
    /// debounced front door, which may park a flusher thread.
    pub fn set_pinned(
        session: &Arc<Self>,
        dims: Option<(u16, u16)>,
        set_by: Option<String>,
    ) {
        use std::sync::atomic::Ordering;
        // Companion T0 — any explicit pin write supersedes ephemeral
        // ownership (last write wins): a normal pin replaces an
        // ephemeral one and an unpin clears it. `set_pinned_ephemeral`
        // re-stamps the owner after delegating here.
        session.ephemeral_pin_owner.store(0, Ordering::Relaxed);
        match dims {
            Some((cols, rows)) => {
                let cols = cols.max(1);
                let rows = rows.max(1);
                // Companion T0 — idempotent re-pin. The mobile
                // claim-pin re-fires on every keyboard show/hide;
                // identical dims must neither re-broadcast (event
                // spam) nor reopen a resize window. Attribution still
                // refreshes (a different setter taking over the same
                // geometry) and the clamp is re-asserted through
                // `request_resize`'s same-dims skip below.
                if session.pinned() == Some((cols, rows)) {
                    *session.pinned_set_by.lock() = set_by;
                    Self::request_resize(session, cols, rows);
                    session.sync_ptyd_meta();
                    return;
                }
                // rows first, cols second: `pinned()` keys "pinned"
                // on cols ≠ 0, so a racing reader never observes
                // (new cols, unset rows).
                session.pinned_rows.store(rows, Ordering::Relaxed);
                session.pinned_cols.store(cols, Ordering::Relaxed);
                *session.pinned_set_by.lock() = set_by;
                // Best-effort broadcast (Err = zero subscribers).
                let _ = session.pin_tx.send(Some((cols, rows)));
                log_debug!(
                    "[v2-pin] session={} pinned to {}x{}",
                    session.session_id,
                    cols,
                    rows,
                );
                // Apply now. The clamp below maps any target to the
                // pinned dims, so passing them directly is exact.
                Self::request_resize(session, cols, rows);
            }
            None => {
                // cols first so `pinned()` flips to unpinned before
                // rows clears (mirror of the pin ordering above).
                session.pinned_cols.store(0, Ordering::Relaxed);
                session.pinned_rows.store(0, Ordering::Relaxed);
                *session.pinned_set_by.lock() = None;
                let _ = session.pin_tx.send(None);
                // Restore the live viewers' geometry: prefer the
                // active claimer's declared viewport, else the most
                // recent eligible one (elect_on_detach's policy).
                let restore = {
                    let reg = session.viewports.lock();
                    let active =
                        session.active_subscriber.load(Ordering::Relaxed);
                    reg.get(&active)
                        .filter(|vp| vp.cols > 0 && vp.rows > 0)
                        .map(|vp| (vp.cols, vp.rows))
                        .or_else(|| {
                            reg.iter()
                                .filter(|(_, vp)| vp.cols > 0 && vp.rows > 0)
                                .max_by_key(|(id, vp)| (vp.last_seen, **id))
                                .map(|(_, vp)| (vp.cols, vp.rows))
                        })
                };
                log_debug!(
                    "[v2-pin] session={} unpinned (restore={:?})",
                    session.session_id,
                    restore,
                );
                if let Some((cols, rows)) = restore {
                    Self::request_resize(session, cols, rows);
                }
            }
        }
        session.sync_ptyd_meta();
    }

    /// Companion T0 — the grid-WS subscriber id that owns the current
    /// ephemeral pin; `0` when the pin (if any) is a normal one (or
    /// there is no pin). Lock-free.
    pub fn ephemeral_pin_owner(&self) -> u64 {
        self.ephemeral_pin_owner
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Companion T0 — EPHEMERAL "claim session" pin: pin this
    /// session's PTY to `cols`×`rows` on behalf of the grid-WS
    /// subscriber `subscriber_id` (the socket IS the identity).
    ///
    /// Semantics are [`Self::set_pinned`]'s — broadcast on `pin_tx`,
    /// immediate apply, inescapable clamp in [`Self::request_resize`],
    /// idempotent on identical dims — plus the ownership stamp:
    ///
    /// - When `subscriber_id`'s socket detaches,
    ///   [`Self::detach_subscriber`] auto-clears the pin (broadcasting
    ///   the cleared state) and the normal election restores the
    ///   survivors' geometry.
    /// - Re-claiming on the same socket updates the dims in place
    ///   (mobile keyboard show/hide, rotation) — one broadcast per
    ///   real change, none for identical dims.
    /// - A later plain [`Self::set_pinned`] — a normal pin from the
    ///   pin-size route OR a manual unpin — supersedes the ownership
    ///   (last write wins), so today's unpin rights are unchanged.
    ///
    /// Never persisted: an ephemeral pin's owner cannot outlive the
    /// daemon, so restart-restore only ever resurrects normal pins.
    pub fn set_pinned_ephemeral(
        session: &Arc<Self>,
        cols: u16,
        rows: u16,
        set_by: Option<String>,
        subscriber_id: u64,
    ) {
        Self::set_pinned(session, Some((cols, rows)), set_by);
        // Stamp AFTER the delegate (which zeroes the slot). No writer
        // race: only `subscriber_id`'s own WS task calls this, and its
        // detach (the only reader that clears on this id) runs after
        // that task's loop exits.
        session
            .ephemeral_pin_owner
            .store(subscriber_id, std::sync::atomic::Ordering::Relaxed);
    }

    /// Read the authoritative label. Cheap (RwLock read + clone of
    /// the inner String). Caller gets an owned String — the lock is
    /// released before return.
    pub fn label(&self) -> String {
        self.label.read().expect("label rwlock poisoned").clone()
    }

    /// Read the current label source. Cheap.
    pub fn label_source(&self) -> LabelSource {
        *self.label_source.read().expect("label_source rwlock poisoned")
    }

    /// Update the label from a PTY title event. Honors `label_source`:
    /// `Pty` and `Seed` allow the update; `Locked` ignores it.
    /// Broadcasts on `label_tx` (so every WS subscriber, not just
    /// the one whose AlacEvent loop received the Title, emits
    /// `LabelChanged` to its client). Returns the new label if it
    /// actually changed; returns `None` if locked or a no-op.
    pub fn try_set_label_from_pty(&self, title: String) -> Option<String> {
        let source = self.label_source();
        if matches!(source, LabelSource::Locked) {
            return None;
        }
        let mut guard = self.label.write().expect("label rwlock poisoned");
        if *guard == title {
            return None;
        }
        *guard = title.clone();
        drop(guard);
        // Broadcast so multi-window peers converge. Best-effort —
        // `send` returns `Err` only when there are zero subscribers.
        let _ = self.label_tx.send(title.clone());
        Some(title)
    }

    /// Explicit caller-driven label set. Used by the new
    /// `/cli/sessions/<id>/label` endpoint and any other path where
    /// a human or another process has decided what the label should
    /// be. `lock=true` flips `label_source` to `Locked` so future
    /// PTY title events can't undo this. Always succeeds, returns
    /// the new label. Broadcasts on `label_tx` so every WS
    /// subscriber emits `LabelChanged` to its client (multi-window
    /// convergence).
    pub fn set_label(&self, label: String, lock: bool) -> String {
        {
            let mut guard = self.label.write().expect("label rwlock poisoned");
            *guard = label.clone();
        }
        if lock {
            *self
                .label_source
                .write()
                .expect("label_source rwlock poisoned") = LabelSource::Locked;
        }
        // Best-effort broadcast — `send` returns `Err` only when
        // there are zero subscribers, which is fine.
        let _ = self.label_tx.send(label.clone());
        self.sync_ptyd_meta();
        label
    }

    /// k2-ptyd P28: push the current label and pin into the session's
    /// ptyd meta (what adoption will restore). Fire-and-forget; a no-op for
    /// in-process sessions. PTY-title label changes are deliberately not
    /// pushed (TUIs retitle constantly and re-emit after adoption anyway).
    fn sync_ptyd_meta(&self) {
        #[cfg(unix)]
        if let PtyIo::Ptyd(p) = &self.pty_io {
            let mut m = p.meta.lock();
            if let Some(obj) = m.as_object_mut() {
                obj.insert("label".into(), serde_json::Value::String(self.label()));
                obj.insert("label_source".into(), serde_json::Value::String(format!("{:?}", self.label_source())));
                obj.insert(
                    "pin".into(),
                    match self.pinned() {
                        Some((c, r)) => serde_json::json!({"cols": c, "rows": r, "set_by": self.pinned_set_by()}),
                        None => serde_json::Value::Null,
                    },
                );
            }
            p.owner.send_nowait(k2_ptyd::proto::Request::UpdateMeta { sid: p.sid.clone(), meta: m.clone() });
        }
    }

    /// Whether the child is still alive for "is this session live?" probes.
    ///
    /// Returns `false` once the alacritty `ChildExit` event has been observed
    /// (`mark_child_exited`). Also returns `false` if we have a recorded PID
    /// and the OS says that process is gone (kill(pid, 0) fails) — covers
    /// KillMode/cgroup/SIGKILL paths where ChildExit never reaches the
    /// observer and the map would otherwise report phantom live sessions
    /// (scout 0.40.78 list finding).
    pub fn is_child_alive(&self) -> bool {
        if self.child_exited.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        #[cfg(unix)]
        {
            if let Some(pid) = self.child_pid() {
                if pid > 0 {
                    // 0 signal = existence check; ESRCH ⇒ process is gone.
                    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
                    if rc != 0 {
                        return false;
                    }
                }
            }
        }
        #[cfg(windows)]
        {
            if let Some(pid) = self.child_pid().and_then(|p| u32::try_from(p).ok()).filter(|p| *p > 0) {
                if !crate::win_proc::pid_alive(pid) {
                    return false;
                }
            }
        }
        true
    }

    /// Flip the `child_exited` flag. Called by the child-exit
    /// observer in the daemon (`v2_spawn::spawn_child_exit_observer`)
    /// when an `AlacEvent::ChildExit` is received. Idempotent —
    /// repeated calls are no-ops.
    pub fn mark_child_exited(&self) {
        self.child_exited
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Write input bytes to the child's stdin. Used for user
    /// keystrokes AND heartbeat-injected signals. Non-blocking.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        match &self.pty_io {
            PtyIo::Alac(n) => n.lock().notify(bytes),
            #[cfg(unix)]
            PtyIo::Ptyd(p) => p.io.write(bytes.into()),
        }
    }

    /// `"in-process"` or `"ptyd"` (diagnostics, tests, `/boot-status`).
    pub fn backend_name(&self) -> &'static str {
        match &self.pty_io {
            PtyIo::Alac(_) => "in-process",
            #[cfg(unix)]
            PtyIo::Ptyd(_) => "ptyd",
        }
    }

    /// Resize the PTY (which SIGWINCHes the child) and the local
    /// Term grid. Idempotent if called with the same dimensions.
    ///
    /// 0.38.0 commits 8 + 9 — discard the visible grid *before*
    /// reflowing so full-screen TUIs that do in-place SIGWINCH redraws
    /// (notably Claude) don't leave stale chrome above the new render.
    ///
    /// Subtle: we use `goto(0,0) + ClearMode::Below` instead of the
    /// more obvious `ClearMode::All`. In non-alt-screen mode (some
    /// TUIs paint into the primary buffer; note current Claude
    /// `/tui fullscreen` DOES use the alt screen + mouse reporting —
    /// `?1049h` plus `?1000h`/`?1002h`/`?1003h`/`?1006h` — so this
    /// path covers the primary-buffer case only), `ClearMode::All`
    /// calls
    /// `grid.clear_viewport()` which **scrolls the visible content
    /// INTO scrollback** before clearing — exactly the unwanted
    /// "every resize appends a copy of the prompt to history"
    /// behavior we just fixed in commit 9. `ClearMode::Below` uses
    /// `grid.reset_region` which discards the cells outright. The
    /// TUI's SIGWINCH-triggered repaint then fills the clean canvas.
    /// Scrollback above the viewport remains untouched: real history
    /// (heartbeat summaries, conversation, acknowledgements) is
    /// preserved and scrollable as before.
    pub fn resize(&self, cols: u16, rows: u16) {
        use alacritty_terminal::vte::ansi::{ClearMode, Handler};

        // S7a pin clamp, belt-and-suspenders. All production resize
        // paths already route through `request_resize` (whose clamp
        // is the real chokepoint), but `resize()` stays public — a
        // direct caller must not be able to move a pinned PTY either.
        let (cols, rows) = match self.pinned() {
            Some(pinned) => pinned,
            None => (cols, rows),
        };
        let cols = cols.max(1);
        let rows = rows.max(1);

        // SIGWINCH the PTY. The child process will re-query
        // TIOCGWINSZ and repaint for the new dimensions.
        match &self.pty_io {
            PtyIo::Alac(n) => n.lock().on_resize(WindowSize {
                num_cols: cols,
                num_lines: rows,
                cell_width: 10,
                cell_height: 20,
            }),
            #[cfg(unix)]
            PtyIo::Ptyd(p) => {
                // TIOCSWINSZ happens in ptyd (it holds the master).
                if let Err(e) = p.owner.request(k2_ptyd::proto::Request::Resize { sid: p.sid.clone(), cols, rows }) {
                    log_debug!("[ptyd] resize failed session={} err={}", self.session_id, e);
                }
            }
        }

        let mut term = self.term.lock();

        // 0.38.0 commit 8/9 + 0.38.1 fix — only clear when dimensions
        // actually change. Previously we cleared unconditionally,
        // which was fine on real resizes but BROKE same-size resize
        // calls (menu opens, focus transitions, observer noise):
        // kernel doesn't SIGWINCH for a same-size resize → TUI
        // doesn't redraw → we cleared the grid → user sees a black
        // screen until the next interaction triggers Claude to
        // repaint. Gating the clear on a real dimension change
        // preserves the no-duplication win for true resizes while
        // making same-size calls a true no-op.
        let dims_changed = (term.columns() as u16) != cols
            || (term.screen_lines() as u16) != rows;

        if dims_changed {
            // Announce the real resize BEFORE the clear, inside the
            // same term-lock critical section: the emitter reads the
            // generation under this lock, so it either observes
            // (old gen, pre-clear content) or (new gen, cleared/
            // reshaped content) — never a blank grid it would treat
            // as an ordinary frame. See `resize_generation`.
            self.resize_generation
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            // Discard-then-reshape inside the same lock so the grid
            // never observes pre-clear stale chrome at new dims.
            term.goto(0, 0);
            term.clear_screen(ClearMode::Below);
            term.resize(TermSize {
                cols: cols as usize,
                rows: rows as usize,
            });
        }
    }

    /// Monotonic count of real (dims-changing) resizes applied. The
    /// grid emitter compares successive reads to detect "a resize
    /// just cleared the viewport" and suppress blank-frame broadcast
    /// until the child's repaint (or a hard timeout). For an exact
    /// read, sample while holding the Term lock — `resize()` bumps
    /// this inside its own term-lock critical section.
    pub fn resize_generation(&self) -> u64 {
        self.resize_generation
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Record (or refresh) a subscriber's last-declared viewport +
    /// recency. Called by the grid-WS handler on every `SetActive`
    /// claim that carries dims and on every `Resize` frame — including
    /// ones dropped for being non-active, since the subscriber still
    /// declared what size IT wants; that declaration is exactly what
    /// the detach election restores to.
    pub fn note_viewport(&self, subscriber_id: u64, cols: u16, rows: u16) {
        let stamp = self
            .viewport_clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        self.viewports.lock().insert(
            subscriber_id,
            SubscriberViewport {
                cols,
                rows,
                last_seen: stamp,
            },
        );
    }

    /// Grid-WS disconnect hook (orca study takeaway #1 — restore the
    /// PTY size when the active viewer detaches). Removes the
    /// departing subscriber's viewport record on ANY disconnect; when
    /// the departing connection held the active claim, promotes the
    /// most-recently-active surviving subscriber with known dims and
    /// resizes the PTY to THEIRS (through the debouncer, so a
    /// disconnect burst still coalesces). Without this, a session a
    /// phone or second desktop briefly commandeered stays stuck at
    /// that viewer's dimensions forever.
    ///
    /// No eligible survivor ⇒ the claim clears to 0 ("no claim" —
    /// first resize from a reattaching viewer wins, the pre-existing
    /// disconnect semantics) and the dims are left as-is.
    pub fn detach_subscriber(session: &Arc<Self>, subscriber_id: u64) {
        use std::sync::atomic::Ordering;

        let successor = {
            let mut reg = session.viewports.lock();
            reg.remove(&subscriber_id);
            let current = session.active_subscriber.load(Ordering::Relaxed);
            elect_on_detach(&reg, subscriber_id, current)
        };
        // Companion T0 — ephemeral claim-pin auto-clear. If the
        // DEPARTING subscriber owns the ephemeral pin, clear it now:
        // AFTER the registry drop above (so the unpin restore can't
        // resurrect the departed viewport) and BEFORE the election
        // apply below (so the promotion resize isn't clamped by a pin
        // whose owner just left). `set_pinned(None)` broadcasts the
        // cleared `pin_changed` and restores the survivors' geometry;
        // the CAS lets a manual unpin / replacement pin that raced
        // this detach win cleanly (no double-clear, no spurious event).
        if session
            .ephemeral_pin_owner
            .compare_exchange(
                subscriber_id,
                0,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            Self::set_pinned(session, None, None);
        }
        match successor {
            Some((id, vp)) => {
                // CAS from the departing id so a viewer that claimed
                // between the election and this store isn't clobbered
                // — their claim already resized the PTY; honor it.
                if session
                    .active_subscriber
                    .compare_exchange(
                        subscriber_id,
                        id,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    session.active_cols.store(vp.cols, Ordering::Relaxed);
                    session.active_rows.store(vp.rows, Ordering::Relaxed);
                    log_debug!(
                        "[v2-perf] side=daemon stage=active_detach_promote \
                         session={} departed={} promoted={} cols={} rows={} \
                         (restored PTY to survivor dims)",
                        session.session_id,
                        subscriber_id,
                        id,
                        vp.cols,
                        vp.rows,
                    );
                    Self::request_resize(session, vp.cols, vp.rows);
                }
            }
            None => {
                // Departing viewer wasn't active, or nobody eligible
                // survives. Release the claim if we held it — CAS so a
                // concurrent takeover isn't accidentally cleared
                // (identical to the pre-election disconnect behavior).
                let _ = session.active_subscriber.compare_exchange(
                    subscriber_id,
                    0,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
        }
    }

    /// Debounced/coalescing front door for size arbitration (orca
    /// study takeaway #3). Racing claims from two clients or a phone
    /// keyboard show/hide used to thrash the PTY through back-to-back
    /// resize+reflow cycles. Requests landing inside the
    /// [`RESIZE_DEBOUNCE_MS`] window after an applied resize coalesce
    /// into ONE trailing apply of the newest geometry (the final
    /// target is never lost — it sits in `pending` until the flusher
    /// fires). An isolated request — including the very first fit of a
    /// fresh session — applies instantly. A target that matches the
    /// live PTY dims is skipped OUTRIGHT (Kessel postmortem — see the
    /// comment in the body).
    ///
    /// The gate lock is held across the underlying [`Self::resize`],
    /// so a leading apply and a trailing flush can never land out of
    /// order, and `resize()`'s discard-viewport-before-reflow behavior
    /// applies to the final geometry unchanged. The flusher is a plain
    /// std thread (not a tokio task) so this is callable from any
    /// context — WS loop, disconnect teardown, sync tests — and only
    /// exists for ~120ms per burst.
    pub fn request_resize(session: &Arc<Self>, cols: u16, rows: u16) {
        // S7a pin clamp — THE chokepoint. While pinned, every
        // incoming target is replaced by the pinned dims, so all
        // four resize paths (grid-WS Resize, typing input-claim
        // snap, SetActive claim, detach-promotion) reduce to the
        // same-dims skip below and the PTY never leaves the pin.
        let (cols, rows) = match session.pinned() {
            Some(pinned) => pinned,
            None => (cols, rows),
        };
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut gate = session.resize_gate.lock();

        // Kessel postmortem — a NO-OP RESIZE IS NOT FREE. Applying a
        // resize whose cols/rows match the live PTY size still cleared
        // the grid + set the winsize, and TUIs correctly do NOT
        // repaint on an unchanged winsize → blank screen until the
        // next output. So when the newest target already matches the
        // current dims — notably when a departing active viewer and
        // the promoted survivor shared a geometry — apply NOTHING:
        // drop any older pending target (this request supersedes it)
        // and leave the gate untouched (no window opens, no
        // `last_applied` stamp, no SIGWINCH).
        let current = Self::current_dims(session);
        if (cols, rows) == current {
            gate.pending = None;
            return;
        }

        let now = Instant::now();
        let window = Duration::from_millis(RESIZE_DEBOUNCE_MS);
        let in_window = gate.flusher_live
            || gate
                .last_applied
                .map_or(false, |t| now.duration_since(t) < window);
        if !in_window {
            gate.last_applied = Some(now);
            session
                .resizes_applied
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            session.resize(cols, rows);
            return;
        }
        gate.pending = Some((cols, rows));
        gate.deadline = now + window;
        if !gate.flusher_live {
            gate.flusher_live = true;
            // Weak so a parked flusher can't extend a torn-down
            // session's life; if the session dies mid-burst there's
            // nothing left worth resizing.
            let weak = Arc::downgrade(session);
            std::thread::spawn(move || loop {
                let sleep_for = {
                    let Some(s) = weak.upgrade() else { return };
                    let mut gate = s.resize_gate.lock();
                    let now = Instant::now();
                    if now < gate.deadline {
                        // A newer request extended the deadline while
                        // we slept — park again until it passes.
                        gate.deadline - now
                    } else {
                        gate.flusher_live = false;
                        if let Some((c, r)) = gate.pending.take() {
                            // Same-dims re-check: a direct `resize()`
                            // caller may have moved the PTY to the
                            // pending target while we were parked — a
                            // no-op apply here would blank the TUI
                            // (see the skip above). All production
                            // routes (grid-WS + HTTP) now come through
                            // this front door, but resize() stays
                            // callable directly.
                            if (c, r) != Self::current_dims(&s) {
                                gate.last_applied = Some(now);
                                s.resizes_applied.fetch_add(
                                    1,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                s.resize(c, r);
                            }
                        }
                        return;
                    }
                    // `s` (the upgraded Arc) drops here, before the
                    // sleep below — the flusher never holds the
                    // session across its park.
                };
                std::thread::sleep(sleep_for);
            });
        }
    }

    /// Attach-critical resize: pin clamp + same-dims skip, but
    /// **always** applies synchronously — never parks in the trailing
    /// [`RESIZE_DEBOUNCE_MS`] window. Used by spawn-reuse fit and grid
    /// attach pre-snap so the spawn response / first snapshot is
    /// already at the target size even if a resize landed in the last
    /// ~120ms. Ordinary Resize / SetActive traffic continues to use
    /// [`Self::request_resize`] (burst coalescing stays intact).
    ///
    /// Cancels any coalesced `pending` target (this call is the
    /// authoritative final geometry). Returns live PTY dims after the
    /// call (pin-clamped when pinned).
    pub fn apply_resize_now(session: &Arc<Self>, cols: u16, rows: u16) -> (u16, u16) {
        let (cols, rows) = match session.pinned() {
            Some(pinned) => pinned,
            None => (cols, rows),
        };
        let cols = cols.max(1);
        let rows = rows.max(1);
        // Hold the gate across the apply so a trailing flusher cannot
        // interleave with (or re-apply after) this sync land.
        let mut gate = session.resize_gate.lock();
        let current = Self::current_dims(session);
        if (cols, rows) == current {
            // Drop any older pending target — live size already matches.
            gate.pending = None;
            return current;
        }
        gate.pending = None;
        gate.last_applied = Some(Instant::now());
        session
            .resizes_applied
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        session.resize(cols, rows);
        Self::current_dims(session)
    }

    /// Live PTY/Term dims, for the same-dims resize skip. Briefly
    /// locks the Term (same FairMutex the snapshot path uses).
    fn current_dims(session: &Self) -> (u16, u16) {
        let term = session.term.lock();
        (term.columns() as u16, term.screen_lines() as u16)
    }

    /// How many resizes the debounced front door has actually applied.
    /// Skipped no-ops and coalesced-away intermediates don't count.
    pub fn resizes_applied(&self) -> u64 {
        self.resizes_applied
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Handle to the daemon-side alacritty Term. Locked briefly
    /// by the WS handler to serialize grid state into
    /// `TermGridSnapshot` / `TermGridDelta` payloads.
    pub fn term(&self) -> Arc<FairMutex<Term<DaemonEventListener>>> {
        Arc::clone(&self.term)
    }

    /// Plain-text projection of the visible grid — one `String` per
    /// viewport row, styles dropped, trailing whitespace trimmed.
    ///
    /// 0.39.45 (GH #38): used by the daemon's verified message
    /// injection to check whether the recipient TUI's input box still
    /// holds an un-submitted payload after the submit CR was written.
    /// Briefly locks the Term (same FairMutex the WS snapshot path
    /// uses), so it's safe to call from any thread.
    pub fn visible_text_rows(&self) -> Vec<String> {
        let term = self.term.lock();
        // Every row trimmed, wrapped or not: byte-identical to the
        // pre-VisibleRow projection the inject verifier relies on.
        visible_rows_of(&term)
            .into_iter()
            .map(|r| r.text.trim_end().to_string())
            .collect()
    }

    /// The visible grid rows with alacritty's soft-wrap flag, so a
    /// caller can rebuild logical lines (a long URL the terminal
    /// wrapped across rows). See [`visible_rows_of`].
    pub fn visible_rows(&self) -> Vec<VisibleRow> {
        let term = self.term.lock();
        visible_rows_of(&term)
    }

    /// Visible rows plus up to `history` scrollback rows, with OSC 8
    /// link targets. See [`scan_rows_of`].
    pub fn scan_rows(&self, history: usize) -> Vec<ScanRow> {
        let term = self.term.lock();
        scan_rows_of(&term, history)
    }

    /// True when the child application has bracketed-paste mode
    /// enabled (`ESC[?2004h` — claude/cursor TUIs switch it on at
    /// startup). 0.39.45 (GH #38): when active, injected message
    /// bodies are wrapped in explicit paste markers so a trailing CR
    /// is unambiguously a submit keystroke even if the child reads
    /// the whole burst in one coalesced `read()` under host load.
    pub fn bracketed_paste_active(&self) -> bool {
        self.term
            .lock()
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
    }

    /// Subscribe to this session's alacritty event broadcast.
    /// Each call returns a fresh `Receiver`; multiple subscribers
    /// can coexist (though v2 is single-subscriber in practice).
    ///
    /// Why broadcast rather than an owned mpsc: remount scenarios
    /// (workspace swap, Tauri window reload) unmount the old
    /// subscriber while the next one is already connecting. A
    /// take-once receiver loses the race on those transitions;
    /// broadcast avoids the ownership handoff entirely.
    ///
    /// A subscriber that lags beyond the channel capacity gets
    /// `RecvError::Lagged(n)` and can either skip ahead or
    /// disconnect. Consumers should treat Wakeup as idempotent —
    /// missing one just means the next one produces an emit that
    /// covers the accumulated damage.
    pub fn subscribe_events(&self) -> broadcast::Receiver<AlacEvent> {
        self.events_tx.subscribe()
    }

    /// Number of REAL viewers currently attached to this session —
    /// the count of live [`ViewerRegistration`] guards from
    /// [`Self::attach_viewer`].
    ///
    /// INVARIANT (2026-07-02 PTY-leak incident, Bug 2): this counts
    /// grid-WS viewer registrations, NEVER `events_tx` receivers.
    /// It used to read `events_tx.receiver_count()`, but the daemon's
    /// own internal observers (the child-exit observer for the
    /// session's whole life, the shared grid emitter from first
    /// attach) hold receivers on that channel too, so the count never
    /// reached its true zero — the GH#22 un-forced close guard saw
    /// "≥1 attached" forever and refused every reap. Internal code is
    /// free to `subscribe_events()` without affecting this signal;
    /// anything that should count as a viewer must go through
    /// [`Self::attach_viewer`].
    ///
    /// This is the authoritative "is a client attached?" signal the
    /// age-out reaper consults before killing a session (GH#22): a
    /// remote client attached over K2 Connect holds a registration,
    /// so the reaper must not reap a session with `subscriber_count() > 0`.
    /// It also agrees with `ever_attached` by construction:
    /// `attach_viewer` is the single write point for BOTH (the latch
    /// is exactly "viewer_count was ever incremented").
    pub fn subscriber_count(&self) -> usize {
        self.viewer_count.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Register a REAL viewer (a grid-WS connection) for the lifetime
    /// of the returned guard. Increments the viewer count and latches
    /// `ever_attached`; dropping the guard — on ANY WS exit path —
    /// decrements. The grid-WS handler is the only intended caller;
    /// internal daemon observers must use [`Self::subscribe_events`]
    /// alone so they never count as viewers (see
    /// [`Self::subscriber_count`] for the incident this pins).
    pub fn attach_viewer(self: &Arc<Self>) -> ViewerRegistration {
        self.viewer_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // PTY-leak breaker latch (see the `ever_attached` field doc):
        // latched here so "ever attached" and "attached now" share one
        // write point and can never disagree about what counts.
        self.ever_attached
            .store(true, std::sync::atomic::Ordering::Relaxed);
        ViewerRegistration {
            session: Arc::clone(self),
        }
    }

    /// Forcefully terminate AND reap the child process.
    ///
    /// **Why this exists.** Dropping the last `Arc<Self>` only closes
    /// the event-loop channel; alacritty's `Pty::Drop` then delivers a
    /// SINGLE SIGHUP to the direct child PID — no killpg, no SIGKILL,
    /// no waitpid. Agent CLIs (claude/codex/…) that ignore or outlive
    /// SIGHUP therefore orphan and accumulate (~200MB each → multi-GB
    /// leak + lag). This mirrors the v1 backend's correct two-phase
    /// kill + reap (`alacritty_backend::kill`).
    ///
    /// **Safety / blast radius.** The child was spawned with
    /// `setsid()` (alacritty `tty/unix.rs`), so it is its own
    /// session/process-group leader: `getpgid(pid) == pid`, a group
    /// DISTINCT from the daemon's. `killpg(pgid, …)` therefore reaches
    /// only the child and its descendants, never the daemon. We
    /// re-derive the pgid from the live PID (rather than trusting a
    /// cached value) and only `killpg` when `pgid == pid` confirms the
    /// child is still its own group leader; otherwise we fall back to a
    /// direct `kill(pid, …)` so we can never signal an unrelated group.
    ///
    /// **Idempotent + best-effort.** The first call runs the sequence
    /// and flips `killed`; subsequent calls are no-ops (so an explicit
    /// `kill()` followed by `Drop` doesn't `waitpid` a possibly-recycled
    /// PID). Every syscall failure is ignored (ESRCH = "already gone" is
    /// the normal, expected outcome once the child has exited).
    pub fn kill(&self) {
        use std::sync::atomic::Ordering;

        // Idempotency gate. compare_exchange so exactly one caller runs
        // the reap; others return immediately.
        if self
            .killed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        // k2-ptyd session: ptyd is the child's parent, so it signals and
        // reaps (same SIGHUP → 100 ms → SIGKILL → reap sequence, P18). Stop
        // K2's IO loop first, mirroring the in-process path where the reap
        // wins and the IO thread exits without a ChildExit.
        #[cfg(unix)]
        if let PtyIo::Ptyd(p) = &self.pty_io {
            // S3: a detached session belongs to the next daemon now (a
            // restart or hand-off); ending it here would kill the agent
            // the restart exists to keep.
            if p.detached.load(Ordering::Acquire) {
                log_debug!("[v2-kill] session={} is detached for the next daemon; left running", self.session_id);
                return;
            }
            p.io.stop();
            match p.owner.request(k2_ptyd::proto::Request::Kill { sid: p.sid.clone(), grace_ms: Some(100) }) {
                Ok(k2_ptyd::proto::Reply::Ok) => {}
                other => log_debug!("[ptyd] kill session={} -> {:?}", self.session_id, other),
            }
            let _ = p.owner.request(k2_ptyd::proto::Request::AckExit { sid: p.sid.clone() });
            p.owner.release_fd(&crate::terminal::ptyd::cell_listener_name(&p.sid));
            p.owner.unwatch(&p.sid);
            crate::terminal::ptyd::adopt::release_conversations_of(&p.sid);
            log_debug!("[v2-kill] session={} ptyd reaped child pid={:?}", self.session_id, self.pid);
            return;
        }

        let pid = match self.pid {
            Some(p) if p > 0 => p,
            _ => {
                // No PID captured — nothing to kill/reap, but the IO
                // thread (and the master fd it owns) must still be shut
                // down explicitly; see release_pty_master below.
                self.release_pty_master();
                return;
            }
        };

        #[cfg(unix)]
        unsafe {
            // Phase 1: SIGHUP to the child's own process group
            // (graceful). Re-derive the pgid from the live PID; only
            // killpg when the child is confirmed to still be its own
            // group leader (setsid invariant), else direct-kill.
            let pgid = libc::getpgid(pid);
            if pgid == pid {
                // Child is its own session/group leader — safe to
                // killpg the whole group (catches grandchildren too).
                if libc::killpg(pgid, libc::SIGHUP) != 0 {
                    libc::kill(pid, libc::SIGHUP);
                }
            } else {
                // pgid couldn't be read (ESRCH: already reaped) or the
                // child isn't a lone group leader — never killpg a
                // group we don't own; signal the PID directly.
                libc::kill(pid, libc::SIGHUP);
            }

            // Brief grace for cooperative shutdown before the hammer.
            std::thread::sleep(std::time::Duration::from_millis(100));

            // Phase 2: SIGKILL (forceful). Prefer the group again when
            // the child is still its own leader so any descendants that
            // survived SIGHUP also die; otherwise direct SIGKILL.
            let pgid2 = libc::getpgid(pid);
            if pgid2 == pid {
                if libc::killpg(pgid2, libc::SIGKILL) != 0 {
                    libc::kill(pid, libc::SIGKILL);
                }
            } else {
                libc::kill(pid, libc::SIGKILL);
            }

            // Reap to prevent a zombie. SIGKILL is async — the child
            // may not have been torn down by the kernel on the first
            // WNOHANG poll, so retry a few times. A return of >0 means
            // reaped; -1 means error (commonly ECHILD: already reaped
            // by alacritty's IO thread or never our child to wait on) —
            // either way we're done.
            let mut status: i32 = 0;
            for _ in 0..5 {
                let r = libc::waitpid(pid, &mut status, libc::WNOHANG);
                if r > 0 || r == -1 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }

        // Windows: no process groups or signals. End the child and its
        // descendants (an agent CLI's node/tool children) the way killpg
        // does on Unix; closing the ConPTY alone left them running.
        #[cfg(windows)]
        if let Ok(p) = u32::try_from(pid) {
            crate::win_proc::terminate_tree(p);
            // Let the exit reach the event loop before it is shut down
            // below: ChildExit is what the daemon's exit observer cleans up
            // on (quota, tokens, the session map). Unix gets the same
            // window from its SIGHUP grace. Done when the observer marks the
            // exit, or 150 ms after the process is gone (a session nobody
            // observes); never more than 1 s.
            let start = std::time::Instant::now();
            let mut gone_at: Option<std::time::Instant> = None;
            while !self.child_exited.load(std::sync::atomic::Ordering::Acquire)
                && start.elapsed() < std::time::Duration::from_secs(1)
            {
                if gone_at.is_none() && !crate::win_proc::pid_alive(p) {
                    gone_at = Some(std::time::Instant::now());
                }
                if gone_at.is_some_and(|t| t.elapsed() >= std::time::Duration::from_millis(150)) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }

        // 2026-07-02 PTY-master leak fix — release the master fd NOW.
        // AFTER the reap on purpose: the IO thread's `Pty::Drop` runs a
        // blocking `child.wait()`, which is instant (ECHILD) once the
        // child is reaped but would park the dying thread indefinitely
        // behind a SIGHUP-ignoring child if we shut down first.
        self.release_pty_master();

        log_debug!("[v2-kill] session={} reaped child pid={}", self.session_id, pid);
    }

    /// Make alacritty's IO thread exit so it drops the `Pty` — closing
    /// the PTY **master fd** — promptly and unconditionally.
    ///
    /// **Why this must be explicit.** The master fd is owned by the
    /// `EventLoop` inside the detached IO thread, and that thread exits
    /// on exactly two triggers: its own `try_wait` observes the child's
    /// exit, or it receives `Msg::Shutdown`. Neither is implied by
    /// teardown: dropping the last `Arc<Self>` only disconnects the
    /// channel, which alacritty's `drain_recv_channel` treats as "no
    /// messages" (no exit) and which never `poller.notify()`s the
    /// parked thread. And the try_wait trigger is LOST whenever
    /// `kill()`'s raw `waitpid` wins the reap race — `next_child_event`
    /// swallows the resulting ECHILD and returns `None`, so the loop
    /// never breaks. Every leaked path converged on the same picture:
    /// child gone, IO thread parked forever, `/dev/ptmx` master held
    /// until daemon exit — ~500 of them exhausted the box's PTY pool.
    ///
    /// The v1 backend always did this (`alacritty_backend::kill` sends
    /// `Msg::Shutdown`); v2 dropped it, same as the pid-reap regression
    /// documented on `pid`. Send failure is the GOOD case — the IO
    /// thread already exited on its own and closed the fd — so it is
    /// deliberately ignored. The Term (grid content, scrollback) is
    /// untouched: exited-session UX reads the Term, never the fd.
    fn release_pty_master(&self) {
        use alacritty_terminal::event_loop::Msg;
        match &self.pty_io {
            PtyIo::Alac(n) => {
                let _ = n.lock().0.send(Msg::Shutdown);
            }
            #[cfg(unix)]
            PtyIo::Ptyd(p) => {
                // Closing our data stream; ptyd keeps the master.
                p.io.stop();
            }
        }
    }

    /// PID of the direct child process, if captured. Exposed for
    /// tests that assert the child is actually gone after `kill()`.
    pub fn child_pid(&self) -> Option<i32> {
        self.pid
    }

    /// Test seam — a handle to the session's event broadcast SENDER,
    /// so tests can inject `AlacEvent`s (Wakeup / ChildExit) with
    /// deterministic timing instead of coaxing the real IO thread
    /// into producing them (real-time PTY echo is incompatible with
    /// paused-clock `tokio::time` cadence tests). Gated on
    /// `test-util` (which dependent crates' dev builds already
    /// enable) so production binaries never link it. Used by the
    /// grid-emitter frame-budget tests in `k2-daemon`.
    #[cfg(any(test, feature = "test-util"))]
    pub fn events_sender_for_tests(&self) -> broadcast::Sender<AlacEvent> {
        self.events_tx.clone()
    }
}

// Explicit `Drop`: forcefully kill + reap the child so agent-CLI
// children never orphan when the last `Arc<Self>` is dropped without
// an explicit `kill()` (e.g. a stray WS handler holding the final
// clone). `kill()` is idempotent, so this is a safe no-op when the
// session was already killed via an unregister/watchdog chokepoint.
impl Drop for DaemonPtySession {
    fn drop(&mut self) {
        self.kill();
    }
}

/// RAII proof that a REAL viewer (a grid-WS connection) is attached
/// to a session. Handed out by [`DaemonPtySession::attach_viewer`];
/// holding it keeps `subscriber_count()` ≥ 1, and dropping it — on
/// ANY WS exit path, clean or panicked — decrements the count. The
/// guard owns a strong `Arc` for the same reason the WS handler does:
/// an attached viewer must keep the session alive (GH#22 — a reap
/// mid-connection would SIGHUP the child out from under the client).
pub struct ViewerRegistration {
    session: Arc<DaemonPtySession>,
}

impl Drop for ViewerRegistration {
    fn drop(&mut self) {
        self.session
            .viewer_count
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Remove PATH entries that fail [`crate::terminal::path_env::is_safe_entry`] from a
/// child env (whichever key carries PATH). PURE. Returns how many were
/// dropped; a clean PATH is left byte-identical.
/// The child PATH every agent spawn gets (issue #15): the caller's own
/// PATH/`Path` if it set one, else the enriched login-shell + known
/// install dirs + inherited PATH; then any unusable entry dropped (a
/// multi-line or control-character entry blocks macOS execvp forever,
/// 0.45.0 smoke). Returns how many entries were dropped.
fn prepare_child_path(child_env: &mut HashMap<String, String>) -> usize {
    if !login_path::env_has_path(child_env) {
        let inherited = login_path::process_path();
        let enriched = login_path::augmented_path(&inherited);
        child_env.insert("PATH".to_string(), enriched);
    }
    drop_unsafe_path_entries(child_env)
}

/// Can an agent spawn exec `program` right now? (0.46.6: a pinned-chat
/// harness move checks this BEFORE it ends the old chat, so a missing CLI
/// never leaves the pinned chat empty.)
///
/// Resolves exactly as [`DaemonPtySession::spawn`] does: the same child
/// PATH ([`prepare_child_path`] over `env`, the preset's env), the same
/// test guard (`K2_TEST_AGENT_SHIM_DIR`: shim dirs only), then where exec
/// will find it — on Windows by cmd.exe's PATHEXT rule
/// ([`crate::terminal::win_cmd::which`]), elsewhere the first regular,
/// executable file on PATH. `Ok` is the file it would run.
pub fn preflight_program(program: &str, env: &HashMap<String, String>) -> Result<std::path::PathBuf, String> {
    let program = program.trim();
    if program.is_empty() {
        return Err("no program to run".to_string());
    }
    let mut child_env = env.clone();
    prepare_child_path(&mut child_env);
    let path = login_path::env_path_entry(&child_env).map(|(_, v)| v.to_string()).unwrap_or_default();
    let guard = crate::terminal::agent_spawn_guard::GuardEnv::from_process();
    let resolved = crate::terminal::agent_spawn_guard::resolve_program(program, &path, &guard)?;
    let has_dir = resolved.contains(['/', '\\']);
    #[cfg(windows)]
    let found = if has_dir {
        Some(std::path::PathBuf::from(&resolved)).filter(|p| p.is_file())
    } else {
        crate::terminal::win_cmd::which(&resolved, &path)
    };
    #[cfg(not(windows))]
    let found = {
        let _ = has_dir;
        crate::terminal::agent_spawn_guard::locate_on_path(&resolved, &path).filter(|p| is_runnable(p))
    };
    found.ok_or_else(|| format!("`{program}` is not installed: it is not on the PATH agents run with"))
}

#[cfg(unix)]
fn is_runnable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(all(not(unix), not(windows)))]
fn is_runnable(p: &std::path::Path) -> bool {
    p.is_file()
}

fn drop_unsafe_path_entries(child_env: &mut HashMap<String, String>) -> usize {
    let Some((key, value)) = login_path::env_path_entry(child_env)
        .map(|(k, v)| (k.to_string(), v.to_string()))
    else {
        return 0;
    };
    let (clean, dropped) = crate::terminal::path_env::drop_unsafe_entries(&value);
    if dropped > 0 {
        child_env.insert(key, clean);
    }
    dropped
}

/// 0.40.34 — wire the staged browser-open shim into a child env map.
/// PURE (no disk, no globals) so the injection rule is unit-testable:
///   - PATH: the shim's directory (`~/.k2/bin`) is PREPENDED via
///     [`crate::open_shim::prepend_bin_dir`] — prepend means FIRST in
///     `execvp` lookup order, so the staged `xdg-open` wins over any
///     system one. Idempotent (no duplicate segment on respawn). A
///     missing PATH key (can't happen after the enrichment above, but
///     belt) gets just the bin dir.
///   - BROWSER: filled with the shim's absolute path ONLY when the
///     caller didn't set one (`entry().or_insert_with` — caller wins).
fn apply_browser_shim_env(
    child_env: &mut HashMap<String, String>,
    shim: &std::path::Path,
) {
    if let Some(bin) = shim.parent() {
        // Prefer the existing PATH/`Path` value (Windows uses `Path`);
        // always write back under `PATH` so a single canonical key is set.
        let current = login_path::env_path_entry(child_env)
            .map(|(_, v)| v)
            .unwrap_or("");
        let merged = crate::open_shim::prepend_bin_dir(current, bin);
        // Drop a Windows-style `Path` sibling so we don't hand the child
        // two competing PATH values after enrichment.
        #[cfg(windows)]
        {
            child_env.retain(|k, _| !k.eq_ignore_ascii_case("PATH"));
        }
        child_env.insert("PATH".to_string(), merged);
    }
    child_env
        .entry("BROWSER".to_string())
        .or_insert_with(|| shim.to_string_lossy().into_owned());
}

#[cfg(test)]
#[path = "daemon_pty_tests.rs"]
mod tests;
