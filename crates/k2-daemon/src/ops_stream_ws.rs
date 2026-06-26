//! `GET /cli/ops/stream` — the Observability / Agent-Ops fan-in WebSocket.
//!
//! Phase C of `.k2/prds/prd-observability-agent-ops.md`. A **read-only**
//! multiplex that subscribes to BOTH existing in-memory live planes — the
//! `session_events` broadcast (`session_events.rs`) and the awareness bus
//! (`k2_core::awareness::bus`) — and re-broadcasts each event on ONE socket,
//! tagged by source, so the agent-ops pane subscribes once instead of
//! hand-stitching N sockets.
//!
//! **Adds no new event types and persists nothing.** Each event is re-emitted
//! verbatim inside a thin additive envelope:
//!
//! ```json
//! {"source":"session",   "event": <SessionEvent, tagged by `kind`>}
//! {"source":"awareness", "event": <AgentSignal>}
//! ```
//!
//! Unlike `/cli/sessions/events`, this stream is **not** `?path=`-filtered: the
//! ops pane is the whole-fleet-on-this-daemon view, so every session event AND
//! every awareness signal is forwarded. (The pane links to `/cli/sessions/grid`
//! per-session for screen replay — grid bytes are deliberately NOT folded in.)
//!
//! Structure mirrors `session_events_ws.rs` / `awareness_ws.rs`:
//!   1. The dispatcher (`routes/dispatcher.rs`) token-auths the upgrade and
//!      passes the connecting token + owner token for the 5s re-auth heartbeat.
//!   2. Handshake via `tokio_tungstenite::accept_async`.
//!   3. Subscribe to BOTH broadcast buses; emit a `hello` frame immediately.
//!   4. `tokio::select!` over both receivers + the 5s re-auth tick +
//!      client-close. `broadcast::error::Lagged` is logged + skipped (never
//!      panics), matching the two source handlers.

use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use k2_core::awareness::{self, AgentSignal};
use k2_core::log_debug;

use crate::session_events::{self, SessionEvent};

/// Monotonically-increasing subscriber id generator — each `hello` frame
/// carries a unique id the pane can log for debug + dedupe across reconnects.
/// Mirrors `session_events_ws::NEXT_SUBSCRIBER_ID`.
static NEXT_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(1);

/// Outbound `hello` frame sent immediately after the WS handshake. Distinct
/// shape from the envelope so the pane special-cases it (subscription is live;
/// re-reconcile after a drop+reconnect).
#[derive(Debug, Serialize)]
struct HelloEvent {
    kind: &'static str,
    subscriber_id: u64,
}

/// WS handler entry point. Called by `routes::dispatcher` after token auth.
/// `token` is the credential the upgrade was authorized with; it is
/// re-validated every 5s so a revoked connect-user is dropped from this live
/// stream within seconds (same heartbeat as `session_events_ws` /
/// `awareness_ws` / the grid WS — the 0.40.9 long-lived-WS re-auth invariant).
pub async fn serve_ops_stream_connection(
    stream: &mut TcpStream,
    token: String,
    owner_token: String,
) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/ops_stream_ws] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();

    let subscriber_id = NEXT_SUBSCRIBER_ID.fetch_add(1, Ordering::Relaxed);

    // Fan-in: subscribe to BOTH existing in-memory planes. No new buses.
    let mut session_rx = session_events::subscribe();
    let mut awareness_rx = awareness::subscribe();

    log_debug!(
        "[daemon/ops_stream_ws] subscriber {} attached (multiplex: session_events + awareness)",
        subscriber_id,
    );

    // Hello first, so the pane marks itself subscribed before any event.
    let hello = HelloEvent {
        kind: "hello",
        subscriber_id,
    };
    if send_json(&mut write, &hello).await.is_err() {
        return;
    }

    // Re-auth heartbeat — every 5s confirm the connecting token is still
    // valid; a revoked connect-user is kicked off this live socket. The owner
    // token never revokes, so owner connections sail through. Mirrors
    // `session_events_ws.rs:116-131`.
    let mut auth_recheck = tokio::time::interval(std::time::Duration::from_secs(5));
    auth_recheck.tick().await; // burn the immediate first tick

    loop {
        tokio::select! {
            _ = auth_recheck.tick() => {
                if !crate::routes::http::token_still_valid(&token, &owner_token) {
                    log_debug!(
                        "[daemon/ops_stream_ws] token revoked mid-session; \
                         closing ops stream for subscriber {}",
                        subscriber_id,
                    );
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
            }
            recv = session_rx.recv() => {
                match recv {
                    Ok(event) => {
                        if send_session_event(&mut write, &event).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Fell behind — skip the dropped events; the pane's
                        // reconcile-on-reconnect path is the recovery story.
                        // Never panic on lag (mirrors the source handlers).
                        log_debug!(
                            "[daemon/ops_stream_ws] subscriber {} lagged {n} session events",
                            subscriber_id,
                        );
                        continue;
                    }
                    Err(RecvError::Closed) => {
                        // Bus closed — only on daemon shutdown. Exit cleanly.
                        log_debug!(
                            "[daemon/ops_stream_ws] session bus closed for subscriber {}",
                            subscriber_id,
                        );
                        break;
                    }
                }
            }
            recv = awareness_rx.recv() => {
                match recv {
                    Ok(signal) => {
                        if send_awareness_signal(&mut write, &signal).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        log_debug!(
                            "[daemon/ops_stream_ws] subscriber {} lagged {n} awareness signals",
                            subscriber_id,
                        );
                        continue;
                    }
                    Err(RecvError::Closed) => {
                        log_debug!(
                            "[daemon/ops_stream_ws] awareness bus closed for subscriber {}",
                            subscriber_id,
                        );
                        break;
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Ping(p))) => {
                        if write.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        let _ = write.send(Message::Close(frame)).await;
                        break;
                    }
                    // Receive-only protocol — ignore inbound Text/Binary.
                    Some(Ok(_)) => {}
                    None => break,
                    Some(Err(e)) => {
                        log_debug!("[daemon/ops_stream_ws] read error: {e}");
                        break;
                    }
                }
            }
        }
    }

    log_debug!(
        "[daemon/ops_stream_ws] subscriber {} detached",
        subscriber_id,
    );
}

/// Wrap a verbatim `SessionEvent` in the additive `{source,event}` envelope
/// and send it. Returns `Err(())` if the socket is gone.
async fn send_session_event<W>(write: &mut W, event: &SessionEvent) -> Result<(), ()>
where
    W: futures_util::SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let env = serde_json::json!({ "source": "session", "event": event });
    send_json(write, &env).await
}

/// Wrap a verbatim `AgentSignal` in the additive `{source,event}` envelope and
/// send it. Returns `Err(())` if the socket is gone.
async fn send_awareness_signal<W>(write: &mut W, signal: &AgentSignal) -> Result<(), ()>
where
    W: futures_util::SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let env = serde_json::json!({ "source": "awareness", "event": signal });
    send_json(write, &env).await
}

async fn send_json<W, T>(write: &mut W, msg: &T) -> Result<(), ()>
where
    W: futures_util::SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    T: Serialize,
{
    let text = match serde_json::to_string(msg) {
        Ok(s) => s,
        Err(e) => {
            log_debug!("[daemon/ops_stream_ws] serialize failed: {e}");
            return Err(());
        }
    };
    write.send(Message::Text(text.into())).await.map_err(|e| {
        log_debug!("[daemon/ops_stream_ws] send failed: {e}");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::awareness::{AgentAddress, SignalKind};

    /// The session envelope tags `source:"session"` and nests the verbatim
    /// `SessionEvent` (still discriminated by its own `kind`) under `event`.
    #[test]
    fn session_envelope_shape() {
        let event = SessionEvent::SessionRemoved {
            workspace_path: "/x/foo".into(),
            pane_group_id: Some("pg".into()),
            agent_name: "tab-pg".into(),
        };
        let env = serde_json::json!({ "source": "session", "event": &event });
        assert_eq!(env.get("source").and_then(|v| v.as_str()), Some("session"));
        assert_eq!(
            env.pointer("/event/kind").and_then(|v| v.as_str()),
            Some("session_removed"),
            "verbatim SessionEvent must keep its own `kind` discriminant",
        );
        assert_eq!(
            env.pointer("/event/workspace_path").and_then(|v| v.as_str()),
            Some("/x/foo"),
        );
    }

    /// The awareness envelope tags `source:"awareness"` and nests the verbatim
    /// `AgentSignal` under `event`.
    #[test]
    fn awareness_envelope_shape() {
        let signal = AgentSignal::new(
            AgentAddress::Broadcast,
            AgentAddress::Broadcast,
            SignalKind::Status { text: "t".into() },
        );
        let env = serde_json::json!({ "source": "awareness", "event": &signal });
        assert_eq!(
            env.get("source").and_then(|v| v.as_str()),
            Some("awareness"),
        );
        assert_eq!(
            env.pointer("/event/id").and_then(|v| v.as_str()),
            Some(signal.id.to_string().as_str()),
            "verbatim AgentSignal must be nested under `event`",
        );
    }
}
