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

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    use k2_core::log_debug;
    use k2_core::session::SessionId;

    use crate::session_token::ValidatedHook;

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

    /// Read the peeked request off the connection, authenticate it
    /// structurally + by scoped token, and serve `/hook/complete`.
    async fn handle_conn(mut stream: UnixStream, this_session_id: String, self_uid: u32) {
        // peer-cred belt (best-effort): read the connecting uid via the raw
        // fd, then carry on with the SAME tokio stream (no consume/rebuild).
        let peer_uid = peer_uid_of(&stream).unwrap_or(u32::MAX);

        // Hook calls are tiny GETs (`curl -sG` → query string + headers, no
        // body); a single read captures the whole request head.
        let mut buf = vec![0u8; 8192];
        let n = match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let req = String::from_utf8_lossy(&buf[..n]);

        let first_line = req.lines().next().unwrap_or("");
        let target = first_line.split_whitespace().nth(1).unwrap_or("");
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p, q),
            None => (target, ""),
        };

        // Bearer header preferred (kept out of logs/transcripts); `?token=`
        // is the fallback. The credential is the per-cell SCOPED token from
        // the child's `K2_HOOK_TOKEN` env.
        let params = crate::routes::http::parse_params(path, query);
        let req_token = params.get("token").cloned().unwrap_or_default();
        let presented = crate::routes::http::extract_bearer(&req)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(req_token);
        let req_pane = params.get("paneId").cloned().unwrap_or_default();

        let (status, body): (&str, String) = if path == "/hook/complete" {
            // require_hook = capability allowlist + scoped-token validation.
            let validated = crate::session_token::require_hook(&presented, path);
            if cell_request_authorized(
                validated.as_ref(),
                &this_session_id,
                &req_pane,
                peer_uid,
                self_uid,
            ) {
                ("200 OK", k2_core::agent_hooks::handle_hook_complete(&params).to_string())
            } else {
                (
                    "403 Forbidden",
                    r#"{"error":"Invalid or missing auth token"}"#.to_string(),
                )
            }
        } else {
            // Phase 1 serves ONLY the lifecycle hook over the cell socket;
            // everything else falls back to loopback-TCP (404 here).
            (
                "404 Not Found",
                r#"{"error":"route not served on cell socket"}"#.to_string(),
            )
        };

        write_response(&mut stream, status, &body).await;
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

    async fn write_response(stream: &mut UnixStream, status: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
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
    }
}
