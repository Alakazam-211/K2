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
//! `.k2so/prds/alacritty-v2.md` for the product context.
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
/// depends on k2so-core; this keeps that surface honest.
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
}

impl EventListener for DaemonEventListener {
    fn send_event(&self, event: AlacEvent) {
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
    pub args: Vec<String>,
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
            env: HashMap::new(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
            sandbox: SandboxSpec::default(),
            cell_uid: None,
            overlay: None,
        }
    }
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
    pty_notifier: Mutex<Notifier>,

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
        // We clone args here because we also persist them on the
        // session below — used by smart-launch's "is there a live
        // PTY for this --resume <session_id>?" check.
        let spawn_args = cfg.args.clone();
        // Sandbox P2a: carry program+args as a readable `ShellSpec` (the seam's
        // `build_worker_invocation` needs to read them back; alacritty's `Shell`
        // fields are pub(crate)). `Passthrough::spawn` maps this back into
        // `tty::Shell::new` at the PTY open, so behavior is byte-identical.
        let shell = cfg.program.as_ref().map(|prog| ShellSpec {
            program: prog.clone(),
            args: spawn_args.clone(),
        });

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
        // so children spawned by bare name (`claude`, `cursor`,
        // `gemini`) can't find binaries in ~/.local/bin (the Claude
        // native installer default), /opt/homebrew/bin, nvm shims,
        // etc. — they ENOENT. Augment the child PATH with the union
        // of the user's login-shell PATH, known install dirs, and the
        // daemon's inherited PATH (helper is no-op / non-mutating on
        // non-unix). Respect a caller-provided PATH: if `cfg.env`
        // already set one explicitly, leave it untouched — only fill
        // in the enriched value when the caller passed none (which is
        // the common case: spawn.rs hands an empty env, so agents get
        // the enriched PATH).
        if !child_env.contains_key("PATH") {
            let inherited = std::env::var("PATH").unwrap_or_default();
            child_env.insert(
                "PATH".to_string(),
                login_path::augmented_path(&inherited),
            );
        }

        child_env
            .entry("TERM".to_string())
            .or_insert_with(|| "xterm-256color".to_string());
        child_env
            .entry("COLORTERM".to_string())
            .or_insert_with(|| "truecolor".to_string());
        child_env
            .entry("TERM_PROGRAM".to_string())
            .or_insert_with(|| "K2".to_string());

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
        let backend = cfg.sandbox.backend();

        // Window ID is used on macOS/Windows to associate the PTY
        // with a specific OS window for controlling-terminal
        // semantics. The daemon has no window, so the backend passes 0.
        let __t_pty = std::time::Instant::now();
        let SpawnedChild { pty, child_pid } = backend.spawn(req)?;

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
            args: spawn_args,
            term,
            pty_notifier: Mutex::new(Notifier(pty_sender)),
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
        label
    }

    /// Whether the child PID is still alive. Returns `false` once
    /// the alacritty `ChildExit` event has been observed and
    /// `mark_child_exited` was called (typically from the daemon's
    /// child-exit observer task). Used by the spawn helper's
    /// idempotency check and the `agents running` reaping pass to
    /// recognize stale entries before reporting them as live.
    pub fn is_child_alive(&self) -> bool {
        !self.child_exited.load(std::sync::atomic::Ordering::Acquire)
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
        self.pty_notifier.lock().notify(bytes);
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
        self.pty_notifier.lock().on_resize(WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: 10,
            cell_height: 20,
        });

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
        use alacritty_terminal::index::{Column, Line, Point};
        let term = self.term.lock();
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
            let trimmed = line.trim_end();
            line.truncate(trimmed.len());
            out.push(line);
        }
        out
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
        let _ = self.pty_notifier.lock().0.send(Msg::Shutdown);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_pty_config_default_is_80x24() {
        let cfg = DaemonPtyConfig::default();
        assert_eq!(cfg.cols, 80);
        assert_eq!(cfg.rows, 24);
        assert!(cfg.program.is_none());
        assert!(cfg.cwd.is_none());
        assert!(cfg.drain_on_exit);
    }

    #[test]
    fn spawn_uses_passthrough_by_default() {
        // Sandbox P1 default-OFF proof: a default config selects the
        // host-direct Passthrough backend, so every existing spawn path is
        // byte-identical to pre-seam behavior.
        let cfg = DaemonPtyConfig::default();
        assert_eq!(cfg.sandbox, SandboxSpec::Passthrough);
        assert_eq!(cfg.sandbox.backend().name(), "passthrough");
    }

    #[test]
    fn daemon_pty_config_default_label_state() {
        // Phase B: defaults are empty label + Pty source. PTY title
        // events fill the label.
        let cfg = DaemonPtyConfig::default();
        assert_eq!(cfg.label, "");
        assert_eq!(cfg.label_source, LabelSource::Pty);
    }

    #[test]
    fn term_size_dimensions_include_scrollback() {
        let size = TermSize {
            cols: 120,
            rows: 40,
        };
        assert_eq!(size.columns(), 120);
        assert_eq!(size.screen_lines(), 40);
        assert_eq!(size.total_lines(), 40 + SCROLLBACK_CAP);
    }

    // Phase B label-state-machine tests. These exercise the
    // `try_set_label_from_pty` / `set_label` / accessor surface
    // without spawning a real PTY — we hand-construct a session
    // and verify the state-machine logic in isolation. Requires a
    // tokio runtime because `broadcast::channel` lives inside a
    // tokio module, and we need a Term + EventLoop to satisfy the
    // struct layout. We use the simplest valid spawn (a `cat`
    // subprocess on Unix) under #[cfg(unix)] — the test exits
    // when the Arc drops at scope end.

    // 2026-07-02 PTY-leak incident (Bug 2) — the viewer-count
    // invariant, on ONE spawned PTY (budget-frugal): internal
    // `subscribe_events()` receivers (the child-exit observer /
    // grid-emitter shape) never count as viewers; only
    // `attach_viewer()` registrations do, the RAII guard decrements
    // on drop, and the `ever_attached` latch flips with the first
    // registration and stays latched.
    #[cfg(unix)]
    #[test]
    fn viewer_count_ignores_internal_receivers_and_tracks_registrations() {
        use std::path::PathBuf;
        use std::sync::atomic::Ordering;
        let cfg = DaemonPtyConfig {
            program: Some("cat".to_string()),
            cwd: Some(PathBuf::from("/tmp")),
            ..Default::default()
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");

        // Internal observers: events receivers only — must be invisible
        // to the viewer signal (pre-fix these inflated it forever).
        let _internal_rx = s.subscribe_events();
        let _internal_rx2 = s.subscribe_events();
        assert_eq!(
            s.subscriber_count(),
            0,
            "internal subscribe_events() receivers must not count as viewers"
        );
        assert!(
            !s.ever_attached.load(Ordering::Relaxed),
            "internal receivers must not latch ever_attached either"
        );

        // Real viewers: attach_viewer registrations count and latch.
        let reg1 = s.attach_viewer();
        let reg2 = s.attach_viewer();
        assert_eq!(s.subscriber_count(), 2, "each registration counts once");
        assert!(
            s.ever_attached.load(Ordering::Relaxed),
            "first registration must latch ever_attached"
        );

        // RAII: every drop decrements; the latch survives.
        drop(reg1);
        assert_eq!(s.subscriber_count(), 1, "drop must decrement");
        drop(reg2);
        assert_eq!(s.subscriber_count(), 0, "count must return to true zero");
        assert!(
            s.ever_attached.load(Ordering::Relaxed),
            "ever_attached must stay latched after all viewers detach"
        );

        s.kill();
    }

    #[cfg(unix)]
    #[test]
    fn label_starts_with_seed_and_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            sandbox: SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: "scout".to_string(),
            label_source: LabelSource::Locked,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        assert_eq!(s.label(), "scout");
        assert_eq!(s.label_source(), LabelSource::Locked);
    }

    #[cfg(unix)]
    #[test]
    fn try_set_label_from_pty_drops_locked() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            sandbox: SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: "scout".to_string(),
            label_source: LabelSource::Locked,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        // PTY would emit "Claude Code" — must be silently dropped.
        let result = s.try_set_label_from_pty("Claude Code".to_string());
        assert!(result.is_none(), "Locked label must reject PTY update");
        assert_eq!(s.label(), "scout", "label must not have changed");
    }

    #[cfg(unix)]
    #[test]
    fn try_set_label_from_pty_accepts_pty_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            sandbox: SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        let r1 = s.try_set_label_from_pty("vim README.md".to_string());
        assert_eq!(r1.as_deref(), Some("vim README.md"));
        assert_eq!(s.label(), "vim README.md");
        // Same value again is a no-op (returns None).
        let r2 = s.try_set_label_from_pty("vim README.md".to_string());
        assert!(r2.is_none(), "no-op rewrite must return None");
    }

    #[cfg(unix)]
    #[test]
    fn set_label_with_lock_locks_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            sandbox: SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            ..Default::default()
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        assert_eq!(s.label_source(), LabelSource::Pty);
        s.set_label("user-named".to_string(), true);
        assert_eq!(s.label(), "user-named");
        assert_eq!(s.label_source(), LabelSource::Locked);
        // Subsequent PTY update must be ignored.
        let r = s.try_set_label_from_pty("Claude Code".to_string());
        assert!(r.is_none());
        assert_eq!(s.label(), "user-named");
    }

    #[cfg(unix)]
    #[test]
    fn set_label_without_lock_keeps_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            sandbox: SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: "seed".to_string(),
            label_source: LabelSource::Seed,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        s.set_label("new-seed".to_string(), false);
        assert_eq!(s.label(), "new-seed");
        // Source preserved — PTY still allowed to update.
        assert_eq!(s.label_source(), LabelSource::Seed);
        let r = s.try_set_label_from_pty("vim".to_string());
        assert_eq!(r.as_deref(), Some("vim"));
    }

    /// Regression test for the v2 process/memory leak: `kill()` MUST
    /// forcefully terminate AND reap the child so no orphan + no zombie
    /// remains. We spawn a long-lived `sleep 600` (a process that will
    /// NOT exit on its own within the test), capture its PID, call
    /// `kill()`, and then prove via `kill(pid, 0)` that the PID is gone
    /// (ESRCH). The child was setsid()'d by alacritty so it's its own
    /// group leader — exactly the case `kill()`'s killpg targets.
    #[cfg(unix)]
    #[test]
    fn kill_terminates_and_reaps_child() {
        use std::path::PathBuf;

        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            sandbox: SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
            // A 600s sleep: will not self-exit during the test, so if
            // the PID is gone afterwards it's because kill() killed it.
            program: Some("sleep".to_string()),
            args: vec!["600".to_string()],
            env: Default::default(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn sleep");

        let pid = s
            .child_pid()
            .expect("child PID must be captured on unix");
        assert!(pid > 0, "captured PID must be positive, got {pid}");

        // Sanity: the child is alive right now (kill(pid, 0) → 0).
        let alive_before = unsafe { libc::kill(pid, 0) };
        assert_eq!(
            alive_before, 0,
            "freshly-spawned child pid={pid} must be alive before kill()"
        );

        // The fix under test.
        s.kill();

        // Poll for the PID to disappear. SIGKILL + our waitpid reap are
        // synchronous-ish, but allow a short window for the kernel to
        // finish teardown. We assert it IS gone — not "best effort".
        let mut gone = false;
        let mut last_errno = 0;
        for _ in 0..50 {
            let r = unsafe { libc::kill(pid, 0) };
            if r == -1 {
                last_errno = std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(0);
                if last_errno == libc::ESRCH {
                    gone = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            gone,
            "child pid={pid} must be gone after kill() (kill(pid,0) → ESRCH); \
             last kill(pid,0) errno was {last_errno} (0 = still alive). \
             A surviving PID means the leak fix did not terminate the child."
        );

        // Reap guarantee: the child must NOT be a zombie. We already
        // waitpid'd inside kill(); a second WNOHANG must therefore
        // return -1/ECHILD (no such child to wait on) rather than the
        // PID again (which would mean a zombie is still reapable here).
        let mut status: i32 = 0;
        let wr = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        assert!(
            wr <= 0,
            "no zombie may remain: waitpid(pid={pid}) returned {wr} \
             (>0 means an unreaped zombie child is still present)"
        );
    }

    // Resize-arbitration polish (orca study takeaways #1/#3/#8) —
    // the detach election is a pure function so these cases pin the
    // policy without a PTY: who becomes active when the active viewer
    // disconnects, and what dims the PTY restores to.

    fn vp(cols: u16, rows: u16, last_seen: u64) -> SubscriberViewport {
        SubscriberViewport {
            cols,
            rows,
            last_seen,
        }
    }

    #[test]
    fn elect_promotes_single_survivor() {
        let mut reg = HashMap::new();
        reg.insert(7, vp(100, 30, 1));
        // Active sub 9 departs; 7 is the only survivor with dims.
        let winner = elect_on_detach(&reg, 9, 9);
        assert_eq!(winner, Some((7, vp(100, 30, 1))));
    }

    #[test]
    fn elect_prefers_most_recent_of_many() {
        let mut reg = HashMap::new();
        reg.insert(3, vp(200, 50, 4));
        reg.insert(7, vp(100, 30, 9));
        reg.insert(8, vp(40, 10, 2));
        // 7 acted most recently (stamp 9) — it wins regardless of id
        // order or dims.
        let winner = elect_on_detach(&reg, 5, 5);
        assert_eq!(winner, Some((7, vp(100, 30, 9))));
    }

    #[test]
    fn elect_no_survivors_means_no_takeover() {
        let reg = HashMap::new();
        assert_eq!(elect_on_detach(&reg, 9, 9), None);
    }

    #[test]
    fn elect_noop_when_departing_was_not_active() {
        let mut reg = HashMap::new();
        reg.insert(7, vp(100, 30, 1));
        // Sub 3 departs while 9 holds the claim — nothing to restore;
        // the active viewer is untouched.
        assert_eq!(elect_on_detach(&reg, 3, 9), None);
    }

    #[test]
    fn elect_skips_departing_and_dimless_records() {
        let mut reg = HashMap::new();
        // Departing entry still present (caller hasn't removed it yet)
        // and a survivor that never declared dims — both ineligible.
        reg.insert(9, vp(40, 10, 8));
        reg.insert(5, vp(0, 0, 7));
        reg.insert(7, vp(100, 30, 2));
        let winner = elect_on_detach(&reg, 9, 9);
        assert_eq!(winner, Some((7, vp(100, 30, 2))));
    }

    #[cfg(unix)]
    fn spawn_cat_session() -> Arc<DaemonPtySession> {
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            ..Default::default()
        };
        DaemonPtySession::spawn(cfg).expect("spawn cat")
    }

    #[cfg(unix)]
    fn term_dims(s: &DaemonPtySession) -> (u16, u16) {
        let tm = s.term();
        let t = tm.lock();
        (t.columns() as u16, t.screen_lines() as u16)
    }

    /// Poll the Term until it reaches `want` or the deadline passes.
    /// Fails LOUDLY with the last observed dims — never a silent skip.
    #[cfg(unix)]
    fn assert_dims_settle(s: &DaemonPtySession, want: (u16, u16), what: &str) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got = term_dims(s);
        while got != want && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            got = term_dims(s);
        }
        assert_eq!(
            got, want,
            "{what}: PTY dims never settled to {want:?} (last saw {got:?})"
        );
    }

    #[cfg(unix)]
    #[test]
    fn first_request_resize_applies_instantly() {
        let s = spawn_cat_session();
        // Initial fit of a fresh session must never be debounced.
        DaemonPtySession::request_resize(&s, 100, 30);
        assert_eq!(
            term_dims(&s),
            (100, 30),
            "first resize must apply synchronously"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resize_burst_coalesces_to_final_target() {
        let s = spawn_cat_session();
        // Leading edge: the first of the burst applies instantly...
        DaemonPtySession::request_resize(&s, 100, 30);
        // ...then two more land inside the debounce window. The
        // intermediate 90x25 must never be applied; only the final
        // 40x10 flushes.
        DaemonPtySession::request_resize(&s, 90, 25);
        DaemonPtySession::request_resize(&s, 40, 10);
        assert_eq!(
            term_dims(&s),
            (100, 30),
            "burst members inside the window must be deferred, not applied"
        );
        assert_dims_settle(&s, (40, 10), "trailing flush");
    }

    #[cfg(unix)]
    #[test]
    fn same_dims_request_is_skipped_entirely() {
        let s = spawn_cat_session();
        // Kessel postmortem: target == live dims ⇒ nothing may be
        // applied — no grid clear, no winsize set, no debounce window
        // opened. (TUIs don't repaint on an unchanged winsize, so a
        // no-op apply blanks the screen until the next output.)
        DaemonPtySession::request_resize(&s, 80, 24);
        assert_eq!(s.resizes_applied(), 0, "same-dims request must be skipped");
        assert_eq!(term_dims(&s), (80, 24));
        // Because the skip opened no window, the next REAL resize
        // still applies instantly.
        DaemonPtySession::request_resize(&s, 100, 30);
        assert_eq!(s.resizes_applied(), 1);
        assert_eq!(
            term_dims(&s),
            (100, 30),
            "real resize after a skipped no-op must be instant"
        );
    }

    #[cfg(unix)]
    #[test]
    fn detach_promotion_with_same_dims_skips_resize() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_cat_session();
        // Survivor 7 and active viewer 9 declared the SAME viewport —
        // the common case of two same-sized desktop windows.
        s.note_viewport(7, 40, 10);
        s.note_viewport(9, 40, 10);
        s.active_subscriber.store(9, Relaxed);
        DaemonPtySession::request_resize(&s, 40, 10);
        assert_dims_settle(&s, (40, 10), "active claim");
        assert_eq!(s.resizes_applied(), 1);

        // 9 departs; 7 is promoted — but its dims already match the
        // live PTY size, so NO resize may be applied.
        DaemonPtySession::detach_subscriber(&s, 9);
        assert_eq!(s.active_subscriber.load(Relaxed), 7, "survivor promoted");
        // Outwait the debounce window: not even a deferred flush may
        // fire for the same-dims target.
        std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
        assert_eq!(
            s.resizes_applied(),
            1,
            "same-dims promotion must not apply a resize"
        );
        assert_eq!(term_dims(&s), (40, 10));
    }

    #[cfg(unix)]
    #[test]
    fn detach_of_active_viewer_restores_survivor_dims() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_cat_session();
        // Sub 7 (a desktop at 100x30) declared first; sub 9 (a phone
        // at 40x10) claimed active and shrank the PTY.
        s.note_viewport(7, 100, 30);
        s.note_viewport(9, 40, 10);
        s.active_subscriber.store(9, Relaxed);
        s.active_cols.store(40, Relaxed);
        s.active_rows.store(10, Relaxed);
        DaemonPtySession::request_resize(&s, 40, 10);
        assert_dims_settle(&s, (40, 10), "phone claim");

        // The phone disconnects: sub 7 is promoted and the PTY
        // restores to ITS dims.
        DaemonPtySession::detach_subscriber(&s, 9);
        assert_eq!(s.active_subscriber.load(Relaxed), 7, "survivor promoted");
        assert_eq!(s.active_cols.load(Relaxed), 100);
        assert_eq!(s.active_rows.load(Relaxed), 30);
        assert_dims_settle(&s, (100, 30), "restore on detach");
    }

    #[cfg(unix)]
    #[test]
    fn detach_of_non_active_viewer_changes_nothing() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_cat_session();
        s.note_viewport(7, 100, 30);
        s.note_viewport(9, 40, 10);
        s.active_subscriber.store(9, Relaxed);
        DaemonPtySession::request_resize(&s, 40, 10);
        assert_dims_settle(&s, (40, 10), "active claim");

        // A passive watcher leaving must not perturb the active claim
        // or the PTY size.
        DaemonPtySession::detach_subscriber(&s, 7);
        assert_eq!(s.active_subscriber.load(Relaxed), 9, "claim untouched");
        assert_eq!(term_dims(&s), (40, 10), "size untouched");
    }

    #[cfg(unix)]
    #[test]
    fn detach_of_last_viewer_clears_claim_and_keeps_dims() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_cat_session();
        s.note_viewport(9, 40, 10);
        s.active_subscriber.store(9, Relaxed);
        DaemonPtySession::request_resize(&s, 40, 10);
        assert_dims_settle(&s, (40, 10), "sole viewer claim");

        // Last viewer out: no survivor to restore to — claim clears to
        // "none" and dims stay put (a reattaching viewer will claim +
        // resize anyway).
        DaemonPtySession::detach_subscriber(&s, 9);
        assert_eq!(s.active_subscriber.load(Relaxed), 0, "claim released");
        assert_eq!(term_dims(&s), (40, 10), "dims left as-is");
    }

    #[cfg(unix)]
    #[test]
    fn resize_generation_bumps_only_on_real_dims_change() {
        let s = spawn_cat_session();
        assert_eq!(s.resize_generation(), 0);
        // Same-dims: the debounced front door skips outright — no
        // clear, no SIGWINCH, no generation bump (the emitter must
        // never open a settle window for a no-op).
        DaemonPtySession::request_resize(&s, 80, 24);
        assert_eq!(s.resize_generation(), 0, "no-op resize must not bump");
        // Direct same-dims resize() is also gated on dims_changed.
        s.resize(80, 24);
        assert_eq!(s.resize_generation(), 0, "direct no-op must not bump");
        // Real change bumps exactly once.
        DaemonPtySession::request_resize(&s, 100, 30);
        assert_eq!(term_dims(&s), (100, 30));
        assert_eq!(s.resize_generation(), 1);
    }

    // ── S7a pin-to-size (prd-presence-multiplayer-v1 §5.5) ──────────
    //
    // The clamp lives at the top of `request_resize` (and, belt-and-
    // suspenders, `resize`), so a pinned session's PTY cannot be moved
    // by ANY resize path. These tests pin that contract at the session
    // level; the daemon crate's grid-WS tests cover the SetActive /
    // detach-promotion callers, and the integration test covers the
    // route + wire.

    #[cfg(unix)]
    #[test]
    fn set_pinned_applies_immediately_and_clamps_request_resize() {
        let s = spawn_cat_session();
        assert_eq!(s.pinned(), None, "fresh session must be unpinned");

        DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("owner".into()));
        assert_eq!(s.pinned(), Some((100, 30)));
        assert_eq!(s.pinned_set_by().as_deref(), Some("owner"));
        assert_dims_settle(&s, (100, 30), "pin must apply its dims");
        let applied_at_pin = s.resizes_applied();

        // Any resize target now clamps to the pin — which equals the
        // live dims, so the same-dims skip applies NOTHING: no apply
        // count bump, no pending flush.
        DaemonPtySession::request_resize(&s, 140, 45);
        assert_eq!(
            s.resizes_applied(),
            applied_at_pin,
            "resize while pinned must be a clamped no-op"
        );
        assert_eq!(term_dims(&s), (100, 30));
        // Outwait the debounce window: no deferred flush may move a
        // pinned PTY either.
        std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
        assert_eq!(
            term_dims(&s),
            (100, 30),
            "pinned dims must hold across the debounce window"
        );
    }

    #[cfg(unix)]
    #[test]
    fn direct_resize_is_clamped_while_pinned() {
        let s = spawn_cat_session();
        DaemonPtySession::set_pinned(&s, Some((100, 30)), None);
        assert_dims_settle(&s, (100, 30), "pin");
        // A direct resize() caller (bypassing the front door) must
        // also be clamped — the pin is inescapable.
        s.resize(150, 50);
        assert_eq!(
            term_dims(&s),
            (100, 30),
            "direct resize() must clamp to the pin"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unpin_restores_active_viewer_viewport() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_cat_session();
        // Viewer 7 is the active claimer at 120x40 (the pre-pin
        // "juggling" state the unpin must clear back to).
        s.note_viewport(7, 120, 40);
        s.active_subscriber.store(7, Relaxed);
        DaemonPtySession::request_resize(&s, 120, 40);
        assert_dims_settle(&s, (120, 40), "active viewer fit");

        DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("owner".into()));
        assert_dims_settle(&s, (100, 30), "pin");

        // Unpin: dims must return to the active viewer's declared
        // viewport without any client having to re-send a Resize.
        DaemonPtySession::set_pinned(&s, None, None);
        assert_eq!(s.pinned(), None);
        assert_eq!(s.pinned_set_by(), None, "attribution clears with the pin");
        assert_dims_settle(&s, (120, 40), "unpin restores active viewport");
    }

    #[cfg(unix)]
    #[test]
    fn unpin_falls_back_to_most_recent_viewport_without_active_claim() {
        let s = spawn_cat_session();
        // Two viewers declared dims but nobody holds the claim (the
        // elect_on_detach eligibility + recency policy applies): the
        // most recent (9) wins.
        s.note_viewport(7, 100, 30);
        s.note_viewport(9, 90, 25);
        DaemonPtySession::set_pinned(&s, Some((60, 20)), None);
        assert_dims_settle(&s, (60, 20), "pin");
        DaemonPtySession::set_pinned(&s, None, None);
        assert_dims_settle(&s, (90, 25), "unpin restores most-recent viewport");
    }

    #[cfg(unix)]
    #[test]
    fn unpin_with_no_viewer_leaves_dims_and_reopens_resizing() {
        let s = spawn_cat_session();
        DaemonPtySession::set_pinned(&s, Some((100, 30)), None);
        assert_dims_settle(&s, (100, 30), "pin");

        // No viewports registered: unpin leaves dims as-is (a
        // reattaching viewer claims + resizes anyway)...
        DaemonPtySession::set_pinned(&s, None, None);
        assert_eq!(s.pinned(), None);
        std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
        assert_eq!(term_dims(&s), (100, 30), "no viewer — dims left as-is");

        // ...and the next resize applies normally again.
        DaemonPtySession::request_resize(&s, 90, 25);
        assert_dims_settle(&s, (90, 25), "resize after unpin must apply");
    }

    #[cfg(unix)]
    #[test]
    fn pin_changes_broadcast_on_the_pin_channel() {
        let s = spawn_cat_session();
        let mut rx = s.subscribe_pins();

        DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("teacher".into()));
        DaemonPtySession::set_pinned(&s, None, None);

        // Both events must be observable, in order — the grid-WS
        // subscriber loop turns these into pin_changed frames.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut seen: Vec<Option<(u16, u16)>> = Vec::new();
        while seen.len() < 2 {
            match rx.try_recv() {
                Ok(ev) => seen.push(ev),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        panic!("pin broadcast incomplete within 2s; saw {seen:?}");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("pin broadcast died: {e}"),
            }
        }
        assert_eq!(seen, vec![Some((100, 30)), None]);
    }

    // ── Companion T0 — ephemeral "claim session" pin ────────────────
    //
    // An ephemeral pin is a normal pin PLUS an owner stamp (the
    // setting grid-WS subscriber id): it clamps identically, but
    // auto-clears when its setter detaches, after which the normal
    // election restores the survivors. Manual unpin and normal pins
    // supersede it (last write wins). The daemon crate's grid-WS tests
    // cover the `claim_pin` socket action that feeds this.

    /// Drain exactly `want` pin-broadcast events (bounded wait, loud
    /// failure — mirrors `pin_changes_broadcast_on_the_pin_channel`).
    #[cfg(unix)]
    fn recv_pin_events(
        rx: &mut broadcast::Receiver<Option<(u16, u16)>>,
        want: usize,
    ) -> Vec<Option<(u16, u16)>> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut seen: Vec<Option<(u16, u16)>> = Vec::new();
        while seen.len() < want {
            match rx.try_recv() {
                Ok(ev) => seen.push(ev),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        panic!(
                            "pin broadcast incomplete within 2s; \
                             saw {seen:?} (wanted {want})"
                        );
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("pin broadcast died: {e}"),
            }
        }
        seen
    }

    /// Assert NO pin event arrives within a settle window — the
    /// idempotence / quiet-path contract (no event spam).
    #[cfg(unix)]
    fn assert_no_pin_event(
        rx: &mut broadcast::Receiver<Option<(u16, u16)>>,
        what: &str,
    ) {
        std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
        match rx.try_recv() {
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
            other => panic!("{what}: expected no pin event, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn ephemeral_pin_clamps_resizes_like_a_normal_pin() {
        let s = spawn_cat_session();
        DaemonPtySession::set_pinned_ephemeral(&s, 100, 30, Some("owner".into()), 9);
        assert_eq!(s.pinned(), Some((100, 30)));
        assert_eq!(s.pinned_set_by().as_deref(), Some("owner"));
        assert_eq!(s.ephemeral_pin_owner(), 9);
        assert_dims_settle(&s, (100, 30), "ephemeral pin must apply its dims");

        // Every other client's resize clamps exactly like a normal pin.
        DaemonPtySession::request_resize(&s, 140, 45);
        std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
        assert_eq!(
            term_dims(&s),
            (100, 30),
            "resize while ephemerally pinned must clamp"
        );
    }

    #[cfg(unix)]
    #[test]
    fn setter_detach_auto_clears_ephemeral_pin_and_restores_survivor() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_cat_session();
        let mut rx = s.subscribe_pins();
        // Desktop 7 at 100x30; phone 9 claims active + ephemeral-pins
        // to its own dims.
        s.note_viewport(7, 100, 30);
        s.note_viewport(9, 40, 10);
        s.active_subscriber.store(9, Relaxed);
        DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, Some("owner".into()), 9);
        assert_dims_settle(&s, (40, 10), "ephemeral pin");

        // The phone's socket drops: the pin auto-clears (broadcast!),
        // the election promotes the desktop, and the PTY restores.
        DaemonPtySession::detach_subscriber(&s, 9);
        assert_eq!(s.pinned(), None, "pin must auto-clear on setter detach");
        assert_eq!(s.pinned_set_by(), None, "attribution clears with the pin");
        assert_eq!(s.ephemeral_pin_owner(), 0, "ownership cleared");
        assert_eq!(s.active_subscriber.load(Relaxed), 7, "survivor promoted");
        assert_dims_settle(&s, (100, 30), "survivor dims restored");
        // Wire truth for the remaining subscribers: pinned, then cleared.
        assert_eq!(
            recv_pin_events(&mut rx, 2),
            vec![Some((40, 10)), None],
            "pin_changed must fire for the pin AND the auto-clear"
        );
    }

    #[cfg(unix)]
    #[test]
    fn other_subscriber_detach_leaves_ephemeral_pin_in_place() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_cat_session();
        s.note_viewport(7, 100, 30);
        s.note_viewport(9, 40, 10);
        s.active_subscriber.store(9, Relaxed);
        DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, None, 9);
        assert_dims_settle(&s, (40, 10), "ephemeral pin");

        // A NON-owning viewer leaving must not perturb the pin.
        DaemonPtySession::detach_subscriber(&s, 7);
        assert_eq!(s.pinned(), Some((40, 10)), "pin survives other detaches");
        assert_eq!(s.ephemeral_pin_owner(), 9, "ownership untouched");
        assert_eq!(term_dims(&s), (40, 10));
    }

    #[cfg(unix)]
    #[test]
    fn manual_unpin_clears_ephemeral_pin_and_later_detach_stays_quiet() {
        let s = spawn_cat_session();
        DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, None, 9);
        assert_dims_settle(&s, (40, 10), "ephemeral pin");

        // "The user on the other end unpins" — today's route calls
        // exactly this. Ownership clears with the pin...
        DaemonPtySession::set_pinned(&s, None, None);
        assert_eq!(s.pinned(), None);
        assert_eq!(s.ephemeral_pin_owner(), 0, "manual unpin drops ownership");

        // ...so the setter's eventual detach is pin-quiet: no spurious
        // second `pin_changed` for a pin that's already gone.
        let mut rx = s.subscribe_pins();
        DaemonPtySession::detach_subscriber(&s, 9);
        assert_no_pin_event(&mut rx, "detach after manual unpin");
    }

    #[cfg(unix)]
    #[test]
    fn normal_pin_replaces_ephemeral_and_survives_setter_detach() {
        let s = spawn_cat_session();
        DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, None, 9);
        assert_dims_settle(&s, (40, 10), "ephemeral pin");

        // A normal pin (pin-size route) lands afterwards: last write
        // wins, the pin is no longer ephemeral...
        DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("owner".into()));
        assert_eq!(s.ephemeral_pin_owner(), 0, "normal pin demotes ownership");
        assert_dims_settle(&s, (100, 30), "normal pin applies");

        // ...so the former setter's detach must NOT clear it
        // (regression: normal pins keep today's semantics exactly).
        DaemonPtySession::detach_subscriber(&s, 9);
        std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
        assert_eq!(
            s.pinned(),
            Some((100, 30)),
            "normal pin must survive the ex-owner's detach"
        );
        assert_eq!(term_dims(&s), (100, 30));
    }

    #[cfg(unix)]
    #[test]
    fn reclaim_updates_dims_in_place_and_same_dims_is_silent() {
        let s = spawn_cat_session();
        let mut rx = s.subscribe_pins();
        // Watcher 7 at 100x30 keeps trying to resize throughout.
        s.note_viewport(7, 100, 30);

        DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, None, 9);
        assert_dims_settle(&s, (40, 10), "initial claim");

        // Keyboard opens: the SAME subscriber re-claims at new dims —
        // update in place (no unpin/repin churn), exactly ONE event.
        DaemonPtySession::set_pinned_ephemeral(&s, 40, 18, None, 9);
        assert_eq!(s.pinned(), Some((40, 18)), "re-claim updates the pin");
        assert_eq!(s.ephemeral_pin_owner(), 9, "ownership unchanged");
        assert_dims_settle(&s, (40, 18), "PTY follows the re-claim");
        assert_eq!(
            recv_pin_events(&mut rx, 2),
            vec![Some((40, 10)), Some((40, 18))],
            "one pin_changed per real dims change"
        );

        // Other clients stay clamped at the NEW dims.
        DaemonPtySession::request_resize(&s, 100, 30);
        std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
        assert_eq!(term_dims(&s), (40, 18), "others clamp to the re-claim");

        // Keyboard bounce with unchanged dims: cheap + idempotent — no
        // broadcast, no resize window, ownership intact.
        let applied_before = s.resizes_applied();
        DaemonPtySession::set_pinned_ephemeral(&s, 40, 18, None, 9);
        assert_no_pin_event(&mut rx, "same-dims re-claim");
        assert_eq!(s.pinned(), Some((40, 18)));
        assert_eq!(s.ephemeral_pin_owner(), 9);
        assert_eq!(
            s.resizes_applied(),
            applied_before,
            "same-dims re-claim must not apply a resize"
        );
    }

    /// A DSR-6 (`\x1b[6n`) parsed by the Term must produce a CPR answer
    /// that lands in the query-reply sink — the seam `spawn()` wires to
    /// the PTY's `Notifier` — and must NOT leak onto the viewer
    /// broadcast (a query answer is transport, not session output).
    #[test]
    fn dsr_query_answer_lands_in_reply_sink_not_broadcast() {
        use alacritty_terminal::vte::ansi::Processor;

        let (tx, mut events_rx) = broadcast::channel::<AlacEvent>(16);
        let (reply_tx, reply_rx) = std::sync::mpsc::channel::<String>();
        let reply_slot = Arc::new(std::sync::OnceLock::new());
        assert!(reply_slot
            .set(Box::new(move |text: String| {
                reply_tx.send(text).expect("capture query reply");
            }) as QueryReplySink)
            .is_ok());
        let listener = DaemonEventListener {
            tx,
            reply_tx: reply_slot,
        };

        let size = TermSize { cols: 80, rows: 24 };
        let mut term = Term::new(TermConfig::default(), &size, listener);
        let mut parser: Processor = Processor::new();
        // Print "ab" first so the answer proves the LIVE cursor
        // position (row 1, col 3), not a hardcoded origin.
        parser.advance(&mut term, b"ab\x1b[6n");

        let reply = reply_rx
            .try_recv()
            .expect("DSR-6 must synchronously produce a CPR answer");
        assert_eq!(reply, "\x1b[1;3R", "CPR must report the live cursor");
        match events_rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => {}
            other => panic!(
                "PtyWrite must not reach the viewer broadcast, got {other:?}"
            ),
        }
    }

    /// Slice 5 — the grok DCS gotcha, pinned. Grok echoes unconsumed
    /// DCS replies (`ESC P … ST`, e.g. XTVERSION `ESC P>|… ST`) into
    /// its composer as literal text, so K2's query-reply layer must
    /// NEVER emit a DCS-shaped answer. We drive the parser with the
    /// query families a TUI might send — XTVERSION (`CSI > 0 q`),
    /// DECRQSS (`DCS $ q m ST`), DA1/DA2, DECRQM, DSR-6 — and assert
    /// every produced reply is CSI-shaped (`ESC [`), none DCS-shaped
    /// (`ESC P`). If a vte/alacritty bump ever adds a DECRQSS/XTVERSION
    /// responder this fails loudly, and DCS must be gated away from
    /// grok panes before shipping.
    #[test]
    fn query_replies_are_never_dcs_shaped() {
        use alacritty_terminal::vte::ansi::Processor;

        let (tx, _events_rx) = broadcast::channel::<AlacEvent>(16);
        let (reply_tx, reply_rx) = std::sync::mpsc::channel::<String>();
        let reply_slot = Arc::new(std::sync::OnceLock::new());
        assert!(reply_slot
            .set(Box::new(move |text: String| {
                reply_tx.send(text).expect("capture query reply");
            }) as QueryReplySink)
            .is_ok());
        let listener = DaemonEventListener {
            tx,
            reply_tx: reply_slot,
        };

        let size = TermSize { cols: 80, rows: 24 };
        let mut term = Term::new(TermConfig::default(), &size, listener);
        let mut parser: Processor = Processor::new();
        // XTVERSION, DECRQSS(SGR), DA1, DA2, DECRQM(?2004), DSR-6.
        parser.advance(
            &mut term,
            b"\x1b[>0q\x1bP$qm\x1b\\\x1b[c\x1b[>c\x1b[?2004$p\x1b[6n",
        );

        let mut replies = Vec::new();
        while let Ok(r) = reply_rx.try_recv() {
            replies.push(r);
        }
        assert!(
            !replies.is_empty(),
            "the query batch must produce at least the DA/DSR answers"
        );
        for r in &replies {
            assert!(
                !r.starts_with("\x1bP"),
                "query reply must never be DCS-shaped (grok echoes DCS as composer text): {r:?}"
            );
            assert!(
                r.starts_with("\x1b["),
                "every reply alacritty produces today is CSI-shaped: {r:?}"
            );
        }
    }

    /// `^[[<digits>;<digits>R` — the tty-ECHO rendering of a CPR answer
    /// (ECHOCTL renders the answer's ESC as caret notation on the grid).
    #[cfg(unix)]
    fn contains_cpr_echo(row: &str) -> bool {
        let bytes = row.as_bytes();
        for start in 0..bytes.len() {
            let rest = &bytes[start..];
            if !rest.starts_with(b"^[[") {
                continue;
            }
            let mut i = 3;
            let row_digits = i;
            while i < rest.len() && rest[i].is_ascii_digit() {
                i += 1;
            }
            if i == row_digits || rest.get(i) != Some(&b';') {
                continue;
            }
            i += 1;
            let col_digits = i;
            while i < rest.len() && rest[i].is_ascii_digit() {
                i += 1;
            }
            if i > col_digits && rest.get(i) == Some(&b'R') {
                return true;
            }
        }
        false
    }

    /// End-to-end through the REAL notifier + PTY: a DSR-6 the child
    /// emits gets answered, and the answer arrives on the child's input.
    /// We type `\x1b[6n\n` at `cat`: the tty ECHO mangles the typed ESC
    /// to `^[` (no dispatch), but cat's OUTPUT carries a real ESC — the
    /// parser dispatches the query, the emulator's CPR answer routes
    /// back into PTY input, and the tty ECHO of that ANSWER renders as
    /// `^[[<row>;<col>R` on the grid. Before the fix nothing comes back
    /// and this times out.
    #[cfg(unix)]
    #[test]
    fn dsr_query_from_child_is_answered_through_the_pty() {
        let s = spawn_cat_session();
        s.write(b"\x1b[6n\n".to_vec());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let rows = s.visible_text_rows();
            if rows.iter().any(|r| contains_cpr_echo(r)) {
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "no CPR answer echoed within 5s — the query reply \
                     never reached the child. grid: {rows:?}"
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Count this process's open PTY master fds via lsof. On macOS a
    /// master opened through posix_openpt shows up as `/dev/ptmx`.
    /// Loud on failure — a missing/broken lsof must fail the test,
    /// never report a fake 0.
    #[cfg(unix)]
    fn count_ptmx_fds() -> usize {
        let pid = std::process::id();
        let out = std::process::Command::new("lsof")
            .args(["-p", &pid.to_string()])
            .output()
            .expect("lsof must be runnable");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains("/dev/ptmx"))
            .count()
    }

    /// PTY-master leak regression (path 1: child exits ON ITS OWN).
    /// The session Arc is still held — as the daemon's session map /
    /// WS handlers hold theirs — and the master fd must be released
    /// anyway: the fd's lifetime is the IO thread's, not the Arc's.
    /// Here the IO thread's own `try_wait` wins the reap, sees
    /// `ChildEvent::Exited`, breaks its loop, and drops the Pty.
    #[cfg(unix)]
    #[test]
    fn child_self_exit_releases_pty_master_fd() {
        let baseline = count_ptmx_fds();
        let cfg = DaemonPtyConfig {
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("/bin/sh".to_string()),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            ..Default::default()
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn sh");
        let pid = s.child_pid().expect("pid captured");

        // Wait for the child to fully disappear (exit + reap).
        let mut gone = false;
        for _ in 0..250 {
            if unsafe { libc::kill(pid, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error()
                    == Some(libc::ESRCH)
            {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gone, "child pid={pid} must exit+reap on its own (sh -c 'exit 0')");

        // The Arc is STILL held here. The master must close anyway.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut now_count = count_ptmx_fds();
        while now_count > baseline && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            now_count = count_ptmx_fds();
        }
        assert!(
            now_count <= baseline,
            "PTY master leaked after child self-exit: baseline={baseline} now={now_count} \
             (Arc still held — the fd's lifetime must not be anchored to the session object)"
        );
        drop(s);
    }

    /// PTY-master leak regression (path 2: teardown via `kill()` when
    /// the IO thread LOST the reap race). Before the fix this leaked
    /// the master fd until daemon exit: an external `waitpid` reaps the
    /// child first, so alacritty's `try_wait` gets ECHILD and
    /// `next_child_event` swallows it (no loop break), and with Arcs
    /// still retained (session map / WS handler / emitter) nothing else
    /// ever woke the parked thread. `kill()` must now release the fd
    /// explicitly via `Msg::Shutdown` — with the Arcs STILL held.
    #[cfg(unix)]
    #[test]
    fn kill_releases_pty_master_fd_with_arc_retained() {
        let baseline = count_ptmx_fds();
        let cfg = DaemonPtyConfig {
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("sleep".to_string()),
            args: vec!["600".to_string()],
            ..Default::default()
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn sleep");
        let retained = Arc::clone(&s); // simulated WS-handler/emitter holder
        let pid = s.child_pid().expect("pid captured");

        // Deterministically lose the race for the IO thread: reap the
        // child HERE (the test process is the parent) before kill()
        // runs. The IO thread's try_wait can now only ever see ECHILD.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            let mut status: i32 = 0;
            let r = libc::waitpid(pid, &mut status, 0);
            // A blocking waitpid parked in the kernel beats the IO
            // thread's kqueue→pipe→try_wait chain in practice; but if
            // the IO thread somehow reaped first (ECHILD here), the
            // post-condition below still pins the contract — the fd
            // must be released with Arcs retained either way.
            assert!(
                r == pid
                    || (r == -1
                        && std::io::Error::last_os_error().raw_os_error()
                            == Some(libc::ECHILD)),
                "waitpid(pid={pid}) returned {r} (expected the pid, or \
                 -1/ECHILD if alacritty's IO thread won the reap race)"
            );
        }

        // The teardown chokepoint every path funnels through
        // (unregister / close route / reaper / Drop).
        s.kill();

        // Both Arcs are STILL held. The master must close anyway.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut now_count = count_ptmx_fds();
        while now_count > baseline && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            now_count = count_ptmx_fds();
        }
        assert!(
            now_count <= baseline,
            "PTY master leaked after kill() with retained Arcs: \
             baseline={baseline} now={now_count} — kill() must shut the \
             IO thread down explicitly, not wait for the last Arc drop"
        );
        drop(retained);
        drop(s);
    }

    /// `kill()` is idempotent: calling it twice must not panic, must
    /// not waitpid a (possibly-recycled) PID a second time, and the
    /// second call is a cheap no-op. Drop then runs kill() a third
    /// time implicitly — also a no-op.
    #[cfg(unix)]
    #[test]
    fn kill_is_idempotent() {
        use std::path::PathBuf;

        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            sandbox: SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
            program: Some("sleep".to_string()),
            args: vec!["600".to_string()],
            env: Default::default(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn sleep");
        let pid = s.child_pid().expect("pid captured");

        s.kill();
        // Second call must be a no-op (returns early on the `killed`
        // gate) — and must not blow up.
        s.kill();

        // Confirm the child is gone exactly once (no double-reap havoc).
        let mut gone = false;
        for _ in 0..50 {
            if unsafe { libc::kill(pid, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error()
                    == Some(libc::ESRCH)
            {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone, "child pid={pid} gone after idempotent kills");
        // Drop fires kill() a third time — no panic, no double free.
        drop(s);
    }
}
