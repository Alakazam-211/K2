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
//!
//! ## Resize settle (blank-frame suppression)
//!
//! A real resize clears the viewport before the child's SIGWINCH
//! repaint; broadcasting that blank intermediate was the visible
//! resize "black flash". The emitter samples the session's
//! `resize_generation` (bumped under the term lock by
//! `daemon_pty::resize`) and, on a change, opens a settle window:
//! damage is drained (zero-viewer pattern) but nothing is broadcast
//! until repaint evidence ([`RESIZE_REPAINT_EVIDENCE_MIN`]) or the
//! hard [`RESIZE_SETTLE_MAX`] timeout, whichever first; the window
//! closes with one forced Full snapshot. Suppressed frames never
//! exist on the wire, so versions stay monotonic and the k1
//! floor/ack/resync semantics are untouched.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use tokio::sync::broadcast;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::{
    build_emit, drain_damage, grid_wire, AlacEvent, DaemonPtySession, EmitDecision,
    EmitState,
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
    /// The same frame in the "k1" binary format
    /// (`k2_core::terminal::grid_wire`). Encoded ONCE per frame,
    /// lazily: `Some` only when ≥1 subscriber of this session opted
    /// into k1 at encode time (`&proto=k1`), so a JSON-only session
    /// never pays the second encode. A k1 subscriber can only ever
    /// forward frames encoded AFTER its attach (its `frame_floor` is
    /// stamped after [`attach`] bumps the k1 refcount, under the same
    /// emit_state lock the emitter takes), so `None` on a
    /// floor-clearing frame is unreachable — the WS loop still falls
    /// back to `text` there, which the client accepts on either
    /// transport.
    pub binary: Option<std::sync::Arc<[u8]>>,
}

/// RAII registration of one subscriber's wire format with the
/// session's shared emitter. Dropping it (WS detach, any exit path)
/// decrements the k1 refcount so the emitter stops binary-encoding
/// when the last k1 subscriber leaves.
pub(crate) struct FormatRegistration {
    k1_subs: Option<Arc<AtomicUsize>>,
}

impl Drop for FormatRegistration {
    fn drop(&mut self) {
        if let Some(c) = &self.k1_subs {
            c.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Frames channel capacity per session. A subscriber that falls more
/// than this many frames behind gets `Lagged` and recovers with a
/// fresh read-only snapshot (handled in `sessions_grid_ws`).
const FRAMES_CAP: usize = 256;

struct EmitterHandle {
    frames_tx: broadcast::Sender<GridFrame>,
    emit_state: Arc<Mutex<EmitState>>,
    /// Live count of subscribers that opted into the k1 binary wire.
    /// Read by the emitter per frame to decide whether to binary-
    /// encode; written only via [`attach`] / [`FormatRegistration`].
    k1_subs: Arc<AtomicUsize>,
}

fn registry() -> &'static Mutex<HashMap<SessionId, EmitterHandle>> {
    static REGISTRY: OnceLock<Mutex<HashMap<SessionId, EmitterHandle>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get-or-spawn the shared emitter for `session` and join its frame
/// stream. Returns the frames receiver + the shared emit state (the
/// caller locks it — together with the Term, in that order — to take
/// its stamped read-only attach snapshot) + the caller's format
/// registration. `wants_k1` subscribers bump the session's k1
/// refcount HERE — before the caller stamps its version floor — which
/// is what guarantees every floor-clearing frame carries a binary
/// encoding.
pub(crate) fn attach(
    session: &Arc<DaemonPtySession>,
    pane_id: &str,
    wants_k1: bool,
) -> (
    broadcast::Receiver<GridFrame>,
    Arc<Mutex<EmitState>>,
    FormatRegistration,
) {
    let mut reg = registry().lock();
    if let Some(h) = reg.get(&session.session_id) {
        let registration = register_format(&h.k1_subs, wants_k1);
        return (h.frames_tx.subscribe(), h.emit_state.clone(), registration);
    }
    let (frames_tx, frames_rx) = broadcast::channel(FRAMES_CAP);
    let emit_state = Arc::new(Mutex::new(EmitState::default()));
    let k1_subs = Arc::new(AtomicUsize::new(0));
    let registration = register_format(&k1_subs, wants_k1);
    reg.insert(
        session.session_id,
        EmitterHandle {
            frames_tx: frames_tx.clone(),
            emit_state: emit_state.clone(),
            k1_subs: k1_subs.clone(),
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
        k1_subs,
    ));
    (frames_rx, emit_state, registration)
}

fn register_format(k1_subs: &Arc<AtomicUsize>, wants_k1: bool) -> FormatRegistration {
    if wants_k1 {
        k1_subs.fetch_add(1, Ordering::Relaxed);
        FormatRegistration {
            k1_subs: Some(k1_subs.clone()),
        }
    } else {
        FormatRegistration { k1_subs: None }
    }
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
///
/// Synchronized output (DECSET 2026, the BSU/ESU frame brackets
/// Ink/claude emit per repaint) needs NO handling here: it is already
/// coalesced one layer down, inside the parser this emitter's Wakeups
/// come from. vte 0.15's `ansi::Processor` buffers every byte between
/// `\x1b[?2026h` and `\x1b[?2026l` (2 MiB cap) and applies the whole
/// update to the Term at ESU — or at its own 150ms safety timeout —
/// and `alacritty_terminal` 0.26's `EventLoop` (which `daemon_pty`
/// spawns) suppresses the Wakeup entirely while all bytes read were
/// synchronized (`event_loop.rs`: "Queue terminal redraw unless all
/// processed bytes were synchronized"), firing it only at sync-END /
/// timeout. So a Wakeup reaching this task is already a complete
/// frame boundary; a mid-frame torn emit can't happen, and no
/// BSU/ESU deferral belongs here. Verified live 2026-07-02: BSU +
/// three writes staggered over ~100ms + ESU through a scratch
/// session produced zero deltas until ESU, then exactly one frame
/// carrying all three; the same probe with ~300ms inside the window
/// showed the buffered content releasing at +150ms — vte's abort
/// timeout doing its job.
const MIN_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// Resize settle — hard ceiling on blank-frame suppression. A real
/// resize clears the viewport (`daemon_pty::resize`: goto(0,0) +
/// ClearMode::Below, kept deliberately) before the child's SIGWINCH
/// repaint lands; frames built in that window are blank and used to
/// broadcast to every viewer as the visible "black flash". While a
/// settle window is open the emitter drains damage without encoding
/// (the zero-viewer pattern) and the first post-settle emit is forced
/// Full — so no damage is ever lost, versions stay monotonic (the
/// suppressed frames simply never exist on the wire), and the k1
/// floor/ack/resync semantics are untouched. If the child never
/// repaints (apps that ignore SIGWINCH), this timeout publishes the
/// daemon's true (cleared) state rather than freezing viewers on
/// stale geometry forever.
const RESIZE_SETTLE_MAX: std::time::Duration = std::time::Duration::from_millis(150);

/// Evidence-of-repaint threshold. The resize's own clear arrives
/// (if at all) essentially instantly; claude/Ink bracket their
/// SIGWINCH repaint in synchronized output, so it lands as ONE
/// coalesced post-ESU Wakeup comfortably later. Any Wakeup this far
/// into the settle window is therefore treated as the repaint and
/// ends suppression immediately — the Kessel lesson that timers
/// guess but the stream tells you (150ms flat-wait ≙ ~3fps; this
/// path typically settles in single-digit ms after the repaint).
const RESIZE_REPAINT_EVIDENCE_MIN: std::time::Duration = std::time::Duration::from_millis(30);

/// Resize-settle decision for one incoming damage cue (Wakeup /
/// Lagged). Pure state transition over the emitter's settle fields so
/// the Wakeup and Lagged arms share one implementation.
enum SettleAction {
    /// Inside an open settle window and the cue is too early to be
    /// the repaint — drain damage, emit nothing.
    Suppress,
    /// No window open (or this cue is the repaint evidence and closed
    /// it) — proceed to the normal emit path.
    Emit,
}

fn settle_gate(
    session: &DaemonPtySession,
    seen_resize_gen: &mut u64,
    settle_since: &mut Option<std::time::Instant>,
    force_full: &mut bool,
) -> SettleAction {
    // A resize since our last look opens (or re-opens) the window.
    // The cue that carried us here is at most the resize's own clear
    // — never the repaint, which needs a SIGWINCH round-trip first.
    let gen = session.resize_generation();
    if gen != *seen_resize_gen {
        *seen_resize_gen = gen;
        *settle_since = Some(std::time::Instant::now());
        *force_full = true;
        return SettleAction::Suppress;
    }
    if let Some(started) = *settle_since {
        if started.elapsed() < RESIZE_REPAINT_EVIDENCE_MIN {
            return SettleAction::Suppress;
        }
        // Comfortably after the clear ⇒ this is the child's repaint
        // (Ink/claude deliver it as one coalesced post-ESU Wakeup).
        *settle_since = None;
    }
    SettleAction::Emit
}

/// Consume accumulated damage WITHOUT encoding — the zero-viewer
/// pattern, reused while a resize-settle window is open so no damage
/// is ever lost and `EmitState` stays coherent for the forced-Full
/// emit that closes the window.
fn drain_only(session: &Arc<DaemonPtySession>, emit_state: &Arc<Mutex<EmitState>>) {
    let mut st = emit_state.lock();
    let term_mutex = session.term();
    let mut term = term_mutex.lock();
    drain_damage(&mut term, &mut st);
}

/// The emitter task: one per live session, exits on child exit /
/// session teardown (events channel closed) and removes itself from
/// the registry.
async fn run(
    session: Arc<DaemonPtySession>,
    pane_id: String,
    frames_tx: broadcast::Sender<GridFrame>,
    emit_state: Arc<Mutex<EmitState>>,
    k1_subs: Arc<AtomicUsize>,
) {
    let mut events_rx = session.subscribe_events();
    let mut last_emit = std::time::Instant::now() - MIN_FRAME_INTERVAL;
    // Resize settle state — see RESIZE_SETTLE_MAX. `force_full` is
    // sticky until a frame actually reaches the channel: a forced
    // emit that itself races a new resize must stay forced.
    let mut seen_resize_gen = session.resize_generation();
    let mut settle_since: Option<std::time::Instant> = None;
    let mut force_full = false;
    loop {
        // While a settle window is open, bound the wait by its
        // deadline: a child that never repaints on SIGWINCH must not
        // freeze every viewer on stale geometry (Kessel §2.6 — a
        // repaint is not guaranteed).
        let ev = match settle_since {
            Some(started) => {
                let deadline =
                    tokio::time::Instant::from_std(started + RESIZE_SETTLE_MAX);
                tokio::select! {
                    ev = events_rx.recv() => Some(ev),
                    _ = tokio::time::sleep_until(deadline) => None,
                }
            }
            None => Some(events_rx.recv().await),
        };
        let Some(ev) = ev else {
            // Settle timeout — publish the daemon's true state (all
            // damage drained during the window folds into this one
            // forced-Full frame).
            settle_since = None;
            force_full = true;
            match emit_once(
                &session, &pane_id, &frames_tx, &emit_state, &k1_subs,
                force_full, seen_resize_gen,
            ) {
                EmitOutcome::ResizeRaced(gen) => {
                    seen_resize_gen = gen;
                    settle_since = Some(std::time::Instant::now());
                }
                EmitOutcome::Clean => force_full = false,
            }
            last_emit = std::time::Instant::now();
            continue;
        };
        match ev {
            Ok(AlacEvent::Wakeup) => {
                if matches!(
                    settle_gate(
                        &session,
                        &mut seen_resize_gen,
                        &mut settle_since,
                        &mut force_full,
                    ),
                    SettleAction::Suppress
                ) {
                    drain_only(&session, &emit_state);
                    continue;
                }
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
                match emit_once(
                    &session, &pane_id, &frames_tx, &emit_state, &k1_subs,
                    force_full, seen_resize_gen,
                ) {
                    EmitOutcome::ResizeRaced(gen) => {
                        // A resize cleared the grid while this frame
                        // was being built — the frame was discarded;
                        // open the settle window for the repaint.
                        seen_resize_gen = gen;
                        settle_since = Some(std::time::Instant::now());
                        force_full = true;
                    }
                    EmitOutcome::Clean => force_full = false,
                }
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
                // guaranteed to follow. Routed through the same
                // settle gate as Wakeup: a lagged cue mid-window is
                // still just damage, not license to broadcast blank.
                log_debug!(
                    "[daemon/grid-emitter] session {} lagged {n} events — emitting accumulated damage",
                    session.session_id
                );
                if matches!(
                    settle_gate(
                        &session,
                        &mut seen_resize_gen,
                        &mut settle_since,
                        &mut force_full,
                    ),
                    SettleAction::Suppress
                ) {
                    drain_only(&session, &emit_state);
                    continue;
                }
                match emit_once(
                    &session, &pane_id, &frames_tx, &emit_state, &k1_subs,
                    force_full, seen_resize_gen,
                ) {
                    EmitOutcome::ResizeRaced(gen) => {
                        seen_resize_gen = gen;
                        settle_since = Some(std::time::Instant::now());
                        force_full = true;
                    }
                    EmitOutcome::Clean => force_full = false,
                }
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

/// Outcome of one emit pass, for the resize-settle machinery.
enum EmitOutcome {
    /// Frame sent, legitimately skipped, or drained — state is clean.
    Clean,
    /// A resize applied while the frame was being built (the
    /// generation advanced past the caller's expectation, observed
    /// under the term lock): the built frame may be the cleared
    /// intermediate, so it was DISCARDED, never broadcast. Carries
    /// the generation sampled under the lock. The discarded build may
    /// have consumed a version number — harmless, versions only need
    /// to stay monotonic on the wire.
    ResizeRaced(u64),
}

/// One emit pass. Lock order: emit_state, then term — [`attach`]'s
/// snapshot path takes the same locks in the same order, which is
/// what makes its version stamp exact.
fn emit_once(
    session: &Arc<DaemonPtySession>,
    pane_id: &str,
    frames_tx: &broadcast::Sender<GridFrame>,
    emit_state: &Arc<Mutex<EmitState>>,
    k1_subs: &Arc<AtomicUsize>,
    force_full: bool,
    expected_resize_gen: u64,
) -> EmitOutcome {
    // Zero viewers: consume damage WITHOUT encoding so EmitState stays
    // coherent for the next attach (see module docs).
    if frames_tx.receiver_count() == 0 {
        let mut st = emit_state.lock();
        let term_mutex = session.term();
        let mut term = term_mutex.lock();
        drain_damage(&mut term, &mut st);
        return EmitOutcome::Clean;
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
    let (decision, gen) = {
        let mut st = emit_state.lock();
        let term_mutex = session.term();
        let mut term = term_mutex.lock();
        // Post-settle repair: resetting the first-emit latch makes
        // build_emit take its full-snapshot path, which restores
        // every client mirror after the window's drained (never
        // encoded) damage. Resize causes full damage anyway, so the
        // cost is what a delta would have paid.
        if force_full {
            st.has_emitted = false;
        }
        let d = build_emit(pane_id, &mut term, &mut st);
        // Sampled under the SAME term lock `resize()` bumps it in:
        // gen ≠ expected here means the grid we just encoded may be
        // the post-clear blank — the caller opens a settle window
        // instead of broadcasting it.
        (d, session.resize_generation())
    };
    if gen != expected_resize_gen {
        return EmitOutcome::ResizeRaced(gen);
    }
    // One encode per FORMAT per frame: JSON always (the default
    // protocol every subscriber can consume), k1 binary only while
    // the session has ≥1 opted-in subscriber.
    let want_k1 = k1_subs.load(Ordering::Relaxed) > 0;
    let frame = match decision {
        EmitDecision::Full(snap) => serialize_frame(
            snap.version,
            &Outbound::Snapshot(&snap),
            want_k1.then(|| grid_wire::encode_snapshot(&snap)),
        ),
        EmitDecision::Delta(delta) => serialize_frame(
            delta.version,
            &Outbound::Delta(&delta),
            want_k1.then(|| grid_wire::encode_delta(&delta)),
        ),
        EmitDecision::Skip => None,
    };
    if let Some(f) = frame {
        // Err = no receivers — raced a disconnect; nothing to do
        // (damage was legitimately consumed; the next attach takes a
        // full snapshot anyway).
        let _ = frames_tx.send(f);
    }
    EmitOutcome::Clean
}

fn serialize_frame(
    version: u64,
    outbound: &Outbound<'_>,
    binary: Option<Vec<u8>>,
) -> Option<GridFrame> {
    match serde_json::to_string(outbound) {
        Ok(json) => Some(GridFrame {
            version,
            text: std::sync::Arc::from(json),
            binary: binary.map(std::sync::Arc::from),
        }),
        Err(e) => {
            log_debug!("[daemon/grid-emitter] frame serialize failed: {e}");
            None
        }
    }
}
