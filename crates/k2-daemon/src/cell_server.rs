//! Per-cell Unix-domain-socket hook server (#58 Phase 1 — flag-gated by
//! `K2_HOOK_SCOPED`, default ON; opt out with `0`/`false`/`off`).
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

    /// Absolute cap on a request body, regardless of the declared
    /// `Content-Length`. The legitimate large payloads here are inbox
    /// compose/respond memos and msg text (kilobytes); 4 MiB is a generous
    /// ceiling that never clips real traffic but refuses an attacker-declared
    /// huge `Content-Length` outright (413) instead of allocating toward it.
    /// Belt for the sandbox boundary: when a sealed cell is an untrusted
    /// tenant, a request over this socket must not be able to drive the host
    /// daemon toward OOM via the body length. (Same-uid own-fleet is already
    /// inside the trust boundary, but this costs nothing and removes the
    /// footgun before the flag is ever flipped on.)
    const MAX_BODY: usize = 4 * 1024 * 1024;

    /// The set of peer uids allowed to drive this cell's socket.
    ///
    /// **The peer-cred belt, widened by exactly ONE known uid for sandboxed
    /// cells (Sandbox B2).** For a bare-PTY (non-microVM) cell the connecting
    /// child runs as the daemon's own user, so the only allowed peer is
    /// `self_uid` and `cell_uid` is `None` → byte-identical to pre-B2. For a
    /// microVM-backed cell the in-guest `k2`/`claude` reaches us over the
    /// VMM's vsock→UDS forwarder, so the peer on the host socket is the VMM
    /// process AFTER it has dropped to the dedicated `k2cell` uid — that ONE
    /// uid (`cell_uid`) is additionally allowed. Never an arbitrary uid: the
    /// set is `{self_uid}` ∪ (`{cell_uid}` iff the cell is microVM-backed and
    /// the dedicated user resolves). The structural defenses (scoped token
    /// bound to this `session_id`, the capability allowlist, the Finding-1
    /// operand pin) remain THE gate; this stays a belt.
    fn peer_uid_allowed(peer_uid: u32, self_uid: u32, cell_uid: Option<u32>) -> bool {
        peer_uid == self_uid || cell_uid == Some(peer_uid)
    }

    /// Pure authorization decision for an accepted per-cell connection.
    ///
    /// `validated` is the output of `require_hook(bearer, "/hook/complete")`
    /// for this connection (so the capability allowlist already fired).
    /// Authorized iff, on top of that:
    ///   1. the peer uid is in the allowed set (peer-cred belt —
    ///      `{self_uid}` ∪ optional sandbox `cell_uid`; see
    ///      [`peer_uid_allowed`]), AND
    ///   2. the token is bound to THIS cell's `session_id` (structural —
    ///      can't replay cell A's token on cell B's socket), AND
    ///   3. the request `paneId` is non-empty and equals the token's pane.
    pub(crate) fn cell_request_authorized(
        validated: Option<&ValidatedHook>,
        this_session_id: &str,
        req_pane: &str,
        peer_uid: u32,
        self_uid: u32,
        cell_uid: Option<u32>,
    ) -> bool {
        if !peer_uid_allowed(peer_uid, self_uid, cell_uid) {
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
    ///   1. the peer uid is in the allowed set (peer-cred belt — `{self_uid}`
    ///      ∪ optional sandbox `cell_uid`; see [`peer_uid_allowed`]), AND
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
        cell_uid: Option<u32>,
    ) -> bool {
        if !peer_uid_allowed(peer_uid, self_uid, cell_uid) {
            return false;
        }
        match validated {
            Some(v) => v.session_id == this_session_id,
            None => false,
        }
    }

    /// Stamp the SENDER identity into a query/form param map from the
    /// connection's resolved principal — the body is NEVER trusted for WHO is
    /// sending (PRD §3.2). `from` + `project_id` are OVERWRITTEN.
    ///
    /// Sandbox P1 — **Finding 1 fix (force + fail-closed).** The operand
    /// workspace PATH (`project` / `project_path`) is FORCED to the
    /// principal's own workspace, OVERWRITING any body-supplied value: a
    /// hand-crafted raw UDS request can no longer set `project=<other-ws>` to
    /// act on another workspace's inbox/data (cross-tenant once the sandbox
    /// seals a cell). When the principal's workspace can't be resolved we
    /// FAIL CLOSED — both keys are REMOVED so the downstream handler 400s
    /// ("Missing project") rather than defaulting somewhere. Recipient args
    /// (`workspace`/`target` = WHO you address) are left untouched — they
    /// route the recipient, not the operand. (`review-checklist` + awareness
    /// take the raw body, not this param map, so they are NOT pinned here —
    /// review-checklist is dropped from the scoped allowlist in P1; awareness
    /// re-stamps `from` via `restamp_awareness_from`.)
    /// Identity stamp — shared with HTTP mail/DNS via
    /// [`crate::caller_workspace::stamp_principal`].
    pub(crate) fn stamp_principal(params: &mut HashMap<String, String>, principal: &HookPrincipal) {
        crate::caller_workspace::stamp_principal(params, principal);
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
            // `name` only exists on the Agent scope — human display name,
            // not the passport / agent_address (often the bare project UUID).
            if scope == "agent" {
                let name = crate::workspace_msg::resolve_workspace(
                    principal.workspace_uuid.trim(),
                )
                .map(|path| k2_core::workspace::display::agent_display_name(&path))
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| {
                    let addr = principal.agent_address.trim();
                    if !addr.is_empty() {
                        addr.to_string()
                    } else {
                        "agent".to_string()
                    }
                });
                from.insert("name".to_string(), serde_json::Value::String(name));
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
    ///
    /// `cell_uid` is per-session tier gating (Sandbox B2 / P4-H6): the caller
    /// (`spawn_session`) allocated the cell's PER-SESSION worker uid and passes
    /// it here, so the allowed peer-uid set is resolved ONCE and captured into
    /// the accept loop. A microVM cell allows EXACTLY its own per-session uid (the
    /// VMM is the host-socket peer after priv-drop to that uid); a bare-PTY cell
    /// passes `None` → the set stays `{daemon uid}`, byte-identical to pre-B2.
    pub fn serve_cell(
        session_id: SessionId,
        listener: std::os::unix::net::UnixListener,
        cell_uid: Option<u32>,
    ) {
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
                                    handle_conn(stream, sid, uid, cell_uid).await;
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
    async fn read_request(stream: &mut UnixStream) -> Result<(String, Vec<u8>), RequestRefused> {
        let mut buf: Vec<u8> = Vec::with_capacity(2048);
        let mut chunk = [0u8; 4096];
        // Accumulate until the header/body delimiter is present.
        let head_end = loop {
            if let Some(pos) = find_crlf_crlf(&buf) {
                break pos;
            }
            if buf.len() > MAX_HEAD {
                return Err(RequestRefused::status("414 URI Too Long"));
            }
            match stream.read(&mut chunk).await {
                Ok(0) => return Err(RequestRefused::status("400 Bad Request")), // EOF before head complete
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => return Err(RequestRefused::status("400 Bad Request")),
            }
        };

        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();

        // Refuse chunked framing — we only frame by Content-Length.
        if crate::routes::http::request_is_chunked(&head) {
            return Err(RequestRefused::status("400 Bad Request"));
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

        // Refuse an oversized declared body BEFORE allocating toward it.
        // The caller drains what the client is still sending after the
        // 413, so the close is clean instead of a reset.
        if content_len > MAX_BODY {
            let body_read = buf.len().saturating_sub(body_start);
            return Err(RequestRefused {
                status: "413 Payload Too Large",
                drain: Some(content_len.saturating_sub(body_read)),
            });
        }

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

    /// Why [`read_request`] refused a request: the status to answer and,
    /// for a 413, how many declared body bytes the client has yet to send
    /// (drained after the answer; see `routes::http::drain_refused_body`).
    struct RequestRefused {
        status: &'static str,
        drain: Option<usize>,
    }

    impl RequestRefused {
        fn status(status: &'static str) -> Self {
            Self { status, drain: None }
        }
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
    async fn handle_conn(
        mut stream: UnixStream,
        this_session_id: String,
        self_uid: u32,
        cell_uid: Option<u32>,
    ) {
        // peer-cred belt (best-effort): read the connecting uid via the raw
        // fd, then carry on with the SAME tokio stream (no consume/rebuild).
        let peer_uid = peer_uid_of(&stream).unwrap_or(u32::MAX);

        let (head, body) = match read_request(&mut stream).await {
            Ok(pair) => pair,
            Err(refused) => {
                write_response(
                    &mut stream,
                    refused.status,
                    "application/json",
                    r#"{"error":"bad request on cell socket"}"#,
                )
                .await;
                if refused.drain.is_some() {
                    crate::routes::http::drain_refused_body(&mut stream, refused.drain).await;
                }
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
        // `/hook/event` names its pane in `X-K2-Pane` (the body is the raw
        // hook JSON); `/hook/complete` in `paneId`.
        let hook_event = path == "/hook/event";
        let req_pane = if hook_event {
            crate::routes::http::extract_header(&head, "x-k2-pane")
                .unwrap_or("")
                .to_string()
        } else {
            params.get("paneId").cloned().unwrap_or_default()
        };
        // DA12 (A4): the cell server checks no method, so the hook ingest
        // does: POST only, before anything else.
        if hook_event && !is_post {
            write_response(
                &mut stream,
                "405 Method Not Allowed",
                "application/json",
                r#"{"error":"POST required"}"#,
            )
            .await;
            return;
        }

        // Authorization: the hook routes additionally pin the pane; every
        // other verb is a normal CLI call gated on uid + session binding.
        let authorized = if path == "/hook/complete" || hook_event {
            cell_request_authorized(
                validated.as_ref(),
                &this_session_id,
                &req_pane,
                peer_uid,
                self_uid,
                cell_uid,
            )
        } else {
            cell_verb_authorized(validated.as_ref(), &this_session_id, peer_uid, self_uid, cell_uid)
        };
        if !authorized {
            // Permission cards (PRD rev 3.1 §3.1, vs-live C3): a route that
            // isn't an agent verb is refused here, before any handler. With
            // the gate on for a VALID passport bound to this cell, that
            // refusal teaches (`owner_only` + `permission`); a bad token, or
            // the gate off, keeps today's text.
            let gated = if k2_core::permission::gate::server_on() && path != "/hook/complete" && !hook_event {
                crate::session_token::validate_hook(&presented)
                    .filter(|v| {
                        cell_verb_authorized(Some(v), &this_session_id, peer_uid, self_uid, cell_uid)
                    })
                    .and_then(|v| {
                        crate::permission::walls::cell_gated_body(
                            method,
                            path,
                            &params,
                            &body,
                            &v.principal.workspace_uuid,
                        )
                    })
            } else {
                None
            };
            let body = gated.as_deref().unwrap_or(r#"{"error":"Invalid or missing auth token"}"#);
            write_response(&mut stream, "403 Forbidden", "application/json", body).await;
            return;
        }

        // DA12: the full-payload hook ingest. Same parser + owner check as
        // the TCP route; 204 unless the body is over 1 MiB (413).
        if hook_event {
            if body.len() > k2_core::agent_hooks::envelope::MAX_BODY_BYTES {
                write_response(
                    &mut stream,
                    "413 Payload Too Large",
                    "application/json",
                    r#"{"error":"hook body over 1 MiB"}"#,
                )
                .await;
                return;
            }
            let hook_headers = k2_core::agent_hooks::envelope::HookHeaders::from_lookup(|n| {
                crate::routes::http::extract_header(&head, n)
            });
            let report = k2_core::agent_hooks::envelope::wants_report(|n| {
                crate::routes::http::extract_header(&head, n)
            });
            let verdict =
                tokio::task::spawn_blocking(move || crate::hook_ingest::ingest(&hook_headers, &body)).await;
            match (report, verdict) {
                // `k2 hooks emit` (GL45) asks for the verdict.
                (true, Ok(v)) => {
                    let body = serde_json::json!({ "verdict": v.as_str() }).to_string();
                    write_response(&mut stream, "200 OK", "application/json", &body).await;
                }
                _ => write_response(&mut stream, "204 No Content", "application/json", "").await,
            }
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
        // C2: inbox routes use `project=` as the TARGET resource (compose
        // into another workspace, list that inbox, …) — preserve across
        // stamp (which rewrites project to the caller's own path).
        //
        // #36: restore BOTH keys to the same target token. Leaving
        // `project_path` as the stamped caller path while only restoring
        // `project` made need_project_path (which used to prefer
        // project_path) silently compose into the caller's own inbox.
        let inbox_target = if path.starts_with("/cli/inbox/") {
            params
                .get("project")
                .or_else(|| params.get("project_path"))
                .cloned()
                .filter(|s| !s.trim().is_empty())
        } else {
            None
        };
        // H8: domain/list treats `project=` as the agent-view switch.
        // Stamp always writes it for a cell — restore the CLIENT value
        // (or strip it) so hostmail domain list is the owner table.
        let domain_list_project = if path == "/cli/mail/domain/list" {
            Some(crate::mail_routes::client_project_param(&params))
        } else {
            None
        };
        params.insert("cell_session_id".to_string(), this_session_id.clone());
        stamp_principal(&mut params, &principal);
        if let Some(t) = inbox_target {
            params.insert("project".to_string(), t.clone());
            params.insert("project_path".to_string(), t);
        }
        if let Some(client_project) = domain_list_project {
            crate::mail_routes::restore_domain_list_audience(
                path,
                &mut params,
                client_project,
            );
        }

        // Permission cards: what the refusal wrapper reads after dispatch
        // (only while the global switch is on; nothing is copied otherwise).
        let teach_input = k2_core::permission::gate::server_on().then(|| {
            (params.clone(), body.clone(), principal.workspace_uuid.clone())
        });
        let path_owned = path.to_string();
        let method_owned = method.to_string();
        // F2: the `/cli/respond` arm pins the response to THIS session — the body
        // never names a session, so a cell can only ever write its OWN log. Clone
        // the bound session id into the worker (the other moved vars own their data).
        let sid_for_dispatch = this_session_id.clone();
        // The pure handlers lock the DB / sleep across inject windows, so run
        // them on the blocking pool exactly like the TCP dispatcher does.
        let (status, ctype, out): (String, &'static str, String) =
            tokio::task::spawn_blocking(move || {
                // Wave 0: install principal for body-based identity resolution
                // (mail/DNS POST handlers read JSON `project`, not the stamped
                // params map). Cleared when the closure returns.
                crate::caller_workspace::with_request_principal(Some(principal.clone()), || {
                    dispatch_cell_verb(
                        &method_owned,
                        &path_owned,
                        &params,
                        &body,
                        &principal,
                        &sid_for_dispatch,
                    )
                })
            })
                .await
                .unwrap_or_else(|e| {
                    (
                        "500 Internal Server Error".to_string(),
                        "application/json",
                        serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
                    )
                });

        // Permission cards (PRD rev 3.1 §3.1): an agent verb's refusal (or
        // hold) gains its `permission` object while both switches are on.
        let out = match &teach_input {
            Some((p, b, ws)) => crate::permission::walls::teach_cell(&status, method, path, p, b, ws, &out)
                .unwrap_or(out),
            None => out,
        };
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
        this_session_id: &str,
    ) -> (String, &'static str, String) {
        let is_post = method.eq_ignore_ascii_case("POST");
        // 0.44.4 — no state change over GET, mirrored from the TCP
        // dispatcher: a GET to a POST-only route (or a readable route's
        // write form) is 405 here too.
        if !is_post && crate::routes::route_policy::get_refused_params(path, params) {
            return (
                "405 Method Not Allowed".to_string(),
                "application/json",
                r#"{"error":"POST required"}"#
                    .to_string(),
            );
        }
        match path {
            // 0.44.4: formerly GET-shaped agent verbs (heartbeat CRUD/fire,
            // connections add/remove, mail attachment write) arrive as POST
            // with query/form params — same handler, same stamped params as
            // the GET form had. Ahead of the JSON-body mail POST arm.
            p if is_post
                && crate::routes::route_policy::is_post_shaped_get_verb(p)
                && crate::session_token::is_agent_verb(p)
                && p != "/cli/workspace/msg"
                && p != "/cli/awareness/publish" =>
            {
                from_cli(crate::cli::dispatch(p, params))
            }
            "/hook/complete" => (
                "200 OK".to_string(),
                "application/json",
                crate::hook_ingest::legacy_complete(params).to_string(),
            ),
            "/cli/workspace/msg" => from_cli(crate::cli::dispatch(path, params)),
            // Folder trust (SS10): an agent reads status and asks (the
            // card); deciding is the owner's (not an agent verb).
            crate::folder_trust::STATUS_PATH | crate::folder_trust::REQUEST_PATH => {
                from_cli(crate::folder_trust::dispatch(is_post, path, params, body, "agent"))
            }
            // 0.46.1 `k2 agent identity`: this passport's own color and
            // picture (the handler pins the workspace and refuses a name).
            // Same body cap as TCP (413).
            crate::agent_identity_routes::IDENTITY_PATH => {
                if body.len() > crate::agent_identity_routes::MAX_BODY {
                    return (
                        "413 Payload Too Large".to_string(),
                        "application/json",
                        r#"{"error":"too_large","message":"body over the limit"}"#.to_string(),
                    );
                }
                match crate::agent_identity_routes::caller_from_passport(this_session_id, principal) {
                    Ok(caller) => from_cli(crate::agent_identity_routes::handle(method, path, params, body, caller)),
                    Err(r) => from_cli(r),
                }
            }
            // Changes tab v1 (`k2 changes`): git status/diff/stage/commit/
            // discard on THIS passport's own workspace only. The workspace
            // comes from the passport, never the request; the handler
            // refuses a path (or repo) outside it with 403.
            p if crate::git_routes::is_agent_route(p) => {
                let own = match crate::caller_workspace::resolve_caller_workspace(Some(principal), None) {
                    Ok(c) => c.path,
                    Err(e) => {
                        return (
                            "403 Forbidden".to_string(),
                            "application/json",
                            serde_json::json!({ "error": e.hint(), "code": "forbidden" }).to_string(),
                        )
                    }
                };
                let actor = crate::routes::http::agent_actor_label(principal);
                from_cli(crate::git_routes::handle_agent_route_as(p, is_post, params, body, &own, &actor))
            }
            // K2 projects mutations + chat post over the cell socket: the
            // bound session's passport, gated by the workspace's "Manage
            // K2 projects" switch (0162) like the TCP arm. Reads fall to
            // the `is_agent_verb && !is_post` arm below.
            p if is_post && p.starts_with("/cli/project-group/") => {
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                from_cli(crate::project_group_routes::dispatch_post_as(p, body, &caller, "cell"))
            }
            // K2 projects reads over the cell socket: the chat itself is for
            // member projects only (0.46.2, the same check as the TCP arm);
            // everything else is the plain read chain.
            p if !is_post && p.starts_with("/cli/project-group/") => {
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                match crate::project_group_routes::chat_read_refusal(p, params, &caller) {
                    Some(r) => from_cli(r),
                    None => from_cli(crate::cli::dispatch(p, params)),
                }
            }
            // k2 sidecar v1: new / list / stop for THIS cell's session
            // (SC36). The handler applies the agent gate from the bound
            // session. Same body caps as TCP (413).
            // LLM login wallet over the cell socket: read-only for every
            // passport (the handler refuses mutations with owner_only).
            // Agent hire over the cell socket: the bound session's passport.
            // Only the agent-verb hire paths pass require_hook here.
            // Clone proposals: this cell's passport proposes, lists and
            // withdraws for its own workspace (approve/deny/finish never
            // pass require_hook on this socket).
            p if crate::clone_proposal_routes::is_route(p) => {
                if crate::clone_proposal_routes::is_post_route(p) && !is_post {
                    return ("405 Method Not Allowed".to_string(), "application/json", r#"{"error":"POST required"}"#.to_string());
                }
                if body.len() > crate::clone_proposal_routes::MAX_BODY {
                    return from_cli(crate::clone_proposal_routes::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::clone_proposal_routes::handle(p, who, body, params, "cell"))
            }
            // Permission cards (prd-agent-settings-cards-v1 rev 3.1 §2.4,
            // lane A1): this cell's passport asks for one named change.
            // The handler applies both switches (off → today's 403 body).
            p if crate::permission::routes::is_route(p) => {
                if body.len() > crate::permission::routes::MAX_BODY {
                    return from_cli(crate::permission::routes::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::permission::routes::handle(
                    p,
                    who,
                    is_post,
                    body,
                    params,
                    crate::permission::routes::Transport::Cell,
                ))
            }
            p if crate::hire_routes::is_route(p) => {
                if body.len() > crate::hire_routes::MAX_BODY {
                    return from_cli(crate::hire_routes::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::hire_routes::handle_cli(p, who, is_post, body, params, None))
            }
            // K2 compute nodes: the agent verbs for THIS cell's session
            // (owner verbs never pass require_hook on this socket). The two
            // node sockets are TCP-only.
            // RS-A15 (0.46.1): this cell's passport sets its own App's guests'
            // rooms within the owner's grant; the grant itself is the owner's.
            p if crate::apps::guest_rooms::ROUTES.contains(&p) => {
                if crate::apps::guest_rooms::POST_ROUTES.contains(&p) && !is_post {
                    return ("405 Method Not Allowed".to_string(), "application/json", r#"{"error":"POST required"}"#.to_string());
                }
                if body.len() > crate::apps::grace::MAX_BODY {
                    return from_cli(crate::apps::grace::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::apps::guest_rooms::handle(p, &who, is_post, body, params))
            }
            // prd-custom-app-push-v1 (0.46.4): `k2 app push …` for this
            // cell's passport (its own workspace's Apps; the handler checks).
            p if crate::app_push::owner_routes::is_route(p) => {
                if crate::app_push::owner_routes::is_post_route(p) && !is_post {
                    return ("405 Method Not Allowed".to_string(), "application/json", r#"{"error":"POST required"}"#.to_string());
                }
                if body.len() > crate::app_push::owner_routes::MAX_BODY {
                    return from_cli(crate::app_push::owner_routes::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::app_push::owner_routes::handle(p, who, body, params))
            }
            // 0.46.0 App grace (no preview flag): this cell's passport; the
            // handler scopes it to its own workspace's Apps and holds what
            // widens.
            p if crate::apps::grace::is_route(p) => {
                if crate::apps::grace::is_post_route(p) && !is_post {
                    return ("405 Method Not Allowed".to_string(), "application/json", r#"{"error":"POST required"}"#.to_string());
                }
                if body.len() > crate::apps::grace::MAX_BODY {
                    return from_cli(crate::apps::grace::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::apps::grace::handle(p, who, body, params))
            }
            // Room-scope R9/R16 (0.46.1): `k2 app members|self|usage` for
            // this cell's passport (its own room and its own Apps).
            p if crate::apps::surfaces::is_route(p) => {
                if crate::apps::surfaces::is_post_route(p) && !is_post {
                    return ("405 Method Not Allowed".to_string(), "application/json", r#"{"error":"POST required"}"#.to_string());
                }
                if body.len() > crate::apps::routes::MAX_BODY {
                    return from_cli(crate::apps::routes::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::apps::surfaces::handle(p, who, body, params))
            }
            // App runtime stage 1: this cell's passport (the handler applies
            // the workspace scope and "Allow this agent to manage Apps").
            p if crate::apps::routes::is_route(p) => {
                if crate::apps::routes::is_post_route(p) && !is_post {
                    return ("405 Method Not Allowed".to_string(), "application/json", r#"{"error":"POST required"}"#.to_string());
                }
                if body.len() > crate::apps::routes::MAX_BODY {
                    return from_cli(crate::apps::routes::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::apps::routes::handle(p, who, body, params))
            }
            // Usage v2: this cell's passport reads its own workspace's
            // tokens / turns and the limits (the handler scopes it).
            p if crate::usage_routes::is_route(p) => {
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::usage_routes::handle(p, &who, is_post, body, params))
            }
            // Sew on K2 (S29 K12): this cell's passport; the handler needs
            // "Manage Sew" for every mutation and refuses person-only steps.
            p if crate::sew::routes::is_route(p) => {
                if crate::sew::routes::is_post_route(p) && !is_post {
                    return ("405 Method Not Allowed".to_string(), "application/json", r#"{"error":"POST required"}"#.to_string());
                }
                if body.len() > crate::sew::routes::MAX_BODY {
                    return from_cli(crate::sew::routes::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::sew::routes::handle(p, who, body, params))
            }
            // `k2 db upgrade` (0.46.3 S5): an agent with "Allow this agent
            // to manage Postgres versions" checks, moves and rolls back its
            // own databases, and reads the move's progress, from its tab.
            // The cell socket is on this box, so the caller is local; the
            // handler's versions_access gate still refuses a sandboxed
            // session and everything the switch doesn't grant.
            p if p == "/cli/db/upgrade" => {
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                let who = crate::sql::pgmove::k2::classify(&who, true);
                from_cli(crate::sql::pgmove::k2::handle(who, is_post, body, params))
            }
            p if crate::compute_routes::is_route(p) && !crate::compute_routes::is_socket(p) => {
                if crate::compute_routes::is_post_route(p) && !is_post {
                    return ("405 Method Not Allowed".to_string(), "application/json", r#"{"error":"POST required"}"#.to_string());
                }
                if body.len() > crate::compute_routes::MAX_BODY {
                    return from_cli(crate::compute_routes::too_large());
                }
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::compute_routes::handle(p, who, body, params))
            }
            // Garden v3 data sources: EVERY `/cli/sources/*` path for THIS
            // cell's session (agent or K2 shell tab); the handler refuses
            // person steps with 403 person_only, never 404 [GV52].
            p if crate::sources_routes::is_route(p) => {
                if crate::sources_routes::is_post_route(p) && !is_post {
                    return from_cli(crate::sources_routes::method_not_allowed());
                }
                if body.len() > crate::sources_routes::MAX_BODY {
                    return from_cli(crate::sources_routes::too_large());
                }
                let who = crate::sources_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::sources_routes::handle(p, who, body, params))
            }
            // Garden data (contract-garden-data-v1 §4.7): EVERY
            // `/cli/zen/data/*` path for THIS cell's session (agent or K2
            // shell tab, agent rules); person routes answer 403
            // person_only. The widget door `/cli/zen/call` is never here.
            p if crate::zen_data_routes::is_agent_route(p) => {
                if crate::zen_data_routes::is_post_route(p) && !is_post {
                    return from_cli(crate::zen_data_routes::method_not_allowed());
                }
                if body.len() > crate::zen_data_routes::max_body(p) {
                    return from_cli(crate::zen_data_routes::too_large(p));
                }
                let who = crate::zen_data_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::zen_data_routes::handle(p, who, body, params, ""))
            }
            // K2 vault over the cell socket: the bound session's passport
            // (agent or K2 shell tab), agent-level (KV15).
            p if crate::vault_routes::is_route(p) => {
                if crate::vault_routes::is_post_route(p) && !is_post {
                    return from_cli(crate::cli_response::CliResponse::method_not_allowed());
                }
                if body.len() > crate::vault_routes::MAX_BODY {
                    return from_cli(crate::vault_routes::too_large());
                }
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                let vc = crate::vault::actor::from_caller(caller, Some(this_session_id.to_string()));
                from_cli(crate::vault_routes::handle(p, vc, body, params, "cell"))
            }
            // `k2 job` (item 5): this cell's agent manages its own
            // workspace's jobs (the handler pins the workspace).
            p if crate::jobs::routes::is_route(p) => {
                if crate::jobs::routes::is_post_route(p) && !is_post {
                    return from_cli(crate::cli_response::CliResponse::method_not_allowed());
                }
                if body.len() > crate::jobs::routes::MAX_BODY {
                    return from_cli(crate::jobs::routes::too_large());
                }
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                let vc = crate::vault::actor::from_caller(caller, Some(this_session_id.to_string()));
                from_cli(crate::jobs::routes::handle(p, vc, body, params))
            }
            p if crate::llm_accounts_routes::is_route(p) => {
                if crate::llm_accounts_routes::is_post_route(p) && !is_post {
                    return from_cli(crate::cli_response::CliResponse::method_not_allowed());
                }
                if body.len() > crate::llm_accounts_routes::MAX_BODY {
                    return from_cli(crate::llm_accounts_routes::too_large());
                }
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                from_cli(crate::llm_accounts_routes::handle(p, caller, body, params, "cell"))
            }
            // App users' wallets (0.46.3), the owner side, over the cell
            // socket: the same handler and gate as the main port. A passport
            // (agent or K2 shell tab) reaches `settings` only, for an App its
            // own workspace owns with switch 2 (read; fallback writes);
            // people, remove and personal agents answer `owner_only`. No
            // wallet route ever returns a person's item or secret to an agent.
            p if crate::person_wallet_routes::is_owner_route(p) => {
                if !is_post && !crate::person_wallet_routes::OWNER_GET.contains(&p) {
                    return from_cli(crate::cli_response::CliResponse::method_not_allowed());
                }
                if body.len() > crate::person_wallet_routes::MAX_BODY {
                    return from_cli(crate::person_wallet_routes::too_large());
                }
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                from_cli(crate::person_wallet_routes::handle_owner(p, caller, is_post, body, params, "cell"))
            }
            p if p.starts_with("/cli/sidecar/") && crate::sidecar_routes::is_route(p) => {
                if crate::sidecar_routes::is_post_route(p) && !is_post {
                    return from_cli(crate::cli_response::CliResponse::method_not_allowed());
                }
                if body.len() > crate::sidecar_routes::body_cap(p) {
                    return from_cli(crate::sidecar_routes::too_large(p));
                }
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                from_cli(crate::sidecar_routes::handle(p, caller, body, params, "cell"))
            }
            // Root helper (prd-k2-root-helper-v1 §8.3): `k2 root-helper
            // status` from inside a K2 terminal. Only status passes
            // require_hook on this socket; update/rollback are refused by the
            // handler for any passport (owner/admin steps) should one ever
            // arrive. Not in `cli::dispatch`, so without this arm the status
            // read fell through to its "route not found".
            p if crate::root_helper_routes::is_route(p) => {
                if crate::root_helper_routes::is_post_route(p) && !is_post {
                    return from_cli(crate::cli_response::CliResponse::method_not_allowed());
                }
                if body.len() > crate::root_helper_routes::MAX_BODY {
                    return from_cli(crate::root_helper_routes::too_large());
                }
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                from_cli(crate::root_helper_routes::handle(p, caller, body, params))
            }
            // App (skin) roster reads. On TCP these GETs admit the owner or
            // any validated passport (`owner_or_scoped_hook_auth`); here the
            // passport is already validated for this path. They live only in
            // the TCP dispatcher, not `cli::dispatch`. Manage POSTs still get
            // the cell sentinel below so the CLI uses TCP, where the
            // Agent-tab switch is checked.
            "/cli/skin/users" if !is_post => from_cli(crate::skin_routes::handle_users_get()),
            "/cli/skin/roles" if !is_post => from_cli(crate::skin_routes::handle_roles_get()),
            "/cli/skin/roles/suggest" if !is_post => from_cli(crate::app_mobile_routes::handle_roles_suggest()),
            "/cli/skin-tokens" if !is_post => from_cli(crate::skin_routes::handle_tokens_get()),
            // `/cli/respond` is POST-only (TCP answers a GET with 405).
            "/cli/respond" if !is_post => {
                from_cli(crate::cli_response::CliResponse::method_not_allowed())
            }
            "/cli/thread" => from_cli(crate::cli::dispatch(path, params)),
            // inbox-catchup-v1 (0.46.3): Thread entries read as this cell's
            // passport; project chat entries ask the same member check as
            // the project chat read; one catch-up per session at a time.
            path if is_post && path == crate::overlay_routes::catchup::ROUTE => {
                let caller = crate::sidecar_routes::caller_from_session(this_session_id, principal);
                let key = format!("cell:{this_session_id}");
                from_cli(crate::overlay_routes::catchup::handle(params, body, None, Some(&key), Some(&caller)))
            }
            path if is_post && path.starts_with("/cli/thread/") => {
                from_cli(crate::overlay_routes::dispatch_post(path, params, body))
            }
            // C2: peer-gated live terminal peek (`k2 read <ws>`).
            "/cli/terminal/read" => from_cli(crate::cli::dispatch(path, params)),
            // F2 (sandbox API): the in-cell agent's RESPONSE egress. Append the
            // message text to THIS session's response log (pinned to
            // `this_session_id` — the body can't name another session). The text
            // is read from `message` (fallback `text`); `final` ("1"/"true") marks
            // the turn's last line. Drained by `GET /v1/sandboxes/<id>/messages`.
            "/cli/respond" if is_post => {
                let text = params
                    .get("message")
                    .or_else(|| params.get("text"))
                    .cloned()
                    .unwrap_or_default();
                let final_ = params
                    .get("final")
                    .map(|v| {
                        let v = v.trim();
                        v == "1" || v.eq_ignore_ascii_case("true")
                    })
                    .unwrap_or(false);
                let seq = crate::sandbox_responses::append(this_session_id, text, final_);
                // S9 / Scout: arm work-completion reaper — same as the TCP
                // `/cli/respond` path in dispatcher.rs. Host sessions use this
                // UDS cell (K2_HOOK_SOCK); without on_respond, `--final` only
                // drains the message ring and never enters Grace → cells leak.
                if let Some(sid) = k2_core::session::SessionId::parse(this_session_id) {
                    crate::sandbox_reaper::on_respond(&sid, final_);
                }
                (
                    "200 OK".to_string(),
                    "application/json",
                    serde_json::json!({ "ok": true, "seq": seq }).to_string(),
                )
            }
            // Host-session / sandbox completion lifecycle — `k2 done`.
            // mark_complete only; NEVER appends to the respond drain.
            "/cli/session/complete" if is_post => {
                if let Some(sid) = k2_core::session::SessionId::parse(this_session_id) {
                    crate::sandbox_reaper::mark_complete(&sid);
                }
                if let Some(reason) = params.get("reason").filter(|s| !s.is_empty()) {
                    k2_core::log_debug!(
                        "[session-complete] session={} reason={}",
                        this_session_id,
                        reason
                    );
                }
                (
                    "200 OK".to_string(),
                    "application/json",
                    serde_json::json!({ "ok": true, "complete": true }).to_string(),
                )
            }
            // 0.40.34 browser-open: the staged k2-open shim's scoped-session
            // egress (curl --unix-socket + Bearer over K2_HOOK_SOCK). The
            // handler validates http/https-only + length cap and broadcasts
            // the app-level `open_url` event; no filesystem/exec surface, so
            // the session binding + the capability allowlist are the whole
            // gate. POST-only (feedback_post_only_route_guards) — a stray
            // GET 405s loudly here instead of falling to the TCP-fallback
            // sentinel.
            "/cli/browser/open-url" => {
                if !is_post {
                    return from_cli(crate::cli_response::CliResponse::method_not_allowed());
                }
                from_cli(crate::browser_routes::handle_open_url(params, body, "shim"))
            }
            // Design mode (D-A13): `k2 browser note` from a cell. The params
            // already carry this session's stamp (principal_bound), so the
            // post is the agent's own and only lands in its own workspace.
            "/cli/browser/notes" | "/cli/browser/notes/redeliver" => {
                if !is_post {
                    return from_cli(crate::cli_response::CliResponse::method_not_allowed());
                }
                let caller = crate::browser_routes::NotesCaller::Agent;
                if path == "/cli/browser/notes" {
                    from_cli(crate::browser_routes::handle_notes(params, body, &caller))
                } else {
                    from_cli(crate::browser_routes::handle_notes_redeliver(params, body, &caller))
                }
            }
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
            // Wave 0: agent mail verbs (identity forced from principal above +
            // with_request_principal). M5 extra-gated by the same helper as
            // TCP (flag on this principal's path). M6 stay owner_only.
            p if p.starts_with("/cli/mail/") && is_post => {
                if let Err(r) =
                    crate::mail_routes::mail_manage_authorized(p, false, Some(principal))
                {
                    return from_cli(r);
                }
                from_cli(crate::mail_routes::dispatch_post(p, body))
            }
            p if crate::mail_routes::is_mail_manage_surface(p) && !is_post => {
                if let Err(r) =
                    crate::mail_routes::mail_manage_authorized(p, false, Some(principal))
                {
                    return from_cli(r);
                }
                from_cli(crate::cli::dispatch(p, params))
            }
            // DNS K1: agent DNS verbs (principal-bound; toggle-gated in handlers).
            p if p.starts_with("/cli/dns/") && is_post => {
                from_cli(crate::dns_routes::dispatch_post(p, body))
            }
            p if (p == "/cli/certs/issue"
                || p == "/cli/certs/renew"
                || p == "/cli/domains/names"
                || p == "/cli/domains/names/remove")
                && is_post =>
            {
                from_cli(crate::domain_routes::dispatch_post(p, body))
            }
            // 0.45.7: `k2 publish status` / `subdomain create|target|rm`
            // from a sealed cell — the daemon makes the control-plane call
            // with the token it holds. Same handler and agent rules as TCP
            // (`connect_proxy`: own workspace's nested labels only, no
            // primary, no reserved label, rate-limited); the principal is
            // the validated passport, never the body. GET → 405 above.
            "/cli/connect/proxy" if is_post => {
                from_cli(crate::connect_proxy::handle(body, Some(principal)))
            }
            // Published services: POST run/start/stop/rm + subdomain
            // claim/unclaim. CLI `cli_post_json` / `cli_post` send JSON or
            // query params; cell handle_conn only merges form, so flatten
            // here then re-stamp so identity wins over body's `project`.
            // GET list/logs/leftovers stay on is_agent_verb && !is_post.
            // Self-host: do not preserve a foreign project=.
            p if p.starts_with("/cli/publish/") && is_post => {
                let mut params = params.clone();
                crate::routes::http::merge_json_object_params(&mut params, body);
                stamp_principal(&mut params, principal);
                let who = crate::compute_routes::who_from_cell(this_session_id, principal);
                from_cli(crate::publish_routes::dispatch_post_as(p, &params, who))
            }
            // PR1 federation dual-auth: peers / peer-roster / send over the
            // cell UDS. Dark when federation is off (same 404 as TCP). Principal
            // already installed by the outer with_request_principal so send
            // forces from_workspace. pair/confirm stay off is_agent_verb.
            p if p.starts_with("/cli/federation/") => {
                if !k2_core::federation::enabled() {
                    return (
                        "404 Not Found".to_string(),
                        "application/json",
                        r#"{"error":"not found"}"#.to_string(),
                    );
                }
                match p {
                    "/cli/federation/send" if is_post => {
                        from_cli(crate::federation_routes::handle_send(body))
                    }
                    "/cli/federation/peers" if !is_post => {
                        from_cli(crate::federation_routes::handle_peers())
                    }
                    "/cli/federation/peer-roster" if !is_post => {
                        let peer = params.get("peer").cloned().unwrap_or_default();
                        from_cli(crate::federation_routes::handle_peer_roster(&peer))
                    }
                    "/cli/federation/peer-rosters" if !is_post => {
                        let peer = params.get("peer").cloned();
                        from_cli(crate::federation_rosters::handle_peer_rosters(peer.as_deref()))
                    }
                    _ => (
                        "404 Not Found".to_string(),
                        "application/json",
                        r#"{"error":"route not served on cell socket"}"#.to_string(),
                    ),
                }
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
        // Graceful half-close (SHUT_WR): flush the reply, then signal a clean
        // EOF. A bare drop here races libkrun's vsock reverse path — an abrupt
        // close can drop the buffered reply before the guest reads it, so the
        // in-guest `k2`/proxy sees an empty response (and only the FIRST
        // request per cell ever appeared to work). With the half-close the
        // cell-channel carries unlimited sequential + concurrent requests
        // (proven on-box: 5 sequential + 3 concurrent, all 8 delivered).
        // libkrun itself has NO one-connection-per-cell limit; this was the
        // whole of it. Centralized here so all write_response call sites get it.
        let _ = stream.shutdown().await;
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
                cred_mode: crate::session_token::CredMode::default(),
                provider: crate::session_token::Provider::default(),
            }
        }

        #[test]
        fn authorized_when_uid_session_and_pane_all_match() {
            let v = vh("sid-1", "pane-1");
            // Non-microVM cell → no extra cell uid; only the daemon uid passes.
            assert!(cell_request_authorized(Some(&v), "sid-1", "pane-1", 501, 501, None));
        }

        #[test]
        fn rejected_on_uid_mismatch() {
            // peer-cred belt: a different uid is refused even with a valid
            // token bound to this cell + pane.
            let v = vh("sid-1", "pane-1");
            assert!(!cell_request_authorized(Some(&v), "sid-1", "pane-1", 999, 501, None));
        }

        #[test]
        fn rejected_when_token_bound_to_a_different_cell() {
            // Structural binding: cell B's token presented on cell A's socket.
            let v = vh("sid-OTHER", "pane-1");
            assert!(!cell_request_authorized(Some(&v), "sid-1", "pane-1", 501, 501, None));
        }

        #[test]
        fn rejected_on_pane_mismatch_or_empty() {
            let v = vh("sid-1", "pane-1");
            assert!(!cell_request_authorized(Some(&v), "sid-1", "pane-2", 501, 501, None));
            assert!(!cell_request_authorized(Some(&v), "sid-1", "", 501, 501, None));
        }

        #[test]
        fn rejected_when_token_did_not_validate() {
            assert!(!cell_request_authorized(None, "sid-1", "pane-1", 501, 501, None));
        }

        // ── Sandbox B2: per-session peer-uid widening (option a) ─────────────

        #[test]
        fn peer_uid_allowed_is_daemon_only_without_cell_uid() {
            // Non-microVM cell (`cell_uid == None`): ONLY the daemon uid —
            // byte-identical to pre-B2.
            assert!(peer_uid_allowed(501, 501, None));
            assert!(!peer_uid_allowed(999, 501, None));
        }

        #[test]
        fn peer_uid_allowed_widens_to_exactly_the_cell_uid_for_microvm() {
            // microVM cell: the daemon uid AND the one dedicated `k2cell` uid
            // (999) pass; an unrelated uid (1000) still does NOT.
            assert!(peer_uid_allowed(501, 501, Some(999)));
            assert!(peer_uid_allowed(999, 501, Some(999)));
            assert!(!peer_uid_allowed(1000, 501, Some(999)));
        }

        #[test]
        fn microvm_cell_authorizes_the_dropped_vmm_uid() {
            // The VMM (post priv-drop to k2cell=999) is the host-socket peer
            // for an in-guest verb; with a valid token bound to this cell it is
            // authorized when the cell is microVM-backed (cell_uid = Some(999)).
            let v = vh("sid-1", "pane-1");
            assert!(cell_request_authorized(Some(&v), "sid-1", "pane-1", 999, 501, Some(999)));
            // …but only with the EXACT cell uid — a stray uid is still refused.
            assert!(!cell_request_authorized(Some(&v), "sid-1", "pane-1", 1000, 501, Some(999)));
        }

        // ── generic-verb authorization (uid + session binding, no pane) ──

        #[test]
        fn verb_authorized_on_uid_and_session_match() {
            let v = vh("sid-1", "pane-1");
            // No paneId needed for a generic verb; only uid + session bind.
            assert!(cell_verb_authorized(Some(&v), "sid-1", 501, 501, None));
        }

        #[test]
        fn verb_rejected_on_wrong_uid() {
            let v = vh("sid-1", "pane-1");
            assert!(!cell_verb_authorized(Some(&v), "sid-1", 999, 501, None));
        }

        #[test]
        fn verb_rejected_on_wrong_session() {
            // A token minted for a different cell, replayed on this socket.
            let v = vh("sid-OTHER", "pane-1");
            assert!(!cell_verb_authorized(Some(&v), "sid-1", 501, 501, None));
        }

        #[test]
        fn verb_rejected_when_token_did_not_validate() {
            assert!(!cell_verb_authorized(None, "sid-1", 501, 501, None));
        }

        #[test]
        fn verb_authorized_for_dropped_vmm_uid_on_microvm_cell() {
            // Generic verb (msg/inbox/…) from the in-guest CLI: peer is the VMM
            // dropped to k2cell=999; the cell uid widening lets it through with
            // a session-bound token, but a stray uid is still refused.
            let v = vh("sid-1", "pane-1");
            assert!(cell_verb_authorized(Some(&v), "sid-1", 999, 501, Some(999)));
            assert!(!cell_verb_authorized(Some(&v), "sid-1", 1000, 501, Some(999)));
            // And the session binding still gates even the cell uid.
            let other = vh("sid-OTHER", "pane-1");
            assert!(!cell_verb_authorized(Some(&other), "sid-1", 999, 501, Some(999)));
        }

        // ── 0.40.34 browser-open verb over the cell socket ───────────────

        #[test]
        fn cell_dispatch_serves_browser_open_url_post_only() {
            let params: HashMap<String, String> = HashMap::new();
            let p = principal();
            // POST with the shim's form body → 200 (validated + broadcast).
            let (status, _, body) = dispatch_cell_verb(
                "POST",
                "/cli/browser/open-url",
                &params,
                b"url=https%3A%2F%2Fexample.com%2Fx",
                &p,
                "sid-1",
            );
            assert_eq!(status, "200 OK", "body: {body}");
            // Non-http target → 400, never a silent success.
            let (status, _, body) = dispatch_cell_verb(
                "POST",
                "/cli/browser/open-url",
                &params,
                b"url=file%3A%2F%2F%2Fetc%2Fpasswd",
                &p,
                "sid-1",
            );
            assert_eq!(status, "400 Bad Request", "body: {body}");
            // GET → 405 loudly (feedback_post_only_route_guards), not the
            // 404 TCP-fallback sentinel and never a mutation.
            let (status, _, _) = dispatch_cell_verb(
                "GET",
                "/cli/browser/open-url",
                &params,
                b"",
                &p,
                "sid-1",
            );
            assert_eq!(status, "405 Method Not Allowed");
        }

        // ── 0.40.127 published-services POST over the cell socket ─────

        #[test]
        fn cell_dispatch_serves_publish_run_post_not_cell_sentinel() {
            let _ = k2_core::db::init_for_tests();
            let ws_uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
            let ws_path = "/tmp/k2-cell-publish-run-W";
            std::fs::create_dir_all(ws_path).expect("create workspace dir");
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.execute(
                    "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
                    rusqlite::params![ws_uuid, ws_path, "W"],
                )
                .expect("insert project W");
            }
            let p = HookPrincipal {
                workspace_uuid: ws_uuid.to_string(),
                agent_address: "julie".to_string(),
            };
            let params: HashMap<String, String> = HashMap::new();
            // JSON body only — cell handle_conn does not merge JSON; the
            // POST arm must flatten or handle_run 400s Missing name.
            let body = serde_json::json!({
                "name": "julie-probe",
                "cmd": "true",
                "port": 38471,
                "noTunnel": true,
                "cwd": "/no/such/k2-publish-cwd",
                "project": "/evil",
            })
            .to_string();
            let (status, _, out) = dispatch_cell_verb(
                "POST",
                "/cli/publish/run",
                &params,
                body.as_bytes(),
                &p,
                "sid-1",
            );
            assert_ne!(
                status, "404 Not Found",
                "POST /cli/publish/run must not be the cell-sentinel; body={out}"
            );
            assert!(
                !out.contains("not served on cell socket"),
                "POST /cli/publish/run must not fall through to the TCP-fallback sentinel; body={out}"
            );
            assert!(
                !out.contains("Missing name"),
                "JSON name/cmd/port/noTunnel must be flattened into params; body={out}"
            );
            assert!(
                !out.contains("/evil"),
                "foreign JSON project must not win over stamp; body={out}"
            );
            // cwd flatten is visible: we passed a missing dir so the handler
            // 400s on cwd after name/cmd/port/project, not on Missing name.
            assert_eq!(status, "400 Bad Request", "body={out}");
            assert!(
                out.contains("cwd is not a directory"),
                "expected cwd validation after JSON flatten; body={out}"
            );

            let (gstatus, _, _) = dispatch_cell_verb(
                "GET",
                "/cli/publish/run",
                &params,
                b"",
                &p,
                "sid-1",
            );
            assert_eq!(gstatus, "405 Method Not Allowed");
        }

        /// 0.45.7: a sealed cell's agent publishes end to end — the
        /// connect proxy is served on the cell socket with the agent rules
        /// (own nested labels only), and the daemon adds the tunnel token.
        #[test]
        fn cell_dispatch_serves_connect_proxy_with_agent_rules() {
            use std::io::{Read, Write};
            let _home = crate::test_support::TempHome::new();
            let _ag = k2_core::test_env::EnvVar::remove("K2_AIRGAP");
            let _ = k2_core::db::init_for_tests();
            let token = "k2c_cell_test_token_51f0";
            std::fs::write(
                k2_core::tunnel::config::config_path(),
                format!(r#"{{"token":"{token}","subdomain":"rosson"}}"#),
            )
            .expect("seed tunnel.json");

            // Fake control plane: records each request, answers by method.
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind fake cp");
            let base = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let rec = seen.clone();
            std::thread::spawn(move || {
                while let Ok((mut sock, _)) = listener.accept() {
                    sock.set_read_timeout(Some(std::time::Duration::from_secs(5))).expect("timeout");
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = sock.read(&mut buf).expect("fake cp read");
                        if n == 0 {
                            break;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&raw).into_owned();
                        if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                            let clen = head
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().expect("clen"))
                                })
                                .unwrap_or(0);
                            if rest.len() >= clen {
                                break;
                            }
                        }
                    }
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    let (status, body) = if text.starts_with("GET ") {
                        ("200 OK", r#"{"subdomains":[{"label":"rosson","primary":true,"tier":"pro"}]}"#)
                    } else {
                        ("201 Created", r#"{"host":"cellapp.rosson.k2.dev"}"#)
                    };
                    rec.lock().expect("seen").push(text);
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    sock.write_all(resp.as_bytes()).expect("fake cp write");
                }
            });
            let _cp = k2_core::test_env::EnvVar::set("K2_CONNECT_BASE", &base);

            let ws_uuid = uuid::Uuid::new_v4().to_string();
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.execute(
                    "INSERT INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
                    rusqlite::params![ws_uuid, format!("/tmp/k2-cell-proxy-{ws_uuid}"), "cellws"],
                )
                .expect("insert project");
            }
            let p = HookPrincipal { workspace_uuid: ws_uuid.clone(), agent_address: "agent".into() };
            let params: HashMap<String, String> = HashMap::new();
            let call = |method: &str, body: &str| {
                dispatch_cell_verb(method, "/cli/connect/proxy", &params, body.as_bytes(), &p, "sid-1")
            };

            // Create: served (not the sentinel), token added by the daemon,
            // label claimed for the cell's workspace.
            let (status, _, out) =
                call("POST", r#"{"method":"POST","endpoint":"/subdomains","body":{"label":"cellapp","target":"localhost:3000"}}"#);
            assert_eq!(status, "200 OK", "body={out}");
            assert!(!out.contains("not served on cell socket"), "{out}");
            assert!(!out.contains(token), "token never in a response: {out}");
            let v: serde_json::Value = serde_json::from_str(&out).expect("json");
            assert_eq!(v["status"], serde_json::json!(201), "{v}");
            let writes: Vec<String> =
                seen.lock().expect("seen").iter().filter(|r| r.starts_with("POST ")).cloned().collect();
            assert_eq!(writes.len(), 1, "{writes:?}");
            assert!(
                writes[0].to_ascii_lowercase().contains(&format!("authorization: bearer {token}")),
                "{}",
                writes[0]
            );
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                assert_eq!(
                    k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, "cellapp").expect("owner_of"),
                    Some(ws_uuid.clone())
                );
            }

            // Agent rules hold on the cell socket too.
            let (status, _, out) =
                call("POST", r#"{"method":"DELETE","endpoint":"/subdomains/rosson"}"#);
            assert_eq!(status, "403 Forbidden", "primary: {out}");
            assert!(out.contains("primary_label"), "{out}");
            let (status, _, out) =
                call("POST", r#"{"method":"POST","endpoint":"/subdomains","body":{"label":"meet","target":"localhost:1"}}"#);
            assert_eq!(status, "400 Bad Request", "reserved: {out}");
            assert!(out.contains("reserved_label"), "{out}");
            let (status, _, out) = call("POST", r#"{"method":"GET","endpoint":"/api/account"}"#);
            assert_eq!(status, "400 Bad Request", "not allowed: {out}");
            assert!(out.contains("not_allowed"), "{out}");
            let (status, _, _) = call("GET", "");
            assert_eq!(status, "405 Method Not Allowed");
            let writes = seen.lock().expect("seen").iter().filter(|r| !r.starts_with("GET ")).count();
            assert_eq!(writes, 1, "refused calls sent nothing");
        }

        /// 0.46.1 `k2 agent identity` over the cell socket: served (not the
        /// TCP sentinel), pinned to the passport's own workspace, color and
        /// picture only.
        #[test]
        fn cell_dispatch_serves_own_agent_identity() {
            let _home = crate::test_support::TempHome::new();
            let _ = k2_core::db::init_for_tests();
            let own = uuid::Uuid::new_v4().to_string();
            let other = uuid::Uuid::new_v4().to_string();
            let own_handle = format!("cellself{}", &own[..8]);
            let other_handle = format!("cellother{}", &other[..8]);
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                for (id, handle) in [(&own, &own_handle), (&other, &other_handle)] {
                    conn.execute(
                        "INSERT INTO projects (id, path, name, handle) VALUES (?1, ?2, ?3, ?3)",
                        rusqlite::params![id, format!("/tmp/k2-cell-identity-{id}"), handle],
                    )
                    .expect("insert project");
                }
            }
            let p = HookPrincipal { workspace_uuid: own.clone(), agent_address: "agent".into() };
            let params: HashMap<String, String> = HashMap::new();
            let call = |method: &str, body: &str| {
                dispatch_cell_verb(method, "/cli/agent/identity", &params, body.as_bytes(), &p, "sid-cell")
            };
            let color = |id: &str| -> String {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.query_row("SELECT color FROM projects WHERE id = ?1", rusqlite::params![id], |r| r.get(0))
                    .expect("color")
            };

            // Read: itself, with no workspace named.
            let (status, _, out) = call("GET", "");
            assert_eq!(status, "200 OK", "{out}");
            let v: serde_json::Value = serde_json::from_str(&out).expect("json");
            assert_eq!(v["handle"], own_handle.as_str(), "{v}");
            assert_eq!(v["canEditIdentity"], true, "{v}");
            assert_eq!(v["canEditHandle"], false, "{v}");

            // Write its own color.
            let (status, _, out) = call("POST", r##"{"color":"#123456"}"##);
            assert_eq!(status, "200 OK", "{out}");
            assert!(!out.contains("not served on cell socket"), "{out}");
            assert_eq!(color(&own), "#123456");

            // Another workspace and a name are refused; nothing written.
            let before = color(&other);
            let (status, _, out) = call("POST", &format!(r##"{{"workspace":"{other_handle}","color":"#654321"}}"##));
            assert_eq!(status, "403 Forbidden", "{out}");
            assert!(out.contains("not_your_agent"), "{out}");
            assert_eq!(color(&other), before, "nothing written to another workspace");
            let (status, _, out) = call("POST", r#"{"displayName":"Renamed"}"#);
            assert_eq!(status, "403 Forbidden", "{out}");
            assert!(out.contains("agent_cannot_rename"), "{out}");

            // The handle door never answers on the cell socket for a passport.
            let (status, _, out) =
                dispatch_cell_verb("POST", "/cli/agent/handle", &params, br#"{"handle":"x"}"#, &p, "sid-cell");
            assert_ne!(status, "200 OK", "{out}");
        }

        #[test]
        fn cell_dispatch_subdomain_claim_stamps_self_not_foreign() {
            let _ = k2_core::db::init_for_tests();
            let docs = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
            let sales = "cccccccc-dddd-eeee-ffff-000000000001";
            let docs_path = "/tmp/k2-cell-docs-claim";
            let sales_path = "/tmp/k2-cell-sales-claim";
            std::fs::create_dir_all(docs_path).expect("docs dir");
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.execute(
                    "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
                    rusqlite::params![docs, docs_path, "Documents"],
                )
                .expect("insert docs");
                conn.execute(
                    "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
                    rusqlite::params![sales, sales_path, "Sales"],
                )
                .expect("insert sales");
                k2_core::db::schema::SubdomainWorkspace::self_stamp(
                    &conn,
                    "sales-portal",
                    sales,
                    Some("localhost:1"),
                )
                .expect("sales sticker");
            }
            let p = HookPrincipal {
                workspace_uuid: docs.to_string(),
                agent_address: "docs-agent".to_string(),
            };
            let params: HashMap<String, String> = HashMap::new();
            let body = serde_json::json!({
                "label": "docs-portal",
                "target": "localhost:3000",
                "project": sales_path,
            })
            .to_string();
            let (status, _, out) = dispatch_cell_verb(
                "POST",
                "/cli/publish/subdomain/claim",
                &params,
                body.as_bytes(),
                &p,
                "sid-1",
            );
            assert_eq!(status, "200 OK", "self stamp must succeed; body={out}");
            assert!(out.contains("docs-portal"), "body={out}");

            let steal = serde_json::json!({
                "label": "sales-portal",
                "target": "localhost:9",
                "project": sales_path,
            })
            .to_string();
            let (sstatus, _, sout) = dispatch_cell_verb(
                "POST",
                "/cli/publish/subdomain/claim",
                &params,
                steal.as_bytes(),
                &p,
                "sid-1",
            );
            assert_eq!(sstatus, "409 Conflict", "must not steal Sales; body={sout}");
            let sv: serde_json::Value = serde_json::from_str(&sout).expect("json refusal");
            assert_eq!(sv["code"], "label_owned_by_other_workspace", "body={sout}");
            // N6: the agent hears which agent holds it (to k2 msg it),
            // never the workspace's id or folder path.
            assert_eq!(sv["owner"]["agent"], "Sales", "body={sout}");
            assert!(sv["owner"]["address"].is_string(), "body={sout}");
            assert!(!sout.contains(sales) && !sout.contains(sales_path), "no id or path leak; body={sout}");

            // An agent cannot transfer, even when it asks to.
            let xfer = serde_json::json!({
                "label": "sales-portal",
                "target": "localhost:9",
                "transfer": true,
            })
            .to_string();
            let (xstatus, _, xout) = dispatch_cell_verb(
                "POST",
                "/cli/publish/subdomain/claim",
                &params,
                xfer.as_bytes(),
                &p,
                "sid-1",
            );
            assert_eq!(xstatus, "403 Forbidden", "agent transfer; body={xout}");
            assert!(xout.contains("transfer_owner_only"), "body={xout}");
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let map = k2_core::db::schema::SubdomainWorkspace::map(&conn).unwrap();
                assert_eq!(
                    map.get("sales-portal").map(String::as_str),
                    Some(sales),
                    "foreign 0074 must be unchanged"
                );
                assert_eq!(
                    map.get("docs-portal").map(String::as_str),
                    Some(docs),
                    "self stamp must land on the cell workspace"
                );
                let _ = k2_core::db::schema::SubdomainWorkspace::unclaim(&conn, "sales-portal");
                let _ = k2_core::db::schema::SubdomainWorkspace::unclaim(&conn, "docs-portal");
            }

            let (gstatus, _, _) = dispatch_cell_verb(
                "GET",
                "/cli/publish/subdomain/claim",
                &params,
                b"",
                &p,
                "sid-1",
            );
            assert_eq!(gstatus, "405 Method Not Allowed");
        }

        // ── every agent GET verb is served on the cell socket ────────────

        /// `k2 root-helper status` from a K2 terminal answered "route not
        /// found": the path passed `is_agent_verb`, so the cell handed it to
        /// `cli::dispatch`, which never knew root-helper routes. Sweep: for
        /// EVERY classified `/cli` route with a GET form that a passport may
        /// call, the cell socket must not answer `cli::dispatch`'s "route
        /// not found" (the CLI only falls back to TCP on the cell sentinel,
        /// so that 404 is a dead end) nor the cell sentinel itself (a GET
        /// agent verb belongs on this socket).
        /// App users' wallets on the cell socket (0.46.3): an agent reaches
        /// its own App's wallet settings only, with switch 2, through the
        /// main port's gate; people, personal agents and remove are
        /// `owner_only`, and nothing returns a person's item.
        #[test]
        fn wallet_settings_on_the_cell_socket_use_the_main_port_gate() {
            let _home = crate::test_support::TempHome::new();
            let _db = k2_core::db::scoped_for_test();
            let p = principal();
            let ws = p.workspace_uuid.clone();
            let app = "svc-cell-wallet";
            let q: HashMap<String, String> = [("app".to_string(), app.to_string())].into_iter().collect();
            let code = |b: &str| -> String {
                serde_json::from_str::<serde_json::Value>(b).expect("json")["error"]["code"].as_str().unwrap_or("").to_string()
            };
            // No switch 2 (not even an App of its own): refused, never a 404.
            let (status, _, body) = dispatch_cell_verb("GET", "/cli/app/wallet/settings", &q, b"", &p, "sid-wallet");
            assert!(status.starts_with("403"), "{status} {body}");
            assert_eq!(code(&body), "switch2_required", "{body}");
            // Its workspace owns the App with switch 1 + 2: it reads the setting
            // and may set the fallback (never `enabled`).
            {
                let h = k2_core::db::shared();
                let c = h.lock();
                c.execute("INSERT INTO projects (id, name, path, handle, agents_can_manage_skin, agents_can_manage_apps_full) VALUES (?1, ?1, '/tmp/cw', ?1, 1, 1)", rusqlite::params![ws]).unwrap();
                c.execute(
                    "INSERT INTO apps (app_id, service_id, owner_workspace, state, created_by, created_at, updated_at) VALUES ('app-cw', ?1, ?2, 'running', 'test', 1, 1)",
                    rusqlite::params![app, ws],
                )
                .unwrap();
            }
            let (status, _, body) = dispatch_cell_verb("GET", "/cli/app/wallet/settings", &q, b"", &p, "sid-wallet");
            assert!(status.starts_with("200"), "{status} {body}");
            assert!(body.contains("\"fallback\":false"), "{body}");
            let (status, _, body) = dispatch_cell_verb(
                "POST",
                "/cli/app/wallet/settings",
                &HashMap::new(),
                serde_json::json!({"app": app, "fallback": true}).to_string().as_bytes(),
                &p,
                "sid-wallet",
            );
            assert!(status.starts_with("200"), "{status} {body}");
            let (status, _, body) = dispatch_cell_verb(
                "POST",
                "/cli/app/wallet/settings",
                &HashMap::new(),
                serde_json::json!({"app": app, "enabled": true}).to_string().as_bytes(),
                &p,
                "sid-wallet",
            );
            assert!(status.starts_with("403"), "{status} {body}");
            assert_eq!(code(&body), "owner_only", "{body}");
            // Everything else on the owner side: owner_only.
            for path in ["/cli/app/wallet/people", "/cli/app/wallet/personal-agents"] {
                let (status, _, body) = dispatch_cell_verb("GET", path, &q, b"", &p, "sid-wallet");
                assert!(status.starts_with("403"), "{path}: {status} {body}");
                assert_eq!(code(&body), "owner_only", "{path}: {body}");
            }
        }

        #[test]
        fn every_agent_get_verb_is_known_on_the_cell_socket() {
            // The sweep calls the mail GET verbs, which can read the
            // singleton mail_server row: hold its lock so no parallel
            // test's row (and mock Stalwart) leaks in (soak 2026-10-14).
            let _mail = crate::mail::mail_server_test_lock();
            let _home = crate::test_support::TempHome::new();
            let _ = k2_core::db::init_for_tests();
            let p = principal();
            let params: HashMap<String, String> = HashMap::new();
            let mut checked = 0usize;
            let mut misses = Vec::new();
            for r in crate::routes::route_policy::ROUTES {
                if r.get.is_none() || !crate::session_token::is_agent_verb(r.path) {
                    continue;
                }
                checked += 1;
                let (status, _, body) =
                    dispatch_cell_verb("GET", r.path, &params, b"", &p, "sid-sweep");
                if body.contains("route not found") || body.contains("not served on cell socket") {
                    misses.push(format!("{} -> {status} {body}", r.path));
                }
            }
            assert!(checked >= 100, "sweep must cover the agent GET verbs, saw {checked}");
            assert!(misses.is_empty(), "agent GET verbs unknown on the cell socket:\n{}", misses.join("\n"));
        }

        /// 0.46.3 box usage (prd-app-resources-disk-guard-v1 §12): the
        /// existing App-route arm serves `GET /cli/app/resources` to the
        /// cell's passport, and refuses GET on its two POST rows.
        #[test]
        fn cell_dispatch_serves_box_usage_and_its_post_rows_are_post_only() {
            let _home = crate::test_support::TempHome::new();
            let _ = k2_core::db::init_for_tests();
            k2_core::deploy::set_enabled(true);
            let p = principal();
            let params: HashMap<String, String> = HashMap::new();
            let (status, _, body) = dispatch_cell_verb("GET", "/cli/app/resources", &params, b"", &p, "sid-res");
            assert!(!body.contains("route not found") && !body.contains("not served on cell socket"), "{status} {body}");
            let v: serde_json::Value = serde_json::from_str(&body).expect("JSON body");
            assert!(v.get("ok").is_some() || v["error"]["code"].is_string(), "the App arm answered: {status} {body}");
            for path in ["/cli/app/resources/settings", "/cli/app/resources/rescan"] {
                let (status, _, body) = dispatch_cell_verb("GET", path, &params, b"", &p, "sid-res");
                assert_eq!(status, "405 Method Not Allowed", "{path}: {body}");
            }
        }

        #[test]
        fn cell_dispatch_serves_app_roster_reads_and_respond_get_is_405() {
            let _home = crate::test_support::TempHome::new();
            let p = principal();
            let params: HashMap<String, String> = HashMap::new();
            for (path, key) in [
                ("/cli/skin/users", "users"),
                ("/cli/skin/roles", "roles"),
                ("/cli/skin-tokens", "tokens"),
            ] {
                let (status, _, body) = dispatch_cell_verb("GET", path, &params, b"", &p, "sid-1");
                assert_eq!(status, "200 OK", "{path}: {body}");
                let v: serde_json::Value = serde_json::from_str(&body).expect("JSON body");
                assert!(v.get(key).is_some(), "{path}: want `{key}` in {body}");
                // Manage POSTs keep the sentinel: the CLI retries over TCP,
                // where the Agent-tab switch is checked.
                let (status, _, body) = dispatch_cell_verb("POST", path, &params, b"{}", &p, "sid-1");
                assert_eq!(status, "404 Not Found", "{path} POST: {body}");
                assert!(body.contains("not served on cell socket"), "{path} POST: {body}");
            }
            let (status, _, _) = dispatch_cell_verb("GET", "/cli/respond", &params, b"", &p, "sid-1");
            assert_eq!(status, "405 Method Not Allowed");
        }

        #[test]
        fn cell_dispatch_serves_root_helper_status_and_refuses_update() {
            let _home = crate::test_support::TempHome::new();
            let p = principal();
            let params: HashMap<String, String> = HashMap::new();
            let (status, _, body) =
                dispatch_cell_verb("GET", "/cli/root-helper/status", &params, b"", &p, "sid-rh");
            assert_eq!(status, "200 OK", "body: {body}");
            let v: serde_json::Value = serde_json::from_str(&body).expect("status JSON");
            assert!(v.get("state").is_some(), "status body has state: {body}");
            for path in ["/cli/root-helper/update", "/cli/root-helper/rollback"] {
                let (status, _, body) = dispatch_cell_verb("POST", path, &params, b"{}", &p, "sid-rh");
                assert_eq!(status, "403 Forbidden", "{path}: {body}");
                assert!(body.contains("owner_only"), "{path}: {body}");
                let (status, _, _) = dispatch_cell_verb("GET", path, &params, b"", &p, "sid-rh");
                assert_eq!(status, "405 Method Not Allowed", "{path}");
            }
            // The socket's auth gate still keeps update/rollback off it.
            assert!(crate::session_token::is_agent_verb("/cli/root-helper/status"));
            assert!(!crate::session_token::is_agent_verb("/cli/root-helper/update"));
            assert!(!crate::session_token::is_agent_verb("/cli/root-helper/rollback"));
        }

        /// Garden v3 data sources [GV52]: every `/cli/sources/*` path is an
        /// agent verb and is answered by the sources handler on the cell
        /// socket (never cli::dispatch's 404 nor the cell sentinel); a GET
        /// to a POST-only path is 405.
        #[test]
        fn cell_dispatch_serves_every_sources_route() {
            let _home = crate::test_support::TempHome::new();
            let _ = k2_core::db::init_for_tests();
            let p = principal();
            let params: HashMap<String, String> = HashMap::new();
            for path in crate::sources_routes::ROUTES {
                assert!(crate::session_token::is_agent_verb(path), "{path} must be an agent verb");
                let method = if crate::sources_routes::is_post_route(path) { "POST" } else { "GET" };
                let (status, _, body) = dispatch_cell_verb(method, path, &params, b"{}", &p, "sid-src");
                assert!(
                    !body.contains("route not found") && !body.contains("not served on cell socket"),
                    "{path}: {status} {body}"
                );
                let _: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{path}: {e}: {body}"));
                if crate::sources_routes::is_post_route(path) {
                    let (status, _, _) = dispatch_cell_verb("GET", path, &params, b"", &p, "sid-src");
                    assert_eq!(status, "405 Method Not Allowed", "{path}");
                }
            }
        }

        /// Garden data (contract-garden-data-v1 §4.7): every
        /// `/cli/zen/data/*` path is an agent verb and is answered by the
        /// Garden data handler on the cell socket (never cli::dispatch's 404
        /// nor the cell sentinel), with a JSON body; a GET to a POST row is
        /// 405. The widget door `/cli/zen/call` is not an agent verb.
        #[test]
        fn cell_dispatch_serves_every_zen_data_route() {
            let _home = crate::test_support::TempHome::new();
            let _ = k2_core::db::init_for_tests();
            let p = principal();
            let params: HashMap<String, String> = HashMap::new();
            assert!(!crate::session_token::is_agent_verb(crate::zen_data_routes::CALL_ROUTE));
            for path in crate::zen_data_routes::ROUTES.iter().filter(|p| crate::zen_data_routes::is_agent_route(p)) {
                assert!(crate::session_token::is_agent_verb(path), "{path} must be an agent verb");
                let method = if crate::zen_data_routes::is_post_route(path) { "POST" } else { "GET" };
                let (status, _, body) = dispatch_cell_verb(method, path, &params, b"{}", &p, "sid-gd");
                assert!(
                    !body.contains("route not found") && !body.contains("not served on cell socket"),
                    "{path}: {status} {body}"
                );
                let v: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{path}: {e}: {body}"));
                assert!(v.get("error").is_some() || status == "200 OK", "{path}: {status} {body}");
                if crate::zen_data_routes::is_post_route(path) {
                    let (status, _, _) = dispatch_cell_verb("GET", path, &params, b"", &p, "sid-gd");
                    assert_eq!(status, "405 Method Not Allowed", "{path}");
                }
            }
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

        /// Sandbox P1 — Finding 1: the operand workspace is FORCED to the
        /// principal's own workspace (overwriting a body-supplied `project`),
        /// and an unresolvable principal FAILS CLOSED (both keys removed).
        #[test]
        fn stamp_forces_project_over_body() {
            let _ = k2_core::db::init_for_tests();

            // A real workspace W that the principal owns (resolvable by uuid).
            let ws_uuid = "11111111-2222-3333-4444-555555555555";
            let ws_path = "/tmp/k2-stamp-force-W";
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.execute(
                    "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
                    rusqlite::params![ws_uuid, ws_path, "W"],
                )
                .expect("insert project W");
            }
            let owner = HookPrincipal {
                workspace_uuid: ws_uuid.to_string(),
                agent_address: "agent-W".to_string(),
            };

            // The body tries to operate on /evil — must be FORCED to W's path.
            let mut params: HashMap<String, String> = HashMap::new();
            params.insert("project".to_string(), "/evil-other-workspace".to_string());
            params.insert("project_path".to_string(), "/evil-other-workspace".to_string());
            stamp_principal(&mut params, &owner);
            assert_eq!(
                params.get("project").map(String::as_str),
                Some(ws_path),
                "body `project` must be FORCED to the principal's own workspace"
            );
            assert_eq!(
                params.get("project_path").map(String::as_str),
                Some(ws_path),
                "body `project_path` must be FORCED to the principal's own workspace"
            );
            // Chat attribution is the workspace HANDLE (the street
            // address), not the display name and not the technical
            // agent_address / passport key. 3c8ed526 (D7/D18) split
            // Agent Name (display "W") from Handle (slug "w").
            assert_eq!(params.get("from").map(String::as_str), Some("w"));

            // Fail-closed: an unresolvable principal REMOVES both keys even
            // when the body supplied them.
            let ghost = HookPrincipal {
                workspace_uuid: "00000000-0000-0000-0000-000000000000".to_string(),
                agent_address: "ghost".to_string(),
            };
            let mut p2: HashMap<String, String> = HashMap::new();
            p2.insert("project".to_string(), "/evil".to_string());
            p2.insert("project_path".to_string(), "/evil".to_string());
            stamp_principal(&mut p2, &ghost);
            assert!(
                p2.get("project").is_none(),
                "fail-closed must remove `project` for an unresolvable principal"
            );
            assert!(
                p2.get("project_path").is_none(),
                "fail-closed must remove `project_path` for an unresolvable principal"
            );
        }
    }
}
