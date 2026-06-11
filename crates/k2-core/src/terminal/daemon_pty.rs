//! Alacritty_v2 daemon-hosted terminal session.
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
use alacritty_terminal::tty::{self, Options as TtyOptions, Shell};
use alacritty_terminal::Term;
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::log_debug;
use crate::session::SessionId;
use crate::terminal::login_path;

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

/// Minimal `EventListener` that broadcasts every alacritty lifecycle
/// event to any number of subscribers via `tokio::sync::broadcast`.
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

#[derive(Clone)]
pub struct DaemonEventListener {
    tx: broadcast::Sender<AlacEvent>,
}

impl EventListener for DaemonEventListener {
    fn send_event(&self, event: AlacEvent) {
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
        }
    }
}

/// A daemon-hosted terminal session.
///
/// Holds the PTY, the alacritty Term, and the PTY writer handle.
/// Typically stored inside an `Arc` so multiple subsystems
/// (session_map, registry, WS handler) can share one handle.
///
/// Dropping the last Arc closes the PTY channel, which causes
/// alacritty's IO thread to exit naturally. The thread handle is
/// NOT stored — we let it clean up itself on channel close.
pub struct DaemonPtySession {
    pub session_id: SessionId,
    pub cwd: Option<PathBuf>,
    pub program: Option<String>,
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

    /// Notifier for writing input bytes + signaling resize. The
    /// Notifier wraps the alacritty event loop's sender channel;
    /// dropping it closes the channel and shuts the IO thread down.
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
        let shell = cfg
            .program
            .as_ref()
            .map(|prog| Shell::new(prog.clone(), spawn_args.clone()));

        // Build the env we hand to alacritty's tty::new. Without an
        // explicit TERM/COLORTERM, child processes inherit alacritty's
        // default (TERM=dumb on this version), which makes Claude Code,
        // bash prompts, ls --color, vim — basically every TUI — turn
        // OFF colors. Mirror what `alacritty_backend.rs` does for the
        // legacy renderer (TERM=xterm-256color + COLORTERM=truecolor +
        // TERM_PROGRAM=K2SO) so v2 children render the same colors as
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
            .or_insert_with(|| "K2SO".to_string());

        let pty_options = TtyOptions {
            shell,
            working_directory: cfg.cwd.clone(),
            drain_on_exit: cfg.drain_on_exit,
            env: child_env,
            #[cfg(target_os = "windows")]
            escape_args: false,
        };

        // Window ID is used on macOS/Windows to associate the PTY
        // with a specific OS window for controlling-terminal
        // semantics. The daemon has no window, so we pass 0.
        let __t_pty = std::time::Instant::now();
        let pty = tty::new(&pty_options, window_size, 0)?;

        // Capture the direct child PID NOW, while we still own the Pty
        // — `EventLoop::new` consumes it below. alacritty's `Pty`
        // exposes `child() -> &std::process::Child`; `.id()` is the
        // PID as a u32. This PID is what `kill()` forcefully reaps on
        // teardown so agent-CLI children don't orphan. The child is a
        // setsid() session leader (its own process group), so killpg
        // on its pgid is daemon-safe.
        #[cfg(unix)]
        let child_pid: Option<i32> = Some(pty.child().id() as i32);
        #[cfg(not(unix))]
        let child_pid: Option<i32> = None;

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
        let listener = DaemonEventListener {
            tx: events_tx.clone(),
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

        // Spawn the IO thread. The handle is `JoinHandle<(EventLoop, State)>`
        // — we intentionally don't store it. When the last `Arc<Self>`
        // drops, `pty_notifier` drops, the EventLoopSender closes,
        // the IO thread sees the Shutdown variant and exits on its own.
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

        Ok(Arc::new(Self {
            session_id: cfg.session_id,
            cwd: cfg.cwd,
            program: cfg.program,
            pid: child_pid,
            killed: std::sync::atomic::AtomicBool::new(false),
            args: spawn_args,
            term,
            pty_notifier: Mutex::new(Notifier(pty_sender)),
            events_tx,
            child_exited: std::sync::atomic::AtomicBool::new(false),
            active_subscriber: std::sync::atomic::AtomicU64::new(0),
            active_cols: std::sync::atomic::AtomicU16::new(0),
            active_rows: std::sync::atomic::AtomicU16::new(0),
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
    /// more obvious `ClearMode::All`. In non-alt-screen mode (Claude's
    /// TUI is not alt-screen), `ClearMode::All` calls
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

    /// Number of live subscribers currently attached to this
    /// session's event broadcast.
    ///
    /// Each attached grid-WS connection
    /// (`sessions_grid_ws::handle`) holds a `broadcast::Receiver`
    /// obtained from [`subscribe_events`]; that receiver lives for
    /// the duration of the WS connection and drops when the client
    /// detaches. `broadcast::Sender::receiver_count` therefore
    /// reports the real number of clients watching this session —
    /// > 0 means "someone is attached", 0 means "nobody is looking".
    ///
    /// This is the authoritative "is a client attached?" signal the
    /// age-out reaper consults before killing a session (GH#22): a
    /// remote client attached over K2 Connect keeps a live receiver,
    /// so the reaper must not reap a session with `subscriber_count() > 0`.
    pub fn subscriber_count(&self) -> usize {
        self.events_tx.receiver_count()
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

        // Close the event-loop channel first so alacritty's IO thread
        // begins shutting down (its own Pty::Drop SIGHUPs the child as
        // a bonus). We don't rely on it for the kill — the explicit
        // sequence below is authoritative.
        // (pty_notifier is dropped when `self` drops; nothing to do
        // here — the kill sequence stands on its own.)

        let pid = match self.pid {
            Some(p) if p > 0 => p,
            _ => return,
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

        log_debug!("[v2-kill] session={} reaped child pid={}", self.session_id, pid);
    }

    /// PID of the direct child process, if captured. Exposed for
    /// tests that assert the child is actually gone after `kill()`.
    pub fn child_pid(&self) -> Option<i32> {
        self.pid
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

    #[cfg(unix)]
    #[test]
    fn label_starts_with_seed_and_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
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
