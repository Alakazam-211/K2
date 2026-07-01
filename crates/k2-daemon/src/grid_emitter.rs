//! Shared per-session grid emitter — the 0.39.46 "starved viewer" fix.
//!
//! ## The bug this kills
//!
//! Pre-0.39.46, EVERY grid-WS connection ran its own `build_emit()`
//! against the session's Term. But alacritty's damage tracker is a
//! single shared accumulator on the Term, and `build_emit` consumes it
//! (`term.damage()` + `term.reset_damage()`). With ≥2 subscribers on
//! one session — the host app plus a K2 Connect client is the common
//! case — every PTY wakeup became a race: whichever connection's
//! handler locked the Term first got the damage; every other
//! connection computed "no damage, no new scrollback" → `Skip` → its
//! mirror silently missed those rows. The visible symptom: a remote
//! viewer's terminal missing the line that "just moved up" on an
//! input-box wrap, healing only on tab-switch (reconnect → fresh full
//! snapshot). Worse, every NEW subscriber's initial snapshot also
//! reset damage, starving existing subscribers at attach time.
//!
//! ## The model
//!
//! Damage is consumed by exactly ONE consumer per session: a shared
//! emitter task. It owns the session's single [`EmitState`], runs
//! `build_emit` per Wakeup, serializes the resulting Snapshot/Delta
//! ONCE, and broadcasts the encoded frame to every subscriber (a
//! bonus: N viewers no longer cost N encodes per frame).
//!
//! Subscribers attach via [`attach`]:
//!   1. subscribe to the frames channel FIRST,
//!   2. take a READ-ONLY full snapshot under the (emit_state, term)
//!      locks — no damage reset, no state mutation — stamped with the
//!      emitter's current version,
//!   3. forward only frames with `version > stamp`.
//!
//! The lock order (emit_state, then term) matches the emitter's own
//! `emit_once`, which makes the stamp exact: any frame versioned
//! after the stamp covers strictly post-snapshot changes, so deltas'
//! `scrollback_appended` rows (which are NOT idempotent — the client
//! concatenates them) can never duplicate rows the snapshot already
//! carries, and nothing between subscribe and snapshot can be missed.
//!
//! With zero subscribers the emitter still consumes damage on each
//! Wakeup — WITHOUT encoding — purely to keep `EmitState`
//! (`last_history_size` especially) coherent; a stale value would
//! make the first delta after a viewer attaches re-append scrollback
//! rows the attach snapshot already contains.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use tokio::sync::broadcast;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::{
    build_emit, drain_damage, AlacEvent, DaemonPtySession, EmitDecision, EmitState,
};

use crate::sessions_grid_ws::Outbound;

/// One encoded grid frame, ready for the wire. `text` is the full
/// serialized `{"event":"snapshot"|"delta","payload":...}` message —
/// `Arc<str>` so fanning it out to N subscribers shares one encode
/// (tungstenite 0.23's `Message::Text` takes an owned `String`, so
/// each subscriber pays one copy at send time — still N× cheaper than
/// the old N× encode).
#[derive(Clone)]
pub(crate) struct GridFrame {
    pub version: u64,
    pub text: std::sync::Arc<str>,
}

/// Frames channel capacity per session. A subscriber that falls more
/// than this many frames behind gets `Lagged` and recovers with a
/// fresh read-only snapshot (handled in `sessions_grid_ws`).
const FRAMES_CAP: usize = 256;

struct EmitterHandle {
    frames_tx: broadcast::Sender<GridFrame>,
    emit_state: Arc<Mutex<EmitState>>,
}

fn registry() -> &'static Mutex<HashMap<SessionId, EmitterHandle>> {
    static REGISTRY: OnceLock<Mutex<HashMap<SessionId, EmitterHandle>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get-or-spawn the shared emitter for `session` and join its frame
/// stream. Returns the frames receiver + the shared emit state (the
/// caller locks it — together with the Term, in that order — to take
/// its stamped read-only attach snapshot).
pub(crate) fn attach(
    session: &Arc<DaemonPtySession>,
    pane_id: &str,
) -> (broadcast::Receiver<GridFrame>, Arc<Mutex<EmitState>>) {
    let mut reg = registry().lock();
    if let Some(h) = reg.get(&session.session_id) {
        return (h.frames_tx.subscribe(), h.emit_state.clone());
    }
    let (frames_tx, frames_rx) = broadcast::channel(FRAMES_CAP);
    let emit_state = Arc::new(Mutex::new(EmitState::default()));
    reg.insert(
        session.session_id,
        EmitterHandle {
            frames_tx: frames_tx.clone(),
            emit_state: emit_state.clone(),
        },
    );
    log_debug!(
        "[daemon/grid-emitter] spawning shared emitter for session {}",
        session.session_id
    );
    tokio::spawn(run(
        Arc::clone(session),
        pane_id.to_string(),
        frames_tx,
        emit_state.clone(),
    ));
    (frames_rx, emit_state)
}

/// Minimum interval between encoded frames (~60 Hz). An isolated
/// Wakeup (keystroke echo on an idle session) emits immediately —
/// zero added latency. Only when Wakeups arrive faster than this do
/// we sleep out the remainder of the interval and coalesce the burst
/// into one frame. Bursty output (`cat` a big file, TUI redraw
/// storms) used to produce one encode+broadcast per Wakeup with no
/// pacing at all; alacritty's damage accumulator unions everything
/// that lands during the window, so the coalesced frame is complete.
/// The legacy `alacritty_backend` emission loop used the same 16ms
/// floor.
const MIN_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// The emitter task: one per live session, exits on child exit /
/// session teardown (events channel closed) and removes itself from
/// the registry.
async fn run(
    session: Arc<DaemonPtySession>,
    pane_id: String,
    frames_tx: broadcast::Sender<GridFrame>,
    emit_state: Arc<Mutex<EmitState>>,
) {
    let mut events_rx = session.subscribe_events();
    let mut last_emit = std::time::Instant::now() - MIN_FRAME_INTERVAL;
    loop {
        match events_rx.recv().await {
            Ok(AlacEvent::Wakeup) => {
                let since = last_emit.elapsed();
                let mut child_exited = false;
                if since < MIN_FRAME_INTERVAL {
                    // Mid-burst: wait out the frame interval, then
                    // absorb everything that queued up meanwhile so
                    // the whole burst becomes one frame.
                    tokio::time::sleep(MIN_FRAME_INTERVAL - since).await;
                    loop {
                        use broadcast::error::TryRecvError;
                        match events_rx.try_recv() {
                            Ok(AlacEvent::ChildExit(_)) => {
                                child_exited = true;
                                break;
                            }
                            // Wakeups fold into the accumulated
                            // damage; Title/Bell/labels ride each
                            // connection's own subscription.
                            Ok(_) => {}
                            // Lagged mid-drain: damage still
                            // accumulates on the Term, keep draining.
                            Err(TryRecvError::Lagged(_)) => {}
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Closed) => {
                                child_exited = true;
                                break;
                            }
                        }
                    }
                }
                emit_once(&session, &pane_id, &frames_tx, &emit_state);
                last_emit = std::time::Instant::now();
                if child_exited {
                    break;
                }
            }
            Ok(AlacEvent::ChildExit(_)) => break,
            Ok(_other) => {
                // Title / Bell / labels ride each connection's own
                // events subscription — not the emitter's concern.
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                // Missed Wakeups. Damage accumulates on the Term, so
                // one emit pass now covers everything we skipped —
                // but we must run it, because no further Wakeup is
                // guaranteed to follow.
                log_debug!(
                    "[daemon/grid-emitter] session {} lagged {n} events — emitting accumulated damage",
                    session.session_id
                );
                emit_once(&session, &pane_id, &frames_tx, &emit_state);
                last_emit = std::time::Instant::now();
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    registry().lock().remove(&session.session_id);
    log_debug!(
        "[daemon/grid-emitter] emitter for session {} exited",
        session.session_id
    );
}

/// One emit pass. Lock order: emit_state, then term — [`attach`]'s
/// snapshot path takes the same locks in the same order, which is
/// what makes its version stamp exact.
fn emit_once(
    session: &Arc<DaemonPtySession>,
    pane_id: &str,
    frames_tx: &broadcast::Sender<GridFrame>,
    emit_state: &Arc<Mutex<EmitState>>,
) {
    // Zero viewers: consume damage WITHOUT encoding so EmitState stays
    // coherent for the next attach (see module docs).
    if frames_tx.receiver_count() == 0 {
        let mut st = emit_state.lock();
        let term_mutex = session.term();
        let mut term = term_mutex.lock();
        drain_damage(&mut term, &mut st);
        return;
    }

    // Build the decision under the locks, but serialize AFTER
    // releasing them: `build_emit` returns owned Snapshot/Delta
    // values, and JSON-encoding a full snapshot (grid + up to 5000
    // scrollback rows) is the single longest stretch of this path.
    // Holding the Term's FairMutex through it blocked alacritty's
    // PTY IO thread from parsing new output — a direct source of
    // render stalls under heavy output. Frame ordering is unaffected:
    // this emitter task is the only frames_tx sender, so frames still
    // hit the channel in version order, and the attach path's version
    // stamp (taken under the same locks) stays exact.
    let decision = {
        let mut st = emit_state.lock();
        let term_mutex = session.term();
        let mut term = term_mutex.lock();
        build_emit(pane_id, &mut term, &mut st)
    };
    let frame = match decision {
        EmitDecision::Full(snap) => {
            serialize_frame(snap.version, &Outbound::Snapshot(&snap))
        }
        EmitDecision::Delta(delta) => {
            serialize_frame(delta.version, &Outbound::Delta(&delta))
        }
        EmitDecision::Skip => None,
    };
    if let Some(f) = frame {
        // Err = no receivers — raced a disconnect; nothing to do
        // (damage was legitimately consumed; the next attach takes a
        // full snapshot anyway).
        let _ = frames_tx.send(f);
    }
}

fn serialize_frame(version: u64, outbound: &Outbound<'_>) -> Option<GridFrame> {
    match serde_json::to_string(outbound) {
        Ok(json) => Some(GridFrame {
            version,
            text: std::sync::Arc::from(json),
        }),
        Err(e) => {
            log_debug!("[daemon/grid-emitter] frame serialize failed: {e}");
            None
        }
    }
}
