//! HTTP framing helpers shared by the daemon's route dispatcher.
//!
//! Extracted from `main.rs` during the 0.39.0 route refactor.
//! Everything here is plumbing — token parsing, query parsing, response
//! writing, the POST-only method gate, the OPTIONS preflight responder,
//! body reading — that the dispatcher in `routes::dispatcher` invokes on
//! every connection.
//!
//! These helpers were previously `fn` in `main.rs` (binary-private);
//! moving them under `routes/` keeps `main.rs` focused on boot +
//! migrations + watchdog.

use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Constant-time equality for the OWNER token compare — the crown-jewel
/// credential. A plain `==` on `&str` short-circuits on the first byte that
/// differs, so an attacker measuring response time could recover the token
/// one byte at a time. `subtle::ConstantTimeEq` compares every byte
/// regardless. Unequal lengths can't be equal (and length is not secret —
/// the owner token is a fixed-width hex string), so we short-circuit only
/// on length, then do the data compare in constant time.
///
/// Every owner-token check in the daemon (`token_is_owner`, the owner arm of
/// `token_ok`/`token_still_valid`/`actor_role`, and the `state.token`
/// compares in the dispatcher) routes through here.
pub(crate) fn ct_eq_token(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.ct_eq(b).into()
}

/// Extract the raw `token=<value>` from a URL-encoded query string. No
/// full urlencoded decoding — the token is always hex so there's nothing
/// to decode. Returns the first `token=` value seen (the contract callers
/// depend on when a malformed query duplicates the key).
pub(crate) fn extract_token(query: &str) -> Option<&str> {
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("token=") {
            return Some(v);
        }
    }
    None
}

/// COMPAT-58: extract the `Authorization: Bearer <token>` credential from a
/// request-headers blob (the peeked request head, up through `\r\n\r\n`).
///
/// The scoped per-cell hook channel (#58) authenticates with a Bearer
/// HEADER, not `?token=`, so the credential never lands in the query string
/// — which would be captured in the daemon's recorded transcript AND any
/// access log (PRD §3.4). This is tried FIRST at the `/hook/complete` call
/// site; [`extract_token`] (`?token=`) remains the back-compat fallback, so
/// every existing query-param caller is unchanged.
///
/// The least-invasive integration choice (per the issue brief): a SIBLING
/// helper tried at the call site, rather than threading the headers blob
/// into [`extract_token`]'s `&str query` signature — which is shared by
/// `token_ok` / `token_is_owner` / `actor_role` and would ripple through
/// every owner-token caller for zero Phase-0 benefit.
///
/// Field name matched case-insensitively (RFC 9110 §5.6); the `Bearer`
/// scheme token likewise; the credential itself is case-SENSITIVE.
pub(crate) fn extract_bearer(headers_blob: &str) -> Option<&str> {
    for line in headers_blob.lines() {
        let Some(colon) = line.find(':') else { continue };
        let (name, rest) = line.split_at(colon);
        if !name.eq_ignore_ascii_case("authorization") {
            continue;
        }
        // Drop the ':' then split "<scheme> <credential>" on the first run
        // of whitespace.
        let value = rest[1..].trim();
        let mut parts = value.splitn(2, char::is_whitespace);
        let Some(scheme) = parts.next() else { continue };
        if !scheme.eq_ignore_ascii_case("bearer") {
            continue;
        }
        let Some(cred) = parts.next() else { continue };
        let cred = cred.trim();
        if cred.is_empty() {
            continue;
        }
        return Some(cred);
    }
    None
}

/// True iff the request's `?token=` is the OWNER token (the local daemon
/// token, `state.token`). This is the STRICT gate — owner-only routes
/// (user management + tunnel control) MUST use this, NOT [`token_ok`],
/// so an authenticated connect-user can't escalate.
///
/// K2SO #617: connect-user sessions deliberately do NOT satisfy this.
pub(crate) fn token_is_owner(query: &str, owner_token: &str) -> bool {
    extract_token(query).is_some_and(|tok| ct_eq_token(tok, owner_token))
}

/// The shared authorization gate for the bulk of `/cli/*` routes.
///
/// K2SO #617: a request is authorized when its `?token=` is EITHER the
/// OWNER token (`owner_token`, == `state.token`) OR a live connect-user
/// session token (validated in-memory by
/// [`k2_core::connect_users::validate_session`]). This is what lets a
/// remote connect-user — who logged in over the public tunnel with a
/// username+password and holds a session token — actually USE the daemon.
///
/// Owner-only routes (ALL `/cli/users/*` + tunnel start/stop/config-POST)
/// must NOT use this; they use [`token_is_owner`] so a connect-user is
/// barred from user management + tunnel control.
pub(crate) fn token_ok(query: &str, owner_token: &str) -> bool {
    let Some(tok) = extract_token(query) else {
        return false;
    };
    // Empty token never authorizes (matches pre-#617 behavior: an empty
    // value must not slip through against a non-empty owner token, and an
    // empty session token is never issued).
    if tok.is_empty() {
        return false;
    }
    if ct_eq_token(tok, owner_token) {
        return true;
    }
    k2_core::connect_users::validate_session(tok).is_some()
}

/// Re-validate an ALREADY-EXTRACTED token mid-connection — the long-lived
/// WS loops (`/cli/sessions/grid`, `/cli/sessions/events`) call this on a
/// timer so a connect-user who is disabled/removed/role-changed (which
/// `revoke_user_sessions` drops from the in-memory session map) is kicked
/// off their LIVE socket, not just blocked on the next new request. The
/// owner token is never revoked, so it always re-validates. Empty never
/// authorizes (mirrors [`token_ok`]).
pub(crate) fn token_still_valid(token: &str, owner_token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if ct_eq_token(token, owner_token) {
        return true;
    }
    k2_core::connect_users::validate_session(token).is_some()
}

/// Resolve the effective permission [`Role`](k2_core::connect_users::Role)
/// for a request's `?token=` (K2SO #629).
///
/// - The OWNER token (`owner_token`, == `state.token`) ALWAYS maps to
///   `Role::Owner` — the host machine's owner is the top authority and that
///   authority lives in the daemon token, not any stored row.
/// - A live connect-user session token resolves to that user's stored role.
/// - Anything else (empty, unknown, expired) → `None` (unauthorized).
pub(crate) fn actor_role(
    query: &str,
    owner_token: &str,
) -> Option<k2_core::connect_users::Role> {
    let tok = extract_token(query)?;
    if tok.is_empty() {
        return None;
    }
    if ct_eq_token(tok, owner_token) {
        return Some(k2_core::connect_users::Role::Owner);
    }
    k2_core::connect_users::role_for_session(tok)
}

/// OWNER-or-manager authorization guard (K2SO #629). Returns the actor's
/// resolved [`Role`](k2_core::connect_users::Role) when the request is
/// authorized to MANAGE users (owner token OR a session whose user
/// `can_manage_users` — i.e. Admin or Owner). On rejection it drains the
/// peeked request and sends `403 Forbidden`, returning `None`; the caller
/// MUST early-return.
///
/// This replaces the bare [`require_owner`] on the management routes so an
/// Admin connect-user can add/remove/enable/disable users, while a Member
/// (or an unknown token) is still barred.
pub(crate) async fn require_manage(
    stream: &mut TcpStream,
    buf: &mut [u8],
    query: &str,
    owner_token: &str,
) -> Option<k2_core::connect_users::Role> {
    match actor_role(query, owner_token) {
        Some(role) if k2_core::connect_users::can_manage_users(role) => Some(role),
        _ => {
            let _ = stream.read(buf).await;
            send_response(
                stream,
                "403 Forbidden",
                "application/json",
                r#"{"error":"invalid or missing token"}"#,
            )
            .await;
            None
        }
    }
}

/// Parse an `application/x-www-form-urlencoded` POST body into a params
/// map, reusing the same URL-decoding parser the query string goes
/// through. Returns an empty map for empty, non-UTF-8, or JSON bodies
/// (JSON-bodied routes have their own typed parsers — this helper is
/// only for the form-bodied routes that moved long values out of the
/// query string in 0.39.45, see GH #35/#37/#29).
pub(crate) fn parse_form_body(body: &[u8]) -> std::collections::HashMap<String, String> {
    let Ok(s) = std::str::from_utf8(body) else {
        return std::collections::HashMap::new();
    };
    let s = s.trim();
    if s.is_empty() || s.starts_with('{') || s.starts_with('[') {
        return std::collections::HashMap::new();
    }
    k2_core::agent_hooks::parse_query_params(&format!("/x?{}", s))
}

/// Reassemble a full `path?query` URL and hand off to k2_core's
/// URL-decoding query parser. The core helper knows how to unescape
/// `%20`/`+` and multi-byte UTF-8 — we just combine the pieces.
pub(crate) fn parse_params(
    path: &str,
    query: &str,
) -> std::collections::HashMap<String, String> {
    let path_and_query = if query.is_empty() {
        path.to_string()
    } else {
        format!("{}?{}", path, query)
    };
    k2_core::agent_hooks::parse_query_params(&path_and_query)
}

/// Extract the project directory from query params. Accepts BOTH
/// `project=<path>` (the short form src-tauri's agent_hooks server
/// uses and the k2so CLI sends) and `project_path=<path>` (the long
/// form earlier daemon routes adopted). Empty values are treated the
/// same as missing.
#[allow(dead_code)] // covered by inline tests; reserved for future
                    // handlers that prefer the fallback over inline
                    // `need_project` lookups in `cli.rs`.
pub(crate) fn project_param(
    params: &std::collections::HashMap<String, String>,
) -> Option<String> {
    for key in &["project_path", "project"] {
        if let Some(v) = params.get(*key) {
            if !v.is_empty() {
                return Some(v.clone());
            }
        }
    }
    None
}

/// Write a single HTTP response with the canonical header set the
/// daemon emits on every route — permissive CORS for the Tauri
/// WebView, content-length, the supplied status and content-type.
///
/// **0.39.7:** This function no longer emits `Connection: close`.
/// HTTP/1.1's default is keep-alive; the dispatcher loops on the same
/// `TcpStream` to serve subsequent requests, so we let the connection
/// stay open and rely on the client-supplied `Connection: close`
/// header (or the dispatcher's idle-timeout) to tear it down.
///
/// Why this matters: macOS's WKWebView Networking process has a
/// soft `RLIMIT_NOFILE = 256`. Pre-0.39.7 every renderer fetch was a
/// fresh TCP socket because of the forced close; WKWebView's slow
/// fd cleanup left a steady stream of `CLOSE_WAIT` sockets that
/// progressively filled the table and locked up the UI after ~50min
/// of normal use. See issue #2 + `release-notes-0.39.7.md`.
pub(crate) async fn send_response(stream: &mut TcpStream, status: &str, ct: &str, body: &str) {
    // CORS headers on every response so the Tauri WebView (cross-
    // origin from tauri://localhost or http://localhost:5173 to
    // http://127.0.0.1:<port>) can read the body. Token auth
    // gates every real request so permissive origin adds no risk.
    let resp = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {ct}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Expose-Headers: *\r\n\r\n{}",
        body.len(),
        body,
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// Respond to a CORS preflight (OPTIONS) with permissive headers so
/// the WebView accepts the subsequent GET/POST. 204 No Content is
/// the conventional preflight response status.
///
/// **0.39.7:** as with [`send_response`], no longer emits
/// `Connection: close`. The same TCP connection is then reused for
/// the actual GET/POST that the preflight authorized — saves a
/// socket per write-side operation.
pub(crate) async fn send_cors_preflight(stream: &mut TcpStream) {
    let resp = "HTTP/1.1 204 No Content\r\n\
        Access-Control-Allow-Origin: *\r\n\
        Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
        Access-Control-Allow-Headers: Content-Type, Authorization\r\n\
        Access-Control-Max-Age: 600\r\n\
        Content-Length: 0\r\n\r\n";
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// Parse a `Connection:` header value from a request-headers blob
/// (the first chunk of the request, up through `\r\n\r\n`) and
/// return `true` iff it explicitly requests `close`.
///
/// 0.39.7: the dispatcher uses this to decide whether to break out
/// of the keep-alive loop after responding. HTTP/1.1's default is
/// keep-alive, so absence of the header (or any non-`close` value)
/// means "keep the socket open." Comparison is ASCII-case-insensitive
/// per RFC 9110 §5.6.
pub(crate) fn request_wants_close(headers_blob: &str) -> bool {
    for line in headers_blob.lines() {
        // Header lines are `Field-Name: value` with optional folding
        // (which RFC 7230 deprecated; we don't handle it). Locate the
        // colon, lowercase the field name in-place via ASCII.
        let Some(colon) = line.find(':') else { continue };
        let name = &line[..colon];
        if !name.eq_ignore_ascii_case("connection") {
            continue;
        }
        let value = line[colon + 1..].trim();
        // `Connection` is comma-separated tokens (e.g.
        // `keep-alive, Upgrade`). Match `close` token-wise.
        for tok in value.split(',') {
            if tok.trim().eq_ignore_ascii_case("close") {
                return true;
            }
        }
    }
    false
}

/// Return `true` iff the request advertises a `Transfer-Encoding` whose
/// final coding is `chunked` (RFC 9112 §6.1).
///
/// H5 (E2E splice hardening): the daemon's body reader
/// ([`read_post_body`]) is **Content-Length-driven only** — it does not
/// decode chunked framing. Accepting a chunked POST would leave the
/// chunk size-lines + trailers unconsumed in the keep-alive stream, and
/// the dispatcher would mis-parse those bytes as the next request
/// (request smuggling / keep-alive desync). The dispatcher rejects any
/// request for which this returns `true` with a 400 + connection close.
///
/// Per RFC 9112 a `Transfer-Encoding` header makes the message chunked
/// when `chunked` is the final encoding; in practice clients only ever
/// send the bare `chunked` token, but we match it token-wise to be safe
/// and case-insensitively per RFC 9110 §5.6.
pub(crate) fn request_is_chunked(headers_blob: &str) -> bool {
    for line in headers_blob.lines() {
        let Some(colon) = line.find(':') else { continue };
        let name = &line[..colon];
        if !name.eq_ignore_ascii_case("transfer-encoding") {
            continue;
        }
        let value = line[colon + 1..].trim();
        // `Transfer-Encoding` is a comma-separated list; chunked, when
        // present, is the data-framing coding. Treat ANY `chunked` token
        // as "we can't frame this" — we refuse rather than try to decode.
        for tok in value.split(',') {
            if tok.trim().eq_ignore_ascii_case("chunked") {
                return true;
            }
        }
    }
    false
}

/// Method-gate guard for POST-only `/cli/*` routes.
///
/// Per the [`feedback_post_only_route_guards`] memory: every mutating
/// `/cli/*` route must reject non-POST methods at the handler top, NOT
/// rely on the top-level dispatch's GET/POST allowlist — that allowlist
/// only blocks methods like PUT/DELETE/HEAD, it does NOT block GET on
/// POST-allowlisted routes. Without an explicit per-handler gate, a
/// `curl http://127.0.0.1:<port>/cli/dangerous-route?token=X` GET would
/// silently trigger the mutation.
///
/// Returns `true` when the request is a POST and the caller should
/// continue; returns `false` after sending a `405 Method Not Allowed`
/// response, in which case the caller MUST early-return without touching
/// the stream further. The peeked request bytes are drained on rejection
/// so the response actually goes out.
///
/// Usage:
///
/// ```ignore
/// if !require_post(&mut stream, &mut buf, is_post).await { return; }
/// ```
pub(crate) async fn require_post(stream: &mut TcpStream, buf: &mut [u8], is_post: bool) -> bool {
    if is_post {
        return true;
    }
    let _ = stream.read(buf).await;
    send_response(
        stream,
        "405 Method Not Allowed",
        "application/json",
        r#"{"error":"POST required"}"#,
    )
    .await;
    false
}

/// OWNER-only authorization guard for the strict `/cli/*` routes (user
/// management + tunnel control). Returns `true` when the request's
/// `?token=` is the owner token and the caller should continue; returns
/// `false` after draining the peeked request and sending `403 Forbidden`,
/// in which case the caller MUST early-return.
///
/// K2SO #617: this is deliberately distinct from the [`token_ok`] gate
/// used by the rest of `/cli/*`. A connect-user's session token passes
/// `token_ok` (general daemon access) but fails this — so a connect-user
/// can USE the daemon yet cannot manage users or control the host's
/// tunnel. The 403 body matches the shared shape every other gate emits
/// so clients key off one error string.
pub(crate) async fn require_owner(
    stream: &mut TcpStream,
    buf: &mut [u8],
    query: &str,
    owner_token: &str,
) -> bool {
    if token_is_owner(query, owner_token) {
        return true;
    }
    let _ = stream.read(buf).await;
    send_response(
        stream,
        "403 Forbidden",
        "application/json",
        r#"{"error":"invalid or missing token"}"#,
    )
    .await;
    false
}

/// OWNER-or-ADMIN authorization guard (K2SO #660). Returns `true` when the
/// request is authorized to perform a privileged HOST-level operation —
/// either the owner token is presented, OR a live connect-user session whose
/// role is `Owner` or `Admin` (`can_manage_users`). A `Member` session (or an
/// unknown/missing/empty token) is rejected with `403 Forbidden`, after which
/// the caller MUST early-return.
///
/// Why this is distinct from [`require_owner`]: restarting the host over K2
/// Connect can ONLY be authorized with a connect-user SESSION token (the
/// remote user never holds the on-box owner token). [`require_owner`] would
/// 403 that legitimate request. This guard broadens authorization to the same
/// owner-or-admin tier [`require_manage`] uses for user management, but in a
/// boolean (non-role-returning) shape that mirrors [`require_owner`] so the
/// restart arm reads as a single drop-in swap. Exactly ONE response is
/// written: success returns without touching the stream, rejection drains the
/// peeked request and sends ONE `403`.
pub(crate) async fn require_owner_or_admin(
    stream: &mut TcpStream,
    buf: &mut [u8],
    query: &str,
    owner_token: &str,
) -> bool {
    match actor_role(query, owner_token) {
        Some(role) if k2_core::connect_users::can_manage_users(role) => true,
        _ => {
            let _ = stream.read(buf).await;
            send_response(
                stream,
                "403 Forbidden",
                "application/json",
                r#"{"error":"invalid or missing token"}"#,
            )
            .await;
            false
        }
    }
}

/// Composer 1c (D4) — the capability decision for the session-scoped
/// `POST /cli/terminal/send-message` route.
///
/// This route instructs an agent running `--dangerously-skip-permissions`
/// (= full shell + fs RCE), so the gate is server-enforced and fails
/// CLOSED. Three outcomes:
///
/// - [`SendMessageAuth::Owner`] — the request carries the OWNER token. The
///   owner is ALWAYS allowed (unchanged from 1a), independent of the opt-in
///   flag. The caller resolves the attributed `from` server-side via
///   `workspace_msg::resolve_owner_from()`.
/// - [`SendMessageAuth::ConnectUser`] — a live connect-user session whose
///   role is `>= Member` (the inert capability check; a future Viewer role
///   below Member slots in here automatically, D4) AND the host has opted
///   into remote multi-user instruction (`remote_instruct_opt_in`, default
///   OFF). Carries the daemon-resolved `username` — the `from` attribution
///   (D3: resolved from the token, NEVER the request body).
/// - [`SendMessageAuth::Denied`] — anything else (missing/unknown/expired
///   token, opt-in OFF, role below Member). The caller MUST drain-then-403
///   (mirror [`require_manage`]).
///
/// A revoked connect-user fails here at request time (`validate_session`
/// returns `None` once `revoke_user_sessions` drops the in-memory entry);
/// the dispatcher re-validates again at inject time for the mid-flight
/// revocation window (M2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SendMessageAuth {
    /// Owner token — always allowed.
    Owner,
    /// Connect-user (role >= Member) on an opted-in host. Carries the
    /// daemon-resolved username used as the `from` attribution.
    ConnectUser { username: String },
    /// Reject — caller drains the body then sends 403.
    Denied,
}

/// Decide whether a `send-message` request is authorized (Composer 1c, D4).
///
/// `remote_instruct_opt_in` is the host's `app_settings.allow_remote_instruct`
/// flag (default OFF); it gates ONLY the connect-user path. The owner token
/// is allowed regardless. See [`SendMessageAuth`].
pub(crate) fn authorize_send_message(
    query: &str,
    owner_token: &str,
    remote_instruct_opt_in: bool,
) -> SendMessageAuth {
    // Owner is always allowed — independent of the opt-in flag (1a parity).
    if token_is_owner(query, owner_token) {
        return SendMessageAuth::Owner;
    }
    // Connect-user path: the host must have opted in, AND the actor's role
    // must clear the capability floor (>= Member). Both fail CLOSED.
    if !remote_instruct_opt_in {
        return SendMessageAuth::Denied;
    }
    let role_ok = actor_role(query, owner_token)
        .is_some_and(|r| r >= k2_core::connect_users::Role::Member);
    if !role_ok {
        return SendMessageAuth::Denied;
    }
    // Resolve the username server-side from the token (D3) — never the body.
    // A revoked/expired token returns None here → Denied.
    match extract_token(query).and_then(k2_core::connect_users::validate_session) {
        Some(username) => SendMessageAuth::ConnectUser { username },
        None => SendMessageAuth::Denied,
    }
}

/// Read the body of a POST request. Consumes the request line and
/// headers from the peeked stream, then returns whatever bytes
/// follow the `\r\n\r\n` separator up to the Content-Length header.
///
/// MVP implementation — assumes the full request arrived in the
/// 4KB peek buffer (fine for the single JSON AgentSignal payloads
/// E7 + E8 handle). Production-grade Content-Length-driven
/// streaming is deferred; the largest signal we expect is ~1KB, so
/// 4KB is 4× the headroom.
pub(crate) async fn read_post_body(stream: &mut TcpStream, buf: &mut [u8]) -> Vec<u8> {
    // Phase 4.5: the old single-read version worked with curl (which
    // batches headers + body into one TCP send) but broke with
    // browser fetch, which sends headers in one packet and body in
    // a separate packet. A single `stream.read()` would only return
    // the headers, leaving the body unread — and the JSON parser
    // got "EOF at column 0".
    //
    // Loop until we have the full body: read headers (first chunk),
    // parse Content-Length, then keep reading until we've got that
    // many body bytes or EOF.
    let mut accumulated: Vec<u8> = Vec::new();
    let mut header_end: Option<usize> = None;
    let mut content_length: Option<usize> = None;

    loop {
        // Read into `buf` and append to `accumulated` until headers
        // end is found.
        let n = match stream.read(buf).await {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        accumulated.extend_from_slice(&buf[..n]);

        if header_end.is_none() {
            if let Some(pos) = accumulated
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
            {
                header_end = Some(pos + 4);
                let headers_str =
                    std::str::from_utf8(&accumulated[..pos]).unwrap_or("");
                content_length = headers_str.lines().find_map(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                });
            }
        }

        // Once headers end is known, check if we have the whole body.
        if let (Some(body_start), Some(clen)) = (header_end, content_length) {
            if accumulated.len() >= body_start + clen {
                return accumulated[body_start..body_start + clen].to_vec();
            }
        }
        // Without Content-Length, fall back to "one read gave us
        // everything" heuristic once we've seen the headers.
        if let (Some(body_start), None) = (header_end, content_length) {
            return accumulated[body_start..].to_vec();
        }
    }

    // EOF before we got the full body (or before headers ended).
    // Return whatever we have between header end and EOF; caller's
    // parser will surface a helpful error if it's incomplete.
    if let Some(body_start) = header_end {
        if accumulated.len() > body_start {
            return accumulated[body_start..].to_vec();
        }
    }
    Vec::new()
}

// ─────────────────────────────────────────────────────────────────────
// Inline unit tests — pure-logic helpers that gate every connection
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ct_eq_token: constant-time owner-token compare ──────────────

    #[test]
    fn ct_eq_token_matches_identical() {
        assert!(ct_eq_token("abc123", "abc123"));
    }

    #[test]
    fn ct_eq_token_rejects_differing_same_length() {
        assert!(!ct_eq_token("abc123", "abc124"));
    }

    #[test]
    fn ct_eq_token_rejects_differing_length() {
        // Length mismatch can never be equal (length is not secret).
        assert!(!ct_eq_token("abc", "abc123"));
        assert!(!ct_eq_token("abc123", "abc"));
    }

    #[test]
    fn ct_eq_token_rejects_empty_vs_nonempty() {
        assert!(!ct_eq_token("", "abc123"));
        assert!(!ct_eq_token("abc123", ""));
        // Two empties ARE equal — callers gate empties separately before this.
        assert!(ct_eq_token("", ""));
    }

    #[test]
    fn ct_eq_token_is_case_sensitive() {
        assert!(!ct_eq_token("ABC123", "abc123"));
    }

    // ── token_ok: auth gate ─────────────────────────────────────────

    #[test]
    fn token_ok_accepts_matching_token() {
        assert!(token_ok("token=abc123", "abc123"));
    }

    #[test]
    fn token_ok_rejects_mismatched_token() {
        assert!(!token_ok("token=wrong", "abc123"));
    }

    #[test]
    fn token_ok_rejects_missing_token_param() {
        // Other params present but no `token=` key.
        assert!(!token_ok("project_path=/tmp/foo&name=bar", "abc123"));
    }

    #[test]
    fn token_ok_rejects_empty_query() {
        assert!(!token_ok("", "abc123"));
    }

    #[test]
    fn token_ok_rejects_empty_token_value_when_expected_nonempty() {
        // `token=` with no value should not slip through against a
        // non-empty expected token.
        assert!(!token_ok("token=", "abc123"));
    }

    #[test]
    fn token_ok_finds_token_among_multiple_params() {
        // Token may appear anywhere in the query string — the loop
        // must keep walking past earlier params.
        let q = "project_path=/tmp/foo&name=bar&token=abc123&extra=baz";
        assert!(token_ok(q, "abc123"));
    }

    #[test]
    fn token_ok_first_token_value_wins_when_duplicated() {
        // If `token=` appears twice (malformed query), the first
        // value is what gates the request — that's the current
        // behavior and the contract callers depend on.
        assert!(token_ok("token=first&token=second", "first"));
        assert!(!token_ok("token=first&token=second", "second"));
    }

    #[test]
    fn token_ok_is_case_sensitive() {
        // Tokens are random hex, but if the field name's case ever
        // shifts the auth gate must fail closed (no match means no
        // entry). `Token=` is not equivalent to `token=`.
        assert!(!token_ok("Token=abc123", "abc123"));
    }

    // ── extract_bearer + Bearer-then-query precedence (COMPAT-58) ───

    #[test]
    fn extract_bearer_reads_authorization_header() {
        let head = "POST /hook/complete HTTP/1.1\r\nAuthorization: Bearer abc.def\r\n\r\n";
        assert_eq!(extract_bearer(head), Some("abc.def"));
    }

    #[test]
    fn extract_bearer_scheme_is_case_insensitive_credential_is_not() {
        // Scheme casing must not matter (RFC 9110 §5.6)…
        assert_eq!(
            extract_bearer("GET / HTTP/1.1\r\nAuthorization: bearer Tok123\r\n"),
            Some("Tok123"),
        );
        // …but the credential is returned verbatim (case preserved).
        assert_eq!(
            extract_bearer("GET / HTTP/1.1\r\nAUTHORIZATION: BEARER Tok123\r\n"),
            Some("Tok123"),
        );
    }

    #[test]
    fn extract_bearer_ignores_non_bearer_and_absent() {
        assert_eq!(
            extract_bearer("GET / HTTP/1.1\r\nAuthorization: Basic dXNlcg==\r\n"),
            None,
        );
        assert_eq!(extract_bearer("GET / HTTP/1.1\r\nHost: x\r\n"), None);
        // Bearer with no credential → None (don't authorize an empty token).
        assert_eq!(extract_bearer("GET / HTTP/1.1\r\nAuthorization: Bearer \r\n"), None);
    }

    #[test]
    fn bearer_then_query_precedence_matches_hook_call_site() {
        // The `/hook/complete` scoped arm prefers the Bearer header and
        // falls back to `?token=`. Assert that exact resolution order here
        // so the contract is pinned even though the dispatcher wires it.
        let head_with_bearer =
            "POST /hook/complete?token=QVAL HTTP/1.1\r\nAuthorization: Bearer BVAL\r\n\r\n";
        let head_no_bearer = "POST /hook/complete?token=QVAL HTTP/1.1\r\nHost: x\r\n\r\n";

        let resolve = |head: &str| -> Option<String> {
            extract_bearer(head)
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .or_else(|| extract_token("token=QVAL").map(str::to_string))
        };
        assert_eq!(resolve(head_with_bearer).as_deref(), Some("BVAL"), "Bearer wins");
        assert_eq!(
            resolve(head_no_bearer).as_deref(),
            Some("QVAL"),
            "falls back to ?token= when no Bearer",
        );
    }

    // ── project_param: project_path / project fallback ──────────────

    #[test]
    fn project_param_returns_project_path_when_set() {
        let mut params = std::collections::HashMap::new();
        params.insert("project_path".to_string(), "/tmp/work".to_string());
        assert_eq!(project_param(&params), Some("/tmp/work".to_string()));
    }

    #[test]
    fn project_param_falls_back_to_project_alias() {
        let mut params = std::collections::HashMap::new();
        params.insert("project".to_string(), "/tmp/alias".to_string());
        assert_eq!(project_param(&params), Some("/tmp/alias".to_string()));
    }

    #[test]
    fn project_param_prefers_project_path_over_alias() {
        // Both set → primary key wins.
        let mut params = std::collections::HashMap::new();
        params.insert("project_path".to_string(), "/tmp/primary".to_string());
        params.insert("project".to_string(), "/tmp/alias".to_string());
        assert_eq!(project_param(&params), Some("/tmp/primary".to_string()));
    }

    #[test]
    fn project_param_returns_none_when_neither_set() {
        let params = std::collections::HashMap::new();
        assert_eq!(project_param(&params), None);
    }

    #[test]
    fn project_param_skips_empty_string_value() {
        // An empty value must be treated as "unset" so the fallback
        // alias key gets a chance. If `project_path=` is empty but
        // `project=/tmp/x` is set, callers expect /tmp/x.
        let mut params = std::collections::HashMap::new();
        params.insert("project_path".to_string(), "".to_string());
        params.insert("project".to_string(), "/tmp/x".to_string());
        assert_eq!(project_param(&params), Some("/tmp/x".to_string()));
    }

    // ── parse_params: thin wrapper over core's query parser ─────────

    #[test]
    fn parse_params_extracts_keys_from_query_string() {
        let params = parse_params("/cli/foo", "name=bar&token=xyz");
        assert_eq!(params.get("name").map(String::as_str), Some("bar"));
        assert_eq!(params.get("token").map(String::as_str), Some("xyz"));
    }

    #[test]
    fn parse_params_empty_query_returns_empty_map() {
        let params = parse_params("/cli/foo", "");
        assert!(
            params.is_empty(),
            "expected empty params, got: {params:?}",
        );
    }

    // ── request_wants_close: keep-alive break-out gate ──────────────

    #[test]
    fn request_wants_close_returns_false_when_header_absent() {
        // HTTP/1.1 default is keep-alive — absence of `Connection`
        // means the loop should keep going.
        let headers = "GET /cli/foo HTTP/1.1\r\nHost: 127.0.0.1\r\n";
        assert!(!request_wants_close(headers));
    }

    #[test]
    fn request_wants_close_returns_true_for_explicit_close() {
        let headers = "GET / HTTP/1.1\r\nConnection: close\r\n";
        assert!(request_wants_close(headers));
    }

    #[test]
    fn request_wants_close_returns_false_for_keep_alive() {
        // HTTP/1.0 clients send this explicitly to opt in; HTTP/1.1
        // ones may send it redundantly. Either way: not close.
        let headers = "GET / HTTP/1.1\r\nConnection: keep-alive\r\n";
        assert!(!request_wants_close(headers));
    }

    #[test]
    fn request_wants_close_is_case_insensitive_on_field_name_and_value() {
        // RFC 9110 §5.6: field names + token values are case-insensitive.
        // We must not miss `CONNECTION: CLOSE` or `connection: Close`.
        assert!(request_wants_close("GET / HTTP/1.1\r\nCONNECTION: CLOSE\r\n"));
        assert!(request_wants_close("GET / HTTP/1.1\r\nconnection: Close\r\n"));
    }

    #[test]
    fn request_wants_close_handles_token_in_multi_value_header() {
        // `Connection` is a comma-separated token list. `close` may
        // appear alongside e.g. `Upgrade`. We must find it.
        assert!(request_wants_close(
            "GET / HTTP/1.1\r\nConnection: keep-alive, close\r\n"
        ));
        assert!(request_wants_close(
            "GET / HTTP/1.1\r\nConnection: Upgrade, close\r\n"
        ));
    }

    #[test]
    fn request_wants_close_ignores_unrelated_headers_containing_close() {
        // `X-Close-Reason: close` is NOT a Connection header. The
        // field-name match must gate against this.
        let headers =
            "GET / HTTP/1.1\r\nX-Close-Reason: close\r\nCookie: close=foo\r\n";
        assert!(!request_wants_close(headers));
    }

    #[test]
    fn request_wants_close_handles_no_value() {
        // `Connection:` with no value should not match.
        let headers = "GET / HTTP/1.1\r\nConnection:\r\n";
        assert!(!request_wants_close(headers));
    }

    // ── request_is_chunked: H5 chunked-desync guard ─────────────────

    #[test]
    fn request_is_chunked_false_when_header_absent() {
        let headers = "POST /cli/x HTTP/1.1\r\nContent-Length: 12\r\n\r\n";
        assert!(!request_is_chunked(headers));
    }

    #[test]
    fn request_is_chunked_true_for_bare_chunked() {
        let headers = "POST /cli/x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(request_is_chunked(headers));
    }

    #[test]
    fn request_is_chunked_is_case_insensitive() {
        assert!(request_is_chunked(
            "POST / HTTP/1.1\r\nTRANSFER-ENCODING: CHUNKED\r\n"
        ));
        assert!(request_is_chunked(
            "POST / HTTP/1.1\r\ntransfer-encoding: Chunked\r\n"
        ));
    }

    #[test]
    fn request_is_chunked_matches_token_in_multi_value_list() {
        // e.g. `gzip, chunked` — chunked is the final framing coding.
        assert!(request_is_chunked(
            "POST / HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n"
        ));
    }

    #[test]
    fn request_is_chunked_false_for_unrelated_te_value() {
        // A non-chunked transfer-coding (rare but legal) must not trip it.
        let headers = "POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n";
        assert!(!request_is_chunked(headers));
    }

    #[test]
    fn request_is_chunked_ignores_content_length_only() {
        // The normal daemon-client shape must pass through untouched.
        let headers =
            "POST /cli/awareness/publish HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 3\r\n\r\n{}";
        assert!(!request_is_chunked(headers));
    }

    // ── token_still_valid: the 5s WS re-auth heartbeat boolean ──────
    //
    // This is the single predicate every long-lived WS loop acts on each
    // tick (grid/session-events already; Part 1a adds bytes/subscribe/
    // awareness/events). A revoked connect-user MUST flip it false within
    // one tick so the socket tears down within ~5s.

    #[test]
    fn token_still_valid_accepts_owner_token() {
        // The owner (local daemon) token is never revoked, so an
        // owner-opened socket must always re-validate.
        assert!(token_still_valid("owner-secret", "owner-secret"));
    }

    #[test]
    fn token_still_valid_rejects_empty_token() {
        // An empty token never authorizes — must fail closed even when
        // the expected owner token is empty (degenerate boot state).
        assert!(!token_still_valid("", "owner-secret"));
        assert!(!token_still_valid("", ""));
    }

    #[test]
    fn token_still_valid_rejects_unknown_non_owner_token() {
        // A non-empty token that is neither the owner token nor a live
        // session must fail (no session map entry → `validate_session`
        // returns None). No temp HOME needed: an unknown token can't
        // match any session regardless of store state.
        assert!(!token_still_valid("not-a-real-session", "owner-secret"));
    }

    // Redirect `$HOME` to a fresh tempdir so the `connect-users.json` store +
    // session lifecycle are isolated from the real machine and other tests,
    // run `f`, then restore `$HOME`. Delegates to the crate-wide
    // `test_support::with_temp_home` so this serializes on the SAME lock as
    // every other HOME-swapping test in the crate (e.g. the tunnel-TLS cert
    // tests). `$HOME` is a process-global singleton; a module-local lock would
    // not prevent cross-module stomping.
    use crate::test_support::with_temp_home;

    #[test]
    fn token_still_valid_follows_session_lifecycle_create_then_revoke() {
        // The end-to-end predicate the heartbeat depends on: a freshly
        // minted connect-user session re-validates true; after the owner
        // revokes (disable / remove / role-change → `revoke_user_sessions`)
        // the SAME token flips false on the very next recheck — which is
        // what tears the live WS down within ~5s.
        with_temp_home(|| {
            let owner = "owner-token-xyz";
            k2_core::connect_users::add_user("reauth_user", "password123")
                .expect("add_user");
            let session = k2_core::connect_users::create_session("reauth_user");

            // Sanity: a session token is not the owner token, yet still
            // authorizes through the session-map path.
            assert_ne!(session.as_str(), owner);
            assert!(
                token_still_valid(&session, owner),
                "fresh session must re-validate",
            );

            // Owner revokes every session for this user (the in-memory
            // map drop performed by disable/remove/role-change).
            k2_core::connect_users::revoke_user_sessions("reauth_user");

            assert!(
                !token_still_valid(&session, owner),
                "revoked session must fail the next recheck (tears the WS down)",
            );
            // Owner token is independent of any user — never revoked.
            assert!(
                token_still_valid(owner, owner),
                "owner token survives a user revoke",
            );
        });
    }

    // ── authorize_send_message: Composer 1c capability gate (D4) ────────
    //
    // The load-bearing security gate for the send-message route. The owner
    // token is ALWAYS allowed; a connect-user is allowed ONLY when the host
    // opted in (`allow_remote_instruct`) AND role >= Member; everything else
    // is Denied (→ drain-then-403). These assertions fail LOUDLY.

    #[test]
    fn authorize_send_message_owner_always_allowed() {
        let owner = "owner-token-xyz";
        // Owner is allowed even when the opt-in flag is OFF (1a parity).
        assert_eq!(
            authorize_send_message(&format!("token={owner}"), owner, false),
            SendMessageAuth::Owner,
        );
        assert_eq!(
            authorize_send_message(&format!("token={owner}"), owner, true),
            SendMessageAuth::Owner,
        );
    }

    #[test]
    fn authorize_send_message_unknown_token_denied() {
        // A non-owner, non-session token is rejected regardless of opt-in.
        with_temp_home(|| {
            let owner = "owner-token-xyz";
            assert_eq!(
                authorize_send_message("token=not-a-session", owner, true),
                SendMessageAuth::Denied,
            );
            // Missing token param → Denied.
            assert_eq!(
                authorize_send_message("project=/tmp/x", owner, true),
                SendMessageAuth::Denied,
            );
        });
    }

    #[test]
    fn authorize_send_message_connect_user_blocked_when_opt_in_off() {
        // The whole point of the safe default: a fully-valid connect-user
        // (role >= Member) is STILL Denied while the host opt-in is OFF.
        with_temp_home(|| {
            let owner = "owner-token-xyz";
            k2_core::connect_users::add_user("member_user", "password123")
                .expect("add_user");
            let session = k2_core::connect_users::create_session("member_user");

            assert_eq!(
                authorize_send_message(&format!("token={session}"), owner, false),
                SendMessageAuth::Denied,
                "connect-user must be BLOCKED until the host opts in",
            );
        });
    }

    #[test]
    fn authorize_send_message_connect_user_allowed_when_opt_in_on() {
        // With the host opted in, a Member connect-user is allowed and the
        // resolved `from` is THEIR username (D3 — never the body).
        with_temp_home(|| {
            let owner = "owner-token-xyz";
            k2_core::connect_users::add_user("member_user", "password123")
                .expect("add_user");
            let session = k2_core::connect_users::create_session("member_user");

            assert_eq!(
                authorize_send_message(&format!("token={session}"), owner, true),
                SendMessageAuth::ConnectUser {
                    username: "member_user".to_string()
                },
            );
        });
    }

    #[test]
    fn authorize_send_message_revoked_token_denied_at_gate() {
        // M2 (gate half): a revoked connect-user token flips to Denied on the
        // next request even with the opt-in ON — no injection is authorized.
        with_temp_home(|| {
            let owner = "owner-token-xyz";
            k2_core::connect_users::add_user("member_user", "password123")
                .expect("add_user");
            let session = k2_core::connect_users::create_session("member_user");

            // Sanity: allowed before revoke.
            assert_eq!(
                authorize_send_message(&format!("token={session}"), owner, true),
                SendMessageAuth::ConnectUser {
                    username: "member_user".to_string()
                },
            );

            k2_core::connect_users::revoke_user_sessions("member_user");

            assert_eq!(
                authorize_send_message(&format!("token={session}"), owner, true),
                SendMessageAuth::Denied,
                "revoked session must be Denied (no injection authorized)",
            );
        });
    }
}
