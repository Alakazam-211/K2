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
}

impl PolicyError {
    /// HTTP status for this failure (fail-closed — a server-side provisioning
    /// failure, not the caller's fault).
    pub fn status(&self) -> &'static str {
        match self {
            PolicyError::NoEphemeralWorkspace(_) => "503 Service Unavailable",
        }
    }

    /// Client-safe message (no secrets; the path is daemon-internal but benign).
    pub fn message(&self) -> String {
        match self {
            PolicyError::NoEphemeralWorkspace(e) => {
                format!("could not provision sandbox workspace: {e}")
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

    // (3) Host-curated env — the caller's env is DROPPED ENTIRELY. Stage the
    // Anthropic key from the validated PRINCIPAL (never the body), reusing B3a's
    // `Provider → key_env_var` mapping. The key is NEVER logged.
    let mut env: HashMap<String, String> = HashMap::new();
    match principal {
        V1Principal::Api(p) => match p.anthropic_key.as_deref().map(str::trim) {
            Some(key) if !key.is_empty() => {
                env.insert(Provider::Anthropic.key_env_var().to_string(), key.to_string());
                log_debug!(
                    "[v1-sandbox] staged principal Anthropic key into ephemeral session env (principal={})",
                    p.id
                );
            }
            _ => log_debug!(
                "[v1-sandbox] principal={} has no usable Anthropic key; cell will have no model credential",
                p.id
            ),
        },
        V1Principal::Owner => {
            // FOLLOW-UP: there is no app-level Anthropic key source for the owner
            // principal yet, so own-use spawns stage no key. Own-use validation
            // runs on-box with a key-bearing API key; an app-level fallback can
            // slot in here later without touching the invariant.
            log_debug!(
                "[v1-sandbox] owner principal: no app-level Anthropic key fallback (follow-up); none staged"
            );
        }
    }

    // (4) Dimensions are hints only — clamp to sane bounds.
    let cols = clamp_dim(req.cols, 80, 16, 500);
    let rows = clamp_dim(req.rows, 24, 4, 300);

    // Prompt: explicitly IGNORED for v1 (never forwarded as argv). Touch it so
    // the drop is intentional + visible, not an oversight.
    if req.prompt.is_some() {
        log_debug!("[v1-sandbox] caller prompt is ignored in v1 (not forwarded as argv)");
    }

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
    log_debug!(
        "[v1-sandbox] provisioned ephemeral workspace {} (per-session chown deferred to the spawn door — P4-H6)",
        dir.display()
    );
    Ok(dir)
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
        // env carries ONLY the host-staged principal key (caller env dropped).
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-ant-principal-key"),
        );
        assert_eq!(env.len(), 1, "env must be host-curated ONLY (one key)");
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
        assert!(env.is_empty(), "keyless principal → fully empty host-curated env");
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
}
