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
//! - `command`/`args` = HOST-RESOLVED, never the wire. The caller's
//!   command/args/flags are DROPPED ENTIRELY (no RCE from the wire). The
//!   ephemeral door ([`resolve_spawn`]) fixes `claude` + the standard headless
//!   flags (no workspace → nothing to resolve against); the workspace door
//!   ([`resolve_workspace_session`]) resolves the WORKSPACE'S CONFIGURED agent
//!   via the de-generalization seam ([`k2_core::workspace::agent_resolve`]).
//! - `env` = HOST-CURATED ONLY. The caller's env is DROPPED ENTIRELY. The
//!   PRINCIPAL's stored LLM credential is staged (never the body) under the
//!   env var(s) its `provider` metadata maps to (W5, migration 0071 —
//!   [`k2_core::api_keys::ApiPrincipal::staged_env_pairs`]; NULL provider =
//!   anthropic → ANTHROPIC_API_KEY, the original behavior; unknown provider
//!   → NOTHING, fail-closed).

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::sandbox::WorkspaceMountSpec;

use crate::routes::http::V1Principal;
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

/// The standard headless claude flags for an EPHEMERAL API sandbox session
/// ([`resolve_spawn`] only). The microVM jail — NOT claude's permission
/// prompts — is the security boundary, so the in-jail agent runs headless
/// (mirrors the pinned-chat base flags). The caller CANNOT add to or override
/// these; `spawn_session` additionally auto-injects a fresh `--session-id` for
/// conversation persistence.
///
/// The workspace-scoped door does NOT use this constant — it resolves the
/// workspace's configured agent (see [`resolve_workspace_session`]). The
/// ephemeral door keeps the claude default because there is NO workspace
/// context: `POST /v1/sandboxes` provisions a throwaway dir, so there is no
/// `projects` row (and thus no configured preset) to consult.
const STANDARD_CLAUDE_ARGS: &[&str] = &["--dangerously-skip-permissions"];

/// Claude's auto-approve flag — ENSURED (never stripped) on the resolved
/// preset args for a claude cell, so a user-customized claude preset that
/// dropped it still runs headless inside the jail. Kept in sync with
/// `k2_core::workspace::agent_resolve::FALLBACK_AGENT_ARGS`.
const CLAUDE_SKIP_PERMISSIONS_FLAG: &str = "--dangerously-skip-permissions";

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
pub(crate) fn resolve_spawn(
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
        // DROP any caller command/args/flags — host-fixed claude + headless
        // args. The EPHEMERAL door has no workspace context (throwaway dir, no
        // `projects` row), so there is no configured preset to resolve — the
        // literal claude default stays (see STANDARD_CLAUDE_ARGS).
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
        quota_workspace: None,
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

/// Build the HOST-CURATED cell env shared by BOTH sandbox doors (ephemeral
/// `resolve_spawn` and workspace-scoped `resolve_workspace_session`) so the two
/// curate an IDENTICAL environment. The caller's env is DROPPED ENTIRELY; the
/// Anthropic key is staged from the validated PRINCIPAL (never the body), the
/// F2 cell-recipe defaults are seeded, and a non-empty prompt is staged
/// (un-logged) into `K2_REQUEST_PROMPT`. Neither the key nor the prompt VALUE is
/// ever logged.
fn build_cell_env(principal: &V1Principal, req: &ApiSandboxRequest) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = HashMap::new();
    // LLM credential — from the PRINCIPAL only, staged under the env var(s)
    // its `provider` metadata maps to (W5, migration 0071 —
    // `ApiPrincipal::staged_env_pairs`: NULL provider = anthropic →
    // ANTHROPIC_API_KEY exactly as before; unknown provider → NOTHING,
    // fail-closed). Env VALUES are never logged — only var NAMES.
    match principal {
        V1Principal::Api(p) => {
            let staged = p.staged_env_pairs();
            if staged.is_empty() {
                // No usable credential OR unknown provider (the seam already
                // logged the structured unknown-provider warning).
                log_debug!(
                    "[v1-sandbox] principal={} staged no LLM credential; cell will have no model credential",
                    p.id
                );
            } else {
                let names: Vec<&str> = staged.iter().map(|(k, _)| k.as_str()).collect();
                log_debug!(
                    "[v1-sandbox] staged principal LLM credential into session env vars {:?} (principal={})",
                    names,
                    p.id
                );
                for (k, v) in staged {
                    env.insert(k, v);
                }
            }
        }
        V1Principal::Owner => {
            log_debug!(
                "[v1-sandbox] owner principal: no app-level LLM key fallback (follow-up); none staged"
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

    // Â§5 CONTINUITY: mirror the SAFE subset of the host user's ~/.claude/settings.json
    // (model/effort/tui/theme/â¦) into the cell so an API-launched agent behaves like the
    // workspace's human sessions. Excludes host-specific/dangerous fields
    // (hooks/permissions/mcpServers/apiKeyHelper/env). Guest-init writes it to
    // $HOME/.claude/settings.json (fallback: a minimal default).
    env.insert(
        "K2_SANDBOX_SETTINGS_JSON".to_string(),
        continuity_settings_json(),
    );
    env
}

/// Build the cell's settings.json: the SAFE continuity subset of the host user's
/// ~/.claude/settings.json + a forced `skipDangerousModePermissionPrompt` (the
/// jail is the security boundary, NOT claude's own prompt). ALLOWLIST, so a
/// dangerous/unknown field can never leak into the cell.
fn continuity_settings_json() -> String {
    const ALLOW: &[&str] = &[
        "theme", "tui", "model", "effortLevel", "outputStyle", "verbose",
        "spinnerTipsEnabled", "autoCompactEnabled", "includeCoAuthoredBy",
        "messageIdleNotifThresholdMs", "todoFeatureEnabled", "alwaysThinkingEnabled",
    ];
    let mut out = serde_json::Map::new();
    if let Some(home) = dirs::home_dir() {
        if let Ok(txt) = std::fs::read_to_string(home.join(".claude").join("settings.json")) {
            if let Ok(serde_json::Value::Object(m)) =
                serde_json::from_str::<serde_json::Value>(&txt)
            {
                for k in ALLOW {
                    if let Some(v) = m.get(*k) {
                        out.insert((*k).to_string(), v.clone());
                    }
                }
            }
        }
    }
    if !out.contains_key("theme") {
        out.insert("theme".to_string(), serde_json::json!("dark"));
    }
    out.insert(
        "skipDangerousModePermissionPrompt".to_string(),
        serde_json::json!(true),
    );
    serde_json::Value::Object(out).to_string()
}

/// The root under which per-(workspace, session) PERSISTENT `/work` scratch
/// layers live: `~/.k2/sandbox-overlays`. Derived ONLY from the daemon's home
/// dir (never a caller path), mirroring [`sandbox_sessions_root`].
fn sandbox_overlays_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("sandbox-overlays")
}

/// The root under which per-WORKSPACE sandbox homes live:
/// `~/.k2/sandbox-homes`. Each `<ws>/.claude` accumulates that workspace's
/// sandbox session transcripts + memory (mounted at `<home>/.claude` in every
/// cell for that workspace). Derived ONLY from the daemon's home dir.
fn sandbox_homes_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("sandbox-homes")
}

/// The daemon's real home dir = the in-cell `HOME` for a mirror session (e.g.
/// `/home/k2` on the sandbox box). Everything mirrors the daemon's filesystem:
/// the sandbox home mounts at `<home>/.claude`, the canonical memory at
/// `<home>/.claude/projects/<slug>/memory`.
fn incell_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/k2"))
}

/// Compute the claude project SLUG for an in-cell cwd: every `/` → `-` (e.g.
/// `/home/k2/ai` → `-home-k2-ai`). This is exactly the layout the PRD locks and
/// the key under which claude stores `projects/<slug>/{memory,<session>.jsonl}`.
/// The cell's cwd IS the real workspace path, so the slug matches the canonical
/// slug a normal workspace session would use → RO memory + `--resume` resolve
/// the identical real paths.
fn slug_for_cwd(cwd: &str) -> String {
    cwd.replace('/', "-")
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

/// Lexical containment check: assert `leaf` (built from `root` + validated
/// segments) stays UNDER `root` with only `Normal` components beyond it. No
/// `fs::canonicalize` (which would follow a symlink out) — we keep the P4 TOCTOU
/// posture and never follow a symlink into the layer. Combined with the segment
/// re-assert, a constructed path can never escape its root. FAIL-CLOSED.
fn assert_contained(leaf: &std::path::Path, root: &std::path::Path) -> Result<(), PolicyError> {
    if !leaf.starts_with(root) {
        return Err(PolicyError::NoPersistentLayer(format!(
            "constructed path {} escapes root {}",
            leaf.display(),
            root.display()
        )));
    }
    for comp in leaf.strip_prefix(root).unwrap_or(leaf).components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return Err(PolicyError::NoPersistentLayer(format!(
                "constructed path {} contains a non-normal component",
                leaf.display()
            )));
        }
    }
    Ok(())
}

/// Create (idempotently) a dir at 0700, fail-CLOSED on any mkdir/chmod error.
/// The spawn door later chowns the RW mirror dirs to the per-session cell uid
/// (P4-H6); 0700 keeps them host-private atop the pivot_root jail.
fn mkdir_0700(dir: &std::path::Path) -> Result<(), PolicyError> {
    // Resume-safe: on REOPEN the dir already exists owned by the PRIOR session's
    // cell uid (the spawn door chowned it). The daemon has CAP_CHOWN but NOT
    // CAP_FOWNER, so `set_permissions` (chmod) below would EPERM. Reclaim ownership
    // to ourselves first (CAP_CHOWN); the spawn door re-chowns to the NEW cell uid.
    // Best-effort + only reached on the resume path (fresh dirs are already ours).
    #[cfg(unix)]
    if dir.exists() {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) {
            unsafe { libc::chown(c.as_ptr(), libc::geteuid(), libc::getegid()); }
        }
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| PolicyError::NoPersistentLayer(format!("mkdir {}: {e}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            PolicyError::NoPersistentLayer(format!("chmod 0700 {}: {e}", dir.display()))
        })?;
    }
    Ok(())
}

/// Provision (idempotently) the per-WORKSPACE sandbox home `.claude` dir at
/// `~/.k2/sandbox-homes/<ws_slug>/.claude` and return it. Created ONCE per
/// workspace and REUSED across that workspace's sessions (it accumulates every
/// session's `.jsonl` + memory so they list together under the real slug).
/// Security: built ONLY from the daemon HOME + the RE-ASSERTED `ws_slug`
/// segment; containment-checked; 0700; chowned to the cell uid at the spawn
/// door. FAIL-CLOSED.
fn provision_sandbox_home(ws_slug: &str, session_id: &str) -> Result<PathBuf, PolicyError> {
    // PER-SESSION home: `~/.k2/sandbox-homes/<ws>/<sid>/.claude`. Per-session
    // (NOT per-workspace-shared) so each cell's 0700 dir is owned by ITS uid with
    // no reuse conflict: a shared home gets chowned to the FIRST session's cell
    // uid at the spawn door, and the daemon (no CAP_FOWNER) then can't re-chmod it
    // for the next session (`chmod: Operation not permitted`). Cross-session
    // listing/audit is via the `sandbox_sessions` DB index, not a shared `.claude`.
    assert_safe_path_segment(ws_slug)?;
    assert_safe_path_segment(session_id)?;
    let root = sandbox_homes_root();
    let leaf = root.join(ws_slug).join(session_id);
    assert_contained(&leaf, &root)?;
    // `.claude` is a FIXED subcomponent (not a segment) — safe to join.
    let claude = leaf.join(".claude");
    mkdir_0700(&claude)?;
    log_debug!(
        "[v1-sandbox/ws] provisioned per-session sandbox home {} (0700; per-session chown at the spawn door — P4-H6)",
        claude.display()
    );
    Ok(claude)
}

/// OP #4 fork: COW-copy the SOURCE session's persistent layer into the fresh
/// destination session's (already-provisioned, empty) layer, renaming the source
/// `<src>.jsonl` -> `<dst>.jsonl` so `claude --resume <dst>` continues the source's
/// history under the NEW id. The spawn door recursively re-chowns copied files to
/// the new cell uid. FAIL-CLOSED + path-safe (both ids + slug re-asserted).
/// Top-down recursive `chown` of a tree to the DAEMON uid (reclaim), so the daemon
/// can read a layer 0700-owned by a prior cell uid (e.g. to COW-copy it for a fork).
/// Top-down: chown the dir FIRST (CAP_CHOWN needs no read), THEN read_dir + recurse.
fn chown_tree_to_self(path: &std::path::Path) {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
            libc::chown(c.as_ptr(), libc::geteuid(), libc::getegid());
        }
    }
    if path.is_dir() && !path.is_symlink() {
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                chown_tree_to_self(&e.path());
            }
        }
    }
}

pub fn fork_layer(ws_slug: &str, src_id: &str, dst_id: &str) -> Result<(), PolicyError> {
    assert_safe_path_segment(ws_slug)?;
    assert_safe_path_segment(src_id)?;
    assert_safe_path_segment(dst_id)?;
    let homes = sandbox_homes_root();
    let src_claude = homes.join(ws_slug).join(src_id).join(".claude");
    let dst_claude = homes.join(ws_slug).join(dst_id).join(".claude");
    assert_contained(&src_claude, &homes)?;
    assert_contained(&dst_claude, &homes)?;
    if src_claude.is_dir() {
        chown_tree_to_self(&src_claude);
        let st = std::process::Command::new("cp")
            .arg("-a")
            .arg(format!("{}/.", src_claude.display()))
            .arg(&dst_claude)
            .status()
            .map_err(|e| PolicyError::NoPersistentLayer(format!("fork cp: {e}")))?;
        if !st.success() {
            return Err(PolicyError::NoPersistentLayer("fork cp of source layer failed".into()));
        }
        let projects = dst_claude.join("projects");
        if let Ok(rd) = std::fs::read_dir(&projects) {
            for slug_dir in rd.flatten() {
                let sp = slug_dir.path().join(format!("{src_id}.jsonl"));
                let dp = slug_dir.path().join(format!("{dst_id}.jsonl"));
                if sp.exists() {
                    let _ = std::fs::rename(&sp, &dp);
                }
            }
        }
    }
    let ovr = sandbox_overlays_root();
    let src_work = ovr.join(ws_slug).join(src_id).join("work-scratch");
    let dst_work = ovr.join(ws_slug).join(dst_id).join("work-scratch");
    assert_contained(&dst_work, &ovr)?;
    if src_work.is_dir() {
        chown_tree_to_self(&src_work);
        let _ = std::process::Command::new("cp")
            .arg("-a")
            .arg(format!("{}/.", src_work.display()))
            .arg(&dst_work)
            .status();
    }
    Ok(())
}

/// Provision (idempotently) the per-SESSION `/work` scratch source at
/// `~/.k2/sandbox-overlays/<ws_slug>/<session_id>/work-scratch` and return it.
/// Keyed by `(workspace, session)`, restored on resume (NOT deleted on
/// ChildExit). Security: built ONLY from the daemon HOME + the two RE-ASSERTED
/// segments; containment-checked; 0700; chowned to the cell uid at the spawn
/// door. FAIL-CLOSED.
fn provision_work_scratch(ws_slug: &str, session_id: &str) -> Result<PathBuf, PolicyError> {
    assert_safe_path_segment(ws_slug)?;
    assert_safe_path_segment(session_id)?;
    let root = sandbox_overlays_root();
    let session_dir = root.join(ws_slug).join(session_id);
    assert_contained(&session_dir, &root)?;
    let work = session_dir.join("work-scratch");
    mkdir_0700(&work)?;
    log_debug!(
        "[v1-sandbox/ws] provisioned per-session /work scratch {} (0700; per-session chown at the spawn door — P4-H6)",
        work.display()
    );
    Ok(work)
}

/// Resolve a WORKSPACE-SCOPED session into a host-trusted [`SpawnRequest`] that
/// carries the **MIRROR** mount spec (fs-mirror PRD, LOCKED): the workspace +
/// canonical memory RO at their REAL paths, the per-workspace sandbox home + the
/// per-session `/work` RW. The workspace analogue of [`resolve_spawn`]:
///
/// - `ws_path` = the HOST-RESOLVED, AUTHORIZED workspace absolute path (a
///   `projects.path` from `resolve_authorized_workspace` — NEVER a caller path).
///   It is BOTH the RO workspace mirror source AND the in-cell cwd (mirror).
/// - `ws_slug` (+ `session_id`) key the RW dirs (both re-asserted safe).
/// - `session_id` is the HOST-DECIDED id — FORCED into the spawn AND staged as
///   `K2_SESSION_ID` so the guest-init can splice the provider's session flag
///   (`claude --session-id <it>`); the returned/addressable `sessionId`
///   therefore equals the `.jsonl` key → resume + audit resolve the real path.
/// - `command`/`args` = the workspace's CONFIGURED agent (agent_resolve seam),
///   auto-approved inside the jail — see step (6b). NEVER a caller command.
/// - `cwd` = `ws_path` (the REAL mirrored path). It is a SUBDIR of the in-cell
///   HOME (`/home/k2`), never `== $HOME`, never a caller path (the `$HOME`-
///   narrowing invariant is preserved + debug-asserted).
/// - FAIL-CLOSED: a provisioning failure returns `Err` → the route 5xxs; we
///   NEVER fall back to `$HOME` or an ephemeral dir. `ephemeral_cwd` is left
///   `None` so the child-exit observer does NOT delete the RW dirs (persistence).
pub(crate) fn resolve_workspace_session(
    ws_path: &str,
    ws_slug: &str,
    session_id: &SessionId,
    principal: &V1Principal,
    req: &ApiSandboxRequest,
) -> Result<SpawnRequest, PolicyError> {
    let sid = session_id.to_string();

    // (1) cwd = the REAL workspace path (the mirror premise) — the cell mounts
    //     the workspace RO at this same path, so paths are exactly where the
    //     agent expects. It is a SUBDIR of the in-cell HOME, NEVER == HOME.
    let cwd = ws_path.to_string();
    let home = incell_home();
    let home_str = home.to_string_lossy().into_owned();
    debug_assert!(
        cwd != home_str,
        "workspace-scoped cwd must be a SUBDIR of the in-cell HOME, never == HOME",
    );

    // (2) The claude project SLUG from the (real) cwd — keys memory + the `.jsonl`.
    let slug = slug_for_cwd(&cwd);

    // (3) Provision the RW mirror dirs (fail-CLOSED; no $HOME/ephemeral fallback):
    //     (a) the per-WORKSPACE sandbox home (reused across the ws's sessions),
    //     (b) the per-SESSION `/work` scratch (persistent, restored on resume).
    let sandbox_home_rw = provision_sandbox_home(ws_slug, &sid)?;
    let work_rw = provision_work_scratch(ws_slug, &sid)?;

    // (4) The RO canonical-memory bind source = the REAL memory path
    //     `<home>/.claude/projects/<slug>/memory`. OPTIONAL: bound only when it
    //     already exists (a fresh workspace has none) — absence is NOT an error.
    let memory_src = home
        .join(".claude")
        .join("projects")
        .join(&slug)
        .join("memory");
    let canonical_memory_ro = if memory_src.is_dir() {
        Some(memory_src)
    } else {
        None
    };

    // (5) Host-minted agent name (same anti-hijack namespace as `resolve_spawn`).
    let agent_name = format!(
        "api-{}-{}",
        sanitize_id(&principal.display_id()),
        uuid::Uuid::new_v4()
    );

    // (6) Host-curated env + the mirror/guest-init wiring. HOME = the real
    //     in-cell home (so `claude` uses `<home>/.claude` — the mounted sandbox
    //     home — natively; CLAUDE_CONFIG_DIR is intentionally NOT set). The
    //     `K2_*_DIR` vars tell the guest-init the REAL in-cell mount points;
    //     the guest-init execs the resolved agent argv (splicing
    //     `--session-id "$K2_SESSION_ID"` for claude).
    let mut env = build_cell_env(principal, req);

    // Audit index (fs-mirror §5): write a daemon-owned meta.json so the cockpit's
    // Sandboxed-chats panel can list this session by title/time WITHOUT reading the
    // cell-uid-owned 0700 transcript. First-spawn wins (resume preserves the title).
    {
        let title = req.prompt.as_deref().unwrap_or("").trim().to_string();
        let created_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        crate::sandbox_chat_routes::write_meta(ws_slug, &session_id.to_string(), &title, created_ms);
    }
    env.insert("HOME".to_string(), home_str.clone());
    env.insert("K2_SESSION_ID".to_string(), sid.clone());
    env.insert("K2_WS_DIR".to_string(), cwd.clone());
    env.insert("K2_HOME_DIR".to_string(), home_str.clone());
    if let Some(mem) = canonical_memory_ro.as_ref() {
        env.insert(
            "K2_MEM_DIR".to_string(),
            mem.to_string_lossy().into_owned(),
        );
    }

    let cols = clamp_dim(req.cols, 80, 16, 500);
    let rows = clamp_dim(req.rows, 24, 4, 300);

    // (6b) The WORKSPACE'S CONFIGURED agent command — the de-generalization
    // seam (`projects.default_agent` → global default → literal claude), the
    // same resolution the host-sessions door uses. The caller has NO say in
    // the command (any body command/args were dropped at parse time).
    //
    // CELL PHILOSOPHY (unlike the host door): the microVM jail IS the
    // security boundary, so the in-cell agent runs AUTO-APPROVED. For claude
    // we ENSURE `--dangerously-skip-permissions` (the built-in preset already
    // carries it → byte-identical argv to the pre-resolution hardcode; a
    // user-customized preset that dropped it gets it back). Other presets
    // spawn their own command+args exactly as configured — their auto-approve
    // flags are already in the preset args (e.g. `gemini --yolo`,
    // `codex … --dangerously-bypass-approvals-and-sandbox`); we never strip
    // and never invent flags for providers we don't own.
    //
    // Session identity stays ENV-carried (`K2_SESSION_ID`, spliced by the
    // guest-init for claude argv) — never argv here, so non-claude presets
    // run bare-but-correct (degraded resume) until their in-cell resume
    // grammar exists.
    let resolved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_resolve::resolve_agent_command(&conn, ws_path)
    };
    let command = resolved.command.clone();
    let mut args = resolved.args.clone();
    if resolved.is_claude() {
        k2_core::workspace::agent_resolve::ensure_flag(&mut args, CLAUDE_SKIP_PERMISSIONS_FLAG);
    }

    // (7) The workspace-scoped MIRROR mount spec — carried to the worker.
    let overlay = WorkspaceMountSpec {
        workspace_ro: PathBuf::from(ws_path),
        sandbox_home_rw,
        canonical_memory_ro,
        work_rw,
        home,
        slug: slug.clone(),
    };

    log_debug!(
        "[v1-sandbox/ws] resolved MIRROR session id={} ws_slug={} cwd={} slug={} home={} memory={} command={} args={:?} (workspace+memory RO, sandbox-home+/work RW)",
        session_id,
        ws_slug,
        cwd,
        slug,
        home_str,
        overlay
            .canonical_memory_ro
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<absent>".to_string()),
        command,
        args,
    );

    Ok(SpawnRequest {
        agent_name,
        cwd,
        // The workspace's CONFIGURED agent (6b) — auto-approved inside the
        // jail; NEVER a caller command.
        command: Some(command),
        args: Some(args),
        cols,
        rows,
        env: Some(env),
        label: None,
        label_locked: None,
        sandbox: Some(true),
        // PERSISTENT: NOT ephemeral. Leave `ephemeral_cwd = None` so the
        // child-exit observer NEVER deletes the RW dirs (resume needs them).
        ephemeral_cwd: None,
        principal_key: None,
        quota_workspace: None,
        overlay: Some(overlay),
        // FORCE the host-decided session id so the returned/addressable id
        // equals the persistent-layer + `.jsonl` key.
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
            provider: None,
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
            capabilities: k2_core::api_keys::ApiCapabilities::all(),
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
        // Exactly the host-curated set: principal key + 6 recipe vars + prompt +
        // the continuity settings.json (§5 — always staged, mirrors the workspace).
        assert!(
            env.get("K2_SANDBOX_SETTINGS_JSON").is_some(),
            "continuity settings.json is always staged into the cell env",
        );
        assert_eq!(env.len(), 9, "env must be host-curated ONLY (no caller env)");
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
        // 6 recipe vars + the continuity settings.json (always staged, §5).
        assert!(
            env.get("K2_SANDBOX_SETTINGS_JSON").is_some(),
            "continuity settings.json is always staged into the cell env",
        );
        assert_eq!(env.len(), 7, "keyless → 6 recipe vars + continuity settings");
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
            provider: None,
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
            capabilities: k2_core::api_keys::ApiCapabilities::all(),
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

    /// W5 — an openai-provider principal stages OPENAI_API_KEY (+ base URL)
    /// into the cell env instead of ANTHROPIC_API_KEY; an unknown provider
    /// stages NOTHING (fail-closed) while the recipe defaults survive.
    #[test]
    fn resolve_stages_provider_mapped_key() {
        let principal = V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: "key-uuid-oai".to_string(),
            anthropic_key: Some("sk-oai-cell-key".to_string()),
            provider: Some("openai".to_string()),
            base_url: Some("https://oai-proxy.example/v1".to_string()),
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
            capabilities: k2_core::api_keys::ApiCapabilities::all(),
        });
        let spawn = resolve_spawn(&principal, &ApiSandboxRequest::default())
            .expect("resolve must succeed");
        if let Some(d) = spawn.ephemeral_cwd.as_ref() {
            let _ = std::fs::remove_dir_all(d);
        }
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(env.get("OPENAI_API_KEY").map(String::as_str), Some("sk-oai-cell-key"));
        assert_eq!(
            env.get("OPENAI_BASE_URL").map(String::as_str),
            Some("https://oai-proxy.example/v1"),
        );
        assert!(
            env.get("ANTHROPIC_API_KEY").is_none(),
            "an openai key must not be staged as an Anthropic credential",
        );

        // Unknown provider → NOTHING staged (fail-closed); the 6 recipe vars
        // + continuity settings remain (same census as the keyless test).
        let principal = V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: "key-uuid-unk".to_string(),
            anthropic_key: Some("sk-mystery".to_string()),
            provider: Some("mystery-llm".to_string()),
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
            capabilities: k2_core::api_keys::ApiCapabilities::all(),
        });
        let spawn = resolve_spawn(&principal, &ApiSandboxRequest::default())
            .expect("resolve must succeed");
        if let Some(d) = spawn.ephemeral_cwd.as_ref() {
            let _ = std::fs::remove_dir_all(d);
        }
        let env = spawn.env.as_ref().expect("env present");
        assert!(
            !env.keys().any(|k| k.contains("API_KEY")),
            "unknown provider must stage NO credential var; env keys: {:?}",
            env.keys().collect::<Vec<_>>(),
        );
        assert_eq!(env.len(), 7, "unknown-provider env census = keyless census");
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

    // ── Sandbox v2 (fs-mirror PRD) — WORKSPACE-SCOPED MIRROR provisioning ──

    /// Best-effort cleanup of a provisioned workspace's RW dirs (both roots).
    fn cleanup_mirror_ws(ws_slug: &str) {
        let _ = std::fs::remove_dir_all(sandbox_overlays_root().join(ws_slug));
        let _ = std::fs::remove_dir_all(sandbox_homes_root().join(ws_slug));
    }

    /// Built-in preset ids (seeded by `seed_agent_presets` — k2-core db/mod.rs).
    const CLAUDE_PRESET_ID: &str = "b0a1c2d3-e4f5-6789-abcd-ef0123456001";
    const CODEX_PRESET_ID: &str = "b0a1c2d3-e4f5-6789-abcd-ef0123456002";
    const GEMINI_PRESET_ID: &str = "b0a1c2d3-e4f5-6789-abcd-ef0123456003";

    /// Register a workspace with an explicit `default_agent` so the resolver
    /// is DETERMINISTIC in tests (never falls through to the dev box's real
    /// global default in `~/.k2/settings.json`).
    fn insert_project_with_agent(name: &str, path: &str, default_agent: &str) {
        k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, default_agent) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), name, path, default_agent],
        )
        .expect("insert project");
    }

    /// SLUG computation: the in-cell cwd with every `/`→`-`, matching claude's
    /// `projects/<slug>` layout so RO memory + `--resume` resolve the real path.
    #[test]
    fn slug_for_cwd_replaces_separators() {
        assert_eq!(slug_for_cwd("/home/k2/ai"), "-home-k2-ai");
        assert_eq!(slug_for_cwd("/home/k2/projects/ai"), "-home-k2-projects-ai");
        assert_eq!(slug_for_cwd("/a"), "-a");
    }

    /// PATH-ESCAPE REJECTION (load-bearing): the segment re-assert rejects every
    /// traversal / separator / control token, so a path built from a segment can
    /// never escape `~/.k2/sandbox-*`. A benign name passes.
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

    /// A crafted unsafe slug / session id can NEVER escape the RW roots: the
    /// mirror provisioners fail-closed (NoPersistentLayer) rather than building
    /// an escaping path.
    #[test]
    fn mirror_provisioners_reject_escape_attempts() {
        assert!(matches!(
            provision_sandbox_home("../etc", "sid"),
            Err(PolicyError::NoPersistentLayer(_)),
        ));
        assert!(matches!(
            provision_work_scratch("ws", "../../root"),
            Err(PolicyError::NoPersistentLayer(_)),
        ));
        assert!(matches!(
            provision_work_scratch("a/b", "sid"),
            Err(PolicyError::NoPersistentLayer(_)),
        ));
        assert!(matches!(
            provision_sandbox_home("a\0b", "sid"),
            Err(PolicyError::NoPersistentLayer(_)),
        ));
    }

    /// The RW dirs are created idempotently, 0700, and UNDER their respective
    /// roots. A second call on the same key succeeds (re-provision / resume).
    #[test]
    fn mirror_provisioners_create_0700_dirs_idempotently() {
        let ws = format!("v1ws-mirror-{}", uuid::Uuid::new_v4());
        let sid = uuid::Uuid::new_v4().to_string();

        let home = provision_sandbox_home(&ws, &sid).expect("provision sandbox home");
        let work = provision_work_scratch(&ws, &sid).expect("provision work");

        assert!(home.is_dir(), "sandbox home must exist");
        assert!(work.is_dir(), "work scratch must exist");
        assert!(home.starts_with(sandbox_homes_root()));
        assert!(home.ends_with(".claude"));
        assert!(work.starts_with(sandbox_overlays_root()));
        assert!(work.to_string_lossy().contains(&ws));
        assert!(work.to_string_lossy().contains(&sid));
        assert!(work.ends_with("work-scratch"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for d in [&home, &work] {
                let mode = std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o700, "{} must be 0700, got {:o}", d.display(), mode);
            }
        }
        // Idempotent: re-provision returns the same paths (resume re-uses them).
        assert_eq!(provision_sandbox_home(&ws, &sid).expect("re-home"), home);
        assert_eq!(provision_work_scratch(&ws, &sid).expect("re-work"), work);

        cleanup_mirror_ws(&ws);
    }

    /// `resolve_workspace_session` produces a host-trusted MIRROR spec: cwd = the
    /// REAL workspace path (never $HOME), workspace_ro == cwd, the RW dirs under
    /// their roots, a FORCED session id, `K2_SESSION_ID`/`HOME`/`K2_WS_DIR` staged
    /// into the env, sandbox forced on, the workspace's configured agent
    /// (claude here — BYTE-IDENTICAL argv to the pre-resolution hardcode),
    /// `ephemeral_cwd = None`.
    #[test]
    fn resolve_workspace_session_carries_mirror_spec() {
        let ws_slug = format!("v1ws-resolve-{}", uuid::Uuid::new_v4());
        // A real workspace path UNDER the in-cell home so cwd is a subdir of HOME.
        let ws_path = incell_home().join(&ws_slug);
        let ws_path_str = ws_path.to_string_lossy().into_owned();
        insert_project_with_agent(&ws_slug, &ws_path_str, CLAUDE_PRESET_ID);
        let sid = SessionId::new();

        let spawn = resolve_workspace_session(
            &ws_path_str,
            &ws_slug,
            &sid,
            &V1Principal::Owner,
            &ApiSandboxRequest::default(),
        )
        .expect("resolve workspace session");

        // cwd = the REAL workspace path (mirror), NEVER == $HOME.
        assert_eq!(spawn.cwd, ws_path_str);
        assert_ne!(Some(PathBuf::from(&spawn.cwd)), dirs::home_dir());
        // sandbox forced on; a claude workspace resolves BYTE-IDENTICAL argv
        // to the old hardcode (command + the standard headless flag, no more,
        // no less — the danger flag is KEPT: the jail is the boundary).
        assert_eq!(spawn.sandbox, Some(true));
        assert_eq!(spawn.command.as_deref(), Some("claude"));
        assert_eq!(
            spawn.args.as_deref(),
            Some(&["--dangerously-skip-permissions".to_string()][..]),
            "claude cell argv must stay byte-identical to the pre-resolution hardcode",
        );
        // FORCED session id == the layer / .jsonl key (so it can be resumed).
        assert_eq!(spawn.forced_session_id, Some(sid));
        // PERSISTENT: no ephemeral teardown handle.
        assert!(spawn.ephemeral_cwd.is_none(), "mirror dirs must NOT be torn down");
        // The MIRROR spec carries the RO workspace (== cwd) + the RW dirs.
        let ov = spawn.overlay.as_ref().expect("mirror spec present");
        assert_eq!(ov.workspace_ro, ws_path);
        assert_eq!(ov.home, incell_home());
        assert_eq!(ov.slug, slug_for_cwd(&ws_path_str));
        assert!(ov.sandbox_home_rw.starts_with(sandbox_homes_root()));
        assert!(ov.sandbox_home_rw.ends_with(".claude"));
        assert!(ov.work_rw.starts_with(sandbox_overlays_root()));
        assert!(ov.work_rw.to_string_lossy().contains(&sid.to_string()));
        assert!(ov.work_rw.ends_with("work-scratch"));
        // A fresh test workspace has no canonical memory → optional bind absent.
        assert!(ov.canonical_memory_ro.is_none(), "no memory dir → skip the bind");
        // The guest-init wiring is staged into the env.
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(env.get("K2_SESSION_ID").map(String::as_str), Some(sid.to_string().as_str()));
        assert_eq!(env.get("K2_WS_DIR").map(String::as_str), Some(ws_path_str.as_str()));
        assert_eq!(
            env.get("HOME").map(String::as_str),
            Some(incell_home().to_string_lossy().as_ref()),
        );
        // agent name is host-namespaced (anti-hijack).
        assert!(spawn.agent_name.starts_with("api-"));

        cleanup_mirror_ws(&ws_slug);
    }

    /// A CODEX workspace resolves its OWN preset argv (de-Claudified): the
    /// built-in codex preset command verbatim, auto-approve flag KEPT (the
    /// jail is the boundary — cells never strip), nothing claude-specific
    /// injected. Env staging (K2_SESSION_ID / K2_REQUEST_PROMPT) unchanged.
    #[test]
    fn workspace_session_resolves_codex_preset_argv() {
        let ws_slug = format!("v1ws-codex-{}", uuid::Uuid::new_v4());
        let ws_path = incell_home().join(&ws_slug);
        let ws_path_str = ws_path.to_string_lossy().into_owned();
        insert_project_with_agent(&ws_slug, &ws_path_str, CODEX_PRESET_ID);
        let sid = SessionId::new();

        let spawn = resolve_workspace_session(
            &ws_path_str,
            &ws_slug,
            &sid,
            &V1Principal::Owner,
            &ApiSandboxRequest { prompt: Some("do the thing".into()), ..Default::default() },
        )
        .expect("resolve workspace session");

        assert_eq!(spawn.command.as_deref(), Some("codex"));
        assert_eq!(
            spawn.args.as_deref(),
            Some(
                &[
                    "-c".to_string(),
                    "model_reasoning_effort=high".to_string(),
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                ][..]
            ),
            "codex cell runs the preset's own argv — auto-approve KEPT, no claude flags",
        );
        // The prompt is still ENV-staged (never argv), same as claude cells.
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(env.get("K2_REQUEST_PROMPT").map(String::as_str), Some("do the thing"));
        assert_eq!(env.get("K2_SESSION_ID").map(String::as_str), Some(sid.to_string().as_str()));
        assert_eq!(spawn.sandbox, Some(true));

        cleanup_mirror_ws(&ws_slug);
    }

    /// A GEMINI workspace keeps its preset's own auto-approve flag (`--yolo`)
    /// — cells NEVER strip danger flags (contrast: the host-sessions door
    /// strips them by default).
    #[test]
    fn workspace_session_resolves_gemini_preset_argv() {
        let ws_slug = format!("v1ws-gemini-{}", uuid::Uuid::new_v4());
        let ws_path = incell_home().join(&ws_slug);
        let ws_path_str = ws_path.to_string_lossy().into_owned();
        insert_project_with_agent(&ws_slug, &ws_path_str, GEMINI_PRESET_ID);

        let spawn = resolve_workspace_session(
            &ws_path_str,
            &ws_slug,
            &SessionId::new(),
            &V1Principal::Owner,
            &ApiSandboxRequest::default(),
        )
        .expect("resolve workspace session");

        assert_eq!(spawn.command.as_deref(), Some("gemini"));
        assert_eq!(
            spawn.args.as_deref(),
            Some(&["--yolo".to_string()][..]),
            "gemini cell keeps the preset's own auto-approve flag",
        );

        cleanup_mirror_ws(&ws_slug);
    }

    /// A user-customized CLAUDE preset that dropped the headless flag gets it
    /// ENSURED back — inside the jail claude must run auto-approved, or the
    /// cell wedges on a permission prompt no one can answer.
    #[test]
    fn workspace_session_ensures_claude_skip_permissions() {
        k2_core::db::init_for_tests();
        let preset_id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
                 VALUES (?1, ?2, 'claude --model opus', '', 1, 99, 0)",
                rusqlite::params![preset_id, format!("custom-claude-{preset_id}")],
            )
            .expect("insert custom preset");
        }
        let ws_slug = format!("v1ws-customclaude-{}", uuid::Uuid::new_v4());
        let ws_path = incell_home().join(&ws_slug);
        let ws_path_str = ws_path.to_string_lossy().into_owned();
        insert_project_with_agent(&ws_slug, &ws_path_str, &preset_id);

        let spawn = resolve_workspace_session(
            &ws_path_str,
            &ws_slug,
            &SessionId::new(),
            &V1Principal::Owner,
            &ApiSandboxRequest::default(),
        )
        .expect("resolve workspace session");

        assert_eq!(spawn.command.as_deref(), Some("claude"));
        assert_eq!(
            spawn.args.as_deref(),
            Some(
                &[
                    "--model".to_string(),
                    "opus".to_string(),
                    "--dangerously-skip-permissions".to_string(),
                ][..]
            ),
            "claude cells get --dangerously-skip-permissions ensured (appended once)",
        );

        cleanup_mirror_ws(&ws_slug);
    }
}
