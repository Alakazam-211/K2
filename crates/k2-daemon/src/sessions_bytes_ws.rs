//! `/cli/sessions/bytes` WebSocket endpoint (Canvas Plan Phase 2).
//!
//! Parallel to `sessions_ws.rs` but streams the Session's raw byte
//! stream instead of parsed Frames. Clients that maintain their
//! own terminal emulator (e.g. Kessel post-Phase-4, running an
//! `alacritty_terminal::Term` inside Tauri) subscribe here to get
//! pixel-perfect bytes that they drive through their local vte at
//! their own width.
//!
//! On upgrade:
//!
//!   1. Parse `?session=<UUID>&from=<offset>` from query string.
//!   2. Look up the `SessionEntry` in the registry. Unknown session
//!      → HTTP 400 error body before upgrade.
//!   3. WebSocket handshake.
//!   4. Subscribe to the Session's byte broadcast first (order
//!      biases toward duplicate delivery over dropped delivery).
//!   5. Send a `session:ack` text envelope:
//!
//!      ```text
//!      {"event":"session:ack","payload":{
//!         "sessionId":"…",
//!         "fromOffset": <the offset the caller asked for>,
//!         "currentFrontOffset": <earliest byte still in ring>,
//!         "currentBackOffset": <next byte to be written>
//!      }}
//!      ```
//!
//!      Clients use this to tell whether the ring covered their
//!      request or whether they need to read the on-disk archive
//!      for bytes [fromOffset, currentFrontOffset). This crate
//!      currently only serves from the ring + live tail (archive
//!      replay is a future enhancement); the ack exposes the gap
//!      so clients can decide how to handle it.
//!   6. Flush the ring snapshot as one or more **binary** WebSocket
//!      messages. Each chunk is sent as a single binary frame.
//!   7. Enter a `tokio::select!` loop forwarding every live
//!      broadcast byte chunk as a binary message until the client
//!      closes, the broadcast closes, or an IO error occurs.
//!
//! **Why binary frames for bytes, text for ack.** The byte stream
//! is strictly 8-bit data (ANSI escapes, UTF-8 text, control
//! chars) — base64 or JSON-escape would inflate it ~33%. The ack
//! is JSON for uniformity with `sessions_ws.rs`.

use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::session::{registry, SessionId};

/// Entry point for the `/cli/sessions/bytes` branch in main.rs.
/// Validates query params, hands off to the WS loop. Returns
/// cleanly on any protocol / lookup failure so the caller's
/// connection handler can drop the socket.
pub async fn serve_session_bytes_connection(
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
    // Default to offset 0 (full history) if the client doesn't pass
    // `from`. Invalid (non-numeric) `from` is a hard error —
    // catching it up front avoids ambiguity later.
    let from_offset: u64 = match params.get("from") {
        None => 0,
        Some(s) => match s.parse::<u64>() {
            Ok(v) => v,
            Err(_) => {
                send_error_then_close(
                    stream,
                    "'from' query param must be a non-negative integer",
                )
                .await;
                return;
            }
        },
    };

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
            log_debug!("[daemon/sessions_bytes_ws] ws handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();

    // Subscribe BEFORE snapshot so the race biases toward duplicate
    // delivery rather than dropped (the vte is idempotent on byte
    // replay — a duplicate chunk is a no-op; a missing chunk
    // corrupts state).
    let mut rx = entry.bytes_subscribe();
    let snapshot = entry.bytes_snapshot_from(from_offset);
    let (front, back) = entry.bytes_offsets();

    // Build ack envelope. Clients parse it to decide whether the
    // ring covered their request; if `fromOffset < currentFrontOffset`
    // they need to backfill from the on-disk archive for the gap.
    let ack = serialize_event(
        "session:ack",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "fromOffset": from_offset,
            "currentFrontOffset": front,
            "currentBackOffset": back,
        }),
    );
    if let Err(e) = write.send(Message::Text(ack)).await {
        log_debug!("[daemon/sessions_bytes_ws] ack write failed: {e}");
        return;
    }

    // Compute ring replay stats for logging + for clients reading
    // the daemon log. Also: each chunk in the snapshot may contain
    // bytes before `from_offset` if the first chunk straddles it —
    // skip the prefix on the first chunk so the client receives
    // exactly the byte range it asked for.
    let mut replay_bytes = 0usize;
    let mut replay_chunks = 0usize;
    for chunk in snapshot {
        let data = if chunk.start_offset < from_offset {
            let skip = (from_offset - chunk.start_offset) as usize;
            if skip >= chunk.data.len() {
                continue;
            }
            chunk.data[skip..].to_vec()
        } else {
            chunk.data.to_vec()
        };
        if data.is_empty() {
            continue;
        }
        replay_bytes += data.len();
        replay_chunks += 1;
        if let Err(e) = write.send(Message::Binary(data)).await {
            log_debug!(
                "[daemon/sessions_bytes_ws] replay write failed (client gone) \
                 after {replay_chunks} chunks / {replay_bytes} bytes: {e}"
            );
            return;
        }
    }

    log_debug!(
        "[daemon/sessions_bytes_ws] subscriber connected for {session_id} \
         from={} ring=[{},{}) replay={}b/{}c (live subscribers: {})",
        from_offset,
        front,
        back,
        replay_bytes,
        replay_chunks,
        entry.bytes_subscriber_count()
    );

    // Live forwarding loop.
    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(chunk) => {
                        if chunk.is_empty() {
                            continue;
                        }
                        if let Err(e) = write
                            .send(Message::Binary(chunk.to_vec()))
                            .await
                        {
                            log_debug!(
                                "[daemon/sessions_bytes_ws] live write failed: {e}"
                            );
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log_debug!(
                            "[daemon/sessions_bytes_ws] subscriber lagged {n} chunks"
                        );
                        // Can't easily recover inline — the client
                        // now has a byte gap and its vte state is
                        // uncertain. Close cleanly so the client can
                        // reconnect and replay from a newer offset.
                        let _ = write.send(Message::Close(None)).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        log_debug!(
                            "[daemon/sessions_bytes_ws] broadcast closed, \
                             disconnecting subscriber for {session_id}"
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
                            log_debug!("[daemon/sessions_bytes_ws] pong failed: {e}");
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // Byte subscribers are read-only — ignore
                        // any client-sent data.
                    }
                    Some(Err(e)) => {
                        log_debug!("[daemon/sessions_bytes_ws] read error: {e}");
                        break;
                    }
                }
            }
        }
    }
}

/// Pre-upgrade HTTP error response. The socket is still raw HTTP
/// when we send this — the caller invoked us before the WS
/// handshake, so a 400 with a JSON error body is appropriate.
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
