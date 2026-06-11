//! `/cli/sessions/subscribe` WebSocket endpoint (0.34.0 Phase 2).
//!
//! Mirrors the design of `events.rs` but for per-session `Frame`
//! streams. On upgrade:
//!
//!   1. Parse `?session=<UUID>` + auth token from the query string.
//!   2. Look up the session in `k2_core::session::registry`. If
//!      it's not registered, send a `session:error` event and close.
//!   3. Subscribe to the session's broadcast channel FIRST, then
//!      snapshot the replay ring. This ordering biases toward
//!      possible duplicate delivery (frames landing between the two
//!      calls may appear in both the snapshot and live stream) over
//!      dropped delivery. Phase 3 adds seqno-based dedupe.
//!   4. Send a `session:ack` event with the replay count so the
//!      client knows how many catch-up frames to expect.
//!   5. Flush the replay snapshot as `session:frame` events.
//!   6. Enter a `tokio::select!` loop forwarding every live
//!      broadcast frame as `session:frame`. Client-side close, read
//!      error, or broadcast closure exits the loop.
//!
//! Wire-format events (JSON text frames):
//!
//!   {"event":"session:ack",   "payload":{"sessionId":"...", "replayCount":N}}
//!   {"event":"session:frame", "payload":<Frame>}
//!   {"event":"session:error", "payload":{"message":"..."}}
//!
//! `Frame` is serialized via its existing `#[serde(tag="frame",
//! content="data")]` form, so the subscriber sees well-typed
//! discriminated JSON without a second mapping table.

use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::session::{registry, Frame, SessionId};

/// Entry point for the `/cli/sessions/subscribe` branch in main.rs.
/// Validates the token, parses the session id, hands off to the WS
/// loop. Returns cleanly on any protocol / lookup failure so the
/// daemon's connection handler can drop the socket without
/// additional handling.
pub async fn serve_session_subscribe_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
) {
    // 0.39.7: stream borrowed (was owned). See events.rs.
    let session_id = match params.get("session").and_then(|s| SessionId::parse(s)) {
        Some(id) => id,
        None => {
            send_error_then_close(stream, "missing or malformed 'session' query param")
                .await;
            return;
        }
    };

    // Look up registry entry BEFORE the WS upgrade so an unknown
    // session gets a clear error message before any data frames flow.
    let entry = match registry::lookup(&session_id) {
        Some(e) => e,
        None => {
            let msg = format!("session {session_id} not found in registry");
            send_error_then_close(stream, &msg).await;
            return;
        }
    };

    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/sessions_ws] ws handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();

    // Subscribe BEFORE snapshot so the race window biases toward
    // possible duplicate delivery (at-least-once) rather than
    // dropped delivery.
    let mut rx = entry.subscribe();
    let replay = entry.replay_snapshot();

    // Instrumentation: tally what's in the ring so the daemon log
    // shows exactly what this subscriber is about to receive. If
    // the scrollback problem ever reappears, this line is the
    // answer to "did the ring have the data?" — distinct from
    // "did the client render it?"
    let mut text_frames = 0usize;
    let mut text_bytes = 0usize;
    let mut mode_frames = 0usize;
    let mut boundary_seen = false;
    for frame in &replay {
        match frame {
            Frame::Text { bytes, .. } => {
                text_frames += 1;
                text_bytes += bytes.len();
            }
            Frame::ModeChange { .. } => mode_frames += 1,
            Frame::SemanticEvent {
                kind: k2_core::session::SemanticKind::Custom { kind, .. },
                ..
            } if kind == "grow_boundary" => {
                boundary_seen = true;
            }
            _ => {}
        }
    }
    log_debug!(
        "[daemon/sessions_ws] subscriber for {session_id} will drain \
         replay: frames={} text={} text_bytes~={} mode={} \
         grow_boundary={} (live subscribers on entry: {})",
        replay.len(),
        text_frames,
        text_bytes,
        mode_frames,
        boundary_seen,
        entry.subscriber_count()
    );

    // Send ack.
    let ack = serialize_event(
        "session:ack",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "replayCount": replay.len(),
        }),
    );
    if let Err(e) = write.send(Message::Text(ack)).await {
        log_debug!("[daemon/sessions_ws] ack write failed: {e}");
        return;
    }

    // Flush replay.
    let mut replay_sent = 0usize;
    for frame in replay {
        let msg = match serialize_frame_event(&frame) {
            Ok(m) => m,
            Err(e) => {
                log_debug!("[daemon/sessions_ws] serialize replay frame: {e}");
                continue;
            }
        };
        if let Err(e) = write.send(Message::Text(msg)).await {
            log_debug!(
                "[daemon/sessions_ws] replay write failed after {replay_sent} \
                 frames (client gone): {e}"
            );
            return;
        }
        replay_sent += 1;
    }

    log_debug!(
        "[daemon/sessions_ws] subscriber connected for {session_id}, \
         replay_sent={} (live subscribers on entry: {})",
        replay_sent,
        entry.subscriber_count()
    );

    // Live forwarding loop.
    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(frame) => {
                        let msg = match serialize_frame_event(&frame) {
                            Ok(m) => m,
                            Err(e) => {
                                log_debug!(
                                    "[daemon/sessions_ws] serialize live frame: {e}"
                                );
                                continue;
                            }
                        };
                        if let Err(e) = write.send(Message::Text(msg)).await {
                            log_debug!(
                                "[daemon/sessions_ws] live write failed: {e}"
                            );
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log_debug!(
                            "[daemon/sessions_ws] subscriber lagged {n} frames"
                        );
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        log_debug!(
                            "[daemon/sessions_ws] broadcast closed, disconnecting \
                             subscriber for {session_id}"
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
                            log_debug!("[daemon/sessions_ws] pong failed: {e}");
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // Ignore inbound data — subscribers are
                        // read-only from the daemon's perspective.
                    }
                    Some(Err(e)) => {
                        log_debug!("[daemon/sessions_ws] read error: {e}");
                        break;
                    }
                }
            }
        }
    }
}

/// Attempt to send a pre-upgrade HTTP error response. The caller
/// used `serve_session_subscribe_connection` before the WS upgrade
/// happened, so the socket is still raw HTTP.
async fn send_error_then_close(stream: &mut TcpStream, message: &str) {
    use tokio::io::AsyncWriteExt;
    let body = format!("{{\"error\":{}}}", serde_json::json!(message));
    let resp = format!(
        "HTTP/1.1 400 Bad Request\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n\
         {}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

fn serialize_event(event: &str, payload: serde_json::Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "event": event,
        "payload": payload,
    }))
    .unwrap_or_else(|_| String::from("{\"event\":\"error\",\"payload\":{}}"))
}

fn serialize_frame_event(frame: &Frame) -> Result<String, serde_json::Error> {
    serde_json::to_string(&serde_json::json!({
        "event": "session:frame",
        "payload": frame,
    }))
}

// ──────────────────────────────────────────────────────────────────────
// Unit tests (serialization shape only — the full WS round-trip is
// exercised by the integration tests in D6).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::session::{Frame, SemanticKind};

    #[test]
    fn text_frame_event_has_expected_shape() {
        let frame = Frame::Text {
            bytes: b"hi".to_vec(),
            style: None,
        };
        let json = serialize_frame_event(&frame).unwrap();
        assert!(json.contains(r#""event":"session:frame""#), "{json}");
        assert!(json.contains(r#""frame":"Text""#), "{json}");
    }

    #[test]
    fn semantic_event_frame_carries_kind() {
        let frame = Frame::SemanticEvent {
            kind: SemanticKind::ToolCall,
            payload: serde_json::json!({"name":"bash"}),
        };
        let json = serialize_frame_event(&frame).unwrap();
        assert!(json.contains(r#""event":"session:frame""#), "{json}");
        // SemanticKind is tagged with `"type"`, not `"kind"`, so the
        // full event is {"event":"session:frame","payload":{"frame":
        // "SemanticEvent","data":{"kind":{"type":"ToolCall"}, ...}}}.
        // Subscribers filter by `event` then destructure `payload`
        // into a Frame; no second mapping table.
        assert!(json.contains(r#""frame":"SemanticEvent""#), "{json}");
        assert!(json.contains(r#""type":"ToolCall""#), "{json}");
    }

    #[test]
    fn ack_payload_round_trips() {
        let ack = serialize_event(
            "session:ack",
            serde_json::json!({
                "sessionId": "00000000-0000-0000-0000-000000000000",
                "replayCount": 5,
            }),
        );
        assert!(ack.contains(r#""event":"session:ack""#));
        assert!(ack.contains(r#""replayCount":5"#));
    }
}
