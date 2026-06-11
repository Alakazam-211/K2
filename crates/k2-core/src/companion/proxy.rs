use std::collections::HashMap;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, TcpStream};
use super::auth;
use super::types::CompanionState;

/// Extract the real client IP from X-Forwarded-For (ngrok sets this). Falls
/// back to the raw TCP peer address, which will be 127.0.0.1 once traffic
/// arrives through the local ngrok forwarder. Only processes that already
/// have local execution can spoof XFF, and at that point they are past the
/// trust boundary anyway — honest fallback is fine.
pub fn client_ip(headers: &HashMap<String, String>, remote_addr: &str) -> IpAddr {
    if let Some(xff) = headers.get("x-forwarded-for") {
        if let Some(first) = xff.split(',').next() {
            if let Ok(ip) = first.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    remote_addr
        .rsplit_once(':')
        .map(|(ip, _)| ip)
        .unwrap_or(remote_addr)
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .unwrap_or(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))
}

/// True iff the HTTP route launches a fresh terminal (arbitrary shell command
/// with arbitrary args). These are gated behind CompanionSettings.allow_remote_spawn
/// because they give the caller arbitrary-code-execution on the user's Mac.
pub fn is_privileged_spawn_path(path: &str) -> bool {
    let clean = path.split('?').next().unwrap_or("");
    matches!(
        clean,
        "/companion/terminal/spawn" | "/companion/terminal/spawn-background"
    )
}

/// True iff a WS method name is one of the privileged spawn methods.
pub fn is_privileged_spawn_method(method: &str) -> bool {
    matches!(method, "terminal.spawn" | "terminal.spawn_background")
}

/// Route mapping: companion endpoint → (internal route, allowed methods)
fn map_route(method: &str, path: &str) -> Option<&'static str> {
    let clean = path.split('?').next().unwrap_or("");
    match (method, clean) {
        ("GET",  "/companion/agents")         => Some("/cli/agents/list"),
        ("GET",  "/companion/agents/running")  => Some("/cli/agents/running"),
        ("GET",  "/companion/agents/work")     => Some("/cli/agents/work"),
        ("GET",  "/companion/reviews")         => Some("/cli/reviews"),
        ("POST", "/companion/review/approve")  => Some("/cli/review/approve"),
        ("POST", "/companion/review/reject")   => Some("/cli/review/reject"),
        ("POST", "/companion/review/feedback") => Some("/cli/review/feedback"),
        ("GET",  "/companion/terminal/read")   => Some("/cli/terminal/read"),
        ("POST", "/companion/terminal/write")  => Some("/cli/terminal/write"),
        ("GET",  "/companion/status")          => Some("/cli/mode"),
        ("POST", "/companion/agents/wake")     => Some("/cli/agents/launch"),
        ("GET",  "/companion/projects")         => Some("/cli/companion/projects"),
        ("GET",  "/companion/projects/summary") => Some("/cli/companion/projects-summary"),
        ("GET",  "/companion/sessions")         => Some("/cli/companion/sessions"),
        ("GET",  "/companion/presets")          => Some("/cli/companion/presets"),
        ("POST", "/companion/terminal/spawn")   => Some("/cli/terminal/spawn"),
        ("POST", "/companion/terminal/spawn-background") => Some("/cli/terminal/spawn-background"),
        _ => None,
    }
}

/// Parse HTTP headers from raw request bytes.
pub fn parse_headers(request: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for line in request.lines().skip(1) {
        if line.is_empty() { break; }
        if let Some((key, val)) = line.split_once(':') {
            headers.insert(key.trim().to_lowercase(), val.trim().to_string());
        }
    }
    headers
}

/// Extract query parameters from a URL path.
pub fn parse_query(path: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    if let Some(query) = path.split_once('?').map(|(_, q)| q) {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                params.insert(
                    urldecode(k),
                    urldecode(v),
                );
            }
        }
    }
    params
}

/// Parse JSON body from POST request (after the blank line separator).
pub fn parse_json_body(request: &str) -> Option<serde_json::Value> {
    let body = request.split("\r\n\r\n").nth(1)?;
    serde_json::from_str(body).ok()
}

/// URL-decode a string (handles %XX and +).
fn urldecode(s: &str) -> String {
    let s = s.replace('+', " ");
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

/// Map a WebSocket method name to an internal route.
/// Returns (internal_route, is_global) — global routes don't need a project param.
pub fn map_ws_method(method: &str) -> Option<(&'static str, bool)> {
    match method {
        "projects.list"    => Some(("/cli/companion/projects", true)),
        "projects.summary" => Some(("/cli/companion/projects-summary", true)),
        "sessions.list"    => Some(("/cli/companion/sessions", true)),
        "agents.list"      => Some(("/cli/agents/list", false)),
        "agents.running"   => Some(("/cli/agents/running", false)),
        "agents.work"      => Some(("/cli/agents/work", false)),
        "agents.wake"      => Some(("/cli/agents/launch", false)),
        "reviews.list"     => Some(("/cli/reviews", false)),
        "review.approve"   => Some(("/cli/review/approve", false)),
        "review.reject"    => Some(("/cli/review/reject", false)),
        "review.feedback"  => Some(("/cli/review/feedback", false)),
        "terminal.read"    => Some(("/cli/terminal/read", false)),
        "terminal.write"   => Some(("/cli/terminal/write", false)),
        "terminal.spawn"   => Some(("/cli/terminal/spawn", false)),
        "terminal.spawn_background" => Some(("/cli/terminal/spawn-background", false)),
        "presets.list"     => Some(("/cli/companion/presets", true)),
        "status"           => Some(("/cli/mode", false)),
        _ => None,
    }
}

/// Execute a WebSocket method by forwarding to the internal K2SO HTTP server.
/// Converts method params to query params on the internal request.
pub fn dispatch_ws_method(
    state: &CompanionState,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (internal_route, is_global) = map_ws_method(method)
        .ok_or_else(|| format!("Unknown method: {}", method))?;

    let project = params.get("project").and_then(|v| v.as_str()).unwrap_or("");
    if !is_global && project.is_empty() {
        return Err("Missing 'project' param".to_string());
    }

    // Build query params
    let mut query = vec![
        format!("token={}", urlencode(&state.hook_token)),
    ];
    if !project.is_empty() {
        query.push(format!("project={}", urlencode(project)));
    }

    // Forward all params (except project/token which are already handled)
    if let Some(obj) = params.as_object() {
        for (k, v) in obj {
            if k == "project" || k == "token" { continue; }
            let val = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            query.push(format!("{}={}", urlencode(k), urlencode(&val)));
        }
    }

    let url = format!(
        "http://127.0.0.1:{}{}?{}",
        state.hook_port, internal_route, query.join("&")
    );

    let resp = reqwest::blocking::get(&url)
        .map_err(|e| format!("Internal request failed: {}", e))?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();

    if status.is_success() {
        let data: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::Value::String(text));
        Ok(serde_json::json!({"ok": true, "data": data}))
    } else {
        let error: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::json!({"error": text}));
        let msg = error.get("error").and_then(|e| e.as_str()).unwrap_or("Internal error");
        Err(msg.to_string())
    }
}

/// URL-encode a string for query parameters.
fn urlencode(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// Validate the `Host` request header against an allowlist of bare hostnames
/// (port stripped). Defends against DNS rebinding: an attacker DNS-pointing
/// their domain at the user's ngrok URL would send `Host: attacker.com`,
/// which won't match the real tunnel host.
///
/// Policy:
///   - Missing/empty Host → reject (HTTP/1.1 requires Host)
///   - Host == the tunnel URL's host → allow
///   - Host is loopback (127.0.0.1, localhost, ::1) → allow
///   - Host matches an entry in `allowlist` (CORS origins) → allow
///   - Anything else → reject
pub fn host_allowed(
    request_host: Option<&str>,
    tunnel_url: Option<&str>,
    allowlist: &[String],
) -> bool {
    let raw = match request_host {
        Some(s) => s.trim(),
        None => return false,
    };
    if raw.is_empty() {
        return false;
    }
    let host = strip_port(&raw.to_lowercase());

    if matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return true;
    }

    if let Some(tunnel) = tunnel_url {
        if host == strip_port(&normalize_origin_host(tunnel)) {
            return true;
        }
    }

    for allowed in allowlist {
        if host == strip_port(&normalize_origin_host(allowed)) {
            return true;
        }
    }

    false
}

/// Extract the host (with port) from a normalized origin or full URL.
fn normalize_origin_host(url: &str) -> String {
    let norm = normalize_origin(url);
    match norm.split_once("://") {
        Some((_, rest)) => rest.to_string(),
        None => norm,
    }
}

/// Drop the port from a `host[:port]` string, handling bracketed IPv6.
fn strip_port(host_with_port: &str) -> String {
    if host_with_port.starts_with('[') {
        if let Some(end) = host_with_port.find(']') {
            return host_with_port[1..end].to_string();
        }
    }
    host_with_port
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Decide whether a WebSocket upgrade should be accepted based on its Origin
/// header. Called before `tungstenite::accept` so we can reject with a plain
/// HTTP 403 before the protocol handshake.
///
/// Policy:
///   - Missing / empty Origin → allow (native mobile apps don't set Origin)
///   - Origin equals the current tunnel URL (scheme://host) → allow
///   - Origin exactly matches an entry in `cors_origins` → allow
///   - Origin is loopback (http://127.0.0.1, http://localhost[:port]) → allow
///   - Anything else → reject
pub fn ws_origin_allowed(
    request_origin: Option<&str>,
    tunnel_url: Option<&str>,
    allowlist: &[String],
) -> bool {
    let origin = match request_origin {
        Some(s) => s.trim(),
        None => return true,
    };
    if origin.is_empty() {
        return true;
    }

    if let Some(tunnel) = tunnel_url {
        if origin_matches(origin, tunnel) {
            return true;
        }
    }

    if allowlist.iter().any(|allowed| origin_matches(origin, allowed)) {
        return true;
    }

    is_loopback_origin(origin)
}

/// Compare two origin strings scheme+host+port, ignoring path + trailing slash.
fn origin_matches(a: &str, b: &str) -> bool {
    normalize_origin(a) == normalize_origin(b)
}

fn normalize_origin(s: &str) -> String {
    let trimmed = s.trim().trim_end_matches('/');
    // Strip any trailing path — keep scheme://host[:port].
    if let Some(scheme_end) = trimmed.find("://") {
        let after_scheme = &trimmed[scheme_end + 3..];
        let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
        format!("{}://{}", &trimmed[..scheme_end], &after_scheme[..host_end]).to_lowercase()
    } else {
        trimmed.to_lowercase()
    }
}

fn is_loopback_origin(origin: &str) -> bool {
    let norm = normalize_origin(origin);
    let host_part = match norm.split_once("://") {
        Some((_, rest)) => rest,
        None => return false,
    };
    // Bracketed IPv6 looks like `[::1]:9000` or `[::1]`. Non-bracketed split
    // on the first `:` gives us host-vs-port.
    let host = if host_part.starts_with('[') {
        match host_part.find(']') {
            Some(end) => &host_part[1..end],
            None => return false,
        }
    } else {
        host_part.split(':').next().unwrap_or("")
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Resolve the CORS `Access-Control-Allow-Origin` value for a request.
/// Returns Some(origin) iff the request's `Origin` header matches an entry in
/// the allowlist. Returns None otherwise — caller must not emit CORS headers
/// (browser will block the response; native apps don't care).
pub fn allowed_cors_origin(
    request_origin: Option<&str>,
    allowlist: &[String],
) -> Option<String> {
    let origin = request_origin?.trim();
    if origin.is_empty() || allowlist.is_empty() {
        return None;
    }
    if allowlist.iter().any(|allowed| allowed == origin) {
        Some(origin.to_string())
    } else {
        None
    }
}

/// Send a JSON response. CORS headers are emitted only if `cors_origin` is
/// Some (caller resolved via `allowed_cors_origin`). Other hardening headers
/// (X-Frame-Options, X-Content-Type-Options, Referrer-Policy) are always set
/// since the API serves JSON and is never meant to be framed.
pub fn send_response(
    stream: &mut TcpStream,
    status: u16,
    body: &serde_json::Value,
    cors_origin: Option<&str>,
) {
    let body_str = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        _ => "Error",
    };

    let mut headers = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         X-Frame-Options: DENY\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n",
        status, status_text, body_str.len()
    );

    if let Some(origin) = cors_origin {
        // Reflect the validated origin. `Vary: Origin` tells caches this
        // response is origin-specific.
        headers.push_str(&format!(
            "Access-Control-Allow-Origin: {}\r\n\
             Access-Control-Allow-Headers: Authorization, Content-Type\r\n\
             Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
             Access-Control-Allow-Credentials: true\r\n\
             Vary: Origin\r\n",
            origin
        ));
    }

    headers.push_str("\r\n");
    headers.push_str(&body_str);
    let _ = stream.write_all(headers.as_bytes());
}

/// Forward a validated companion request to the internal K2SO HTTP server.
/// Converts POST JSON bodies to GET query params for the internal server.
pub fn proxy_to_internal(
    state: &CompanionState,
    method: &str,
    path: &str,
    _headers: &HashMap<String, String>,
    request: &str,
    project: &str,
) -> Result<serde_json::Value, String> {
    let internal_route = map_route(method, path)
        .ok_or_else(|| "Unknown companion endpoint".to_string())?;

    // Build query params: start with token + project
    let mut params = vec![
        format!("token={}", urlencode(&state.hook_token)),
        format!("project={}", urlencode(project)),
    ];

    // Forward query params from the companion request
    let query_params = parse_query(path);
    for (k, v) in &query_params {
        if k != "token" && k != "project" {
            params.push(format!("{}={}", urlencode(k), urlencode(v)));
        }
    }

    // For POST requests, convert JSON body fields to query params
    if method == "POST" {
        if let Some(body) = parse_json_body(request) {
            if let Some(obj) = body.as_object() {
                for (k, v) in obj {
                    let val = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    params.push(format!("{}={}", urlencode(k), urlencode(&val)));
                }
            }
        }
    }

    let url = format!(
        "http://127.0.0.1:{}{}?{}",
        state.hook_port, internal_route, params.join("&")
    );

    // Forward to internal server
    let resp = reqwest::blocking::get(&url)
        .map_err(|e| format!("Internal proxy failed: {}", e))?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();

    // Parse internal response and wrap in companion envelope
    if status.is_success() {
        let data: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::Value::String(text));
        Ok(serde_json::json!({"ok": true, "data": data}))
    } else {
        let error: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::json!({"error": text}));
        let msg = error.get("error").and_then(|e| e.as_str()).unwrap_or("Internal error");
        Err(msg.to_string())
    }
}

/// Handle a companion HTTP request: auth check, route, proxy.
pub fn handle_request(
    stream: &mut TcpStream,
    state: &CompanionState,
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    request: &str,
    remote_addr: &str,
) {
    let clean_path = path.split('?').next().unwrap_or("");
    let request_origin = headers.get("origin").map(|s| s.as_str());
    let cors = allowed_cors_origin(request_origin, &state.cors_origins);
    let cors_ref = cors.as_deref();

    // DNS rebinding defense: reject any Host header that isn't the tunnel
    // URL, loopback, or an operator-configured allowlist entry.
    let request_host = headers.get("host").map(|s| s.as_str());
    let tunnel_snapshot: Option<String> = state.tunnel_url.lock().clone();
    if !host_allowed(request_host, tunnel_snapshot.as_deref(), &state.cors_origins) {
        log_debug!(
            "[companion] Rejected request — Host {:?} not allowed",
            request_host
        );
        send_response(
            stream,
            403,
            &serde_json::json!({
                "ok": false,
                "error": "Host not allowed"
            }),
            cors_ref,
        );
        return;
    }

    // OPTIONS (CORS preflight)
    if method == "OPTIONS" {
        send_response(stream, 200, &serde_json::json!({"ok": true}), cors_ref);
        return;
    }

    // Auth endpoint — no Bearer token needed, uses Basic Auth
    if clean_path == "/companion/auth" && method == "POST" {
        handle_auth(stream, state, headers, remote_addr, cors_ref);
        return;
    }

    // Logout — delete the caller's own session. Requires the Bearer token
    // being revoked; we don't validate age/rate limit here since revocation
    // is strictly safety-net — always fine to purge.
    if clean_path == "/companion/auth/revoke" && method == "POST" {
        handle_revoke(stream, state, headers, cors_ref);
        return;
    }

    // WebSocket upgrade — handled separately
    if clean_path == "/companion/ws" {
        send_response(
            stream,
            400,
            &serde_json::json!({"ok": false, "error": "WebSocket upgrade must use GET with Upgrade header"}),
            cors_ref,
        );
        return;
    }

    // All other routes require Bearer token
    let auth_header = headers.get("authorization").cloned().unwrap_or_default();
    let token = match auth::parse_bearer(&auth_header) {
        Some(t) => t,
        None => {
            send_response(
                stream,
                401,
                &serde_json::json!({"ok": false, "error": "Missing Authorization: Bearer <token>"}),
                cors_ref,
            );
            return;
        }
    };

    // Validate session + rate limit
    if let Err(msg) = auth::validate_bearer(&token, state) {
        let status = if msg.contains("Rate limit") { 429 } else { 401 };
        send_response(
            stream,
            status,
            &serde_json::json!({"ok": false, "error": msg}),
            cors_ref,
        );
        return;
    }

    // Gate arbitrary-command endpoints unless the operator has explicitly
    // opted in. Default-off limits blast radius if a bearer token is stolen.
    if is_privileged_spawn_path(clean_path) && !state.allow_remote_spawn {
        send_response(
            stream,
            403,
            &serde_json::json!({
                "ok": false,
                "error": "Remote terminal spawn is disabled. Enable 'Allow remote spawn' in Companion settings and restart the tunnel to permit this endpoint.",
            }),
            cors_ref,
        );
        return;
    }

    // Extract project from query params (companion clients must specify which workspace)
    let query = parse_query(path);
    let project = query.get("project").cloned().unwrap_or_default();

    // Proxy to internal server
    match proxy_to_internal(state, method, path, headers, request, &project) {
        Ok(data) => send_response(stream, 200, &data, cors_ref),
        Err(e) => send_response(
            stream,
            400,
            &serde_json::json!({"ok": false, "error": e}),
            cors_ref,
        ),
    }
}

/// Handle POST /companion/auth — Basic Auth login.
fn handle_auth(
    stream: &mut TcpStream,
    state: &CompanionState,
    headers: &HashMap<String, String>,
    remote_addr: &str,
    cors: Option<&str>,
) {
    // Per-IP rate limit — mitigates password brute-force over the tunnel.
    let client = client_ip(headers, remote_addr);
    let limiter_decision = state.auth_limiter.lock().check_and_record(client);
    if let Err(retry_after) = limiter_decision {
        log_debug!(
            "[companion] Auth rate-limited: ip={} retry_after={}s",
            client,
            retry_after
        );
        send_response(
            stream,
            429,
            &serde_json::json!({
                "ok": false,
                "error": "Too many auth attempts — retry later",
                "retryAfterSeconds": retry_after,
            }),
            cors,
        );
        return;
    }

    let auth_header = headers.get("authorization").cloned().unwrap_or_default();
    let (username, password) = match auth::parse_basic_auth(&auth_header) {
        Some(creds) => creds,
        None => {
            send_response(
                stream,
                401,
                &serde_json::json!({"ok": false, "error": "Missing Authorization: Basic <credentials>"}),
                cors,
            );
            return;
        }
    };

    // Read settings (via the core-side settings bridge; the Tauri app
    // registers the provider at startup) to validate the username.
    let snap = super::settings_bridge::read_settings();
    if username != snap.username {
        send_response(
            stream,
            401,
            &serde_json::json!({"ok": false, "error": "Invalid credentials"}),
            cors,
        );
        return;
    }

    // Load hash from Keychain (preferred) with lazy migration of any legacy
    // on-disk hash. Returns None if no password is configured.
    let hash = match auth::load_password_hash() {
        Some(h) => h,
        None => {
            send_response(
                stream,
                401,
                &serde_json::json!({"ok": false, "error": "Invalid credentials"}),
                cors,
            );
            return;
        }
    };
    if !auth::verify_password(&password, &hash) {
        send_response(
            stream,
            401,
            &serde_json::json!({"ok": false, "error": "Invalid credentials"}),
            cors,
        );
        return;
    }

    // Create session
    let session = auth::create_session(remote_addr);
    let token = session.token.clone();
    let expires_at = session.expires_at.to_rfc3339();

    state.sessions.lock().insert(token.clone(), session);

    send_response(
        stream,
        200,
        &serde_json::json!({
            "ok": true,
            "data": {
                "token": token,
                "expiresAt": expires_at,
            }
        }),
        cors,
    );
}

/// Handle POST /companion/auth/revoke — delete the caller's session.
///
/// Uses constant-time compare (matching `validate_bearer`) to find and purge
/// the session without leaking which-bucket timing. Idempotent: a missing
/// token still returns 200 so callers can safely retry. Returns 400 only if
/// the Authorization header is malformed.
fn handle_revoke(
    stream: &mut TcpStream,
    state: &CompanionState,
    headers: &HashMap<String, String>,
    cors: Option<&str>,
) {
    use subtle::ConstantTimeEq;
    let auth_header = headers.get("authorization").cloned().unwrap_or_default();
    let token = match auth::parse_bearer(&auth_header) {
        Some(t) => t,
        None => {
            send_response(
                stream,
                400,
                &serde_json::json!({
                    "ok": false,
                    "error": "Missing Authorization: Bearer <token>"
                }),
                cors,
            );
            return;
        }
    };

    let mut sessions = state.sessions.lock();
    let mut matched: Option<String> = None;
    for key in sessions.keys() {
        if key.as_bytes().ct_eq(token.as_bytes()).into() {
            matched = Some(key.clone());
            break;
        }
    }
    if let Some(k) = matched {
        sessions.remove(&k);
    }
    drop(sessions);

    // Also disconnect any WS clients bound to this token.
    {
        let mut clients = state.ws_clients.lock();
        clients.retain(|c| {
            let matches: bool = c.session_token.as_bytes().ct_eq(token.as_bytes()).into();
            !matches
        });
    }

    send_response(stream, 200, &serde_json::json!({"ok": true}), cors);
}

#[cfg(test)]
mod security_tests {
    use super::*;

    // ── CORS ──

    #[test]
    fn cors_empty_allowlist_returns_none() {
        assert!(allowed_cors_origin(Some("https://example.com"), &[]).is_none());
    }

    #[test]
    fn cors_missing_origin_returns_none() {
        let allowlist = vec!["https://example.com".to_string()];
        assert!(allowed_cors_origin(None, &allowlist).is_none());
    }

    #[test]
    fn cors_exact_match_reflects_origin() {
        let allowlist = vec!["https://companion.example.com".to_string()];
        assert_eq!(
            allowed_cors_origin(Some("https://companion.example.com"), &allowlist),
            Some("https://companion.example.com".to_string())
        );
    }

    #[test]
    fn cors_mismatched_origin_returns_none() {
        let allowlist = vec!["https://companion.example.com".to_string()];
        assert!(
            allowed_cors_origin(Some("https://evil.example.com"), &allowlist).is_none()
        );
    }

    // ── WS Origin ──

    #[test]
    fn ws_missing_origin_allowed() {
        // Native mobile clients don't send Origin.
        assert!(ws_origin_allowed(None, Some("https://x.ngrok.app"), &[]));
    }

    #[test]
    fn ws_empty_origin_allowed() {
        assert!(ws_origin_allowed(Some(""), Some("https://x.ngrok.app"), &[]));
    }

    #[test]
    fn ws_tunnel_origin_allowed() {
        assert!(ws_origin_allowed(
            Some("https://x.ngrok.app"),
            Some("https://x.ngrok.app"),
            &[],
        ));
    }

    #[test]
    fn ws_tunnel_with_trailing_slash_normalized() {
        assert!(ws_origin_allowed(
            Some("https://x.ngrok.app/"),
            Some("https://x.ngrok.app"),
            &[],
        ));
    }

    #[test]
    fn ws_allowlisted_origin_allowed() {
        let allow = vec!["https://companion.example.com".to_string()];
        assert!(ws_origin_allowed(
            Some("https://companion.example.com"),
            None,
            &allow,
        ));
    }

    #[test]
    fn ws_loopback_always_allowed() {
        assert!(ws_origin_allowed(Some("http://127.0.0.1:3000"), None, &[]));
        assert!(ws_origin_allowed(Some("http://localhost:8080"), None, &[]));
        assert!(ws_origin_allowed(Some("http://[::1]:9000"), None, &[]));
    }

    #[test]
    fn ws_hostile_origin_rejected() {
        let allow = vec!["https://companion.example.com".to_string()];
        assert!(!ws_origin_allowed(
            Some("https://evil.example.com"),
            Some("https://x.ngrok.app"),
            &allow,
        ));
    }

    #[test]
    fn ws_near_miss_subdomain_rejected() {
        // Guard against naive substring match — a.example.com must not match example.com
        assert!(!ws_origin_allowed(
            Some("https://attacker-example.com"),
            Some("https://example.com"),
            &[],
        ));
    }

    // ── Host header (DNS rebinding defense) ──

    #[test]
    fn host_missing_rejected() {
        assert!(!host_allowed(None, Some("https://x.ngrok.app"), &[]));
        assert!(!host_allowed(Some(""), Some("https://x.ngrok.app"), &[]));
    }

    #[test]
    fn host_loopback_allowed() {
        assert!(host_allowed(Some("127.0.0.1:3000"), None, &[]));
        assert!(host_allowed(Some("localhost:8080"), None, &[]));
        assert!(host_allowed(Some("127.0.0.1"), None, &[]));
    }

    #[test]
    fn host_tunnel_allowed() {
        assert!(host_allowed(
            Some("x.ngrok.app"),
            Some("https://x.ngrok.app"),
            &[],
        ));
    }

    #[test]
    fn host_rebinding_attacker_rejected() {
        // Attacker points evil.com DNS at ngrok IP. Host header carries evil.com.
        assert!(!host_allowed(
            Some("evil.com"),
            Some("https://x.ngrok.app"),
            &[],
        ));
    }

    #[test]
    fn host_allowlist_entry_allowed() {
        let allow = vec!["https://companion.example.com".to_string()];
        assert!(host_allowed(
            Some("companion.example.com"),
            None,
            &allow,
        ));
    }

    #[test]
    fn host_port_stripped_for_comparison() {
        assert!(host_allowed(
            Some("x.ngrok.app:443"),
            Some("https://x.ngrok.app"),
            &[],
        ));
    }

    // ── client_ip extraction ──

    #[test]
    fn client_ip_prefers_xff_first_value() {
        let mut headers = HashMap::new();
        headers.insert(
            "x-forwarded-for".to_string(),
            "203.0.113.42, 10.0.0.1".to_string(),
        );
        assert_eq!(
            client_ip(&headers, "127.0.0.1:54321"),
            "203.0.113.42".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_falls_back_to_peer() {
        let headers = HashMap::new();
        assert_eq!(
            client_ip(&headers, "127.0.0.1:54321"),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        );
    }

    #[test]
    fn client_ip_handles_ipv6_peer() {
        let headers = HashMap::new();
        // IPv6 peer_addr format is [::1]:port
        assert_eq!(
            client_ip(&headers, "[::1]:54321"),
            "::1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_ignores_malformed_xff() {
        let mut headers = HashMap::new();
        headers.insert("x-forwarded-for".to_string(), "not-an-ip".to_string());
        assert_eq!(
            client_ip(&headers, "127.0.0.1:54321"),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        );
    }

    // ── Rate limiter ──

    #[test]
    fn rate_limiter_allows_under_threshold() {
        use super::super::types::AuthRateLimiter;
        let mut limiter = AuthRateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
        for _ in 0..AuthRateLimiter::MINUTE_LIMIT {
            assert!(limiter.check_and_record(ip).is_ok());
        }
    }

    #[test]
    fn rate_limiter_blocks_over_threshold() {
        use super::super::types::AuthRateLimiter;
        let mut limiter = AuthRateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
        for _ in 0..AuthRateLimiter::MINUTE_LIMIT {
            let _ = limiter.check_and_record(ip);
        }
        // Next attempt must be blocked.
        assert!(limiter.check_and_record(ip).is_err());
    }

    #[test]
    fn rate_limiter_is_per_ip() {
        use super::super::types::AuthRateLimiter;
        let mut limiter = AuthRateLimiter::new();
        let ip_a = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
        let ip_b = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 2));
        for _ in 0..AuthRateLimiter::MINUTE_LIMIT {
            let _ = limiter.check_and_record(ip_a);
        }
        assert!(limiter.check_and_record(ip_a).is_err());
        // Different IP must not share the budget.
        assert!(limiter.check_and_record(ip_b).is_ok());
    }
}
