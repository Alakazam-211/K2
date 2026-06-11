//! Grow-then-shrink settle watcher.
//!
//! Every session the daemon spawns starts at an artificially large
//! `rows` value (see `GROW_ROWS` in `session_stream_pty`). The child
//! TUI paints into that big canvas and LineMux captures every byte
//! as a Frame in the session's ring. After the initial paint
//! settles, the daemon SIGWINCHes the PTY down to the user's real
//! rows — the TUI repaints at the final size, and subscribers who
//! attach after the shrink see: older rows in scrollback (from the
//! big paint) + the TUI's current UI in the live grid (from the
//! post-shrink repaint).
//!
//! This module owns the "has the initial paint settled?" decision.
//! The caller runs `run_grow_settle` inside the HTTP spawn handler
//! and awaits it — the HTTP response is blocked until settle
//! completes, guaranteeing no thin-client subscriber witnesses the
//! grow phase.
//!
//! **Settle triggers** (first to fire wins):
//!
//! 1. `IDLE_MS` of no frames after the first frame — catches both
//!    simple shells (bash prints prompt, idles) and paint-and-wait
//!    TUIs (Claude, less, man pages). This is the common path for
//!    every harness we care about.
//! 2. `CEILING_MS` hard ceiling — safety net for pathological TUIs
//!    that never settle (think `tail -f /dev/urandom`).
//!
//! **Why no mode-change fast path.** An earlier version of this
//! module treated `Frame::ModeChange { mode: BracketedPaste |
//! FocusReporting | AltScreen, on: true }` as an immediate settle
//! trigger on the theory that a modern TUI only asserts those
//! modes after its cold-start is done. That's true for fresh
//! `claude` launches, but catastrophically wrong for
//! `claude --resume <uuid>` — the resume path emits its mode
//! declarations DURING cold-start, BEFORE reading the saved
//! conversation from disk and painting the replay. Settling there
//! SIGWINCHes the PTY down to the user's real size before Claude
//! ever paints the conversation, which is the exact failure mode
//! the grow trick is supposed to fix. Removing the mode-change
//! trigger adds ~300-400 ms to fresh-launch spawn times in exchange
//! for correct resume capture — an unconditional win since the
//! resume path is the whole point.
//!
//! **Backpressure**: the tokio broadcast channel's `RecvError::Lagged`
//! is treated as "keep going." We don't need every frame, just the
//! mode-change signal; if we're lagged, the TUI is emitting fast
//! enough that idle-timer resets are preventing idle-settle anyway.

use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;

use crate::session::{ModeKind, SessionEntry};

/// Idle window after the first frame before we declare the session
/// settled. Tuned so simple shells (bash prints prompt then idles)
/// don't add noticeable latency while long-running TUI cold-starts
/// still have room to emit their mode-change trigger.
pub const IDLE_MS: u64 = 400;

/// Hard ceiling on the grow-settle wait. Bounds spawn latency in the
/// worst case (a TUI that never emits a recognizable settle signal
/// AND never goes idle). 3 s is long enough for any reasonable cold-
/// start, short enough that a stuck session doesn't block a thin
/// client indefinitely.
pub const CEILING_MS: u64 = 3_000;

/// Why `run_grow_settle` returned. Diagnostic; callers log it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleReason {
    /// A ModeChange frame (alt_screen / bracketed_paste /
    /// focus_reporting with `on=true`) signaled that the TUI
    /// finished its cold start.
    ModeChange(ModeKind),
    /// No frames arrived for `IDLE_MS` after the first frame
    /// (typical non-TUI shell).
    Idle,
    /// Hit `CEILING_MS` without either other trigger firing.
    /// Something's probably stuck; we shrink anyway rather than
    /// block the spawn response forever.
    Ceiling,
    /// The broadcast channel closed before we could settle — the
    /// session died during grow. Caller should propagate this as a
    /// spawn failure rather than attempting to resize.
    Closed,
}

impl SettleReason {
    /// Human-readable tag for the spawn log line.
    pub fn tag(&self) -> &'static str {
        match self {
            SettleReason::ModeChange(ModeKind::AltScreen) => "alt_screen",
            SettleReason::ModeChange(ModeKind::BracketedPaste) => "bracketed_paste",
            SettleReason::ModeChange(ModeKind::FocusReporting) => "focus_reporting",
            SettleReason::ModeChange(_) => "mode_change_other",
            SettleReason::Idle => "idle",
            SettleReason::Ceiling => "ceiling",
            SettleReason::Closed => "closed",
        }
    }
}

/// Block until the session's initial paint has settled. Intended to
/// be called from the HTTP spawn handler inside a tokio runtime.
///
/// `entry` is the `SessionEntry` that was just registered; the
/// function subscribes to its broadcast and watches for the first of
/// the three triggers described in the module-level docs.
pub async fn run_grow_settle(entry: &SessionEntry) -> SettleReason {
    let mut rx = entry.subscribe();
    let ceiling = tokio::time::sleep(Duration::from_millis(CEILING_MS));
    tokio::pin!(ceiling);
    let mut got_first_frame = false;

    loop {
        let idle_future = async {
            if got_first_frame {
                tokio::time::sleep(Duration::from_millis(IDLE_MS)).await;
                true
            } else {
                // Until the first frame arrives, the "idle" arm is
                // functionally disabled — we park forever on an
                // unresolvable future so `tokio::select!` picks one
                // of the other branches.
                std::future::pending::<bool>().await
            }
        };

        tokio::select! {
            biased;

            // Ceiling first: a stuck session must always shrink
            // eventually, even if traffic is keeping idle-reset
            // busy.
            _ = &mut ceiling => return SettleReason::Ceiling,

            // Any frame counts as activity — we don't special-case
            // ModeChange here anymore (see module docs). The frame
            // just flips `got_first_frame` so the idle arm becomes
            // active, and then continues waiting for quiet.
            recv = rx.recv() => match recv {
                Ok(_frame) => {
                    got_first_frame = true;
                }
                Err(RecvError::Lagged(_)) => {
                    // The TUI is emitting faster than we're
                    // reading. That's fine — any lag means traffic
                    // is still flowing, so idle won't fire. Mark
                    // that we've seen activity so the idle-arm
                    // becomes live if things quiet down.
                    got_first_frame = true;
                }
                Err(RecvError::Closed) => return SettleReason::Closed,
            },

            // Idle wins if traffic stops after the first frame.
            _ = idle_future => return SettleReason::Idle,
        }
    }
}
