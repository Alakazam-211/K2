//! HTTP route dispatcher for the k2so-daemon binary.
//!
//! Pre-0.39.0 this entire function body lived inline in `main.rs`'s
//! `handle_connection`. Extraction is a mechanical move — every match
//! arm, every per-route comment, every method-gating guard is preserved
//! verbatim. HTTP framing helpers (`send_response`, `send_cors_preflight`,
//! `require_post`, `read_post_body`, `token_ok`, `parse_params`) live in
//! the sibling `routes::http` module; the dispatch sub-helpers
//! (`handle_archive_orphans`, `dispatch_unit6_post`) live at the bottom
//! of this file.
//!
//! `/cli/*` POST routes use `super::http::require_post` to enforce
//! method gating per [[feedback_post_only_route_guards]] memory; the
//! starts_with arms (`/cli/git/`, `/cli/workspaces/`,
//! `/cli/focus-groups/`, `/cli/sections/`, `/cli/workspace-layouts/`,
//! `/cli/timer/`, `/cli/presets/`, `/cli/window-state/`,
//! `/cli/projects/`, `/cli/fs/`, `/cli/chat/`, `/cli/themes/`,
//! `/cli/skill-layers/`, `/cli/review-checklist/`, `/cli/inbox/`)
//! inherit the gate from the top-level `method != "GET" && !(is_post &&
//! post_allowed)` 405 short-circuit. See `feedback_post_only_route_guards`
//! for the full rationale.

use std::time::Duration;

use tokio::io::AsyncReadExt;
// #651: `flush()` on the TcpStream before the restart handler triggers
// graceful shutdown — the 200 MUST be on the wire before the process dies.
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// #67 — resolve the EFFECTIVE remote-instruct opt-in for the workspace
/// that owns the live PTY `session_id`.
///
/// Maps `session_id` → live session → its spawn cwd → the matching
/// `projects.path` row's per-workspace flag, OR'd with the app-level
/// master (`remote_instruct_allowed_for_path`). This is the value fed to
/// [`super::http::authorize_send_message`]; it gates ONLY the connect-user
/// path (the owner token is always allowed).
///
/// Fail-CLOSED: an unparsable/unknown session id, a session with no cwd,
/// or an unregistered workspace → `false` (deny the connect-user). The
/// owner is unaffected because `authorize_send_message` short-circuits on
/// the owner token before consulting this flag.
fn remote_instruct_opt_in_for_session(session_id: &str) -> bool {
    // App-level master opts in everything (back-compat) — short-circuit
    // before any session lookup so an owner-disabled-but-app-on host still
    // works even mid-spawn.
    if k2_core::app_settings::load().allow_remote_instruct {
        return true;
    }
    let Some(sid) = k2_core::session::SessionId::parse(session_id) else {
        return false;
    };
    let Some(live) = crate::session_lookup::lookup_by_session_id(&sid) else {
        return false;
    };
    let cwd = live.cwd();
    if cwd.is_empty() {
        return false;
    }
    // App-level already checked above; this reads ONLY the per-workspace
    // flag for the resolved cwd (the OR-with-master is handled by the
    // early return above, but the core helper also re-checks it harmlessly).
    k2_core::workspace::settings::remote_instruct_allowed_for_path(&cwd)
}

/// Outcome of [`handle_one_request`] — tells the outer keep-alive loop
/// whether to wait for the next request on this socket or tear it down.
///
/// **`KeepAlive`** — a regular HTTP response was sent and the client
/// didn't request close. Loop and serve the next request on the same
/// socket.
///
/// **`Done`** — close the socket. Reasons:
/// - WS upgrade handed off; the WS handler owned read/write semantics
///   for the lifetime of the upgraded connection.
/// - Client sent `Connection: close` in the request headers.
/// - Auth failure, malformed request, or any other early-exit path
///   (closing is the safe default — if a client is sending broken
///   requests we don't want to amplify the problem by keeping the
///   connection alive).
/// - Idle-timeout while waiting for the next request.
enum DispatchOutcome {
    /// HTTP request handled; outer loop should poll for the next one.
    KeepAlive,
    /// Close the socket — WS handoff, client close request, error, or
    /// idle timeout.
    Done,
}

/// How long to wait for the next request on an idle keep-alive socket
/// before closing it. 60 s is comfortably longer than the renderer's
/// slowest poll cycle (~30 s) so idle connections recycle without
/// hoarding fds, but short enough that abandoned sockets don't sit
/// indefinitely.
const KEEP_ALIVE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Cap on requests served per TCP connection. A pathological client
/// loop can't infinitely hold a single socket open. 10 000 is plenty
/// for any sane session; at 2 s/poll that's ~5.5 hours of nonstop
/// polling on one socket before recycle.
const KEEP_ALIVE_MAX_REQUESTS: u32 = 10_000;

/// Cap on the request HEAD (request line + headers) the dispatcher will
/// buffer/parse. 16KB gives legacy query-string clients 4× the old 4KB
/// peek window; anything larger gets an explicit `414` instead of the
/// pre-0.39.45 silent query truncation (GH #35/#37). Long values belong
/// in POST bodies, which `read_post_body` streams without this cap.
const REQUEST_HEAD_MAX: usize = 16 * 1024;

/// Serve one TCP connection, looping over requests on the same socket
/// (HTTP/1.1 keep-alive).
///
/// **0.39.7 (Issue #2):** pre-0.39.7 this function served exactly ONE
/// request per connection because [`super::http::send_response`] hard-
/// coded `Connection: close`. Combined with the renderer's ~12 different
/// `setInterval` HTTP polls, every fetch was a fresh TCP socket. macOS's
/// WKWebView Networking process has a soft `RLIMIT_NOFILE = 256`; over
/// ~50 min the trickle of `CLOSE_WAIT` sockets the WebView didn't
/// close fast enough filled the fd table, and the UI progressively
/// locked up.
///
/// Now: the dispatcher loops, calling [`handle_one_request`] until the
/// client sends `Connection: close`, the connection idles out
/// ([`KEEP_ALIVE_IDLE_TIMEOUT`]), or a WS handoff or fatal error closes
/// it.
///
/// `/events` (and the other WS endpoints) are handled the same way as
/// before — on the FIRST request only. After a WS handoff the loop
/// exits because the WS handler now owns read/write semantics on the
/// stream; there is no concept of "another request" on an upgraded
/// connection. The mid-keep-alive WS upgrade case is forbidden by
/// HTTP/1.1 anyway.
pub async fn dispatch(mut stream: TcpStream, state: crate::DaemonState) {
    let mut requests_served: u32 = 0;
    loop {
        let outcome = handle_one_request(&mut stream, &state).await;
        match outcome {
            DispatchOutcome::KeepAlive => {
                requests_served = requests_served.saturating_add(1);
                if requests_served >= KEEP_ALIVE_MAX_REQUESTS {
                    return;
                }
                // Continue: loop body re-enters `handle_one_request`
                // which awaits the next request (with idle timeout).
            }
            DispatchOutcome::Done => return,
        }
    }
}

/// Serve exactly one HTTP request on `stream`, returning whether the
/// outer keep-alive loop should poll for another request.
///
/// `/events` is the one exception: on a valid token we hand off to
/// [`crate::events::serve_events_connection`] which performs the
/// WebSocket upgrade via `tokio_tungstenite::accept_async` — that
/// function consumes the handshake bytes itself, so we DO NOT read
/// the request body here for that route. The WS handler now takes
/// `&mut TcpStream` so we keep ownership of the socket; on return
/// we exit with [`DispatchOutcome::Done`] because the upgraded
/// connection has no concept of a "next request."
async fn handle_one_request(
    stream: &mut TcpStream,
    state: &crate::DaemonState,
) -> DispatchOutcome {
    // Peek just the request line + headers so we can route on path
    // without consuming the body. Enough for WS handshakes (which
    // tokio-tungstenite will re-read) and the small GET bodies (which
    // have no body).
    //
    // 0.39.7: wrap the peek in an idle-timeout so a client that
    // opened a connection and went silent doesn't hold an fd forever.
    // The timeout only covers the wait-for-next-request window; once
    // bytes arrive the request is fully served without further time
    // pressure (LLM inference et al. can legitimately take tens of
    // seconds).
    let mut buf = [0u8; REQUEST_HEAD_MAX];
    let mut n = match tokio::time::timeout(
        KEEP_ALIVE_IDLE_TIMEOUT,
        stream.peek(&mut buf),
    )
    .await
    {
        Ok(Ok(n)) if n > 0 => n,
        // Idle timeout, EOF (peer closed), or read error → close.
        _ => return DispatchOutcome::Done,
    };

    // 0.39.45 (#35/#37): the request HEAD (request line + headers) may
    // not arrive in one TCP segment — and pre-0.39.45 a single peek was
    // all we looked at, so a long URL-encoded query string was parsed
    // from whatever fit in the first 4KB peek and the tail was SILENTLY
    // dropped (the ~2.7KB inbox-body truncation). Keep peeking until the
    // `\r\n\r\n` header terminator is visible, the buffer fills, or the
    // peer stalls past a short deadline. Peeks never consume, so WS
    // handshake handoff and `read_post_body` re-reads are unaffected.
    {
        let head_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") && n < buf.len() {
            if tokio::time::Instant::now() >= head_deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
            n = match stream.peek(&mut buf).await {
                Ok(m) if m > 0 => m,
                _ => return DispatchOutcome::Done,
            };
        }
    }

    // Buffer full and still no header terminator → the request head
    // exceeds what we'll parse. Refuse LOUDLY (414) instead of silently
    // truncating the query string mid-parameter (GH #35/#37 was exactly
    // that failure mode: `success: true` while the durable record lost
    // its tail). Long values belong in a POST body, which has no cap.
    if n == buf.len() && !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
        let _ = stream.read(&mut buf).await;
        super::http::send_response(
            &mut *stream,
            "414 URI Too Long",
            "application/json",
            r#"{"error":"request line + headers exceed 16KB; send long values (text/body) in a POST form body instead of the query string"}"#,
        )
        .await;
        return DispatchOutcome::Done;
    }
    let req = String::from_utf8_lossy(&buf[..n]);

    // 0.39.7: parse the request's `Connection:` header. If the client
    // requested close, we still serve THIS request normally, but
    // we return `DispatchOutcome::Done` at the end instead of
    // keep-alive. HTTP/1.0 clients send `close` explicitly; HTTP/1.1
    // clients can send it to opt out of the default keep-alive.
    let client_wants_close = super::http::request_wants_close(&req);

    // COMPAT-58: parse `Authorization: Bearer <sid>.<secret>` while the
    // header blob is still in scope (it's dropped at the bottom of the
    // routing prologue). Consumed ONLY by the dormant scoped arm of
    // `/hook/complete`; the `?token=` fallback keeps every legacy caller
    // unchanged. Cheap, side-effect-free parse → no behavior change.
    let bearer_token = super::http::extract_bearer(&req).map(str::to_string);

    // Hosted web cookie auth (PRD §2.3): Secure flag decision + web-client
    // header detection must run while the request head is still borrowed.
    let request_secure = super::http::request_is_secure(&req);
    let web_client_header = super::http::has_web_client_header(&req);

    let first_line = req.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    let (method, path_and_query) = match parts.as_slice() {
        [m, p, ..] => (*m, *p),
        _ => {
            // Consume what we peeked so the error gets delivered.
            let _ = stream.read(&mut buf).await;
            super::http::send_response(&mut *stream, "400 Bad Request", "text/plain", "bad request\n").await;
            return DispatchOutcome::Done;
        }
    };

    // H5 (E2E splice hardening): reject `Transfer-Encoding: chunked`.
    // `read_post_body` is Content-Length-driven ONLY — it never decodes
    // chunked framing. A chunked POST would therefore leave the chunk
    // size-lines + trailers unconsumed in the stream, and the keep-alive
    // loop would parse those leftover bytes as the NEXT request → request
    // smuggling / keep-alive desync. The daemon's own clients never send
    // chunked, so refuse it LOUDLY (400) and close the connection rather
    // than half-read a body we can't frame. The check runs on every
    // request (the head is already peeked), keeping the relay strictly L4.
    if super::http::request_is_chunked(&req) {
        let _ = stream.read(&mut buf).await;
        super::http::send_response(
            &mut *stream,
            "400 Bad Request",
            "application/json",
            r#"{"error":"Transfer-Encoding: chunked is not supported; send a Content-Length-framed body"}"#,
        )
        .await;
        // Force-close: we cannot safely find the next request boundary in
        // a chunked stream we don't decode, so keep-alive is unsafe here.
        return DispatchOutcome::Done;
    }

    // Phase 4.5: handle CORS preflight before the method allowlist.
    // The Tauri WebView origin (tauri://localhost or http://localhost:5173
    // in dev) is cross-origin relative to http://127.0.0.1:<port>, so
    // the browser sends an OPTIONS preflight before every POST. We
    // answer it with permissive CORS headers — token auth still
    // gates every real request, so `Access-Control-Allow-Origin: *`
    // adds no security risk and avoids hard-coding every possible
    // Tauri dev-server port.
    if method == "OPTIONS" {
        let _ = stream.read(&mut buf).await;
        super::http::send_cors_preflight(&mut *stream).await;
        return DispatchOutcome::Done;
    }

    // Most routes are GET. Specific POST-accepting routes are
    // allowlisted here so non-GET hits other paths get a clean 405.
    let is_post = method == "POST";
    let post_path = path_and_query.split_once('?').map(|(p, _)| p).unwrap_or(path_and_query);
    let post_allowed = matches!(
        post_path,
        "/cli/awareness/publish"
            | "/cli/sessions/v2/spawn"
            | "/cli/sessions/v2/close"
            // Phase 2 Unit 1 — body-bearing companion control routes.
            // Password and session-token live in the body so they
            // don't end up in URL-logged form on shared/loopback
            // intermediaries.
            | "/cli/companion/set-password"
            | "/cli/companion/disconnect-session"
            // K2 Connect tunnel — mutating control routes. Method-gated
            // per-handler below (the top-level dispatch lets a GET
            // through on POST-allowlisted routes; see
            // feedback_post_only_route_guards). Status is a GET via
            // crate::cli::dispatch.
            | "/cli/tunnel/start"
            | "/cli/tunnel/stop"
            // PRD tunnel-disable-unpair — the two-intent split. `disable`/
            // `enable` write the PERSISTED pause flag in tunnel.json (and
            // stop/start the connector); `release` permanently deletes this
            // device's tunnel identity (tombstone + upstream revocation).
            // All owner-token-only like start/stop (tunnel control =
            // exposure control) and method-gated per-handler below
            // (feedback_post_only_route_guards).
            | "/cli/tunnel/disable"
            | "/cli/tunnel/enable"
            | "/cli/tunnel/release"
            // 0074 — nested-subdomain workspace attribution writes
            // (`k2 publish subdomain claim/unclaim` + the create/point/rm
            // stamp seams). Method-gated per-handler below
            // (feedback_post_only_route_guards); token_ok tier — the
            // write is workspace metadata (label → project id), not
            // tunnel control.
            | "/cli/tunnel/subdomains/claim"
            | "/cli/tunnel/subdomains/unclaim"
            // Publish-mutation realtime nudge — on-demand control-plane
            // re-pull of the subdomain map (same fetch as the periodic
            // poll). Method-gated per-handler below
            // (feedback_post_only_route_guards); token_ok tier — it only
            // refreshes a cache the daemon already maintains on a timer.
            | "/cli/tunnel/subdomains/refresh"
            // K2SO #651 — supervisor-agnostic daemon restart. OWNER-ONLY
            // (restarting is the most privileged op; a connect-user session
            // token is rejected). Method-gated per-handler below
            // (feedback_post_only_route_guards). No body — owner token rides
            // the query string.
            | "/cli/daemon/restart"
            // K2SO P3 — remote daemon self-UPDATE (binary-swap shape).
            // OWNER/ADMIN-gated per-handler (require_owner_or_admin) below.
            // `check` fetches the manifest + compares (read-only but POST so
            // it's never idempotent-cached); `start` kicks the async
            // download+verify+stage job; `apply` backs up + spawns the
            // detached swap helper + triggers the P0 graceful shutdown. The
            // status read is a GET via crate::cli::dispatch (misc_routes).
            // Method-gated per-handler (feedback_post_only_route_guards).
            | "/cli/daemon/update/check"
            | "/cli/daemon/update/start"
            | "/cli/daemon/update/apply"
            // #58 Phase-1 close — global scoped-hook-token kill switch.
            // OWNER-ONLY (require_owner, NOT owner-or-admin) panic switch:
            // bumps the global hook epoch so EVERY minted scoped token goes
            // stale at once. Method-gated per-handler below.
            | "/cli/daemon/hook-revoke-all"
            // K2SO 0.39.35 — Shape A phase-relay: the co-located Tauri app
            // POSTs its updater's phase back here so the daemon's
            // update/status poll surfaces it uniformly. Owner/admin-gated +
            // POST-gated per-handler below.
            | "/cli/daemon/app-update/progress"
            // GET reads the redacted config (tokenSet bool, never the
            // secret); POST sets token/subdomain/server. Claimed here for
            // both methods; the handler branches on `is_post`.
            | "/cli/tunnel/config"
            // K2SO #617 — connect-user management (OWNER-ONLY, gated by
            // require_owner in the handlers below) + the PUBLIC login
            // route. All carry credentials/usernames in the JSON body so
            // they're never URL-logged. Method-gated per-handler below.
            | "/cli/users/add"
            | "/cli/users/remove"
            | "/cli/users/set-password"
            | "/cli/users/set-disabled"
            // K2SO #629 — change a connect-user's role. Owner-only (gated
            // per-handler below); method-gated POST.
            | "/cli/users/set-role"
            // Presence S4 — toggle a viewer-role user's ephemeral edit
            // grant. Owner-or-admin (require_manage) + POST-gated
            // per-handler below (feedback_post_only_route_guards).
            | "/cli/presence/grant"
            // K2SO #620 — owner-only password-policy write. GET (read) goes
            // through the GET arm below; POST is method-gated per-handler.
            | "/cli/users/policy"
            | "/cli/auth/login"
            // Presence S3 — kick a connected user. Owner/Admin-gated per
            // handler (require_manage + the kick matrix); username rides
            // the JSON body. Method-gated per-handler below
            // (feedback_post_only_route_guards).
            | "/cli/presence/kick"
            // Self-service password change from the daemon-hosted account
            // portal — connect-user session in the body/query, POST so it's
            // never URL-logged. (Was missing here → 405'd before its arm.)
            | "/cli/auth/change-password"
            // K2 Connect #4 — log out (delete the caller's persisted
            // session record). POST so the session token in `?token=` is
            // never confused with a GET/refresh + so it can't be cached.
            | "/cli/auth/logout"
            // Phase 2 Unit 5 — Claude Auth mutating routes. POST
            // (not GET) so they're not idempotent-cached by any
            // future proxy and so they parallel Unit 1's pattern
            // for "this writes state". The status read-side stays
            // a GET and goes through `crate::cli::dispatch`.
            | "/cli/claude-auth/refresh-now"
            | "/cli/claude-auth/install-scheduler"
            | "/cli/claude-auth/uninstall-scheduler"
            // Phase 2 Unit 2 — LLM control + chat. Chat body
            // carries the user message + workspace context. Load
            // takes a path. Download-default takes no body but is
            // a write-side operation so we accept POST for it too.
            | "/cli/llm/chat"
            | "/cli/llm/load-model"
            | "/cli/llm/download-default"
            // Phase 2 Unit 7a — settings writes. Partial settings
            // payloads live in the body; `reset` is POST so it can't
            // be reached via a stray GET / browser refresh.
            | "/cli/settings/update"
            | "/cli/settings/reset"
            // Phase 2 Unit 6 — filesystem mutations + chat history
            // mutations + theme/skill-layer/review-checklist
            // mutations. JSON bodies carry the arguments (paths,
            // file contents, source/destination tuples) so they
            // aren't URL-encoded in proxy logs.
            | "/cli/fs/search-tree"
            | "/cli/fs/write-file"
            // K2 Connect remote-files Phase 2 — base64 upload of a local
            // file's bytes onto the daemon's disk. Gated below by its own
            // isolated arm (one-line auth swap) ahead of the shared
            // `/cli/fs/` POST arm. Body carries the bytes so they're never
            // URL-logged.
            | "/cli/fs/upload-binary"
            // K2 Connect "Clone to" — streaming upload for LARGE bundles
            // (GH #3). Same isolated-arm gate as upload-binary; the body
            // carries one ordered chunk so a multi-GB transfer never buffers
            // whole and dodges the 100MB single-shot cap.
            | "/cli/fs/upload-chunk"
            // 0.40.22 large-file transfers — server-side folder → zip as a
            // start-then-poll job (status is a GET via misc_routes). Both
            // POSTs are re-asserted with require_post in their arm below.
            | "/cli/fs/compress"
            | "/cli/fs/compress-cancel"
            // Server-side zip → folder extract (inverse of compress). Same
            // start-then-poll job; status is a GET via misc_routes.
            | "/cli/fs/extract"
            | "/cli/fs/extract-cancel"
            // K2 Connect "Clone to" P2 — workspace migration. `bundle`
            // runs on the SOURCE daemon (build the scrubbed tar.gz +
            // capture K2 settings); `unpack` runs on the DESTINATION daemon
            // (extract at recomputed paths + register + apply settings).
            // Both gated below by their own isolated `token_ok` arm (same
            // one-line-swap pattern as upload-binary). Bodies carry paths so
            // they're never URL-logged.
            | "/cli/clone/bundle"
            | "/cli/clone/unpack"
            // 0.40.22 "Clone to this computer" — pull-pack as a start-then-
            // poll job (status is a GET via misc_routes). Both POSTs are
            // re-asserted with require_post in their arm below.
            | "/cli/clone/pack"
            | "/cli/clone/pack-cleanup"
            | "/cli/fs/move"
            | "/cli/fs/copy"
            | "/cli/fs/delete"
            | "/cli/fs/rename"
            | "/cli/fs/create"
            | "/cli/fs/duplicate"
            | "/cli/fs/open-finder"
            | "/cli/fs/open-external"
            | "/cli/chat/rename"
            | "/cli/chat/toggle-pin"
            | "/cli/chat/archive"
            | "/cli/chat/restore"
            | "/cli/chat/migrate-ide"
            | "/cli/sandbox/reopen"
            | "/cli/themes/create-template"
            | "/cli/themes/delete"
            | "/cli/skill-layers/create"
            | "/cli/skill-layers/delete"
            | "/cli/review-checklist/write"
            | "/cli/review-checklist/toggle"
            | "/cli/review-checklist/init"
            // Phase 2 Unit 3 — terminal PTY lifecycle. JSON-bodied
            // mutating routes; method-gated per-handler below.
            | "/cli/terminal/create"
            | "/cli/terminal/kill"
            | "/cli/terminal/resize"
            | "/cli/terminal/kill-foreground"
            | "/cli/terminal/scroll"
            | "/cli/terminal/log"
            | "/cli/terminal/lifecycle-write"
            | "/cli/terminal/set-focus"
            // S7a (presence/multiplayer §5.5) — pin a v2 session's PTY
            // to fixed cols×rows (or clear). JSON-bodied POST;
            // require_post + token_ok in the dedicated arm below.
            | "/cli/terminal/pin-size"
            // Phase 2 Unit 7c — heartbeat-launchd installer + orphan-
            // agents sweep. Body-bearing writes; method-gated below.
            | "/cli/heartbeat/install-launchd"
            | "/cli/heartbeat/uninstall-launchd"
            | "/cli/heartbeat/apply-wake-scheduler"
            | "/cli/agents/archive-orphans"
            // Phase 2 Unit 4 — DB-writing routes (workspaces /
            // focus-groups / sections / workspace-layouts / timer /
            // presets / window-state / projects / git). JSON-bodied
            // writes — implicit method gate via the `starts_with`
            // dispatch arm in handle_connection that runs Unit 4's
            // POST dispatch. Listed explicitly here so the top-level
            // 405 guard never short-circuits them.
            // Workspace States POST routes retired with the product feature.
            | "/cli/workspaces/create" | "/cli/workspaces/delete" | "/cli/workspaces/set-nav-visible"
            | "/cli/focus-groups/create" | "/cli/focus-groups/update" | "/cli/focus-groups/delete"
            | "/cli/focus-groups/assign" | "/cli/focus-groups/reconcile"
            | "/cli/sections/create" | "/cli/sections/update" | "/cli/sections/delete"
            | "/cli/sections/reorder" | "/cli/sections/assign"
            | "/cli/workspace-layouts/save" | "/cli/workspace-layouts/delete"
            | "/cli/timer/create" | "/cli/timer/delete"
            | "/cli/presets/create" | "/cli/presets/update" | "/cli/presets/delete"
            | "/cli/presets/reorder" | "/cli/presets/reset"
            | "/cli/window-state/set"
            | "/cli/projects/create" | "/cli/projects/update" | "/cli/projects/delete"
            | "/cli/projects/reorder" | "/cli/projects/touch-interaction"
            | "/cli/projects/touch-interaction-clear"
            // task #672 — canonical Active mutating routes (owner-OR-
            // connect-user auth via token_ok in the /cli/projects/ POST
            // arm; dispatched by db_routes::dispatch_unit4_post).
            | "/cli/projects/activate" | "/cli/projects/pin" | "/cli/projects/dismiss"
            | "/cli/projects/add-from-path"
            | "/cli/projects/add-without-git" | "/cli/projects/init-git-and-open"
            | "/cli/projects/enable-worktrees" | "/cli/projects/detect-icon"
            | "/cli/projects/set-icon" | "/cli/projects/clear-icon"
            | "/cli/projects/open-in-finder" | "/cli/projects/open-in-editor"
            | "/cli/projects/open-in-terminal" | "/cli/projects/refresh-editors"
            | "/cli/git/create-worktree" | "/cli/git/remove-worktree"
            | "/cli/git/reopen-worktree" | "/cli/git/stage" | "/cli/git/unstage"
            | "/cli/git/stage-all" | "/cli/git/commit" | "/cli/git/merge-branch"
            | "/cli/git/abort-merge" | "/cli/git/resolve" | "/cli/git/delete-branch"
            | "/cli/git/prune-worktrees"
            // Phase 2.1 — workspace inbox mutating routes (A22.1).
            // Query-string POSTs (no JSON body); dispatched via
            // `inbox_routes::dispatch_post`. The `/cli/inbox/migrate`
            // route is a one-shot helper for tests / explicit
            // re-migration triggers — daemon first-boot also auto-
            // invokes (Phase 2.1b wiring).
            // Feedback F1 (prd-agent-feedback-notifications §4.3) —
            // mutating routes. JSON-bodied POSTs (create carries the ask
            // title/body/options; comment/answer carry free text) so long
            // values dodge the request-head cap and never URL-log.
            // token_ok (owner OR connect-user, like /cli/chat/*) +
            // require_post in the dedicated arm below; reads (list/show)
            // are GETs via crate::cli::dispatch (feedback_routes).
            | "/cli/feedback/create"
            | "/cli/feedback/comment"
            | "/cli/feedback/answer"
            | "/cli/feedback/resolve"
            | "/cli/feedback/assign"
            // Overlay threads (prd-overlay-threads-v1 S1/S3) — POST-only mutations.
            | "/cli/thread/post"
            | "/cli/thread/ask"
            | "/cli/thread/secret"
            | "/cli/thread/answer"
            | "/cli/thread/void"
            // K2 Mail (prd-email-server-v1 §11) — every mutating
            // `/cli/mail/*` path, listed NOW (foundation slice) so
            // later slices never fight this allowlist. JSON-bodied
            // POSTs; token_ok + require_post in the dedicated arm
            // below, which ALSO owner-or-admin-gates the server/domain/
            // config/approvals/doctor paths (PRD §10). As of S6 every
            // handler is REAL; reads (status/lists) are GETs via
            // crate::cli::dispatch (mail_routes) — except the S4 read
            // family (messages/read/attachments/wait), which has its
            // own spawn_blocking GET arm below.
            | "/cli/mail/server/enable"
            | "/cli/mail/server/disable"
            | "/cli/mail/server/uninstall"
            | "/cli/mail/config/set"
            // S6: POST /cli/mail/doctor = run the probes NOW (owner
            // verb; blocking DNS/TCP/SMTP I/O — served by the mail
            // POST arm's spawn_blocking). The GET on the same path
            // stays in the read chain (latest persisted run).
            | "/cli/mail/doctor"
            | "/cli/mail/domain/add"
            | "/cli/mail/domain/remove"
            | "/cli/mail/domain/check"
            | "/cli/mail/address/create"
            | "/cli/mail/address/delete"
            | "/cli/mail/send"
            | "/cli/mail/reply"
            | "/cli/mail/outbox/cancel"
            | "/cli/mail/approvals/approve"
            | "/cli/mail/approvals/deny"
            // S9 external assistant inboxes (PRD §17.5): owner CRUD
            // (add/remove — owner-or-admin-gated in the mail POST arm
            // via is_owner_level_mutation) + the agent `draft` verb
            // (workspace token; APPENDs a \Draft into the user's OWN
            // external account — no send path exists). add + draft
            // dial the user's IMAP host → the POST arm's
            // spawn_blocking covers them.
            | "/cli/mail/external/add"
            | "/cli/mail/external/remove"
            // `link/*` = the Settings UI's aliases for external add/remove.
            | "/cli/mail/link/add"
            | "/cli/mail/link/remove"
            // O4: begin / complete an OAuth link (Gmail loopback /
            // Microsoft device / remote client-capture complete);
            // owner-or-admin via is_owner_level_mutation's
            // /cli/mail/link/oauth/ prefix. The server-side poll/exchange
            // (blocking reqwest + a loopback listener) rides this arm's
            // spawn_blocking; the paired status GET is owner-gated below.
            | "/cli/mail/link/oauth/start"
            | "/cli/mail/link/oauth/complete"
            // S1 BYO OAuth client — the owner sets/clears their OWN
            // per-provider OAuth client id + (Gmail) secret. Owner-or-admin
            // via is_owner_level_mutation's /cli/mail/oauth-config/ prefix.
            // Reads app_settings + the vault → the POST arm's spawn_blocking.
            | "/cli/mail/oauth-config/set"
            | "/cli/mail/oauth-config/clear"
            // S11: the unified access-management surface (owner-or-admin
            // via is_owner_level_mutation's /cli/mail/access/ prefix).
            | "/cli/mail/access/grant"
            | "/cli/mail/access/revoke"
            | "/cli/mail/access/set-primary"
            | "/cli/mail/access/set-level"
            // 0081 inbox management + delete: the owner set-manage cap
            // route (owner-or-admin via the /cli/mail/access/ prefix) +
            // the workspace-token action verbs (move/flag/archive/delete/
            // folder create+rename — can_manage/can_delete gated in the
            // handlers; delete is always move-to-Trash, never expunge).
            // They rows dial Stalwart/IMAP → the POST arm's spawn_blocking.
            | "/cli/mail/access/set-manage"
            | "/cli/mail/move"
            | "/cli/mail/flag"
            | "/cli/mail/archive"
            | "/cli/mail/delete"
            | "/cli/mail/folder/create"
            | "/cli/mail/folder/rename"
            | "/cli/mail/draft"
            // DNS K1 — principal-bound control-plane proxy. JSON-bodied
            // POSTs; token_ok OR scoped require_hook in the dedicated arm
            // below (mirrors mail). Zone create/delete are owner-only
            // local rejects; agents use access/zones/records/verify.
            | "/cli/dns/access"
            | "/cli/dns/zones"
            | "/cli/dns/records"
            | "/cli/dns/records/add"
            | "/cli/dns/records/remove"
            | "/cli/dns/verify"
            | "/cli/dns/zones/create"
            | "/cli/dns/zones/delete"
            // Projects V1 P2 (prd-projects-v1 §4.1) — project-GROUP
            // mutations (NOT the legacy /cli/projects/* workspace
            // registry). JSON-bodied POSTs (msg carries free chat text;
            // save-layout carries the layout blob) so long values dodge
            // the request-head cap and never URL-log. token_ok (owner OR
            // connect-user) + require_post in the dedicated arm below;
            // the dashboard/* mutations additionally owner-or-admin
            // (§6.3 resolved Q2). Reads (list/show/messages/html-docs)
            // are GETs via crate::cli::dispatch (project_group_routes).
            | "/cli/project-group/create"
            | "/cli/project-group/rename"
            | "/cli/project-group/delete"
            | "/cli/project-group/pin"
            | "/cli/project-group/sort"
            | "/cli/project-group/add-member"
            | "/cli/project-group/remove-member"
            | "/cli/project-group/set-poc"
            | "/cli/project-group/msg"
            | "/cli/project-group/set-icon"
            | "/cli/project-group/set-color"
            | "/cli/project-group/dashboard/save-layout"
            | "/cli/project-group/dashboard/rename"
            | "/cli/project-group/dashboard/create"
            | "/cli/project-group/dashboard/delete"
            | "/cli/project-group/dashboard/reorder"
            // Companion C4 (prd-companion-v2 §4) — push-device
            // registration. JSON-bodied POSTs; token_ok (ANY authed
            // user — owner or connect-user; the phone registers over
            // its own logged-in session) + require_post in the
            // dedicated arm below, which also resolves the acting
            // username from the token for attribution (D3). No GET
            // reads in V1 — GETs 405 via crate::cli::dispatch
            // (push_routes).
            | "/cli/push/register-device"
            | "/cli/push/unregister-device"
            | "/cli/inbox/compose"
            | "/cli/inbox/deliver"
            | "/cli/inbox/deliver-bundle"
            | "/cli/inbox/move"
            | "/cli/inbox/archive"
            | "/cli/inbox/delete"
            | "/cli/inbox/respond"
            | "/cli/inbox/migrate"
            // Workspace knowledge base (brain map) — seed notes +
            // localhost serve on/off. Query/form POSTs; token_ok.
            // Reads (index/note/status) are GETs via crate::cli::dispatch.
            | "/cli/wiki/seed"
            | "/cli/wiki/serve"
            | "/cli/wiki/serve/on"
            | "/cli/wiki/serve/off"
            | "/cli/wiki/chat"
            | "/cli/wiki/chat/on"
            | "/cli/wiki/chat/off"
            // Published services — daemon-owned process + optional nested
            // hostname. POST-only mutating; GET list/logs via cli::dispatch.
            // Not under /cli/tunnel/ (owner-only deny).
            | "/cli/publish/run"
            | "/cli/publish/start"
            | "/cli/publish/stop"
            | "/cli/publish/rm"
            // K2 Connect host-awareness GAP — workspace skill / agent /
            // session / relations / heartbeat-flag / onboarding writes.
            // The renderer previously fired these via LOCAL Tauri
            // invoke(), which misfires when driving a remote host. Each is
            // a JSON-bodied POST wrapping the same k2_core fn the Tauri
            // command called; workspace-scoped (project_path/project_id in
            // the body), token-gated like every /cli data route. Listed
            // here so the top-level 405 guard never short-circuits them.
            | "/cli/skills/create"
            | "/cli/skills/remove"
            | "/cli/skills/write-opt-in"
            | "/cli/onboarding/set-harness-fanout-enabled"
            | "/cli/onboarding/harness-fanout-enabled"
            | "/cli/onboarding/set-agents-md-generate-enabled"
            | "/cli/onboarding/agents-md-generate-enabled"
            | "/cli/canonical/detect-state"
            | "/cli/agents/regenerate-workspace-skill"
            | "/cli/agents/save-agent-md"
            | "/cli/agents/disable-workspace-claude-md"
            | "/cli/agents/run-workspace-ingest"
            | "/cli/agents/save-session-id"
            | "/cli/session/set-surfaced"
            | "/cli/heartbeat/set-show-sessions"
            | "/cli/relations/create"
            | "/cli/relations/delete"
            // K2SO 0.39.39 — daemon-owned pinned-chat session lifecycle
            // (decision D1: ONE idempotent endpoint w/ forceRespawn);
            // re-asserts require_post per-handler below.
            | "/cli/workspace/ensure-pinned-chat"
            // 0.39.39 #676 — daemon-canonical tab title write (token_ok
            // auth in the isolated arm below).
            | "/cli/workspace/set-tab-title"
            // 0.40.34 — browser-open forwarding: the staged k2-open shim
            // (xdg-open/$BROWSER inside a K2 session) POSTs the URL here;
            // the handler broadcasts an app-level `open_url` session event
            // so the CONNECTED app opens it in a browser tab. token_ok +
            // require_post in the dedicated arm below; the scoped-token
            // channel rides the per-cell UDS (cell_server), not this arm.
            | "/cli/browser/open-url"
            // B3a (sandbox) — set the PER-WORKSPACE Anthropic API key (BYO
            // key). OWNER-ONLY (require_owner per-handler below) + POST so the
            // key rides the JSON body, never the URL-logged query string. The
            // key is staged into a microVM cell's guest env at spawn; never
            // logged/echoed.
            | "/cli/workspace/api-key"
            // 0.40.24 S2 (agent CLI) — multi-field per-workspace settings
            // write (`k2 agent set`). JSON body {project, fields:{...}}
            // wrapping the update_project_setting allowlist; token_ok +
            // require_post in the dedicated arm below.
            | "/cli/workspace/set"
            | "/cli/workspace/set-handle"
            // Workspace Resources (prd-workspace-resources-v1) — POST-only
            // add/remove. GET twins 405 via workspace_resources_routes.
            | "/cli/workspace/resources/add"
            | "/cli/workspace/resources/remove"
            // Context management stack (prd-context-hamburger-v1) — optional
            // AGENTS.md layer stack mutations. JSON-bodied POSTs;
            // token_ok + require_post in the dedicated arm below.
            // Reads (layers/show/presets) are GETs via cli::dispatch.
            | "/cli/context/add"
            | "/cli/context/remove"
            | "/cli/context/set-enabled"
            | "/cli/context/move"
            | "/cli/context/regen"
            // Host catalog library authoring (Settings → Context Catalog).
            // Isolated POST arm + require_manage — do NOT ride the
            // `/cli/context/` token_ok prefix (members would create).
            | "/cli/context/catalog/create"
            | "/cli/context/catalog/delete"
            // 0.40.24 S4 (agent CLI) — safe decommission (`k2 agent
            // retire`). JSON body {q, force, dryRun, archiveTo}; the
            // guards refuse (409 → CLI exit 3) instead of prompting.
            // token_ok + require_post in the dedicated arm below.
            | "/cli/agent/retire"
            // P3a (sandbox / K2-as-a-server) — API-key auth-tier MANAGEMENT
            // (owner-only, always-on; the owner pre-mints keys before flipping
            // the external /v1/* surface live). POST so the minted raw key
            // (create) rides the JSON body/response, never a URL-logged query.
            // Method- + owner-gated per-handler below. `list` is a GET.
            | "/cli/api-keys/create"
            | "/cli/api-keys/revoke"
            | "/cli/api-keys/disable"
            | "/cli/api-keys/enable"
            // F2 host read-back (prd-v1-api-completion §4) — the in-session
            // agent's RESPONSE egress over loopback TCP (`k2 respond` from a
            // HOST session, which has no per-cell UDS worth of jail). Auth is
            // a SCOPED per-session hook token ONLY (K2_HOOK_SCOPED mints it;
            // the owner token is REFUSED — it carries no session identity),
            // validated in the dedicated arm below; the append is pinned to
            // the token's OWN session. With scoped hooks off (default) no
            // token ever validates, so the arm is inert. POST so the message
            // text rides the body, never a URL-logged query.
            | "/cli/respond"
            // Host-session completion lifecycle (`k2 done`): mark_complete
            // only — no product message on the respond drain. Same scoped
            // hook auth as `/cli/respond`.
            | "/cli/session/complete"
            // P3b (sandbox / K2-as-a-server) — the external spawn route. POST
            // so the (untrusted) request body never rides a URL-logged query;
            // the `/v1/*` arm below is itself gated by K2_SANDBOX_API + auth.
            // (Without this the top-level POST guard 405s before the arm runs.)
            | "/v1/sandboxes"
            // 0.39.45 (#35/#37/#29) — live-msg POST form. Long message
            // text rides the form-encoded body instead of the query
            // string so it dodges the request-head cap. GET with query
            // params remains supported for older CLIs (dedicated arm
            // below; GET falls through to crate::cli::dispatch).
            | "/cli/workspace/msg"
            // Composer Phase 1a/1c — session-scoped verified send. Body
            // (form OR JSON) carries `session_id` + `text`; `from` is
            // resolved server-side from the token (never the body, D3).
            // 1c capability gate (authorize_send_message, D4): owner token
            // is ALWAYS allowed; a connect-user (role >= Member) is allowed
            // only when the host opted into remote instruction
            // (app_settings.allow_remote_instruct, default OFF) — else
            // drain-then-403. This route instructs an agent running
            // --dangerously-skip-permissions (= full RCE), so the gate is
            // server-enforced. Method-gated per-handler below (the dedicated
            // arm re-checks is_post).
            | "/cli/terminal/send-message"
            // Federation V1 (prd-cross-server-agent-comms) — the cross-server
            // POST routes. The whole surface is DARK by default: the dispatch
            // arm below 404s every `/cli/federation/*` path unless
            // K2_FEDERATION is on, so listing them here is inert in a shipped
            // build. `pair/request` is UNAUTH (creates only Pending);
            // `pair/confirm` is owner-or-admin; `send` is dual-auth
            // (owner-or-admin OR scoped passport, PR1); `inbound` is
            // envelope-authenticated (require_peer), NOT a token. Method-
            // gated per-handler below (require_post). The `roster` read is a GET.
            | "/cli/federation/pair/request"
            | "/cli/federation/pair/confirm"
            | "/cli/federation/inbound"
            | "/cli/federation/send"
            // Remote Session Layer 0+2 — master switch, grants, shell spawn.
            // enable/disable/grant/revoke: owner-or-admin; shell/spawn:
            // token_ok OR k2rs_ grant token, then Layer 0 + grant gate.
            // Method-gated per-handler below (require_post).
            // status + grants are GET.
            | "/cli/remote-session/enable"
            | "/cli/remote-session/disable"
            | "/cli/remote-session/grant"
            | "/cli/remote-session/revoke"
            | "/cli/remote-session/shell/spawn"
            // NOTE: "/v1/sandboxes" already appears earlier in this list (the
            // P3b external spawn route) — do not re-add it here (unreachable
            // duplicate pattern).
    )
        // Sandbox v2 (PRD §A) — the workspace-scoped session routes carry
        // dynamic `<workspace>` / `<session-id>` segments, so they cannot be
        // exact-listed above. POST is valid on `/v1/w/<ws>/sessions` (new /
        // fork) and `/v1/w/<ws>/sessions/<id>` (address); allow the whole
        // `/v1/w/` prefix here so the top-level 405 guard never short-circuits
        // them before the `/v1/` arm runs (that arm + the per-route `is_post`
        // branch below do the real method gating). The surface stays DARK
        // unless the /v1 gate is on (K2_API or legacy K2_SANDBOX_API; the
        // sessions family also needs K2_SANDBOX_API — checked in the `/v1/` arm).
        // Hire is the exact path `POST /v1/w` (no trailing slash / slug).
        || post_path == "/v1/w"
        || post_path.starts_with("/v1/w/");
    if method != "GET" && !(is_post && post_allowed) {
        let _ = stream.read(&mut buf).await;
        super::http::send_response(
            &mut *stream,
            "405 Method Not Allowed",
            "application/json",
            r#"{"error":"method not allowed for this route"}"#,
        )
        .await;
        return DispatchOutcome::Done;
    }

    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_and_query, ""),
    };
    // Copy out of the lossy Cow so we can consume the read buffer below
    // without extending the immutable borrow. Method must be owned too —
    // the CSRF gate below needs it after `req` is dropped.
    let path = path.to_string();
    let method = method.to_string();
    // Hosted web cookie auth (§2.3 / §9.2): fold Bearer / query / cookie
    // into a single effective `token=` query so every existing
    // `token_ok` / `extract_token` call site accepts the cookie without
    // a signature change. Preference: Bearer > ?token= > k2_session cookie.
    // `cookie_only` drives the CSRF gate below.
    let (query, cookie_only) = super::http::effective_auth_query(query, &req);
    // Own the header blob for the CSRF gate (needs header re-check after
    // path is known) and for login web-mode detection.
    let headers_blob = req.to_string();
    drop(req);

    // 0.39.5 readiness gate. While the daemon is still completing
    // first-boot migrations (phase != ready) it has bound its port and
    // answers liveness + the /boot-status handshake — so the renderer's
    // ConnectionGate can SEE us booting and read our version — but every
    // real route returns 503 so no handler runs against half-migrated
    // state. This preserves the pre-0.39.5 "handlers always see migrated
    // state" invariant now that migrations run AFTER the listener binds.
    // See `crate::boot_status`.
    if !crate::boot_status::is_ready()
        && !matches!(
            path.as_str(),
            "/ping" | "/health" | "/boot-status" | "/v1/jwks"
        )
    {
        let _ = stream.read(&mut buf).await;
        super::http::send_response(
            &mut *stream,
            "503 Service Unavailable",
            "application/json",
            r#"{"state":"migrating","error":"daemon is completing first-boot migrations"}"#,
        )
        .await;
        return DispatchOutcome::Done;
    }


    // Hosted web client Layer 0 (PRD §6.7 / §9.4): when the owner has shut
    // the browser door (`webClientEnabled=false`), reject data-plane
    // requests that look like the web SPA. `/boot-status` stays open so
    // the loader does not look dead. Distinct from REMOTE_SESSIONS_DISABLED.
    if !k2_core::app_settings::load().web_client_enabled
        && (path.starts_with("/cli/") || path == "/events")
        && super::http::is_web_client_request(&headers_blob)
    {
        let _ = stream.read(&mut buf).await;
        let r = super::http::web_client_disabled_response();
        super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        return DispatchOutcome::Done;
    }

    // Hosted web CSRF: cookie-only credential on mutating /cli/* requires
    // X-K2-Client: web (or X-K2-CSRF: web). WebSocket upgrades are GET and
    // skip this gate. Query-token / Bearer callers (CLI, desktop) are
    // unaffected. See `super::http::cookie_csrf_gate`.
    if let Some(r) =
        super::http::cookie_csrf_gate(&method, &path, cookie_only, &headers_blob)
    {
        let _ = stream.read(&mut buf).await;
        super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        return DispatchOutcome::Done;
    }

    // K2 Cloud S1 — restricted must-change-password sessions. A live
    // connect-user session whose account is flagged `must_change_password`
    // may only reach whoami / change-password / logout (+ the public
    // login); everything else 403s with `password_change_required`. The
    // owner token and unauthenticated/unknown tokens pass straight
    // through to the normal per-route gates. ONE chokepoint here covers
    // the entire route match below (HTTP + WS upgrades) — see
    // `super::http::session_password_gate`.
    if let Some(r) =
        super::http::session_password_gate(&path, &query, state.token.as_str(), is_post)
    {
        let _ = stream.read(&mut buf).await;
        super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        return DispatchOutcome::Done;
    }

    match path.as_str() {
        "/ping" => {
            let _ = stream.read(&mut buf).await;
            // Unauthenticated. Smallest liveness check.
            super::http::send_response(&mut *stream, "200 OK", "text/plain; charset=utf-8", crate::BANNER).await;
        }
        "/health" => {
            // Unauthenticated liveness probe the behavior test suite
            // polls before it does anything. Mirrors the body shape
            // src-tauri's agent_hooks server returns so tests can talk
            // to either process without branching.
            let _ = stream.read(&mut buf).await;
            super::http::send_response(
                &mut *stream,
                "200 OK",
                "application/json",
                r#"{"status":"ok"}"#,
            )
            .await;
        }
        "/boot-status" => {
            let _ = stream.read(&mut buf).await;
            // 0.39.5: unauthenticated daemon-identity + readiness
            // handshake. The renderer's ConnectionGate polls this to
            // decide whether to mount the app against THIS daemon.
            //
            // - `version`  — exact build string. The LocalPaired policy
            //   (auto-update path) requires it to equal the app's bundled
            //   version, so the renderer never binds to an OUTGOING old
            //   daemon during an update.
            // - `protocol` — daemon↔client API version K2 Connect
            //   range-checks for remote daemons (decoupled from the
            //   marketing version).
            // - `phase`    — starting | migrating | ready (+ reserved
            //   error). Clients treat anything but `ready` as not-ready.
            // - `detail`   — free-text for the UI only; never parsed.
            //
            // Pre-0.39.5 daemons have no such route and return 404, so an
            // outgoing old daemon fails the gate without special-casing.
            // See `crate::boot_status` + `[[project_daemon_handshake_contract]]`.
            let body = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "protocol": crate::boot_status::PROTOCOL,
                "phase": crate::boot_status::phase_str(),
                "detail": crate::boot_status::detail(),
                // 0.40.48 connection resilience: per-PROCESS instance id.
                // The renderer already health-polls this route; comparing
                // `instanceId` across polls is how it detects a silent
                // daemon restart (same subdomain, fresh process — e.g. a
                // self-update) and forces a cold resync instead of
                // trusting connection continuity. Additive + forward-
                // compatible (PROTOCOL not bumped); older clients ignore
                // it, older daemons omit it and clients fall back to the
                // pre-0.40.48 heuristics. Safe on this UNAUTHENTICATED
                // route: a random per-boot UUID carries no identity or
                // fingerprintable state beyond "the process restarted",
                // which /boot-status already implies via `phase`.
                "instanceId": crate::boot_status::instance_id(),
                // 0.39.35: update SHAPE selector. "bundled-app" hosts update
                // via the co-located Tauri app (Shape A); "standalone" hosts
                // via the in-daemon binary swap (Shape B). The renderer reads
                // this to vary copy; update/start routes on it server-side.
                "installKind": crate::boot_status::install_kind(),
                // Hosted web client Layer 0 (PRD §6.7 / §9.4): loader reads
                // this unauthenticated field so a wall-OFF device still
                // answers /boot-status (not "server dead") and can teach.
                "webClient": {
                    "enabled": k2_core::app_settings::load().web_client_enabled,
                },
                // COMPAT-58 (#58 Phase 1 / PR-A): advertise the scoped-hook
                // capability so clients can FEATURE-DETECT it without an app
                // version bump. `supported` = this daemon understands the
                // scoped per-cell token superset; `enabled` = whether
                // K2_HOOK_SCOPED is on for this process (default ON; opt out
                // with 0/false/off). A daemon talking to an OLDER fleet peer
                // that omits this field treats it as unsupported and stays on
                // the owner-token/TCP path. PROTOCOL is intentionally NOT
                // bumped: this is an additive, forward-compatible field.
                "scopedHooks": {
                    "supported": true,
                    "enabled": crate::session_token::scoped_hooks_enabled(),
                },
                // Observability/Agent-Ops Phase E: advertise the read-only
                // `/cli/ops/*` capability (overview + activity + stream) so
                // the agent-ops pane can feature-detect it. Additive +
                // forward-compatible like `scopedHooks` (PROTOCOL not bumped);
                // an older peer omitting it reads as unsupported.
                "ops": {
                    "supported": true,
                    "version": "1",
                },
                // F3 (prd-v1-api-completion §5 / Cloud PRD S3): the external
                // `/v1` API capability — `{enabled, sandboxes:"microvm"|"none"}`
                // — so dashboards/clients render the tier truthfully instead of
                // probing for 404/409. Additive + forward-compatible like
                // `scopedHooks` (PROTOCOL not bumped). Safe on this
                // UNAUTHENTICATED route: both facts are already observable by
                // probing `/v1/*` (surface-404 vs 401, spawn 409); no secrets.
                "api": crate::misc_routes::api_capability(),
                // Air-gap + LAN listen (prd-air-gap-and-lan-listen-v1): hide-UI
                // only. A stale app can still POST; daemon refuse is authority.
                // PROTOCOL not bumped (additive like webClient / scopedHooks).
                "airgap": {
                    "enabled": k2_core::airgap::enabled(),
                },
                "listen": {
                    "lan": k2_core::listen::lan_bound(),
                },
            })
            .to_string();
            super::http::send_response(&mut *stream, "200 OK", "application/json", &body).await;
        }
        // GET / and GET /account — the ONLY browser-facing HTML the daemon
        // serves: a tiny self-contained self-service account page for
        // connect-users (log in → change password) reached at
        // `https://<sub>.k2.dev`. Unauthenticated to LOAD (it's a login
        // form); its fetches hit the token-gated /cli/auth/* routes.
        //
        // Safe to mount at bare `/`: the K2 app talks only to /cli/*, /ping,
        // /health, /boot-status, and the /events WS — never `/`, which
        // previously fell through to the 404 arm. POST/other methods to `/`
        // still 404 via the catch-all.
        "/" | "/account" if !is_post => {
            let _ = stream.read(&mut buf).await;
            let html = crate::connect_users_routes::account_page_html();
            super::http::send_response(
                &mut *stream,
                "200 OK",
                "text/html; charset=utf-8",
                &html,
            )
            .await;
        }
        "/status" => {
            let _ = stream.read(&mut buf).await;
            // Token-gated. Returns a small JSON blob describing the
            // daemon's state so the Tauri app can verify it's talking to
            // the right process.
            if !super::http::token_ok(&query, state.token.as_str()) {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let uptime_secs = state.started_at.elapsed().as_secs();
            let pid = std::process::id();
            // `instanceId` (0.40.48): same per-process id `/boot-status`
            // reports, for token-holding callers that already poll /status.
            let body = format!(
                r#"{{"version":"{}","uptime_secs":{},"pid":{},"port":{},"instanceId":"{}"}}"#,
                env!("CARGO_PKG_VERSION"),
                uptime_secs,
                pid,
                state.port,
                crate::boot_status::instance_id(),
            );
            super::http::send_response(&mut *stream, "200 OK", "application/json", &body).await;
        }
        "/hook/complete" => {
            // Agent-lifecycle hook endpoint. URL-encoded query params
            // carry paneId / tabId / eventType / token. Business logic
            // (ring buffer, emit, WorkspaceSession.status sync) lives in
            // k2_core so src-tauri's existing server hits the same
            // code path.
            let _ = stream.read(&mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let req_token = params.get("token").cloned().unwrap_or_default();

            // COMPAT-58: remove in Phase 3 (owner-token deprecation).
            // LEGACY owner-token arm — unchanged. The owner token authorizes
            // a hook for ANY pane (it is the daemon-wide credential).
            let owner_ok = super::http::ct_eq_token(&req_token, &state.token);

            // #58 Phase 1 SCOPED arm — dual-accept with owner (Phase 2 is
            // owner REJECTION, not this PR). A per-session scoped token
            // authorizes ONLY its own paneId: Bearer preferred (kept out of
            // logs/transcripts), falling back to `?token=`. Flag default ON;
            // with explicit OFF nothing mints → this arm is inert.
            let scoped_ok = if !owner_ok && crate::session_token::scoped_hooks_enabled() {
                let presented = bearer_token
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(req_token.as_str());
                let req_pane = params.get("paneId").map(String::as_str).unwrap_or("");
                match crate::session_token::require_hook(presented, &path) {
                    // Scope enforcement: the token must be bound to the
                    // exact pane the hook is completing. Same token, a
                    // different paneId → no match → 403 (PRD §5 smoke #4).
                    Some(v) => !req_pane.is_empty() && v.pane_id == req_pane,
                    None => false,
                }
            } else {
                false
            };

            if !owner_ok && !scoped_ok {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"Invalid or missing auth token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body = k2_core::agent_hooks::handle_hook_complete(&params);
            super::http::send_response(&mut *stream, "200 OK", "application/json", body).await;
        }
        // Session Stream WS subscribe endpoint (0.34.0 Phase 2).
        // Lives on a /cli/ path but routes to the WS handler rather
        // than crate::cli::dispatch because it's an HTTP upgrade, not a
        // JSON request. Branch must precede the generic /cli/
        // catchall below or the dispatch would swallow it.
        "/cli/sessions/subscribe" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::sessions_ws::serve_session_subscribe_connection(
                stream,
                params,
                state.token.to_string(),
            )
            .await;
            // WS handler took read/write semantics for the upgraded
            // connection's lifetime. Don't loop — close the dispatch.
            return DispatchOutcome::Done;
        }
        // Canvas Plan Phase 2: raw-byte stream subscribe. Parallel
        // to /cli/sessions/subscribe but streams PTY bytes as
        // binary WS frames for clients running their own vte.
        "/cli/sessions/bytes" => {
            // Auth: the owner/connect-user token (token_ok) OR a P3b per-session
            // STREAM token scoped to EXACTLY this request's `?session=` (so an
            // external /v1/sandboxes caller streams with the per-session token,
            // never the API key — which is NOT token_ok and NOT in the stream
            // registry, so it is rejected here).
            if !super::http::token_ok(&query, state.token.as_str())
                && !crate::stream_token::query_authorizes(&query)
            {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::sessions_bytes_ws::serve_session_bytes_connection(
                stream,
                params,
                state.token.to_string(),
            )
            .await;
            return DispatchOutcome::Done;
        }
        // Kessel (A3): grid snapshot + delta WS endpoint.
        // Serves one Tauri thin client per session. Single-subscriber
        // by design. See `.k2so/prds/alacritty-v2.md`.
        "/cli/sessions/grid" => {
            // Auth: the owner/connect-user token (token_ok) OR a P3b per-session
            // STREAM token scoped to EXACTLY this request's `?session=` (the
            // external /v1/sandboxes caller streams with the per-session token;
            // the API key is NEVER accepted here — it fails both checks).
            if !super::http::token_ok(&query, state.token.as_str())
                && !crate::stream_token::query_authorizes(&query)
            {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::sessions_grid_ws::serve_session_grid_connection(
                stream,
                params,
                state.token.to_string(),
            )
            .await;
            return DispatchOutcome::Done;
        }
        // 0.38.0 Commit 4: daemon-authoritative session lifecycle
        // stream. Pushes `session_added`/`session_removed` JSON
        // frames to subscribers whose `path=` matches the affected
        // session's cwd. Renderer + mobile companion consume the
        // same wire format. See `.k2so/prds/daemon-authoritative-tabs.md`.
        p if p == crate::overlay_ws::OVERLAY_WS_PATH => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::overlay_ws::serve_overlay_events_connection(stream, params).await;
            return DispatchOutcome::Done;
        }
        "/cli/sessions/events" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::session_events_ws::serve_session_events_connection(
                stream,
                params,
                state.token.to_string(),
            )
            .await;
            return DispatchOutcome::Done;
        }
        // Awareness Bus endpoints (0.34.0 Phase 3).
        // `/cli/awareness/publish` — POST JSON body → egress::deliver
        // `/cli/awareness/subscribe` — WS, streams bus signals out
        "/cli/awareness/publish" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::awareness_ws::handle_publish(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        "/cli/awareness/subscribe" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Token already validated above; pass it through for the
            // 5s re-auth heartbeat (Part 1a). `extract_token` returns the
            // raw `?token=` value the upgrade was authorized with.
            let token = super::http::extract_token(&query)
                .unwrap_or_default()
                .to_string();
            crate::awareness_ws::serve_awareness_subscribe_connection(
                stream,
                token,
                state.token.to_string(),
            )
            .await;
            return DispatchOutcome::Done;
        }
        // Observability / Agent-Ops fan-in stream (Phase C of
        // `.k2/prds/prd-observability-agent-ops.md`). WS, read-only:
        // multiplexes the existing `session_events` broadcast + the
        // awareness bus onto ONE socket, tagged by source, so the agent-ops
        // pane subscribes once. NOT in `post_allowed` — GET WS, same shape
        // as `/cli/awareness/subscribe`.
        "/cli/ops/stream" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Token already validated above; pass it through for the 5s
            // re-auth heartbeat (same as `/cli/awareness/subscribe`).
            let token = super::http::extract_token(&query)
                .unwrap_or_default()
                .to_string();
            crate::ops_stream_ws::serve_ops_stream_connection(
                stream,
                token,
                state.token.to_string(),
            )
            .await;
            return DispatchOutcome::Done;
        }
        // POST /cli/sessions/v2/spawn — Kessel find-or-spawn
        // (A4). Parallel to /cli/sessions/spawn but produces a
        // DaemonPtySession (registered in v2_session_map) instead
        // of a SessionStreamSession. Idempotent on agent_name: same
        // name → same session, suitable for remount reattach.
        "/cli/sessions/v2/spawn" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::v2_spawn::handle_v2_spawn(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/sessions/v2/close — explicit teardown of a v2
        // session. Called only from `tabs.ts::removeTab` (A6).
        "/cli/sessions/v2/close" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::v2_spawn::handle_v2_close(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/workspace/ensure-pinned-chat — K2SO 0.39.39
        // daemon-owned pinned-chat session lifecycle (decision D1:
        // ONE idempotent endpoint). Body: {project, forceRespawn?}.
        // Resolves the canonical Claude argv via resume_chat, then
        // find-or-spawns under the bare-`<project_id>` canonical key
        // (atomic allocate+register closes the #682 dup-`--session-id`
        // race). ADDED alongside resume-chat-args / v2/spawn /
        // set-chat-session — none of those are removed (the renderer
        // keeps its current path until a later capability-gated cutover).
        //
        // Auth: owner OR connect-user session (token_ok) — same gate as
        // set-chat-session / resume-chat-args which reach the /cli/*
        // catchall. Method gate: explicit require_post (the top-level
        // dispatch lets a GET through on POST-allowlisted routes; see
        // feedback_post_only_route_guards).
        "/cli/workspace/ensure-pinned-chat" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = tokio::task::spawn_blocking(move || {
                crate::pinned_chat::handle_ensure_pinned_chat(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::awareness_ws::HandlerResult {
                status: "500 Internal Server Error",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/companion/set-password — Phase 2 Unit 1.
        // Body: `{"password": "..."}`. Hashes argon2id, stores in
        // macOS Keychain (preferred) or settings.json (fallback),
        // then invalidates every live companion session so the old
        // token can't be replayed.
        //
        // Method gate: see the long-form note on /cli/claude-auth/
        // refresh-now below — the top-level dispatch lets a GET
        // through on POST-allowlisted routes. Mirror Unit 5's
        // explicit `if !is_post` guard so a GET against this route
        // can't trigger the password rotation. Especially important
        // here because this is one of the routes Mobile Companion
        // and K2SO Connect will hit over the ngrok tunnel.
        "/cli/companion/set-password" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::companion_routes::handle_companion_set_password(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // ── Phase 2 Unit 2 — /cli/llm/* ────────────────────────────
        // GET routes are cheap; POST routes go through llm_routes
        // which owns the supervisor's subprocess machinery. All
        // five routes are token-gated by the standard query-string
        // check so callers must pass `?token=<token>` like every
        // other /cli/* endpoint.
        "/cli/llm/check" => {
            let _ = stream.read(&mut buf).await;
            if !super::http::token_ok(&query, state.token.as_str()) {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let r = crate::llm_routes::handle_check();
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/status" => {
            let _ = stream.read(&mut buf).await;
            if !super::http::token_ok(&query, state.token.as_str()) {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let r = crate::llm_routes::handle_status();
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/chat" => {
            // Method gate (see feedback_post_only_route_guards memory + the
            // /cli/claude-auth/refresh-now comment): the top-level dispatch
            // lets a GET through on POST-allowlisted routes. Reject explicitly.
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // Inference is CPU/GPU heavy and may block for tens of
            // seconds. Run on a blocking worker so the runtime's
            // accept-loop threads stay free.
            let r = tokio::task::spawn_blocking(move || {
                crate::llm_routes::handle_chat(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/load-model" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = tokio::task::spawn_blocking(move || {
                crate::llm_routes::handle_load_model(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/download-default" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Body is currently empty; read+drop to flush.
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::llm_routes::handle_download_default(&state.event_tx);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/companion/disconnect-session — Phase 2 Unit 1.
        // Body: `{"sessionToken": "..."}`. Removes the session row
        // and any WS clients still attached to it.
        //
        // Method gate: same rationale as /cli/companion/set-password
        // above. Don't let a GET disconnect a live session.
        "/cli/companion/disconnect-session" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result =
                crate::companion_routes::handle_companion_disconnect_session(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/tunnel/start — K2 Connect tunnel.
        //
        // Spawns/supervises the `frpc` child that dials the hosted frps
        // server, exposing THIS daemon at https://<user>.k2.dev. The
        // optional `subdomain` query param overrides the stored config's
        // requested label; the live daemon port (`state.port`) is the
        // exposed `localPort` when the config doesn't pin one.
        //
        // Method gate: explicit `require_post` — the top-level dispatch
        // lets a GET through on POST-allowlisted routes, and we must
        // never let a curl GET launch a tunnel (see
        // feedback_post_only_route_guards).
        "/cli/tunnel/start" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            // OWNER-ONLY (K2SO #617): starting the tunnel exposes the
            // host's daemon publicly. A connect-user (who reaches the
            // daemon THROUGH the tunnel) must never control it. Strict
            // `require_owner` — a connect-user session token is rejected.
            if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            // No JSON body — params ride the query string. Drain to flush.
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let subdomain = params
                .get("subdomain")
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let daemon_port = state.port;
            let result = tokio::task::spawn_blocking(move || {
                k2_core::tunnel::start_tunnel(subdomain, daemon_port)
            })
            .await
            .unwrap_or_else(|e| Err(format!("worker join: {e}")));
            let resp = match result {
                Ok(status) => {
                    // #675.5 — connector transitioned to running; push the
                    // new status onto the session-events spine so the
                    // renderer can drop its `/cli/tunnel/status` poll
                    // (CompanionSection.tsx).
                    let _ = crate::session_events::emit(
                        crate::session_events::SessionEvent::TunnelStatusChanged {
                            running: status.running,
                            public_url: status.public_url.clone(),
                        },
                    );
                    crate::cli::CliResponse::ok_json(
                        serde_json::to_string(&status)
                            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                    )
                }
                Err(e) => crate::cli::CliResponse::bad_request(e),
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // POST /cli/tunnel/stop — stop the supervised frpc child.
        "/cli/tunnel/stop" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            // OWNER-ONLY (K2SO #617): same rationale as /cli/tunnel/start
            // — a connect-user must not tear down the host's tunnel.
            if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let resp = match k2_core::tunnel::stop_tunnel() {
                Ok(()) => {
                    // Honest post-stop status: re-read from the connector
                    // (reap always runs on stop; status should be stopped).
                    // Emit TunnelStatusChanged from the real status so
                    // clients never see a synthetic "running:false" that
                    // disagrees with tunnel_status().
                    let st = k2_core::tunnel::tunnel_status();
                    let _ = crate::session_events::emit(
                        crate::session_events::SessionEvent::TunnelStatusChanged {
                            running: st.running,
                            public_url: st.public_url.clone(),
                        },
                    );
                    // Keep `ok` for existing clients; add `running` so
                    // callers can confirm the stop actually took.
                    crate::cli::CliResponse::ok_json(format!(
                        r#"{{"ok":true,"running":{}}}"#,
                        if st.running { "true" } else { "false" }
                    ))
                }
                Err(e) => crate::cli::CliResponse::bad_request(e),
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // POST /cli/tunnel/disable — PRD tunnel-disable-unpair §2A: the
        // persisted PAUSE. Writes `enabled: false` into ~/.k2/tunnel.json
        // (so restarts/reboots/orphaned daemons all respect it — the
        // spawn gate re-reads the flag from disk at every frpc spawn) and
        // stops the live connector. Identity/lease intact; /enable is the
        // symmetric undo.
        //
        // Method gate: explicit `require_post` (feedback_post_only_route_
        // guards). OWNER-ONLY: same tier as start/stop — pausing the
        // host's exposure is tunnel control.
        "/cli/tunnel/disable" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = tokio::task::spawn_blocking(k2_core::tunnel::disable_tunnel)
                .await
                .unwrap_or_else(|e| Err(format!("worker join: {e}")));
            let resp = match result {
                Ok(status) => {
                    // Connector is down; push the cleared status (#675.5).
                    let _ = crate::session_events::emit(
                        crate::session_events::SessionEvent::TunnelStatusChanged {
                            running: false,
                            public_url: None,
                        },
                    );
                    crate::cli::CliResponse::ok_json(
                        serde_json::to_string(&status)
                            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                    )
                }
                Err(e) => crate::cli::CliResponse::bad_request(e),
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // POST /cli/tunnel/enable — PRD tunnel-disable-unpair §2A: the
        // one-click undo of /disable. Persists `enabled: true` (plus an
        // optional `subdomain` override, mirroring /start) and brings the
        // tunnel up when the config is connectable.
        //
        // Method gate + OWNER-ONLY: identical tier to /start (enabling IS
        // starting, with the flag write in front).
        "/cli/tunnel/enable" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let subdomain = params
                .get("subdomain")
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let daemon_port = state.port;
            let result = tokio::task::spawn_blocking(move || {
                k2_core::tunnel::enable_tunnel(subdomain, daemon_port)
            })
            .await
            .unwrap_or_else(|e| Err(format!("worker join: {e}")));
            let resp = match result {
                Ok(status) => {
                    let _ = crate::session_events::emit(
                        crate::session_events::SessionEvent::TunnelStatusChanged {
                            running: status.running,
                            public_url: status.public_url.clone(),
                        },
                    );
                    crate::cli::CliResponse::ok_json(
                        serde_json::to_string(&status)
                            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                    )
                }
                Err(e) => crate::cli::CliResponse::bad_request(e),
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // POST /cli/tunnel/release — PRD tunnel-disable-unpair §2B: the
        // DESTRUCTIVE unpair. Stops the tunnel, reports the release
        // upstream (queued + replayed at boot when offline), tombstones
        // `~/.k2/unpaired.json`, and DELETES this device's tunnel identity
        // (tunnel.json + rendered frpc.toml + E2E keypair). The device can
        // never re-claim the subdomain; re-pairing mints a fresh identity.
        //
        // Requires `confirm=1` (or `true`/`yes`) — a destructive verb must
        // never fire from a bare POST; the CLI's `--confirm` and the app's
        // confirm dialog both map here. Method gate + OWNER-ONLY: the most
        // privileged tunnel op there is.
        "/cli/tunnel/release" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let confirmed = matches!(
                params.get("confirm").map(|s| s.as_str()),
                Some("1") | Some("true") | Some("yes")
            );
            let resp = if !confirmed {
                crate::cli::CliResponse::bad_request(
                    "release is destructive and permanent for this device — \
                     pass confirm=1 (CLI: `k2 tunnel release --confirm`)"
                        .to_string(),
                )
            } else {
                let result =
                    tokio::task::spawn_blocking(k2_core::tunnel::release_tunnel_identity)
                        .await
                        .unwrap_or_else(|e| Err(format!("worker join: {e}")));
                match result {
                    Ok(report) => {
                        let _ = crate::session_events::emit(
                            crate::session_events::SessionEvent::TunnelStatusChanged {
                                running: false,
                                public_url: None,
                            },
                        );
                        k2_core::log_debug!(
                            "[tunnel] identity RELEASED for subdomain {:?} (upstream_reported={})",
                            report.subdomain,
                            report.upstream_reported
                        );
                        crate::cli::CliResponse::ok_json(
                            serde_json::to_string(&report)
                                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                        )
                    }
                    Err(e) => crate::cli::CliResponse::bad_request(e),
                }
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // POST /cli/tunnel/subdomains/claim | /unclaim — 0074 nested-
        // subdomain workspace attribution. Attribute (or drop) a nested
        // label's `label → project_id` row; the CLI's create/point/rm
        // seams and the explicit claim/unclaim verbs both land here.
        //
        // Method gate: explicit `require_post` (the top-level dispatch
        // lets a GET through on POST-allowlisted routes; the GET chain
        // additionally 405s these paths — feedback_post_only_route_guards).
        //
        // Auth: `token_ok` (owner OR connect-user session), the same tier
        // as every other /cli data write (set-tab-title precedent). This
        // writes workspace METADATA, not tunnel control — no exposure
        // change, so no require_owner. No body — label + project ride the
        // query string (short, non-secret). DB work on a blocking thread.
        p @ ("/cli/tunnel/subdomains/claim" | "/cli/tunnel/subdomains/unclaim") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Params come from the query string AND the form-encoded POST
            // body (the CLI's cli_post_form transport), body winning on
            // key collision — the inbox-routes convention.
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            let unclaim = p.ends_with("/unclaim");
            let result = tokio::task::spawn_blocking(move || {
                if unclaim {
                    crate::misc_routes::handle_subdomain_unclaim(&params)
                } else {
                    crate::misc_routes::handle_subdomain_claim(&params)
                }
            })
            .await
            .unwrap_or_else(|e| crate::cli::CliResponse::bad_request(format!("worker join: {e}")));
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // POST /cli/tunnel/subdomains/refresh — publish-mutation realtime
        // nudge: re-pull the subdomain map from the control plane NOW (the
        // same fetch as the connector's periodic poll) so a just-created/
        // repointed/removed URL shows up in the UIs without waiting for the
        // next poll tick. The CLI calls this best-effort right after every
        // successful control-plane create/point/rm. The landed map goes
        // through `store()` → change-detect → `tunnel_subdomains_changed`
        // broadcast automatically; the handler emits nothing itself.
        //
        // Method gate: explicit `require_post` (feedback_post_only_route_
        // guards; the GET chain additionally 405s this path). Auth:
        // `token_ok` — same tier as claim/unclaim; this only refreshes a
        // cache the daemon already maintains on a timer, not tunnel control.
        // No params. Blocking control-plane HTTP on a worker thread.
        "/cli/tunnel/subdomains/refresh" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result =
                tokio::task::spawn_blocking(crate::misc_routes::handle_subdomain_refresh)
                    .await
                    .unwrap_or_else(|e| {
                        crate::cli::CliResponse::bad_request(format!("worker join: {e}"))
                    });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // POST /cli/daemon/restart — K2SO #651, supervisor-agnostic remote
        // daemon restart (the foundational slice: bounce a remote K2 server
        // with no GUI).
        //
        // Restart MECHANISM is deliberately NOT `launchctl` (macOS-only).
        // The daemon will also run headless under systemd `Restart=always`.
        // Both supervisors respawn a process that exits, so we restart by
        // TRIGGERING GRACEFUL SHUTDOWN: fire the same `shutdown_tx` the
        // SIGINT handler uses → async_main wakes, tears down (the new reaper
        // reaps PTY children — an abrupt `std::process::exit` would ORPHAN
        // them), the process exits, launchd/systemd respawns it.
        //
        // Method gate: explicit `require_post` — the top-level dispatch lets
        // a GET through on POST-allowlisted routes, and a curl GET must
        // never bounce the daemon (feedback_post_only_route_guards).
        //
        // Auth: `require_owner_or_admin` (K2SO #660). The OWNER token still
        // authorizes (the on-box host owner). ADDITIONALLY a connect-user
        // SESSION whose role is Owner or Admin authorizes — that is the ONLY
        // way a remote user restarting the host OVER K2 Connect can be
        // authorized, since the remote user never holds the on-box owner
        // token. A Member session (or an unknown/missing token) is rejected
        // with 403. Exactly one 403 is written on rejection (the guard owns
        // the response path).
        "/cli/daemon/restart" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::require_owner_or_admin(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            // No JSON body — token rides the query string. Drain to flush.
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;

            // Write + FLUSH the 200 BEFORE anything can trigger shutdown, so
            // the caller always sees the ack even on the fastest teardown.
            super::http::send_response(
                &mut *stream,
                "200 OK",
                "application/json",
                r#"{"ok":true,"restarting":true}"#,
            )
            .await;
            let _ = stream.flush().await;

            // SEAM (#651): `shutdown_tx` is `Some` only in the running
            // daemon. In the test harness it is `None`, so the happy-path
            // (200 + would-restart) is asserted WITHOUT ever firing a real
            // restart — a test must NEVER kill the test process.
            if let Some(tx) = state.shutdown_tx.clone() {
                // Detached task: sleep briefly so the flushed 200 lands and
                // the socket drains on the client side, THEN trigger the
                // graceful teardown. We do NOT block this connection on it.
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    k2_core::log_debug!(
                        "[daemon] #651 restart requested — triggering graceful shutdown"
                    );
                    let _ = tx.send(());
                });
            }
            return DispatchOutcome::Done;
        }
        // #58 Phase-1 close — POST /cli/daemon/hook-revoke-all. The global
        // scoped-hook-token KILL SWITCH: bump the daemon-wide hook epoch so
        // every already-minted scoped token (stamped with the old epoch) goes
        // stale at once (instant + restart-surviving). OWNER-ONLY — this is a
        // documented panic switch, NOT owner-or-admin: a connect-user (even an
        // Admin) must not be able to mass-revoke the box's agent credentials.
        // Method-gated (require_post) so a stray GET can't trip it
        // (feedback_post_only_route_guards). Additive: this is the ONLY new
        // arm in the core dispatcher for #58 — the verb channel touches this
        // file zero times.
        "/cli/daemon/hook-revoke-all" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            crate::session_token::revoke_all();
            super::http::send_response(
                &mut *stream,
                "200 OK",
                "application/json",
                r#"{"ok":true,"revoked":"all-scoped-hook-tokens"}"#,
            )
            .await;
            return DispatchOutcome::Done;
        }
        // ── K2SO P3 — remote daemon self-UPDATE (binary-swap shape) ─────────
        //
        // All three POST routes are OWNER/ADMIN-gated (require_owner_or_admin,
        // K2SO #660) — the same tier as restart: a remote Owner/Admin over K2
        // Connect can drive an update with their SESSION token (they never
        // hold the on-box owner token), but a Member is barred. Each route is
        // explicitly POST-gated (require_post) per feedback_post_only_route_
        // guards: the top-level dispatch lets a GET through on POST-allowlisted
        // routes, and a curl GET must never download/swap/restart the daemon.
        //
        // Network I/O (manifest fetch, artifact download) runs on the blocking
        // pool so it NEVER ties up an accept-loop thread.
        //
        // POST /cli/daemon/update/check — fetch daemon-latest.json, compare to
        // the running version, report {current,latest,available,notes?,url?}.
        // Read-only (only the small JSON manifest is fetched).
        "/cli/daemon/update/check" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::require_owner_or_admin(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            // Blocking HTTP fetch off the accept loop.
            let r = tokio::task::spawn_blocking(crate::update_routes::handle_check)
                .await
                .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/daemon/update/start {version?} — create an async job that
        // downloads this platform's artifact + .sig, VERIFIES minisign against
        // the embedded pubkey (MANDATORY — abort on mismatch), verifies sha256,
        // and stages it. Returns {job_id} immediately; the download runs on a
        // detached worker so the HTTP thread is never blocked.
        "/cli/daemon/update/start" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::require_owner_or_admin(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // handle_start does a (blocking) manifest fetch up front (Shape B)
            // then spawns the detached download worker; run the up-front part
            // off the accept loop too. For a "bundled-app" host it instead
            // emits the app:update-trigger frame over `event_tx` (Shape A) and
            // returns immediately. Thread the broadcast sender in for that.
            let event_tx = Some(state.event_tx.clone());
            let r = tokio::task::spawn_blocking(move || {
                crate::update_routes::handle_start(&body_bytes, event_tx)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/daemon/app-update/progress {job_id,phase,progress?,error?}
        // — Shape A phase-relay (0.39.35). The co-located Tauri app POSTs here
        // at each step of its OWN updater so `/cli/daemon/update/status`
        // reflects app-side progress uniformly. Same owner/admin gate +
        // explicit POST gate as the other update routes. Bad phase ⇒ 400.
        "/cli/daemon/app-update/progress" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::require_owner_or_admin(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::update_routes::handle_app_update_progress(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/daemon/update/apply {job_id} — only when phase==staged.
        // Backs up the running binary, spawns a DETACHED swap/rollback helper,
        // then triggers the P0 graceful shutdown so the supervisor respawns the
        // NEW binary. SEAM: `shutdown_tx` is `None` in the test harness, so the
        // handler returns its 200 ack and SKIPS the backup/helper/shutdown — a
        // test NEVER swaps the binary or kills the process. Real swap/restart is
        // e2e-smoke-test-pending.
        "/cli/daemon/update/apply" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::require_owner_or_admin(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let shutdown_tx = state.shutdown_tx.clone();
            let r = crate::update_routes::handle_apply(&body_bytes, shutdown_tx);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // GET /cli/daemon/update/status?job_id= — poll a job's phase/progress.
        // Read-only, but gated to the SAME owner/admin tier as the mutating
        // update routes (a Member who can't start/apply an update has no need
        // to watch one). Dispatched here (not via the /cli/ catchall, which
        // would accept any session) so the gate is explicit.
        "/cli/daemon/update/status" => {
            if !super::http::require_owner_or_admin(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let _ = stream.read(&mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let job_id = params.get("job_id").cloned().unwrap_or_default();
            let r = crate::update_routes::handle_status(&job_id);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // GET/POST /cli/tunnel/config — read or set the K2 Connect tunnel
        // config. GET returns a REDACTED view (tokenSet bool, never the
        // secret token); POST applies a partial update and persists
        // ~/.k2so/tunnel.json. This is what the desktop K2 Connect page
        // calls to BIND a chosen subdomain (token + subdomain) before
        // `start`. GET must NOT read a body (read_post_body blocks on a
        // bodyless keep-alive GET); only POST drains the body.
        "/cli/tunnel/config" => {
            // Auth split (K2SO #617, POST relaxed for K2 Cloud re-pair):
            // POST MUTATES the host's tunnel binding (token + subdomain)
            // → OWNERSHIP-TIER: the on-box owner token OR an Owner-ROLE
            // connect session (`owner_role_identity`, the 8ca53aa bar —
            // NOT Admin: tunnel identity is ownership-level). Hosted (K2
            // Cloud) customers re-pair a subdomain through the `k2cloud`
            // Owner-role session and never hold the daemon token; the
            // subsequent re-dial goes through /cli/daemon/restart, so
            // start/stop stay strictly owner-token-only (a remote session
            // severing its own tunnel is a footgun). GET returns a
            // redacted, read-only view → authorized (`token_ok`; a
            // connect-user may read it). A must-change-password session
            // never reaches here (session_password_gate chokepoint).
            // Config changes log the NON-secret acting identity — never
            // the token value.
            let post_actor: Option<String> = if is_post {
                let Some(actor) =
                    super::http::owner_role_identity(&query, state.token.as_str())
                else {
                    let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                    super::http::send_response(
                        &mut *stream,
                        "403 Forbidden",
                        "application/json",
                        r#"{"error":"invalid or missing token"}"#,
                    )
                    .await;
                    return DispatchOutcome::Done;
                };
                Some(actor)
            } else {
                if !super::http::token_ok(&query, state.token.as_str()) {
                    super::http::send_response(
                        &mut *stream,
                        "403 Forbidden",
                        "application/json",
                        r#"{"error":"invalid or missing token"}"#,
                    )
                    .await;
                    return DispatchOutcome::Done;
                }
                None
            };
            let resp = if is_post {
                let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
                match serde_json::from_slice::<k2_core::tunnel::TunnelConfigUpdate>(&body_bytes) {
                    Ok(upd) => {
                        // Non-secret change summary for the audit line
                        // (subdomain is public; the token NEVER logs —
                        // only whether one was supplied).
                        let sub = upd.subdomain.clone();
                        let token_updated =
                            upd.token.as_deref().is_some_and(|t| !t.trim().is_empty());
                        match k2_core::tunnel::set_config(upd) {
                            Ok(view) => {
                                let actor = post_actor.as_deref().unwrap_or("owner-token");
                                k2_core::log_debug!(
                                    "[tunnel] config updated by {actor} (subdomain={}, tokenUpdated={token_updated})",
                                    sub.as_deref().unwrap_or("<unchanged>"),
                                );
                                crate::cli::CliResponse::ok_json(
                                    serde_json::to_string(&view)
                                        .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                                )
                            }
                            Err(e) => crate::cli::CliResponse::bad_request(e),
                        }
                    }
                    Err(e) => {
                        crate::cli::CliResponse::bad_request(format!("invalid JSON body: {e}"))
                    }
                }
            } else {
                match k2_core::tunnel::get_config_view() {
                    Ok(view) => crate::cli::CliResponse::ok_json(
                        serde_json::to_string(&view)
                            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                    ),
                    Err(e) => crate::cli::CliResponse::bad_request(e),
                }
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // ── K2SO #617 + #629 — connect-user management ──────────────────
        //
        // These `/cli/users/*` routes manage the public-tunnel auth
        // boundary. Pre-#629 they were strict OWNER-ONLY (require_owner).
        // K2SO #629 introduces a 3-role model (Owner>Admin>Member): the
        // management routes now accept the owner token OR a session whose
        // user `can_manage_users` (Admin|Owner) via `require_manage`, which
        // returns the actor's resolved Role. For remove/set-disabled we
        // additionally enforce `can_act_on` INSIDE the handler so an Admin
        // can't act on an Owner-role target (handler 403s). set-password +
        // policy stay OWNER-ONLY for now (require_owner). set-role is
        // Owner-only (can_change_roles). POST-gated per
        // feedback_post_only_route_guards.
        "/cli/users/add" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            // Owner OR a managing (Admin|Owner) session. Member/unknown → 403.
            if super::http::require_manage(&mut *stream, &mut buf, &query, state.token.as_str()).await.is_none() {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // argon2 hashing is intentionally slow; run off the accept loop.
            let r = tokio::task::spawn_blocking(move || {
                crate::connect_users_routes::handle_add(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/users/remove" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            // OWNER-ONLY (#629): removing users is reserved for owners. Admins
            // can add + enable/disable, but never remove. Same gate as
            // set-role (`can_change_roles` == actor is Owner / owner token).
            let actor_role = super::http::actor_role(&query, state.token.as_str());
            if !actor_role.map(k2_core::connect_users::can_change_roles).unwrap_or(false) {
                let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // Gate above guarantees the actor is Owner; pass it through
            // (handle_remove's can_act_on is a no-op for an Owner actor).
            let r = crate::connect_users_routes::handle_remove(
                actor_role.unwrap_or(k2_core::connect_users::Role::Owner),
                &body_bytes,
            );
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/users/set-password" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            // OWNERSHIP tier (#629 + Linux/cloud reality): on-box owner
            // token OR Owner-ROLE connect session (`can_change_roles`).
            // Pre-fix this used `require_owner` (daemon token only). On
            // Linux hosted boxes the desktop is almost always connected
            // with a connect-user Owner session (seed-users / first owner),
            // never the raw daemon token — so Settings → reset password
            // always 403'd with "invalid or missing token" while local
            // macOS (real owner token) worked. Match set-role's gate.
            // Admin still barred (password reset stays Owner-level).
            let actor_role = super::http::actor_role(&query, state.token.as_str());
            if !actor_role
                .map(k2_core::connect_users::can_change_roles)
                .unwrap_or(false)
            {
                let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // argon2 re-hash — spawn_blocking.
            let r = tokio::task::spawn_blocking(move || {
                crate::connect_users_routes::handle_set_password(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/users/set-disabled" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            let actor_role = match super::http::require_manage(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                Some(r) => r,
                None => return DispatchOutcome::Done,
            };
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::connect_users_routes::handle_set_disabled(actor_role, &body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/users/set-role — CHANGE-ROLES is OWNER-ONLY (K2SO #629).
        // Gated to the owner token OR an Owner-role session (can_change_roles).
        // A managing Admin reaches the other routes but NOT this one.
        "/cli/users/set-role" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            let actor_role = super::http::actor_role(&query, state.token.as_str());
            if !actor_role.map(k2_core::connect_users::can_change_roles).unwrap_or(false) {
                let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::connect_users_routes::handle_set_role(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // ── K2SO #620 — password policy ─────────────────────────────────
        //
        // GET /cli/users/policy — AUTHORIZED (owner OR connect-user
        // session). Lets the self-service portal read the active password
        // requirements to render the hint + client-side validate. Resolve
        // identity like /cli/auth/whoami: owner token first, then a live
        // connect-user session; anything else → 403.
        //
        // POST /cli/users/policy — OWNER-ONLY (mutates the auth boundary's
        // policy). `token_is_owner` gates it; a connect-user session is
        // rejected. Method-gated below (top-level dispatch lets a GET
        // through on POST-allowlisted routes — we branch on `is_post`).
        "/cli/users/policy" => {
            if is_post {
                // OWNER-ONLY write.
                if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                    return DispatchOutcome::Done;
                }
                let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
                let r = crate::connect_users_routes::handle_set_policy(&body_bytes);
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
            } else {
                // AUTHORIZED read (owner OR connect-user session).
                let _ = stream.read(&mut buf).await;
                let tok = super::http::extract_token(&query).unwrap_or("");
                let authorized = (!tok.is_empty()
                    && super::http::ct_eq_token(tok, state.token.as_str()))
                    || k2_core::connect_users::validate_session(tok).is_some();
                if !authorized {
                    super::http::send_response(
                        &mut *stream,
                        "403 Forbidden",
                        "application/json",
                        r#"{"error":"invalid or missing token"}"#,
                    )
                    .await;
                    return DispatchOutcome::Done;
                }
                let r = crate::connect_users_routes::handle_get_policy();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
            }
        }
        // GET /cli/users — list accounts (redacted views; no hashes).
        // K2SO #629: read-side of user management → owner token OR a
        // managing (Admin|Owner) session via `require_manage`. A Member or
        // unknown token is drained+403'd; a GET needs no body.
        "/cli/users" => {
            if super::http::require_manage(&mut *stream, &mut buf, &query, state.token.as_str()).await.is_none() {
                return DispatchOutcome::Done;
            }
            let _ = stream.read(&mut buf).await;
            let r = crate::connect_users_routes::handle_list();
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // ── K2SO #617 — connect-user auth entry ─────────────────────────
        //
        // POST /cli/auth/login — PUBLIC (NO token gate). This is how a
        // remote connect-user trades username+password for a session
        // token over the tunnel. On failure it returns a generic 401 and
        // we add a fixed delay (below) to blunt online brute force.
        // POST-gated so a stray GET can't probe credentials.
        //
        // Hosted web path (PRD §2.3): when the client signals web mode
        // (`X-K2-Client: web` header OR body `"web": true`), a successful
        // login ALSO sets `Set-Cookie: k2_session=<session_token>; HttpOnly;
        // SameSite=Strict; Path=/; Max-Age=<session TTL>` (+ `Secure` when
        // the request is HTTPS / X-Forwarded-Proto: https). Cookie value is
        // the same connect-users session token returned in the JSON body —
        // NEVER the owner daemon token.
        "/cli/auth/login" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // Peek the optional body `web: true` flag before the body is
            // moved into spawn_blocking (header already captured above).
            let web_from_body = serde_json::from_slice::<serde_json::Value>(&body_bytes)
                .ok()
                .and_then(|v| v.get("web").and_then(|w| w.as_bool()))
                .unwrap_or(false);
            let web_mode = web_client_header || web_from_body;
            // argon2 verify is slow + happens regardless of outcome
            // (anti-enumeration) — spawn_blocking off the accept loop.
            let r = tokio::task::spawn_blocking(move || {
                crate::connect_users_routes::handle_login(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
            // Fixed failure delay: slow brute-force without a full rate
            // limiter (deferred). Only on the 401 path so successful
            // logins stay snappy. The argon2 work already adds ~tens of
            // ms; this stacks a deterministic floor on top.
            if r.status.starts_with("401") {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            // Set-Cookie on successful web login. Token is the same
            // connect-users session token already in the JSON body.
            let set_cookie = if web_mode && r.status.starts_with("200") {
                serde_json::from_str::<serde_json::Value>(&r.body)
                    .ok()
                    .and_then(|v| {
                        v.get("token")
                            .and_then(|t| t.as_str())
                            .filter(|t| !t.is_empty())
                            .map(|t| {
                                let max_age = k2_core::connect_users::session_ttl_days()
                                    .saturating_mul(86_400);
                                super::http::session_cookie_set_value(
                                    t,
                                    request_secure,
                                    max_age,
                                )
                            })
                    })
            } else {
                None
            };
            super::http::send_response_with_cookie(
                &mut *stream,
                r.status,
                r.content_type,
                &r.body,
                set_cookie.as_deref(),
            )
            .await;
        }
        // GET /cli/presence/roster — S1 (presence/multiplayer arc).
        // AUTHORIZED (owner OR connect-user session) — same gate as
        // /cli/auth/whoami. Read-only snapshot of the live presence
        // registry; returns `{ "roster": [...] }`, byte-identical in
        // shape to the `presence_changed` event payload so a client can
        // reconcile on `hello` (the ActiveChanged snapshot convention).
        // GET-only: it mutates nothing, so a stray POST gets an explicit
        // 405 rather than silently reading.
        "/cli/presence/roster" => {
            if is_post {
                let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                let r = crate::cli_response::CliResponse {
                    status: "405 Method Not Allowed",
                    content_type: "application/json",
                    body: r#"{"error":"GET required"}"#.to_string(),
                };
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let _ = stream.read(&mut buf).await;
            let tok = super::http::extract_token(&query).unwrap_or("");
            let authorized = (!tok.is_empty()
                && super::http::ct_eq_token(tok, state.token.as_str()))
                || k2_core::connect_users::validate_session(tok).is_some();
            if !authorized {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let r = crate::presence::handle_roster();
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/presence/kick — S3 (presence/multiplayer arc). Kick a
        // connected user: revoke their persisted sessions (durable) + fire
        // their live WS close handles (immediate). Gated like
        // /cli/users/set-disabled: `require_manage` (owner token OR a
        // managing Admin/Owner session) resolves the actor role, and the
        // handler enforces the kick matrix (`can_act_on` + admins can't
        // kick admins) against the target. POST-gated per
        // feedback_post_only_route_guards.
        "/cli/presence/kick" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            let actor_role = match super::http::require_manage(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                Some(r) => r,
                None => return DispatchOutcome::Done,
            };
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::presence::handle_kick(actor_role, &body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/presence/grant — S4 (presence/multiplayer arc §4).
        // Toggle a viewer-role user's ephemeral edit grant:
        // `{"username":..,"granted":bool}` → `{"success":true,...}`.
        // Owner-or-admin (`require_manage`, the users-management gate);
        // POST-gated per feedback_post_only_route_guards. Target
        // validation (exists + role viewer + currently connected) lives
        // in the handler — grants attach to a live connection and are
        // auto-revoked when the user's last connection deregisters.
        "/cli/presence/grant" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            let actor_role = match super::http::require_manage(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                Some(r) => r,
                None => return DispatchOutcome::Done,
            };
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::presence::handle_grant(actor_role, &body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // GET /cli/whoami — cell identity (canonical | sidecar). Dual-auth:
        // owner/connect-user (TCP fallback via env/query) OR scoped hook.
        // Distinct from /cli/auth/whoami (connect-user role).
        "/cli/whoami" => {
            let _ = stream.read(&mut buf).await;
            let (auth_ok, scoped_principal) = token_or_scoped_hook_auth(
                "/cli/whoami",
                &query,
                bearer_token.as_deref(),
                state.token.as_str(),
            );
            if !auth_ok {
                let r = crate::cli::CliResponse::forbidden();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let mut params = super::http::parse_params(&path, &query);
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
            }
            if let Some(v) = {
                let presented = bearer_token
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .or_else(|| super::http::extract_token(&query));
                presented.and_then(|t| crate::session_token::require_hook(t, "/cli/whoami"))
            } {
                params.insert("cell_session_id".to_string(), v.session_id);
            }
            let resp = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::cli::dispatch("/cli/whoami", &params)
                })
            })
            .await
            .unwrap_or_else(|e| {
                crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // GET /cli/auth/whoami — AUTHORIZED (owner OR connect-user). Lets
        // a client confirm its session + learn whether it's the owner.
        // We resolve identity here: owner token first, then a live
        // connect-user session. An unrecognized token is rejected.
        "/cli/auth/whoami" => {
            let _ = stream.read(&mut buf).await;
            let tok = super::http::extract_token(&query).unwrap_or("");
            // K2SO #629: also return the caller's resolved role so the
            // client can gate the Users/Access UI + the role selector. The
            // owner token → Owner; a session → its stored role.
            let r = if !tok.is_empty() && super::http::ct_eq_token(tok, state.token.as_str()) {
                crate::connect_users_routes::handle_whoami(
                    None,
                    true,
                    k2_core::connect_users::Role::Owner,
                )
            } else if let Some(username) =
                k2_core::connect_users::validate_session(tok)
            {
                let role = k2_core::connect_users::role_for_user(&username)
                    .unwrap_or(k2_core::connect_users::Role::Member);
                crate::connect_users_routes::handle_whoami(Some(username), false, role)
            } else {
                crate::cli::CliResponse::forbidden()
            };
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/auth/change-password — SELF-SERVICE (connect-user
        // only). Authorized by the connect-user's SESSION token (the
        // extended `token_ok` accepts it), and the actual username is
        // resolved here from `validate_session`. The OWNER (daemon token,
        // no session) resolves to None → handle_change_password returns a
        // generic 401: this route is for connect-users changing their OWN
        // password, not the owner. POST-gated. argon2 verify+re-hash is
        // slow → spawn_blocking.
        "/cli/auth/change-password" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let tok = super::http::extract_token(&query).unwrap_or("").to_string();
            // Owner token does NOT resolve to a connect-user; only a live
            // session does. Resolve before the blocking hop.
            let username = if !tok.is_empty() && super::http::ct_eq_token(&tok, state.token.as_str()) {
                None
            } else {
                k2_core::connect_users::validate_session(&tok)
            };
            let r = tokio::task::spawn_blocking(move || {
                crate::connect_users_routes::handle_change_password(username, &body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
            // Fixed delay on the 401 path mirrors /cli/auth/login so the
            // self-service form can't be used as a faster brute-force
            // oracle than login itself.
            if r.status.starts_with("401") {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/auth/logout — K2 Connect #4. Deletes the CALLER's own
        // persisted session record (per-device logout). Authorized by the
        // caller's own session token in `?token=` / Bearer / k2_session
        // cookie (effective_auth_query folded them into `query`); the OWNER
        // token has no session so it's a harmless idempotent no-op.
        // POST-gated (mutating /cli/* route → the `if !is_post { 405 }`
        // guard, per the contract). Hosted web: always clears the
        // k2_session cookie (Max-Age=0) so the browser drops it even when
        // the session was already expired server-side.
        "/cli/auth/logout" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            // Drain the (ignored) body so a half-read socket isn't left.
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let tok = super::http::extract_token(&query).unwrap_or("").to_string();
            let r = tokio::task::spawn_blocking(move || {
                crate::connect_users_routes::handle_logout(&tok)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
            let clear = super::http::session_cookie_clear_value(request_secure);
            super::http::send_response_with_cookie(
                &mut *stream,
                r.status,
                r.content_type,
                &r.body,
                Some(&clear),
            )
            .await;
        }
        // POST /cli/claude-auth/refresh-now — Phase 2 Unit 5.
        // No body required (the refresh token comes from the local
        // Keychain / credentials file). POST instead of GET because
        // it mutates token state. Returns the same status payload
        // shape as GET /cli/claude-auth/status.
        //
        // NOTE on method gating: the top-level dispatch only rejects
        // non-GET/non-POST methods; it doesn't reject GET on a
        // POST-allowlisted route (most routes accept both today and
        // gate behavior on body-presence). For Unit 5's mutating
        // routes — which have no body — we must explicitly reject
        // GET in the handler, or a curl GET would silently install /
        // refresh / uninstall the user's launchd scheduler. Caught
        // during smoke testing.
        "/cli/claude-auth/refresh-now" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Drain whatever body the client sent so the socket
            // doesn't get half-read state. We don't use it.
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::claude_auth_host::handle_refresh_now();
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/claude-auth/install-scheduler — Phase 2 Unit 5.
        // Writes ~/.k2so/claude-auth-refresh.sh + loads the
        // launchd plist (macOS) or installs the crontab entry
        // (linux). Idempotent. POST-only (see /refresh-now comment).
        "/cli/claude-auth/install-scheduler" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::claude_auth_host::handle_install_scheduler();
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/claude-auth/uninstall-scheduler — Phase 2 Unit 5.
        // Unloads + removes the plist (macOS) or strips the
        // crontab entry (linux). Idempotent. POST-only.
        "/cli/claude-auth/uninstall-scheduler" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::claude_auth_host::handle_uninstall_scheduler();
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/settings/update — Phase 2 Unit 7a.
        // Body: arbitrary JSON object deep-merged into settings.json.
        // F3 closure runs inside `app_settings::update()` — companion-
        // credential changes invalidate live sessions server-side, in
        // the same process that owns the live companion runtime.
        // Method gate per feedback_post_only_route_guards memory.
        // Remote-access keys (federationEnabled / allowRemoteInstruct /
        // apiEnabled / dnsManageEnabled / agentsCanCreateConnections)
        // additionally require owner-or-admin — resolved here via the same
        // `token_is_owner_or_admin` tier the federation management routes
        // use, enforced key-aware inside the handler (a Member touching a
        // gated key gets an atomic 403; other keys keep `token_ok` behavior).
        "/cli/settings/update" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let actor_can_manage =
                super::http::token_is_owner_or_admin(&query, state.token.as_str());
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result =
                crate::settings_routes::handle_settings_update(&body_bytes, actor_can_manage);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/settings/reset — Phase 2 Unit 7a.
        // Restores `AppSettings::default()`, deletes Keychain hash,
        // invalidates every live companion session. POST (not GET)
        // so a browser refresh can't accidentally trigger it.
        // Method gate per feedback_post_only_route_guards memory.
        "/cli/settings/reset" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let _ = stream.read(&mut buf).await;
            let result = crate::settings_routes::handle_settings_reset();
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // Phase 2 Unit 3 — terminal PTY lifecycle (POST routes).
        // Each handler runs through the process-wide
        // `k2_core::terminal::shared()` TerminalManager so daemon
        // ownership is uniform. The blocking create handler is
        // dispatched via `tokio::task::spawn_blocking` (F5) since
        // posix_spawn + alacritty Term::new can stall briefly under
        // load. The non-blocking handlers (kill/resize/scroll/etc.)
        // are cheap mutex+method calls and run inline.
        //
        // Method gate: see the long-form comment on
        // `/cli/claude-auth/refresh-now`. The top-level dispatch
        // does NOT reject GET on POST-allowlisted routes — without
        // the explicit `if !is_post` guard, a curl GET could
        // silently spawn / kill a PTY.
        "/cli/terminal/create" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // F5: posix_spawn + alacritty Term::new can block; run
            // off the accept-loop thread pool.
            let r = tokio::task::spawn_blocking(move || {
                crate::terminal_lifecycle_routes::handle_create(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/kill" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // Kill can block briefly waiting on child reap; F5.
            let r = tokio::task::spawn_blocking(move || {
                crate::terminal_lifecycle_routes::handle_kill(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/resize" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_resize(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/kill-foreground" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_kill_foreground(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/scroll" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_scroll(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/log" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_log(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/terminal/lifecycle-write — byte-level write for
        // TerminalManager-owned terminals. The existing
        // /cli/terminal/write (GET, in terminal_routes.rs) operates on
        // the session_map's UUID-keyed sessions; the legacy
        // arbitrary-string TerminalManager IDs need a parallel path.
        // Body: `{"id":"...","data":"..."}`.
        "/cli/terminal/lifecycle-write" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_lifecycle_write(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/set-focus" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_set_focus(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // S7a (presence/multiplayer §5.5) — POST /cli/terminal/pin-size:
        // pin a v2 session's PTY to fixed cols×rows / clear the pin.
        // require_post (405 on GET, feedback_post_only_route_guards) +
        // token_ok (owner OR connect-user session; role tightening to
        // claimer-capable users lands with S4/S5). The recorded
        // `set_by` attribution is resolved from the TOKEN, never the
        // body (D3 — same rule as send-message's `from`). Handler
        // writes SQLite → spawn_blocking, like /cli/feedback/*.
        "/cli/terminal/pin-size" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let set_by = if super::http::token_is_owner(&query, state.token.as_str()) {
                "owner".to_string()
            } else {
                // token_ok passed and it isn't the owner token, so this
                // is a live connect-user session; resolve its username.
                // The unreachable-in-practice None (revoked in the gap
                // between the two checks) records the neutral "user".
                super::http::extract_token(&query)
                    .and_then(k2_core::connect_users::validate_session)
                    .unwrap_or_else(|| "user".to_string())
            };
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = tokio::task::spawn_blocking(move || {
                crate::terminal_routes::handle_pin_size(&body_bytes, &set_by)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // Phase 2 Unit 7c — heartbeat-launchd installer routes.
        // Daemon owns its own `dev.k2.heartbeat.plist` so
        // K2SO Connect (remote daemon without Tauri) can install +
        // remove the scheduler under its own GUI session. Method
        // gates are inline so a stray GET can't trigger a
        // launchctl bootstrap. See `crates/k2so-core/src/heartbeats/
        // install.rs` for the install/uninstall bodies.
        "/cli/heartbeat/install-launchd" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // launchctl bootstrap can stall briefly under load; F5.
            let r = tokio::task::spawn_blocking(move || {
                crate::heartbeat_routes::handle_install_launchd(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/heartbeat/uninstall-launchd" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = tokio::task::spawn_blocking(move || {
                crate::heartbeat_routes::handle_uninstall_launchd(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/heartbeat/apply-wake-scheduler" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = tokio::task::spawn_blocking(move || {
                crate::heartbeat_routes::handle_apply_wake_scheduler(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // Phase 2 Unit 7c — orphan-agent sweep, refactored out of
        // src-tauri/src/commands/projects.rs's agent_mode-change
        // path. Body: `{"project_path": "/path"}`.
        "/cli/agents/archive-orphans" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // fs walk + db lock — F5.
            let r = tokio::task::spawn_blocking(move || {
                handle_archive_orphans(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // K2 Connect host-awareness GAP — workspace skill / agent /
        // session / relations / heartbeat-flag / onboarding POST writes.
        // Each wraps the same k2_core fn the renderer's old LOCAL Tauri
        // command called, so the write lands on whichever daemon the
        // renderer is actually talking to (local OR remote). JSON-bodied;
        // method-gated by the `is_post && post_allowed` arm guard + the
        // explicit `require_post` is unnecessary here (the guard IS the
        // gate — a GET on these paths can't match this arm and falls
        // through to the catchall → 404, never a silent mutation).
        // Token-gated like every /cli data route. F5: FS-walk / DB-lock
        // work runs on a blocking thread.
        // 0.39.39 #676 — POST /cli/workspace/set-tab-title. Daemon-
        // canonical tab title write { project, tabId, title }; upserts
        // the `tab_titles` store + broadcasts `TabTitleChanged`. Owner-
        // OR-connect-user auth (token_ok) — a remote connect-user driving
        // a host's tabs is legitimate, same tier as every other /cli data
        // write. Method-gated by this explicit arm (a GET falls through to
        // the catchall → 404, never a silent mutation). DB-lock work runs
        // on a blocking thread (F5).
        p if is_post && post_allowed && p == "/cli/workspace/set-tab-title" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = tokio::task::spawn_blocking(move || {
                crate::db_routes::handle_set_tab_title(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // POST /cli/browser/open-url — 0.40.34 browser-open forwarding.
        // Body (form `url=` from the shim's curl --data-urlencode, or
        // JSON `{"url": ...}`): validate http/https-only + length cap,
        // then broadcast the app-level `open_url` session event so every
        // connected app (local or across the K2 Connect tunnel) can open
        // it in a browser tab. NO local open/xdg-open shell-out on this
        // route, ever. token_ok (owner OR connect-user, same tier as the
        // other /cli data writes — low risk: the payload is a validated
        // URL broadcast, no filesystem/exec surface) + require_post per
        // the feedback_post_only_route_guards house rule. Emit is a
        // non-blocking broadcast send, so no spawn_blocking needed.
        p if is_post && post_allowed && p == "/cli/browser/open-url" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let result = crate::browser_routes::handle_open_url(&params, &body_bytes, "shim");
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // POST /cli/workspace/api-key — B3a (sandbox). Set/clear the
        // PER-WORKSPACE Anthropic API key (BYO key). Body (JSON):
        // `{"project": "<path>", "key": "<api key>"}` (empty `key` clears it).
        // OWNER-ONLY: the key is the workspace's billable credential, so a
        // connect-user session token is rejected (require_owner, not token_ok).
        // POST + body so the secret is never URL-logged. The handler NEVER
        // logs/echoes the key.
        p if is_post && post_allowed && p == "/cli/workspace/api-key" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = tokio::task::spawn_blocking(move || {
                crate::misc_routes::handle_set_workspace_api_key(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Host catalog library create/delete — isolated from the
        // `/cli/context/` token_ok prefix so Member/Viewer cannot author
        // packs. require_post + require_manage (owner token or Admin/Owner).
        p if is_post
            && post_allowed
            && (p == "/cli/context/catalog/create" || p == "/cli/context/catalog/delete") =>
        {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if super::http::require_manage(&mut *stream, &mut buf, &query, state.token.as_str())
                .await
                .is_none()
            {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::context_routes::dispatch_post(&p_owned, &body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Context management stack — `/cli/context/*` mutations (add / remove /
        // set-enabled / move / regen). JSON-bodied POSTs; token_ok +
        // require_post. Handlers run in spawn_blocking (SQLite + FS compose).
        // Catalog create/delete are handled above (require_manage).
        p if is_post && post_allowed && p.starts_with("/cli/context/") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::context_routes::dispatch_post(&p_owned, &body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // POST /cli/workspace/set — 0.40.24 S2 (agent CLI settings plane).
        // Multi-field per-workspace settings write. Body (JSON):
        // `{"project": "<name|path|uuid>", "fields": {"agent_mode": "k2", ...}}`.
        // token_ok (owner or connect-user session, same tier as the other
        // workspace-scoped writes) + require_post per the
        // feedback_post_only_route_guards house rule.
        p if is_post && post_allowed && p == "/cli/workspace/set-handle" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = tokio::task::spawn_blocking(move || {
                crate::workspace_routes::handle_set_handle(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        p if is_post && post_allowed && p == "/cli/workspace/set" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = tokio::task::spawn_blocking(move || {
                crate::workspace_routes::handle_workspace_set(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        p if is_post
            && post_allowed
            && (p == "/cli/workspace/resources/add" || p == "/cli/workspace/resources/remove") =>
        {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                if let Some(obj) = v.as_object() {
                    for (k, val) in obj {
                        let s = match val {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            serde_json::Value::Number(n) => n.to_string(),
                            _ => continue,
                        };
                        if !s.is_empty() {
                            params.insert(k.clone(), s);
                        }
                    }
                }
            }
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::workspace_resources_routes::dispatch_post(&p_owned, &params)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(
                &mut *stream,
                result.status,
                result.content_type,
                &result.body,
            )
            .await;
        }
        // POST /cli/agent/retire — 0.40.24 S4 (agent CLI safe
        // decommission). Body (JSON): `{"q": "<name|path|uuid>",
        // "force": bool, "dryRun": bool, "archiveTo": "<dir>"}`.
        // Guards refuse with 409 (CLI exit 3) instead of prompting;
        // success stops the live session, unwires edges, deregisters,
        // cleans non-cascaded rows, and ARCHIVES the folder (never
        // deletes). token_ok + require_post per the
        // feedback_post_only_route_guards house rule.
        p if is_post && post_allowed && p == "/cli/agent/retire" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = tokio::task::spawn_blocking(move || {
                crate::agent_retire::handle_agent_retire(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        p if is_post
            && post_allowed
            && (p.starts_with("/cli/skills/")
                || p == "/cli/onboarding/set-harness-fanout-enabled"
                || p == "/cli/onboarding/harness-fanout-enabled"
                || p == "/cli/onboarding/set-agents-md-generate-enabled"
                || p == "/cli/onboarding/agents-md-generate-enabled"
                || p == "/cli/canonical/detect-state"
                || p == "/cli/agents/regenerate-workspace-skill"
                || p == "/cli/agents/save-agent-md"
                || p == "/cli/agents/disable-workspace-claude-md"
                || p == "/cli/agents/run-workspace-ingest"
                || p == "/cli/agents/save-session-id"
                || p == "/cli/session/set-surfaced"
                || p == "/cli/heartbeat/set-show-sessions"
                || p == "/cli/relations/create"
                || p == "/cli/relations/delete") =>
        {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // C1: relations create/delete need owner-or-admin OR the
            // agents_can_create_connections toggle (same bar as
            // /cli/connections add|remove).
            let actor_is_privileged =
                super::http::token_is_owner_or_admin(&query, state.token.as_str());
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                dispatch_connect_gap_post(&p_owned, &body_bytes, actor_is_privileged)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Phase 2 Unit 4 — POST routes for git (libgit2 ops). F5:
        // spawn_blocking because diff/merge/status on large repos
        // can block for 100s of ms.
        p if is_post && post_allowed && p.starts_with("/cli/git/") => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::git_routes::dispatch_unit4_git_post(&p_owned, &body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // W6 (0.40.30) — agent-preset MUTATIONS are management-plane
        // writes: a preset row decides what command launches, with what
        // env, and which auto-approve flags the host-session strip
        // trusts — so unlike the generic Unit-4 arm below (any authed
        // session), these gate on require_owner_or_admin (#660 tier,
        // same as daemon update). Explicitly POST-gated per
        // feedback_post_only_route_guards (a stray GET here → 405, not
        // the catchall 404). Handlers stay in `dispatch_unit4_post`.
        // Reads (`/cli/presets/list`, `/cli/presets/get`) stay on the
        // generic GET dispatch — any authed session may look.
        "/cli/presets/create" | "/cli/presets/update" | "/cli/presets/delete"
        | "/cli/presets/reorder" | "/cli/presets/reset" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::require_owner_or_admin(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::db_routes::dispatch_unit4_post(&path, &body_bytes);
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Phase 2 Unit 4 — POST routes for DB-writing domains. JSON-
        // bodied writes; per-route allowlist + same implicit gate
        // pattern as Unit 6. Dispatch is `dispatch_unit4_post`.
        // (`/cli/presets/*` mutations moved to the owner/admin arm
        // above — W6.)
        p if is_post && post_allowed && (
            p.starts_with("/cli/workspaces/")
                || p.starts_with("/cli/focus-groups/")
                || p.starts_with("/cli/sections/")
                || p.starts_with("/cli/workspace-layouts/")
                || p.starts_with("/cli/timer/")
                || p.starts_with("/cli/window-state/")
                || p.starts_with("/cli/projects/")
        ) => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::db_routes::dispatch_unit4_post(p, &body_bytes);
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // K2 Connect remote-files Phase 2 — POST /cli/fs/upload-binary.
        // Writes an uploaded file's bytes onto the daemon's disk
        // (`<workspace>/.k2so/downloads` for the terminal-drop case).
        //
        // SANDBOX/AUTH DECISION: gated by `token_ok` (any authed user —
        // owner token OR a connect-user session), matching every other
        // `/cli/fs/*` data route. This arm is split out from the shared
        // `/cli/fs/` arm below SO THE GATE IS ISOLATED: tightening upload
        // to `require_manage`/`require_owner` later is a ONE-LINE swap
        // here, with no effect on the read/edit fs routes. `post_allowed`
        // + this explicit arm form the method gate (a GET falls through to
        // the catchall → 404).
        p if is_post && post_allowed && p == "/cli/fs/upload-binary" => {
            // ── isolated upload auth gate (swap this one line) ──
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::fs_routes::handle_upload_binary(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // K2 Connect "Clone to" — POST /cli/fs/upload-chunk. Streaming upload
        // for LARGE bundles (GH #3): one ordered chunk per request, appended
        // to a temp `.part` on the daemon's disk and finalized on is_last.
        // Same isolated `token_ok` gate as upload-binary (one-line swap to
        // tighten). Body carries the chunk bytes, never URL-logged.
        p if is_post && post_allowed && p == "/cli/fs/upload-chunk" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::fs_routes::handle_upload_chunk(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // 0.40.22 — POST /cli/fs/compress + /cli/fs/compress-cancel.
        // Server-side folder → zip as an async job (worker thread streams
        // the archive; `GET /cli/fs/compress-status` polls it). Matched on
        // path ALONE (no is_post guard) so a stray GET hits require_post's
        // explicit 405 per feedback_post_only_route_guards. Same isolated
        // `token_ok` gate as upload-binary (one-line swap to tighten).
        p if p == "/cli/fs/compress" || p == "/cli/fs/compress-cancel" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = if p == "/cli/fs/compress" {
                crate::fs_routes::handle_compress(&body_bytes)
            } else {
                crate::fs_routes::handle_compress_cancel(&body_bytes)
            };
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/fs/extract + /cli/fs/extract-cancel — server-side zip
        // → folder (inverse of compress). Same require_post + token_ok
        // gate; `GET /cli/fs/extract-status` polls via misc_routes.
        p if p == "/cli/fs/extract" || p == "/cli/fs/extract-cancel" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = if p == "/cli/fs/extract" {
                crate::fs_routes::handle_extract(&body_bytes)
            } else {
                crate::fs_routes::handle_extract_cancel(&body_bytes)
            };
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // 0.40.22 "Clone to this computer" — POST /cli/clone/pack +
        // /cli/clone/pack-cleanup. Pull-pack as an async job (worker thread
        // builds the bundle; `GET /cli/clone/pack-status` polls it;
        // `cleanup` reclaims the bundle after the client's download).
        // Matched on path ALONE (no is_post guard) so a stray GET hits
        // require_post's explicit 405 per feedback_post_only_route_guards.
        // Same isolated `token_ok` gate as the clone bundle/unpack arm.
        p if p == "/cli/clone/pack" || p == "/cli/clone/pack-cleanup" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = if p == "/cli/clone/pack" {
                crate::clone_routes::handle_clone_pack(&body_bytes)
            } else {
                crate::clone_routes::handle_clone_pack_cleanup(&body_bytes)
            };
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // K2 Connect "Clone to" P2 — POST /cli/clone/bundle + /cli/clone/unpack.
        //
        // `bundle` (SOURCE side) builds the scrubbed tar.gz + captures the
        // source workspace's K2 settings; `unpack` (DESTINATION side)
        // extracts at recomputed paths, registers the folder as a project,
        // and applies the manifest settings.
        //
        // SANDBOX/AUTH DECISION: gated by `token_ok` (any authed user —
        // owner token OR a connect-user session), same isolated-gate
        // pattern as `fs/upload-binary`. Split into its own arm AHEAD of the
        // shared `/cli/fs/` POST arm so tightening to `require_manage` later
        // is a one-line swap here with no effect on the fs routes.
        p if is_post && post_allowed
            && (p == "/cli/clone/bundle" || p == "/cli/clone/unpack") =>
        {
            // ── isolated clone auth gate (swap this one line) ──
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = if p == "/cli/clone/bundle" {
                crate::clone_routes::handle_clone_bundle(&body_bytes)
            } else {
                crate::clone_routes::handle_clone_unpack(&body_bytes)
            };
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // Phase 2 Unit 6 — POST routes for filesystem / chat /
        // themes / skill-layers / review-checklist. All JSON-bodied;
        // delegate to per-domain modules. The match-arm guard
        // (`is_post && post_allowed && starts_with`) is the implicit
        // method gate — a GET on these paths falls through to the
        // generic `/cli/` catchall below, which returns a 404
        // "unknown route" since dispatch doesn't have GET handlers
        // for these paths. Functionally equivalent to Unit 5/7a's
        // explicit 405s; the response code differs but no silent
        // mutation is possible either way.
        p if is_post && post_allowed && (
            p.starts_with("/cli/fs/")
                || p.starts_with("/cli/chat/")
                || p.starts_with("/cli/sandbox/")
                || p.starts_with("/cli/themes/")
                || p.starts_with("/cli/skill-layers/")
                || p.starts_with("/cli/review-checklist/")
        ) => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = dispatch_unit6_post(p, &body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // Phase 2.1 — workspace inbox POST routes. Query-string only
        // (no body). Token gate is explicit per method-gate rule;
        // body is drained to keep the connection clean. Filesystem
        // operations run in spawn_blocking per F5 (atomic-rename of
        // a `.md` file isn't slow, but `safe_delete::trash` calls
        // into macOS Finder via AppleScript and CAN block).
        // Feedback F1 — `/cli/feedback/*` mutations (create / comment /
        // answer / resolve). JSON-bodied POSTs; token_ok (owner OR
        // connect-user session — connect users see + answer feedback,
        // PRD §4.3) + require_post per feedback_post_only_route_guards.
        // Handlers run in spawn_blocking (SQLite writes).
        p if is_post && post_allowed && p.starts_with("/cli/thread/") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            let (auth_ok, scoped_principal) =
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str());
            if !auth_ok {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            if let Some(v) = {
                let presented = bearer_token
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .or_else(|| super::http::extract_token(&query));
                presented.and_then(|t| crate::session_token::require_hook(t, p))
            } {
                params.insert("cell_session_id".to_string(), v.session_id);
            }
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
            }
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::overlay_routes::dispatch_post(&p_owned, &params, &body_bytes)
                })
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        p if is_post && post_allowed && p.starts_with("/cli/feedback/") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let session_author = if super::http::token_is_owner(&query, state.token.as_str()) {
                "owner".to_string()
            } else {
                super::http::extract_token(&query)
                    .and_then(k2_core::connect_users::validate_session)
                    .unwrap_or_else(|| "owner".to_string())
            };
            let result = tokio::task::spawn_blocking(move || {
                crate::feedback_routes::dispatch_post_as(&p_owned, &body_bytes, &session_author)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // DNS K1 — `/cli/dns/*` mutations. JSON-bodied POSTs;
        // token_ok OR scoped require_hook (same dual-auth as mail).
        // Handlers run in spawn_blocking (blocking reqwest to the web
        // DNS API + SQLite toggle reads). Principal stamped for scoped
        // callers; zone create/delete are owner-only local rejects.
        p if is_post && post_allowed && p.starts_with("/cli/dns/") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            let (auth_ok, scoped_principal) =
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str());
            if !auth_ok {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::dns_routes::dispatch_post(&p_owned, &body_bytes)
                })
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // K2 Mail — `/cli/mail/*` mutations. JSON-bodied POSTs;
        // token_ok + require_post per feedback_post_only_route_guards.
        // OWNER-OR-ADMIN additionally gates the server/domain/config/
        // approvals paths (PRD §10: server lifecycle + domains + mode/
        // relay config + approvals are owner surface; address
        // create/delete + send/reply stay workspace-token so agents
        // can act — their own gating is the mail_agent_send mode +
        // cap, enforced handler-side). Handlers run in spawn_blocking
        // (SQLite writes + blocking Stalwart dials; S5's send/reply
        // `--wait` holds the request up to 900 s).
        p if is_post && post_allowed && p.starts_with("/cli/mail/") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            let (auth_ok, scoped_principal) =
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str());
            if !auth_ok {
                // #34: valid scoped passport on owner surface → owner_only
                // (exit 3), not opaque "invalid or missing token".
                let _ = stream.read(&mut buf).await;
                let r = mail_dual_auth_failure(p, &query, bearer_token.as_deref());
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body)
                    .await;
                return DispatchOutcome::Done;
            }
            if crate::mail_routes::is_owner_level_mutation(p)
                && !super::http::token_is_owner_or_admin(&query, state.token.as_str())
            {
                // #34: stable owner_only (exit 3) for non-owner callers
                // that passed dual-auth (e.g. connect-user).
                let _ = stream.read(&mut buf).await;
                let r = crate::mail_routes::owner_only_response();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body)
                    .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::mail_routes::dispatch_post(&p_owned, &body_bytes)
                })
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Projects V1 P2 — `/cli/project-group/*` mutations (create /
        // rename / delete / pin / sort / add-member / remove-member /
        // set-poc / msg / set-icon / set-color / dashboard/save-layout /
        // dashboard/rename / dashboard/create / dashboard/delete /
        // dashboard/reorder).
        // JSON-bodied POSTs; token_ok (owner OR connect-user session —
        // connect users see projects too, PRD §4.1) + require_post per
        // feedback_post_only_route_guards. Project chat post (`msg`)
        // additionally requires role ≥ Member (Owner/Admin/Member;
        // Viewers may read but cannot post). The `dashboard/*` mutations
        // — and the §6.7.7 `set-icon`/`set-color` appearance mutations
        // — are additionally owner-or-admin-gated (PRD §6.3 resolved
        // Q2: owners and admins create/rearrange/save; viewers and
        // non-admin users see but cannot change). Handlers run in
        // spawn_blocking (SQLite writes + the PoC injection's wake path
        // can block). Session author for chat attribution is resolved
        // HERE from the token (owner → "owner", connect-user → username)
        // and passed into the handler — never trusted from the body for
        // human posts (D3).
        p if is_post && post_allowed && p.starts_with("/cli/project-group/") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Project chat: Owner / Admin / Member can post; Viewer → 403.
            // Does NOT tighten dashboard/* / set-icon / set-color (those
            // stay owner-or-admin below).
            if p == "/cli/project-group/msg"
                && !super::http::token_is_at_least_member(&query, state.token.as_str())
            {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"ok":false,"error":{"code":"forbidden","hint":"viewers can read project chat but cannot post"}}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            if (p.starts_with("/cli/project-group/dashboard/")
                || p == "/cli/project-group/set-icon"
                || p == "/cli/project-group/set-color")
                && !super::http::token_is_owner_or_admin(&query, state.token.as_str())
            {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"ok":false,"error":{"code":"forbidden","hint":"dashboards and project appearance can only be changed by the owner or an admin"}}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Resolve acting identity for chat author attribution (D3).
            // Same shape as `/cli/push/*`: owner token → "owner", else
            // the connect-user session's username.
            let session_author = if super::http::token_is_owner(&query, state.token.as_str()) {
                "owner".to_string()
            } else {
                super::http::extract_token(&query)
                    .and_then(k2_core::connect_users::validate_session)
                    .unwrap_or_else(|| "owner".to_string())
            };
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::project_group_routes::dispatch_post(
                    &p_owned,
                    &body_bytes,
                    &session_author,
                )
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Companion C4 — `/cli/push/*` mutations (register-device /
        // unregister-device). JSON-bodied POSTs; token_ok (ANY authed
        // user: owner or a live connect-user session) + require_post
        // per feedback_post_only_route_guards. The acting username is
        // resolved HERE from the session token — owner token →
        // "owner", connect-user token → its session's username — and
        // passed to the handler; the request body is never trusted
        // for attribution (D3). Handlers run in spawn_blocking
        // (SQLite writes).
        p if is_post && post_allowed && p.starts_with("/cli/push/") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let username = if super::http::token_is_owner(&query, state.token.as_str()) {
                Some("owner".to_string())
            } else {
                super::http::extract_token(&query)
                    .and_then(k2_core::connect_users::validate_session)
            };
            // token_ok passed but the session vanished in between
            // (revocation race) — fail closed like the gate above.
            let Some(username) = username else {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            };
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::push_routes::dispatch_post(&p_owned, &body_bytes, &username)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // C2: dual-auth (owner OR scoped) so agent-initiated compose into
        // another workspace is principal-bound and peer-gated. Compose
        // TARGET is `project=` — preserve it across stamp_principal
        // (which otherwise rewrites project to the caller's own path).
        p if is_post && post_allowed && p.starts_with("/cli/inbox/") => {
            let (auth_ok, scoped_principal) =
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str());
            if !auth_ok {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // 0.39.45 (#35/#37): params come from the query string AND
            // the form-encoded POST body, body winning on key collision.
            // Long values (`body`, `title`) ride the body so they dodge
            // the request-head cap that silently clipped inbox memos at
            // ~2.7KB. Query-only senders (older CLIs) keep working.
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            // Preserve compose TARGET across identity stamp. Stamp rewrites
            // both keys to the caller's path — restore BOTH to the same
            // target token so need_project_path cannot prefer a stale
            // project_path and silently write to the caller's own inbox (#36).
            let compose_target = params
                .get("project")
                .or_else(|| params.get("project_path"))
                .cloned()
                .filter(|s| !s.trim().is_empty());
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
                if let Some(t) = compose_target {
                    params.insert("project".to_string(), t.clone());
                    params.insert("project_path".to_string(), t);
                }
            }
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::inbox_routes::dispatch_post(&p_owned, &params)
                })
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body).await;
        }
        // Workspace knowledge base — seed + localhost serve on/off.
        // token_ok (owner OR connect-user), same tier as fs/inbox reads.
        // spawn_blocking so serve start can Handle::block_on bind without
        // pinning an async worker; the accept loop is then tokio::spawn'd.
        p if is_post && post_allowed && p.starts_with("/cli/publish/") => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                if let Some(obj) = v.as_object() {
                    for (k, val) in obj {
                        let s = match val {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            serde_json::Value::Number(n) => n.to_string(),
                            _ => continue,
                        };
                        if !s.is_empty() {
                            params.insert(k.clone(), s);
                        }
                    }
                }
            }
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::publish_routes::dispatch_post(&p_owned, &params)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(
                &mut *stream,
                result.status,
                result.content_type,
                &result.body,
            )
            .await;
        }
        p if is_post && post_allowed && p.starts_with("/cli/wiki/") => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            // JSON body optional: { "enabled": true, "port": 0, "project": "..." }
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                if let Some(obj) = v.as_object() {
                    for (k, val) in obj {
                        let s = match val {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            serde_json::Value::Number(n) => n.to_string(),
                            _ => continue,
                        };
                        if !s.is_empty() {
                            params.insert(k.clone(), s);
                        }
                    }
                }
            }
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::wiki_routes::dispatch_post(&p_owned, &params)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") }).to_string(),
            });
            super::http::send_response(
                &mut *stream,
                result.status,
                result.content_type,
                &result.body,
            )
            .await;
        }
        // 0.39.45 (#35/#37/#29) — live-msg POST form. Same handler as
        // the GET form (crate::cli::dispatch → workspace_routes), but
        // the message `text` (and any other param) may arrive in the
        // form-encoded POST body, dodging the request-head cap that
        // silently clipped long live messages. Body wins on collision.
        // Runs in spawn_blocking: deliver_live sleeps across its
        // inject/verify/retry windows and must not pin a runtime worker.
        //
        // C2 (0.40.45): dual-auth like mail/dns — owner/connect-user via
        // token_ok (NO principal → peer-gate bypass) OR scoped require_hook
        // (principal stamped → peer gate enforces local connection).
        p if is_post && p == "/cli/workspace/msg" => {
            let (auth_ok, scoped_principal) =
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str());
            if !auth_ok {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            // Preserve recipient `workspace=` (routing); stamp identity.
            if let Some(v) = {
                let presented = bearer_token
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .or_else(|| super::http::extract_token(&query));
                presented.and_then(|t| crate::session_token::require_hook(t, p))
            } {
                params.insert("cell_session_id".to_string(), v.session_id);
            }
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
            }
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::cli::dispatch(&p_owned, &params)
                })
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body).await;
        }
        // F2 host read-back (prd-v1-api-completion §4) — `k2 respond` from a
        // NON-SANDBOXED host session, arriving over loopback TCP (host
        // sessions have no per-cell UDS jail requirement; the UDS arm in
        // `cell_server` keeps serving cells AND scoped host sessions when
        // bound). AUTH IS THE SESSION IDENTITY: only a SCOPED per-session
        // hook token (K2_HOOK_SCOPED) validates — `require_hook` structurally
        // never matches the owner token, and the owner token is deliberately
        // NOT accepted here because it names no session (identity from the
        // token, never the body — PRD §2). The append is PINNED to the
        // validated token's own session id, so a session can only ever write
        // its OWN log (cross-session append refused by construction). The
        // log is drained by `GET /v1/(w/<ws>/host-sessions|sandboxes)/<id>/messages`.
        // Body: form-encoded `message` (fallback `text`) + `final` ("1"/"true").
        // Bearer preferred; `?token=`/body `token` is the curl fallback.
        "/cli/respond" => {
            // POST-only (feedback_post_only_route_guards): a stray GET is a
            // clean 405 here, never a fall-through into the generic /cli/
            // dispatch.
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            let presented = bearer_token
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| params.get("token").cloned())
                .unwrap_or_default();
            let r = match crate::session_token::require_hook(&presented, "/cli/respond") {
                Some(validated) => {
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
                    let seq = crate::sandbox_responses::append(
                        &validated.session_id,
                        text,
                        final_,
                    );
                    // S9: work-completion reaper — non-final keeps Working;
                    // --final enters grace window (mark_complete).
                    if let Some(sid) =
                        k2_core::session::SessionId::parse(&validated.session_id)
                    {
                        crate::sandbox_reaper::on_respond(&sid, final_);
                    }
                    crate::cli_response::CliResponse::ok_json(
                        serde_json::json!({ "ok": true, "seq": seq }).to_string(),
                    )
                }
                None => crate::cli_response::CliResponse {
                    status: "403 Forbidden",
                    content_type: "application/json",
                    body: r#"{"error":"Invalid or missing auth token"}"#.to_string(),
                },
            };
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // Host-session completion lifecycle — `k2 done`. Arms Grace via
        // mark_complete ONLY; does NOT append to the respond drain ring
        // (A9: no product final message). Same scoped-hook auth as
        // `/cli/respond` (session identity from the token).
        "/cli/session/complete" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            let presented = bearer_token
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| params.get("token").cloned())
                .unwrap_or_default();
            let r = match crate::session_token::require_hook(
                &presented,
                "/cli/session/complete",
            ) {
                Some(validated) => {
                    if let Some(sid) =
                        k2_core::session::SessionId::parse(&validated.session_id)
                    {
                        crate::sandbox_reaper::mark_complete(&sid);
                    }
                    // Optional reason is logged only (v1); never a drain payload.
                    if let Some(reason) = params.get("reason").filter(|s| !s.is_empty()) {
                        k2_core::log_debug!(
                            "[session-complete] session={} reason={}",
                            validated.session_id,
                            reason
                        );
                    }
                    crate::cli_response::CliResponse::ok_json(
                        serde_json::json!({ "ok": true, "complete": true }).to_string(),
                    )
                }
                None => crate::cli_response::CliResponse {
                    status: "403 Forbidden",
                    content_type: "application/json",
                    body: r#"{"error":"Invalid or missing auth token"}"#.to_string(),
                },
            };
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // Composer Phase 1a/1c — session-scoped verified send. Mirrors the
        // /cli/workspace/msg spawn_blocking pattern (injection sleeps
        // ~520ms across its settle windows + may block on the per-session
        // lock, so it must not pin a runtime worker).
        //
        // 1c CAPABILITY GATE (D4): this route instructs an agent running
        // --dangerously-skip-permissions (= full shell+fs RCE), so the gate
        // is SERVER-ENFORCED and fails CLOSED. Authorized iff:
        //   • the OWNER token is presented (ALWAYS allowed, 1a parity), OR
        //   • a connect-user session with role >= Member AND the host has
        //     opted into remote multi-user instruction
        //     (`app_settings.allow_remote_instruct`, DEFAULT OFF).
        // Anything else → drain-then-403 (mirrors require_manage). The
        // renderer hides the composer on the same signal, but THIS is the
        // source of truth (the renderer-hide is defense-in-depth only).
        //
        // The if-!is_post guard is enforced by the top-level 405 gate
        // (post_allowed lists this route); we additionally pin `p ==`
        // here so only a POST reaches the handler body.
        p if is_post && p == "/cli/terminal/send-message" => {
            // Read + parse the body UP-FRONT. The #67 per-workspace gate
            // needs the target `session_id` (carried in the JSON/form body)
            // to resolve which workspace's opt-in to consult — so the body
            // must be read before the capability decision. Reading it here
            // also means a rejected request has already drained its body
            // before the 403 (mirrors require_manage's drain-then-403).
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // Body may be form-encoded OR JSON; query is the fallback.
            // session_id/text only — `from` is NEVER read from the body
            // (D3), it is resolved below from the token.
            let mut params = super::http::parse_params(&path, &query);
            for (k, v) in super::http::parse_form_body(&body_bytes) {
                params.insert(k, v);
            }
            if let Ok(serde_json::Value::Object(map)) =
                serde_json::from_slice::<serde_json::Value>(&body_bytes)
            {
                for key in ["session_id", "text"] {
                    if let Some(s) = map.get(key).and_then(|v| v.as_str()) {
                        params.insert(key.to_string(), s.to_string());
                    }
                }
            }
            let session_id = params.get("session_id").cloned().unwrap_or_default();
            let text = params.get("text").cloned().unwrap_or_default();

            // #67 — D4 capability decision, now PER-WORKSPACE. The opt-in
            // gates ONLY the connect-user path; the owner is allowed
            // regardless. Resolve the target session's workspace and read
            // its EFFECTIVE opt-in (per-workspace flag OR app-level master);
            // fail-closed when the session/workspace can't be resolved.
            let remote_opt_in = remote_instruct_opt_in_for_session(&session_id);
            let auth = super::http::authorize_send_message(
                &query,
                state.token.as_str(),
                remote_opt_in,
            );
            // D3 — `from` is resolved server-side ONLY, NEVER from the body:
            //   owner       → the user-set "your display name" (app_settings
            //                 `owner_display_name`, sanitized) or "owner".
            //   connect-user → their daemon-validated username.
            // `revalidate` carries the connect-user's token so we can
            // re-check it at inject time (M2 — a user revoked mid-flight
            // must not land an injection).
            let (from, revalidate): (String, Option<String>) = match auth {
                super::http::SendMessageAuth::Owner => {
                    (crate::workspace_msg::resolve_owner_from(), None)
                }
                super::http::SendMessageAuth::ConnectUser { username } => {
                    let tok = super::http::extract_token(&query)
                        .unwrap_or_default()
                        .to_string();
                    (username, Some(tok))
                }
                super::http::SendMessageAuth::Denied => {
                    // Body already read above — respond 403 directly.
                    super::http::send_response(
                        &mut *stream,
                        "403 Forbidden",
                        "application/json",
                        r#"{"error":"invalid or missing token"}"#,
                    )
                    .await;
                    return DispatchOutcome::Done;
                }
            };
            let body = tokio::task::spawn_blocking(move || {
                // M2 (inject-time half): re-validate a connect-user token
                // right before injecting. The gate validated it once, but
                // the injection blocks ~520ms+; a user revoked in that
                // window must NOT land an injection. The owner token is
                // never revoked, so `revalidate` is None for owner sends.
                if let Some(tok) = revalidate.as_deref() {
                    if k2_core::connect_users::validate_session(tok).is_none() {
                        return serde_json::json!({
                            "success": false,
                            "reason": "revoked",
                            "hint": "connect-user session was revoked before delivery"
                        })
                        .to_string();
                    }
                }
                let resp = crate::workspace_msg::send_message_to_session(
                    &session_id,
                    &from,
                    &text,
                );
                if resp.success {
                    crate::workspace_msg::persist_compose_send_after_success(
                        &session_id,
                        &from,
                        &text,
                    );
                }
                serde_json::to_string(&resp)
                    .unwrap_or_else(|_| "{\"success\":false}".to_string())
            })
            .await
            .unwrap_or_else(|e| {
                serde_json::json!({
                    "success": false,
                    "reason": "worker_join",
                    "hint": format!("{e}")
                })
                .to_string()
            });
            super::http::send_response(&mut *stream, "200 OK", "application/json", &body).await;
        }
        // Composer send history — GET last 50 sent lines for a workspace.
        // Same authorize_send_message gate as send-message (owner always;
        // connect-user only when the target workspace is opted in).
        // Scoped API tokens stay off (not an agent verb).
        p if !is_post && p == "/cli/terminal/compose-history" => {
            let _ = stream.read(&mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let workspace_path = params.get("workspace_path").cloned().unwrap_or_default();
            let project_id = params.get("project_id").cloned().unwrap_or_default();
            let opt_in_path = if !project_id.is_empty() {
                k2_core::workspace_compose_history::project_path_for_id(&project_id)
                    .unwrap_or_default()
            } else {
                k2_core::workspace_compose_history::resolve_project_id_for_path(&workspace_path)
                    .and_then(|id| k2_core::workspace_compose_history::project_path_for_id(&id))
                    .unwrap_or_default()
            };
            let remote_opt_in = if opt_in_path.is_empty() {
                k2_core::app_settings::load().allow_remote_instruct
            } else {
                k2_core::workspace::settings::remote_instruct_allowed_for_path(&opt_in_path)
            };
            match super::http::authorize_send_message(
                &query,
                state.token.as_str(),
                remote_opt_in,
            ) {
                super::http::SendMessageAuth::Denied => {
                    super::http::send_response(
                        &mut *stream,
                        "403 Forbidden",
                        "application/json",
                        r#"{"error":"invalid or missing token"}"#,
                    )
                    .await;
                    return DispatchOutcome::Done;
                }
                super::http::SendMessageAuth::Owner
                | super::http::SendMessageAuth::ConnectUser { .. } => {}
            }
            let body = crate::workspace_msg::compose_history_response(&project_id, &workspace_path);
            super::http::send_response(&mut *stream, "200 OK", "application/json", &body).await;
        }
        // ── P3a (sandbox / K2-as-a-server) — API-key auth-tier MANAGEMENT.
        //
        // OWNER-TIER (F4, prd-v1-api-completion §6) + ALWAYS-ON: minting,
        // listing, and revoking keys authorizes on the owner TOKEN or an
        // Owner-ROLE connect session (`api_key_manager_identity` — the same
        // `can_change_roles` bar as /cli/users/set-role), because hosted
        // customers only ever hold the session, never the daemon token.
        // Admin-role does NOT get key management, and an API key CANNOT
        // manage keys (a `k2sk_…` token is neither the owner token nor a
        // session — this gate never consults v1_principal). A
        // must-change-password session never reaches here: the
        // `session_password_gate` chokepoint above already 403s it.
        // Always-on so the owner can pre-create keys before flipping the
        // external /v1/* surface live (harmless while /v1/* is dark). The two
        // POSTs are method-gated per-handler (require_post); `list` is a GET.
        // The minted RAW key is returned ONCE by create + never logged; the
        // resolved actor identity feeds the create/revoke audit log lines.
        p if p.starts_with("/cli/api-keys/") => {
            let r = match p {
                "/cli/api-keys/create" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    let Some(actor) =
                        super::http::api_key_manager_identity(&query, state.token.as_str())
                    else {
                        let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                        let f = crate::cli_response::CliResponse::forbidden();
                        super::http::send_response(&mut *stream, f.status, f.content_type, &f.body).await;
                        return DispatchOutcome::Done;
                    };
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || crate::misc_routes::handle_api_key_create(&body, &actor))
                        .await
                        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/api-keys/revoke" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    let Some(actor) =
                        super::http::api_key_manager_identity(&query, state.token.as_str())
                    else {
                        let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                        let f = crate::cli_response::CliResponse::forbidden();
                        super::http::send_response(&mut *stream, f.status, f.content_type, &f.body).await;
                        return DispatchOutcome::Done;
                    };
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || crate::misc_routes::handle_api_key_revoke(&body, &actor))
                        .await
                        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/api-keys/disable" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    let Some(actor) =
                        super::http::api_key_manager_identity(&query, state.token.as_str())
                    else {
                        let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                        let f = crate::cli_response::CliResponse::forbidden();
                        super::http::send_response(&mut *stream, f.status, f.content_type, &f.body).await;
                        return DispatchOutcome::Done;
                    };
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || {
                        crate::misc_routes::handle_api_key_disable(&body, &actor)
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/api-keys/enable" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    let Some(actor) =
                        super::http::api_key_manager_identity(&query, state.token.as_str())
                    else {
                        let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                        let f = crate::cli_response::CliResponse::forbidden();
                        super::http::send_response(&mut *stream, f.status, f.content_type, &f.body).await;
                        return DispatchOutcome::Done;
                    };
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || {
                        crate::misc_routes::handle_api_key_enable(&body, &actor)
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/api-keys/list" => {
                    // GET, owner-tier. Drain the peeked head then gate.
                    let _ = stream.read(&mut buf).await;
                    if super::http::api_key_manager_identity(&query, state.token.as_str()).is_none() {
                        crate::cli_response::CliResponse::forbidden()
                    } else {
                        tokio::task::spawn_blocking(crate::misc_routes::handle_api_key_list)
                            .await
                            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                    }
                }
                _ => {
                    let _ = stream.read(&mut buf).await;
                    crate::cli_response::CliResponse::not_found()
                }
            };
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // GET /v1/jwks — PUBLIC (unauthenticated), same tier as /boot-status.
        // Capability envelope verify requires the verifier (e.g. Scout) to
        // fetch public keys with NO K2 secret. Gating this behind an API key
        // reintroduced "app holds a long-lived K2 credential" and broke the
        // ES256+JWKS contract (pilot finding #1, 0.40.76). Public keys only —
        // no secrets; standard OIDC/JWKS practice. Served even when the /v1
        // API surface is dark (keys remain public regardless of spawn doors).
        "/v1/jwks" => {
            let _ = stream.read(&mut buf).await;
            let r = crate::v1_capabilities::handle_v1_jwks();
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // ── P3a (sandbox / K2-as-a-server) — the EXTERNAL `/v1/*` surface.
        //
        // DARK BY DEFAULT: with the surface gate OFF (the shipped default)
        // EVERY `/v1/*` path 404s exactly as if the routes didn't exist — the
        // whole external surface is absent and flag-off is byte-identical to
        // no surface. F3 gate split (prd-v1-api-completion §5): the surface
        // gate is `api_enabled()` = K2_API truthy OR the legacy K2_SANDBOX_API
        // (back-compat implies). The SANDBOX route families additionally
        // require `sandbox_api_enabled()` via the guard arms below; with K2_API
        // on but K2_SANDBOX_API off they return the same uniform 404 as any
        // unknown /v1 path (surface-absent — 409 stays reserved for "API on,
        // engine can't sandbox", the `can_sandbox()` check inside the
        // handlers). When ON, each route gates on `v1_principal` (owner token
        // OR a valid non-revoked API key, Bearer-preferred).
        //
        // EXCEPTION: `/v1/jwks` is handled ABOVE without auth (public keys).
        p if p.starts_with("/v1/") => {
            if !crate::misc_routes::api_enabled() {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "404 Not Found",
                    "application/json",
                    r#"{"error":"not found"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // F3: sampled ONCE per request, consulted by the sandbox-family
            // guard arms below. Auth stays FIRST (below) so an unauthenticated
            // probe can't distinguish a gated-off sandbox route (401) from any
            // other /v1 path (also 401) — no gate-state oracle.
            let sandbox_on = crate::misc_routes::sandbox_api_enabled();
            // Authenticate → V1Principal (owner token or a valid API key). The
            // Bearer header is preferred (keeps the secret out of the URL);
            // `?token=` is the fallback. None → 401.
            let principal = super::http::v1_principal(
                &query,
                bearer_token.as_deref(),
                state.token.as_str(),
            );
            let Some(principal) = principal else {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "401 Unauthorized",
                    "application/json",
                    r#"{"error":"invalid or missing API key"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            };
            let r = match p {
                "/v1/ping" => {
                    let _ = stream.read(&mut buf).await;
                    crate::misc_routes::handle_v1_ping(&principal.display_id())
                }
                // F3 gate split: with K2_API on but K2_SANDBOX_API off, the
                // `/v1/sandboxes*` family is SURFACE-ABSENT — the same uniform
                // 404 an unknown /v1 path gets, checked BEFORE any method/shape
                // handling so a stray GET can't draw a 405 oracle. (409 stays
                // reserved for "surface on, engine can't sandbox" inside the
                // handlers.)
                _ if !sandbox_on
                    && (p == "/v1/sandboxes" || p.starts_with("/v1/sandboxes/")) =>
                {
                    let _ = stream.read(&mut buf).await;
                    crate::cli_response::CliResponse::not_found()
                }
                // P3b — POST /v1/sandboxes: the public sandbox-spawn route. The
                // principal is host-resolved above; the policy-resolver inside
                // produces a host-trusted SpawnRequest (the cell/caller never
                // decides workspace/command/env/creds/identity), and the route
                // 409s if this daemon can't deliver a real microVM. POST-only.
                "/v1/sandboxes" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    crate::v1_sandboxes::handle_v1_sandboxes(&principal, &body)
                }
                // F2 — GET /v1/sandboxes/<id>/messages?since=<seq>: drain the
                // in-cell agent's response log. GET-only (a POST to this path is
                // not POST-allowlisted, so the top-level guard 405s it before
                // here). AUTHZ is inside `handle_messages` (default-deny: the
                // requesting principal must OWN the session, else 404). The exact
                // `match p` above can't catch the `<id>` segment, so this guard
                // arm matches the prefix+suffix and parses the id + `since`.
                _ if p.starts_with("/v1/sandboxes/") && p.ends_with("/messages") => {
                    let _ = stream.read(&mut buf).await;
                    // Phase 0: sandboxes capability (shared drain is also used
                    // by host-sessions; only THIS entry point is sandbox-gated).
                    if let Err(resp) = crate::v1_sandboxes::require_sandboxes(&principal) {
                        resp
                    } else {
                        let id = p
                            .strip_prefix("/v1/sandboxes/")
                            .and_then(|s| s.strip_suffix("/messages"))
                            .unwrap_or("");
                        let since = super::http::parse_params(&path, &query)
                            .get("since")
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                        // A multi-segment / empty id is never a real session id —
                        // 404 without an ownership probe.
                        if id.is_empty() || id.contains('/') {
                            crate::cli_response::CliResponse::not_found()
                        } else {
                            crate::v1_sandboxes::handle_messages(&principal, id, since)
                        }
                    }
                }
                // ── Sandbox v2 (PRD §A) — WORKSPACE-SCOPED session front door:
                //   POST /v1/w/<ws>/sessions                    → new (or fork
                //        when body carries `fork_from`) session in <ws>
                //   POST /v1/w/<ws>/sessions/<id>               → address an
                //        existing sandbox session (message/resume intent)
                //   GET  /v1/w/<ws>/sessions/<id>/messages?since=<n> → drain
                //   GET  /v1/w/<ws>/sessions                    → list <ws>'s
                //        sandbox sessions (audit; empty in slice 1)
                //
                // Hire (Julie 2): exact `POST /v1/w` — MUST run before the
                // `/v1/w/<slug>/…` parser. GET → 405 (POST-only).
                "/v1/w" => {
                    if !is_post {
                        let _ = stream.read(&mut buf).await;
                        crate::v1_hire::handle_v1_w(&principal, false, &[])
                    } else {
                        let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                        crate::v1_hire::handle_v1_w(&principal, true, &body)
                    }
                }
                // The exact `match p` above can't catch the `<ws>`/`<id>`
                // segments, so this guard arm parses them manually (mirrors the
                // `/v1/sandboxes/.../messages` guard) and branches on `is_post`.
                // Every path is behind the same default-OFF gate + `v1_principal`
                // auth resolved above; slug resolution + per-key workspace authz
                // + the canonical-off-limits guard live inside the handlers.
                _ if p.starts_with("/v1/w/") => {
                    // Segments AFTER the `/v1/w/` prefix. A trailing `/` or an
                    // empty segment yields an empty element → the shape checks
                    // below reject it (uniform 404, never a 500 / oracle).
                    let rest = p.strip_prefix("/v1/w/").unwrap_or("");
                    let segs: Vec<&str> = rest.split('/').collect();
                    match (segs.as_slice(), is_post) {
                        // F3 gate split: the workspace SANDBOX-SESSION family
                        // (`/v1/w/<ws>/sessions…`) requires K2_SANDBOX_API on
                        // top of the surface gate — when off it is surface-
                        // absent (the same uniform 404 as any unknown /v1
                        // path), checked BEFORE method/shape handling. The
                        // canonical-agent `/v1/w/<ws>/message` route below is
                        // NOT sandbox-gated: it ships with K2_API alone.
                        ([_, "sessions", ..], _) if !sandbox_on => {
                            let _ = stream.read(&mut buf).await;
                            crate::cli_response::CliResponse::not_found()
                        }
                        // POST /v1/w/<ws>/sessions — new / fork.
                        ([ws, "sessions"], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            crate::v1_sandboxes::handle_v1_ws_new(&principal, ws, &body)
                        }
                        // GET /v1/w/<ws>/sessions — list (audit).
                        ([ws, "sessions"], false) => {
                            let _ = stream.read(&mut buf).await;
                            crate::v1_sandboxes::handle_v1_ws_list(&principal, ws)
                        }
                        // POST /v1/w/<ws>/sessions/<id> — address a session.
                        ([ws, "sessions", sid], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            crate::v1_sandboxes::handle_v1_ws_address(&principal, ws, sid, &body)
                        }
                        // GET /v1/w/<ws>/sessions/<id>/messages?since=<n>.
                        ([ws, "sessions", sid, "messages"], false) => {
                            let _ = stream.read(&mut buf).await;
                            let since = super::http::parse_params(&path, &query)
                                .get("since")
                                .and_then(|s| s.parse::<u64>().ok())
                                .unwrap_or(0);
                            crate::v1_sandboxes::handle_v1_ws_messages(&principal, ws, sid, since)
                        }
                        // POST /v1/w/<ws>/message — message the workspace's
                        // CANONICAL agent (talk semantics: live-inject or
                        // wake+resume+inject), gated by remote-instruct opt-in +
                        // a busy/HITL guard. NOT a sandbox cell.
                        ([ws, "message"], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            crate::v1_ws_message::handle_v1_ws_message(&principal, ws, &body)
                        }
                        // Julie 3 — POST /v1/w/<ws>/wiki/notes (write one note).
                        ([ws, "wiki", "notes"], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            crate::v1_hire::handle_v1_wiki_notes(&principal, ws, &body)
                        }
                        ([_ws, "wiki", "notes"], false) => {
                            let _ = stream.read(&mut buf).await;
                            crate::cli_response::CliResponse::method_not_allowed()
                        }
                        // Julie 4 — GET /v1/w/<ws>/context (layer list).
                        ([ws, "context"], false) => {
                            let _ = stream.read(&mut buf).await;
                            crate::v1_hire::handle_v1_context_list(&principal, ws)
                        }
                        // POST /v1/w/<ws>/context — catalog XOR inline layer.
                        ([ws, "context"], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            crate::v1_hire::handle_v1_context_add(&principal, ws, &body)
                        }
                        ([ws, "context", "remove"], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            crate::v1_hire::handle_v1_context_remove(&principal, ws, &body)
                        }
                        ([_ws, "context", "remove"], false) => {
                            let _ = stream.read(&mut buf).await;
                            crate::cli_response::CliResponse::method_not_allowed()
                        }
                        ([ws, "context", "regen"], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            crate::v1_hire::handle_v1_context_regen(&principal, ws, &body)
                        }
                        ([_ws, "context", "regen"], false) => {
                            let _ = stream.read(&mut buf).await;
                            crate::cli_response::CliResponse::method_not_allowed()
                        }
                        // ── F1 (prd-v1-api-completion §3) — NON-SANDBOXED
                        // HOST SESSIONS. Gated on the /v1 SURFACE gate only
                        // (K2_API / legacy implies) — deliberately NOT the
                        // sandbox-family gate: this family exists on EVERY
                        // host, sandbox-capable or not (the whole point).
                        // Responses are honestly labeled `"sandbox":"none"`.
                        // The blocking work (DB + PTY spawn + the locked
                        // injector's settle sleeps) runs on the blocking
                        // pool, mirroring the federation arms.
                        //
                        // POST /v1/w/<ws>/host-sessions — spawn (or resume
                        // with {"session": id}).
                        ([ws, "host-sessions"], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            let ws = ws.to_string();
                            let principal = principal.clone();
                            tokio::task::spawn_blocking(move || {
                                crate::v1_host_sessions::handle_v1_host_new(&principal, &ws, &body)
                            })
                            .await
                            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                        }
                        // GET /v1/w/<ws>/host-sessions — list (audit).
                        ([ws, "host-sessions"], false) => {
                            let _ = stream.read(&mut buf).await;
                            crate::v1_host_sessions::handle_v1_host_list(&principal, ws)
                        }
                        // ── Spawn-queue routes (prd-host-session-spawn-queue-v1).
                        // MUST register before generic `host-sessions/<sid>` so
                        // sid ≠ "queue" (product lock 9).
                        // GET …/host-sessions/queue — list open jobs.
                        ([ws, "host-sessions", "queue"], false) => {
                            let _ = stream.read(&mut buf).await;
                            let ws = ws.to_string();
                            let principal = principal.clone();
                            tokio::task::spawn_blocking(move || {
                                crate::v1_host_sessions::handle_v1_host_queue_list(
                                    &principal, &ws,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| {
                                crate::cli_response::CliResponse::internal_error(e)
                            })
                        }
                        // POST …/queue is not a spawn surface → 405 (GET list only).
                        ([_ws, "host-sessions", "queue"], true) => {
                            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                            crate::cli_response::CliResponse::method_not_allowed()
                        }
                        // GET …/host-sessions/queue/<jobId>.
                        ([ws, "host-sessions", "queue", job_id], false) => {
                            let _ = stream.read(&mut buf).await;
                            let (ws, job_id) = (ws.to_string(), job_id.to_string());
                            let principal = principal.clone();
                            tokio::task::spawn_blocking(move || {
                                crate::v1_host_sessions::handle_v1_host_queue_get(
                                    &principal, &ws, &job_id,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| {
                                crate::cli_response::CliResponse::internal_error(e)
                            })
                        }
                        // POST …/host-sessions/queue/<jobId>/cancel (NOT DELETE).
                        ([ws, "host-sessions", "queue", job_id, "cancel"], true) => {
                            let _body =
                                super::http::read_post_body(&mut *stream, &mut buf).await;
                            let (ws, job_id) = (ws.to_string(), job_id.to_string());
                            let principal = principal.clone();
                            tokio::task::spawn_blocking(move || {
                                crate::v1_host_sessions::handle_v1_host_queue_cancel(
                                    &principal, &ws, &job_id,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| {
                                crate::cli_response::CliResponse::internal_error(e)
                            })
                        }
                        // GET on cancel path → 405 POST required.
                        ([_ws, "host-sessions", "queue", _job_id, "cancel"], false) => {
                            let _ = stream.read(&mut buf).await;
                            crate::cli_response::CliResponse::method_not_allowed()
                        }
                        // POST /v1/w/<ws>/host-sessions/<id> — message-live.
                        ([ws, "host-sessions", sid], true) => {
                            let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                            let (ws, sid) = (ws.to_string(), sid.to_string());
                            let principal = principal.clone();
                            tokio::task::spawn_blocking(move || {
                                crate::v1_host_sessions::handle_v1_host_message(
                                    &principal, &ws, &sid, &body,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                        }
                        // GET /v1/w/<ws>/host-sessions/<id> — status
                        // (started/live/phase). Bare three-segment GET — does
                        // not collide with …/messages or …/kill (four-seg).
                        ([ws, "host-sessions", sid], false) => {
                            let _ = stream.read(&mut buf).await;
                            crate::v1_host_sessions::handle_v1_host_status(&principal, ws, sid)
                        }
                        // GET /v1/w/<ws>/host-sessions/<id>/messages?since=<n>.
                        ([ws, "host-sessions", sid, "messages"], false) => {
                            let _ = stream.read(&mut buf).await;
                            let since = super::http::parse_params(&path, &query)
                                .get("since")
                                .and_then(|s| s.parse::<u64>().ok())
                                .unwrap_or(0);
                            crate::v1_host_sessions::handle_v1_host_messages(
                                &principal, ws, sid, since,
                            )
                        }
                        // POST /v1/w/<ws>/host-sessions/<id>/kill — force-stop
                        // the live PTY (integrator spend-cap). Empty body OK;
                        // drain any POST body so keep-alive clients don't stall.
                        // Must match as a four-segment arm (before the catch-all).
                        ([ws, "host-sessions", sid, "kill"], true) => {
                            let _body =
                                super::http::read_post_body(&mut *stream, &mut buf).await;
                            let (ws, sid) = (ws.to_string(), sid.to_string());
                            let principal = principal.clone();
                            tokio::task::spawn_blocking(move || {
                                crate::v1_host_sessions::handle_v1_host_kill(
                                    &principal, &ws, &sid,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| {
                                crate::cli_response::CliResponse::internal_error(e)
                            })
                        }
                        // Anything else under `/v1/w/` (wrong shape, wrong
                        // method, extra segments) → uniform 404, drain first.
                        _ => {
                            let _ = stream.read(&mut buf).await;
                            crate::cli_response::CliResponse::not_found()
                        }
                    }
                }
                _ => {
                    let _ = stream.read(&mut buf).await;
                    crate::cli_response::CliResponse::not_found()
                }
            };
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // ── Remote Session Layer 0+2 — status / enable / disable / grants /
        // revoke / shell spawn. Always-on route surface (the master switch
        // is a *behavior* gate inside handlers, not a surface-absent flag):
        // - status / enable / disable / grant / grants / revoke: owner-or-admin
        //   (grant tokens must NOT enable/disable or mint/revoke)
        // - shell/spawn: token_ok OR k2rs_ grant token pre-check, then
        //   Layer 0 gate + grant validation (Stage 2: ready:false, no PTY)
        p if p.starts_with("/cli/remote-session/") => {
            let r = match p {
                "/cli/remote-session/status" => {
                    // GET, owner-or-admin. Drain then gate (same shape as
                    // /cli/api-keys/list — do not use require_owner_or_admin
                    // after a drain; it would re-drain and double-write).
                    let _ = stream.read(&mut buf).await;
                    if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
                        crate::cli_response::CliResponse::forbidden()
                    } else {
                        tokio::task::spawn_blocking(crate::remote_session_routes::handle_status)
                            .await
                            .unwrap_or_else(|e| {
                                crate::cli_response::CliResponse::internal_error(e)
                            })
                    }
                }
                "/cli/remote-session/grants" => {
                    // GET, owner-or-admin. List never returns token/hash.
                    let _ = stream.read(&mut buf).await;
                    if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
                        crate::cli_response::CliResponse::forbidden()
                    } else {
                        tokio::task::spawn_blocking(
                            crate::remote_session_routes::handle_grants_list,
                        )
                        .await
                        .unwrap_or_else(|e| {
                            crate::cli_response::CliResponse::internal_error(e)
                        })
                    }
                }
                "/cli/remote-session/enable" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    if !super::http::require_owner_or_admin(
                        &mut *stream,
                        &mut buf,
                        &query,
                        state.token.as_str(),
                    )
                    .await
                    {
                        return DispatchOutcome::Done;
                    }
                    let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(crate::remote_session_routes::handle_enable)
                        .await
                        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/remote-session/disable" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    if !super::http::require_owner_or_admin(
                        &mut *stream,
                        &mut buf,
                        &query,
                        state.token.as_str(),
                    )
                    .await
                    {
                        return DispatchOutcome::Done;
                    }
                    let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(crate::remote_session_routes::handle_disable)
                        .await
                        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/remote-session/grant" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    if !super::http::require_owner_or_admin(
                        &mut *stream,
                        &mut buf,
                        &query,
                        state.token.as_str(),
                    )
                    .await
                    {
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    let issued_by = crate::remote_session_routes::principal_label_from_query(
                        &query,
                        state.token.as_str(),
                    );
                    tokio::task::spawn_blocking(move || {
                        crate::remote_session_routes::handle_grant_create(&body, &issued_by)
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/remote-session/revoke" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    if !super::http::require_owner_or_admin(
                        &mut *stream,
                        &mut buf,
                        &query,
                        state.token.as_str(),
                    )
                    .await
                    {
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || {
                        crate::remote_session_routes::handle_revoke(&body)
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/remote-session/shell/spawn" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    // Prefer A: if token starts with k2rs_, accept into the
                    // handler without token_ok (grant tokens are not owner/
                    // connect sessions). Otherwise require token_ok.
                    let presented = super::http::extract_token(&query)
                        .filter(|t| !t.is_empty())
                        .map(|s| s.to_string());
                    let is_grant = presented
                        .as_deref()
                        .is_some_and(k2_core::remote_sessions::is_grant_token);
                    if !is_grant && !super::http::token_ok(&query, state.token.as_str()) {
                        let _ = stream.read(&mut buf).await;
                        super::http::send_response(
                            &mut *stream,
                            "403 Forbidden",
                            "application/json",
                            r#"{"error":"invalid or missing token"}"#,
                        )
                        .await;
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    let label = match presented.as_deref() {
                        Some(tok) => crate::remote_session_routes::principal_label_from_token(
                            tok,
                            state.token.as_str(),
                        ),
                        None => "unknown".to_string(),
                    };
                    tokio::task::spawn_blocking(move || {
                        crate::remote_session_routes::handle_shell_spawn(
                            &label,
                            presented.as_deref(),
                            &body,
                        )
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                _ => {
                    let _ = stream.read(&mut buf).await;
                    crate::cli_response::CliResponse::not_found()
                }
            };
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // ── Federation V1 (prd-cross-server-agent-comms) — the ONE
        // dispatcher touch for the whole `/cli/federation/*` surface.
        //
        // DARK BY DEFAULT: with K2_FEDERATION OFF (the shipped default) every
        // path here 404s exactly as if the routes didn't exist — zero behavior
        // change. Routes: pair/request (UNAUTH → creates only Pending),
        // pair/confirm (owner SAS confirm → Trusted), inbound (envelope-
        // authenticated ingress), send / peers / peer-roster (PR1 dual-auth:
        // owner-or-admin OR scoped passport), roster (peer signed challenge).
        // Auth model is DECISION-2: inbound is authenticated by the SIGNED
        // ENVELOPE (require_peer inside the handler), never a token.
        // pair/confirm/outbox/pubkey stay owner-or-admin only.
        p if p.starts_with("/cli/federation/") => {
            if !k2_core::federation::enabled() {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "404 Not Found",
                    "application/json",
                    r#"{"error":"not found"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let r = match p {
                "/cli/federation/pair/request" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || {
                        crate::federation_routes::handle_pair_request(&body)
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/federation/pair/confirm" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    // Owner-or-admin gated: a remote owner/admin connect-user
                    // session may confirm peers; a Member session must NOT.
                    // Scoped passports are NOT admitted (R5 / PR1 non-goal).
                    if !super::http::require_owner_or_admin(
                        &mut *stream,
                        &mut buf,
                        &query,
                        state.token.as_str(),
                    )
                    .await
                    {
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || {
                        crate::federation_routes::handle_pair_confirm(&body)
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/federation/inbound" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    // NO token gate — the SIGNED ENVELOPE is the credential
                    // (verify against the pinned key + require_peer, all inside
                    // the handler). DECISION-2: peers are asymmetric-key
                    // principals, never token_ok/owner.
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || {
                        crate::federation_routes::handle_inbound(&body)
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/federation/send" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    // PR1 dual-auth: owner-or-admin OR scoped passport (agent
                    // verbs allowlist). Member connect-users stay barred.
                    // Principal is installed so handle_send forces
                    // from_workspace (Wave 0 PR-C binding).
                    let (auth_ok, scoped_principal) = owner_or_admin_or_scoped_hook_auth(
                        p,
                        &query,
                        bearer_token.as_deref(),
                        state.token.as_str(),
                    );
                    if !auth_ok {
                        let _ = stream.read(&mut buf).await;
                        super::http::send_response(
                            &mut *stream,
                            "403 Forbidden",
                            "application/json",
                            r#"{"error":"invalid or missing token"}"#,
                        )
                        .await;
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    // Blocking: seal + durable enqueue + network dial.
                    tokio::task::spawn_blocking(move || {
                        crate::caller_workspace::with_request_principal(scoped_principal, || {
                            crate::federation_routes::handle_send(&body)
                        })
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/federation/roster" => {
                    // GET, peer-authenticated by a SIGNED CHALLENGE in the query
                    // (fp/ts/sig) — NOT a token (DECISION-2). Verification +
                    // require_peer(fp,"roster") live in the handler; fail-closed.
                    let _ = stream.read(&mut buf).await;
                    let params = super::http::parse_params(&path, &query);
                    let fp = params.get("fp").cloned();
                    let ts = params.get("ts").cloned();
                    let sig = params.get("sig").cloned();
                    tokio::task::spawn_blocking(move || {
                        crate::federation_routes::handle_roster(
                            fp.as_deref(),
                            ts.as_deref(),
                            sig.as_deref(),
                        )
                    })
                    .await
                    .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/federation/pubkey" => {
                    // GET, OWNER-gated. Returns THIS daemon's federation
                    // identity (SPKI PEM + fingerprint + subdomain) so a peer
                    // can pin it during owner-driven auto-pair. Reachable
                    // whenever federation is enabled; a remote owner/admin
                    // connect-user session may read it, a Member must NOT (same
                    // owner-or-admin gate as peers).
                    let _ = stream.read(&mut buf).await;
                    if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
                        crate::cli_response::CliResponse::forbidden()
                    } else {
                        tokio::task::spawn_blocking(crate::federation_routes::handle_pubkey)
                            .await
                            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                    }
                }
                "/cli/federation/peers" => {
                    // GET, dual-auth (PR1): owner-or-admin OR scoped passport
                    // so agents can resolve peers for k2 msg <agent>@host.
                    // Member connect-users stay barred (not token_ok).
                    let _ = stream.read(&mut buf).await;
                    let (auth_ok, _) = owner_or_admin_or_scoped_hook_auth(
                        p,
                        &query,
                        bearer_token.as_deref(),
                        state.token.as_str(),
                    );
                    if !auth_ok {
                        crate::cli_response::CliResponse::forbidden()
                    } else {
                        tokio::task::spawn_blocking(crate::federation_routes::handle_peers)
                            .await
                            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                    }
                }
                "/cli/federation/outbox" => {
                    // GET, OWNER-or-ADMIN-gated. Truthful surface for the
                    // outbox drain: per-peer queued count + oldest age +
                    // dead-letters (`k2 fed outbox`). A Member session must
                    // NOT see queued cross-server traffic.
                    let _ = stream.read(&mut buf).await;
                    if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
                        crate::cli_response::CliResponse::forbidden()
                    } else {
                        tokio::task::spawn_blocking(crate::federation_routes::handle_outbox)
                            .await
                            .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                    }
                }
                "/cli/federation/peer-roster" => {
                    // GET, dual-auth (PR1): owner-or-admin OR scoped passport
                    // so agents can resolve workspace_id for federation send.
                    // Member connect-users stay barred.
                    let _ = stream.read(&mut buf).await;
                    let (auth_ok, _) = owner_or_admin_or_scoped_hook_auth(
                        p,
                        &query,
                        bearer_token.as_deref(),
                        state.token.as_str(),
                    );
                    if !auth_ok {
                        crate::cli_response::CliResponse::forbidden()
                    } else {
                        let params = super::http::parse_params(&path, &query);
                        let peer = params.get("peer").cloned().unwrap_or_default();
                        tokio::task::spawn_blocking(move || {
                            crate::federation_routes::handle_peer_roster(&peer)
                        })
                        .await
                        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                    }
                }
                _ => {
                    let _ = stream.read(&mut buf).await;
                    crate::cli_response::CliResponse::not_found()
                }
            };
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // K2 Mail S5 — the owner read: the Approvals queue. Owner verbs
        // hard-fail for agent/workspace tokens SERVER-SIDE (PRD §11.1.3
        // — kills the self-approval temptation): token_ok +
        // token_is_owner_or_admin, then the normal GET dispatch chain
        // (SQLite-only — no engine dial, so no spawn_blocking needed).
        p if p == "/cli/mail/approvals/list" => {
            let _ = stream.read(&mut buf).await;
            if !super::http::token_ok(&query, state.token.as_str()) {
                // #34: scoped agent on owner GET → owner_only, not invalid token.
                let r = mail_dual_auth_failure(p, &query, bearer_token.as_deref());
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
                let r = crate::mail_routes::owner_only_response();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body)
                    .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            let resp = crate::cli::dispatch(p, &params);
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // DNS K1 — EVERY dns GET runs in spawn_blocking (blocking
        // reqwest to the web DNS API + SQLite toggle reads). Dual auth
        // like mail: token_ok OR scoped require_hook. POSTs never reach
        // here (the is_post dns arm above matches first).
        p if p.starts_with("/cli/dns/") => {
            let _ = stream.read(&mut buf).await;
            let (auth_ok, scoped_principal) =
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str());
            if !auth_ok {
                let r = crate::cli::CliResponse::forbidden();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let mut params = super::http::parse_params(&path, &query);
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
            }
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::cli::dispatch(&p_owned, &params)
                })
            })
            .await
            .unwrap_or_else(|e| {
                crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // K2 Mail S11 / E1 — the unified inbox catalog. Dual-auth like
        // every other mail GET: owner/connect-user via token_ok OR a
        // scoped hook principal via require_hook. Principal is stamped
        // so the agent view resolves via proven workspace_uuid — never
        // raw client `project=` as who-you-are when a passport is present.
        // OWNER view (no principal, no project claim — ALL inboxes) is
        // owner-or-admin-only so agents never learn what exists outside
        // their own access. SQLite-only (no engine dial) but still runs
        // in spawn_blocking so with_request_principal is thread-local
        // for the handler chain (matches other mail dual-auth arms).
        p if p == "/cli/mail/inboxes" => {
            let _ = stream.read(&mut buf).await;
            let (auth_ok, scoped_principal) =
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str());
            if !auth_ok {
                let r = mail_dual_auth_failure(p, &query, bearer_token.as_deref());
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let mut params = super::http::parse_params(&path, &query);
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
            }
            // Owner "all inboxes" view: no scoped principal and no project
            // claim. Scoped agents always take the agent view for their
            // stamped workspace (even if stamp cleared project on an
            // unresolvable principal — handler fails closed).
            let owner_all_view = scoped_principal.is_none()
                && params
                    .get("project")
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true);
            if owner_all_view
                && !super::http::token_is_owner_or_admin(&query, state.token.as_str())
            {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"ok":false,"error":{"code":"forbidden","hint":"listing every inbox requires owner/admin — present a scoped session for the agent view, or pass project=<workspace> on an owner token, or ask your human"}}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::cli::dispatch(&p_owned, &params)
                })
            })
            .await
            .unwrap_or_else(|e| {
                crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // K2 Mail O4 — the OAuth-link long-poll status. Owner-or-admin
        // only (the paired `link/oauth/start` is owner-gated in the POST
        // arm; observing a link is the same owner surface). SQLite/in-
        // memory only — no engine dial, so it needs no spawn_blocking.
        p if p == "/cli/mail/link/oauth/status" => {
            let _ = stream.read(&mut buf).await;
            if !super::http::token_ok(&query, state.token.as_str()) {
                // #34: scoped agent on owner GET → owner_only.
                let r = mail_dual_auth_failure(p, &query, bearer_token.as_deref());
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
                let r = crate::mail_routes::owner_only_response();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body)
                    .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            let resp = crate::cli::dispatch(p, &params);
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // K2 Mail S1 (BYO OAuth client) — the owner reads their per-provider
        // OAuth client config. Owner-or-admin only (the paired
        // oauth-config/{set,clear} POSTs are owner-gated via
        // is_owner_level_mutation; reading the config is the same owner
        // surface). Reports {source, clientId, secretSet} — NEVER the secret
        // value. Reads app_settings + the vault (small fs), no engine dial.
        p if p == "/cli/mail/oauth-config" => {
            let _ = stream.read(&mut buf).await;
            if !super::http::token_ok(&query, state.token.as_str()) {
                // #34: scoped agent on owner GET → owner_only.
                let r = mail_dual_auth_failure(p, &query, bearer_token.as_deref());
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
                let r = crate::mail_routes::owner_only_response();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body)
                    .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            let resp = crate::cli::dispatch(p, &params);
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // K2 Mail — EVERY mail GET runs in spawn_blocking. The handlers
        // are written blocking-style throughout (rusqlite + BLOCKING
        // reqwest): `/cli/mail/preflight` dials the public-IP services,
        // `/cli/mail/status?health=1` pings Stalwart, and
        // `/cli/mail/wait` deliberately HOLDS the request open for up
        // to 900 s (long-poll, PRD §8.2) — none may pin an async
        // runtime worker. LIVE-BOX LESSON (k2-sandbox-01, 2026-07-10):
        // this arm originally listed only the S4 read family; a real
        // `GET /cli/mail/preflight` then dropped its blocking reqwest
        // client on the async worker and panicked tokio ("Cannot drop
        // a runtime in a context where blocking is not allowed") —
        // empty reply on the wire. Blanket-matching the family is the
        // durable fix (a future mail GET can't reintroduce it). POSTs
        // never reach here (the is_post mail arm above matches first);
        // the exact-path `/cli/mail/approvals/list` arm above also
        // stays ahead of this one. Token gate + dispatch chain
        // identical to the /cli/* catch-all below.
        //
        // E2: `address/list?all=true` enumerates EVERY hosted address
        // (Settings→Email table) — owner-or-admin only, same posture as
        // the owner inboxes view. Agent self-view (`?project=` /
        // principal) rides any workspace token.
        p if p.starts_with("/cli/mail/") =>
        {
            let _ = stream.read(&mut buf).await;
            let (auth_ok, scoped_principal) =
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str());
            if !auth_ok {
                // #34: hostmail domain/server/access/… GETs deny scoped
                // agents via is_agent_verb — teach owner_only (exit 3).
                let r = mail_dual_auth_failure(p, &query, bearer_token.as_deref());
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let mut params = super::http::parse_params(&path, &query);
            if p == "/cli/mail/address/list"
                && crate::cli::bool_param(&params, "all")
                && !super::http::token_is_owner_or_admin(&query, state.token.as_str())
            {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"ok":false,"error":{"code":"forbidden","hint":"listing every hosted address requires owner/admin — pass project=<workspace> for the agent view, or ask your human"}}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // #38: stamp_principal writes identity into `from=` (agent
            // address / workspace uuid). Mail `messages` / `wait` treat
            // `from` as an IMAP/JMAP From: filter — so a scoped agent
            // silently searched FROM "<principal>" and always got empty
            // while folder list (no from filter) still worked.
            // Capture the client filter BEFORE stamp; restore after.
            // Absent client --from → remove stamp pollution so no filter.
            let client_from_filter = params
                .get("from")
                .cloned()
                .filter(|s| !s.trim().is_empty());
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
                if p == "/cli/mail/messages" || p == "/cli/mail/wait" {
                    match client_from_filter {
                        Some(f) => {
                            params.insert("from".to_string(), f);
                        }
                        None => {
                            params.remove("from");
                        }
                    }
                }
            }
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::cli::dispatch(&p_owned, &params)
                })
            })
            .await
            .unwrap_or_else(|e| {
                crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // C2 (0.40.45): dual-auth peer-gated agent comms GETs —
        // `/cli/workspace/msg` (legacy GET form), `/cli/terminal/read`
        // (`k2 read <ws>`), and `/cli/inbox/*` reads. Owner/connect-user
        // via token_ok (no principal → peer bypass); scoped via
        // require_hook (principal stamped → peer gate).
        //
        // Stage 3 remote-session: `/cli/terminal/read` ALSO accepts a
        // `k2rs_` grant token (handler enforces bind + Layer 0). Workspace
        // msg / inbox stay token_ok / scoped only.
        p if p == "/cli/workspace/msg"
            || p == "/cli/terminal/read"
            || p.starts_with("/cli/inbox/")
            || p == "/cli/thread"
            || p == "/cli/chatter"
            || p == "/cli/chatterlog" =>
        {
            let _ = stream.read(&mut buf).await;
            let is_grant = super::http::extract_token(&query)
                .is_some_and(k2_core::remote_sessions::is_grant_token);
            let (auth_ok, scoped_principal) = if p == "/cli/terminal/read" && is_grant {
                // Grant token: enter handler; gate_remote_session_io enforces.
                (true, None)
            } else {
                token_or_scoped_hook_auth(p, &query, bearer_token.as_deref(), state.token.as_str())
            };
            if !auth_ok {
                let r = crate::cli::CliResponse::forbidden();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let mut params = super::http::parse_params(&path, &query);
            // Inbox GETs use `project=` as the resource path — preserve
            // both keys as the same target (see #36 compose stamp bug).
            let resource_target = if p.starts_with("/cli/inbox/") {
                params
                    .get("project")
                    .or_else(|| params.get("project_path"))
                    .cloned()
                    .filter(|s| !s.trim().is_empty())
            } else {
                None
            };
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
                if let Some(t) = resource_target {
                    params.insert("project".to_string(), t.clone());
                    params.insert("project_path".to_string(), t);
                }
            }
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::cli::dispatch(&p_owned, &params)
                })
            })
            .await
            .unwrap_or_else(|e| {
                crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // Stage 3: `/cli/terminal/write` accepts owner/connect (token_ok)
        // OR a `k2rs_` grant token. Handler enforces grant binding for
        // remote-session PTYs; non-remote sessions stay owner/connect-only
        // (grant token on a non-remote id → handler 403 NO_GRANT).
        p if p == "/cli/terminal/write" => {
            let _ = stream.read(&mut buf).await;
            let is_grant = super::http::extract_token(&query)
                .is_some_and(k2_core::remote_sessions::is_grant_token);
            if !is_grant && !super::http::token_ok(&query, state.token.as_str()) {
                let r = crate::cli::CliResponse::forbidden();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::cli::dispatch(&p_owned, &params)
            })
            .await
            .unwrap_or_else(|e| {
                crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // Heartbeat schedule family — dual-auth for agent-owned verbs
        // (add/list/edit/fire/…); OS tick install + fleet-wide list stay
        // owner-only and teach `owner_only` (never opaque "invalid token")
        // when a valid scoped passport hits them. Scoped callers are
        // stamped to their own workspace so they cannot schedule into
        // another project's heartbeats.
        p if p.starts_with("/cli/heartbeat/") || p == "/cli/heartbeat-log" => {
            let _ = stream.read(&mut buf).await;
            if crate::session_token::is_agent_verb(p) {
                let (auth_ok, scoped_principal) = token_or_scoped_hook_auth(
                    p,
                    &query,
                    bearer_token.as_deref(),
                    state.token.as_str(),
                );
                if !auth_ok {
                    let r = crate::cli::CliResponse::forbidden();
                    super::http::send_response(&mut *stream, r.status, r.content_type, &r.body)
                        .await;
                    return DispatchOutcome::Done;
                }
                let mut params = super::http::parse_params(&path, &query);
                if let Some(ref principal) = scoped_principal {
                    crate::caller_workspace::stamp_principal(&mut params, principal);
                }
                let p_owned = p.to_string();
                let resp = tokio::task::spawn_blocking(move || {
                    crate::caller_workspace::with_request_principal(scoped_principal, || {
                        crate::cli::dispatch(&p_owned, &params)
                    })
                })
                .await
                .unwrap_or_else(|e| {
                    crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))
                });
                super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                    .await;
            } else {
                // Owner-only heartbeat surfaces (install-launchd, list-all, …).
                if !super::http::token_ok(&query, state.token.as_str()) {
                    let r = auth_scope_failure(p, &query, bearer_token.as_deref());
                    super::http::send_response(&mut *stream, r.status, r.content_type, &r.body)
                        .await;
                    return DispatchOutcome::Done;
                }
                let params = super::http::parse_params(&path, &query);
                let resp = crate::cli::dispatch(p, &params);
                super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                    .await;
            }
        }
        // C1 (0.40.45) — `/cli/connections` dual-auth (owner/connect-user
        // OR scoped agent hook). Mutate (add/remove) is further gated:
        // owner-or-admin always; agents need agents_can_create_connections.
        // List is free for any authenticated principal. Stamps
        // actor_privileged server-side — never trust the client.
        p if p == "/cli/connections" => {
            let _ = stream.read(&mut buf).await;
            let (auth_ok, scoped_principal) = token_or_scoped_hook_auth(
                p,
                &query,
                bearer_token.as_deref(),
                state.token.as_str(),
            );
            if !auth_ok {
                let r = crate::cli::CliResponse::forbidden();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body)
                    .await;
                return DispatchOutcome::Done;
            }
            let mut params = super::http::parse_params(&path, &query);
            // Scoped agent principal is never privileged for mutate.
            let privileged = scoped_principal.is_none()
                && super::http::token_is_owner_or_admin(&query, state.token.as_str());
            params.insert(
                "actor_privileged".to_string(),
                if privileged {
                    "1".to_string()
                } else {
                    "0".to_string()
                },
            );
            if let Some(ref principal) = scoped_principal {
                crate::caller_workspace::stamp_principal(&mut params, principal);
            }
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::caller_workspace::with_request_principal(scoped_principal, || {
                    crate::cli::dispatch(&p_owned, &params)
                })
            })
            .await
            .unwrap_or_else(|e| {
                crate::cli_response::CliResponse::internal_error(format!("worker join: {e}"))
            });
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // Unified /cli/* dispatch. Auth + param validation +
        // per-route handler all live in `crate::cli::dispatch`; main.rs
        // just translates the CliResponse into bytes.
        p if p.starts_with("/cli/") => {
            let _ = stream.read(&mut buf).await;
            let params = super::http::parse_params(&path, &query);
            // Accept the owner daemon token OR a valid connect-user session
            // (token_ok) — matching every other /cli route. Owner-only routes
            // (users/*, tunnel/*) are gated with require_owner ABOVE this
            // catchall, so a connect-user session reaching here is the
            // intended "general daemon access" (read workspaces/files/git/…).
            // Was `req_token != *state.token` (owner-only), which silently
            // refused remote connect-users every data read over the tunnel —
            // so a connected client showed stale local workspaces.
            if !super::http::token_ok(&query, state.token.as_str()) {
                let r = crate::cli::CliResponse::forbidden();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let resp = crate::cli::dispatch(p, &params);
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body).await;
        }
        "/events" => {
            // Token check BEFORE the upgrade so unauthenticated clients
            // see an HTTP 403 instead of a dangling WS close.
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Hand off to tokio-tungstenite; the handshake is still
            // unread in the stream buffer. Plumb the authorizing token
            // (already validated above) + owner token through for the 5s
            // re-auth heartbeat (Part 1a). `extract_token` returns the
            // raw `?token=` value the upgrade was authorized with.
            let token = super::http::extract_token(&query)
                .unwrap_or_default()
                .to_string();
            crate::events::serve_events_connection(
                stream,
                state.event_tx.clone(),
                token,
                state.token.to_string(),
            )
            .await;
            return DispatchOutcome::Done;
        }
        _ => {
            let _ = stream.read(&mut buf).await;
            super::http::send_response(&mut *stream, "404 Not Found", "text/plain", "not found\n").await;
        }
    }

    // 0.39.7: keep-alive default. Every non-WS arm above ends by
    // calling `send_response`; if the request didn't request close,
    // loop and serve another request on the same socket. WS arms,
    // auth failures, and other error paths short-circuit with explicit
    // `return DispatchOutcome::Done` above so they never reach here.
    if client_wants_close {
        DispatchOutcome::Done
    } else {
        DispatchOutcome::KeepAlive
    }
}

// ─────────────────────────────────────────────────────────────────────
// Dispatch sub-helpers
// ─────────────────────────────────────────────────────────────────────

/// Phase 2 Unit 7c — orphan top-tier agent sweep. Inlined handler
/// (instead of a routes module) because the body is two lines of
/// JSON parse + a direct call into `k2_core::workspace::migrations`
/// (canonical post-Phase-2.5d path; was `agents::workspace`).
/// Returns `{"success":true,"archived":["<name>", ...]}`.
fn handle_archive_orphans(body: &[u8]) -> crate::cli::CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        project_path: String,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return crate::cli::CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let archived = k2_core::workspace::migrations::archive_orphan_top_tier_agents(
        &req.project_path,
    );
    crate::cli::CliResponse::ok_json(
        serde_json::json!({ "success": true, "archived": archived }).to_string(),
    )
}

/// Dispatch a Phase 2 Unit 6 POST request body to the right
/// per-domain handler. Path matching is exact — unknown paths fall
/// through to a 404 so the renderer surfaces "route not found"
/// instead of a silent success.
fn dispatch_unit6_post(path: &str, body: &[u8]) -> crate::cli::CliResponse {
    match path {
        // Filesystem
        "/cli/fs/search-tree" => crate::fs_routes::handle_search_tree(body),
        "/cli/fs/write-file" => crate::fs_routes::handle_write_file(body),
        "/cli/fs/move" => crate::fs_routes::handle_move(body),
        "/cli/fs/copy" => crate::fs_routes::handle_copy(body),
        "/cli/fs/delete" => crate::fs_routes::handle_delete(body),
        "/cli/fs/rename" => crate::fs_routes::handle_rename(body),
        "/cli/fs/create" => crate::fs_routes::handle_create(body),
        "/cli/fs/duplicate" => crate::fs_routes::handle_duplicate(body),
        "/cli/fs/open-finder" => crate::fs_routes::handle_open_finder(body),
        "/cli/fs/open-external" => crate::fs_routes::handle_open_external(body),
        // Chat history (state-mutating)
        "/cli/chat/rename" => crate::chat_routes::handle_rename(body),
        "/cli/chat/toggle-pin" => crate::chat_routes::handle_toggle_pin(body),
        "/cli/chat/archive" => crate::chat_routes::handle_archive(body),
        "/cli/chat/restore" => crate::chat_routes::handle_restore(body),
        "/cli/sandbox/reopen" => crate::sandbox_chat_routes::handle_sandbox_reopen(body),
        "/cli/chat/migrate-ide" => crate::chat_routes::handle_migrate_ide(body),
        // Themes
        "/cli/themes/create-template" => crate::themes_routes::handle_create_template(body),
        "/cli/themes/delete" => crate::themes_routes::handle_delete(body),
        // Skill layers
        "/cli/skill-layers/create" => crate::skill_layers_routes::handle_create(body),
        "/cli/skill-layers/delete" => crate::skill_layers_routes::handle_delete(body),
        // Review checklist
        "/cli/review-checklist/write" => crate::review_checklist_routes::handle_write(body),
        "/cli/review-checklist/toggle" => crate::review_checklist_routes::handle_toggle(body),
        "/cli/review-checklist/init" => crate::review_checklist_routes::handle_init(body),
        _ => crate::cli::CliResponse::not_found(),
    }
}

/// Dispatch a K2 Connect host-awareness GAP POST route to its handler.
///
/// These wrap the same `k2_core` fns the renderer used to call via
/// LOCAL Tauri `invoke()` — exposed over HTTP so the write targets
/// whichever daemon the renderer is talking to (local OR remote host).
/// Method gate is upstream (the `is_post && post_allowed` arm guard);
/// token gate is upstream too. Unknown paths 404.
///
/// `actor_is_privileged` is the dispatcher-resolved owner-or-admin bit
/// (C1 — gates relations create/delete the same way as connections
/// add/remove). Other connect-gap routes ignore it.
fn dispatch_connect_gap_post(
    path: &str,
    body: &[u8],
    actor_is_privileged: bool,
) -> crate::cli::CliResponse {
    match path {
        // Workspace skill CRUD + canonical opt-in + harness-fanout marker.
        "/cli/skills/create" => crate::skills_routes::handle_create(body),
        "/cli/skills/remove" => crate::skills_routes::handle_remove(body),
        "/cli/skills/write-opt-in" => crate::skills_routes::handle_write_opt_in(body),
        "/cli/onboarding/set-harness-fanout-enabled" => {
            crate::skills_routes::handle_set_harness_fanout_enabled(body)
        }
        // Host-aware READ mirrors (the GAP fix): the checkbox WRITE above is
        // host-aware, but the renderer used to READ these via LOCAL Tauri
        // invoke() — so on a remote host the marker was written remotely but
        // read locally → always false. These read routes close that gap.
        "/cli/onboarding/harness-fanout-enabled" => {
            crate::skills_routes::handle_harness_fanout_enabled(body)
        }
        "/cli/onboarding/set-agents-md-generate-enabled" => {
            crate::skills_routes::handle_set_agents_md_generate_enabled(body)
        }
        "/cli/onboarding/agents-md-generate-enabled" => {
            crate::skills_routes::handle_agents_md_generate_enabled(body)
        }
        "/cli/canonical/detect-state" => {
            crate::skills_routes::handle_detect_canonical_state(body)
        }
        // Agent / session writes.
        "/cli/agents/regenerate-workspace-skill" => {
            crate::agents_routes::handle_regenerate_workspace_skill(body)
        }
        "/cli/agents/save-agent-md" => crate::agents_routes::handle_save_agent_md(body),
        "/cli/agents/disable-workspace-claude-md" => {
            crate::agents_routes::handle_disable_workspace_claude_md(body)
        }
        "/cli/agents/run-workspace-ingest" => {
            crate::agents_routes::handle_run_workspace_ingest(body)
        }
        "/cli/agents/save-session-id" => crate::agents_routes::handle_save_session_id(body),
        "/cli/session/set-surfaced" => {
            crate::agents_routes::handle_session_set_surfaced(body)
        }
        // Workspace heartbeat-sessions visibility flag.
        "/cli/heartbeat/set-show-sessions" => {
            crate::heartbeat_routes::handle_set_show_heartbeat_sessions(body)
        }
        // Workspace relations (id-based — mirrors the renderer's
        // workspace_relations_* Tauri commands 1:1). C1: gated for
        // non-privileged actors by agents_can_create_connections.
        "/cli/relations/create" => {
            crate::agents_routes::handle_relations_create(body, actor_is_privileged)
        }
        "/cli/relations/delete" => {
            crate::agents_routes::handle_relations_delete(body, actor_is_privileged)
        }
        _ => crate::cli::CliResponse::not_found(),
    }
}

// ─────────────────────────────────────────────────────────────────────

/// Wave 0 / DNS K1 — dual auth for mail + dns families: owner/connect-user
/// via `token_ok`, OR a scoped hook principal via `require_hook` (path must
/// be on the agent-verb allowlist).
fn token_or_scoped_hook_auth(
    path: &str,
    query: &str,
    bearer: Option<&str>,
    owner_token: &str,
) -> (bool, Option<crate::session_token::HookPrincipal>) {
    if super::http::token_ok(query, owner_token) {
        return (true, None);
    }
    scoped_hook_auth(path, query, bearer)
}

/// PR1 federation dual-auth: **owner-or-admin** OR a scoped passport on an
/// agent-verb path. Stricter than [`token_or_scoped_hook_auth`] — Member
/// connect-user sessions do NOT pass (pair/peers/send historically barred
/// Members; passport path is the agent restoration, not a Member widen).
fn owner_or_admin_or_scoped_hook_auth(
    path: &str,
    query: &str,
    bearer: Option<&str>,
    owner_token: &str,
) -> (bool, Option<crate::session_token::HookPrincipal>) {
    if super::http::token_is_owner_or_admin(query, owner_token) {
        return (true, None);
    }
    scoped_hook_auth(path, query, bearer)
}

/// Shared scoped-passport arm of dual-auth helpers: Bearer header or
/// `?token=` must pass [`session_token::require_hook`] for `path`.
fn scoped_hook_auth(
    path: &str,
    query: &str,
    bearer: Option<&str>,
) -> (bool, Option<crate::session_token::HookPrincipal>) {
    let presented = bearer
        .filter(|s| !s.is_empty())
        .or_else(|| super::http::extract_token(query))
        .unwrap_or("");
    if presented.is_empty() {
        return (false, None);
    }
    match crate::session_token::require_hook(presented, path) {
        Some(v) => (true, Some(v.principal)),
        None => (false, None),
    }
}

/// Presented bearer for dual-auth failure classification (query token or
/// Authorization header). Empty when neither is present.
fn presented_bearer<'a>(query: &'a str, bearer: Option<&'a str>) -> &'a str {
    bearer
        .filter(|s| !s.is_empty())
        .or_else(|| super::http::extract_token(query))
        .unwrap_or("")
}

/// #34: when dual-auth fails on a mail route, distinguish a **valid
/// scoped agent passport on an owner surface** (teaching `owner_only`)
/// from a missing/bogus credential (`Invalid or missing auth token`).
fn mail_dual_auth_failure(
    path: &str,
    query: &str,
    bearer: Option<&str>,
) -> crate::cli_response::CliResponse {
    let presented = presented_bearer(query, bearer);
    if !presented.is_empty()
        && crate::session_token::validate_hook(presented).is_some()
        && crate::mail_routes::is_mail_owner_surface(path)
    {
        return crate::mail_routes::owner_only_response();
    }
    crate::cli_response::CliResponse::forbidden()
}

/// When auth fails on a dual-auth family (heartbeat, …): if the caller
/// presented a **valid scoped agent passport** on an owner-only path,
/// teach `owner_only` (exit 3) instead of the opaque
/// "Invalid or missing auth token" that made agents think their
/// passport was broken. Missing/garbage credentials still get classic
/// forbidden.
fn auth_scope_failure(
    path: &str,
    query: &str,
    bearer: Option<&str>,
) -> crate::cli_response::CliResponse {
    let presented = presented_bearer(query, bearer);
    if !presented.is_empty() && crate::session_token::validate_hook(presented).is_some() {
        // Valid passport, wrong surface — never pretend the token is invalid.
        if path.starts_with("/cli/heartbeat/") || path == "/cli/heartbeat-log" {
            return crate::cli_response::CliResponse {
                status: "403 Forbidden",
                content_type: "application/json",
                body: serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "owner_only",
                        "hint": "requires owner/admin — ask your human (OS schedule install, fleet-wide heartbeat list, and set-show-sessions are owner surfaces; use k2 heartbeat schedule/list/fire for workspace schedules)",
                    },
                })
                .to_string(),
            };
        }
        return crate::cli_response::CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "error": {
                    "code": "owner_only",
                    "hint": "requires owner/admin — ask your human",
                },
            })
            .to_string(),
        };
    }
    crate::cli_response::CliResponse::forbidden()
}

// Inline unit tests — dispatch sub-helpers
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_scope_failure_missing_token_is_classic_forbidden() {
        let r = auth_scope_failure("/cli/heartbeat/install-launchd", "", None);
        assert_eq!(r.status, "403 Forbidden");
        assert!(
            r.body.contains("Invalid or missing auth token"),
            "missing credential must stay classic forbidden: {}",
            r.body
        );
        assert!(!r.body.contains("owner_only"));
    }

    #[test]
    fn auth_scope_failure_garbage_token_is_classic_forbidden() {
        let r = auth_scope_failure(
            "/cli/heartbeat/list-all",
            "token=not-a-real-passport",
            None,
        );
        assert_eq!(r.status, "403 Forbidden");
        assert!(
            r.body.contains("Invalid or missing auth token"),
            "garbage credential must stay classic forbidden: {}",
            r.body
        );
    }

    #[test]
    fn dispatch_unit6_post_unknown_path_returns_404() {
        let resp = dispatch_unit6_post("/cli/does-not-exist", b"{}");
        assert_eq!(resp.status, "404 Not Found");
        assert!(
            resp.body.contains("route not found"),
            "404 body should mention 'route not found': {}",
            resp.body,
        );
    }

    #[test]
    fn dispatch_unit6_post_empty_path_returns_404() {
        // A blank path should never match a real route.
        let resp = dispatch_unit6_post("", b"{}");
        assert_eq!(resp.status, "404 Not Found");
    }

    #[test]
    fn dispatch_unit6_post_path_is_case_sensitive() {
        // Exact match required — upper/lower-case variants must NOT
        // route to the lowercase handler. Closing this avoids subtle
        // routing collisions if a future handler uses mixed case.
        let resp = dispatch_unit6_post("/CLI/FS/CREATE", b"{}");
        assert_eq!(resp.status, "404 Not Found");
    }
}
