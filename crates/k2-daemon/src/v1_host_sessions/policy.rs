//! F1 (prd-v1-api-completion §3) — the PASSTHROUGH POLICY RESOLVER for
//! `/v1/w/<ws>/host-sessions`: the ONE new security-critical piece of the
//! host-sessions family.
//!
//! Mirrors `v1_sandboxes::policy` in role: it takes the UNTRUSTED public body
//! ([`ApiHostSessionRequest`] — every field a HINT) plus a HOST-RESOLVED
//! [`V1Principal`] and an ALREADY-AUTHORIZED workspace path, and produces a
//! HOST-TRUSTED [`SpawnRequest`]. The caller NEVER decides workspace, command,
//! env, credential, or identity. Invariants:
//!
//! - `cwd` **PINNED to the granted workspace's registered path** — the
//!   `projects.path` value `resolve_authorized_workspace` returned. NEVER
//!   `$HOME`, NEVER a caller-supplied path (the body carries no path field at
//!   all).
//! - `command`/`args` = the WORKSPACE'S CONFIGURED agent command (the
//!   de-generalization seam, [`k2_core::workspace::agent_resolve`]) with that
//!   provider's session-id / resume conventions spliced in host-side. The
//!   caller's command/args are DROPPED ENTIRELY (there is no field for them).
//! - **Dangerous auto-approve flags are STRIPPED by default**: on the host
//!   (no microVM jail) the agent's own permission prompts ARE a safety layer,
//!   so `--dangerously-skip-permissions` and friends are removed from the
//!   resolved preset args unless the workspace owner explicitly opted in
//!   (`projects.api_skip_permissions`, default OFF — migration 0069).
//! - `env` = HOST-CURATED ONLY. The caller's env is DROPPED ENTIRELY (no
//!   field). The Anthropic key is staged from the PRINCIPAL's api_keys row
//!   (0058 `anthropic_api_key`) exactly as cells do — never from the body.
//! - `agent_name` host-minted `api-<principal>-<uuid>` (the same anti-hijack
//!   namespace as the sandbox doors — can't collide with `tab-…`/pinned keys,
//!   and `v2_session_map` labels `api-…` passthrough sessions `backend:"host"`
//!   on `SessionAdded`).
//! - `sandbox: None` — the deliberate PASSTHROUGH spawn. The route's response
//!   labels it honestly (`"sandbox":"none"`); we never pretend isolation.

use std::collections::HashMap;

use k2_core::log_debug;
use k2_core::session::SessionId;

use crate::routes::http::V1Principal;
use crate::session_token::Provider;
use crate::v2_spawn::SpawnRequest;

/// The UNTRUSTED public request body for `POST /v1/w/<ws>/host-sessions`.
/// Every field is a HINT, never a trust input. Absent/empty body → defaults.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ApiHostSessionRequest {
    /// Optional initial prompt. Delivered into the spawned agent's PTY once
    /// the TUI is ready (host sessions have no guest-init to read an env
    /// staging var) — never argv, value never logged.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Optional terminal width hint (clamped host-side).
    #[serde(default)]
    pub cols: Option<u16>,
    /// Optional terminal height hint (clamped host-side).
    #[serde(default)]
    pub rows: Option<u16>,
    /// Idle-reap timeout in seconds — clamped 30..86400, default 180
    /// (identical semantics to the sandbox family; `sandbox_reaper`).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Resume an EXISTING host session by id (PRD §3: "or resume with
    /// {\"session\": id}"). Validated + canonical-guarded + checked against
    /// this workspace's own api-spawned session index at the route door.
    #[serde(default)]
    pub session: Option<String>,
}

/// The auto-approve flags the seeded agent presets carry. On the host these
/// are STRIPPED unless the workspace owner opted in (`api_skip_permissions`).
/// Keep in sync with the built-in `agent_presets` seeds (`k2-core/src/db/mod.rs`).
const DANGER_FLAGS: &[&str] = &[
    "--dangerously-skip-permissions",          // claude
    "--dangerously-bypass-approvals-and-sandbox", // codex
    "--yolo",                                  // gemini
    "--always-approve",                        // grok
    "--allow-all",                             // copilot
];

/// Clamp an optional dimension hint into `[min, max]`, defaulting when absent.
fn clamp_dim(hint: Option<u16>, default: u16, min: u16, max: u16) -> u16 {
    hint.unwrap_or(default).clamp(min, max)
}

/// Keep only URL/path-safe identity chars so the host-minted `agent_name` is
/// well-formed regardless of the principal id shape (mirrors
/// `v1_sandboxes::policy::sanitize_id`).
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

/// Remove every known dangerous auto-approve flag from a resolved preset's
/// args. Pure + order-preserving so the rest of the preset command survives
/// byte-identically.
fn strip_danger_flags(args: Vec<String>) -> Vec<String> {
    args.into_iter()
        .filter(|a| !DANGER_FLAGS.contains(&a.as_str()))
        .collect()
}

/// Resolve the untrusted body + authenticated principal + AUTHORIZED
/// workspace path into a host-trusted [`SpawnRequest`] for a NON-SANDBOXED
/// host session. `session_id` is the HOST-DECIDED id (fresh mint or the
/// validated resume target) — it is FORCED into the spawn AND spliced into
/// the agent command's session grammar, so the returned/addressable
/// `sessionId` equals the provider's conversation id.
///
/// Infallible by design: there is nothing to provision (no ephemeral dir, no
/// overlay); the agent-command resolver itself falls back to literal claude
/// rather than erroring (a stale preset must never brick a spawn).
pub fn resolve_host_spawn(
    principal: &V1Principal,
    ws_path: &str,
    session_id: &SessionId,
    resume: bool,
    req: &ApiHostSessionRequest,
) -> SpawnRequest {
    let sid = session_id.to_string();

    // (1) The workspace's configured agent command — the de-generalization
    // seam (projects.default_agent → global default → literal claude). The
    // caller has NO say in the command.
    let resolved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_resolve::resolve_agent_command(&conn, ws_path)
    };
    let command = resolved.command.clone();
    let mut args = resolved.args.clone();

    // (2) DANGER-FLAG POLICY (PRD §3, LOCKED default): unlike cells, do NOT
    // force auto-approve — on the host the agent's own permission prompts are
    // a safety layer. Strip the known flags unless the workspace owner
    // explicitly opted in (per-workspace `api_skip_permissions`, default OFF,
    // fail-closed on unknown workspace/NULL).
    let skip_permissions_opt_in =
        k2_core::workspace::settings::get_api_skip_permissions(ws_path);
    if !skip_permissions_opt_in {
        let before = args.len();
        args = strip_danger_flags(args);
        if args.len() != before {
            log_debug!(
                "[v1-host] stripped auto-approve flag(s) from resolved agent args for ws={} (api_skip_permissions is OFF)",
                ws_path
            );
        }
    }

    // (3) Session-identity grammar, spliced HOST-SIDE via the ProviderResume
    // adapter table: fresh → the premint flag (`--session-id <sid>` for
    // claude/grok); resume → the provider's resume grammar (`--resume <sid>`,
    // pi `--session`, codex `resume <sid>`). Self-minting / unknown providers
    // spawn bare (degraded-but-correct, same posture as every other daemon
    // spawn site) — and because the argv then carries no identity, v2_spawn's
    // `autoinject_premint_session_id` is a no-op for unknown providers.
    if let Some(adapter) =
        k2_core::workspace::provider_resume::provider_resume_for_command(&command)
    {
        if resume {
            args = adapter.resume_args(&args, &sid);
        } else if let Some(preminted) = adapter.premint_args(&args, &sid) {
            args = preminted;
        }
    }

    // (4) Host-minted agent name — same anti-hijack namespace as the sandbox
    // doors; also what v2_session_map keys the `backend:"host"` SessionAdded
    // label off.
    let agent_name = format!(
        "api-{}-{}",
        sanitize_id(&principal.display_id()),
        uuid::Uuid::new_v4()
    );

    // (5) Host-curated env. The caller's env is DROPPED ENTIRELY (the body
    // has no env field); the ONLY entry is the Anthropic key staged from the
    // PRINCIPAL's api_keys row — never the body, never logged. An Owner-token
    // principal stages nothing (own-use: the host's ambient login applies).
    let mut env: HashMap<String, String> = HashMap::new();
    if let V1Principal::Api(p) = principal {
        match p.anthropic_key.as_deref().map(str::trim) {
            Some(key) if !key.is_empty() => {
                env.insert(Provider::Anthropic.key_env_var().to_string(), key.to_string());
                log_debug!(
                    "[v1-host] staged principal Anthropic key into host-session env (principal={})",
                    p.id
                );
            }
            _ => log_debug!(
                "[v1-host] principal={} has no usable Anthropic key; host session uses the host's ambient credential",
                p.id
            ),
        }
    }

    let cols = clamp_dim(req.cols, 80, 16, 500);
    let rows = clamp_dim(req.rows, 24, 4, 300);

    log_debug!(
        "[v1-host] resolved host session id={} ws={} command={} args={:?} resume={} skip_permissions_opt_in={}",
        sid,
        ws_path,
        command,
        args,
        resume,
        skip_permissions_opt_in,
    );

    SpawnRequest {
        agent_name,
        // PINNED to the granted workspace's registered path — never $HOME,
        // never a caller path.
        cwd: ws_path.to_string(),
        command: Some(command),
        args: Some(args),
        cols,
        rows,
        env: Some(env),
        label: None,
        label_locked: None,
        // The DELIBERATE passthrough: no sandbox requested, no backend echo
        // in the raw spawn body (the route's own response says "none").
        sandbox: None,
        // Nothing ephemeral to tear down — the cwd is the REAL workspace.
        ephemeral_cwd: None,
        // Stamped by the route door AFTER quota acquire (mirrors sandboxes).
        principal_key: None,
        overlay: None,
        // FORCE the host-decided id so the returned/addressable sessionId
        // equals the conversation id spliced into the agent argv above.
        forced_session_id: Some(*session_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(id: &str, key: Option<&str>) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: key.map(str::to_string),
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
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

    /// The core resolver contract: cwd PINNED to the workspace path, the
    /// default (claude) command with `--dangerously-skip-permissions`
    /// STRIPPED and `--session-id <sid>` spliced, principal key staged,
    /// host-minted `api-` name, forced session id, sandbox None.
    #[test]
    fn resolve_pins_cwd_strips_danger_and_splices_session_id() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-default";
        insert_project("v1host-policy-default", ws_path);
        let sid = SessionId::new();

        let spawn = resolve_host_spawn(
            &api("key-1", Some("sk-ant-host-key")),
            ws_path,
            &sid,
            false,
            &ApiHostSessionRequest { cols: Some(120), rows: Some(40), ..Default::default() },
        );

        // cwd PINNED — the registered workspace path, verbatim.
        assert_eq!(spawn.cwd, ws_path);
        // The workspace default resolves to the built-in claude preset; the
        // danger flag is STRIPPED (default OFF) and the premint spliced.
        assert_eq!(spawn.command.as_deref(), Some("claude"));
        let args = spawn.args.as_deref().expect("args present");
        assert!(
            !args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "auto-approve must be stripped by default; args={args:?}"
        );
        assert_eq!(
            args,
            &["--session-id".to_string(), sid.to_string()][..],
            "claude premint convention with the FORCED id"
        );
        // Principal key staged; nothing else in the curated env.
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("sk-ant-host-key"));
        assert_eq!(env.len(), 1, "host-curated env is EXACTLY the principal key");
        // Identity + spawn wiring.
        assert!(spawn.agent_name.starts_with("api-key-1-"), "{}", spawn.agent_name);
        assert_eq!(spawn.forced_session_id, Some(sid));
        assert_eq!(spawn.sandbox, None, "deliberate passthrough — no sandbox request");
        assert!(spawn.ephemeral_cwd.is_none());
        assert!(spawn.overlay.is_none());
        assert_eq!(spawn.cols, 120);
        assert_eq!(spawn.rows, 40);
    }

    /// Owner opt-in (`api_skip_permissions=1`) keeps the preset's flags.
    #[test]
    fn opt_in_keeps_auto_approve_flags() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-optin";
        insert_project("v1host-policy-optin", ws_path);
        k2_core::workspace::settings::update_project_setting(
            ws_path,
            "api_skip_permissions",
            "1",
        )
        .expect("set opt-in");
        let sid = SessionId::new();

        let spawn = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            false,
            &ApiHostSessionRequest::default(),
        );
        let args = spawn.args.as_deref().expect("args");
        assert!(
            args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "opt-in keeps the preset's flag; args={args:?}"
        );
        // Owner principal stages no key.
        assert_eq!(spawn.env.as_ref().map(|e| e.len()), Some(0));
    }

    /// Resume splices the provider's RESUME grammar (`--resume <sid>` for
    /// claude), not the premint.
    #[test]
    fn resume_uses_resume_grammar() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-resume";
        insert_project("v1host-policy-resume", ws_path);
        let sid = SessionId::new();

        let spawn = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            true,
            &ApiHostSessionRequest::default(),
        );
        assert_eq!(
            spawn.args.as_deref(),
            Some(&["--resume".to_string(), sid.to_string()][..]),
        );
    }

    /// A blank principal key stages nothing (never an empty assignment).
    #[test]
    fn blank_principal_key_is_dropped() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-blankkey";
        insert_project("v1host-policy-blankkey", ws_path);
        let spawn = resolve_host_spawn(
            &api("key-b", Some("   ")),
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        assert_eq!(spawn.env.as_ref().map(|e| e.len()), Some(0));
    }

    #[test]
    fn strip_danger_flags_removes_all_known_flags_only() {
        let args: Vec<String> = [
            "--dangerously-skip-permissions",
            "--model",
            "opus",
            "--yolo",
            "--always-approve",
            "--allow-all",
            "--dangerously-bypass-approvals-and-sandbox",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            strip_danger_flags(args),
            vec!["--model".to_string(), "opus".to_string()],
        );
    }

    #[test]
    fn dims_are_clamped() {
        assert_eq!(clamp_dim(Some(5), 80, 16, 500), 16);
        assert_eq!(clamp_dim(Some(9999), 24, 4, 300), 300);
        assert_eq!(clamp_dim(None, 80, 16, 500), 80);
    }
}
