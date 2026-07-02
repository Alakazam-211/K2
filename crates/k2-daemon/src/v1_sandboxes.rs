//! `POST /v1/sandboxes` — the PUBLIC sandbox-spawn route (P3b).
//!
//! The external "K2-as-a-server" front door: an authenticated [`V1Principal`]
//! (owner token or a valid API key — resolved by `routes::http::v1_principal`)
//! asks for a fresh isolated agent session. The whole `/v1/*` surface sits
//! behind `misc_routes::sandbox_api_enabled()` (`K2_SANDBOX_API`, default OFF),
//! so this route is DARK / 404 unless explicitly enabled.
//!
//! Flow:
//!   1. **REFUSE if we can't sandbox** — INVERT v2/spawn's degrade-to-passthrough:
//!      a public sandbox API must NEVER silently run unsandboxed. If
//!      `v2_spawn::can_sandbox()` is false (every macOS / feature-off build) →
//!      `409 Conflict`. The real path is on-box (Linux + `sandbox-microvm` +
//!      `K2_SANDBOX`).
//!   2. **Policy-resolve** the untrusted body into a host-trusted `SpawnRequest`
//!      ([`policy::resolve_spawn`] — the AUTHZ DOOR; ephemeral cwd, host-minted
//!      name, dropped caller env/args, principal-staged key).
//!   3. **Spawn** via the PROVEN `v2_spawn::spawn_session` internals.
//!   4. **Mint a per-session stream token** (`stream_token`) and return the
//!      grid/bytes stream URLs scoped to THIS session — the API key is never
//!      broadened to the raw PTY streams.

pub mod policy;

use crate::cli_response::CliResponse;
use crate::routes::http::V1Principal;
use crate::{sandbox_quota, stream_token, v2_spawn};

use policy::{resolve_spawn, resolve_workspace_session, ApiSandboxRequest};

use k2_core::session::SessionId;

/// Handle `POST /v1/sandboxes`. The caller is ALREADY authenticated to a
/// [`V1Principal`] by the dispatcher's `/v1/*` gate; this never re-checks creds,
/// it resolves them into a host-trusted spawn.
pub fn handle_v1_sandboxes(principal: &V1Principal, body: &[u8]) -> CliResponse {
    // (1) REFUSE if this daemon cannot deliver a real microVM. A public sandbox
    // API must NEVER degrade to an unsandboxed passthrough cell — the inverse of
    // v2/spawn's degrade-loud behavior. On Mac / feature-off this ALWAYS 409s.
    if !v2_spawn::can_sandbox() {
        return CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: r#"{"error":"this daemon cannot sandbox (microVM backend unavailable)"}"#
                .to_string(),
        };
    }

    // (2) Parse the UNTRUSTED body. Empty/whitespace body → all defaults.
    let api_req: ApiSandboxRequest = if body.iter().all(|b| b.is_ascii_whitespace()) {
        ApiSandboxRequest::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(r) => r,
            Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
        }
    };

    // (3) P4-H4 — CONCURRENT-CELL CAP. ACQUIRE a quota slot BEFORE we provision
    // anything (no ephemeral dir, no spawn) so a rejected request leaves ZERO
    // side effects. Caps are resolved per-principal (owner = high own-use cap,
    // api-key = standard cap) + the daemon-global ceiling — all env-overridable.
    // At the limit → 429 with a machine-readable `code`. The matching RELEASE is
    // the child-exit observer (authoritative; fires on clean exit AND crash) on
    // the success path, or an explicit `release` on the early-failure paths
    // below. This whole route is behind `K2_SANDBOX_API` (default OFF), so
    // normal v2/spawn + cockpit sessions never reach this counter.
    let principal_key = principal.display_id();
    if let Err(qe) = sandbox_quota::try_acquire(&principal_key) {
        return CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: serde_json::json!({ "error": qe.message(), "code": qe.code() }).to_string(),
        };
    }

    // (4) Run the POLICY-RESOLVER → host-trusted SpawnRequest. Fail-CLOSED: a
    // provisioning failure 5xxs (NEVER a $HOME fallback). On failure we RELEASE
    // the slot we just acquired (no session ⇒ no observer ⇒ no auto-release).
    let mut spawn_req = match resolve_spawn(principal, &api_req) {
        Ok(r) => r,
        Err(e) => {
            sandbox_quota::release(&principal_key);
            return CliResponse {
                status: e.status(),
                content_type: "application/json",
                body: serde_json::json!({ "error": e.message() }).to_string(),
            }
        }
    };
    // Stamp the principal key so the child-exit observer RELEASES this session's
    // quota slot on teardown (the single point that fires on clean exit AND on
    // crash/OOM/kill-9 — the counter can't leak once a session exists).
    spawn_req.principal_key = Some(principal_key.clone());
    // Keep a handle to the ephemeral dir so we can clean up if the spawn itself
    // fails (no session ⇒ no child-exit observer ⇒ no automatic teardown).
    let ephemeral_cwd = spawn_req.ephemeral_cwd.clone();

    // (5) Spawn through the PROVEN v2 internals (inherits B3a key-inject, scoped
    // token/UDS, child-exit teardown incl. the ephemeral-dir rm + quota release).
    let result = v2_spawn::spawn_session(spawn_req);
    if result.status != "200 OK" {
        // No session ⇒ no observer ⇒ release the slot we acquired ourselves.
        sandbox_quota::release(&principal_key);
        cleanup_ephemeral(&ephemeral_cwd);
        // Surface the spawn's own status/body (already a JSON error envelope).
        return CliResponse {
            status: result.status,
            content_type: "application/json",
            body: result.body,
        };
    }

    // (6) Parse the spawn response for the new session id + agent name. NOTE:
    // these are can't-happen internal errors AFTER a 200 spawn — a live session
    // (and its child-exit observer) now exists, so the observer OWNS the quota
    // release; we must NOT release here (that would double-decrement).
    let v: serde_json::Value = match serde_json::from_str(&result.body) {
        Ok(v) => v,
        Err(_) => {
            cleanup_ephemeral(&ephemeral_cwd);
            return CliResponse::internal_error("spawn response was not JSON");
        }
    };
    let session_id = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("");
    let agent_name = v.get("agentName").and_then(|x| x.as_str()).unwrap_or("");
    if session_id.is_empty() {
        cleanup_ephemeral(&ephemeral_cwd);
        return CliResponse::internal_error("spawn response carried no sessionId");
    }
    let Some(sid) = k2_core::session::SessionId::parse(session_id) else {
        cleanup_ephemeral(&ephemeral_cwd);
        return CliResponse::internal_error("spawn response sessionId was malformed");
    };

    // F2: record THIS session's owning principal so a later
    // `GET /v1/sandboxes/<id>/messages` can authorize (default-deny: a requester
    // whose principal id != this owner is rejected by `handle_messages`). The id
    // is the stable, non-secret `display_id()` ("owner" or the API-key UUID).
    crate::sandbox_responses::record_owner(session_id, &principal_key);

    // (7) Mint a per-session STREAM token (bound to THIS session only) and build
    // the GRID stream URL. The caller streams with this token — NEVER the API
    // key, which is never accepted on grid/bytes.
    //
    // GRID ONLY (P3 follow-up): a v2/sandbox session is a `DaemonPtySession`,
    // which is single-subscriber-by-design with NO raw-byte broadcast — only the
    // structured grid (Snapshot/Delta) stream via `grid_emitter`. The
    // `/cli/sessions/bytes` WS looks sessions up in the LEGACY
    // `k2_core::session::registry` (not `v2_session_map`), so it 400s for a v2
    // session even after the stream-token authorizes. So we DON'T advertise a
    // bytes URL that can't work — grid is the canonical v2 stream. Raw-PTY bytes
    // for v2 sessions would need a new byte-broadcast on DaemonPtySession (a
    // real feature) — deferred.
    let stream_tok = stream_token::mint(&sid);
    let grid = format!("/cli/sessions/grid?session={session_id}&token={stream_tok}");

    CliResponse::ok_json(
        serde_json::json!({
            "sessionId": session_id,
            "agentName": agent_name,
            // can_sandbox() was true above ⇒ resolve_sandbox(true) is Microvm.
            "sandbox": "microvm",
            "stream": { "grid": grid },
        })
        .to_string(),
    )
}

/// Handle `GET /v1/sandboxes/<id>/messages?since=<seq>` (F2) — drain the in-cell
/// agent's response log for ONE session.
///
/// AUTHZ (default-deny, the load-bearing check): the requesting [`V1Principal`]
/// MUST own `session_id`. Ownership is the `principal.display_id()` recorded at
/// create time in [`crate::sandbox_responses::record_owner`]. We compare the
/// requester's `display_id()` against the recorded owner and return **404** (not
/// 403) on any mismatch OR unknown session — a uniform "no such sandbox" so the
/// route never even confirms the existence of another principal's session, let
/// alone leaks its transcript. Owner identity comes from the host-resolved
/// principal (the dispatcher's `/v1/*` auth gate), NEVER the request.
///
/// On success: `{"messages":[...],"latest_seq":<n>}` where `latest_seq` is the
/// highest `seq` the caller now holds (its next-poll cursor) — the max of the
/// returned slice, falling back to the requested `since` when the slice is empty.
pub fn handle_messages(principal: &V1Principal, session_id: &str, since: u64) -> CliResponse {
    // AUTHZ: the session must exist AND be owned by THIS principal. Unknown owner
    // and owner-mismatch are BOTH 404 (no existence oracle across principals).
    let requester = principal.display_id();
    match crate::sandbox_responses::owner_of(session_id) {
        Some(owner) if owner == requester => {}
        _ => {
            return CliResponse {
                status: "404 Not Found",
                content_type: "application/json",
                body: r#"{"error":"no such sandbox"}"#.to_string(),
            };
        }
    }

    let messages = crate::sandbox_responses::since(session_id, since);
    // The caller's new cursor: the highest seq it now holds. Empty slice ⇒ it
    // already had everything up to `since`, so the cursor stays at `since`.
    let latest_seq = messages.iter().map(|m| m.seq).max().unwrap_or(since);
    CliResponse::ok_json(
        serde_json::json!({
            "messages": messages,
            "latest_seq": latest_seq,
        })
        .to_string(),
    )
}

/// Best-effort removal of a half-provisioned ephemeral dir on an early-exit path
/// (spawn failure / malformed response) where no child-exit observer will fire.
fn cleanup_ephemeral(dir: &Option<std::path::PathBuf>) {
    if let Some(d) = dir.as_ref() {
        let _ = std::fs::remove_dir_all(d);
    }
}

// ─────────────────────────────────────────────────────────────────────
// Sandbox v2 (PRD §A / §G2) — WORKSPACE-SCOPED session front door.
//
// SLICE 1 = the front door ONLY: route + slug resolution + per-key workspace
// authz + the canonical-off-limits guard. The persistent per-session overlay
// mount (PRD §B/§C) is slices 2-3 — for now, after resolving+authorizing the
// workspace, we thread the resolved (path, session-id, op) into
// [`WorkspaceSessionRequest`] and delegate to the EXISTING ephemeral spawn
// ([`handle_v1_sandboxes`], provisioning UNCHANGED). See the TODO in
// [`spawn_for_workspace_slice1`].
// ─────────────────────────────────────────────────────────────────────

/// The caller INTENT for a workspace-scoped session (PRD §G2 #3). Never the
/// canonical session — the API only ever operates on api-spawned sandbox
/// sessions (enforced by the canonical-off-limits guard at the door).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsSessionOp {
    /// `POST /v1/w/<ws>/sessions` with no `fork_from` → a fresh session.
    New,
    /// `POST /v1/w/<ws>/sessions` with `fork_from=<id>` → branch a NEW session
    /// from an existing sandbox session's state (PRD §G2 #3c). `from` is a
    /// validated, non-canonical sandbox session id.
    Fork { from: String },
    /// `POST /v1/w/<ws>/sessions/<id>` → address an existing sandbox session
    /// (the F3 liveness router decides deliver-live vs resume; slices 2-3).
    Address { session_id: String },
}

/// A HOST-TRUSTED workspace-scoped session request. Every field is host-resolved
/// (the workspace PATH comes from the projects registry via an AUTHORIZED slug,
/// never the wire), so slices 2-3 can consume it to mount the RO canonical
/// workspace lower + the persistent per-session upper. In SLICE 1 it is built,
/// logged, and then the spawn still runs ephemerally (see
/// [`spawn_for_workspace_slice1`]).
#[derive(Debug, Clone)]
pub struct WorkspaceSessionRequest {
    /// The AUTHORIZED workspace slug exactly as addressed in the URL (already
    /// percent-decoded + validated; the per-key grant vocabulary).
    pub workspace_slug: String,
    /// The registered workspace's absolute path (a `projects.path` value —
    /// NEVER constructed from caller input).
    pub workspace_path: String,
    /// The caller's intent (new / fork / address).
    pub op: WsSessionOp,
}

/// The uniform "no such workspace" 404 — returned for an UNKNOWN slug, an
/// UNAUTHORIZED (not-granted) slug, a malformed segment, and a canonical /
/// unknown session id ALIKE. One indistinguishable response so the surface is
/// never an existence oracle across workspaces or principals (PRD §A, §F).
fn uniform_ws_404() -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: r#"{"error":"no such workspace"}"#.to_string(),
    }
}

/// Percent-decode + validate ONE URL path segment (a workspace slug or a
/// session id) into a safe, separator-free string. Returns `None` (→ the
/// caller maps to a uniform 404) for anything that could escape the intended
/// namespace. Decode happens FIRST, then validation, so an encoded `%2e%2e`
/// (`..`) or `%2f` (`/`) is caught AFTER decoding, not smuggled past it.
///
/// Rejects: empty (pre/post decode), any `/` or `\` separator, the traversal
/// tokens `.`/`..` (and any `..` substring — a workspace basename never needs
/// it), a NUL, and any control char. What survives is a single, printable,
/// separator-free component — and even then the workspace resolver only ever
/// returns a REGISTERED `projects.path`, so a hostile value can at worst 404.
fn decode_and_validate_segment(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let decoded = k2_core::agent_hooks::urldecode(raw);
    if decoded.is_empty() {
        return None;
    }
    if decoded.contains('/') || decoded.contains('\\') || decoded.contains('\0') {
        return None;
    }
    if decoded == "." || decoded == ".." || decoded.contains("..") {
        return None;
    }
    if decoded.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(decoded)
}

/// Resolve a validated workspace `slug` to a REGISTERED workspace's absolute
/// path — name/basename ONLY, NEVER a caller-supplied path.
///
/// **Why this can't be tricked into an arbitrary host path:** the function only
/// ever RETURNS a `projects.path` value already stored in the registry. It
/// matches the slug against `projects.name` (exact, then case-insensitive — the
/// #33 casing fallback) and, failing that, against each registered project's
/// folder BASENAME. It NEVER treats the slug as a path, never joins it onto a
/// root, and (unlike `workspace_msg::resolve_workspace`) has NO absolute-path or
/// UUID branch — a leading `/` can't even reach here (the segment validator
/// rejects separators). So the worst a hostile slug can do is match nothing → 404.
pub fn resolve_workspace_slug(slug: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();

    // Exact display-name match (the common case; slug == workspace name).
    if let Ok(path) = conn.query_row(
        "SELECT path FROM projects WHERE name = ?1 ORDER BY rowid LIMIT 1",
        rusqlite::params![slug],
        |r| r.get::<_, String>(0),
    ) {
        return Some(path);
    }
    // Case-insensitive name fallback (mirrors resolve_workspace #33 — LLM/human
    // callers infer plausible casing). Exact-case still wins above.
    if let Ok(path) = conn.query_row(
        "SELECT path FROM projects WHERE name = ?1 COLLATE NOCASE ORDER BY rowid LIMIT 1",
        rusqlite::params![slug],
        |r| r.get::<_, String>(0),
    ) {
        return Some(path);
    }
    // Basename match: the slug equals a registered project's FOLDER basename
    // (for workspaces whose display name was customized away from the folder).
    // We compare the basename of each stored path — still only ever returning a
    // registered path.
    let mut stmt = conn.prepare("SELECT path FROM projects").ok()?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).ok()?;
    for p in rows.flatten() {
        let matches_base = std::path::Path::new(&p)
            .file_name()
            .map(|b| b.to_string_lossy().eq_ignore_ascii_case(slug))
            .unwrap_or(false);
        if matches_base {
            return Some(p);
        }
    }
    None
}

/// Resolve `slug` → an AUTHORIZED registered workspace path, or a uniform 404
/// (PRD §G2 #4 — per-key workspace scoping done right).
///
/// **Authz FIRST, then existence** — and BOTH failures return the SAME 404, so
/// the door is never an existence oracle:
/// - [`V1Principal::Owner`] (owner token) → authorized for ALL workspaces.
/// - [`V1Principal::Api`] → authorized ONLY if its per-key grant
///   ([`k2_core::api_keys::ApiPrincipal::authorizes_workspace`]) admits `slug`.
///   FAIL-CLOSED: a key with no grant (NULL column — the value existing rows
///   backfill to) reaches ZERO workspaces.
/// An unauthorized key returns before we even resolve, so it can NOT learn
/// whether a workspace it wasn't granted exists (no cross-tenant oracle). An
/// authorized-but-unknown slug returns the identical 404.
pub fn resolve_authorized_workspace(
    principal: &V1Principal,
    slug: &str,
) -> Result<String, CliResponse> {
    let authorized = match principal {
        V1Principal::Owner => true,
        V1Principal::Api(p) => p.authorizes_workspace(slug),
    };
    if !authorized {
        // Unauthorized: identical 404 to "unknown", BEFORE any resolution, so a
        // not-granted key can't distinguish exists-but-forbidden from absent.
        return Err(uniform_ws_404());
    }
    match resolve_workspace_slug(slug) {
        Some(path) => Ok(path),
        None => Err(uniform_ws_404()),
    }
}

/// CANONICAL-OFF-LIMITS guard (PRD §G2 #3, LOCKED) — is `session_id` a
/// workspace's CANONICAL (cockpit-owned) session? The workspace API may ONLY
/// address api-spawned SANDBOX sessions; it must NEVER resolve a caller-supplied
/// id to a canonical session (that would let an external key drive the owner's
/// real workspace agent). We treat a match against `workspace_sessions.session_id`
/// as canonical → the caller gets the uniform 404.
///
/// **FAIL-CLOSED on error:** if the lookup itself errors we return `true`
/// (treat as canonical / off-limits and refuse) rather than risk admitting a
/// canonical id when the DB is momentarily unavailable.
pub fn session_is_canonical(session_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match conn.query_row(
        "SELECT COUNT(*) FROM workspace_sessions WHERE session_id = ?1",
        rusqlite::params![session_id],
        |r| r.get::<_, i64>(0),
    ) {
        Ok(n) => n > 0,
        Err(_) => true, // fail-closed: unknown DB state → treat as off-limits.
    }
}

/// `POST /v1/w/<ws>/sessions` — start a NEW session (or FORK when the body
/// carries `fork_from=<id>`). Validates the slug, authorizes the workspace,
/// validates + canonical-guards any fork source, then (SLICE 1) delegates to
/// the ephemeral spawn.
pub fn handle_v1_ws_new(principal: &V1Principal, ws_raw: &str, body: &[u8]) -> CliResponse {
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Optional `fork_from` in the body = a FORK intent (PRD §G2 #3c). A present
    // but malformed / canonical source is a hard uniform 404 (never silently
    // downgraded to a plain new — the caller asked to branch specific state).
    let op = match parse_fork_from(body) {
        ForkFrom::None => WsSessionOp::New,
        ForkFrom::Invalid => return uniform_ws_404(),
        ForkFrom::Id(src) => {
            if session_is_canonical(&src) {
                return uniform_ws_404();
            }
            WsSessionOp::Fork { from: src }
        }
    };

    let req = WorkspaceSessionRequest {
        workspace_slug: slug,
        workspace_path: ws_path,
        op,
    };
    spawn_for_workspace_slice1(principal, req, body)
}

/// `POST /v1/w/<ws>/sessions/<id>` — ADDRESS an existing sandbox session
/// (message / resume intent; the F3 liveness router decides in slices 2-3).
pub fn handle_v1_ws_address(
    principal: &V1Principal,
    ws_raw: &str,
    sid_raw: &str,
    body: &[u8],
) -> CliResponse {
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let Some(sid) = decode_and_validate_segment(sid_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    // CANONICAL OFF-LIMITS — never address a workspace's canonical session.
    if session_is_canonical(&sid) {
        return uniform_ws_404();
    }

    // OP #3 message-live (F3 liveness router): if the addressed session is LIVE
    // (a running cell registered in v2_session_map), DELIVER the caller's prompt
    // into its interactive claude PTY (submit with CR) instead of cold-resuming a
    // duplicate cell. Cold sessions fall through to the resume spawn below.
    if let Some(parsed) = k2_core::session::SessionId::parse(&sid) {
        if let Some(live) = crate::v2_session_map::lookup_by_session_id(&parsed) {
            crate::sandbox_reaper::stamp(&parsed);
            let prompt = serde_json::from_slice::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("prompt").and_then(|x| x.as_str()).map(str::to_string))
                .unwrap_or_default();
            if !prompt.is_empty() {
                let mut bytes = prompt.into_bytes();
                bytes.push(b'\r');
                live.write(bytes);
            }
            return CliResponse::ok_json(
                serde_json::json!({ "sessionId": sid, "delivered": true, "live": true }).to_string(),
            );
        }
    }

    let req = WorkspaceSessionRequest {
        workspace_slug: slug,
        workspace_path: ws_path,
        op: WsSessionOp::Address { session_id: sid },
    };
    spawn_for_workspace_slice1(principal, req, body)
}

/// `GET /v1/w/<ws>/sessions` — list this workspace's sandbox sessions (audit).
/// Authorizes the workspace, then reads the §5 host BRIDGE index
/// (`sandbox_sessions`) for this workspace slug, newest first. Returns each
/// session's id + created/last-active timestamps (never the host filesystem
/// paths — those are daemon-internal audit-resume state, not caller data).
pub fn handle_v1_ws_list(principal: &V1Principal, ws_raw: &str) -> CliResponse {
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    // Authz first — an unauthorized caller must not even learn the ws exists.
    if let Err(resp) = resolve_authorized_workspace(principal, &slug) {
        return resp;
    }
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::SandboxSession::list_for_workspace(&conn, &slug).unwrap_or_default()
    };
    let sessions: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "sessionId": r.session_id,
                "createdAt": r.created_at,
                "lastActiveAt": r.last_active_at,
            })
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({ "workspace": slug, "sessions": sessions }).to_string(),
    )
}

/// `GET /v1/w/<ws>/sessions/<id>/messages?since=<seq>` — drain a sandbox
/// session's response log. Authorizes the workspace, refuses the canonical
/// session, then reuses the F2 [`handle_messages`] drain (whose own default-deny
/// per-principal owner check still applies — a workspace-authorized caller can
/// still only read a session it OWNS, else 404).
pub fn handle_v1_ws_messages(
    principal: &V1Principal,
    ws_raw: &str,
    sid_raw: &str,
    since: u64,
) -> CliResponse {
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let Some(sid) = decode_and_validate_segment(sid_raw) else {
        return uniform_ws_404();
    };
    if let Err(resp) = resolve_authorized_workspace(principal, &slug) {
        return resp;
    }
    // CANONICAL OFF-LIMITS — never drain a workspace's canonical transcript.
    if session_is_canonical(&sid) {
        return uniform_ws_404();
    }
    handle_messages(principal, &sid, since)
}

/// Parsed `fork_from` field of a `POST .../sessions` body.
enum ForkFrom {
    /// No `fork_from` present (or blank) → a plain new session.
    None,
    /// Present but not a safe segment → hard 404 (the caller asked to fork a
    /// specific source; we refuse rather than silently make a fresh session).
    Invalid,
    /// A validated, separator-free source session id.
    Id(String),
}

/// Extract + validate the optional `fork_from` source id from a
/// `POST .../sessions` body. Accepts `fork_from` or `forkFrom`. Absent/blank →
/// [`ForkFrom::None`]; present-but-unsafe → [`ForkFrom::Invalid`]. A non-JSON /
/// empty body is simply "no fork" (parity with the ephemeral route's tolerant
/// body handling).
fn parse_fork_from(body: &[u8]) -> ForkFrom {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return ForkFrom::None;
    }
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        // A malformed body is NOT a fork request — the new-session handler will
        // itself run; treat as no-fork here (the spawn path re-parses the body).
        Err(_) => return ForkFrom::None,
    };
    let raw = v
        .get("fork_from")
        .or_else(|| v.get("forkFrom"))
        .and_then(|x| x.as_str());
    match raw {
        None => ForkFrom::None,
        Some(s) if s.trim().is_empty() => ForkFrom::None,
        Some(s) => match decode_and_validate_segment(s.trim()) {
            Some(id) => ForkFrom::Id(id),
            None => ForkFrom::Invalid,
        },
    }
}

/// SLICE 2 — WORKSPACE-SCOPED spawn. Replaces the slice-1 ephemeral delegate:
/// provisions the PERSISTENT per-session overlay layer via
/// [`resolve_workspace_session`] and spawns through the PROVEN v2 internals,
/// mirroring [`handle_v1_sandboxes`]'s door discipline (refuse-if-can't,
/// quota-first, fail-closed, per-session stream token, F2 owner record).
///
/// SESSION ID per op (PRD §G2 #3): `New`/`Fork` mint a FRESH id (the layer +
/// the returned/addressable id are keyed by it); `Address` uses the URL id
/// (parsed to a real `SessionId`, else 400). The id is FORCED into the spawn so
/// the returned `sessionId` equals the persistent-layer key — resume can
/// re-find it.
///
/// SLICE 3: the worker mounts `overlay(RO ws, upper)` at `/workspace`.
/// SLICE 4: `Fork` must COW-copy the source session's layer into the new id's
/// layer, and `Address` must run the F3 LIVENESS router (deliver-into-live vs
/// `claude --resume` + re-mount). For SLICE 2 all three provision + spawn with
/// their (minted or given) id + a fresh/reused layer; the fork COW-copy and the
/// live-delivery routing are NOT yet implemented (flagged below).
fn spawn_for_workspace_slice1(
    principal: &V1Principal,
    req: WorkspaceSessionRequest,
    body: &[u8],
) -> CliResponse {
    // (1) REFUSE if this daemon cannot deliver a real microVM (never degrade to
    // an unsandboxed cell) — identical to the ephemeral door.
    if !v2_spawn::can_sandbox() {
        return CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: r#"{"error":"this daemon cannot sandbox (microVM backend unavailable)"}"#
                .to_string(),
        };
    }

    // (2) Parse the UNTRUSTED body into hints (prompt/dims). Empty → defaults.
    // The `fork_from` field was already parsed + validated by the caller into
    // `req.op`; here we only need the prompt/dim hints.
    let api_req: ApiSandboxRequest = if body.iter().all(|b| b.is_ascii_whitespace()) {
        ApiSandboxRequest::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(r) => r,
            Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
        }
    };

    // (3) Decide the HOST session id from the op (fresh mint vs the addressed
    // id). SLICE 4 will branch here on live-vs-cold for `Address` and COW the
    // source layer for `Fork`; SLICE 2 just keys the layer by this id.
    let session_id: SessionId = match &req.op {
        WsSessionOp::New => SessionId::new(),
        WsSessionOp::Fork { from } => {
            // SLICE 4: COW-copy `<from>`'s persistent layer into the new id's
            // layer (branch its files). SLICE 2 provisions a FRESH empty layer.
            k2_core::log_debug!(
                "[v1-sandbox/ws] fork_from={} — fresh layer for now (COW-copy is SLICE 4)",
                from
            );
            SessionId::new()
        }
        WsSessionOp::Address { session_id } => match SessionId::parse(session_id) {
            Some(sid) => sid,
            // A validated-but-non-UUID segment can't key a real session/layer.
            None => return uniform_ws_404(),
        },
    };

    // (4) P4-H4 CONCURRENT-CELL CAP — acquire the quota slot BEFORE we provision
    // the layer, so a rejected request leaves ZERO side effects (no dir mkdir,
    // no spawn). Released by the child-exit observer on the success path, or
    // explicitly on the early-failure paths below.
    let principal_key = principal.display_id();
    if let Err(qe) = sandbox_quota::try_acquire(&principal_key) {
        return CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: serde_json::json!({ "error": qe.message(), "code": qe.code() }).to_string(),
        };
    }

    // (5) Resolve → host-trusted SpawnRequest carrying the RO workspace base +
    // the PERSISTENT per-session layer. Fail-CLOSED: a provisioning failure
    // 5xxs (NEVER a $HOME/ephemeral fallback); release the slot we acquired.
    let mut spawn_req = match resolve_workspace_session(
        &req.workspace_path,
        &req.workspace_slug,
        &session_id,
        principal,
        &api_req,
    ) {
        Ok(r) => r,
        Err(e) => {
            sandbox_quota::release(&principal_key);
            return CliResponse {
                status: e.status(),
                content_type: "application/json",
                body: serde_json::json!({ "error": e.message() }).to_string(),
            };
        }
    };
    spawn_req.principal_key = Some(principal_key.clone());

    // Idle-reap timeout — the caller's per-request knob (default 180s), NOT a
    // magic number. Staged into the cell env as a self-terminate backstop and
    // armed on the daemon idle-reaper after a successful spawn (below).
    let timeout_secs = crate::sandbox_reaper::normalize_timeout(
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("timeout_secs").and_then(|x| x.as_u64())),
    );
    if let Some(env) = spawn_req.env.as_mut() {
        env.insert("K2_SANDBOX_TIMEOUT_SECS".to_string(), timeout_secs.to_string());
    }

    // Capture the MIRROR spec's host paths BEFORE the spawn consumes `spawn_req`,
    // so we can record the §5 index row after a successful spawn (below).
    // SLICE 4 (reopen/resume): an addressed op re-opens an existing session, so
    // tell the guest-init to `claude --resume <id>` (its K2_RESUME branch) instead
    // of minting a fresh `--session-id`. The persistent per-session layer already
    // holds the prior `<id>.jsonl` at the real slug path, so --resume finds it.
    if matches!(req.op, WsSessionOp::Address { .. }) {
        if let Some(env) = spawn_req.env.as_mut() {
            env.insert("K2_RESUME".to_string(), "1".to_string());
        }
    }

    // OP #4 fork: COW-copy the source session's layer into this fresh id's layer
    // (rename <src>.jsonl -> <dst>.jsonl) and resume it, so the fork continues the
    // source's history under a NEW id. The spawn door recursively re-chowns the
    // copied files to the new cell uid.
    if let WsSessionOp::Fork { from } = &req.op {
        if let Err(e) = policy::fork_layer(&req.workspace_slug, from, &session_id.to_string()) {
            sandbox_quota::release(&principal_key);
            return CliResponse {
                status: e.status(),
                content_type: "application/json",
                body: serde_json::json!({ "error": e.message() }).to_string(),
            };
        }
        if let Some(env) = spawn_req.env.as_mut() {
            env.insert("K2_RESUME".to_string(), "1".to_string());
        }
    }
    let mirror_for_index = spawn_req.overlay.clone();

    // (6) Spawn through the PROVEN v2 internals (per-session uid + P4-H6 chown of
    // the persistent layer, egress, child-exit teardown). NOTE: unlike the
    // ephemeral path we do NOT clean up any dir on failure — the persistent
    // layer is meant to survive (a later resume re-uses it); only the quota slot
    // is released (no session ⇒ no observer).
    let result = v2_spawn::spawn_session(spawn_req);
    if result.status != "200 OK" {
        sandbox_quota::release(&principal_key);
        return CliResponse {
            status: result.status,
            content_type: "application/json",
            body: result.body,
        };
    }

    // (7) Parse the spawn response; record the F2 owner; mint a per-session
    // stream token. A live session (+ its observer) now exists, so the observer
    // OWNS the quota release — we must NOT release here.
    let v: serde_json::Value = match serde_json::from_str(&result.body) {
        Ok(v) => v,
        Err(_) => return CliResponse::internal_error("spawn response was not JSON"),
    };
    let session_id_str = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("");
    let agent_name = v.get("agentName").and_then(|x| x.as_str()).unwrap_or("");
    if session_id_str.is_empty() {
        return CliResponse::internal_error("spawn response carried no sessionId");
    }
    let Some(sid) = k2_core::session::SessionId::parse(session_id_str) else {
        return CliResponse::internal_error("spawn response sessionId was malformed");
    };
    crate::sandbox_responses::record_owner(session_id_str, &principal_key);

    // Arm the idle-reaper for this cell (idempotent; a resume re-arms the clock).
    crate::sandbox_reaper::register(sid, timeout_secs);

    // (§5 index) Record the host BRIDGE row so the per-workspace LIST surfaces
    // this session (audit) and the resume path can re-mount its sandbox home +
    // `/work` layer. The claude-session-id == `K2_SESSION_ID` == `session_id_str`
    // (we inject it), so the `.jsonl` real path is KNOWN:
    // `<sandbox_home>/projects/<slug>/<session_id>.jsonl`. Best-effort — a failed
    // insert must NOT fail the (already live) session; it only degrades audit.
    if let Some(m) = mirror_for_index.as_ref() {
        let sandbox_home_path = m.sandbox_home_rw.to_string_lossy().into_owned();
        let jsonl_path = m
            .sandbox_home_rw
            .join("projects")
            .join(&m.slug)
            .join(format!("{session_id_str}.jsonl"))
            .to_string_lossy()
            .into_owned();
        let row = k2_core::db::schema::SandboxSession {
            session_id: session_id_str.to_string(),
            workspace_slug: req.workspace_slug.clone(),
            workspace_path: m.workspace_ro.to_string_lossy().into_owned(),
            sandbox_home_path,
            jsonl_path,
            layer_path: m.work_rw.to_string_lossy().into_owned(),
            slug: m.slug.clone(),
            created_at: 0,      // ignored — table default is unixepoch()
            last_active_at: 0,  // ignored — table default is unixepoch()
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = k2_core::db::schema::SandboxSession::upsert(&conn, &row) {
            k2_core::log_debug!(
                "[v1-sandbox/ws] WARN could not record sandbox_sessions index row for session={session_id_str}: {e}"
            );
        }
    }

    let stream_tok = stream_token::mint(&sid);
    let grid = format!("/cli/sessions/grid?session={session_id_str}&token={stream_tok}");
    CliResponse::ok_json(
        serde_json::json!({
            "sessionId": session_id_str,
            "agentName": agent_name,
            "workspace": req.workspace_slug,
            "sandbox": "microvm",
            "stream": { "grid": grid },
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On this (macOS / feature-off) build `can_sandbox()` is false, so the
    /// route MUST 409 BEFORE provisioning anything — never degrading to an
    /// unsandboxed passthrough cell. This is the load-bearing refuse-if-can't
    /// invariant. (On the box with K2_SANDBOX the spawn path is exercised by the
    /// on-box test; it can't run on Mac because the microVM backend is absent.)
    #[test]
    fn refuses_409_when_daemon_cannot_sandbox() {
        // Guard: this assertion is only meaningful where can_sandbox() is false
        // — which is every build this test suite runs on (macOS / feature-off).
        assert!(
            !v2_spawn::can_sandbox(),
            "test premise: this build cannot sandbox",
        );
        let principal = V1Principal::Owner;
        let resp = handle_v1_sandboxes(&principal, b"{}");
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        assert!(
            resp.body.contains("cannot sandbox"),
            "409 must explain the refusal; body={}",
            resp.body
        );
    }

    /// The 409 fires for an API-key principal too (same refuse-if-can't door).
    #[test]
    fn refuses_409_for_api_principal_too() {
        let principal = V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: "key-x".to_string(),
            anthropic_key: Some("sk-ant-x".to_string()),
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
        });
        let resp = handle_v1_sandboxes(&principal, b"");
        assert_eq!(resp.status, "409 Conflict");
    }

    fn api(id: &str) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: None,
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
        })
    }

    /// F2 AUTHZ: the owning principal can read its session's log; a DIFFERENT
    /// principal and the owner sentinel both get a uniform 404 (no cross-principal
    /// existence oracle), and an unknown session is 404 too.
    #[test]
    fn messages_authz_only_owner_reads_else_404() {
        let sid = "f2-authz-sess";
        crate::sandbox_responses::record_owner(sid, "key-owner");
        crate::sandbox_responses::append(sid, "hello".to_string(), false);

        // Owner reads its own log.
        let ok = handle_messages(&api("key-owner"), sid, 0);
        assert_eq!(ok.status, "200 OK", "owner must read its own log; body={}", ok.body);
        assert!(ok.body.contains("hello"), "log line surfaced: {}", ok.body);
        assert!(ok.body.contains("\"latest_seq\":1"), "cursor advanced: {}", ok.body);

        // A different API principal is REFUSED (404, no leak).
        let other = handle_messages(&api("key-attacker"), sid, 0);
        assert_eq!(other.status, "404 Not Found", "cross-principal read must 404");
        assert!(!other.body.contains("hello"), "must NOT leak the transcript");

        // The owner-token principal does not match an api-key owner → 404.
        let owner_tok = handle_messages(&V1Principal::Owner, sid, 0);
        assert_eq!(owner_tok.status, "404 Not Found");

        // Unknown session → 404 (uniform with the mismatch case).
        let unknown = handle_messages(&api("key-owner"), "no-such-session-xyz", 0);
        assert_eq!(unknown.status, "404 Not Found");
    }

    /// `since` is a real cursor: a second poll past the last seq returns empty and
    /// holds the cursor at `since`.
    #[test]
    fn messages_since_cursor_drains_incrementally() {
        let sid = "f2-cursor-sess";
        crate::sandbox_responses::record_owner(sid, "key-c");
        crate::sandbox_responses::append(sid, "one".to_string(), false);
        crate::sandbox_responses::append(sid, "two".to_string(), true);

        // since=1 returns only the second line.
        let r = handle_messages(&api("key-c"), sid, 1);
        assert_eq!(r.status, "200 OK");
        assert!(r.body.contains("two") && !r.body.contains("\"one\""), "body={}", r.body);
        assert!(r.body.contains("\"latest_seq\":2"), "body={}", r.body);

        // since=2 (caller is caught up) → empty, cursor stays at 2.
        let r2 = handle_messages(&api("key-c"), sid, 2);
        assert!(r2.body.contains("\"messages\":[]"), "empty slice; body={}", r2.body);
        assert!(r2.body.contains("\"latest_seq\":2"), "cursor held; body={}", r2.body);
    }

    // ── Sandbox v2 (PRD §A/§G2) — workspace-scoped front door ─────────────
    //
    // Note: `db::shared()` in a test context auto-inits the PROCESS-GLOBAL
    // in-memory DB (`init_for_tests`), shared across every test in the binary.
    // These tests therefore use UNIQUE workspace names / session ids (a
    // `v1ws-` prefix) so they never collide with each other or other modules.
    // On this Mac build `can_sandbox()` is false, so a request that PASSES the
    // door reaches the ephemeral spawn and 409s — a clean discriminator:
    // 404 = blocked at the door, 409 = door passed (reached the spawn).

    fn apik(id: &str, grant: Option<&str>) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: None,
            scope: "owner".to_string(),
            allowed_workspaces: grant.map(str::to_string),
        })
    }

    fn insert_project(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project");
        id
    }

    fn insert_canonical_session(project_id: &str, session_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions \
                 (id, project_id, terminal_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'claude', 'system', 'active', unixepoch())",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), project_id, "term-1", session_id],
        )
        .expect("insert canonical session");
    }

    /// Slug/segment validation: percent-decode FIRST, then reject traversal,
    /// separators, empties, and control chars — so an encoded `..`/`/` cannot
    /// slip past. A normal (or benign percent-encoded) name survives.
    #[test]
    fn segment_validation_rejects_traversal_and_separators() {
        // Accepted.
        assert_eq!(decode_and_validate_segment("ai").as_deref(), Some("ai"));
        assert_eq!(decode_and_validate_segment("my-ws_2").as_deref(), Some("my-ws_2"));
        assert_eq!(decode_and_validate_segment("my%20ws").as_deref(), Some("my ws"));
        // Empty (pre + post decode).
        assert!(decode_and_validate_segment("").is_none());
        // Traversal tokens (literal + encoded → decoded → rejected).
        assert!(decode_and_validate_segment("..").is_none());
        assert!(decode_and_validate_segment(".").is_none());
        assert!(decode_and_validate_segment("a..b").is_none());
        assert!(decode_and_validate_segment("%2e%2e").is_none(), "encoded '..'");
        assert!(decode_and_validate_segment("%2e%2e%2fetc").is_none(), "encoded '../etc'");
        // Separators (literal + encoded).
        assert!(decode_and_validate_segment("a/b").is_none());
        assert!(decode_and_validate_segment("a\\b").is_none());
        assert!(decode_and_validate_segment("a%2fb").is_none(), "encoded '/'");
        // NUL + control chars.
        assert!(decode_and_validate_segment("a%00b").is_none(), "encoded NUL");
        assert!(decode_and_validate_segment("a%0ab").is_none(), "encoded newline");
    }

    /// `fork_from` classification: absent/blank → None; a safe id → Id; an
    /// UNSAFE id (separator / traversal) → Invalid (a hard refusal, never a
    /// silent downgrade to a plain new session).
    #[test]
    fn parse_fork_from_classifies_intent() {
        assert!(matches!(parse_fork_from(b""), ForkFrom::None));
        assert!(matches!(parse_fork_from(b"   "), ForkFrom::None));
        assert!(matches!(parse_fork_from(b"{}"), ForkFrom::None));
        assert!(matches!(parse_fork_from(br#"{"fork_from":""}"#), ForkFrom::None));
        assert!(matches!(parse_fork_from(br#"{"fork_from":"abc-123"}"#), ForkFrom::Id(_)));
        assert!(matches!(parse_fork_from(br#"{"forkFrom":"abc-123"}"#), ForkFrom::Id(_)));
        assert!(matches!(parse_fork_from(br#"{"fork_from":"a/b"}"#), ForkFrom::Invalid));
        assert!(matches!(parse_fork_from(br#"{"fork_from":".."}"#), ForkFrom::Invalid));
        // A malformed body is not a fork request (tolerant — the spawn re-parses).
        assert!(matches!(parse_fork_from(b"not json"), ForkFrom::None));
    }

    /// PER-KEY AUTHZ (PRD §G2 #4). An UNAUTHORIZED api key gets a uniform 404
    /// BEFORE any resolution (no cross-tenant existence oracle) — this branch
    /// needs no DB row. A NO-grant key reaches nothing.
    #[test]
    fn authz_denies_ungranted_key_uniformly_without_oracle() {
        // Granted only ["v1ws-ai"] → addressing a different ws is a uniform 404.
        let scoped = apik("k-scoped", Some(r#"["v1ws-ai"]"#));
        let e = resolve_authorized_workspace(&scoped, "v1ws-secret").unwrap_err();
        assert_eq!(e.status, "404 Not Found");
        assert!(e.body.contains("no such workspace"), "uniform body: {}", e.body);
        // A key with NO grant (NULL column — the fail-closed backfill) reaches
        // nothing, even a name that exists.
        let nogrant = apik("k-none", None);
        assert_eq!(
            resolve_authorized_workspace(&nogrant, "v1ws-anything").unwrap_err().status,
            "404 Not Found",
        );
    }

    /// PER-KEY AUTHZ + RESOLUTION against the registry: owner = all; a granted
    /// (or `*`) key resolves; a not-granted key 404s even though the ws exists
    /// (same 404 as unknown → no oracle); owner on an unknown slug 404s alike.
    #[test]
    fn authz_resolves_for_owner_and_granted_key_only() {
        let path = "/tmp/k2-v1ws-authz-ai";
        insert_project("v1ws-authz-ai", path);

        // Owner → all workspaces. (`.ok()` drops the Err — CliResponse has no
        // Debug — so `.expect` on the Option is the ergonomic assert here.)
        assert_eq!(
            resolve_authorized_workspace(&V1Principal::Owner, "v1ws-authz-ai").ok().expect("owner resolves"),
            path,
        );
        // Explicitly-granted key → resolves.
        let granted = apik("k-g", Some(r#"["v1ws-authz-ai"]"#));
        assert_eq!(
            resolve_authorized_workspace(&granted, "v1ws-authz-ai").ok().expect("granted resolves"),
            path,
        );
        // Wildcard key → resolves.
        let star = apik("k-*", Some("*"));
        assert_eq!(
            resolve_authorized_workspace(&star, "v1ws-authz-ai").ok().expect("wildcard resolves"),
            path,
        );
        // A key granted a DIFFERENT ws → 404 even though this ws exists.
        let other = apik("k-o", Some(r#"["v1ws-other"]"#));
        assert_eq!(
            resolve_authorized_workspace(&other, "v1ws-authz-ai").unwrap_err().status,
            "404 Not Found",
        );
        // Owner on an unknown slug → identical 404 (uniform shape).
        assert_eq!(
            resolve_authorized_workspace(&V1Principal::Owner, "v1ws-nonexistent-xyz")
                .unwrap_err()
                .status,
            "404 Not Found",
        );
    }

    /// CANONICAL-OFF-LIMITS guard (PRD §G2 #3): an id registered as a
    /// workspace's canonical `workspace_sessions.session_id` is off-limits; an
    /// unknown (sandbox) id is not canonical.
    #[test]
    fn canonical_session_guard() {
        let pid = insert_project("v1ws-canon", "/tmp/k2-v1ws-canon");
        insert_canonical_session(&pid, "v1ws-canon-sid-abc");
        assert!(session_is_canonical("v1ws-canon-sid-abc"), "registered canonical id is off-limits");
        assert!(!session_is_canonical("v1ws-unknown-sandbox-id"), "sandbox id is not canonical");
    }

    /// End-to-end `handle_v1_ws_new`: an AUTHORIZED request passes the door and
    /// reaches the ephemeral spawn (409 on this Mac build); every door-block
    /// case (malformed slug, unknown slug, ungranted key, canonical fork
    /// source) returns the uniform 404 and never reaches the spawn.
    #[test]
    fn ws_new_door_passes_authorized_blocks_everything_else() {
        let pid = insert_project("v1ws-new", "/tmp/k2-v1ws-new");

        // Authorized owner + valid slug → reaches spawn → 409 (can't sandbox).
        let r = handle_v1_ws_new(&V1Principal::Owner, "v1ws-new", b"{}");
        assert_eq!(r.status, "409 Conflict", "authorized reaches spawn; body={}", r.body);

        // Malformed slug → 404 at the door.
        assert_eq!(handle_v1_ws_new(&V1Principal::Owner, "%2e%2e", b"{}").status, "404 Not Found");
        // Unknown slug → 404.
        assert_eq!(
            handle_v1_ws_new(&V1Principal::Owner, "v1ws-ghost-none", b"{}").status,
            "404 Not Found",
        );
        // Ungranted api key (ws exists) → 404 (no oracle).
        let ungranted = apik("k-u", Some(r#"["v1ws-other"]"#));
        assert_eq!(handle_v1_ws_new(&ungranted, "v1ws-new", b"{}").status, "404 Not Found");

        // Fork from the workspace's CANONICAL session → off-limits 404.
        insert_canonical_session(&pid, "v1ws-new-canon");
        let body = br#"{"fork_from":"v1ws-new-canon"}"#;
        assert_eq!(handle_v1_ws_new(&V1Principal::Owner, "v1ws-new", body).status, "404 Not Found");
        // Fork from a non-canonical id → passes the door → 409 (reaches spawn).
        let body_ok = br#"{"fork_from":"v1ws-new-sandbox-src"}"#;
        assert_eq!(handle_v1_ws_new(&V1Principal::Owner, "v1ws-new", body_ok).status, "409 Conflict");
    }

    /// `handle_v1_ws_address`: the CANONICAL session is off-limits (404), a
    /// non-canonical sandbox id passes the door (409), and a malformed session
    /// segment 404s.
    #[test]
    fn ws_address_refuses_canonical_session() {
        let pid = insert_project("v1ws-addr", "/tmp/k2-v1ws-addr");
        insert_canonical_session(&pid, "v1ws-addr-canon");

        // Canonical → off-limits.
        assert_eq!(
            handle_v1_ws_address(&V1Principal::Owner, "v1ws-addr", "v1ws-addr-canon", b"{}").status,
            "404 Not Found",
        );
        // Non-canonical (sandbox) id → passes door → 409.
        assert_eq!(
            handle_v1_ws_address(&V1Principal::Owner, "v1ws-addr", "v1ws-addr-sandbox", b"{}").status,
            "409 Conflict",
        );
        // Malformed session segment → 404.
        assert_eq!(
            handle_v1_ws_address(&V1Principal::Owner, "v1ws-addr", "a%2fb", b"{}").status,
            "404 Not Found",
        );
    }

    /// `handle_v1_ws_messages`: canonical is off-limits, and an unowned (but
    /// non-canonical) sandbox id 404s via the reused F2 owner authz.
    #[test]
    fn ws_messages_refuses_canonical_and_reuses_owner_authz() {
        let pid = insert_project("v1ws-msg", "/tmp/k2-v1ws-msg");
        insert_canonical_session(&pid, "v1ws-msg-canon");

        // Canonical transcript → off-limits.
        assert_eq!(
            handle_v1_ws_messages(&V1Principal::Owner, "v1ws-msg", "v1ws-msg-canon", 0).status,
            "404 Not Found",
        );
        // Non-canonical but not owned by this principal → 404 (F2 default-deny).
        assert_eq!(
            handle_v1_ws_messages(&V1Principal::Owner, "v1ws-msg", "v1ws-msg-unowned", 0).status,
            "404 Not Found",
        );
    }

    /// `handle_v1_ws_list`: authorizes first (unauthorized → 404), then returns
    /// an empty sandbox-session list for slice 1.
    #[test]
    fn ws_list_authorizes_then_returns_empty() {
        insert_project("v1ws-list", "/tmp/k2-v1ws-list");

        let r = handle_v1_ws_list(&V1Principal::Owner, "v1ws-list");
        assert_eq!(r.status, "200 OK", "authorized list; body={}", r.body);
        assert!(r.body.contains("\"sessions\":[]"), "empty session list; body={}", r.body);
        // Unauthorized key → 404 (must not learn the ws exists).
        let nogrant = apik("k-list-none", None);
        assert_eq!(handle_v1_ws_list(&nogrant, "v1ws-list").status, "404 Not Found");
    }
}
