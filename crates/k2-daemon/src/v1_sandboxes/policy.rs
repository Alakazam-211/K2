//! P3b POLICY-RESOLVER — the HOST-SIDE AUTHZ DOOR for `POST /v1/sandboxes`.
//!
//! This is the security boundary for external callers. [`resolve_spawn`] takes
//! the UNTRUSTED public body ([`ApiSandboxRequest`] — every field a HINT) plus a
//! HOST-RESOLVED [`V1Principal`] (the authenticated identity, never the wire),
//! and produces a HOST-TRUSTED [`SpawnRequest`]. The cell/caller NEVER decides
//! workspace, command, env, credential, or identity. The invariants:
//!
//! - `sandbox = Some(true)` always (the route 409s before us if it can't deliver).
//! - `cwd` = a freshly PROVISIONED ephemeral dir, chowned to the sandbox-cell
//!   uid. **NEVER `$HOME`, NEVER a caller path.** No dir → [`PolicyError`]
//!   (fail-closed); we NEVER fall back to `$HOME` (= mounting the operator's home
//!   into the cell = sandbox-escape).
//! - `agent_name` = host-minted `api-<principal>-<uuid>` (no find-or-spawn hijack
//!   of an existing `tab-…` / pinned-UUID session).
//! - `command`/`args` = `claude` + the standard headless flags. The caller's
//!   command/args/flags are DROPPED ENTIRELY (no RCE from the wire).
//! - `env` = HOST-CURATED ONLY. The caller's env is DROPPED ENTIRELY. The
//!   Anthropic key is staged from the PRINCIPAL (never the body), reusing B3a's
//!   `Provider → key_env_var` mapping.

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::sandbox::OverlaySpec;

use crate::routes::http::V1Principal;
use crate::session_token::Provider;
use crate::v2_spawn::SpawnRequest;

/// The UNTRUSTED public request body. Every field is a HINT, never a trust
/// input. Absent/empty body → all defaults.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ApiSandboxRequest {
    /// Optional initial prompt. **IGNORED for v1** (see module note on prompt
    /// handling) — never forwarded as argv. Parsed so the field is accepted +
    /// explicitly dropped rather than erroring the request.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Optional terminal width hint (clamped host-side).
    #[serde(default)]
    pub cols: Option<u16>,
    /// Optional terminal height hint (clamped host-side).
    #[serde(default)]
    pub rows: Option<u16>,
}

/// Why the resolver REFUSED to produce a host-trusted request. Fail-closed: the
/// route maps this to a 5xx and the ephemeral dir (if any) is cleaned up — we
/// NEVER degrade to a `$HOME` cwd.
#[derive(Debug)]
pub enum PolicyError {
    /// No ephemeral workspace could be provisioned/handed to the cell uid.
    NoEphemeralWorkspace(String),
    /// Sandbox v2 (PRD §B/§C): the PERSISTENT per-session overlay layer
    /// (`~/.k2/sandbox-overlays/<ws>/<sid>/{upper,work}`) could not be
    /// provisioned (mkdir/chmod/containment failure). Fail-CLOSED: the route
    /// 5xxs; we NEVER fall back to `$HOME` or a throwaway dir.
    NoPersistentLayer(String),
}

impl PolicyError {
    /// HTTP status for this failure (fail-closed — a server-side provisioning
    /// failure, not the caller's fault).
    pub fn status(&self) -> &'static str {
        match self {
            PolicyError::NoEphemeralWorkspace(_) => "503 Service Unavailable",
            PolicyError::NoPersistentLayer(_) => "503 Service Unavailable",
        }
    }

    /// Client-safe message (no secrets; the path is daemon-internal but benign).
    pub fn message(&self) -> String {
        match self {
            PolicyError::NoEphemeralWorkspace(e) => {
                format!("could not provision sandbox workspace: {e}")
            }
            PolicyError::NoPersistentLayer(e) => {
                format!("could not provision persistent session layer: {e}")
            }
        }
    }
}

/// The standard headless claude flags for an API sandbox session. The microVM
/// jail — NOT claude's permission prompts — is the security boundary, so the
/// in-jail agent runs headless (mirrors the pinned-chat base flags). The caller
/// CANNOT add to or override these; `spawn_session` additionally auto-injects a
/// fresh `--session-id` for conversation persistence.
const STANDARD_CLAUDE_ARGS: &[&str] = &["--dangerously-skip-permissions"];

/// In-cell scratch tmpdir staged as `CLAUDE_CODE_TMPDIR` (F2 cell-recipe). A
/// single easy-to-change constant: the box must confirm claude-as-guest-root can
/// create a 0-owned dir here; fallback `/run/cc-tmp` needs a guest-init tmpfs
/// mount. (We validate on the box later — the seam is just this constant.)
const CELL_TMPDIR: &str = "/dev/shm/cc";

/// Git identity DEFAULTS staged into the cell env (F2) so an in-cell `git init`
/// + commit works without a global gitconfig (the ephemeral cwd has none). Used
/// for BOTH author and committer name/email.
const CELL_GIT_NAME: &str = "K2 Sandbox";
const CELL_GIT_EMAIL: &str = "sandbox@k2.local";

/// Resolve the untrusted body + authenticated principal into a host-trusted
/// [`SpawnRequest`]. See the module note for the full invariant list. The
/// returned request carries `ephemeral_cwd = Some(<the provisioned dir>)` so the
/// child-exit observer tears the dir down on session exit.
pub fn resolve_spawn(
    principal: &V1Principal,
    req: &ApiSandboxRequest,
) -> Result<SpawnRequest, PolicyError> {
    // (1) Ephemeral cwd — provision + chown to the cell uid. Fail-CLOSED: a
    // provisioning failure returns Err; we NEVER fall back to `default_cwd()`
    // (= `$HOME`), which would mount the operator's home into the cell.
    let cwd = provision_ephemeral_workspace()?;
    let cwd_str = cwd.to_string_lossy().into_owned();
    // Risk-3 belt: the ephemeral cwd is NEVER the operator's home.
    debug_assert!(
        Some(cwd.as_path()) != dirs::home_dir().as_deref(),
        "ephemeral sandbox cwd must NEVER be $HOME",
    );

    // (2) Host-minted agent_name — the `api-` prefix + a fresh uuid guarantees
    // it can never collide with a `tab-<id>` ad-hoc tab or a bare-UUID pinned
    // session, so the find-or-spawn path can't be hijacked into an existing
    // session by a caller-influenced name.
    let agent_name = format!(
        "api-{}-{}",
        sanitize_id(&principal.display_id()),
        uuid::Uuid::new_v4()
    );

    // (3) Host-curated env — the caller's env is DROPPED ENTIRELY; the Anthropic
    // key is staged from the validated PRINCIPAL (never the body) + the F2
    // cell-recipe defaults + the (un-logged) prompt. Shared with the
    // workspace-scoped resolver so both doors curate an IDENTICAL cell env.
    let env = build_cell_env(principal, req);

    // (4) Dimensions are hints only — clamp to sane bounds.
    let cols = clamp_dim(req.cols, 80, 16, 500);
    let rows = clamp_dim(req.rows, 24, 4, 300);

    Ok(SpawnRequest {
        agent_name,
        cwd: cwd_str,
        // DROP any caller command/args/flags — host-fixed claude + headless args.
        command: Some("claude".to_string()),
        args: Some(STANDARD_CLAUDE_ARGS.iter().map(|s| s.to_string()).collect()),
        cols,
        rows,
        env: Some(env),
        label: None,
        label_locked: None,
        // FORCE sandbox on — the route refused already if it can't deliver.
        sandbox: Some(true),
        ephemeral_cwd: Some(cwd),
        // P4-H4: the spawn DOOR (`handle_v1_sandboxes`) acquires the quota slot
        // and stamps the principal key here AFTER resolve, so the child-exit
        // observer can release it. The resolver leaves it `None`.
        principal_key: None,
        // The ephemeral `/v1/sandboxes` path is NOT workspace-scoped: no overlay
        // mount, and the session id is minted by the spawn (no persistent layer
        // to key). Both are `Some` only on the workspace-scoped door below.
        overlay: None,
        forced_session_id: None,
    })
}

/// Clamp an optional dimension hint into `[min, max]`, defaulting when absent.
fn clamp_dim(hint: Option<u16>, default: u16, min: u16, max: u16) -> u16 {
    hint.unwrap_or(default).clamp(min, max)
}

/// Keep only URL/path-safe identity chars so the host-minted `agent_name` is
/// well-formed regardless of the principal id shape (api-key ids are UUIDs / the
/// literal `"owner"`, but defend anyway).
fn sanitize_id(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if cleaned.is_empty() {
        "anon".to_string()
    } else {
        cleaned
    }
}

/// The root under which per-session ephemeral workspaces are provisioned.
fn sandbox_sessions_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("sandbox-sessions")
}

/// Provision a fresh, EMPTY per-session workspace dir for the cell.
///
/// **P4-H6 — the chown moved to the spawn door.** Through H5 this function
/// chowned the dir to the shared `k2cell` uid. With per-session uids the cell's
/// drop uid is ALLOCATED by the spawn door (`v2_spawn::spawn_session`), which is
/// the only place that knows it — so the door does the `chown <ephemeral> →
/// <per-session uid>` (fail-closed there) right before it boots the cell. Here
/// we only MINT the empty dir (daemon-owned). Fail-CLOSED on mkdir; the caller
/// NEVER falls back to `$HOME`.
fn provision_ephemeral_workspace() -> Result<PathBuf, PolicyError> {
    let dir = sandbox_sessions_root().join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).map_err(|e| {
        PolicyError::NoEphemeralWorkspace(format!("mkdir {}: {e}", dir.display()))
    })?;
    // P4-H6 hardening: force 0700 — `create_dir_all` honors umask (often 022 →
    // 0755), and once the spawn door chowns this dir to the per-session cell
    // uid, 0700 means NO other cell's uid can traverse/read it host-side
    // (defense-in-depth on top of the pivot_root primary isolation, which
    // already prevents a cell from seeing any dir but its own bind-mounted one).
    // Root (the worker, pre-drop) still binds it in regardless.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            PolicyError::NoEphemeralWorkspace(format!("chmod 0700 {}: {e}", dir.display()))
        })?;
    }
    log_debug!(
        "[v1-sandbox] provisioned ephemeral workspace {} (per-session chown deferred to the spawn door — P4-H6)",
        dir.display()
    );
    Ok(dir)
}

// ─────────────────────────────────────────────────────────────────────────
// Sandbox v2 (PRD §B/§C/§G2) — WORKSPACE-SCOPED persistent session provisioning.
//
// `resolve_workspace_session` is the workspace analogue of `resolve_spawn`: it
// produces a host-trusted [`SpawnRequest`] that carries the RO canonical
// workspace base + a PERSISTENT per-(workspace, session) writable layer, instead
// of a throwaway ephemeral dir. SLICE 2 = this provisioning + threading; the
// worker MOUNT of the overlay is SLICE 3.
// ─────────────────────────────────────────────────────────────────────────

/// The in-guest mount point a workspace-scoped cell lands in (PRD §B). SLICE 3
/// mounts the merged overlay(RO base, upper, work) here; the cell's cwd is set
/// to it. It is a GUEST path (never a host path, never `$HOME`).
const GUEST_WORKSPACE_MOUNT: &str = "/workspace";

/// Build the HOST-CURATED cell env shared by BOTH sandbox doors (ephemeral
/// `resolve_spawn` and workspace-scoped `resolve_workspace_session`) so the two
/// curate an IDENTICAL environment. The caller's env is DROPPED ENTIRELY; the
/// Anthropic key is staged from the validated PRINCIPAL (never the body), the
/// F2 cell-recipe defaults are seeded, and a non-empty prompt is staged
/// (un-logged) into `K2_REQUEST_PROMPT`. Neither the key nor the prompt VALUE is
/// ever logged.
fn build_cell_env(principal: &V1Principal, req: &ApiSandboxRequest) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = HashMap::new();
    // Anthropic key — from the PRINCIPAL only (reuses B3a's Provider mapping).
    match principal {
        V1Principal::Api(p) => match p.anthropic_key.as_deref().map(str::trim) {
            Some(key) if !key.is_empty() => {
                env.insert(Provider::Anthropic.key_env_var().to_string(), key.to_string());
                log_debug!(
                    "[v1-sandbox] staged principal Anthropic key into session env (principal={})",
                    p.id
                );
            }
            _ => log_debug!(
                "[v1-sandbox] principal={} has no usable Anthropic key; cell will have no model credential",
                p.id
            ),
        },
        V1Principal::Owner => {
            log_debug!(
                "[v1-sandbox] owner principal: no app-level Anthropic key fallback (follow-up); none staged"
            );
        }
    }

    // F2 cell-recipe DEFAULTS (`or_insert_with` — a pre-set value wins; nothing
    // here clobbers the principal key). IS_SANDBOX lets claude run headless as
    // guest-root; CLAUDE_CODE_TMPDIR points at the cell scratch; GIT_* give
    // `git init`+commit a working identity with no global config.
    env.entry("IS_SANDBOX".to_string()).or_insert_with(|| "1".to_string());
    env.entry("CLAUDE_CODE_TMPDIR".to_string())
        .or_insert_with(|| CELL_TMPDIR.to_string());
    env.entry("GIT_AUTHOR_NAME".to_string())
        .or_insert_with(|| CELL_GIT_NAME.to_string());
    env.entry("GIT_COMMITTER_NAME".to_string())
        .or_insert_with(|| CELL_GIT_NAME.to_string());
    env.entry("GIT_AUTHOR_EMAIL".to_string())
        .or_insert_with(|| CELL_GIT_EMAIL.to_string());
    env.entry("GIT_COMMITTER_EMAIL".to_string())
        .or_insert_with(|| CELL_GIT_EMAIL.to_string());

    // Prompt: UN-DROPPED (F2), staged into the cell env (only when non-empty),
    // never as argv, VALUE never logged.
    if let Some(p) = req.prompt.as_deref() {
        if !p.trim().is_empty() {
            env.entry("K2_REQUEST_PROMPT".to_string())
                .or_insert_with(|| p.to_string());
            log_debug!(
                "[v1-sandbox] staged caller prompt into cell env as K2_REQUEST_PROMPT (value NOT logged)"
            );
        }
    }
    env
}

/// The root under which per-(workspace, session) PERSISTENT overlay layers live:
/// `~/.k2/sandbox-overlays`. Derived ONLY from the daemon's home dir (never a
/// caller path), mirroring [`sandbox_sessions_root`].
fn sandbox_overlays_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("sandbox-overlays")
}

/// Re-assert (defense in depth) that a path SEGMENT — the `<ws-slug>` or
/// `<session-id>` we key the persistent layer by — is safe to use as a single
/// path component. Slice 1's [`super::decode_and_validate_segment`] already
/// validated these off the wire, but a path built from them must NEVER escape
/// `~/.k2/sandbox-overlays/`, so we re-check HERE at the point of path
/// construction: reject empty, any `/`/`\` separator, `\0`, the traversal
/// tokens `.`/`..` (and any `..` substring), and any control char. What passes
/// is a single, printable, separator-free component.
fn assert_safe_path_segment(seg: &str) -> Result<(), PolicyError> {
    let bad = seg.is_empty()
        || seg.contains('/')
        || seg.contains('\\')
        || seg.contains('\0')
        || seg == "."
        || seg == ".."
        || seg.contains("..")
        || seg.chars().any(|c| c.is_control());
    if bad {
        return Err(PolicyError::NoPersistentLayer(format!(
            "unsafe path segment {seg:?}"
        )));
    }
    Ok(())
}

/// The provisioned PERSISTENT layer for a `(workspace, session)` pair.
struct PersistentLayer {
    /// `~/.k2/sandbox-overlays/<ws>/<sid>/upper` (0700, chowned to the cell uid
    /// at the spawn door). The session's durable writable layer.
    upper: PathBuf,
    /// `~/.k2/sandbox-overlays/<ws>/<sid>/work` (0700). The overlayfs workdir.
    work: PathBuf,
}

/// Create (idempotently) the PERSISTENT per-`(ws_slug, session_id)` writable
/// layer and return its `{upper, work}` dirs. Security-critical:
///
/// - The path is built ONLY from the daemon HOME dir + the two RE-ASSERTED
///   segments — never a caller path. `..`/`/`/control chars can't reach here.
/// - After construction we assert the leaf `<ws>/<sid>` dir starts_with the
///   overlays root LEXICALLY (no `fs::canonicalize`, which would follow
///   symlinks — we respect the P4 TOCTOU posture and never follow a symlink
///   into the layer). Combined with the segment re-assert, the layer can never
///   escape `~/.k2/sandbox-overlays/`.
/// - `upper`/`work` are forced to 0700 (create_dir_all honors umask). The spawn
///   door then chowns them to the per-session cell uid (P4-H6) → cross-tenant
///   unreadable. Idempotent: re-provisioning an existing layer (a resume)
///   re-creates nothing but re-asserts perms.
/// - FAIL-CLOSED: any mkdir/chmod/containment failure returns
///   [`PolicyError::NoPersistentLayer`]; the caller 5xxs and NEVER falls back to
///   `$HOME` or an ephemeral dir.
fn provision_persistent_layer(
    ws_slug: &str,
    session_id: &str,
) -> Result<PersistentLayer, PolicyError> {
    // (1) RE-ASSERT both segments before they touch a path (defense in depth).
    assert_safe_path_segment(ws_slug)?;
    assert_safe_path_segment(session_id)?;

    // (2) Build the leaf path from HOME + validated segments ONLY.
    let root = sandbox_overlays_root();
    let leaf = root.join(ws_slug).join(session_id);

    // (3) CONTAINMENT assertion (lexical, symlink-free). Every component of the
    // leaf beyond the root must be a plain `Normal` component (no `..`, no root,
    // no prefix), and the leaf must start_with the root. This can't be tricked
    // because the segments are already `..`/separator-free, but we assert it as
    // the load-bearing invariant rather than trusting the callers.
    if !leaf.starts_with(&root) {
        return Err(PolicyError::NoPersistentLayer(format!(
            "constructed layer path {} escapes overlays root {}",
            leaf.display(),
            root.display()
        )));
    }
    for comp in leaf.strip_prefix(&root).unwrap_or(&leaf).components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return Err(PolicyError::NoPersistentLayer(format!(
                "constructed layer path {} contains a non-normal component",
                leaf.display()
            )));
        }
    }

    let upper = leaf.join("upper");
    let work = leaf.join("work");

    // (4) Create the layer dirs idempotently, fail-CLOSED on any error.
    for dir in [&upper, &work] {
        std::fs::create_dir_all(dir).map_err(|e| {
            PolicyError::NoPersistentLayer(format!("mkdir {}: {e}", dir.display()))
        })?;
        // Force 0700 (create_dir_all honors umask → often 0755). After the spawn
        // door chowns to the per-session uid, 0700 keeps the layer private to
        // that cell host-side (P4-H6 defense-in-depth atop the pivot_root jail).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
                PolicyError::NoPersistentLayer(format!("chmod 0700 {}: {e}", dir.display()))
            })?;
        }
    }

    log_debug!(
        "[v1-sandbox/ws] provisioned persistent layer upper={} work={} (0700; per-session chown deferred to the spawn door — P4-H6)",
        upper.display(),
        work.display()
    );
    Ok(PersistentLayer { upper, work })
}

/// Resolve a WORKSPACE-SCOPED session into a host-trusted [`SpawnRequest`] that
/// carries the RO canonical workspace + the PERSISTENT per-session overlay layer
/// (PRD §B/§C). The workspace analogue of [`resolve_spawn`]:
///
/// - `ws_path` = the HOST-RESOLVED, AUTHORIZED workspace absolute path (a
///   `projects.path` from `resolve_authorized_workspace` — NEVER a caller path).
///   It becomes the overlay RO LOWER (mounted read-only in SLICE 3).
/// - `ws_slug` + `session_id` key the persistent layer (both re-asserted safe).
/// - `session_id` is the HOST-DECIDED id (a fresh mint for new/fork, or the
///   validated addressed id) — it is FORCED into the spawn so the returned /
///   addressable `sessionId` equals the layer key (resume can re-find it).
/// - `cwd` = [`GUEST_WORKSPACE_MOUNT`] (`/workspace`) — the guest mount point,
///   never `$HOME`, never a raw host path (the `$HOME`-narrowing invariant is
///   preserved + debug-asserted).
/// - FAIL-CLOSED: a layer-provision failure returns `Err` → the route 5xxs;
///   we NEVER fall back to `$HOME` or an ephemeral dir. `ephemeral_cwd` is left
///   `None` so the child-exit observer does NOT delete the layer (persistence).
pub fn resolve_workspace_session(
    ws_path: &str,
    ws_slug: &str,
    session_id: &SessionId,
    principal: &V1Principal,
    req: &ApiSandboxRequest,
) -> Result<SpawnRequest, PolicyError> {
    // (1) Provision (or reuse) the persistent per-session writable layer.
    //     Fail-CLOSED: no `$HOME`/ephemeral fallback.
    let layer = provision_persistent_layer(ws_slug, &session_id.to_string())?;

    // (2) cwd = the GUEST mount point the overlay lands at (SLICE 3), NEVER
    //     `$HOME`, NEVER a raw host path. Preserve the $HOME-narrowing invariant.
    let cwd = GUEST_WORKSPACE_MOUNT.to_string();
    debug_assert!(
        dirs::home_dir().map(|h| h.to_string_lossy().into_owned()) != Some(cwd.clone()),
        "workspace-scoped cwd must NEVER be $HOME",
    );

    // (3) Read the per-workspace FS mode (default 'overlay'; fail-safe).
    let fs_mode = k2_core::workspace::settings::get_workspace_fs_mode(ws_path);

    // (4) Host-minted agent name (same anti-hijack namespace as `resolve_spawn`).
    let agent_name = format!(
        "api-{}-{}",
        sanitize_id(&principal.display_id()),
        uuid::Uuid::new_v4()
    );

    // (5) Host-curated env (shared with the ephemeral door).
    let env = build_cell_env(principal, req);

    let cols = clamp_dim(req.cols, 80, 16, 500);
    let rows = clamp_dim(req.rows, 24, 4, 300);

    // (6) The workspace-scoped overlay mount spec — carried to the worker
    //     (SLICE 3). RO base = the canonical workspace; upper/work = the
    //     persistent layer; mode = the per-workspace setting.
    let overlay = OverlaySpec {
        workspace_ro_base: PathBuf::from(ws_path),
        overlay_upper: layer.upper,
        overlay_work: layer.work,
        fs_mode,
    };

    log_debug!(
        "[v1-sandbox/ws] resolved workspace session id={} ws_slug={} ws_path={} fs_mode={} cwd={} (RO base + persistent upper/work; worker mount is SLICE 3)",
        session_id,
        ws_slug,
        ws_path,
        fs_mode.as_flag(),
        cwd,
    );

    Ok(SpawnRequest {
        agent_name,
        cwd,
        command: Some("claude".to_string()),
        args: Some(STANDARD_CLAUDE_ARGS.iter().map(|s| s.to_string()).collect()),
        cols,
        rows,
        env: Some(env),
        label: None,
        label_locked: None,
        sandbox: Some(true),
        // PERSISTENT layer: NOT ephemeral. Leave `ephemeral_cwd = None` so the
        // child-exit observer NEVER deletes the layer (PRD §C — resume needs it).
        ephemeral_cwd: None,
        principal_key: None,
        overlay: Some(overlay),
        // FORCE the host-decided session id so the returned/addressable id
        // equals the persistent-layer key.
        forced_session_id: Some(*session_id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A principal carrying an Anthropic key → it is staged under
    /// `ANTHROPIC_API_KEY`, the caller's env is dropped, the command is the
    /// host-fixed `claude`, and the cwd is a fresh ephemeral dir (NEVER `$HOME`).
    #[test]
    fn resolve_stages_principal_key_and_drops_caller_inputs() {
        let principal = V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: "key-uuid-1".to_string(),
            anthropic_key: Some("sk-ant-principal-key".to_string()),
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
        });
        let req = ApiSandboxRequest {
            prompt: Some("ignored prompt".to_string()),
            cols: Some(120),
            rows: Some(40),
        };

        let spawn = resolve_spawn(&principal, &req).expect("resolve must succeed");
        // Clean up the provisioned ephemeral dir.
        if let Some(d) = spawn.ephemeral_cwd.as_ref() {
            let _ = std::fs::remove_dir_all(d);
        }

        // sandbox FORCED on.
        assert_eq!(spawn.sandbox, Some(true));
        // command is host-fixed claude; args are exactly the standard headless set.
        assert_eq!(spawn.command.as_deref(), Some("claude"));
        assert_eq!(
            spawn.args.as_deref(),
            Some(&["--dangerously-skip-permissions".to_string()][..])
        );
        // env carries the host-staged principal key (caller env dropped) PLUS the
        // F2 cell-recipe defaults. No caller-supplied env ever appears.
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-ant-principal-key"),
        );
        // F2 cell-recipe is staged.
        assert_eq!(env.get("IS_SANDBOX").map(String::as_str), Some("1"));
        assert_eq!(env.get("CLAUDE_CODE_TMPDIR").map(String::as_str), Some("/dev/shm/cc"));
        assert_eq!(env.get("GIT_AUTHOR_NAME").map(String::as_str), Some("K2 Sandbox"));
        assert_eq!(env.get("GIT_COMMITTER_NAME").map(String::as_str), Some("K2 Sandbox"));
        assert_eq!(env.get("GIT_AUTHOR_EMAIL").map(String::as_str), Some("sandbox@k2.local"));
        assert_eq!(env.get("GIT_COMMITTER_EMAIL").map(String::as_str), Some("sandbox@k2.local"));
        // F2: a non-empty prompt is UN-DROPPED into K2_REQUEST_PROMPT (not argv).
        assert_eq!(
            env.get("K2_REQUEST_PROMPT").map(String::as_str),
            Some("ignored prompt"),
            "non-empty prompt is staged verbatim into K2_REQUEST_PROMPT",
        );
        // Exactly the host-curated set: principal key + 6 recipe vars + prompt.
        assert_eq!(env.len(), 8, "env must be host-curated ONLY (no caller env)");
        // cwd is a fresh ephemeral dir, NEVER $HOME.
        assert_ne!(
            Some(PathBuf::from(&spawn.cwd)),
            dirs::home_dir(),
            "ephemeral cwd must never be $HOME",
        );
        assert!(
            spawn.cwd.contains("sandbox-sessions"),
            "cwd must be under the ephemeral sandbox root: {}",
            spawn.cwd
        );
        // agent_name is host-namespaced — can't collide with tab-/pinned sessions.
        assert!(
            spawn.agent_name.starts_with("api-"),
            "agent_name must be host-namespaced: {}",
            spawn.agent_name
        );
        // dims honored (within clamp).
        assert_eq!(spawn.cols, 120);
        assert_eq!(spawn.rows, 40);
        // ephemeral_cwd is set for teardown and equals the cwd.
        assert_eq!(
            spawn.ephemeral_cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
            Some(spawn.cwd.clone()),
        );
    }

    /// A principal with NO Anthropic key (e.g. owner own-use) → no key staged,
    /// env is empty, and the cwd is still a fresh ephemeral dir.
    #[test]
    fn resolve_stages_no_key_for_keyless_principal() {
        let principal = V1Principal::Owner;
        let req = ApiSandboxRequest::default();
        let spawn = resolve_spawn(&principal, &req).expect("resolve must succeed");
        if let Some(d) = spawn.ephemeral_cwd.as_ref() {
            let _ = std::fs::remove_dir_all(d);
        }

        let env = spawn.env.as_ref().expect("env present");
        assert!(
            env.get("ANTHROPIC_API_KEY").is_none(),
            "a keyless principal stages NO Anthropic key",
        );
        // F2: no key, but the cell-recipe defaults are still staged (6 vars).
        assert_eq!(env.get("IS_SANDBOX").map(String::as_str), Some("1"));
        assert_eq!(env.get("CLAUDE_CODE_TMPDIR").map(String::as_str), Some("/dev/shm/cc"));
        assert_eq!(env.len(), 6, "keyless principal → exactly the 6 recipe vars");
        // Default request (no prompt) stages NO K2_REQUEST_PROMPT.
        assert!(
            env.get("K2_REQUEST_PROMPT").is_none(),
            "empty prompt must not stage K2_REQUEST_PROMPT (behavior unchanged)",
        );
        assert_ne!(Some(PathBuf::from(&spawn.cwd)), dirs::home_dir());
        assert_eq!(spawn.command.as_deref(), Some("claude"));
    }

    /// A blank Anthropic key is treated as absent (never an empty assignment).
    #[test]
    fn resolve_drops_blank_principal_key() {
        let principal = V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: "key-uuid-2".to_string(),
            anthropic_key: Some("   ".to_string()),
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
        });
        let spawn = resolve_spawn(&principal, &ApiSandboxRequest::default())
            .expect("resolve must succeed");
        if let Some(d) = spawn.ephemeral_cwd.as_ref() {
            let _ = std::fs::remove_dir_all(d);
        }
        assert!(
            spawn.env.as_ref().unwrap().get("ANTHROPIC_API_KEY").is_none(),
            "a blank key must never produce an ANTHROPIC_API_KEY entry",
        );
    }

    /// Two resolves yield DISTINCT ephemeral dirs + distinct host-minted names.
    #[test]
    fn resolve_mints_distinct_workspaces_and_names() {
        let p = V1Principal::Owner;
        let a = resolve_spawn(&p, &ApiSandboxRequest::default()).expect("a");
        let b = resolve_spawn(&p, &ApiSandboxRequest::default()).expect("b");
        for s in [&a, &b] {
            if let Some(d) = s.ephemeral_cwd.as_ref() {
                let _ = std::fs::remove_dir_all(d);
            }
        }
        assert_ne!(a.cwd, b.cwd, "each session gets its own ephemeral workspace");
        assert_ne!(a.agent_name, b.agent_name, "each session gets a unique host-minted name");
    }

    #[test]
    fn clamp_dim_bounds_and_defaults() {
        assert_eq!(clamp_dim(None, 80, 16, 500), 80, "absent → default");
        assert_eq!(clamp_dim(Some(5), 80, 16, 500), 16, "below min → min");
        assert_eq!(clamp_dim(Some(9999), 80, 16, 500), 500, "above max → max");
        assert_eq!(clamp_dim(Some(120), 80, 16, 500), 120, "in range → as-is");
    }

    #[test]
    fn sanitize_id_strips_unsafe_chars() {
        assert_eq!(sanitize_id("owner"), "owner");
        assert_eq!(sanitize_id("abc-123_DEF"), "abc-123_DEF");
        assert_eq!(sanitize_id("a/b c"), "a-b-c");
        assert_eq!(sanitize_id(""), "anon");
    }

    // ── Sandbox v2 (PRD §B/§C/§G2) — WORKSPACE-SCOPED provisioning ─────────

    use k2_core::terminal::sandbox::FsMode;

    /// Best-effort cleanup of a provisioned persistent layer's workspace dir.
    fn cleanup_overlay_ws(ws_slug: &str) {
        let _ = std::fs::remove_dir_all(sandbox_overlays_root().join(ws_slug));
    }

    /// PATH-ESCAPE REJECTION (load-bearing): the segment re-assert rejects every
    /// traversal / separator / control token, so a path built from a segment can
    /// never escape `~/.k2/sandbox-overlays/`. A benign name passes.
    #[test]
    fn segment_reassert_rejects_escapes() {
        assert!(assert_safe_path_segment("ai").is_ok());
        assert!(assert_safe_path_segment("a-b_9").is_ok());
        for bad in ["", "..", ".", "a..b", "a/b", "a\\b", "a\0b", "\n", "../etc"] {
            assert!(
                assert_safe_path_segment(bad).is_err(),
                "must reject unsafe segment {bad:?}",
            );
        }
    }

    /// A crafted unsafe slug / session id can NEVER escape the overlays root:
    /// `provision_persistent_layer` fails-closed (NoPersistentLayer) rather than
    /// building an escaping path.
    #[test]
    fn provision_persistent_layer_rejects_escape_attempts() {
        assert!(matches!(
            provision_persistent_layer("../etc", "sid"),
            Err(PolicyError::NoPersistentLayer(_)),
        ));
        assert!(matches!(
            provision_persistent_layer("ws", "../../root"),
            Err(PolicyError::NoPersistentLayer(_)),
        ));
        assert!(matches!(
            provision_persistent_layer("a/b", "sid"),
            Err(PolicyError::NoPersistentLayer(_)),
        ));
    }

    /// The layer is created idempotently: upper + work dirs exist UNDER the
    /// overlays root, are 0700, and a second call on the same key succeeds
    /// (re-provision / resume) without error.
    #[test]
    fn provision_persistent_layer_creates_0700_dirs_idempotently() {
        let ws = format!("v1ws-layer-{}", uuid::Uuid::new_v4());
        let sid = uuid::Uuid::new_v4().to_string();

        let layer = provision_persistent_layer(&ws, &sid).expect("provision");
        // Both dirs exist and are contained under the overlays root.
        assert!(layer.upper.is_dir(), "upper must exist");
        assert!(layer.work.is_dir(), "work must exist");
        assert!(layer.upper.starts_with(sandbox_overlays_root()));
        assert!(layer.upper.ends_with("upper"));
        assert!(layer.work.ends_with("work"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for d in [&layer.upper, &layer.work] {
                let mode = std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o700, "{} must be 0700, got {:o}", d.display(), mode);
            }
        }
        // Idempotent: a second provision on the same key succeeds + returns the
        // same paths (resume re-uses the layer, never errors).
        let again = provision_persistent_layer(&ws, &sid).expect("re-provision");
        assert_eq!(again.upper, layer.upper);
        assert_eq!(again.work, layer.work);

        cleanup_overlay_ws(&ws);
    }

    /// FS-MODE read + default: an unregistered/NULL workspace reads the PRD
    /// default `overlay`; a workspace with `sandbox_fs_mode='ro+scratch'` reads
    /// `RoScratch`.
    #[test]
    fn fs_mode_default_overlay_and_ro_scratch_read() {
        // Unknown path → default overlay.
        assert_eq!(
            k2_core::workspace::settings::get_workspace_fs_mode("/tmp/k2-v1ws-fsmode-none"),
            FsMode::Overlay,
        );
        // Registered workspace with ro+scratch set → RoScratch.
        let path = "/tmp/k2-v1ws-fsmode-ro";
        let id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, sandbox_fs_mode) VALUES (?1, ?2, ?3, 'ro+scratch')",
                rusqlite::params![id, "v1ws-fsmode-ro", path],
            )
            .expect("insert project");
        }
        assert_eq!(
            k2_core::workspace::settings::get_workspace_fs_mode(path),
            FsMode::RoScratch,
        );
    }

    /// `resolve_workspace_session` produces a host-trusted spec that carries the
    /// RIGHT fields: RO base = the workspace path, upper/work under the overlays
    /// root, fs_mode from the setting, cwd = `/workspace` (never $HOME), a FORCED
    /// session id equal to the layer key, sandbox forced on, headless claude, and
    /// `ephemeral_cwd = None` (the layer PERSISTS — never torn down on exit).
    #[test]
    fn resolve_workspace_session_carries_overlay_spec() {
        let ws_slug = format!("v1ws-resolve-{}", uuid::Uuid::new_v4());
        let ws_path = "/tmp/k2-v1ws-resolve-base";
        let sid = SessionId::new();

        let spawn = resolve_workspace_session(
            ws_path,
            &ws_slug,
            &sid,
            &V1Principal::Owner,
            &ApiSandboxRequest::default(),
        )
        .expect("resolve workspace session");

        // cwd = the guest mount point, NEVER $HOME / a host path.
        assert_eq!(spawn.cwd, "/workspace");
        assert_ne!(Some(PathBuf::from(&spawn.cwd)), dirs::home_dir());
        // sandbox forced on; host-fixed headless claude.
        assert_eq!(spawn.sandbox, Some(true));
        assert_eq!(spawn.command.as_deref(), Some("claude"));
        // FORCED session id == the layer key (so the returned id can be resumed).
        assert_eq!(spawn.forced_session_id, Some(sid));
        // PERSISTENT: no ephemeral teardown handle.
        assert!(spawn.ephemeral_cwd.is_none(), "persistent layer must NOT be torn down");
        // The overlay spec carries the RO base + the persistent upper/work under
        // the overlays root, with the (default) overlay fs mode.
        let ov = spawn.overlay.as_ref().expect("overlay spec present");
        assert_eq!(ov.workspace_ro_base, PathBuf::from(ws_path));
        assert!(ov.overlay_upper.starts_with(sandbox_overlays_root()));
        assert!(ov.overlay_upper.to_string_lossy().contains(&ws_slug));
        assert!(ov.overlay_upper.to_string_lossy().contains(&sid.to_string()));
        assert!(ov.overlay_upper.ends_with("upper"));
        assert!(ov.overlay_work.ends_with("work"));
        assert_eq!(ov.fs_mode, FsMode::Overlay, "unset workspace → default overlay");
        // agent name is host-namespaced (anti-hijack).
        assert!(spawn.agent_name.starts_with("api-"));

        cleanup_overlay_ws(&ws_slug);
    }
}
