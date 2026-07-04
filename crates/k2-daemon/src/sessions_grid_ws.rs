//! `/cli/sessions/grid` WebSocket endpoint (Kessel).
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
//! Message format is JSON text for both directions by default. A
//! client may opt into the "k1" binary wire with `&proto=k1` on the
//! WS URL: `snapshot`/`delta` frames are then sent as WS BINARY
//! messages in the fixed-layout format of
//! `k2_core::terminal::grid_wire` (~2.5× smaller, DataView-decoded
//! client-side), while every other event (title, bell, labels,
//! child_exit, error) and ALL inbound actions stay JSON text. k1
//! clients additionally `{"action":"ack","version":N}` each applied
//! frame batch, which drives the ack-gated pacing below; non-opted
//! clients keep the pre-k1 behavior byte-identical (mixed-version
//! fleet requirement).

use std::collections::{HashMap, VecDeque};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::{
    grid_wire, snapshot_term, AlacEvent, DaemonPtySession, TermGridDelta,
    TermGridSnapshot,
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
    /// S7a pin-to-size — current pin state at WS-connect time. Sent
    /// once, right after `LabelInitial`, ONLY when the session is
    /// pinned (absence = unpinned, so an older client / unpinned
    /// session sees a byte-identical connect sequence). `set_by` is
    /// "owner" or the pinning connect-user's username.
    PinInitial { cols: u16, rows: u16, set_by: Option<String> },
    /// S7a pin-to-size — pin state changed mid-session (the
    /// `LabelChanged` pattern, fed by the session's pin broadcast).
    /// Pinned: `{"cols":N,"rows":N,"cleared":false}`. Unpinned:
    /// `{"cleared":true}` (cols/rows omitted).
    PinChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        cols: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
        cleared: bool,
    },
    /// Bell character (`\a`, OSC 7). Mirrors how iTerm decides
    /// "agent is now waiting for input": Claude rings the bell
    /// when it's done and ready for a reply. Renderer can use
    /// this as a definitive "idle, waiting" transition signal,
    /// independent of any viewport-text scan.
    Bell,
    /// OSC 52 clipboard STORE from the child app (`ESC]52;c;…`),
    /// `text` already base64-decoded by alacritty. Sent to EVERY
    /// viewer of the session — the daemon doesn't guess focus; each
    /// client applies it to ITS OS clipboard only while it is the
    /// active viewer (wezterm's attached-client model, deliberately
    /// not tmux's stomp-every-client). Copy direction ONLY: the
    /// read-back/query form is a clipboard-exfiltration primitive,
    /// is denied by alacritty's default `Osc52::OnlyCopy` policy,
    /// and the daemon never writes a response. Payloads over
    /// [`CLIPBOARD_MAX_BYTES`] are dropped whole (see
    /// [`clipboard_frame`]). Rides the JSON event channel like
    /// title/bell — the k1 binary wire is unaffected.
    Clipboard { text: String },
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
    /// k1 wire — the client applied a frame batch; `version` is the
    /// highest frame version it has rendered. Sent once per rAF
    /// flush (not per message). Drives the ack-gated pacing: the
    /// daemon stops forwarding deltas to a connection whose unacked
    /// backlog exceeds [`UNACKED_FRAMES_MAX`] / [`UNACKED_BYTES_MAX`]
    /// and resyncs it with a fresh full snapshot on its next ack.
    /// JSON (non-k1) clients never send this; an older daemon
    /// receiving it logs "malformed inbound" and carries on.
    Ack { version: u64 },
}

/// k1 pacing: stop forwarding frames to a connection once this many
/// forwarded frames are unacknowledged...
const UNACKED_FRAMES_MAX: usize = 32;
/// ...or once the unacked frames total this many payload bytes,
/// whichever trips first. Bounds the worst case for a slow viewer:
/// instead of an unbounded delta firehose (or broadcast-`Lagged`
/// storms, whose recovery payload is the LARGEST message we have —
/// a full snapshot with scrollback — at the worst time), it gets a
/// quiet gap and ONE authoritative snapshot when it catches up.
const UNACKED_BYTES_MAX: usize = 2 * 1024 * 1024;

/// Ceiling on a forwarded OSC 52 payload (decoded bytes). Anything a
/// human meaningfully copies fits far under this; vte's std build
/// buffers OSC payloads UNBOUNDED (`osc_raw: Vec<u8>`,
/// vte-0.15.0/src/lib.rs:64), so without this cap a hostile/buggy
/// child could push multi-MB strings at every attached viewer.
const CLIPBOARD_MAX_BYTES: usize = 1024 * 1024;

/// Gate an OSC 52 store into an outbound frame. `None` = drop the
/// payload WHOLE — a silently-truncated clipboard is worse than none.
fn clipboard_frame(text: String) -> Option<Outbound<'static>> {
    if text.len() > CLIPBOARD_MAX_BYTES {
        return None;
    }
    Some(Outbound::Clipboard { text })
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
    session: &std::sync::Arc<DaemonPtySession>,
    subscriber_id: u64,
    active: bool,
    cols: Option<u16>,
    rows: Option<u16>,
) -> SetActiveOutcome {
    // Any frame that declares dims refreshes this subscriber's
    // viewport record (+ recency) — even a redundant claim or a
    // release. The registry is what the detach election restores
    // from, so it must track the newest declared geometry.
    if let (Some(c), Some(r)) = (cols, rows) {
        session.note_viewport(subscriber_id, c, r);
    }
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
                // Through the debouncer: racing claims from two clients
                // (or a viewport flapping with the mobile keyboard)
                // coalesce to the final geometry instead of thrashing
                // the PTY through resize+reflow cycles.
                DaemonPtySession::request_resize(session, c, r);
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
    owner_token: String,
) {
    // The token that authorized this connection. Re-validated on a timer
    // inside the loop so a revoked connect-user (disabled / removed /
    // role-changed) is dropped from their LIVE socket within seconds —
    // not left streaming until they happen to disconnect.
    let token = params.get("token").cloned().unwrap_or_default();

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

    // k1 binary wire opt-in. Anything other than an exact `proto=k1`
    // (absent, empty, unknown value) keeps the JSON default — an
    // older client's behavior stays byte-identical.
    let proto_k1 = params.get("proto").map(|p| p == "k1").unwrap_or(false);

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
    // 2026-07-02 (PTY-leak Bug 2) — register as a REAL viewer for this
    // connection's lifetime. `subscriber_count()` counts these
    // registrations, NOT events_tx receivers (internal daemon observers
    // hold receivers too and used to inflate the count so it never hit
    // zero, permanently defeating the GH#22 un-forced close guard). The
    // RAII guard decrements on ANY exit path out of this handler. It
    // also latches `ever_attached` (v2_spawn's never-attached bare-shell
    // cap) — one write point, so the two signals can never disagree.
    let _viewer_reg = session.attach_viewer();
    // Phase B: subscribe to out-of-band label updates. When the
    // CLI route or another process sets the session's label, we
    // need to push `LabelChanged` to this client.
    let mut labels_rx = session.subscribe_labels();
    // S7a: subscribe to pin-state changes (same pattern) so this
    // client's pin UI converges when any caller pins/unpins.
    let mut pins_rx = session.subscribe_pins();

    let pane_id = format!("kessel-{}", session.session_id);

    // 0.39.46 — join the session's SHARED emitter. Damage is consumed
    // by exactly one consumer per session (the emitter task); this
    // connection just forwards its pre-encoded frames. Pre-0.39.46,
    // every connection ran its own build_emit and raced the others
    // for the Term's single damage accumulator — with two viewers
    // (host app + K2 Connect client) the loser Skip'd and its mirror
    // silently missed rows (the "starved remote viewer" bug).
    //
    // Order matters: subscribe FIRST, then snapshot — see the version
    // floor below. `_format_reg` is the RAII k1-refcount registration;
    // holding it for the connection's lifetime keeps the emitter
    // binary-encoding, and dropping it on any exit path releases that.
    let (mut frames_rx, shared_emit_state, _format_reg) =
        crate::grid_emitter::attach(&session, &pane_id, proto_k1);

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
    if send_snapshot(&mut write, &initial_snapshot, proto_k1)
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
    // S7a: tell a fresh subscriber about an existing pin, right
    // after the label (the same initial-state slot). Only sent when
    // pinned — an unpinned session's connect sequence is unchanged.
    if let Some((pin_cols, pin_rows)) = session.pinned() {
        if send_outbound(
            &mut write,
            &Outbound::PinInitial {
                cols: pin_cols,
                rows: pin_rows,
                set_by: session.pinned_set_by(),
            },
        )
        .await
        .is_err()
        {
            return;
        }
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
    // ("A client actually looked at this session" is latched by the
    // `attach_viewer()` registration above — see the PTY-leak Bug 2
    // comment — so `v2_spawn`'s never-attached bare-shell cap and the
    // live viewer count share one attach definition.)

    // Re-auth heartbeat: every 5s, confirm the token that opened this
    // socket is still valid. When an owner disables/removes/role-changes a
    // connect-user, `revoke_user_sessions` drops their token from the
    // in-memory session map — but an already-upgraded WebSocket would keep
    // streaming this machine's live terminal until the client happened to
    // disconnect. This timer closes that hole: a revoked token fails
    // `token_still_valid` and we tear the socket down. The owner token is
    // never revoked, so owner connections sail through the compare.
    let mut auth_recheck =
        tokio::time::interval(std::time::Duration::from_secs(5));
    // Burn the immediate first tick so we don't re-validate the instant
    // after the dispatcher already authorized us.
    auth_recheck.tick().await;

    // k1 ack-gated pacing state. Only ever mutated when `proto_k1`;
    // a JSON connection never touches it (today's behavior, no acks
    // expected). `unacked` holds (version, payload-bytes) of frames
    // forwarded but not yet covered by a client ack; when it exceeds
    // the thresholds we stop forwarding (`paused`) — the SHARED
    // emitter keeps running for every other subscriber — and the
    // connection's next ack triggers a fresh read-only full snapshot
    // through the same version-floor machinery as attach/Lagged.
    let mut unacked: VecDeque<(u64, usize)> = VecDeque::new();
    let mut unacked_bytes: usize = 0;
    let mut paused = false;

    // Main loop: event-driven. Every Wakeup from alacritty is a
    // cue to build_emit + send. Inbound messages route to
    // session.write() / session.resize(). No coalescing for v1 —
    // build_emit itself returns Skip when nothing changed, which
    // keeps the volume sane.
    loop {
        tokio::select! {
            // Re-auth heartbeat — see `auth_recheck` above. Closes the
            // socket the moment the connecting user is revoked.
            _ = auth_recheck.tick() => {
                // Accept the owner/connect-user token OR the P3b per-session
                // stream token bound to THIS session (revoked on teardown +
                // TTL-bounded), so an external /v1/sandboxes viewer isn't torn
                // off a still-live stream.
                let still_ok = crate::routes::http::token_still_valid(&token, &owner_token)
                    || crate::stream_token::authorizes_session(&token, &session.session_id);
                if !still_ok {
                    log_debug!(
                        "[daemon/sessions_grid_ws] token revoked mid-session; \
                         closing grid WS for session {}",
                        session.session_id
                    );
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
            }
            // 0.39.46 — pre-encoded Snapshot/Delta frames from the
            // session's shared emitter. The version floor drops frames
            // our attach snapshot already covers (see above).
            frame = frames_rx.recv() => {
                match frame {
                    Ok(f) => {
                        if f.version > frame_floor {
                            if proto_k1 && paused {
                                // Ack backlog over threshold: drop the
                                // frame for THIS connection only. The
                                // resync snapshot on the next ack is
                                // stamped with a fresh floor, so every
                                // frame skipped here is covered by it
                                // — including `scrollback_appended`
                                // rows, which are NOT idempotent and
                                // must never be both skipped-then-
                                // snapshotted AND replayed.
                            } else {
                                let msg = if proto_k1 {
                                    match &f.binary {
                                        Some(b) => Message::Binary(b.to_vec()),
                                        // Unreachable for floor-clearing
                                        // frames (attach bumps the k1
                                        // refcount before the floor is
                                        // stamped); the JSON text is
                                        // still a correct fallback —
                                        // the client decodes both.
                                        None => Message::Text(f.text.to_string()),
                                    }
                                } else {
                                    Message::Text(f.text.to_string())
                                };
                                let payload_len = msg.len();
                                if write.send(msg).await.is_err() {
                                    break;
                                }
                                if proto_k1 {
                                    unacked.push_back((f.version, payload_len));
                                    unacked_bytes += payload_len;
                                    if unacked.len() >= UNACKED_FRAMES_MAX
                                        || unacked_bytes >= UNACKED_BYTES_MAX
                                    {
                                        if !paused {
                                            log_debug!(
                                                "[daemon/sessions_grid_ws] k1 ack backlog \
                                                 ({} frames / {} bytes) — pausing deltas for \
                                                 session {} sub {}",
                                                unacked.len(),
                                                unacked_bytes,
                                                session.session_id,
                                                subscriber_id,
                                            );
                                        }
                                        paused = true;
                                    }
                                }
                            }
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
                        // The re-stamped floor supersedes the ack
                        // backlog: frames ≤ floor are covered by this
                        // snapshot, so pacing restarts clean (stale
                        // acks for them pop nothing).
                        unacked.clear();
                        unacked_bytes = 0;
                        paused = false;
                        if send_snapshot(&mut write, &snap, proto_k1)
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
            // S7a: pin-state changes (from /cli/terminal/pin-size or a
            // restart-restore). Push to this client so every window's
            // pin UI + letterboxing converge without polling.
            pin = pins_rx.recv() => {
                match pin {
                    Ok(state) => {
                        let frame = match state {
                            Some((c, r)) => Outbound::PinChanged {
                                cols: Some(c),
                                rows: Some(r),
                                cleared: false,
                            },
                            None => Outbound::PinChanged {
                                cols: None,
                                rows: None,
                                cleared: true,
                            },
                        };
                        if send_outbound(&mut write, &frame).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Fell behind on pin events — re-emit the
                        // current authoritative state so the client
                        // converges (mirror of the label Lagged arm).
                        let frame = match session.pinned() {
                            Some((c, r)) => Outbound::PinChanged {
                                cols: Some(c),
                                rows: Some(r),
                                cleared: false,
                            },
                            None => Outbound::PinChanged {
                                cols: None,
                                rows: None,
                                cleared: true,
                            },
                        };
                        let _ = send_outbound(&mut write, &frame).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Session dropped — the events_rx arm owns
                        // the graceful shutdown; don't break here.
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
                    Ok(AlacEvent::ClipboardStore(_, text)) => {
                        // OSC 52 copy — forward to this viewer (every
                        // attached WS runs this same arm, so the frame
                        // reaches ALL viewers; the client applies it
                        // only when active — see Outbound::Clipboard).
                        // The load/query form never reaches here:
                        // alacritty's default OnlyCopy policy denies
                        // it, and no response bytes are ever written
                        // back to the PTY.
                        let len = text.len();
                        match clipboard_frame(text) {
                            Some(frame) => {
                                if send_outbound(&mut write, &frame)
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            None => {
                                log_debug!(
                                    "[daemon/sessions_grid_ws] OSC52 payload \
                                     ({len}B) over {CLIPBOARD_MAX_BYTES}B cap — \
                                     dropped (session {})",
                                    session.session_id
                                );
                            }
                        }
                    }
                    Ok(_other) => {
                        // ClipboardLoad / ColorRequest / etc.
                        // Ignored for v2 — not part of the minimal
                        // grid-rendering contract (and ClipboardLoad
                        // must NEVER be answered — see
                        // Outbound::Clipboard).
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
                                        session.note_viewport(
                                            subscriber_id,
                                            my_cols,
                                            my_rows,
                                        );
                                        session.active_cols.store(
                                            my_cols,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        session.active_rows.store(
                                            my_rows,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        DaemonPtySession::request_resize(
                                            &session, my_cols, my_rows,
                                        );
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
                                // Input claim (above) snaps the PTY to these,
                                // and the session-side registry feeds the
                                // detach election (restore-on-disconnect).
                                my_cols = cols;
                                my_rows = rows;
                                session.note_viewport(subscriber_id, cols, rows);
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
                                    // Debounced: a flapping viewport (mobile
                                    // keyboard show/hide) coalesces to its
                                    // final geometry; the very first fit of a
                                    // fresh session still applies instantly.
                                    DaemonPtySession::request_resize(
                                        &session, cols, rows,
                                    );
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
                            Ok(Inbound::Ack { version }) => {
                                // k1 pacing. Acks from a non-k1 client
                                // are ignored (none exist today; the
                                // guard keeps JSON connections exactly
                                // on their pre-k1 path).
                                if !proto_k1 {
                                    continue;
                                }
                                while unacked
                                    .front()
                                    .is_some_and(|(v, _)| *v <= version)
                                {
                                    let (_, n) = unacked.pop_front().unwrap();
                                    unacked_bytes -= n;
                                }
                                if paused {
                                    // First ack after the backlog
                                    // tripped: resync with a fresh
                                    // READ-ONLY snapshot through the
                                    // SAME version-floor machinery as
                                    // attach/Lagged — the re-stamped
                                    // floor guarantees no delta whose
                                    // `scrollback_appended` rows this
                                    // snapshot already contains can be
                                    // forwarded afterwards (those rows
                                    // are concatenated client-side,
                                    // NOT idempotent), and every frame
                                    // dropped while paused is covered.
                                    let snap = {
                                        let st = shared_emit_state.lock();
                                        let term_mutex = session.term();
                                        let term = term_mutex.lock();
                                        frame_floor = st.version;
                                        snapshot_term(&pane_id, &*term, frame_floor)
                                    };
                                    unacked.clear();
                                    unacked_bytes = 0;
                                    paused = false;
                                    log_debug!(
                                        "[daemon/sessions_grid_ws] k1 ack (v{version}) after \
                                         backlog pause — resync snapshot at floor {} for \
                                         session {} sub {}",
                                        frame_floor,
                                        session.session_id,
                                        subscriber_id,
                                    );
                                    if send_snapshot(&mut write, &snap, true)
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
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

    // Evict this connection's viewport record; if we held the active
    // claim, the session promotes the most-recently-active surviving
    // subscriber and restores the PTY to THEIR dims (orca study
    // takeaway #1 — without this, a session a phone briefly
    // commandeered stayed stuck at phone dimensions). With no
    // survivor the claim clears to 0, exactly the pre-election CAS
    // behavior this replaces.
    DaemonPtySession::detach_subscriber(&session, subscriber_id);

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

/// Send a full snapshot in the connection's negotiated wire format:
/// k1 → one binary `grid_wire` message, default → the standard JSON
/// text frame. Used by the attach snapshot and both resync paths
/// (broadcast `Lagged`, ack-backlog recovery).
async fn send_snapshot<W>(
    write: &mut W,
    snap: &TermGridSnapshot,
    k1: bool,
) -> Result<(), ()>
where
    W: futures_util::SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    if k1 {
        write
            .send(Message::Binary(grid_wire::encode_snapshot(snap)))
            .await
            .map_err(|e| {
                log_debug!("[daemon/sessions_grid_ws] send failed: {e}");
            })
    } else {
        send_outbound(write, &Outbound::Snapshot(snap)).await
    }
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
            sandbox: k2_core::terminal::SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
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

    /// Poll the Term until it reaches `want` or the deadline passes.
    /// Debounced resizes flush within `RESIZE_DEBOUNCE_MS`; failing
    /// LOUDLY with the last observed dims, never a silent skip.
    #[cfg(unix)]
    fn assert_dims_settle(s: &DaemonPtySession, want: (u16, u16), what: &str) {
        use k2_core::terminal::Dimensions;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let got = {
                let tm = s.term();
                let t = tm.lock();
                (t.columns() as u16, t.screen_lines() as u16)
            };
            if got == want {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("{what}: PTY dims never settled to {want:?} (last saw {got:?})");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn second_claim_resizes_to_its_dims() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_test_session();
        // Sub 7 claims at 120x40.
        assert_eq!(
            apply_set_active(&s, 7, true, Some(120), Some(40)),
            SetActiveOutcome::Claim
        );
        // Sub 9 claims (displacing 7) at ITS dims 100x30 — most-recent
        // viewer wins. The claim lands inside the debounce window of
        // 7's resize, so the PTY reaches 9's size on the trailing
        // flush rather than instantly (coalescing is the point: only
        // the final geometry of a claim race gets applied).
        let outcome = apply_set_active(&s, 9, true, Some(100), Some(30));
        assert_eq!(outcome, SetActiveOutcome::Claim);
        assert_eq!(s.active_subscriber.load(Relaxed), 9);
        assert_eq!(s.active_cols.load(Relaxed), 100);
        assert_eq!(s.active_rows.load(Relaxed), 30);
        assert_dims_settle(&s, (100, 30), "PTY must follow most-recent claimer");
    }

    #[cfg(unix)]
    #[test]
    fn active_disconnect_restores_survivor_dims() {
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_test_session();
        // Sub 7 (desktop) claims at 120x40; sub 9 (phone) then
        // commandeers the session at 40x10.
        apply_set_active(&s, 7, true, Some(120), Some(40));
        apply_set_active(&s, 9, true, Some(40), Some(10));
        assert_dims_settle(&s, (40, 10), "phone claim");

        // The phone's WS drops. The teardown path runs the detach
        // election: sub 7 is promoted and the PTY restores to ITS
        // dims — no more sessions stuck at phone dimensions.
        DaemonPtySession::detach_subscriber(&s, 9);
        assert_eq!(s.active_subscriber.load(Relaxed), 7, "survivor promoted");
        assert_eq!(s.active_cols.load(Relaxed), 120);
        assert_eq!(s.active_rows.load(Relaxed), 40);
        assert_dims_settle(&s, (120, 40), "restore on active disconnect");
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

    // ── S7a pin-to-size — the claim paths cannot move a pinned PTY ──
    //
    // The clamp sits in `DaemonPtySession::request_resize`, which is
    // the single funnel for all four resize paths. These tests cover
    // the two claim-shaped callers that live in THIS crate's WS layer:
    // the SetActive claim snap and the detach-promotion restore. (The
    // typing input-claim path calls request_resize with the same
    // shape as SetActive; the integration test drives it over a real
    // socket.)

    #[cfg(unix)]
    #[test]
    fn setactive_claim_cannot_resize_a_pinned_session() {
        use k2_core::terminal::Dimensions;
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_test_session();
        DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("owner".into()));
        assert_dims_settle(&s, (100, 30), "pin");

        // A claim WITH dims still claims (arbitration state is
        // untouched by the pin) but its snap-resize is clamped.
        let outcome = apply_set_active(&s, 7, true, Some(140), Some(45));
        assert_eq!(outcome, SetActiveOutcome::Claim);
        assert_eq!(s.active_subscriber.load(Relaxed), 7, "claim still lands");
        // The viewer's declared viewport is still recorded (unpin
        // restores from it later)...
        assert_eq!(s.active_cols.load(Relaxed), 140);
        assert_eq!(s.active_rows.load(Relaxed), 45);
        // ...but the PTY must not move. Outwait the debounce window so
        // a deferred flush can't sneak the resize in either.
        std::thread::sleep(std::time::Duration::from_millis(
            k2_core::terminal::daemon_pty::RESIZE_DEBOUNCE_MS * 2 + 50,
        ));
        {
            let tm = s.term();
            let t = tm.lock();
            assert_eq!(
                (t.columns() as u16, t.screen_lines() as u16),
                (100, 30),
                "SetActive claim must not resize a pinned session"
            );
        }

        // Unpin: the active claimer's declared viewport is restored —
        // "clears back to the juggling".
        DaemonPtySession::set_pinned(&s, None, None);
        assert_dims_settle(&s, (140, 45), "unpin restores the claimer's dims");
    }

    #[cfg(unix)]
    #[test]
    fn detach_promotion_cannot_resize_a_pinned_session() {
        use k2_core::terminal::Dimensions;
        use std::sync::atomic::Ordering::Relaxed;

        let s = spawn_test_session();
        // Desktop (7) then phone (9) claim; both snaps precede the pin.
        apply_set_active(&s, 7, true, Some(120), Some(40));
        apply_set_active(&s, 9, true, Some(40), Some(10));
        assert_dims_settle(&s, (40, 10), "phone claim");

        DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("owner".into()));
        assert_dims_settle(&s, (100, 30), "pin");

        // Phone disconnects: the election still promotes the desktop
        // (arbitration is pin-agnostic) but the restore-resize to the
        // survivor's dims is clamped — the class keeps its 100x30.
        DaemonPtySession::detach_subscriber(&s, 9);
        assert_eq!(s.active_subscriber.load(Relaxed), 7, "survivor promoted");
        std::thread::sleep(std::time::Duration::from_millis(
            k2_core::terminal::daemon_pty::RESIZE_DEBOUNCE_MS * 2 + 50,
        ));
        {
            let tm = s.term();
            let t = tm.lock();
            assert_eq!(
                (t.columns() as u16, t.screen_lines() as u16),
                (100, 30),
                "detach-promotion must not resize a pinned session"
            );
        }
    }

    /// S7a — FROZEN WIRE SHAPES for the pin frames the renderer (S7b)
    /// will parse. `pin_changed` carries `{cols,rows,cleared:false}`
    /// when pinned and exactly `{"cleared":true}` when cleared;
    /// `pin_initial` carries the state + attribution at connect time.
    #[test]
    fn pin_frames_serialize_to_frozen_contract() {
        let pinned = Outbound::PinChanged {
            cols: Some(100),
            rows: Some(30),
            cleared: false,
        };
        assert_eq!(
            serde_json::to_string(&pinned).expect("serialize"),
            r#"{"event":"pin_changed","payload":{"cols":100,"rows":30,"cleared":false}}"#
        );
        let cleared = Outbound::PinChanged {
            cols: None,
            rows: None,
            cleared: true,
        };
        assert_eq!(
            serde_json::to_string(&cleared).expect("serialize"),
            r#"{"event":"pin_changed","payload":{"cleared":true}}"#
        );
        let initial = Outbound::PinInitial {
            cols: 100,
            rows: 30,
            set_by: Some("owner".to_string()),
        };
        assert_eq!(
            serde_json::to_string(&initial).expect("serialize"),
            r#"{"event":"pin_initial","payload":{"cols":100,"rows":30,"set_by":"owner"}}"#
        );
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

    // OSC 52 clipboard egress (copy direction only). Pins the wire
    // shape the client's `case 'clipboard'` handler parses, the
    // drop-whole cap semantics, and — end-to-end through a real PTY +
    // alacritty Term — that a child's OSC 52 STORE surfaces on the
    // session event broadcast with the payload base64-DECODED.

    #[test]
    fn clipboard_frame_serializes_to_the_clipboard_event() {
        let frame = clipboard_frame("hello-osc52".to_string())
            .expect("small payload must forward");
        let json = serde_json::to_string(&frame).expect("serialize");
        assert_eq!(
            json,
            r#"{"event":"clipboard","payload":{"text":"hello-osc52"}}"#
        );
    }

    #[test]
    fn clipboard_frame_drops_oversize_whole_and_keeps_cap_exact() {
        // Exactly at the cap forwards…
        let at_cap = "x".repeat(CLIPBOARD_MAX_BYTES);
        assert!(clipboard_frame(at_cap).is_some(), "cap is inclusive");
        // …one byte over drops the WHOLE payload (never truncates).
        let over = "x".repeat(CLIPBOARD_MAX_BYTES + 1);
        assert!(clipboard_frame(over).is_none(), "oversize must drop");
    }

    #[cfg(unix)]
    #[test]
    fn osc52_store_surfaces_decoded_on_the_event_broadcast() {
        let s = spawn_test_session(); // `cat` echoes stdin → PTY out
        let mut rx = s.subscribe_events();
        // Write the OSC 52 STORE as input; `cat` writes it back
        // through the PTY, alacritty parses it (default OnlyCopy
        // admits stores), base64-decodes the payload and broadcasts
        // ClipboardStore. aGVsbG8tb3NjNTI= == "hello-osc52". The
        // trailing \n matters twice: the PTY is in canonical mode, so
        // `cat` doesn't read the line until a newline arrives — and
        // the tty driver's own ECHO mangles ESC to caret notation
        // (`^[`), so only cat's verbatim output carries a real ESC
        // byte the parser can dispatch.
        s.write(b"\x1b]52;c;aGVsbG8tb3NjNTI=\x07\n".to_vec());
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match rx.try_recv() {
                Ok(AlacEvent::ClipboardStore(_, text)) => {
                    assert_eq!(text, "hello-osc52", "payload must arrive decoded");
                    return;
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    if std::time::Instant::now() >= deadline {
                        panic!("no ClipboardStore event within 5s");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => panic!("event broadcast died: {e}"),
            }
        }
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
