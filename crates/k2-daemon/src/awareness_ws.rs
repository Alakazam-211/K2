//! `/cli/awareness/publish` + `/cli/awareness/subscribe` — daemon-
//! side access to the Awareness Bus.
//!
//! E7 of Phase 3. Two routes:
//!
//!   - `POST /cli/awareness/publish` — JSON body is an
//!     `AgentSignal`. Handler passes it to `awareness::ingress::from_cli`
//!     which delegates to egress. Returns the `DeliveryReport` as JSON.
//!
//!   - `GET /cli/awareness/subscribe` — WS upgrade. Stream every
//!     `bus::publish()`-ed signal to connected clients. Mirror of
//!     the D4 `sessions_ws::serve_session_subscribe_connection`
//!     pattern.
//!
//! Wire format matches `sessions_ws` — each frame is a JSON text
//! message `{"event":"awareness:signal","payload":<AgentSignal>}`.
//! Consumers filter by `event` and destructure `payload`.

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use k2_core::awareness::{self, AgentSignal};
use k2_core::log_debug;

// ──────────────────────────────────────────────────────────────────────
// POST /cli/awareness/publish
// ──────────────────────────────────────────────────────────────────────

/// Handler for `POST /cli/awareness/publish`. Called by
/// `main.rs::handle_connection` after token auth + body read.
///
/// Returns a JSON body describing the response. Errors come back
/// as `{"error":"..."}` with status 400; success returns the
/// `DeliveryReport` fields.
pub fn handle_publish(body: &[u8]) -> HandlerResult {
    let signal: AgentSignal = match serde_json::from_slice(body) {
        Ok(s) => s,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse AgentSignal: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };

    let report = awareness::from_cli(signal);

    let json_body = serde_json::json!({
        "injected_to_pty": report.injected_to_pty,
        "woke_offline_target": report.woke_offline_target,
        "inbox_path": report.inbox_path.map(|p| p.to_string_lossy().into_owned()),
        "activity_feed_row_id": report.activity_feed_row_id,
        "published_to_bus": report.published_to_bus,
    });

    HandlerResult {
        status: "200 OK",
        body: serde_json::to_string(&json_body).unwrap_or_else(|_| "{}".into()),
    }
}

pub struct HandlerResult {
    pub status: &'static str,
    pub body: String,
}

// ──────────────────────────────────────────────────────────────────────
// GET /cli/awareness/subscribe (WebSocket)
// ──────────────────────────────────────────────────────────────────────

/// WS handler. Upgrade the TCP stream, subscribe to the in-core
/// bus, and fan every signal out to the connected client as a JSON
/// text message. Exits when the bus closes (daemon shutdown) or
/// the client disconnects.
pub async fn serve_awareness_subscribe_connection(stream: &mut TcpStream) {
    // 0.39.7: stream borrowed (was owned). See `events.rs` for
    // rationale — dispatcher keeps stream ownership across its
    // keep-alive loop iterations.
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/awareness_ws] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = awareness::subscribe();
    log_debug!(
        "[daemon/awareness_ws] subscriber connected (bus subscribers: {})",
        awareness::subscriber_count()
    );

    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(signal) => {
                        let msg = match serialize_signal_event(&signal) {
                            Ok(m) => m,
                            Err(e) => {
                                log_debug!(
                                    "[daemon/awareness_ws] serialize failed: {e}"
                                );
                                continue;
                            }
                        };
                        if let Err(e) = write.send(Message::Text(msg)).await {
                            log_debug!(
                                "[daemon/awareness_ws] send failed (client gone): {e}"
                            );
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log_debug!(
                            "[daemon/awareness_ws] subscriber lagged {n} signals"
                        );
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        log_debug!(
                            "[daemon/awareness_ws] bus closed, disconnecting subscriber"
                        );
                        break;
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        if let Err(e) = write.send(Message::Pong(p)).await {
                            log_debug!("[daemon/awareness_ws] pong failed: {e}");
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        log_debug!("[daemon/awareness_ws] read error: {e}");
                        break;
                    }
                }
            }
        }
    }
}

fn serialize_signal_event(signal: &AgentSignal) -> Result<String, serde_json::Error> {
    serde_json::to_string(&serde_json::json!({
        "event": "awareness:signal",
        "payload": signal,
    }))
}

// ──────────────────────────────────────────────────────────────────────
// Unit tests — serialization shape
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::awareness::{AgentAddress, Delivery, SignalKind, WorkspaceId};

    fn minimal_signal() -> AgentSignal {
        AgentSignal::new(
            AgentAddress::Broadcast,
            AgentAddress::Broadcast,
            SignalKind::Status {
                text: "t".into(),
            },
        )
    }

    #[test]
    fn awareness_signal_event_shape() {
        let s = minimal_signal();
        let msg = serialize_signal_event(&s).unwrap();
        assert!(msg.contains(r#""event":"awareness:signal""#), "{msg}");
        assert!(msg.contains(r#""delivery":"live""#), "{msg}");
    }

    #[test]
    fn handle_publish_returns_delivery_report_fields() {
        // Test-util DB makes activity_feed insert safe under
        // k2-daemon unit-test context (dev-dep enables
        // `test-util` on k2so-core).
        k2_core::db::init_for_tests();
        let signal = minimal_signal();
        let body = serde_json::to_vec(&signal).unwrap();
        let result = handle_publish(&body);
        assert_eq!(result.status, "200 OK");
        assert!(
            result.body.contains("published_to_bus"),
            "body missing field: {}",
            result.body
        );
    }

    #[test]
    fn handle_publish_bad_json_returns_400() {
        let result = handle_publish(b"{this is not json");
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("parse AgentSignal"));
    }
}
