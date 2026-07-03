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
//! starts_with arms (`/cli/git/`, `/cli/states/`, `/cli/workspaces/`,
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
            // K2SO #620 — owner-only password-policy write. GET (read) goes
            // through the GET arm below; POST is method-gated per-handler.
            | "/cli/users/policy"
            | "/cli/auth/login"
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
            // Phase 2 Unit 7c — heartbeat-launchd installer + orphan-
            // agents sweep. Body-bearing writes; method-gated below.
            | "/cli/heartbeat/install-launchd"
            | "/cli/heartbeat/uninstall-launchd"
            | "/cli/heartbeat/apply-wake-scheduler"
            | "/cli/agents/archive-orphans"
            // Phase 2 Unit 4 — DB-writing routes (states / workspaces /
            // focus-groups / sections / workspace-layouts / timer /
            // presets / window-state / projects / git). JSON-bodied
            // writes — implicit method gate via the `starts_with`
            // dispatch arm in handle_connection that runs Unit 4's
            // POST dispatch. Listed explicitly here so the top-level
            // 405 guard never short-circuits them.
            | "/cli/states/create" | "/cli/states/update" | "/cli/states/delete"
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
            | "/cli/inbox/compose"
            | "/cli/inbox/move"
            | "/cli/inbox/archive"
            | "/cli/inbox/delete"
            | "/cli/inbox/respond"
            | "/cli/inbox/migrate"
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
            // P3a (sandbox / K2-as-a-server) — API-key auth-tier MANAGEMENT
            // (owner-only, always-on; the owner pre-mints keys before flipping
            // the external /v1/* surface live). POST so the minted raw key
            // (create) rides the JSON body/response, never a URL-logged query.
            // Method- + owner-gated per-handler below. `list` is a GET.
            | "/cli/api-keys/create"
            | "/cli/api-keys/revoke"
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
            // `pair/confirm`/`send` are owner-gated; `inbound` is authenticated
            // by the signed envelope itself (require_peer), NOT a token. Method-
            // gated per-handler below (require_post). The `roster` read is a GET.
            | "/cli/federation/pair/request"
            | "/cli/federation/pair/confirm"
            | "/cli/federation/inbound"
            | "/cli/federation/send"
            // P3b (sandbox / K2-as-a-server) — the EXTERNAL public spawn route.
            // POST-only; gated below by the /v1/* surface flag (K2_SANDBOX_API)
            // + v1_principal + per-handler require_post. Listed here so the
            // top-level 405 guard never short-circuits it (without this entry
            // POST /v1/sandboxes 405s before ever reaching the /v1/ arm).
            | "/v1/sandboxes"
    )
        // Sandbox v2 (PRD §A) — the workspace-scoped session routes carry
        // dynamic `<workspace>` / `<session-id>` segments, so they cannot be
        // exact-listed above. POST is valid on `/v1/w/<ws>/sessions` (new /
        // fork) and `/v1/w/<ws>/sessions/<id>` (address); allow the whole
        // `/v1/w/` prefix here so the top-level 405 guard never short-circuits
        // them before the `/v1/` arm runs (that arm + the per-route `is_post`
        // branch below do the real method gating). The surface stays DARK
        // unless K2_SANDBOX_API is on (checked inside the `/v1/` arm).
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
    // without extending the immutable borrow.
    let path = path.to_string();
    let query = query.to_string();
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
        && !matches!(path.as_str(), "/ping" | "/health" | "/boot-status")
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
                // 0.39.35: update SHAPE selector. "bundled-app" hosts update
                // via the co-located Tauri app (Shape A); "standalone" hosts
                // via the in-daemon binary swap (Shape B). The renderer reads
                // this to vary copy; update/start routes on it server-side.
                "installKind": crate::boot_status::install_kind(),
                // COMPAT-58 (#58 Phase 0): advertise the scoped-hook
                // capability so clients can FEATURE-DETECT it without an app
                // version bump. `supported` = this daemon understands the
                // scoped per-cell token superset (always true once Phase 0
                // lands); `enabled` = whether K2_HOOK_SCOPED is on for this
                // process (default OFF). A daemon talking to an OLDER fleet
                // peer that omits this field treats it as unsupported and
                // stays on the owner-token/TCP path. PROTOCOL is intentionally
                // NOT bumped: this is an additive, forward-compatible field,
                // not a breaking contract change (boot_status::PROTOCOL gates
                // only the latter).
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
            let body = format!(
                r#"{{"version":"{}","uptime_secs":{},"pid":{},"port":{}}}"#,
                env!("CARGO_PKG_VERSION"),
                uptime_secs,
                pid,
                state.port,
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

            // #58 Phase 0 SCOPED arm — DORMANT unless K2_HOOK_SCOPED is on.
            // A per-session scoped token authorizes ONLY its own paneId:
            // the Bearer header is preferred (kept out of logs/transcripts),
            // falling back to the `?token=` value. With the flag OFF nothing
            // ever mints a scoped token, so `require_hook` never matches →
            // this arm is inert and the gate is byte-identical to the
            // owner-only check above.
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
                    // #675.5 — connector stopped; push the cleared status.
                    let _ = crate::session_events::emit(
                        crate::session_events::SessionEvent::TunnelStatusChanged {
                            running: false,
                            public_url: None,
                        },
                    );
                    crate::cli::CliResponse::ok_json(r#"{"ok":true}"#.to_string())
                }
                Err(e) => crate::cli::CliResponse::bad_request(e),
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
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
            // Auth split (K2SO #617): POST MUTATES the host's tunnel
            // binding (token + subdomain) → OWNER-ONLY. GET returns a
            // redacted, read-only view → authorized (a connect-user may
            // read it). So: POST gates on `token_is_owner`, GET on the
            // extended `token_ok`. A connect-user POSTing here gets 403.
            let authorized = if is_post {
                super::http::token_is_owner(&query, state.token.as_str())
            } else {
                super::http::token_ok(&query, state.token.as_str())
            };
            if !authorized {
                if is_post {
                    let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                }
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let resp = if is_post {
                let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
                match serde_json::from_slice::<k2_core::tunnel::TunnelConfigUpdate>(&body_bytes) {
                    Ok(upd) => match k2_core::tunnel::set_config(upd) {
                        Ok(view) => crate::cli::CliResponse::ok_json(
                            serde_json::to_string(&view)
                                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                        ),
                        Err(e) => crate::cli::CliResponse::bad_request(e),
                    },
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
            // OWNER-ONLY for now (#629 keeps password resets at owner level).
            if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
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
        "/cli/auth/login" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
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
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
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
        // caller's own session token in `?token=`; the OWNER token has no
        // session so it's a harmless idempotent no-op. POST-gated (mutating
        // /cli/* route → the `if !is_post { 405 }` guard, per the contract).
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
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
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
        // Remote-access keys (federationEnabled / allowRemoteInstruct)
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
        // POST /cli/workspace/set — 0.40.24 S2 (agent CLI settings plane).
        // Multi-field per-workspace settings write. Body (JSON):
        // `{"project": "<name|path|uuid>", "fields": {"agent_mode": "k2", ...}}`.
        // token_ok (owner or connect-user session, same tier as the other
        // workspace-scoped writes) + require_post per the
        // feedback_post_only_route_guards house rule.
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
            && (p.starts_with("/cli/skills/")
                || p == "/cli/onboarding/set-harness-fanout-enabled"
                || p == "/cli/onboarding/harness-fanout-enabled"
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
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                dispatch_connect_gap_post(&p_owned, &body_bytes)
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
        // Phase 2 Unit 4 — POST routes for DB-writing domains. JSON-
        // bodied writes; per-route allowlist + same implicit gate
        // pattern as Unit 6. Dispatch is `dispatch_unit4_post`.
        p if is_post && post_allowed && (
            p.starts_with("/cli/states/")
                || p.starts_with("/cli/workspaces/")
                || p.starts_with("/cli/focus-groups/")
                || p.starts_with("/cli/sections/")
                || p.starts_with("/cli/workspace-layouts/")
                || p.starts_with("/cli/timer/")
                || p.starts_with("/cli/presets/")
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
        p if is_post && post_allowed && p.starts_with("/cli/inbox/") => {
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
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::inbox_routes::dispatch_post(&p_owned, &params)
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
        // 0.39.45 (#35/#37/#29) — live-msg POST form. Same handler as
        // the GET form (crate::cli::dispatch → workspace_routes), but
        // the message `text` (and any other param) may arrive in the
        // form-encoded POST body, dodging the request-head cap that
        // silently clipped long live messages. Body wins on collision.
        // Runs in spawn_blocking: deliver_live sleeps across its
        // inject/verify/retry windows and must not pin a runtime worker.
        p if is_post && p == "/cli/workspace/msg" => {
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
            let p_owned = p.to_string();
            let resp = tokio::task::spawn_blocking(move || {
                crate::cli::dispatch(&p_owned, &params)
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
        // ── P3a (sandbox / K2-as-a-server) — API-key auth-tier MANAGEMENT.
        //
        // OWNER-ONLY (require_owner / token_is_owner) + ALWAYS-ON: minting,
        // listing, and revoking keys is the owner's job, so a connect-user
        // session token is rejected and an API key CANNOT manage keys (it never
        // reaches here — these gate on the owner token, never v1_principal).
        // Always-on so the owner can pre-create keys before flipping the
        // external /v1/* surface live (harmless while /v1/* is dark). The two
        // POSTs are method-gated per-handler (require_post); `list` is a GET.
        // The minted RAW key is returned ONCE by create + never logged.
        p if p.starts_with("/cli/api-keys/") => {
            let r = match p {
                "/cli/api-keys/create" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || crate::misc_routes::handle_api_key_create(&body))
                        .await
                        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/api-keys/revoke" => {
                    if !super::http::require_post(&mut *stream, &mut buf, is_post).await {
                        return DispatchOutcome::Done;
                    }
                    if !super::http::require_owner(&mut *stream, &mut buf, &query, state.token.as_str()).await {
                        return DispatchOutcome::Done;
                    }
                    let body = super::http::read_post_body(&mut *stream, &mut buf).await;
                    tokio::task::spawn_blocking(move || crate::misc_routes::handle_api_key_revoke(&body))
                        .await
                        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(e))
                }
                "/cli/api-keys/list" => {
                    // GET, OWNER-gated. Drain the peeked head then check owner.
                    let _ = stream.read(&mut buf).await;
                    if !super::http::token_is_owner(&query, state.token.as_str()) {
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
        // ── P3a (sandbox / K2-as-a-server) — the EXTERNAL `/v1/*` surface.
        //
        // DARK BY DEFAULT: with K2_SANDBOX_API OFF (the shipped default) EVERY
        // `/v1/*` path 404s exactly as if the routes didn't exist — the whole
        // external surface is absent and flag-off is byte-identical to no
        // surface. When ON, each route gates on `v1_principal` (owner token OR a
        // valid non-revoked API key, Bearer-preferred). `/v1/ping` is the
        // minimal P3a test route (P3b adds the real POST /v1/sandboxes spawn).
        p if p.starts_with("/v1/") => {
            if !crate::misc_routes::sandbox_api_enabled() {
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
                // ── Sandbox v2 (PRD §A) — WORKSPACE-SCOPED session front door:
                //   POST /v1/w/<ws>/sessions                    → new (or fork
                //        when body carries `fork_from`) session in <ws>
                //   POST /v1/w/<ws>/sessions/<id>               → address an
                //        existing sandbox session (message/resume intent)
                //   GET  /v1/w/<ws>/sessions/<id>/messages?since=<n> → drain
                //   GET  /v1/w/<ws>/sessions                    → list <ws>'s
                //        sandbox sessions (audit; empty in slice 1)
                //
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
        // ── Federation V1 (prd-cross-server-agent-comms) — the ONE
        // dispatcher touch for the whole `/cli/federation/*` surface.
        //
        // DARK BY DEFAULT: with K2_FEDERATION OFF (the shipped default) every
        // path here 404s exactly as if the routes didn't exist — zero behavior
        // change. Routes: pair/request (UNAUTH → creates only Pending),
        // pair/confirm (owner SAS confirm → Trusted), inbound (envelope-
        // authenticated ingress → deliver to INBOX ONLY), send (owner-gated
        // outbound seal+dial), roster (GET stub). Each mutating route starts
        // with `if !is_post { 405 }` via require_post (the top-level dispatch
        // lets a GET through on POST-allowlisted routes; see
        // feedback_post_only_route_guards). Auth model is DECISION-2: inbound is
        // authenticated by the SIGNED ENVELOPE (require_peer inside the
        // handler), never a token; confirm/send take the owner token.
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
                    // Local actor initiates outbound → owner-or-admin required
                    // (a remote owner/admin connect-user may send; a Member may
                    // not).
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
                    // Blocking: seal + durable enqueue + network dial.
                    tokio::task::spawn_blocking(move || {
                        crate::federation_routes::handle_send(&body)
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
                    // GET, OWNER-or-ADMIN-gated (local convenience for the
                    // renderer's cross-server picker). A Member connect-user
                    // session must NOT see the pinned-peer list.
                    let _ = stream.read(&mut buf).await;
                    if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
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
                    // GET, OWNER-gated. The LOCAL daemon dials a PAIRED peer's
                    // signed roster GET and returns its agent projection so the
                    // renderer can populate the dropdown. Blocking (network).
                    // OWNER-or-ADMIN-gated; a Member session must NOT.
                    let _ = stream.read(&mut buf).await;
                    if !super::http::token_is_owner_or_admin(&query, state.token.as_str()) {
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
fn dispatch_connect_gap_post(path: &str, body: &[u8]) -> crate::cli::CliResponse {
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
        // workspace_relations_* Tauri commands 1:1).
        "/cli/relations/create" => crate::agents_routes::handle_relations_create(body),
        "/cli/relations/delete" => crate::agents_routes::handle_relations_delete(body),
        _ => crate::cli::CliResponse::not_found(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Inline unit tests — dispatch sub-helpers
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
