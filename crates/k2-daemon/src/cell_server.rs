//! Per-cell Unix-domain-socket hook server (#58 Phase 1 — flag-gated by
//! `K2_HOOK_SCOPED`, default OFF).
//!
//! ## What this is
//!
//! Phase 0 built the per-cell `UnixListener` bind helper ([`crate::cell_uds`])
//! and the scoped-token registry ([`crate::session_token`]) but never SERVED
//! traffic on the socket. Phase 1 turns it on: when scoped hooks are enabled,
//! [`crate::v2_spawn::handle_v2_spawn`] mints a per-cell token, binds the
//! cell's socket, and hands the listener here. [`serve_cell`] runs a small
//! accept loop that authenticates + serves the agent **lifecycle hook**
//! (`/hook/complete`) for exactly THAT cell.
//!
//! ## Why a dedicated mini-server instead of the TCP dispatcher
//!
//! The PRD's full "generalize `dispatch` over a stream trait" is a larger
//! change (and the H1 close); Phase 1 keeps the TCP `routes::dispatcher`
//! UNTOUCHED and serves only the one route the cell channel needs now. The
//! general `k2` verbs (`msg`/`reply`/…) still ride loopback-TCP in Phase 1
//! (with the disk OWNER token — see the `cli/k2` + `notify.sh` inversion), so
//! anything other than `/hook/complete` here returns 404 and the caller falls
//! back to TCP. No stranding; minimal blast radius.
//!
//! ## Identity = the connection, never the body (PRD §3.2)
//!
//! The daemon CREATED `<sid>.sock` and owns the `session_id → claims` map, so
//! bytes on this socket are THIS cell by construction. We additionally:
//!   - require the presented bearer to be a valid scoped token bound to this
//!     cell's `session_id` AND the request `paneId`, and
//!   - read the peer credential (belt) and require the connecting uid to be
//!     the daemon's own uid (the bare-PTY child runs as the same user).

#[cfg(unix)]
pub use unix_impl::serve_cell;

#[cfg(unix)]
mod unix_impl {
    use std::os::unix::io::AsRawFd;

    use std::collections::HashMap;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    use k2_core::log_debug;
    use k2_core::session::SessionId;

    use crate::session_token::{HookPrincipal, ValidatedHook};

    /// Max request-head size before we refuse with 414 (mirrors the TCP
    /// dispatcher's head cap). Bodies are read AFTER the head via
    /// Content-Length; the head itself (request line + headers) is tiny.
    const MAX_HEAD: usize = 16 * 1024;

    /// Pure authorization decision for an accepted per-cell connection.
    ///
    /// `validated` is the output of `require_hook(bearer, "/hook/complete")`
    /// for this connection (so the capability allowlist already fired).
    /// Authorized iff, on top of that:
    ///   1. the peer uid equals the daemon's own uid (peer-cred belt), AND
    ///   2. the token is bound to THIS cell's `session_id` (structural —
    ///      can't replay cell A's token on cell B's socket), AND
    ///   3. the request `paneId` is non-empty and equals the token's pane.
    pub(crate) fn cell_request_authorized(
        validated: Option<&ValidatedHook>,
        this_session_id: &str,
        req_pane: &str,
        peer_uid: u32,
        self_uid: u32,
    ) -> bool {
        if peer_uid != self_uid {
            return false;
        }
        match validated {
            Some(v) => {
                v.session_id == this_session_id && !req_pane.is_empty() && v.pane_id == req_pane
            }
            None => false,
        }
    }

    /// Pure authorization decision for an accepted per-cell connection on a
    /// GENERIC agent verb (everything other than `/hook/complete`, which has
    /// the extra pane match via [`cell_request_authorized`]).
    ///
    /// `validated` is `require_hook(bearer, path)` for this connection — so
    /// the capability allowlist (msg / inbox / review / awareness-publish)
    /// already fired. Authorized iff, on top of that:
    ///   1. the peer uid equals the daemon's own uid (peer-cred belt), AND
    ///   2. the token is bound to THIS cell's `session_id` (structural — a
    ///      token minted for cell A can't be replayed on cell B's socket).
    ///
    /// No `paneId` match here: a generic verb is a normal CLI call, not the
    /// lifecycle hook, so the pane is irrelevant — the session binding + the
    /// allowlist are the gate.
    pub(crate) fn cell_verb_authorized(
        validated: Option<&ValidatedHook>,
        this_session_id: &str,
        peer_uid: u32,
        self_uid: u32,
    ) -> bool {
        if peer_uid != self_uid {
            return false;
        }
        match validated {
            Some(v) => v.session_id == this_session_id,
            None => false,
        }
    }

    /// Stamp the SENDER identity into a query/form param map from the
    /// connection's resolved principal — the body is NEVER trusted for WHO
    /// is sending (PRD §3.2). `from` + `project_id` are OVERWRITTEN. The
    /// agent's OWN workspace PATH (`project` / `project_path`) is resolved
    /// from the principal and injected ONLY when the caller did not already
    /// supply one — the scoped CLI omits it (it's identity-derived, not
    /// forgeable), but an explicit recipient/target on a non-own-workspace
    /// verb is never clobbered. Recipient args (`workspace`/`target`) are
    /// left untouched.
    pub(crate) fn stamp_principal(params: &mut HashMap<String, String>, principal: &HookPrincipal) {
        params.insert("from".to_string(), principal.agent_address.clone());
        params.insert("project_id".to_string(), principal.workspace_uuid.clone());

        let needs_path = params
            .get("project_path")
            .or_else(|| params.get("project"))
            .map(|s| s.is_empty())
            .unwrap_or(true);
        if needs_path {
            if let Some(path) = resolve_principal_path(principal) {
                params.insert("project".to_string(), path.clone());
                params.insert("project_path".to_string(), path);
            }
        }
    }

    /// Resolve the principal's workspace UUID → on-disk path (the agent's own
    /// workspace), so the own-workspace verbs (inbox reads/writes) route
    /// correctly over the UDS without the body carrying a forgeable path.
    /// `None` when the uuid is empty or unknown (e.g. test fixtures) — the
    /// downstream handler then reports its normal "Missing project".
    fn resolve_principal_path(principal: &HookPrincipal) -> Option<String> {
        let uuid = principal.workspace_uuid.trim();
        if uuid.is_empty() {
            return None;
        }
        crate::workspace_msg::resolve_workspace(uuid)
    }

    /// Re-stamp an awareness `AgentSignal` JSON body so `from.workspace` +
    /// `from.name` reflect the connection's principal, never the body. Works
    /// on the raw JSON `Value` (not the typed enum) so it's robust to the
    /// `Agent` / `Workspace` / `Broadcast` shapes: it only sets the fields a
    /// given `scope` actually carries. Returns the re-serialized bytes; if
    /// the body doesn't parse as JSON it is returned unchanged (the handler
    /// then 400s on its own typed parse).
    pub(crate) fn restamp_awareness_from(body: &[u8], principal: &HookPrincipal) -> Vec<u8> {
        let mut v: serde_json::Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        if let Some(from) = v.get_mut("from").and_then(|f| f.as_object_mut()) {
            let scope = from
                .get("scope")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let scope = scope.as_str();
            // `workspace` is present on Agent + Workspace scopes.
            if scope == "agent" || scope == "workspace" {
                from.insert(
                    "workspace".to_string(),
                    serde_json::Value::String(principal.workspace_uuid.clone()),
                );
            }
            // `name` only exists on the Agent scope.
            if scope == "agent" {
                from.insert(
                    "name".to_string(),
                    serde_json::Value::String(principal.agent_address.clone()),
                );
            }
        }
        serde_json::to_vec(&v).unwrap_or_else(|_| body.to_vec())
    }

    /// The daemon's effective uid — the bare-PTY child runs as the same user,
    /// so a connecting peer with a different uid is rejected.
    fn self_uid() -> u32 {
        // SAFETY: geteuid() is always safe — no args, no failure mode.
        unsafe { libc::geteuid() }
    }

    /// Spawn the per-cell accept loop on the tokio runtime. `listener` was
    /// bound synchronously by `handle_v2_spawn` (so a bind failure surfaces
    /// there, before this is called). The loop runs until the socket file is
    /// removed (cell teardown) or the listener errors.
    pub fn serve_cell(session_id: SessionId, listener: std::os::unix::net::UnixListener) {
        if let Err(e) = listener.set_nonblocking(true) {
            log_debug!("[hook-scoped] WARN set_nonblocking cell sock {session_id}: {e}");
            return;
        }
        let tok_listener = match tokio::net::UnixListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                log_debug!("[hook-scoped] WARN from_std cell sock {session_id}: {e}");
                return;
            }
        };
        let sid_str = session_id.to_string();
        let sock_path = crate::cell_uds::cell_socket_path(&session_id);
        let uid = self_uid();

        tokio::spawn(async move {
            // Periodic liveness check: when the cell tears down, the
            // child-exit observer removes the socket file → we stop. Cheap
            // (30s) so a dead cell doesn't leave a parked task forever.
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    accepted = tok_listener.accept() => {
                        match accepted {
                            Ok((stream, _addr)) => {
                                let sid = sid_str.clone();
                                tokio::spawn(async move {
                                    handle_conn(stream, sid, uid).await;
                                });
                            }
                            Err(e) => {
                                log_debug!("[hook-scoped] cell sock {sid_str} accept error: {e}; stopping");
                                break;
                            }
                        }
                    }
                    _ = ticker.tick() => {
                        if !sock_path.exists() {
                            log_debug!("[hook-scoped] cell sock {sid_str} gone; stopping accept loop");
                            break;
                        }
                    }
                }
            }
        });
    }

    /// Accumulate the request head (until `\r\n\r\n`, capped at
    /// [`MAX_HEAD`]) then read exactly `Content-Length` body bytes.
    /// `Transfer-Encoding: chunked` is REFUSED (mirrors the TCP dispatcher —
    /// we never decode chunked framing, so a chunked body would desync). One
    /// connection serves one request. Returns `(head, body)` or an error
    /// status string to write back.
    async fn read_request(stream: &mut UnixStream) -> Result<(String, Vec<u8>), &'static str> {
        let mut buf: Vec<u8> = Vec::with_capacity(2048);
        let mut chunk = [0u8; 4096];
        // Accumulate until the header/body delimiter is present.
        let head_end = loop {
            if let Some(pos) = find_crlf_crlf(&buf) {
                break pos;
            }
            if buf.len() > MAX_HEAD {
                return Err("414 URI Too Long");
            }
            match stream.read(&mut chunk).await {
                Ok(0) => return Err("400 Bad Request"), // EOF before head complete
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => return Err("400 Bad Request"),
            }
        };

        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();

        // Refuse chunked framing — we only frame by Content-Length.
        if crate::routes::http::request_is_chunked(&head) {
            return Err("400 Bad Request");
        }

        // Read exactly Content-Length more bytes for the body (default 0).
        let content_len = head
            .lines()
            .find_map(|l| {
                let (name, val) = l.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("content-length") {
                    val.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);

        let body_start = head_end + 4; // skip the \r\n\r\n
        let mut body: Vec<u8> = buf.get(body_start..).map(|s| s.to_vec()).unwrap_or_default();
        while body.len() < content_len {
            if body.len() > MAX_HEAD + content_len {
                break;
            }
            match stream.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        body.truncate(content_len);
        Ok((head, body))
    }

    /// Find the byte offset of the `\r\n\r\n` header/body delimiter.
    fn find_crlf_crlf(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }

    /// Read the request off the connection, authenticate it structurally +
    /// by scoped token, stamp the sender identity from the principal, and
    /// dispatch to the SAME pure handler functions the TCP dispatcher calls
    /// (no stream-generalization; we touch `dispatcher.rs` zero times for the
    /// verb channel). Anything not on the scoped allowlist → 404 sentinel so
    /// the CLI falls back to loopback-TCP.
    async fn handle_conn(mut stream: UnixStream, this_session_id: String, self_uid: u32) {
        // peer-cred belt (best-effort): read the connecting uid via the raw
        // fd, then carry on with the SAME tokio stream (no consume/rebuild).
        let peer_uid = peer_uid_of(&stream).unwrap_or(u32::MAX);

        let (head, body) = match read_request(&mut stream).await {
            Ok(pair) => pair,
            Err(status) => {
                write_response(
                    &mut stream,
                    status,
                    "application/json",
                    r#"{"error":"bad request on cell socket"}"#,
                )
                .await;
                return;
            }
        };

        let first_line = head.lines().next().unwrap_or("");
        let mut it = first_line.split_whitespace();
        let method = it.next().unwrap_or("");
        let target = it.next().unwrap_or("");
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p, q),
            None => (target, ""),
        };

        // params = query + (for form verbs) the urlencoded POST body, body
        // winning on collision — exactly the TCP dispatcher's merge order.
        let mut params = crate::routes::http::parse_params(path, query);
        let is_post = method.eq_ignore_ascii_case("POST");
        let body_is_form = is_post
            && !body.is_empty()
            && {
                let s = String::from_utf8_lossy(&body);
                let t = s.trim_start();
                !t.starts_with('{') && !t.starts_with('[')
            };
        if body_is_form {
            for (k, v) in crate::routes::http::parse_form_body(&body) {
                params.insert(k, v);
            }
        }

        // Bearer header preferred (kept out of logs/transcripts); `?token=`
        // is the fallback. The credential is the per-cell SCOPED token from
        // the child's `K2_HOOK_TOKEN` env.
        let req_token = params.get("token").cloned().unwrap_or_default();
        let presented = crate::routes::http::extract_bearer(&head)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(req_token);

        // require_hook = capability allowlist (default-deny) + scoped-token
        // validation against the process registry. Computed ONCE for every
        // verb, before any handler runs.
        let validated = crate::session_token::require_hook(&presented, path);
        let req_pane = params.get("paneId").cloned().unwrap_or_default();

        // Authorization: `/hook/complete` additionally pins the paneId; every
        // other verb is a normal CLI call gated on uid + session binding.
        let authorized = if path == "/hook/complete" {
            cell_request_authorized(validated.as_ref(), &this_session_id, &req_pane, peer_uid, self_uid)
        } else {
            cell_verb_authorized(validated.as_ref(), &this_session_id, peer_uid, self_uid)
        };
        if !authorized {
            write_response(
                &mut stream,
                "403 Forbidden",
                "application/json",
                r#"{"error":"Invalid or missing auth token"}"#,
            )
            .await;
            return;
        }

        // SAFE: authorized implies `validated` is Some.
        let principal = match validated.as_ref() {
            Some(v) => v.principal.clone(),
            None => {
                write_response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"Invalid or missing auth token"}"#,
                )
                .await;
                return;
            }
        };

        // Sender identity is stamped server-side; body never trusted for it.
        stamp_principal(&mut params, &principal);

        let path_owned = path.to_string();
        let method_owned = method.to_string();
        // The pure handlers lock the DB / sleep across inject windows, so run
        // them on the blocking pool exactly like the TCP dispatcher does.
        let (status, ctype, out): (String, &'static str, String) =
            tokio::task::spawn_blocking(move || dispatch_cell_verb(&method_owned, &path_owned, &params, &body, &principal))
                .await
                .unwrap_or_else(|e| {
                    (
                        "500 Internal Server Error".to_string(),
                        "application/json",
                        serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
                    )
                });

        write_response(&mut stream, &status, ctype, &out).await;
    }

    /// Route an authorized cell-socket request to the SAME pure handler the
    /// TCP dispatcher uses (the "core insight": share business logic by
    /// calling these functions directly). Returns `(status, content_type,
    /// body)`. Unknown paths → the 404 TCP-fallback sentinel.
    fn dispatch_cell_verb(
        method: &str,
        path: &str,
        params: &HashMap<String, String>,
        body: &[u8],
        principal: &HookPrincipal,
    ) -> (String, &'static str, String) {
        let is_post = method.eq_ignore_ascii_case("POST");
        match path {
            "/hook/complete" => (
                "200 OK".to_string(),
                "application/json",
                k2_core::agent_hooks::handle_hook_complete(params).to_string(),
            ),
            "/cli/workspace/msg" => from_cli(crate::cli::dispatch(path, params)),
            "/cli/awareness/publish" => {
                let restamped = restamp_awareness_from(body, principal);
                let r = crate::awareness_ws::handle_publish(&restamped);
                (r.status.to_string(), "application/json", r.body)
            }
            "/cli/review-checklist/write" if is_post => {
                from_cli(crate::review_checklist_routes::handle_write(body))
            }
            "/cli/review-checklist/toggle" if is_post => {
                from_cli(crate::review_checklist_routes::handle_toggle(body))
            }
            "/cli/review-checklist/init" if is_post => {
                from_cli(crate::review_checklist_routes::handle_init(body))
            }
            // Inbox: POSTs are mutations (dispatch_post); GET reads route
            // through cli::dispatch (→ misc_routes → inbox_routes::handle_*),
            // exactly as the TCP path does.
            p if p.starts_with("/cli/inbox/") && is_post => {
                from_cli(crate::inbox_routes::dispatch_post(p, params))
            }
            p if crate::session_token::is_agent_verb(p) && !is_post => {
                from_cli(crate::cli::dispatch(p, params))
            }
            _ => (
                "404 Not Found".to_string(),
                "application/json",
                r#"{"error":"route not served on cell socket"}"#.to_string(),
            ),
        }
    }

    /// Normalize a `CliResponse` into the `(status, content_type, body)`
    /// triple the cell writer emits.
    fn from_cli(r: crate::cli_response::CliResponse) -> (String, &'static str, String) {
        (r.status.to_string(), r.content_type, r.body)
    }

    /// Read the peer uid of a connected tokio `UnixStream` without consuming
    /// it: borrow the raw fd and call `getpeereid` / `SO_PEERCRED` directly.
    #[cfg(target_os = "linux")]
    fn peer_uid_of(stream: &UnixStream) -> Option<u32> {
        let fd = stream.as_raw_fd();
        let mut ucred = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: fd is a valid connected socket for the call; sized out-param.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut ucred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if rc == 0 {
            Some(ucred.uid)
        } else {
            None
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn peer_uid_of(stream: &UnixStream) -> Option<u32> {
        let fd = stream.as_raw_fd();
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: fd is a valid connected socket; sized out-params.
        let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        if rc == 0 {
            Some(uid as u32)
        } else {
            None
        }
    }

    async fn write_response(stream: &mut UnixStream, status: &str, content_type: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let _ = stream.write_all(resp.as_bytes()).await;
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::session_token::HookPrincipal;

        fn vh(session_id: &str, pane_id: &str) -> ValidatedHook {
            ValidatedHook {
                session_id: session_id.to_string(),
                pane_id: pane_id.to_string(),
                principal: HookPrincipal {
                    workspace_uuid: "ws".to_string(),
                    agent_address: "a".to_string(),
                },
            }
        }

        #[test]
        fn authorized_when_uid_session_and_pane_all_match() {
            let v = vh("sid-1", "pane-1");
            assert!(cell_request_authorized(Some(&v), "sid-1", "pane-1", 501, 501));
        }

        #[test]
        fn rejected_on_uid_mismatch() {
            // peer-cred belt: a different uid is refused even with a valid
            // token bound to this cell + pane.
            let v = vh("sid-1", "pane-1");
            assert!(!cell_request_authorized(Some(&v), "sid-1", "pane-1", 999, 501));
        }

        #[test]
        fn rejected_when_token_bound_to_a_different_cell() {
            // Structural binding: cell B's token presented on cell A's socket.
            let v = vh("sid-OTHER", "pane-1");
            assert!(!cell_request_authorized(Some(&v), "sid-1", "pane-1", 501, 501));
        }

        #[test]
        fn rejected_on_pane_mismatch_or_empty() {
            let v = vh("sid-1", "pane-1");
            assert!(!cell_request_authorized(Some(&v), "sid-1", "pane-2", 501, 501));
            assert!(!cell_request_authorized(Some(&v), "sid-1", "", 501, 501));
        }

        #[test]
        fn rejected_when_token_did_not_validate() {
            assert!(!cell_request_authorized(None, "sid-1", "pane-1", 501, 501));
        }

        // ── generic-verb authorization (uid + session binding, no pane) ──

        #[test]
        fn verb_authorized_on_uid_and_session_match() {
            let v = vh("sid-1", "pane-1");
            // No paneId needed for a generic verb; only uid + session bind.
            assert!(cell_verb_authorized(Some(&v), "sid-1", 501, 501));
        }

        #[test]
        fn verb_rejected_on_wrong_uid() {
            let v = vh("sid-1", "pane-1");
            assert!(!cell_verb_authorized(Some(&v), "sid-1", 999, 501));
        }

        #[test]
        fn verb_rejected_on_wrong_session() {
            // A token minted for a different cell, replayed on this socket.
            let v = vh("sid-OTHER", "pane-1");
            assert!(!cell_verb_authorized(Some(&v), "sid-1", 501, 501));
        }

        #[test]
        fn verb_rejected_when_token_did_not_validate() {
            assert!(!cell_verb_authorized(None, "sid-1", 501, 501));
        }

        // ── awareness body re-stamp (identity from principal, not body) ──

        fn principal() -> HookPrincipal {
            HookPrincipal {
                workspace_uuid: "ws-real-uuid".to_string(),
                agent_address: "real-agent".to_string(),
            }
        }

        #[test]
        fn restamp_overwrites_agent_from_workspace_and_name() {
            // The body CLAIMS to be a different agent in a different
            // workspace; the re-stamp must overwrite both with the principal.
            let body = serde_json::json!({
                "from": { "scope": "agent", "workspace": "forged-ws", "name": "forged-agent" },
                "to":   { "scope": "broadcast" },
                "kind": { "kind": "msg", "data": { "text": "hi" } }
            })
            .to_string();
            let out = restamp_awareness_from(body.as_bytes(), &principal());
            let v: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
            assert_eq!(v["from"]["workspace"], serde_json::json!("ws-real-uuid"));
            assert_eq!(v["from"]["name"], serde_json::json!("real-agent"));
            // Recipient is NOT touched.
            assert_eq!(v["to"]["scope"], serde_json::json!("broadcast"));
        }

        #[test]
        fn restamp_sets_workspace_for_workspace_scope_but_no_name() {
            let body = serde_json::json!({
                "from": { "scope": "workspace", "workspace": "forged-ws" },
                "to":   { "scope": "broadcast" }
            })
            .to_string();
            let out = restamp_awareness_from(body.as_bytes(), &principal());
            let v: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
            assert_eq!(v["from"]["workspace"], serde_json::json!("ws-real-uuid"));
            assert!(v["from"].get("name").is_none(), "workspace scope has no name");
        }

        #[test]
        fn restamp_returns_original_on_non_json() {
            let body = b"not json at all";
            let out = restamp_awareness_from(body, &principal());
            assert_eq!(out, body, "unparseable body is passed through unchanged");
        }

        // ── principal stamping into a param map ──────────────────────────

        #[test]
        fn stamp_overwrites_from_and_project_id_and_keeps_recipient() {
            // In-memory DB so the own-workspace path resolution (which queries
            // `projects`) is deterministic + side-effect-free: "ws-real-uuid"
            // matches no project → no `project` injection, leaving the
            // assertions below about from/project_id/workspace clean.
            let _ = k2_core::db::init_for_tests();
            let mut params: HashMap<String, String> = HashMap::new();
            params.insert("from".to_string(), "FORGED".to_string());
            params.insert("project_id".to_string(), "forged-uuid".to_string());
            params.insert("workspace".to_string(), "recipient-ws".to_string());
            stamp_principal(&mut params, &principal());
            assert_eq!(params.get("from").map(String::as_str), Some("real-agent"));
            assert_eq!(params.get("project_id").map(String::as_str), Some("ws-real-uuid"));
            // Recipient routing arg is NOT overwritten by identity stamping.
            assert_eq!(params.get("workspace").map(String::as_str), Some("recipient-ws"));
        }
    }
}
