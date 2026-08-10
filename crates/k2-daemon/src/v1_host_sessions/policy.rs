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
//!   so the UNION of the resolved preset's DECLARED `danger_flags`
//!   (migration 0070 metadata) and the legacy hardcoded floor
//!   ([`DANGER_FLAGS`]) is removed from the resolved preset args unless the
//!   workspace owner explicitly opted in (`projects.api_skip_permissions`,
//!   default OFF — migration 0069). A preset with NULL metadata (or a fully
//!   custom command with no preset) still gets the floor stripped, plus a
//!   loud log line naming the residual: flags we've never audited can't be
//!   stripped.
//! - `env` = HOST-CURATED ONLY. The caller's env is DROPPED ENTIRELY (no
//!   field). The base layer is the resolved preset's own `env` metadata
//!   (migration 0070 — host data, not caller data); the PRINCIPAL's stored
//!   LLM credential is staged ON TOP from its api_keys row under the env
//!   var(s) the row's `provider` metadata maps to (W5, migration 0071 —
//!   [`k2_core::api_keys::ApiPrincipal::staged_env_pairs`]; NULL provider =
//!   anthropic → ANTHROPIC_API_KEY exactly as before; unknown provider →
//!   NOTHING, fail-closed) — never from the body, and K2-curated entries
//!   always override preset entries.
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
use crate::v2_spawn::SpawnRequest;

/// Named plan for how the cold / dead-resume arms deliver the first-turn
/// prompt (prd-host-session-launch-param-prompt-v1 §4.2).
///
/// - `attached == true` → prompt rides the **ephemeral** exec argv; the
///   route must **not** start the detached post-spawn inject thread.
/// - `attached == false` + non-empty prompt → existing post-spawn inject.
/// - empty prompt → `attached == false`, no inject.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchPromptPlan {
    pub attached: bool,
}

/// The UNTRUSTED public request body for `POST /v1/w/<ws>/host-sessions`.
/// Every field is a HINT, never a trust input. Absent/empty body → defaults.
///
/// Deliberately has NO guest-policy / system-prompt field (Phase 0b): the
/// owner `api_guest_policy` and frozen `API_SPAWN_PREAMBLE` are host-only.
/// Unknown JSON keys (including attacker-supplied `api_guest_policy`) are
/// ignored by serde.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ApiHostSessionRequest {
    /// Optional initial / next prompt. On cold spawn + dead resume, when the
    /// provider supports interactive launch-param, attached to the
    /// **ephemeral** exec argv only (never durable `args_json` / recovery).
    /// Unsupported providers fall back to post-spawn paste inject. Live
    /// follow-ups always inject. Value never logged in full.
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
    /// Capability envelope hints (PRD S1–S3). Optional. Absent → no staged handle.
    /// Unknown shape rejected at the door (not silently ignored when present
    /// but invalid — see `v1_capabilities::parse_capabilities`).
    #[serde(default)]
    pub capabilities: Option<serde_json::Value>,
    /// Durable spawn-queue admit hint (prd-host-session-spawn-queue-v1).
    /// When the feature flag is ON (`K2_HOST_SESSION_SPAWN_QUEUE`):
    /// - absent / `true` → enqueue on cap refuse (nowait acquire)
    /// - `false` → immediate 429 (no S8 wait, no enqueue)
    /// When the feature is OFF this field is ignored (legacy S8 then 429).
    #[serde(default)]
    pub queue: Option<bool>,
    /// Optional client idempotency key. Double-submit with the same id under
    /// the same principal+workspace returns the same `jobId` while open.
    #[serde(default, alias = "clientRequestId")]
    pub client_request_id: Option<String>,
}

/// The legacy hardcoded FLOOR of known auto-approve flags. Since W2
/// (0.40.30) the strip is data-driven — the resolved preset's DECLARED
/// `danger_flags` (migration 0070) are stripped too — but this floor is
/// KEPT so a pre-0070 DB (or a preset whose metadata backfill never ran)
/// stays exactly as safe as before. On the host these are STRIPPED unless
/// the workspace owner opted in (`api_skip_permissions`).
/// Keep in sync with `db::BUILT_IN_PRESET_METADATA`'s five flag owners
/// (`k2-core/src/db/mod.rs`) — same flags, declared per-owner there.
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
/// args: the UNION of the hardcoded [`DANGER_FLAGS`] floor and the preset's
/// own `declared` flags (migration-0070 metadata; empty slice when the
/// metadata is NULL/absent). Pure + order-preserving so the rest of the
/// preset command survives byte-identically.
fn strip_danger_flags(args: Vec<String>, declared: &[String]) -> Vec<String> {
    args.into_iter()
        .filter(|a| {
            !DANGER_FLAGS.contains(&a.as_str()) && !declared.iter().any(|d| d == a)
        })
        .collect()
}

/// Ensure unattended auto-approve flags are present when the workspace has
/// opted in (`api_skip_permissions` / public wiki chat). Strip only *keeps*
/// flags the preset already had — if the resolved argv lost them (or the
/// preset never declared them), an unattended host-session still HITLs.
/// Prepend missing flags from the preset's declared list, else the
/// command-matched floor (claude → `--dangerously-skip-permissions`, …).
fn ensure_danger_flags(mut args: Vec<String>, declared: &[String], command: &str) -> Vec<String> {
    let cmd = command.rsplit('/').next().unwrap_or(command).to_ascii_lowercase();
    let needed: Vec<String> = if !declared.is_empty() {
        declared.to_vec()
    } else {
        let mut v = Vec::new();
        if cmd.contains("claude") {
            v.push("--dangerously-skip-permissions".into());
        }
        if cmd.contains("codex") {
            v.push("--dangerously-bypass-approvals-and-sandbox".into());
        }
        if cmd.contains("gemini") {
            v.push("--yolo".into());
        }
        if cmd.contains("grok") {
            v.push("--always-approve".into());
        }
        if cmd.contains("copilot") {
            v.push("--allow-all".into());
        }
        // Unknown binary: no safe auto-flag to invent.
        v
    };
    for flag in needed.into_iter().rev() {
        if !args.iter().any(|a| a == &flag) {
            args.insert(0, flag);
        }
    }
    args
}

/// W4 (0.40.30) — resolve the READINESS dialect of the workspace's
/// configured agent for the post-spawn initial-prompt injector, through
/// the ONE shared precedence chain
/// ([`k2_core::workspace::provider_resume::resolve_injection_profile`]):
///
/// 1. the resolved preset's declared `readiness` metadata
///    (migration 0070: `'bracketed-paste'` | `'settle:<ms>'`),
/// 2. else the static provider table (?2004h LIES for
///    codex/gemini/pi/hermes — their startup dialogs assert bracketed
///    paste before they're ready and EAT injected text; those need the
///    settle-floor wait, not a paste poll),
/// 3. else the claude-shaped poll default.
///
/// Same resolution seam as [`resolve_host_spawn`] step (1)
/// (`agent_resolve::resolve_agent_command` — workspace default → global
/// default → literal claude), so the profile always describes the agent
/// that spawn actually launched.
pub fn resolve_host_injection_profile(
    ws_path: &str,
) -> k2_core::workspace::provider_resume::InjectionProfile {
    let resolved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_resolve::resolve_agent_command(&conn, ws_path)
    };
    let provider =
        k2_core::workspace::provider_resume::provider_resume_for_command(&resolved.command)
            .map(|p| p.provider)
            .unwrap_or("");
    k2_core::workspace::provider_resume::resolve_injection_profile(
        resolved.readiness.as_deref(),
        provider,
    )
}

/// Resolve the untrusted body + authenticated principal + AUTHORIZED
/// workspace path into a host-trusted [`SpawnRequest`] for a NON-SANDBOXED
/// host session, plus a [`LaunchPromptPlan`] describing whether the
/// spawn-time prompt stack was attached as an ephemeral launch-param.
///
/// `session_id` is the HOST-DECIDED id (fresh mint or the validated resume
/// target) — it is FORCED into the spawn AND spliced into the agent
/// command's session grammar, so the returned/addressable `sessionId`
/// equals the provider's conversation id.
///
/// Infallible by design: there is nothing to provision (no ephemeral dir, no
/// overlay); the agent-command resolver itself falls back to literal claude
/// rather than erroring (a stale preset must never brick a spawn). Launch-
/// param assembly failure falls back to inject (`plan.attached = false`).
pub(crate) fn resolve_host_spawn(
    principal: &V1Principal,
    ws_path: &str,
    session_id: &SessionId,
    resume: bool,
    req: &ApiHostSessionRequest,
) -> (SpawnRequest, LaunchPromptPlan) {
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

    // (2) DANGER-FLAG POLICY (prd-api-skip-permissions-default-on-v1):
    // `/v1` host-sessions are headless. Product default is
    // `api_skip_permissions` ON (NULL/1) so auto-approve flags stay on
    // the argv. Owners may opt OUT (explicit 0) to restore stripping —
    // on the host the agent's own permission prompts are then a safety
    // layer again. Unknown workspace path still fail-closes OFF.
    //
    // HONEST RESIDUAL when the metadata is unknown (`danger_flags == None`:
    // a fully custom command with no preset row, or a preset whose
    // metadata is NULL/malformed): we can only strip the five audited
    // floor flags. If that agent has its OWN auto-approve flag under a
    // name we've never seen, it SURVIVES into the spawn — there is no
    // grammar oracle for arbitrary binaries. We log a loud structured
    // warning so the operator can declare the flags on the preset row
    // (or accept the residual); we deliberately do NOT guess-strip
    // unknown args (a false positive would break the agent's argv).
    // Also keep skip ON when durable wiki_public_chat is enabled (both
    // checked so chat works even if skip was cleared by hand).
    let skip_permissions_opt_in =
        k2_core::workspace::settings::get_api_skip_permissions(ws_path)
            || k2_core::workspace::settings::get_wiki_public_chat(ws_path);
    let declared_owned: Vec<String> = resolved.danger_flags.clone().unwrap_or_default();
    let declared: &[String] = declared_owned.as_slice();
    if !skip_permissions_opt_in {
        if resolved.danger_flags.is_none() {
            log_debug!(
                "[v1-host] WARNING event=danger_flags_unknown ws={} preset_id={:?} command={}: \
                 resolved agent declares no danger_flags metadata; only the hardcoded floor is \
                 stripped — the agent's own auto-approve flags (if any) cannot be known and \
                 would survive into the host spawn",
                ws_path,
                resolved.preset_id,
                command,
            );
        }
        let before = args.len();
        args = strip_danger_flags(args, declared);
        if args.len() != before {
            log_debug!(
                "[v1-host] stripped auto-approve flag(s) from resolved agent args for ws={} (api_skip_permissions is OFF)",
                ws_path
            );
        }
    } else {
        // Opt-in: do not strip, and ENSURE auto-approve flags are present so
        // every host-session spawn is unattended (visitor chat / API keys).
        let before = args.clone();
        args = ensure_danger_flags(args, declared, &command);
        if args != before {
            log_debug!(
                "[v1-host] ensured auto-approve flag(s) on host-session spawn for ws={} (api_skip_permissions/public chat ON) args={:?}",
                ws_path,
                args
            );
        }
    }

    // (3) Session-identity grammar, spliced HOST-SIDE via the ProviderResume
    // adapter table: fresh → the premint flag (`--session-id <sid>` for
    // claude/grok); resume → the provider's resume grammar (`--resume <sid>`,
    // pi `--session`, codex `resume <sid>`). Self-minting providers
    // (pi/codex/gemini/cursor/hermes) spawn bare on a FRESH session — the
    // route's deferred adoption discovers + stamps their provider-minted id
    // post-hoc; unknown providers spawn bare period (degraded-but-correct,
    // same posture as every other daemon spawn site) — and because the argv
    // then carries no identity, v2_spawn's `autoinject_premint_session_id`
    // is a no-op for unknown providers.
    //
    // On RESUME the id spliced into the grammar is the caller's validated
    // resume TARGET (`req.session` — the route already checked it against
    // this workspace's own api-spawned index), NOT necessarily `sid`: for a
    // self-minting provider the on-disk conversation id is provider-minted
    // and — for hermes — not even UUID-shaped, so it cannot always ride the
    // forced daemon SessionId. When the target IS a UUID the route forces
    // `sid == target` and the two coincide (the claude/grok premint case).
    if let Some(adapter) =
        k2_core::workspace::provider_resume::provider_resume_for_command(&command)
    {
        if resume {
            let target = req
                .session
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&sid);
            args = adapter.resume_args(&args, target);
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
    // has no env field). Base layer: the resolved preset's own env metadata
    // (migration 0070 — host-owned data). On top: the PRINCIPAL's stored
    // LLM credential, staged under the env var(s) its `provider` metadata
    // maps to (W5, migration 0071 — `ApiPrincipal::staged_env_pairs`: NULL
    // provider = anthropic → exactly ANTHROPIC_API_KEY, the pre-0071
    // behavior; openai → OPENAI_API_KEY [+OPENAI_BASE_URL]; google →
    // GEMINI_API_KEY+GOOGLE_API_KEY; xai → XAI_API_KEY; UNKNOWN provider →
    // NOTHING, fail-closed). Never from the body, never logged; K2-staged
    // entries OVERRIDE same-named preset entries (K2-curated env wins). An
    // Owner-token principal stages nothing (own-use: the host's ambient
    // login applies). Env VALUES are never logged — only var NAMES.
    let mut env: HashMap<String, String> = resolved.env_map();
    if let V1Principal::Api(p) = principal {
        let staged = p.staged_env_pairs();
        if staged.is_empty() {
            // No usable credential OR unknown provider (the seam already
            // logged the structured unknown-provider warning).
            log_debug!(
                "[v1-host] principal={} staged no LLM credential; host session uses the host's ambient credential",
                p.id
            );
        } else {
            let names: Vec<&str> = staged.iter().map(|(k, _)| k.as_str()).collect();
            log_debug!(
                "[v1-host] staged principal LLM credential into host-session env vars {:?} (principal={})",
                names,
                p.id
            );
            for (k, v) in staged {
                env.insert(k, v);
            }
        }
    }
    // Stage the host-minted agent name so in-cell tools that still require
    // `--agent` / K2SO_AGENT_NAME (legacy k2 done checkin, roster verbs) work
    // without a silent no-op. Lifecycle complete on 0.40.87+ uses session
    // context, but agents often still set the name explicitly.
    env.insert("K2SO_AGENT_NAME".to_string(), agent_name.clone());
    env.insert("K2_AGENT_NAME".to_string(), agent_name.clone());
    // Host-session id for in-cell path derivation (parity with sandbox
    // policy `K2_SESSION_ID`). Same string as API `sessionId` / cap JWT
    // `sub`. Cold + dead-resume rebuild env; live inject keeps the process.
    env.insert("K2_SESSION_ID".to_string(), sid.clone());
    // D6: API-cell lifecycle discriminator for `k2 done` → mark_complete.
    // Set ONLY on /v1 host-session spawn (not wake/heartbeat/canonical).
    // Workspace agents under COMPAT-58 may have K2_HOOK_SOCK + scoped token
    // but must keep legacy checkin --done; they never receive this env.
    env.insert("K2_API_CELL".to_string(), "1".to_string());
    env.insert("K2SO_API_CELL".to_string(), "1".to_string());

    let cols = clamp_dim(req.cols, 80, 16, 500);
    let rows = clamp_dim(req.rows, 24, 4, 300);

    // (6) Launch-param (cold + dead resume only — this resolver is only
    // used on those arms). Compose the frozen spawn stack as ONE trailing
    // interactive user message when the provider supports it. Identity
    // args stay on `args` (durable); full exec argv with prompt goes on
    // `exec_args` (ephemeral, fire-once S1).
    let caller_prompt = req.prompt.as_deref().unwrap_or("").trim();
    let mut plan = LaunchPromptPlan { attached: false };
    let mut exec_args: Option<Vec<String>> = None;
    let mut launch_string_for_log: Option<String> = None;
    if !caller_prompt.is_empty() {
        let guest = k2_core::workspace::settings::get_api_guest_policy(ws_path);
        let launch_string =
            crate::v1_host_sessions::compose_spawn_inject(&guest, caller_prompt);
        if let Some(with_prompt) =
            k2_core::workspace::provider_launch_prompt::append_interactive_prompt(
                &command,
                &args,
                &launch_string,
            )
        {
            plan.attached = true;
            launch_string_for_log = Some(launch_string);
            exec_args = Some(with_prompt);
        }
    }

    // Log identity args, or redacted exec shape when attached (never the
    // full launch-prompt payload — S3 / A12).
    let log_args = if let (true, Some(ls)) = (plan.attached, launch_string_for_log.as_deref()) {
        k2_core::workspace::provider_launch_prompt::redact_launch_prompt_args(
            exec_args.as_deref().unwrap_or(&args),
            ls,
        )
    } else {
        args.clone()
    };
    log_debug!(
        "[v1-host] resolved host session id={} ws={} command={} args={:?} resume={} skip_permissions_opt_in={} launch_prompt_attached={}",
        sid,
        ws_path,
        command,
        log_args,
        resume,
        skip_permissions_opt_in,
        plan.attached,
    );

    let spawn = SpawnRequest {
        agent_name,
        // PINNED to the granted workspace's registered path — never $HOME,
        // never a caller path.
        cwd: ws_path.to_string(),
        command: Some(command),
        // Identity-only — what register / args_json / recovery see.
        args: Some(args),
        // Ephemeral exec with trailing launch prompt when attached.
        exec_args,
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
        quota_workspace: None,
        overlay: None,
        // FORCE the host-decided id so the returned/addressable sessionId
        // equals the conversation id spliced into the agent argv above.
        forced_session_id: Some(*session_id),
    };
    (spawn, plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(id: &str, key: Option<&str>) -> V1Principal {
        api_with_provider(id, key, None, None)
    }

    /// W5 — an API principal with explicit provider/base_url metadata.
    fn api_with_provider(
        id: &str,
        key: Option<&str>,
        provider: Option<&str>,
        base_url: Option<&str>,
    ) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: key.map(str::to_string),
            provider: provider.map(str::to_string),
            base_url: base_url.map(str::to_string),
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
            capabilities: k2_core::api_keys::ApiCapabilities::all(),
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
    /// default (claude) command keeps `--dangerously-skip-permissions`
    /// (product default ON for headless /v1) and `--session-id <sid>`
    /// spliced, principal key staged, host-minted `api-` name, forced
    /// session id, sandbox None.
    #[test]
    fn resolve_pins_cwd_keeps_danger_by_default_and_splices_session_id() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-default";
        insert_project("v1host-policy-default", ws_path);
        let sid = SessionId::new();

        let (spawn, _) = resolve_host_spawn(
            &api("key-1", Some("sk-ant-host-key")),
            ws_path,
            &sid,
            false,
            &ApiHostSessionRequest { cols: Some(120), rows: Some(40), ..Default::default() },
        );

        // cwd PINNED — the registered workspace path, verbatim.
        assert_eq!(spawn.cwd, ws_path);
        // Product default ON: auto-approve flags kept; premint spliced.
        assert_eq!(spawn.command.as_deref(), Some("claude"));
        let args = spawn.args.as_deref().expect("args present");
        assert!(
            args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "auto-approve kept by default (api_skip_permissions ON); args={args:?}"
        );
        assert!(
            args.iter().any(|a| a == "--session-id")
                && args.iter().any(|a| a == &sid.to_string()),
            "claude premint convention with the FORCED id; args={args:?}"
        );
        // Principal key + agent-name markers + D6 API-cell lifecycle markers.
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("sk-ant-host-key"));
        assert!(env.get("K2SO_AGENT_NAME").is_some());
        assert!(env.get("K2_AGENT_NAME").is_some());
        assert_eq!(env.get("K2_API_CELL").map(String::as_str), Some("1"));
        assert_eq!(env.get("K2SO_API_CELL").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("K2_SESSION_ID").map(String::as_str),
            Some(sid.to_string().as_str())
        );
        assert_eq!(
            env.len(),
            6,
            "principal key + agent name pair + K2_SESSION_ID + K2_API_CELL pair"
        );
        // Identity + spawn wiring.
        assert!(spawn.agent_name.starts_with("api-key-1-"), "{}", spawn.agent_name);
        assert_eq!(spawn.forced_session_id, Some(sid));
        assert_eq!(spawn.sandbox, None, "deliberate passthrough — no sandbox request");
        assert!(spawn.ephemeral_cwd.is_none());
        assert!(spawn.overlay.is_none());
        assert_eq!(spawn.cols, 120);
        assert_eq!(spawn.rows, 40);
    }

    /// Explicit opt-out (`api_skip_permissions=0`) strips auto-approve flags.
    #[test]
    fn opt_out_strips_auto_approve_flags() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-optout";
        insert_project("v1host-policy-optout", ws_path);
        k2_core::workspace::settings::update_project_setting(
            ws_path,
            "api_skip_permissions",
            "0",
        )
        .expect("set opt-out");
        let sid = SessionId::new();

        let (spawn, _) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            false,
            &ApiHostSessionRequest::default(),
        );
        let args = spawn.args.as_deref().expect("args");
        assert!(
            !args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "opt-out strips the preset's flag; args={args:?}"
        );
        // Owner principal stages no credential key; agent-name + D6 markers only.
        let env = spawn.env.as_ref().expect("env present");
        assert!(env.get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(env.get("K2_API_CELL").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("K2_SESSION_ID").map(String::as_str),
            Some(sid.to_string().as_str())
        );
        assert_eq!(
            env.len(),
            5,
            "agent name pair + K2_SESSION_ID + K2_API_CELL pair (no credentials for owner)"
        );
    }

    /// Resume splices the provider's RESUME grammar (`--resume <sid>` for
    /// claude), not the premint.
    #[test]
    fn resume_uses_resume_grammar() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-resume";
        insert_project("v1host-policy-resume", ws_path);
        let sid = SessionId::new();

        let (spawn, _) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            true,
            &ApiHostSessionRequest::default(),
        );
        let args = spawn.args.as_deref().expect("args");
        // Default ON keeps auto-approve; resume grammar is still present.
        assert!(
            args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "default ON keeps danger flag; args={args:?}"
        );
        assert!(
            args.windows(2).any(|w| w[0] == "--resume" && w[1] == sid.to_string()),
            "resume grammar must splice forced sid; args={args:?}"
        );
    }

    /// W2 preset-env merge: the resolved preset's migration-0070 env is
    /// the base layer of the host-curated env, and the K2-staged
    /// principal key OVERRIDES a same-named preset entry.
    #[test]
    fn preset_env_merges_under_principal_key() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-preset-env";
        insert_project("v1host-policy-preset-env", ws_path);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let preset_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO agent_presets \
                     (id, label, command, icon, enabled, sort_order, is_built_in, env) \
                 VALUES (?1, ?2, 'zz-env-agent', '', 1, 991, 0, \
                         '{\"ZZ_PRESET_VAR\":\"from-preset\",\"ANTHROPIC_API_KEY\":\"preset-key-must-lose\"}')",
                rusqlite::params![preset_id, format!("policy-env-{preset_id}")],
            )
            .expect("insert env preset");
            conn.execute(
                "UPDATE projects SET default_agent = ?1 WHERE path = ?2",
                rusqlite::params![preset_id, ws_path],
            )
            .expect("set default_agent");
        }

        let (spawn, _) = resolve_host_spawn(
            &api("key-env", Some("sk-ant-principal-wins")),
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(
            env.get("ZZ_PRESET_VAR").map(String::as_str),
            Some("from-preset"),
            "preset env reaches the curated map"
        );
        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-ant-principal-wins"),
            "K2-staged principal key must OVERRIDE the preset's entry"
        );
        assert_eq!(env.get("K2_API_CELL").map(String::as_str), Some("1"));
        assert!(env.get("K2_SESSION_ID").is_some());
        assert_eq!(
            env.len(),
            7,
            "preset var + principal key + agent name pair + K2_SESSION_ID + K2_API_CELL pair"
        );
    }

    /// Slice W3: on resume, the id spliced into the provider grammar is the
    /// caller's validated TARGET (`req.session`) — which for a self-minting
    /// provider (hermes) may not be UUID-shaped and thus can't equal the
    /// forced daemon SessionId.
    #[test]
    fn resume_splices_the_caller_target_not_the_forced_sid() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-target";
        insert_project("v1host-policy-target", ws_path);
        let forced = SessionId::new();

        let (spawn, _) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &forced,
            true,
            &ApiHostSessionRequest {
                session: Some("20260706_090000_abcdef".to_string()),
                ..Default::default()
            },
        );
        let args = spawn.args.as_deref().expect("args");
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--resume" && w[1] == "20260706_090000_abcdef"),
            "resume grammar must carry the provider-minted target, never the forced sid; args={args:?}"
        );
        assert!(
            !args.iter().any(|a| a == &forced.to_string()),
            "forced sid must not appear in resume argv; args={args:?}"
        );
        assert_eq!(
            spawn.forced_session_id,
            Some(forced),
            "the daemon session id stays the forced one"
        );
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(
            env.get("K2_SESSION_ID").map(String::as_str),
            Some(forced.to_string().as_str()),
            "K2_SESSION_ID is the host-session id (API sessionId / cap sub), not the provider resume target"
        );
    }

    /// W5 — an openai-provider principal stages OPENAI_API_KEY (+ the
    /// OPENAI_BASE_URL pass-through) and NOT ANTHROPIC_API_KEY, so a
    /// codex-preset workspace's agent boots authenticated over the API.
    #[test]
    fn openai_provider_key_stages_openai_env_vars() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-openai";
        insert_project("v1host-policy-openai", ws_path);

        let (spawn, _) = resolve_host_spawn(
            &api_with_provider(
                "key-oai",
                Some("sk-oai-host-key"),
                Some("openai"),
                Some("https://oai-proxy.example/v1"),
            ),
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(env.get("OPENAI_API_KEY").map(String::as_str), Some("sk-oai-host-key"));
        assert_eq!(
            env.get("OPENAI_BASE_URL").map(String::as_str),
            Some("https://oai-proxy.example/v1"),
            "base_url pass-through rides with the openai key"
        );
        assert!(
            env.get("ANTHROPIC_API_KEY").is_none(),
            "an openai key must NOT masquerade as an Anthropic credential"
        );
        assert_eq!(env.get("K2_API_CELL").map(String::as_str), Some("1"));
        assert!(env.get("K2_SESSION_ID").is_some());
        assert_eq!(
            env.len(),
            7,
            "openai pair + agent name pair + K2_SESSION_ID + K2_API_CELL pair"
        );
    }

    /// W5 — a google-provider principal stages the credential under BOTH
    /// GEMINI_API_KEY and GOOGLE_API_KEY.
    #[test]
    fn google_provider_key_stages_both_google_vars() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-google";
        insert_project("v1host-policy-google", ws_path);

        let (spawn, _) = resolve_host_spawn(
            &api_with_provider("key-goo", Some("goog-key-1"), Some("google"), None),
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        let env = spawn.env.as_ref().expect("env present");
        assert_eq!(env.get("GEMINI_API_KEY").map(String::as_str), Some("goog-key-1"));
        assert_eq!(env.get("GOOGLE_API_KEY").map(String::as_str), Some("goog-key-1"));
        assert!(env.get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(env.get("K2_API_CELL").map(String::as_str), Some("1"));
        assert!(env.get("K2_SESSION_ID").is_some());
        assert_eq!(
            env.len(),
            7,
            "google pair + agent name pair + K2_SESSION_ID + K2_API_CELL pair"
        );
    }

    /// W5 — FAIL-CLOSED: an unknown provider stages NOTHING (no guessed
    /// env var, no Anthropic fallback, never the value anywhere).
    #[test]
    fn unknown_provider_stages_nothing() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-unkprov";
        insert_project("v1host-policy-unkprov", ws_path);

        let (spawn, _) = resolve_host_spawn(
            &api_with_provider("key-unk", Some("sk-mystery"), Some("mystery-llm"), None),
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        let env = spawn.env.as_ref().expect("env present");
        assert!(
            !env.keys().any(|k| k.contains("API_KEY") || k.contains("BASE_URL")),
            "unknown provider must stage NO credential var; env keys: {:?}",
            env.keys().collect::<Vec<_>>(),
        );
        // Agent-name + D6 lifecycle markers + session id still stage (not credentials).
        assert_eq!(env.get("K2_API_CELL").map(String::as_str), Some("1"));
        assert!(env.get("K2_SESSION_ID").is_some());
        assert_eq!(
            env.len(),
            5,
            "agent name pair + K2_SESSION_ID + K2_API_CELL pair only (fail-closed on credentials)"
        );
    }

    /// A blank principal key stages no credential (never an empty assignment).
    #[test]
    fn blank_principal_key_is_dropped() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-blankkey";
        insert_project("v1host-policy-blankkey", ws_path);
        let (spawn, _) = resolve_host_spawn(
            &api("key-b", Some("   ")),
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        let env = spawn.env.as_ref().expect("env present");
        assert!(
            env.get("ANTHROPIC_API_KEY").is_none(),
            "blank principal key must not produce ANTHROPIC_API_KEY"
        );
        assert_eq!(env.get("K2_API_CELL").map(String::as_str), Some("1"));
        assert!(env.get("K2_SESSION_ID").is_some());
        assert_eq!(
            env.len(),
            5,
            "agent name pair + K2_SESSION_ID + K2_API_CELL pair only"
        );
    }

    /// Wire a workspace's default agent to a CUSTOM preset row with raw
    /// migration-0070 `danger_flags` metadata (None = NULL = unknown).
    fn configure_custom_agent(ws_path: &str, command: &str, danger_flags: Option<&str>) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let preset_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agent_presets \
                 (id, label, command, icon, enabled, sort_order, is_built_in, danger_flags) \
             VALUES (?1, ?2, ?3, '', 1, 990, 0, ?4)",
            rusqlite::params![preset_id, format!("policy-{preset_id}"), command, danger_flags],
        )
        .expect("insert custom preset");
        let rows = conn
            .execute(
                "UPDATE projects SET default_agent = ?1 WHERE path = ?2",
                rusqlite::params![preset_id, ws_path],
            )
            .expect("set default_agent");
        assert_eq!(rows, 1, "project row must exist");
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
            strip_danger_flags(args, &[]),
            vec!["--model".to_string(), "opus".to_string()],
        );
    }

    #[test]
    fn strip_danger_flags_unions_declared_with_floor() {
        let args: Vec<String> = ["--auto-yes", "--model", "fast", "--yolo"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            strip_danger_flags(args, &["--auto-yes".to_string()]),
            vec!["--model".to_string(), "fast".to_string()],
            "declared flag AND floor flag both stripped"
        );
    }

    /// Opt-out: a custom preset that DECLARES its own auto-approve flag
    /// (`--auto-yes`, not in the hardcoded floor) gets it STRIPPED.
    #[test]
    fn declared_custom_danger_flag_is_stripped_when_opted_out() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-declared-flag";
        insert_project("v1host-policy-declared-flag", ws_path);
        k2_core::workspace::settings::update_project_setting(
            ws_path,
            "api_skip_permissions",
            "0",
        )
        .expect("set opt-out");
        configure_custom_agent(
            ws_path,
            "zz-policy-agent --auto-yes --model fast",
            Some(r#"["--auto-yes"]"#),
        );

        let (spawn, _) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        assert_eq!(spawn.command.as_deref(), Some("zz-policy-agent"));
        let args = spawn.args.as_deref().expect("args");
        assert!(
            !args.iter().any(|a| a == "--auto-yes"),
            "declared danger flag must be stripped; args={args:?}"
        );
        assert!(
            args.iter().any(|a| a == "--model") && args.iter().any(|a| a == "fast"),
            "non-danger args survive byte-identically; args={args:?}"
        );
    }

    /// NULL-metadata preset + opt-out: the hardcoded floor is STILL stripped,
    /// and — the documented residual — an unaudited custom auto-approve flag
    /// survives because it cannot be known.
    #[test]
    fn null_metadata_preset_still_strips_the_floor_when_opted_out() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-null-meta";
        insert_project("v1host-policy-null-meta", ws_path);
        k2_core::workspace::settings::update_project_setting(
            ws_path,
            "api_skip_permissions",
            "0",
        )
        .expect("set opt-out");
        configure_custom_agent(
            ws_path,
            "zz-nullmeta-agent --dangerously-skip-permissions --auto-yes",
            None,
        );

        let (spawn, _) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        let args = spawn.args.as_deref().expect("args");
        assert!(
            !args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "floor flag stripped even with NULL metadata; args={args:?}"
        );
        assert!(
            args.iter().any(|a| a == "--auto-yes"),
            "HONEST RESIDUAL: an undeclared custom flag cannot be stripped \
             (this assertion documents the limitation, not an aspiration); args={args:?}"
        );
    }

    /// `api_skip_permissions=1` bypasses stripping ENTIRELY — declared
    /// custom flags AND floor flags all survive.
    #[test]
    fn opt_in_bypasses_declared_flag_stripping_too() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-policy-optin-declared";
        insert_project("v1host-policy-optin-declared", ws_path);
        configure_custom_agent(
            ws_path,
            "zz-optin-agent --auto-yes --yolo",
            Some(r#"["--auto-yes"]"#),
        );
        k2_core::workspace::settings::update_project_setting(
            ws_path,
            "api_skip_permissions",
            "1",
        )
        .expect("set opt-in");

        let (spawn, _) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest::default(),
        );
        let args = spawn.args.as_deref().expect("args");
        assert!(
            args.iter().any(|a| a == "--auto-yes") && args.iter().any(|a| a == "--yolo"),
            "opt-in keeps every flag; args={args:?}"
        );
    }

    #[test]
    fn dims_are_clamped() {
        assert_eq!(clamp_dim(Some(5), 80, 16, 500), 16);
        assert_eq!(clamp_dim(Some(9999), 24, 4, 300), 300);
        assert_eq!(clamp_dim(None, 80, 16, 500), 80);
    }

    // ── W4 — the injection-profile resolution seam ───────────────────

    /// `configure_custom_agent` variant that also stamps raw
    /// migration-0070 `readiness` metadata onto the preset row.
    fn configure_custom_agent_readiness(ws_path: &str, command: &str, readiness: Option<&str>) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let preset_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agent_presets \
                 (id, label, command, icon, enabled, sort_order, is_built_in, readiness) \
             VALUES (?1, ?2, ?3, '', 1, 990, 0, ?4)",
            rusqlite::params![preset_id, format!("policy-{preset_id}"), command, readiness],
        )
        .expect("insert custom preset");
        let rows = conn
            .execute(
                "UPDATE projects SET default_agent = ?1 WHERE path = ?2",
                rusqlite::params![preset_id, ws_path],
            )
            .expect("set default_agent");
        assert_eq!(rows, 1, "project row must exist");
    }

    /// Level 1: a preset-DECLARED settle wins even for a binary the
    /// static table would poll (and for one it's never studied).
    #[test]
    fn injection_profile_preset_settle_beats_static_table() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-profile-settle";
        insert_project("v1host-profile-settle", ws_path);
        configure_custom_agent_readiness(ws_path, "zz-custom-agent --fast", Some("settle:2500"));

        let p = resolve_host_injection_profile(ws_path);
        assert!(!p.ready_via_bracketed_paste, "declared settle must disable the paste poll");
        assert_eq!(p.post_spawn_settle, std::time::Duration::from_millis(2500));
    }

    /// Level 2: malformed readiness metadata degrades to the STATIC
    /// provider table (here: codex, a studied ?2004h liar), never to a
    /// guessed dialect and never to a panic.
    #[test]
    fn injection_profile_malformed_readiness_degrades_to_static_table() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-profile-malformed";
        insert_project("v1host-profile-malformed", ws_path);
        configure_custom_agent_readiness(ws_path, "codex", Some("settle:not-a-number"));

        let p = resolve_host_injection_profile(ws_path);
        assert_eq!(
            p,
            k2_core::workspace::provider_resume::injection_profile_for_provider("codex"),
            "malformed metadata must resolve exactly the codex table entry"
        );
        assert!(!p.ready_via_bracketed_paste);
    }

    /// Level 3: NULL metadata + a provider the table never studied =
    /// the claude-shaped poll default (today's behavior, unchanged).
    #[test]
    fn injection_profile_unknown_everything_is_the_poll_default() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-profile-default";
        insert_project("v1host-profile-default", ws_path);
        configure_custom_agent_readiness(ws_path, "zz-unstudied-agent", None);

        assert_eq!(
            resolve_host_injection_profile(ws_path),
            k2_core::workspace::provider_resume::DEFAULT_INJECTION_PROFILE
        );
    }

    // ── Launch-param prompt (prd-host-session-launch-param-prompt-v1) ─

    /// A1: Claude cold spawn attaches interactive initial message; no
    /// `--print`. Identity args stay durable; exec_args carries prompt.
    #[test]
    fn launch_param_claude_cold_attaches_no_print() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-lp-claude-cold";
        insert_project("v1host-lp-claude-cold", ws_path);
        let sid = SessionId::new();
        let marker = "CALLER-COLD-PROMPT-MARKER-A1";

        let (spawn, plan) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            false,
            &ApiHostSessionRequest {
                prompt: Some(marker.to_string()),
                ..Default::default()
            },
        );
        assert!(plan.attached, "claude must attach launch-param");
        let identity = spawn.args.as_deref().expect("identity args");
        let exec = spawn.exec_args.as_deref().expect("exec args");
        // Durable identity has NO caller prompt (A8 / fire-once).
        assert!(
            !identity.iter().any(|a| a.contains(marker)),
            "identity args must not hold prompt; got {identity:?}"
        );
        let guest = k2_core::workspace::settings::get_api_guest_policy(ws_path);
        let expected =
            crate::v1_host_sessions::compose_spawn_inject(&guest, marker);
        assert_eq!(
            exec.last().map(String::as_str),
            Some(expected.as_str()),
            "exec trailing arg is the full spawn stack; exec={exec:?}"
        );
        assert!(
            expected.contains(crate::v1_host_sessions::API_SPAWN_PREAMBLE),
            "spawn stack must include respond preamble (A9)"
        );
        assert!(!exec.iter().any(|a| a == "--print" || a == "-p"));
        assert!(
            identity
                .windows(2)
                .any(|w| w[0] == "--session-id" && w[1] == sid.to_string()),
            "premint identity present; identity={identity:?}"
        );
    }

    /// A2: Claude dead resume uses `--resume` SSOT + next prompt.
    #[test]
    fn launch_param_claude_resume_ssot_plus_prompt() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-lp-claude-resume";
        insert_project("v1host-lp-claude-resume", ws_path);
        let sid = SessionId::new();
        let marker = "CALLER-RESUME-PROMPT-MARKER-A2";

        let (spawn, plan) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            true,
            &ApiHostSessionRequest {
                session: Some(sid.to_string()),
                prompt: Some(marker.to_string()),
                ..Default::default()
            },
        );
        assert!(plan.attached);
        let identity = spawn.args.as_deref().expect("identity");
        let exec = spawn.exec_args.as_deref().expect("exec");
        assert!(
            identity
                .windows(2)
                .any(|w| w[0] == "--resume" && w[1] == sid.to_string()),
            "resume grammar; identity={identity:?}"
        );
        assert!(
            !identity.iter().any(|a| a.contains(marker)),
            "fire-once: identity has no prompt"
        );
        assert!(
            exec.last().map(|s| s.contains(marker)).unwrap_or(false),
            "exec has prompt; exec={exec:?}"
        );
        assert!(!exec.iter().any(|a| a == "--print"));
    }

    /// A6 / Codex: `resume <id> <prompt>` subcommand shape.
    #[test]
    fn launch_param_codex_resume_subcommand_plus_prompt() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-lp-codex";
        insert_project("v1host-lp-codex", ws_path);
        configure_custom_agent(ws_path, "codex", None);
        let sid = SessionId::new();
        let marker = "CODEX-RESUME-PROMPT";

        let (spawn, plan) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            true,
            &ApiHostSessionRequest {
                session: Some(sid.to_string()),
                prompt: Some(marker.to_string()),
                ..Default::default()
            },
        );
        assert!(plan.attached, "codex supports launch-param");
        let identity = spawn.args.as_deref().expect("identity");
        let exec = spawn.exec_args.as_deref().expect("exec");
        assert_eq!(
            identity.get(0).map(String::as_str),
            Some("resume"),
            "codex identity={identity:?}"
        );
        assert_eq!(identity.get(1).map(String::as_str), Some(sid.to_string().as_str()));
        assert!(
            !identity.iter().any(|a| a.contains(marker)),
            "identity fire-once"
        );
        assert!(exec.last().map(|s| s.contains(marker)).unwrap_or(false));
        assert!(!exec.iter().any(|a| a == "exec"));
    }

    /// A6 / Pi: dead resume uses `--session`, never `--continue`.
    #[test]
    fn launch_param_pi_session_not_continue() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-lp-pi";
        insert_project("v1host-lp-pi", ws_path);
        configure_custom_agent(ws_path, "pi", None);
        let sid = SessionId::new();
        let marker = "PI-PROMPT";

        let (spawn, plan) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            true,
            &ApiHostSessionRequest {
                session: Some(sid.to_string()),
                prompt: Some(marker.to_string()),
                ..Default::default()
            },
        );
        assert!(plan.attached);
        let identity = spawn.args.as_deref().expect("identity");
        let exec = spawn.exec_args.as_deref().expect("exec");
        assert!(
            identity
                .windows(2)
                .any(|w| w[0] == "--session" && w[1] == sid.to_string()),
            "pi --session; identity={identity:?}"
        );
        assert!(!identity.iter().any(|a| a == "--continue"));
        assert!(!exec.iter().any(|a| a == "--continue"));
        assert!(exec.last().map(|s| s.contains(marker)).unwrap_or(false));
    }

    /// Hermes / unknown → attached=false (inject fallback D10).
    #[test]
    fn launch_param_hermes_falls_back_to_inject() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-lp-hermes";
        insert_project("v1host-lp-hermes", ws_path);
        configure_custom_agent(ws_path, "hermes", None);
        let sid = SessionId::new();

        let (spawn, plan) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &sid,
            false,
            &ApiHostSessionRequest {
                prompt: Some("hermes cannot take argv prompt".into()),
                ..Default::default()
            },
        );
        assert!(!plan.attached, "hermes must inject-fallback");
        assert!(spawn.exec_args.is_none());
        let identity = spawn.args.as_deref().unwrap_or(&[]);
        assert!(!identity.iter().any(|a| a.contains("hermes cannot")));
    }

    /// Empty prompt → no launch-param, no exec_args.
    #[test]
    fn launch_param_empty_prompt_not_attached() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-lp-empty";
        insert_project("v1host-lp-empty", ws_path);
        let (spawn, plan) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest {
                prompt: Some("   ".into()),
                ..Default::default()
            },
        );
        assert!(!plan.attached);
        assert!(spawn.exec_args.is_none());
    }

    /// Fire-once: durable args never hold the launch string even when
    /// attached (register / recovery replay safety — A8 / A10).
    #[test]
    fn launch_param_fire_once_identity_excludes_prompt() {
        k2_core::db::init_for_tests();
        let ws_path = "/tmp/k2-v1host-lp-fireonce";
        insert_project("v1host-lp-fireonce", ws_path);
        let secret = "NEVER-IN-ARGS-JSON-OR-RECOVERY";
        let (spawn, plan) = resolve_host_spawn(
            &V1Principal::Owner,
            ws_path,
            &SessionId::new(),
            false,
            &ApiHostSessionRequest {
                prompt: Some(secret.into()),
                ..Default::default()
            },
        );
        assert!(plan.attached);
        let identity_json = serde_json::to_string(spawn.args.as_ref().unwrap()).unwrap();
        assert!(
            !identity_json.contains(secret),
            "args_json snapshot must not contain prompt: {identity_json}"
        );
        // Simulated recovery input is identity only.
        let recovered: Vec<String> =
            serde_json::from_str(&identity_json).expect("round-trip identity");
        assert!(!recovered.iter().any(|a| a.contains(secret)));
    }
}
