//! K2SO Mobile Companion API
//!
//! Exposes a curated subset of K2SO's internal HTTP API through an ngrok tunnel.
//! The companion app connects to this tunnel to monitor and interact with agents remotely.
//!
//! Architecture:
//! - Starts an ngrok tunnel and accepts connections directly from it
//! - Proxies validated requests to the internal K2SO HTTP server
//! - Supports WebSocket for real-time event push
//! - Tokio runtime kept alive for the tunnel's lifetime

// Sub-modules. Pure infrastructure (auth, keychain, proxy, types,
// websocket) plus the four host-bridges (settings_bridge,
// terminal_bridge, event_sink, app_event_source) that let this module
// stay Tauri-free.
pub mod app_event_source;
pub mod auth;
pub mod cli_routes;
pub mod event_sink;
pub mod keychain;
pub mod proxy;
pub mod settings_bridge;
pub mod terminal_bridge;
pub mod types;
pub mod websocket;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use ngrok::config::ForwarderBuilder;
use ngrok::tunnel::EndpointInfo;

use types::{AuthRateLimiter, CompanionState};

/// Module-level companion state. Mutex<Option> allows stop + restart
/// (unlike OnceLock). `pub` so src-tauri's companion commands can read
/// it — the previous pub(crate) was k2so-core-local which blocked
/// downstream callers after the migration.
pub static STATE: Mutex<Option<CompanionState>> = Mutex::new(None);
/// Flag indicating the companion is running.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Flag to cancel a pending auto-start attempt.
static CANCEL_START: AtomicBool = AtomicBool::new(false);

/// Start the companion API proxy.
/// Reads settings for credentials + ngrok token, starts tunnel, spawns listener.
/// Returns the public tunnel URL.
/// Start the companion API proxy. Reads settings via the core
/// `settings_bridge` (Tauri app registers `TauriCompanionSettingsProvider`
/// at startup) and emits lifecycle events via `event_sink`. No Tauri
/// dep — safe to move into k2so-core in the next commit.
pub fn start_companion() -> Result<String, String> {
    // Clear any previous cancel signal
    CANCEL_START.store(false, Ordering::Relaxed);

    if RUNNING.load(Ordering::Relaxed) {
        return Err("Companion is already running".to_string());
    }

    let companion = settings_bridge::read_settings();

    if companion.username.is_empty() {
        return Err("Username is required".to_string());
    }
    if !auth::has_password() {
        return Err("Password must be set before enabling companion".to_string());
    }
    if companion.ngrok_auth_token.is_empty() {
        return Err("ngrok auth token is required".to_string());
    }

    let hook_port = crate::hook_config::get_port();
    let hook_token = crate::hook_config::get_token();

    if hook_port == 0 || hook_token.is_empty() {
        return Err("K2SO internal server not ready yet".to_string());
    }

    let ngrok_token = companion.ngrok_auth_token.clone();
    let ngrok_domain = companion.ngrok_domain.clone();

    // Bind a local TcpListener for the companion HTTP server
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind companion listener: {}", e))?;
    let local_port = listener.local_addr().unwrap().port();

    // Start ngrok tunnel — forwards traffic to our local companion HTTP server
    let (tunnel_url, _rt) = start_ngrok_tunnel(&ngrok_token, &ngrok_domain, local_port)?;

    // Check if stop was requested while we were connecting
    if CANCEL_START.load(Ordering::Relaxed) {
        crate::log_debug!("[companion] Start cancelled during tunnel connect");
        return Err("Start cancelled".to_string());
    }

    log_debug!("[companion] Tunnel: {} → localhost:{}", tunnel_url, local_port);

    // Channel to keep the tunnel runtime alive
    let (keepalive_tx, _keepalive_rx) = std::sync::mpsc::channel::<()>();

    // Initialize state
    let state = CompanionState {
        tunnel_url: Mutex::new(Some(tunnel_url.clone())),
        sessions: Mutex::new(HashMap::new()),
        ws_clients: Mutex::new(Vec::new()),
        shutdown: AtomicBool::new(false),
        hook_port,
        hook_token: hook_token.to_string(),
        cors_origins: companion.cors_origins.clone(),
        allow_remote_spawn: companion.allow_remote_spawn,
        auth_limiter: Mutex::new(AuthRateLimiter::new()),
        reflow_cache: Mutex::new(HashMap::new()),
        _tunnel_keepalive: Mutex::new(Some(keepalive_tx)),
    };
    *STATE.lock() = Some(state);
    RUNNING.store(true, Ordering::Relaxed);

    // Spawn the local companion HTTP server (handles auth + proxying)
    std::thread::spawn(move || {
        run_local_listener(listener);
    });

    // Keep the tokio runtime alive on a background thread (ngrok tunnel needs it)
    std::thread::spawn(move || {
        // _rt stays alive as long as this thread runs
        // _keepalive_rx blocks until the sender is dropped (on stop)
        let _ = _keepalive_rx.recv();
        crate::log_debug!("[companion] Tunnel runtime shutting down");
        drop(_rt);
    });

    // Spawn session cleanup thread (every 5 minutes, expire old sessions)
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            if !RUNNING.load(Ordering::Relaxed) { break; }
            if let Some(ref state) = *STATE.lock() {
                let mut sessions = state.sessions.lock();
                sessions.retain(|_, s| !s.is_expired());
            }
        }
    });

    // Prominent startup warning. The companion tunnel is public surface, and
    // there are a couple of knobs that materially change the exposure. Put
    // them in the log so operators can verify what's live at a glance.
    log_debug!(
        "[companion] ⚠️  TUNNEL ACTIVE — {} is now reachable from the public internet. \
         remote_spawn={} cors_origins={} (toggle these in Settings → Companion)",
        tunnel_url,
        if companion.allow_remote_spawn { "ENABLED (arbitrary shell commands permitted)" } else { "disabled" },
        if companion.cors_origins.is_empty() { "none (browser access blocked)".to_string() } else { companion.cors_origins.join(", ") },
    );

    // Notify the UI so it can show a banner / toast. Goes through the
    // k2so-core event sink the Tauri app registered in setup().
    event_sink::emit(
        "companion:tunnel_activated",
        serde_json::json!({
            "tunnelUrl": tunnel_url,
            "allowRemoteSpawn": companion.allow_remote_spawn,
            "corsOriginsCount": companion.cors_origins.len(),
        }),
    );

    // Subscribe to the host's app-event source for the events companion
    // mirrors to WS clients.
    register_event_listeners();

    // Spawn terminal output polling thread. Pulls grids via
    // terminal_bridge::get_grid (Tauri app registered the provider).
    std::thread::spawn(|| {
        run_terminal_polling();
    });

    // Spawn tunnel health check — restarts companion if local listener dies
    let health_port = local_port;
    std::thread::spawn(move || {
        // Wait before first check to let everything settle
        std::thread::sleep(std::time::Duration::from_secs(30));
        let mut consecutive_failures = 0u32;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(15));
            if !RUNNING.load(Ordering::Relaxed) { break; }

            // Probe the LOCAL listener — this is ground truth (ngrok URL can return
            // HTML error pages that look like HTTP 200 even when the forwarder is dead)
            let probe = reqwest::blocking::Client::new()
                .get(&format!("http://127.0.0.1:{}/companion/auth", health_port))
                .timeout(std::time::Duration::from_secs(3))
                .send();

            match probe {
                Ok(_) => {
                    consecutive_failures = 0;
                }
                Err(_) => {
                    consecutive_failures += 1;
                    crate::log_debug!("[companion] Tunnel health check failed ({}/3)", consecutive_failures);
                    if consecutive_failures >= 3 {
                        crate::log_debug!("[companion] Tunnel dead — restarting companion");
                        // Stop and restart
                        let _ = stop_companion();
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        match start_companion() {
                            Ok(url) => log_debug!("[companion] Auto-restarted: {}", url),
                            Err(e) => log_debug!("[companion] Auto-restart failed: {}", e),
                        }
                        break; // New health check thread spawned by the new start_companion
                    }
                }
            }
        }
    });

    Ok(tunnel_url)
}

/// Stop the companion API proxy.
pub fn stop_companion() -> Result<(), String> {
    // Signal any in-flight start attempt to abort
    CANCEL_START.store(true, Ordering::Relaxed);

    if !RUNNING.load(Ordering::Relaxed) {
        return Ok(());
    }

    // Signal shutdown and drop keepalive
    {
        let guard = STATE.lock();
        if let Some(ref state) = *guard {
            state.shutdown.store(true, Ordering::Relaxed);
            *state._tunnel_keepalive.lock() = None;
            *state.tunnel_url.lock() = None;
            state.ws_clients.lock().clear();
            state.sessions.lock().clear();
        }
    }

    // Clear the state entirely so a fresh one can be created on restart
    *STATE.lock() = None;

    // Drop the forwarder to stop ngrok from forwarding traffic
    *NGROK_FORWARDER.lock() = None;

    // Session kept alive in NGROK_SESSION — closed on next start.

    RUNNING.store(false, Ordering::Relaxed);
    log_debug!("[companion] Stopped");

    Ok(())
}

/// Invalidate every active companion session + kick every connected WS client.
///
/// Called when the companion credentials (username / password hash) change,
/// or on explicit settings reset. Idempotent: safe to call when the companion
/// isn't running.
pub fn invalidate_all_sessions(reason: &str) {
    let guard = STATE.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return,
    };
    let removed = {
        let mut sessions = state.sessions.lock();
        let n = sessions.len();
        sessions.clear();
        n
    };
    let disconnected = {
        let mut clients = state.ws_clients.lock();
        let n = clients.len();
        clients.clear();
        n
    };
    if removed + disconnected > 0 {
        crate::log_debug!(
            "[companion] Invalidated {} session(s) and {} WS client(s): {}",
            removed,
            disconnected,
            reason
        );
    }
}

/// Get companion status. Non-blocking — uses try_lock to avoid blocking the main thread.
pub fn companion_status() -> serde_json::Value {
    if !RUNNING.load(Ordering::Relaxed) {
        return serde_json::json!({
            "running": false,
            "tunnelUrl": null,
            "connectedClients": 0,
            "sessions": [],
        });
    }

    // Use try_lock to avoid blocking the UI thread if the companion thread
    // holds the lock. parking_lot::Mutex::try_lock returns Option (not
    // Result like std::sync), so pattern-match on Some/None.
    let guard = match STATE.try_lock() {
        Some(g) => g,
        None => {
            // Lock contended — return minimal status without blocking
            return serde_json::json!({
                "running": true,
                "tunnelUrl": null,
                "connectedClients": 0,
                "sessions": [],
            });
        }
    };

    if let Some(ref state) = *guard {
        let url = state.tunnel_url.try_lock().and_then(|u| u.clone());
        let session_list: Vec<serde_json::Value> = state.sessions.try_lock()
            .map(|sessions| sessions.values().map(|s| {
                serde_json::json!({
                    "token": format!("{}...", &s.token[..8.min(s.token.len())]),
                    "remoteAddr": s.remote_addr,
                    "createdAt": s.created_at.to_rfc3339(),
                    "expiresAt": s.expires_at.to_rfc3339(),
                })
            }).collect())
            .unwrap_or_default();
        let session_count = session_list.len();
        let ws_count = state.ws_clients.try_lock().map(|c| c.len()).unwrap_or(0);

        serde_json::json!({
            "running": true,
            "tunnelUrl": url,
            "connectedClients": session_count,
            "wsClients": ws_count,
            "sessions": session_list,
        })
    } else {
        serde_json::json!({ "running": false })
    }
}

/// Start the ngrok tunnel using listen_and_forward to proxy to a local port.
/// Returns the public URL and the tokio runtime (must be kept alive for the tunnel).
///
/// The ngrok connect future never yields, so tokio timeouts can't cancel it.
/// We run it on a dedicated thread and wait with recv_timeout. If it times out,
/// the thread keeps running (the runtime stays alive there) — next call will
/// find the session in the static and reuse it.
fn start_ngrok_tunnel(ngrok_token: &str, ngrok_domain: &str, local_port: u16) -> Result<(String, tokio::runtime::Runtime), String> {
    let token = ngrok_token.to_string();
    let domain = ngrok_domain.to_string();
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(Err(format!("Failed to create tokio runtime: {}", e)));
                return;
            }
        };

        // Single block_on call — setup + keepalive in one async block.
        // The forwarder's background task runs on this runtime. If we exit
        // block_on and re-enter it, the task can get interrupted. Keeping
        // everything in one block_on ensures the runtime event loop stays
        // active for the forwarder's lifetime.
        rt.block_on(async {
            // Close any existing session
            if let Some(mut old_session) = NGROK_SESSION.lock().take() {
                crate::log_debug!("[companion] Closing previous ngrok session...");
                let _ = old_session.close().await;
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }

            crate::log_debug!("[companion] Creating new ngrok session");
            let connect_result: Result<String, String> = async {
                let session = ngrok::Session::builder()
                    .authtoken(&token)
                    .connect()
                    .await
                    .map_err(|e| format!("ngrok connect failed: {}", e))?;

                let forward_url = url::Url::parse(&format!("http://localhost:{}", local_port))
                    .map_err(|e| format!("Invalid forward URL: {}", e))?;

                let mut endpoint = session.http_endpoint();
                if !domain.is_empty() {
                    endpoint.domain(&domain);
                }
                let listener = endpoint
                    .listen_and_forward(forward_url)
                    .await
                    .map_err(|e| format!("ngrok tunnel failed: {}", e))?;

                let url = listener.url().to_string();
                NGROK_SESSION.lock().replace(session);
                NGROK_FORWARDER.lock().replace(listener);

                Ok(url)
            }.await;

            // Send result back to caller
            let _ = tx.send(connect_result);

            // Park in the event loop — keeps the forwarder's async task alive.
            // The forwarder proxies connections as long as this block_on is active.
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                if !RUNNING.load(Ordering::Relaxed) { break; }
            }
        });
        crate::log_debug!("[companion] Tunnel runtime thread exiting");
    });

    // Wait up to 20 seconds for the tunnel URL
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(Ok(url)) => {
            // The real runtime stays alive on the spawned thread (block_on loop).
            // Create a lightweight runtime for the keepalive thread contract.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .map_err(|e| format!("Failed to create runtime: {}", e))?;
            Ok((url, rt))
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err("ngrok connect timed out (20s) — old session may still be active".to_string()),
    }
}

static NGROK_SESSION: Mutex<Option<ngrok::Session>> = Mutex::new(None);
static NGROK_FORWARDER: Mutex<Option<ngrok::forwarder::Forwarder<ngrok::tunnel::HttpTunnel>>> = Mutex::new(None);

/// Local companion HTTP server — accepts connections from ngrok (forwarded via listen_and_forward).
/// Same blocking TCP pattern as agent_hooks.rs.
fn run_local_listener(listener: std::net::TcpListener) {
    use std::io::Read;

    // Set a timeout so the listener doesn't block forever on accept —
    // allows periodic shutdown checks even when no connections arrive.
    listener.set_nonblocking(false).ok();

    log_debug!("[companion] Local listener started on port {}", listener.local_addr().map(|a| a.port()).unwrap_or(0));
    let mut request_count = 0u64;
    for stream in listener.incoming() {
        request_count += 1;
        // Wrap each connection in catch_unwind to prevent a single bad request
        // from killing the entire listener thread.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Check shutdown
            {
                let guard = STATE.lock();
                if let Some(ref state) = *guard {
                    if state.shutdown.load(Ordering::Relaxed) { return false; }
                }
            }

            let Ok(mut stream) = stream else {
                crate::log_debug!("[companion] Listener accept error — continuing");
                return true;
            };
            let remote_addr = stream.peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|_| "ngrok".to_string());

            // Peek at the request to detect WebSocket upgrade without consuming data.
            // tungstenite::accept() needs to read the upgrade request itself.
            let mut peek_buf = [0u8; 4096];
            let peek_n = match stream.peek(&mut peek_buf) {
                Ok(0) => return true,
                Ok(n) => n,
                Err(_) => return true,
            };
            let peek_str = String::from_utf8_lossy(&peek_buf[..peek_n]);
            let is_websocket = peek_str.to_lowercase().contains("upgrade: websocket");

            if is_websocket {
                // Extract path from the first line (before consuming)
                let path = peek_str.lines().next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/companion/ws");
                let path = path.to_string();

                // Parse Origin + Host from the upgrade request and enforce
                // policy before handing the stream to tungstenite. A rejected
                // upgrade gets a plain HTTP 403 and we drop the stream.
                let headers = proxy::parse_headers(&peek_str);
                let request_origin = headers.get("origin").map(|s| s.as_str());
                let request_host = headers.get("host").map(|s| s.as_str());

                let (allowed, tunnel_snapshot, allowlist_snapshot) = {
                    let guard = STATE.lock();
                    match guard.as_ref() {
                        Some(state) => {
                            let tunnel = state.tunnel_url.try_lock().and_then(|u| u.clone());
                            let allowlist = state.cors_origins.clone();
                            let origin_ok = proxy::ws_origin_allowed(
                                request_origin,
                                tunnel.as_deref(),
                                &allowlist,
                            );
                            let host_ok = proxy::host_allowed(
                                request_host,
                                tunnel.as_deref(),
                                &allowlist,
                            );
                            (origin_ok && host_ok, tunnel, allowlist)
                        }
                        None => (false, None, Vec::new()),
                    }
                };
                let _ = (tunnel_snapshot, allowlist_snapshot);

                if !allowed {
                    use std::io::Write;
                    let body = b"{\"ok\":false,\"error\":\"WebSocket Origin not allowed\"}";
                    let response = format!(
                        "HTTP/1.1 403 Forbidden\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         X-Frame-Options: DENY\r\n\
                         X-Content-Type-Options: nosniff\r\n\
                         \r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.write_all(body);
                    crate::log_debug!(
                        "[companion-ws] Rejected WS upgrade — Origin {:?} not allowed",
                        request_origin
                    );
                    return true;
                }

                // Pass unread stream to tungstenite — it reads the upgrade request itself
                let ws_guard = STATE.lock();
                if let Some(ref ws_state) = *ws_guard {
                    let state_ptr = ws_state as *const CompanionState;
                    drop(ws_guard);
                    websocket::handle_ws_upgrade(stream, &path, unsafe { &*state_ptr });
                }
                return true;
            }

            // Not WebSocket — read the full request for HTTP handling
            let mut buf = [0u8; 65536];
            let n = match stream.read(&mut buf) {
                Ok(0) => return true,
                Ok(n) => n,
                Err(_) => return true,
            };
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let first_line = request.lines().next().unwrap_or("");
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            let (method, path) = match parts.as_slice() {
                [m, p, ..] => (*m, *p),
                _ => return true,
            };

            let headers = proxy::parse_headers(&request);

            let guard = STATE.lock();
            if let Some(ref state) = *guard {
                proxy::handle_request(&mut stream, state, method, path, &headers, &request, &remote_addr);
            }
            true // continue loop
        }));

        match result {
            Ok(false) => break, // shutdown requested
            Ok(true) => continue,
            Err(panic) => {
                let msg = panic.downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                crate::log_debug!("[companion] Listener caught panic: {} — continuing", msg);
                continue; // Don't let a panic kill the listener
            }
        }
    }

    log_debug!("[companion] Local listener ended after {} requests", request_count);
    RUNNING.store(false, Ordering::Relaxed);
}

/// Subscribe to host app events so companion can mirror them to WS
/// clients. Runs through the `app_event_source` bridge the Tauri app
/// registered at startup, so no direct Tauri dep is taken here.
fn register_event_listeners() {
    let events: &[&'static str] = &["agent:lifecycle", "agent:reply", "sync:projects"];
    app_event_source::subscribe(
        events,
        Box::new(|event_name, payload| {
            if let Some(ref state) = *STATE.lock() {
                let ws_event = serde_json::json!({
                    "event": event_name,
                    "data": payload,
                });
                websocket::broadcast_event(state, &ws_event.to_string());
            }
        }),
    );

    // Terminal grid updates are handled by run_terminal_polling() which reads
    // CompactLine data directly via terminal_bridge at 10fps and broadcasts
    // to subscribed WS clients. No event subscription needed.
}

/// Poll terminal grids for subscribed WebSocket clients.
/// Reads CompactLine data directly from the terminal manager (no HTTP roundtrip).
/// Broadcasts both rich CompactLine grid updates and full scrollback.
///
/// Change detection: each grid carries a monotonic `seqno` bumped by the
/// alacritty backend on every emission. We cache the last-broadcast seqno
/// per terminal and skip all downstream work (reflow, JSON encode, emit)
/// when seqno is unchanged. Before 0.32.13 this used an ahash content hash,
/// which needed to iterate every line on every poll — criterion benches
/// show the seqno compare is ~1000× faster than the hash path.
fn run_terminal_polling() {
    let mut last_seqnos: HashMap<String, u64> = HashMap::new();

    loop {
        // 100ms interval = ~10fps (throttled for mobile bandwidth)
        std::thread::sleep(std::time::Duration::from_millis(100));

        if !RUNNING.load(Ordering::Relaxed) { break; }

        let _poll_tick = crate::perf_hist!("terminal_poll_tick");

        let guard = STATE.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => break,
        };

        // Collect all subscribed terminal IDs
        let terminal_ids: Vec<String> = {
            let clients = state.ws_clients.lock();
            let mut ids = std::collections::HashSet::new();
            for client in clients.iter() {
                if client.authenticated {
                    ids.extend(client.subscribed_terminals.iter().cloned());
                }
            }
            ids.into_iter().collect()
        };

        if terminal_ids.is_empty() { continue; }

        for tid in &terminal_ids {
            if let Ok(grid) = terminal_bridge::get_grid(tid) {
                // Seqno-based change detection: compare the grid's monotonic
                // counter against what we last broadcast. Constant-time,
                // no iteration over lines. A seqno of 0 means "unstamped"
                // (shouldn't happen for a terminal created post-0.32.13
                // but guard anyway — treat 0 as always-dirty).
                let _h = crate::perf_hist!("grid_hash");
                let prev_seqno = last_seqnos.get(tid).copied().unwrap_or(0);
                if grid.seqno != 0 && grid.seqno == prev_seqno {
                    continue;
                }
                last_seqnos.insert(tid.clone(), grid.seqno);
                drop(_h);

                // Broadcast rich CompactLine grid update (reflowed per-client if mobile dims set)
                websocket::broadcast_terminal_grid(state, tid, &grid);

                // Push full scrollback for subscribers that need smooth streaming.
                // This fires at the same frequency as terminal:grid (~10fps during
                // active output) so the mobile app gets real-time push delivery
                // of the full conversation thread — no request-response round-trip.
                if let Ok(scrollback_lines) =
                    terminal_bridge::read_lines_with_scrollback(tid, 500, true)
                {
                    websocket::broadcast_terminal_scrollback(state, tid, &scrollback_lines);
                }

                // 0.32.13: legacy plain-text `terminal:output` event retired.
                // Mobile clients reconstruct plain text from `terminal:grid`
                // (CompactLine.text field). Going from 3 broadcasts → 2 per
                // tick; P2.2 cuts scrollback too for unchanged grids.
                // Companion App team was notified in the 0.32.12 memo.
            }
        }
    }
}
