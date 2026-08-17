use super::auth;
use super::proxy::{dispatch_ws_method, parse_query};
use super::types::{CompanionState, WsClient};
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Write;
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Instant;
use tungstenite::{accept, Message};

/// HTTP upgrade already peeked, unread — reject without handing the
/// stream to tungstenite (otherwise `/companion/sessions/grid` becomes
/// JSON-RPC via [`handle_ws_upgrade`]).
fn write_http_json(
    stream: &mut TcpStream,
    status: u16,
    status_text: &str,
    body: &serde_json::Value,
) {
    let body_str = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         X-Frame-Options: DENY\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\
         \r\n{body_str}",
        body_str.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// Path-branch target for `GET /companion/sessions/grid`.
///
/// Auth is companion-session-token only. On success the unread stream is
/// handed to the daemon-registered adapter, which runs
/// `serve_session_grid_connection` as a viewer-default identity.
pub fn handle_grid_upgrade(
    mut stream: TcpStream,
    path: &str,
    headers: &HashMap<String, String>,
    state: &CompanionState,
) {
    let query = parse_query(path);
    let token = match auth::extract_grid_token(
        query.get("token").map(String::as_str),
        headers.get("authorization").map(String::as_str),
    ) {
        Some(t) => t,
        None => {
            write_http_json(
                &mut stream,
                401,
                "Unauthorized",
                &serde_json::json!({"ok":false,"error":"Missing companion session token"}),
            );
            return;
        }
    };

    if let Err(msg) = auth::authorize_grid_token(&token, state) {
        let (status, text) = if msg.contains("Rate limit") {
            (429, "Too Many Requests")
        } else {
            (403, "Forbidden")
        };
        write_http_json(
            &mut stream,
            status,
            text,
            &serde_json::json!({"ok":false,"error":"Invalid session token"}),
        );
        return;
    }

    let session_id = query.get("session").cloned().filter(|s| !s.is_empty());
    let Some(session_id) = session_id else {
        write_http_json(
            &mut stream,
            400,
            "Bad Request",
            &serde_json::json!({"ok":false,"error":"missing or malformed 'session' query param"}),
        );
        return;
    };

    let proto = query.get("proto").cloned();

    let Some(handler) = super::grid_upgrade_handler() else {
        write_http_json(
            &mut stream,
            404,
            "Not Found",
            &serde_json::json!({"ok":false,"error":"grid upgrade not registered"}),
        );
        return;
    };

    state.note_grid_ws_open(&token, &session_id);
    handler(super::CompanionGridUpgrade {
        stream,
        session_id,
        proto,
        companion_token: token,
    });
}

/// Handle a WebSocket upgrade request.
/// Accepts the connection, then runs the WS protocol:
///   1. First message must be auth (validates Bearer token)
///   2. Subsequent messages are method calls or terminal subscriptions
///   3. Server pushes events (terminal:grid, agent:lifecycle, heartbeat)
pub fn handle_ws_upgrade(stream: TcpStream, path: &str, state: &CompanionState) {
    // Accept token from query params for backwards compatibility,
    // but the client should also send auth as first WS message.
    let query = parse_query(path);
    let initial_token = query.get("token").cloned();

    // Upgrade to WebSocket.
    // The stream must NOT have been read yet — tungstenite::accept reads the
    // HTTP upgrade request itself and sends the 101 Switching Protocols response.
    let ws = match accept(stream) {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[companion-ws] WebSocket upgrade failed: {}", e);
            return;
        }
    };

    log_debug!("[companion-ws] Client connected");

    // Create channel for the writer thread
    let (tx, rx) = mpsc::channel::<String>();

    // Pre-authenticate if token was in query params (backwards compat with current mobile app)
    let pre_authenticated = if let Some(ref token) = initial_token {
        auth::validate_bearer(token, state).is_ok()
    } else {
        false
    };

    let client_token = initial_token.unwrap_or_default();
    let client_id = uuid::Uuid::new_v4().to_string();

    // Register client
    {
        let mut clients = state.ws_clients.lock();
        clients.push(WsClient {
            client_id: client_id.clone(),
            session_token: client_token.clone(),
            authenticated: pre_authenticated,
            subscribed_terminals: HashSet::new(),
            mobile_dims: None,
            sender: tx.clone(),
            last_seen: Instant::now(),
        });
    }

    // Split WebSocket into read and write halves via the underlying TcpStream.
    // tungstenite wraps a Read+Write stream — we can't split it directly.
    // Instead, we run BOTH read and write on the SAME thread, using non-blocking
    // channel receives between blocking reads.
    let reader_state = unsafe { &*(state as *const CompanionState) };
    let reader_token = client_token.clone();

    std::thread::spawn(move || {
        let mut ws = ws;
        let mut authenticated = pre_authenticated;
        let mut session_token = reader_token;
        let mut last_heartbeat = Instant::now();

        // Set a read timeout so we can interleave writes between reads
        let _ = ws
            .get_ref()
            .set_read_timeout(Some(std::time::Duration::from_millis(50)));

        loop {
            // Phase 1: Try to send any pending outbound messages (non-blocking)
            while let Ok(msg) = rx.try_recv() {
                if ws.send(Message::Text(msg)).is_err() {
                    return; // connection dead
                }
            }

            // Send heartbeat every 30s
            if last_heartbeat.elapsed() >= std::time::Duration::from_secs(30) {
                let hb = serde_json::json!({"event": "heartbeat"}).to_string();
                if ws.send(Message::Text(hb)).is_err() {
                    return;
                }
                last_heartbeat = Instant::now();
            }

            // Phase 2: Try to read one incoming message (with 50ms timeout)
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };

                    // Update last_seen
                    {
                        let mut clients = reader_state.ws_clients.lock();
                        if let Some(client) = clients
                            .iter_mut()
                            .find(|c| c.session_token == session_token)
                        {
                            client.last_seen = Instant::now();
                        }
                    }

                    let id = msg
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
                    let params = msg.get("params").cloned().unwrap_or(serde_json::json!({}));

                    // Handle auth method (must be first message if not pre-authenticated)
                    if method == "auth" {
                        let token = params.get("token").and_then(|v| v.as_str()).unwrap_or("");
                        match auth::validate_bearer(token, reader_state) {
                            Ok(_) => {
                                authenticated = true;
                                session_token = token.to_string();
                                // Update client's token and auth status
                                let mut clients = reader_state.ws_clients.lock();
                                if let Some(client) =
                                    clients.iter_mut().find(|c| c.client_id == client_id)
                                {
                                    client.session_token = session_token.clone();
                                    client.authenticated = true;
                                }
                                drop(clients);
                                send_response(
                                    &tx,
                                    id.as_deref(),
                                    Ok(serde_json::json!({"authenticated": true})),
                                );
                            }
                            Err(e) => {
                                send_response(&tx, id.as_deref(), Err(format!("{}", e)));
                            }
                        }
                        continue;
                    }

                    // All other methods require authentication
                    if !authenticated {
                        send_response(
                            &tx,
                            id.as_deref(),
                            Err("Not authenticated. Send auth method first.".to_string()),
                        );
                        continue;
                    }

                    // Handle ping (keepalive)
                    if method == "ping" {
                        send_response(&tx, id.as_deref(), Ok(serde_json::json!({"pong": true})));
                        continue;
                    }

                    // Logout: purge the current session and close the socket.
                    if method == "auth.revoke" {
                        use subtle::ConstantTimeEq;
                        {
                            let mut sessions = reader_state.sessions.lock();
                            let mut matched: Option<String> = None;
                            for key in sessions.keys() {
                                if key.as_bytes().ct_eq(session_token.as_bytes()).into() {
                                    matched = Some(key.clone());
                                    break;
                                }
                            }
                            if let Some(k) = matched {
                                sessions.remove(&k);
                            }
                        }
                        send_response(&tx, id.as_deref(), Ok(serde_json::json!({"revoked": true})));
                        // Tear this client down — next loop iteration will see
                        // the send-channel drop and exit cleanly.
                        let _ = ws.send(Message::Close(None));
                        break;
                    }

                    // Handle terminal subscribe/unsubscribe/resize
                    if method == "terminal.subscribe" {
                        let terminal_id = params
                            .get("terminalId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if terminal_id.is_empty() {
                            send_response(
                                &tx,
                                id.as_deref(),
                                Err("Missing terminalId".to_string()),
                            );
                        } else {
                            // Extract optional mobile dimensions for shadow terminal reflow.
                            // Subtract 1 column as safety margin — mobile font metrics
                            // (sub-pixel rounding, webview rendering) can differ slightly
                            // from integer column math, causing the prompt line to wrap.
                            let cols = params
                                .get("cols")
                                .and_then(|v| v.as_u64())
                                .map(|v| v.saturating_sub(1).max(10) as u16);
                            let rows = params
                                .get("rows")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u16);
                            let dims = match (cols, rows) {
                                (Some(c), Some(r)) if c > 0 && r > 0 => Some((c, r)),
                                _ => None,
                            };

                            let mut clients = reader_state.ws_clients.lock();
                            if let Some(client) =
                                clients.iter_mut().find(|c| c.client_id == client_id)
                            {
                                client.subscribed_terminals.insert(terminal_id.to_string());
                                if dims.is_some() {
                                    client.mobile_dims = dims;
                                }
                            }
                            drop(clients);
                            log_debug!(
                                "[companion-ws] Subscribed to terminal: {} (dims: {:?})",
                                terminal_id,
                                dims
                            );
                            send_response(
                                &tx,
                                id.as_deref(),
                                Ok(serde_json::json!({
                                    "subscribed": terminal_id,
                                    "mobileDims": dims.map(|(c, r)| serde_json::json!({"cols": c, "rows": r})),
                                })),
                            );
                        }
                        continue;
                    }

                    if method == "terminal.resize" {
                        let terminal_id = params
                            .get("terminalId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        // Same 1-column safety margin as subscribe
                        let cols = params
                            .get("cols")
                            .and_then(|v| v.as_u64())
                            .map(|v| v.saturating_sub(1).max(10) as u16)
                            .unwrap_or(0);
                        let rows = params
                            .get("rows")
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u16)
                            .unwrap_or(0);
                        if terminal_id.is_empty() || cols == 0 || rows == 0 {
                            send_response(
                                &tx,
                                id.as_deref(),
                                Err("Missing terminalId, cols, or rows".to_string()),
                            );
                        } else {
                            let mut clients = reader_state.ws_clients.lock();
                            if let Some(client) =
                                clients.iter_mut().find(|c| c.client_id == client_id)
                            {
                                client.mobile_dims = Some((cols, rows));
                            }
                            drop(clients);
                            log_debug!(
                                "[companion-ws] Terminal resize: {} → {}x{}",
                                terminal_id,
                                cols,
                                rows
                            );
                            send_response(
                                &tx,
                                id.as_deref(),
                                Ok(serde_json::json!({"resized": true})),
                            );
                        }
                        continue;
                    }

                    if method == "terminal.unsubscribe" {
                        let terminal_id = params
                            .get("terminalId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !terminal_id.is_empty() {
                            let mut clients = reader_state.ws_clients.lock();
                            if let Some(client) = clients
                                .iter_mut()
                                .find(|c| c.session_token == session_token)
                            {
                                client.subscribed_terminals.remove(terminal_id);
                            }
                        }
                        send_response(
                            &tx,
                            id.as_deref(),
                            Ok(serde_json::json!({"unsubscribed": true})),
                        );
                        continue;
                    }

                    // Handle legacy subscribe/unsubscribe format (backwards compat)
                    let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if msg_type == "subscribe" || msg_type == "unsubscribe" {
                        let terminal_id =
                            msg.get("terminalId").and_then(|t| t.as_str()).unwrap_or("");
                        if !terminal_id.is_empty() {
                            let mut clients = reader_state.ws_clients.lock();
                            if let Some(client) = clients
                                .iter_mut()
                                .find(|c| c.session_token == session_token)
                            {
                                if msg_type == "subscribe" {
                                    client.subscribed_terminals.insert(terminal_id.to_string());
                                } else {
                                    client.subscribed_terminals.remove(terminal_id);
                                }
                            }
                        }
                        continue;
                    }

                    // Dispatch API method to internal server
                    if !method.is_empty() {
                        // Gate privileged spawn methods: refuse unless the
                        // operator has explicitly enabled remote spawn.
                        if super::proxy::is_privileged_spawn_method(method)
                            && !reader_state.allow_remote_spawn
                        {
                            send_response(
                                &tx,
                                id.as_deref(),
                                Err("Remote terminal spawn is disabled. Enable 'Allow remote spawn' in Companion settings and restart the tunnel.".to_string()),
                            );
                            continue;
                        }
                        let result = dispatch_ws_method(reader_state, method, &params);
                        send_response(&tx, id.as_deref(), result);
                        continue;
                    }
                }
                Ok(Message::Ping(data)) => {
                    let _ = ws.send(Message::Pong(data));
                }
                Ok(Message::Close(_)) => break,
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    // Read timeout — loop back to send pending messages, then retry read
                    continue;
                }
                Err(_) => break,
                _ => {}
            }
        }

        // Remove client on disconnect
        let mut clients = reader_state.ws_clients.lock();
        clients.retain(|c| c.client_id != client_id);
        log_debug!("[companion-ws] Client disconnected");
    });
}

/// Send a response to a WS client via its sender channel.
fn send_response(
    tx: &mpsc::Sender<String>,
    id: Option<&str>,
    result: Result<serde_json::Value, String>,
) {
    let msg = match (id, result) {
        (Some(id), Ok(data)) => serde_json::json!({
            "id": id,
            "result": data,
        }),
        (Some(id), Err(e)) => serde_json::json!({
            "id": id,
            "error": { "code": 400, "message": e },
        }),
        (None, Ok(data)) => serde_json::json!({
            "result": data,
        }),
        (None, Err(e)) => serde_json::json!({
            "error": { "code": 400, "message": e },
        }),
    };
    let _ = tx.send(msg.to_string());
}

/// Broadcast a push event to all authenticated WebSocket clients.
pub fn broadcast_event(state: &CompanionState, event_json: &str) {
    let clients = state.ws_clients.lock();
    for client in clients.iter() {
        if client.authenticated {
            let _ = client.sender.send(event_json.to_string());
        }
    }
}

/// Broadcast full scrollback history to subscribed clients.
/// Fires at the same frequency as terminal:grid (~10fps during active output).
/// Enables smooth real-time streaming on mobile without request-response round-trips.
pub fn broadcast_terminal_scrollback(state: &CompanionState, terminal_id: &str, lines: &[String]) {
    let event = serde_json::json!({
        "event": "terminal:scrollback",
        "data": {
            "terminalId": terminal_id,
            "lines": lines,
            "totalLines": lines.len(),
        }
    });
    let event_str = event.to_string();

    let clients = state.ws_clients.lock();
    for client in clients.iter() {
        if client.authenticated
            && client.subscribed_terminals.contains(terminal_id)
            && !state.client_skips_legacy_terminal(&client.session_token, terminal_id)
        {
            let _ = client.sender.send(event_str.clone());
        }
    }
}

/// Broadcast terminal output to clients subscribed to that terminal.
///
/// **Retired 0.32.13.** No longer called from the poll loop — mobile clients
/// reconstruct plain text from the richer `terminal:grid` event's
/// `CompactLine.text` field. Kept on the call surface for one release cycle
/// so older clients that explicitly opt into it (via a feature flag) can
/// still receive it if needed; schedule removal in 0.33.x.
#[allow(dead_code)]
pub fn broadcast_terminal_output(state: &CompanionState, terminal_id: &str, lines: &[String]) {
    let event = serde_json::json!({
        "event": "terminal:output",
        "data": {
            "terminalId": terminal_id,
            "lines": lines,
        }
    });
    let event_str = event.to_string();

    let clients = state.ws_clients.lock();
    for client in clients.iter() {
        if client.authenticated && client.subscribed_terminals.contains(terminal_id) {
            let _ = client.sender.send(event_str.clone());
        }
    }
}

/// Broadcast a CompactLine grid update to subscribed clients.
/// If a client has mobile_dims set, the grid is reflowed to those dimensions.
///
/// Reflow is cached per `(terminal_id, (cols, rows))` keyed by grid seqno.
/// When multiple clients share the same mobile dimensions (or the next
/// tick arrives without a grid change), the expensive reflow + serialize
/// is reused instead of recomputed per-client-per-tick. Criterion shows
/// the cache-hit path is ~2,250× faster than a fresh reflow.
pub fn broadcast_terminal_grid(
    state: &CompanionState,
    terminal_id: &str,
    grid: &crate::terminal::grid_types::GridUpdate,
) {
    let _h = crate::perf_hist!("broadcast_grid");
    let clients = state.ws_clients.lock();

    // Lazily serialize the desktop (un-reflowed) JSON once per call.
    let mut desktop_json: Option<String> = None;

    for client in clients.iter() {
        if !client.authenticated || !client.subscribed_terminals.contains(terminal_id) {
            continue;
        }
        // D2: this IPA opened a k1 grid WS for the same terminal — skip
        // CompactLine so we do not double-paint. Other clients still get it.
        if state.client_skips_legacy_terminal(&client.session_token, terminal_id) {
            continue;
        }

        let grid_json = if let Some((cols, rows)) = client.mobile_dims {
            // Per-dimension reflow cache. Only valid while seqno matches
            // the current grid; otherwise recompute + replace.
            let cache_key = (terminal_id.to_string(), (cols, rows));
            let cached = {
                let cache = state.reflow_cache.lock();
                cache.get(&cache_key).and_then(|(cached_seqno, json)| {
                    if *cached_seqno == grid.seqno && grid.seqno != 0 {
                        Some(json.clone())
                    } else {
                        None
                    }
                })
            };
            if let Some(json) = cached {
                json
            } else {
                let reflowed = crate::terminal::reflow::reflow_grid(grid, cols, rows);
                let json = serde_json::to_string(&reflowed).unwrap_or_default();
                // Store for subsequent clients this tick + future ticks at
                // the same seqno + dims.
                state
                    .reflow_cache
                    .lock()
                    .insert(cache_key, (grid.seqno, json.clone()));
                json
            }
        } else {
            // Desktop dimensions — no reflow, just serialize once and reuse.
            if desktop_json.is_none() {
                desktop_json = Some(serde_json::to_string(grid).unwrap_or_default());
            }
            desktop_json.clone().unwrap_or_default()
        };

        let event = format!(
            r#"{{"event":"terminal:grid","data":{{"terminalId":"{}","grid":{}}}}}"#,
            terminal_id, grid_json
        );
        let _ = client.sender.send(event);
    }
}

#[cfg(test)]
mod scrollback_skip_tests {
    use super::*;
    use crate::companion::types::CompanionState;
    use crate::terminal::grid_types::{CompactLine, GridUpdate};

    fn push_client(state: &CompanionState, token: &str, terminal: &str) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        state.ws_clients.lock().push(WsClient {
            client_id: uuid::Uuid::new_v4().to_string(),
            session_token: token.to_string(),
            authenticated: true,
            subscribed_terminals: HashSet::from([terminal.to_string()]),
            mobile_dims: None,
            sender: tx,
            last_seen: Instant::now(),
        });
        rx
    }

    #[test]
    fn grid_client_skips_scrollback_other_ipa_still_gets_it() {
        let state = CompanionState::new(0, "hook-secret".into());
        let grid_rx = push_client(&state, "new-ipa-token", "term-1");
        let old_rx = push_client(&state, "old-ipa-token", "term-1");
        state.note_grid_ws_open("new-ipa-token", "term-1");

        broadcast_terminal_scrollback(&state, "term-1", &["hello".into()]);

        let old = old_rx
            .try_recv()
            .expect("old IPA must still receive scrollback");
        assert!(old.contains("terminal:scrollback"), "{old}");
        assert!(old.contains("hello"), "{old}");
        assert!(
            grid_rx.try_recv().is_err(),
            "the client with a live grid WS must not get terminal:scrollback"
        );
    }

    #[test]
    fn grid_client_skips_compactline_other_ipa_still_gets_it() {
        let state = CompanionState::new(0, "hook-secret".into());
        let grid_rx = push_client(&state, "new-ipa-token", "term-1");
        let old_rx = push_client(&state, "old-ipa-token", "term-1");
        state.note_grid_ws_open("new-ipa-token", "term-1");

        let grid = GridUpdate {
            cols: 8,
            rows: 1,
            cursor_col: 0,
            cursor_row: 0,
            cursor_visible: true,
            cursor_shape: "block".into(),
            lines: vec![CompactLine {
                row: 0,
                text: "x".into(),
                spans: Vec::new(),
                wrapped: false,
            }],
            full: true,
            mode: 0,
            display_offset: 0,
            selection: None,
            perf: None,
            seqno: 1,
        };
        broadcast_terminal_grid(&state, "term-1", &grid);

        let old = old_rx
            .try_recv()
            .expect("old IPA must still receive CompactLine");
        assert!(old.contains("terminal:grid"), "{old}");
        assert!(
            grid_rx.try_recv().is_err(),
            "the client with a live grid WS must not get terminal:grid"
        );
    }

    #[test]
    fn skip_is_per_terminal_not_global() {
        let state = CompanionState::new(0, "hook-secret".into());
        let (tx, rx) = mpsc::channel();
        state.ws_clients.lock().push(WsClient {
            client_id: "c1".into(),
            session_token: "tok".into(),
            authenticated: true,
            subscribed_terminals: HashSet::from(["term-a".into(), "term-b".into()]),
            mobile_dims: None,
            sender: tx,
            last_seen: Instant::now(),
        });
        state.note_grid_ws_open("tok", "term-a");

        broadcast_terminal_scrollback(&state, "term-a", &["a".into()]);
        broadcast_terminal_scrollback(&state, "term-b", &["b".into()]);

        let got = rx
            .try_recv()
            .expect("term-b scrollback should still arrive");
        assert!(got.contains("term-b"), "{got}");
        assert!(rx.try_recv().is_err(), "term-a should have been skipped");
    }
}
