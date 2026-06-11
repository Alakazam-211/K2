//! `/cli/sessions/grid` WebSocket endpoint (Alacritty_v2).
//!
//! Serves grid snapshots + deltas from a daemon-hosted
//! `DaemonPtySession`'s `alacritty_terminal::Term` to a single
//! Tauri-side thin client. This is the daemon half of the A3 + A5
//! protocol defined in `.k2so/prds/alacritty-v2.md`.
//!
//! Flow:
//!
//!   1. Parse `?session=<UUID>&token=<token>` from query. 400 on
//!      malformed, 403 on auth fail (enforced by caller in main.rs).
//!   2. Look up the session in `v2_session_map`. 400 if not found.
//!   3. WebSocket handshake.
//!   4. Join the session's SHARED grid emitter (0.39.46,
//!      `crate::grid_emitter`) — multiple subscribers per session are
//!      first-class; damage is consumed by exactly ONE emitter task
//!      and pre-encoded frames fan out to every subscriber.
//!   5. Emit an initial READ-ONLY full snapshot as
//!      `{"event":"snapshot","payload":...}`, stamped with the
//!      emitter's current version (frames at/below the stamp are
//!      dropped — the snapshot already covers them).
//!   6. Enter select loop:
//!        - On a shared-emitter frame: forward the pre-encoded
//!          `Snapshot`/`Delta` text when its version clears the stamp.
//!        - On AlacEvent::ChildExit: send final exit message, close WS.
//!        - On inbound `{"action":"input","text":...}`: write to PTY.
//!        - On inbound `{"action":"resize","cols":N,"rows":N}`:
//!          SIGWINCH + Term.resize.
//!        - On client close: exit loop; session stays alive.
//!
//! **Binding the session Arc**: the Arc from `v2_session_map` stays
//! alive for the duration of this handler. On disconnect we drop
//! our clone; if another Arc is held (by the map or a future
//! subscriber), the session persists. Only the map's removal or
//! explicit close tears it down.
//!
//! Message format is JSON text (not binary) for both directions.
//! Bandwidth of a typical delta is small (damaged rows only); the
//! JSON framing is convenient and matches the protocol style of
//! `sessions_ws.rs` / the Awareness Bus.

use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::{
    snapshot_term, AlacEvent, DaemonPtySession, TermGridDelta, TermGridSnapshot,
};

use crate::v2_session_map;

/// Outbound WS message. Tagged as `{"event":"<kind>","payload":...}`.
/// `pub(crate)` since 0.39.46: the shared grid emitter
/// (`crate::grid_emitter`) serializes Snapshot/Delta frames ONCE and
/// fans the encoded text out to every subscriber.
#[derive(Debug, Serialize)]
#[serde(tag = "event", content = "payload", rename_all = "snake_case")]
pub(crate) enum Outbound<'a> {
    /// Full grid + scrollback. Sent once on connect; repeat only
    /// when `build_emit` returns `Full` (e.g. full damage or reset).
    Snapshot(&'a TermGridSnapshot),
    /// Incremental update since the last emit.
    Delta(&'a TermGridDelta),
    /// Child process exit notification. Sent once just before the
    /// server closes the WS. `exit_code` is `None` on signal-kill.
    #[allow(dead_code)]
    ChildExit { exit_code: Option<i32> },
    /// Terminal title change (`OSC 0/1/2`). Used by the renderer
    /// for the same fast-idle / fast-working hints legacy pulls
    /// from `terminal:title:<id>` Tauri events: Claude Code's
    /// braille-spinner prefix while working, the ✱-family glyphs
    /// the moment it goes idle, etc.
    ///
    /// **0.37.4 (Phase B):** the renderer keeps using this for
    /// activity-marker detection only — tab labels come from
    /// `LabelInitial` / `LabelChanged` instead. Title is kept on
    /// the wire so legacy idle/working hints don't regress.
    Title { title: String },
    /// Authoritative label for the session at WS-connect time
    /// (Phase B). Sent once, immediately after the initial
    /// snapshot. Renderer should display this as the tab label
    /// (or whatever surface it owns) until a `LabelChanged`
    /// supersedes it. Daemon-owned — survives renderer reload via
    /// re-fetch on the next connect.
    LabelInitial { label: String },
    /// Authoritative label updated mid-session (Phase B). Fired
    /// when the daemon's PTY-title interceptor accepts a change
    /// (label_source ∈ {Pty, Seed}) or when an explicit caller hits
    /// `set_label` via the new `/cli/sessions/<id>/label` route.
    /// Renderer just replaces its mirror — no decision-making in
    /// the client.
    LabelChanged { label: String },
    /// Bell character (`\a`, OSC 7). Mirrors how iTerm decides
    /// "agent is now waiting for input": Claude rings the bell
    /// when it's done and ready for a reply. Renderer can use
    /// this as a definitive "idle, waiting" transition signal,
    /// independent of any viewport-text scan.
    Bell,
    /// Pre-handshake or handshake-time fatal error. Client should
    /// treat as terminal and may retry once.
    Error { message: String },
}

/// Inbound WS message. Tagged as `{"action":"<kind>",...}`.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Inbound {
    /// User keystroke(s) / paste. UTF-8 text; ESC sequences
    /// encoded as the bytes they represent (`\u001b...`).
    Input { text: String },
    /// Resize request from the client's ResizeObserver.
    Resize { cols: u16, rows: u16 },
    /// 0.37.11 — viewer-claim: this subscriber is becoming the
    /// active viewer (or releasing the claim). Active viewers are
    /// the only subscribers whose `Resize` frames the daemon
    /// honors. Renderer sends `true` on window focus, `false` on
    /// blur. Once the daemon-side `active_subscriber` is set,
    /// resize from non-active subscribers is ignored — eliminates
    /// the "two windows fighting over the TUI size" problem.
    ///
    /// 0.39.43 (PRD `daemon-multi-client-arbitration.md` Issue A) —
    /// the claim now optionally carries the viewer's current viewport
    /// `cols`/`rows`. On a real claim the daemon records them on the
    /// session AND immediately resizes the PTY to them, so the grid
    /// snaps to the active viewer's size on claim instead of waiting
    /// for a follow-up `Resize`. `cols`/`rows` are optional for
    /// back-compat: an older client that sends `{action:'set_active',
    /// active:true}` (no dims) behaves exactly as before — the claim
    /// is recorded but the PTY size is left untouched.
    SetActive {
        active: bool,
        #[serde(default)]
        cols: Option<u16>,
        #[serde(default)]
        rows: Option<u16>,
    },
}

/// 0.37.11 — monotonically-increasing subscriber id generator.
/// Each WS accept claims the next value; the id is passed to the
/// session's `active_subscriber` atomic on viewer-claim. Starts at
/// 1 because 0 is the "no claim" sentinel value.
static NEXT_SUBSCRIBER_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

/// The state transition a `SetActive` frame implies, given the
/// session's current `active_subscriber` value and the requesting
/// subscriber's id. Extracted as a pure function so the idempotence
/// rules (Issue #8) are unit-testable without spinning up a PTY +
/// WebSocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetActiveOutcome {
    /// `active:true` from a subscriber that does NOT currently hold
    /// the claim — store our id (displacing any prior claimer).
    Claim,
    /// `active:false` from the subscriber that currently holds the
    /// claim — clear it (CAS guards against a concurrent takeover).
    Release,
    /// No state change: a redundant claim (we already hold it) or a
    /// redundant release (we never held it). Skip the store + skip
    /// the log so a chatty client can't flood the grid broadcast.
    NoOp,
}

/// Decide what a `SetActive { active }` frame should do given the
/// current `active_subscriber` (0 == no claim) and the requesting
/// `subscriber_id` (always nonzero — 0 is the sentinel).
fn decide_set_active(current: u64, subscriber_id: u64, active: bool) -> SetActiveOutcome {
    if active {
        // Claiming. Only a real state change if we're not already
        // the active subscriber. (current may be 0 = unclaimed, or
        // another subscriber's id = displace them — both are claims.)
        if current == subscriber_id {
            SetActiveOutcome::NoOp
        } else {
            SetActiveOutcome::Claim
        }
    } else {
        // Releasing. Only a real change if we currently hold it.
        if current == subscriber_id {
            SetActiveOutcome::Release
        } else {
            SetActiveOutcome::NoOp
        }
    }
}

/// Apply a `SetActive { active, cols, rows }` frame to the session.
///
/// Computes the transition with [`decide_set_active`] (idempotence
/// rules from Issue #8) and applies only real state changes:
///
/// - **Claim** — store our `subscriber_id` as the active subscriber,
///   displacing any prior claimer (most-recent-claim-wins). 0.39.43
///   (PRD `daemon-multi-client-arbitration.md` Issue A): if the claim
///   carried the viewer's viewport `cols`/`rows`, record them on the
///   session AND immediately [`DaemonPtySession::resize`] the PTY to
///   them — the active viewer drives the size the instant they claim,
///   no waiting for a follow-up `Resize`. Dims are optional for
///   back-compat: an older client that claims with no dims records the
///   claim but leaves the PTY size untouched (pre-0.39.43 behavior).
/// - **Release** — CAS-clear the active subscriber so a viewer that
///   took over concurrently isn't accidentally cleared.
/// - **NoOp** — redundant claim/release: no store, no resize, no log.
///
/// Returns the outcome so callers/tests can assert what happened.
/// Extracted from the WS loop so the store + resize behavior is
/// unit-testable against a real `DaemonPtySession` without a socket.
fn apply_set_active(
    session: &DaemonPtySession,
    subscriber_id: u64,
    active: bool,
    cols: Option<u16>,
    rows: Option<u16>,
) -> SetActiveOutcome {
    let prev = session
        .active_subscriber
        .load(std::sync::atomic::Ordering::Relaxed);
    let outcome = decide_set_active(prev, subscriber_id, active);
    match outcome {
        SetActiveOutcome::Claim => {
            session.active_subscriber.store(
                subscriber_id,
                std::sync::atomic::Ordering::Relaxed,
            );
            if let (Some(c), Some(r)) = (cols, rows) {
                session
                    .active_cols
                    .store(c, std::sync::atomic::Ordering::Relaxed);
                session
                    .active_rows
                    .store(r, std::sync::atomic::Ordering::Relaxed);
                session.resize(c, r);
                log_debug!(
                    "[v2-perf] side=daemon stage=active_claim \
                     session={} sub={} cols={} rows={} \
                     (resized to active viewer)",
                    session.session_id,
                    subscriber_id,
                    c,
                    r,
                );
            } else {
                log_debug!(
                    "[v2-perf] side=daemon stage=active_claim \
                     session={} sub={} (no dims)",
                    session.session_id,
                    subscriber_id,
                );
            }
        }
        SetActiveOutcome::Release => {
            let _ = session.active_subscriber.compare_exchange(
                subscriber_id,
                0,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            );
            log_debug!(
                "[v2-perf] side=daemon stage=active_release session={} sub={}",
                session.session_id,
                subscriber_id,
            );
        }
        SetActiveOutcome::NoOp => {}
    }
    outcome
}

pub async fn serve_session_grid_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
) {
    // 0.39.7: stream borrowed (was owned). See events.rs.
    let session_id = match params.get("session").and_then(|s| SessionId::parse(s)) {
        Some(id) => id,
        None => {
            send_error_then_close(
                stream,
                "missing or malformed 'session' query param",
            )
            .await;
            return;
        }
    };

    let session = match v2_session_map::lookup_by_session_id(&session_id) {
        Some(s) => s,
        None => {
            let msg = format!("session {session_id} not found in v2 session map");
            send_error_then_close(stream, &msg).await;
            return;
        }
    };

    // 0.37.11 — claim a unique subscriber id for this connection.
    // The id stays stable for the WS's lifetime; the renderer's
    // `SetActive` frame stamps this value into the session's
    // `active_subscriber`. `Resize` frames check against it.
    let subscriber_id = NEXT_SUBSCRIBER_ID.fetch_add(
        1,
        std::sync::atomic::Ordering::Relaxed,
    );

    // This connection's last-requested terminal dims, tracked on EVERY
    // `Resize` (even when dropped because we're not the active subscriber)
    // so that when this client sends `Input` and becomes the active viewer
    // (PRD §6.4 — typing claims active), we can snap the PTY to ITS size
    // immediately rather than waiting for a follow-up Resize. 0 = unknown.
    let mut my_cols: u16 = 0;
    let mut my_rows: u16 = 0;

    let __t_accept = std::time::Instant::now();
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/sessions_grid_ws] ws handshake failed: {e}");
            return;
        }
    };
    let ws_accept_ms = __t_accept.elapsed().as_secs_f64() * 1000.0;
    log_debug!(
        "[v2-perf] side=daemon stage=ws_accept ms={:.3} session={}",
        ws_accept_ms,
        session.session_id
    );
    let (mut write, mut read) = ws.split();

    // Subscribe to the session's event broadcast. Multiple
    // subscribers can coexist (though v2 is effectively
    // single-subscriber); remount-during-swap sequences that used
    // to fail with "busy" now subscribe fresh each time and
    // render from a new initial snapshot.
    let mut events_rx = session.subscribe_events();
    // Phase B: subscribe to out-of-band label updates. When the
    // CLI route or another process sets the session's label, we
    // need to push `LabelChanged` to this client.
    let mut labels_rx = session.subscribe_labels();

    let pane_id = format!("alacritty-v2-{}", session.session_id);

    // 0.39.46 — join the session's SHARED emitter. Damage is consumed
    // by exactly one consumer per session (the emitter task); this
    // connection just forwards its pre-encoded frames. Pre-0.39.46,
    // every connection ran its own build_emit and raced the others
    // for the Term's single damage accumulator — with two viewers
    // (host app + K2 Connect client) the loser Skip'd and its mirror
    // silently missed rows (the "starved remote viewer" bug).
    //
    // Order matters: subscribe FIRST, then snapshot — see the version
    // floor below.
    let (mut frames_rx, shared_emit_state) =
        crate::grid_emitter::attach(&session, &pane_id);

    // Initial full snapshot — READ-ONLY. No reset_damage (that would
    // starve every OTHER subscriber of the damage they haven't
    // emitted yet), no EmitState mutation (the emitter owns it).
    // Stamped with the emitter's current version; `frame_floor`
    // then drops any frame at/below the stamp: those cover changes
    // this snapshot already contains, and a delta's
    // `scrollback_appended` is NOT idempotent — forwarding one would
    // duplicate scrollback rows on the client. The (emit_state, term)
    // lock order matches the emitter's emit pass, which is what makes
    // the stamp exact in every interleaving.
    let __t_first_snap = std::time::Instant::now();
    let mut frame_floor: u64;
    let initial_snapshot = {
        let st = shared_emit_state.lock();
        // Bind the Arc<FairMutex<...>> to a local so it outlives
        // the guard. `session.term()` returns a temporary Arc.
        let term_mutex = session.term();
        let term = term_mutex.lock();
        frame_floor = st.version;
        snapshot_term(&pane_id, &*term, frame_floor)
    };
    let snap_rows = initial_snapshot.rows;
    let snap_cols = initial_snapshot.cols;
    let snap_scrollback = initial_snapshot.scrollback.len();
    if send_outbound(&mut write, &Outbound::Snapshot(&initial_snapshot))
        .await
        .is_err()
    {
        // Client disconnected before we could send. `events_rx`
        // drops implicitly on return; broadcast subscribers don't
        // need to be restored.
        return;
    }
    // Phase B: emit the authoritative session label immediately
    // after the snapshot. Subscribers display this as the tab
    // label. If the daemon was given an empty seed (most spawn
    // paths still are during the rollout window), the renderer
    // falls back to its own display-name lookup.
    let initial_label = session.label();
    if send_outbound(
        &mut write,
        &Outbound::LabelInitial { label: initial_label },
    )
    .await
    .is_err()
    {
        return;
    }
    let first_snap_ms = __t_first_snap.elapsed().as_secs_f64() * 1000.0;
    log_debug!(
        "[v2-perf] side=daemon CONNECT-SUMMARY session={} ws_accept_ms={:.3} first_snap_ms={:.3} rows={} cols={} scrollback={}",
        session.session_id,
        ws_accept_ms,
        first_snap_ms,
        snap_rows,
        snap_cols,
        snap_scrollback
    );

    log_debug!(
        "[daemon/sessions_grid_ws] subscriber attached to session {} (pane {})",
        session.session_id,
        pane_id
    );

    // Main loop: event-driven. Every Wakeup from alacritty is a
    // cue to build_emit + send. Inbound messages route to
    // session.write() / session.resize(). No coalescing for v1 —
    // build_emit itself returns Skip when nothing changed, which
    // keeps the volume sane.
    loop {
        tokio::select! {
            // 0.39.46 — pre-encoded Snapshot/Delta frames from the
            // session's shared emitter. The version floor drops frames
            // our attach snapshot already covers (see above).
            frame = frames_rx.recv() => {
                match frame {
                    Ok(f) => {
                        if f.version > frame_floor
                            && write.send(Message::Text(f.text.to_string())).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Fell behind the frame stream (slow link —
                        // tunnel backpressure). Recover with a fresh
                        // READ-ONLY snapshot and re-stamp the floor;
                        // the emitter's state is untouched, so other
                        // subscribers are unaffected.
                        log_debug!(
                            "[daemon/sessions_grid_ws] subscriber lagged {n} frames, sending fresh snapshot"
                        );
                        let snap = {
                            let st = shared_emit_state.lock();
                            let term_mutex = session.term();
                            let term = term_mutex.lock();
                            frame_floor = st.version;
                            snapshot_term(&pane_id, &*term, frame_floor)
                        };
                        if send_outbound(&mut write, &Outbound::Snapshot(&snap))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Emitter exited (child exit / teardown). The
                        // events_rx arm sees ChildExit/Closed and owns
                        // the graceful shutdown — don't break here.
                    }
                }
            }
            // Phase B: out-of-band label changes (from /cli/sessions/<id>/label
            // or a multi-window peer's set). Push to this client so
            // its tab updates without a renderer round-trip.
            label = labels_rx.recv() => {
                match label {
                    Ok(new_label) => {
                        if send_outbound(
                            &mut write,
                            &Outbound::LabelChanged { label: new_label },
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Subscriber fell behind on label events.
                        // Re-emit the current authoritative label so
                        // the client converges.
                        let current = session.label();
                        let _ = send_outbound(
                            &mut write,
                            &Outbound::LabelChanged { label: current },
                        )
                        .await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Session dropped — main loop will see the
                        // events_rx Closed too. Don't break here;
                        // let the event loop terminate cleanly.
                    }
                }
            }
            ev = events_rx.recv() => {
                match ev {
                    Ok(AlacEvent::Wakeup) => {
                        // 0.39.46: grid emission moved to the shared
                        // emitter (frames_rx arm above). Wakeups still
                        // arrive on this subscription; ignore them.
                    }
                    Ok(AlacEvent::ChildExit(status)) => {
                        let exit = Outbound::ChildExit {
                            exit_code: status.code(),
                        };
                        let _ = send_outbound(&mut write, &exit).await;
                        // Send a Close frame before tearing down the
                        // socket so the browser sees a graceful close.
                        // Without this, WebKit fires `onerror` →
                        // frontend renders "ws error" instead of the
                        // child_exit message that just preceded it.
                        let _ = write.send(Message::Close(None)).await;
                        break;
                    }
                    Ok(AlacEvent::Title(title)) => {
                        // Forward as Outbound::Title so the renderer
                        // can use the same idle/working hints legacy
                        // pulls from `terminal:title:<id>` Tauri
                        // events (activity-marker detection only —
                        // post-Phase B, tab labels come from
                        // LabelInitial/LabelChanged instead).
                        let _ = send_outbound(
                            &mut write,
                            &Outbound::Title { title: title.clone() },
                        )
                        .await;
                        // Phase B: feed the title into the daemon's
                        // label state machine. Honors LabelSource;
                        // when not Locked, updates the label and
                        // broadcasts on `label_tx` so EVERY
                        // subscriber's labels_rx arm wakes and emits
                        // `LabelChanged` (multi-window convergence).
                        // Don't emit here — let the broadcast path
                        // handle it uniformly.
                        let _ = session.try_set_label_from_pty(title);
                    }
                    Ok(AlacEvent::ResetTitle) => {
                        // OSC 0 reset → empty title. Treated by the
                        // renderer the same as a non-marker title
                        // (no idle hint).
                        let _ = send_outbound(
                            &mut write,
                            &Outbound::Title { title: String::new() },
                        )
                        .await;
                        // Phase B: empty label → renderer falls
                        // back to its workspace-derived helper.
                        // Same broadcast path as Title.
                        let _ = session.try_set_label_from_pty(String::new());
                    }
                    Ok(AlacEvent::Bell) => {
                        // Bell — used by Claude / Codex to signal
                        // "I'm done and waiting for input." Same
                        // signal iTerm uses for its "agent waiting"
                        // notifications.
                        let _ = send_outbound(&mut write, &Outbound::Bell).await;
                    }
                    Ok(_other) => {
                        // ClipboardStore / ColorRequest / etc.
                        // Ignored for v2 — not part of the minimal
                        // grid-rendering contract.
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // 0.39.46: grid frames ride frames_rx now (its
                        // own Lagged arm re-snapshots). Lagging HERE
                        // only means missed Title/Bell lifecycle
                        // events — re-emit the authoritative label so
                        // the cheap-to-converge surface converges; the
                        // rest is transient by nature.
                        log_debug!(
                            "[daemon/sessions_grid_ws] subscriber lagged {n} lifecycle events"
                        );
                        let current = session.label();
                        if send_outbound(
                            &mut write,
                            &Outbound::LabelChanged { label: current },
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Session dropped (last Arc released). Exit.
                        break;
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let parsed: Result<Inbound, _> = serde_json::from_str(&text);
                        match parsed {
                            Ok(Inbound::Input { text }) => {
                                // PRD §6.4 — typing IS an active-viewer claim.
                                // Across two machines both clients can be
                                // window-focused at once, so a focus-only model
                                // can strand a typing-but-not-refocused viewer
                                // at the wrong PTY size. The most-recent client
                                // to send Input owns the session: flip
                                // `active_subscriber` to us (idempotent) and
                                // snap the PTY to our last-known dims. This also
                                // guarantees the renderer's bare-re-mount claim
                                // suppression can never strand an interacting
                                // viewer. 0 dims = unknown → leave size for our
                                // next Resize.
                                let active = session
                                    .active_subscriber
                                    .load(std::sync::atomic::Ordering::Relaxed);
                                if active != subscriber_id {
                                    session.active_subscriber.store(
                                        subscriber_id,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                    if my_cols > 0 && my_rows > 0 {
                                        session.active_cols.store(
                                            my_cols,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        session.active_rows.store(
                                            my_rows,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        session.resize(my_cols, my_rows);
                                    }
                                    log_debug!(
                                        "[v2-perf] side=daemon stage=active_claim_via_input \
                                         session={} sub={} cols={} rows={}",
                                        session.session_id,
                                        subscriber_id,
                                        my_cols,
                                        my_rows,
                                    );
                                }
                                session.write(text.into_bytes());
                            }
                            Ok(Inbound::Resize { cols, rows }) => {
                                // Remember our own requested dims even when the
                                // resize is dropped below (non-active) — an
                                // Input claim (above) snaps the PTY to these.
                                my_cols = cols;
                                my_rows = rows;
                                // 0.37.11 — only the active subscriber
                                // can resize. `active = 0` means "no
                                // claim yet" — accept (first-resize-
                                // wins, preserves single-viewer behavior
                                // for sessions where no one ever sends
                                // SetActive). Otherwise gate on match.
                                let active = session
                                    .active_subscriber
                                    .load(std::sync::atomic::Ordering::Relaxed);
                                if active == 0 || active == subscriber_id {
                                    session.active_cols.store(
                                        cols,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                    session.active_rows.store(
                                        rows,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                    session.resize(cols, rows);
                                } else {
                                    log_debug!(
                                        "[v2-perf] side=daemon stage=resize_ignored \
                                         session={} from_sub={} active_sub={} \
                                         (non-active viewer resize dropped)",
                                        session.session_id,
                                        subscriber_id,
                                        active,
                                    );
                                }
                            }
                            Ok(Inbound::SetActive { active, cols, rows }) => {
                                // Issue #8 backstop: make the claim/release
                                // handler idempotent. A long-lived window
                                // with many mounted-but-hidden panes used
                                // to fire `set_active(true)` from every pane
                                // on each window-focus event, re-storing the
                                // same `active_subscriber` and re-logging on
                                // every redundant claim. With the renderer
                                // now keying on visible+focused, this should
                                // be quiet — but we enforce it daemon-side so
                                // a noisy/buggy client can't reintroduce the
                                // grid-broadcast flood that produced the
                                // "stall then recover" symptom. We compute the
                                // transition with a pure helper (unit-tested
                                // below), apply only real state changes, and
                                // only log when something actually changed.
                                apply_set_active(
                                    &session,
                                    subscriber_id,
                                    active,
                                    cols,
                                    rows,
                                );
                            }
                            Err(e) => {
                                log_debug!(
                                    "[daemon/sessions_grid_ws] malformed inbound: {e}"
                                );
                                // Non-fatal — ignore and keep the socket open.
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        // Binary inbound not used by v2 protocol; drop.
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if write.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        // Echo a Close frame so the client sees a clean
                        // graceful-close handshake. Without this, WebKit
                        // logs "The network connection was lost" because
                        // it gets TCP FIN before our Close frame, which
                        // RFC 6455 §7 calls an abnormal close. The frame
                        // payload is mirrored per spec recommendation.
                        let _ = write.send(Message::Close(frame)).await;
                        break;
                    }
                    None => break,
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(e)) => {
                        log_debug!(
                            "[daemon/sessions_grid_ws] ws read error: {e}"
                        );
                        break;
                    }
                }
            }
        }
    }

    // 0.37.11 — release the active claim on disconnect IF we still
    // hold it. CAS so a viewer that took over our claim before we
    // disconnected isn't accidentally cleared.
    let _ = session.active_subscriber.compare_exchange(
        subscriber_id,
        0,
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
    );

    // Drop `events_rx` implicitly on return. Broadcast subscribers
    // don't need to be "restored" — the next connection just calls
    // `subscribe_events()` for a fresh receiver.
    drop(events_rx);

    log_debug!(
        "[daemon/sessions_grid_ws] subscriber detached from session {} (sub_id={})",
        session.session_id,
        subscriber_id,
    );
}

async fn send_outbound<W>(write: &mut W, msg: &Outbound<'_>) -> Result<(), ()>
where
    W: futures_util::SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let text = match serde_json::to_string(msg) {
        Ok(s) => s,
        Err(e) => {
            log_debug!(
                "[daemon/sessions_grid_ws] serialize outbound failed: {e}"
            );
            return Err(());
        }
    };
    write.send(Message::Text(text.into())).await.map_err(|e| {
        log_debug!("[daemon/sessions_grid_ws] send failed: {e}");
    })
}

async fn send_error_then_close(stream: &mut TcpStream, msg: &str) {
    let err = Outbound::Error {
        message: msg.to_string(),
    };
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut write, _read) = ws.split();
    let _ = send_outbound(&mut write, &err).await;
    let _ = write.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Issue #8: the active-viewer claim/release handler must be
    // idempotent. These tests pin the pure decision so a future edit
    // can't silently reintroduce the redundant-store + redundant-log
    // churn that flooded the per-session grid broadcast channel.

    #[test]
    fn claiming_when_unclaimed_is_a_real_claim() {
        // active_subscriber == 0 (no claim), sub 7 claims → Claim.
        assert_eq!(decide_set_active(0, 7, true), SetActiveOutcome::Claim);
    }

    #[test]
    fn claiming_twice_with_same_subscriber_is_a_noop() {
        // We already hold the claim; re-asserting it changes nothing
        // and must not store or log again.
        assert_eq!(decide_set_active(7, 7, true), SetActiveOutcome::NoOp);
    }

    #[test]
    fn claiming_when_another_holds_it_displaces() {
        // Most-recent-claim-wins: sub 9 claims while sub 7 holds it →
        // a real Claim (the handler stores 9, displacing 7).
        assert_eq!(decide_set_active(7, 9, true), SetActiveOutcome::Claim);
    }

    #[test]
    fn releasing_our_own_claim_is_a_real_release() {
        assert_eq!(decide_set_active(7, 7, false), SetActiveOutcome::Release);
    }

    #[test]
    fn releasing_when_unclaimed_is_a_noop() {
        // Never held it (current == 0) → release is a no-op, no CAS
        // attempt, no log.
        assert_eq!(decide_set_active(0, 7, false), SetActiveOutcome::NoOp);
    }

    #[test]
    fn releasing_someone_elses_claim_is_a_noop() {
        // sub 7 sends release while sub 9 holds the claim — must NOT
        // clear sub 9's claim and must NOT log. The runtime CAS would
        // also reject this, but we short-circuit before logging.
        assert_eq!(decide_set_active(9, 7, false), SetActiveOutcome::NoOp);
    }

    // 0.39.43 (PRD `daemon-multi-client-arbitration.md` Issue A):
    // `apply_set_active` must record the active viewer's dims on the
    // session AND snap the PTY to them on a real claim, so the active
    // viewer drives the size the instant they claim. These tests spawn
    // a real `DaemonPtySession` (a `cat` subprocess on unix, mirroring
    // the daemon_pty.rs label tests) and exercise the helper the WS
    // loop calls. They require a tokio runtime because the session's
    // broadcast channel lives inside a tokio module.

    #[cfg(unix)]
    fn spawn_test_session() -> std::sync::Arc<DaemonPtySession> {
        use k2_core::terminal::{DaemonPtyConfig, LabelSource};
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(std::path::PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
        };
        DaemonPtySession::spawn(cfg).expect("spawn cat")
    }

    #[cfg(unix)]
    #[test]
    fn claim_with_dims_records_and_resizes_pty() {
        use k2_core::terminal::Dimensions;
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_test_session();
        // Sanity: starts at the spawn dims, no dims captured yet.
        {
            let tm = s.term();
            let t = tm.lock();
            assert_eq!(t.columns() as u16, 80);
            assert_eq!(t.screen_lines() as u16, 24);
        }
        assert_eq!(s.active_cols.load(Relaxed), 0);
        assert_eq!(s.active_rows.load(Relaxed), 0);

        // Subscriber 7 claims active WITH its viewport dims.
        let outcome = apply_set_active(&s, 7, true, Some(120), Some(40));
        assert_eq!(outcome, SetActiveOutcome::Claim);
        assert_eq!(s.active_subscriber.load(Relaxed), 7);
        // Dims recorded on the session...
        assert_eq!(s.active_cols.load(Relaxed), 120);
        assert_eq!(s.active_rows.load(Relaxed), 40);
        // ...AND the PTY snapped to them immediately (no follow-up Resize).
        {
            let tm = s.term();
            let t = tm.lock();
            assert_eq!(t.columns() as u16, 120, "PTY must resize to claimer dims");
            assert_eq!(t.screen_lines() as u16, 40);
        }
    }

    #[cfg(unix)]
    #[test]
    fn second_claim_resizes_to_its_dims() {
        use k2_core::terminal::Dimensions;
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_test_session();
        // Sub 7 claims at 120x40.
        assert_eq!(
            apply_set_active(&s, 7, true, Some(120), Some(40)),
            SetActiveOutcome::Claim
        );
        // Sub 9 claims (displacing 7) at ITS dims 100x30 — most-recent
        // viewer wins, PTY resizes to 9's size.
        let outcome = apply_set_active(&s, 9, true, Some(100), Some(30));
        assert_eq!(outcome, SetActiveOutcome::Claim);
        assert_eq!(s.active_subscriber.load(Relaxed), 9);
        assert_eq!(s.active_cols.load(Relaxed), 100);
        assert_eq!(s.active_rows.load(Relaxed), 30);
        {
            let tm = s.term();
            let t = tm.lock();
            assert_eq!(t.columns() as u16, 100, "PTY must follow most-recent claimer");
            assert_eq!(t.screen_lines() as u16, 30);
        }
    }

    #[cfg(unix)]
    #[test]
    fn claim_without_dims_is_back_compat_no_resize() {
        use k2_core::terminal::Dimensions;
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_test_session();
        // Older client: claim with NO dims. Records the claim, leaves
        // the PTY size untouched (pre-0.39.43 behavior).
        let outcome = apply_set_active(&s, 7, true, None, None);
        assert_eq!(outcome, SetActiveOutcome::Claim);
        assert_eq!(s.active_subscriber.load(Relaxed), 7);
        assert_eq!(s.active_cols.load(Relaxed), 0, "no dims captured");
        assert_eq!(s.active_rows.load(Relaxed), 0);
        {
            let tm = s.term();
            let t = tm.lock();
            assert_eq!(t.columns() as u16, 80, "PTY size unchanged on dimless claim");
            assert_eq!(t.screen_lines() as u16, 24);
        }
    }

    #[cfg(unix)]
    #[test]
    fn release_clears_active_subscriber() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_test_session();
        apply_set_active(&s, 7, true, Some(120), Some(40));
        assert_eq!(s.active_subscriber.load(Relaxed), 7);
        // `active:false` from the holder releases the claim.
        let outcome = apply_set_active(&s, 7, false, None, None);
        assert_eq!(outcome, SetActiveOutcome::Release);
        assert_eq!(s.active_subscriber.load(Relaxed), 0, "claim cleared on release");
    }

    #[cfg(unix)]
    #[test]
    fn back_compat_deserialize_set_active_without_dims() {
        // An older client sends `{action:'set_active', active:true}`
        // with no cols/rows — must deserialize with None dims.
        let parsed: Inbound =
            serde_json::from_str(r#"{"action":"set_active","active":true}"#)
                .expect("legacy set_active must still parse");
        match parsed {
            Inbound::SetActive { active, cols, rows } => {
                assert!(active);
                assert_eq!(cols, None);
                assert_eq!(rows, None);
            }
            other => panic!("expected SetActive, got {other:?}"),
        }
        // New client with dims parses them.
        let parsed: Inbound = serde_json::from_str(
            r#"{"action":"set_active","active":true,"cols":120,"rows":40}"#,
        )
        .expect("set_active with dims must parse");
        match parsed {
            Inbound::SetActive { active, cols, rows } => {
                assert!(active);
                assert_eq!(cols, Some(120));
                assert_eq!(rows, Some(40));
            }
            other => panic!("expected SetActive, got {other:?}"),
        }
    }
}
