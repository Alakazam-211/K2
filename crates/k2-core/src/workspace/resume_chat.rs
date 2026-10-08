//! Resume-chat argument resolver — daemon-owned per the daemon-first
//! architecture invariant (`feedback_daemon_first.md`).
//!
//! The pinned chat tab in K2SO and any other thin client that wants to
//! attach to a workspace's canonical agent session calls into this
//! helper to get the right command + args. Three cases (each speaking
//! the resolved provider's own dialect via the Slice-3
//! [`crate::workspace::provider_resume`] adapter — Claude behavior is
//! byte-identical to pre-Slice-3):
//!
//! 1. **A saved session_id exists in `workspace_sessions` AND the
//!    provider's on-disk conversation is present** — return the
//!    adapter's resume grammar (`claude --resume <id>`,
//!    `pi --session <id>`, `codex resume <id>`, …) so the same
//!    conversation continues. The user's chat history is intact.
//! 2. **The saved id's on-disk conversation is gone, BUT the workspace
//!    has another real session on disk** (workspace remove+readd,
//!    manual SQL clear, a never-run pre-allocation left over from an
//!    earlier mint, OR a reused pinned-chat PTY that's actively running
//!    a bare-spawned session) — resume the **most-recently-active**
//!    on-disk session and persist its id, instead of minting a fresh
//!    one. This is the GH#24 convergence fix: minting + overwriting an
//!    unconfirmed `--session-id` that a *reused* PTY never runs left
//!    its JSONL absent forever, so every resolve re-minted — an endless
//!    loop on remote/companion clients (they re-ask on each reconnect).
//! 3. **No saved session AND no on-disk session at all** (a genuinely
//!    brand-new workspace):
//!    - Providers with a PREMINT style (claude `--session-id <uuid>`;
//!      grok `--session-id <uuid>`, new-sessions-only per the storage
//!      study): pre-allocate a fresh UUID, persist it via
//!      `workspace_sessions.session_id` BEFORE the agent spawns (so
//!      v2_spawn's auto-stamp hook can match it against argv), then
//!      return `<premint-flag> <uuid>`. The session is "pre-decided" —
//!      the pinned tab and any subsequent attach see the same UUID.
//!    - Providers WITHOUT one (pi/codex/gemini/cursor mint their own
//!      ids): spawn bare and return
//!      [`ResumeChatArgs::pending_session_discovery`] `= true` so the
//!      caller runs the post-hoc adoption helper
//!      (`provider_resume::defer_adopt_discovered_session`) shortly
//!      after spawn. Only the harness is persisted up front.
//!
//! ## Which agent? (Slice 3 command resolution)
//!
//! - **RESUMING an existing canonical session** (cases 1–2 with a
//!   saved row): the stored `workspace_sessions.harness` picks the
//!   adapter — the canonical session's agent may legitimately differ
//!   from the workspace default (owner decision, de-generalization
//!   research §scope).
//! - **A NEW session** (no saved row / unknown harness on the auto
//!   path): `agent_resolve::resolve_agent_command` (workspace default →
//!   global default → claude) governs.
//! - **Unknown provider** (any arbitrary custom command): degrade
//!   to a fresh bare spawn with no resume/premint flags — exactly the
//!   Slice-2 `// Slice 3:` gate behavior.
//!
//! `--dangerously-skip-permissions` stays CLAUDE-ONLY: when the
//! resolved command is claude, base args are pinned to exactly
//! `["--dangerously-skip-permissions"]` (byte-identical to the
//! pre-Slice-3 hardcode, even against a customized Claude preset —
//! this resolver never consumed preset args for claude and still
//! doesn't). Other providers get their preset args verbatim (grok's
//! seeded preset already carries `--always-approve`; nothing is
//! invented).
//!
//! ## Explicit selection (Issue B — daemon-multi-client-arbitration §6)
//!
//! When the user picks a different chat from the pinned-tab dropdown,
//! the renderer persists the choice via `set-chat-session` and re-ensures
//! with `explicitSelection: true`. In that mode the saved `session_id`
//! is the user's *authoritative gesture* — the resolver returns
//! the resume grammar for `<saved>` and **skips the case-2 converge
//! fallback** so an explicit pick is never silently reverted to the
//! newest on-disk session. If the chosen id's conversation is genuinely
//! gone, the resolver returns an `Err` (surfaced as a toast) rather
//! than swapping to a different conversation. The auto path (cold
//! mount, restart-recovery, CLI) keeps the converge fallback, so GH#24
//! stays fixed.
//!
//! Lifted from `src-tauri/src/commands/k2so_agents.rs::k2so_agents_resume_chat_args`
//! (which was a Tauri-side command pre-0.37.5). Moving it to k2so-core
//! means:
//!
//!   - The daemon's `/cli/workspace/resume-chat-args` route calls it
//!     directly (canonical thin-client surface — Tauri proxies through
//!     this route via HTTP, mobile companion + future MCP do the same).
//!   - CLI verb `k2so workspace resume-chat-args <ws>` calls the same
//!     route.
//!   - Tests can exercise the logic without booting Tauri.

use rusqlite::params;

use crate::workspace::agent_resolve::{resolve_agent_command, ResolvedAgentCommand};
use crate::workspace::provider_resume::{
    provider_resume_for_command, provider_resume_for_provider, ProviderResume,
};

/// Resolved launch config for a thin client to spawn the workspace's
/// agent and attach to its canonical session.
#[derive(Debug, Clone)]
pub struct ResumeChatArgs {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    /// The session UUID we're resuming OR pre-allocating. Empty ONLY
    /// when the provider mints its own ids
    /// (`pending_session_discovery`) or is unknown to the adapter
    /// table (bare spawn, no session identity).
    pub resume_session: String,
    /// `true` when the saved session_id was usable (conversation on
    /// disk), `false` when we pre-allocated a fresh UUID / spawned
    /// bare.
    pub resumed_existing: bool,
    /// Provider key governing this launch — the `harness` value
    /// written to / read from `workspace_sessions` ("claude", "grok",
    /// "pi", …; the command's first token for unknown providers).
    pub provider: String,
    /// `true` when the provider mints its own session ids and the id
    /// must be adopted POST-HOC: the caller spawns bare, then runs
    /// `provider_resume::defer_adopt_discovered_session(provider, path)`
    /// shortly after spawn to stamp `workspace_sessions.session_id` +
    /// `harness` with the id the agent actually created on disk.
    /// Always `false` for claude/grok (premint) and unknown providers
    /// (nothing to discover).
    pub pending_session_discovery: bool,
    /// W2 (0.40.30): the spawning preset's env map (migration 0070
    /// `agent_presets.env`). Merged into the child env by the DAEMON
    /// spawn sites; deliberately EXCLUDED from [`ResumeChatArgs::to_json`]
    /// — env values may hold credentials and must never cross the wire
    /// or reach a log line. `None` = no preset env.
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// W4 (0.40.30): the spawning preset's RAW `readiness` metadata
    /// (migration 0070 `agent_presets.readiness`). Consumed by the
    /// daemon wake injector through the ONE shared precedence chain
    /// (`provider_resume::resolve_injection_profile`: preset metadata →
    /// static provider table → default). Deliberately EXCLUDED from
    /// [`ResumeChatArgs::to_json`] — the wire shape is frozen and only
    /// daemon injection sites consume this. `None` = unknown/legacy.
    pub readiness: Option<String>,
}

impl ResumeChatArgs {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "command": self.command,
            "args": self.args,
            "cwd": self.cwd,
            "resumeSession": self.resume_session,
            "resumedExisting": self.resumed_existing,
            "provider": self.provider,
            "pendingSessionDiscovery": self.pending_session_discovery,
        })
    }
}

/// Build the resume-chat args for a workspace (auto path — no explicit
/// user gesture). Thin wrapper over [`resolve_resume_chat_args_ex`] with
/// `explicit_selection = false`, preserving the GH#24 converge fallback
/// and the #681 brand-new mint. Existing callers (restart-recovery, CLI
/// verb, Tauri proxy) keep their behavior unchanged.
pub fn resolve_resume_chat_args(project_path: &str) -> Result<ResumeChatArgs, String> {
    resolve_resume_chat_args_ex(project_path, false)
}

/// Build the resume-chat args for a workspace.
///
/// **Daemon-first.** This helper has no Tauri / IPC dependency; both
/// the daemon's HTTP route and Tauri's thin proxy command call into
/// it. The CLI verb hits the same daemon route.
///
/// - `project_path`: filesystem path to the workspace (used as `cwd`
///   for the spawned agent and as the lookup key into `projects.path`).
/// - `explicit_selection`: `true` ONLY when the user made an explicit
///   dropdown session pick (Issue B, daemon-multi-client-arbitration
///   PRD §6). When `true`, the saved `workspace_sessions.session_id` is
///   the user's authoritative choice: the resolver returns the resume
///   grammar for `<saved>` and **SKIPS the GH#24 converge fallback** so
///   an explicit pick can never be silently reverted to the newest
///   on-disk session. If that id's conversation is genuinely missing,
///   it returns a clear `Err` rather than swapping to a different
///   session. When `false` (the auto path — cold mount,
///   restart-recovery, CLI), the GH#24 converge fallback + #681
///   brand-new mint stay intact.
/// - Returns `Err` on DB I/O failures, and (only when
///   `explicit_selection`) when the explicitly-chosen id is missing on
///   disk or its stored harness maps to no known provider. The
///   auto-path "no saved session" case is a SUCCESS that returns
///   pre-allocated / bare-spawn args.
pub fn resolve_resume_chat_args_ex(
    project_path: &str,
    explicit_selection: bool,
) -> Result<ResumeChatArgs, String> {
    // Identity snapshot + the workspace's resolved default agent
    // (levels 2–4: projects.default_agent → global → claude). One
    // scoped lock; the adapter's fs probes below run unlocked.
    let (project_id, saved_session, saved_harness, default_cmd) = {
        let db = crate::db::shared();
        let conn = db.lock();
        let project_id: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE path = ?1",
                params![project_path],
                |row| row.get(0),
            )
            .ok();
        let row = project_id.as_ref().and_then(|pid| {
            crate::db::schema::WorkspaceSession::get(&conn, pid)
                .ok()
                .flatten()
        });
        let saved_session = row
            .as_ref()
            .and_then(|r| r.session_id.clone())
            .filter(|s| !s.is_empty());
        let saved_harness = row.map(|r| r.harness);
        let default_cmd = resolve_agent_command(&conn, project_path);
        (project_id, saved_session, saved_harness, default_cmd)
    };

    // ── Case 1: saved session id — the stored HARNESS picks the
    // adapter (the canonical session's agent wins over the workspace
    // default; migration-0039 rows default to 'claude', preserving
    // every pre-Slice-3 workspace).
    if let Some(ref sid) = saved_session {
        let harness = saved_harness.as_deref().unwrap_or("claude");
        match provider_resume_for_provider(harness) {
            Some(adapter) => {
                if adapter.session_file_exists(sid, project_path) {
                    let (command, base_args, env, readiness) =
                        spawn_command_for(adapter, &default_cmd);
                    let args = adapter.resume_args(&base_args, sid);
                    return Ok(ResumeChatArgs {
                        command,
                        args,
                        cwd: project_path.to_string(),
                        resume_session: sid.clone(),
                        resumed_existing: true,
                        provider: adapter.provider.to_string(),
                        pending_session_discovery: false,
                        env,
                        readiness,
                    });
                }

                // Issue B (daemon-multi-client-arbitration §6): EXPLICIT
                // selection wins. The user picked this id from the dropdown —
                // it is an authoritative gesture, not a possibly-stale saved
                // id. We reached here because the exists-check was false,
                // which on the auto path falls through to the converge
                // fallback below and would silently revert the pick to the
                // newest on-disk session (the no-op the user reported). On
                // the explicit path we must NOT do that: surface a clear
                // error so the renderer can toast, rather than swapping to a
                // different conversation behind the user's back.
                if explicit_selection {
                    return Err(format!(
                        "selected chat session {sid} has no conversation on disk for this workspace; \
                         not switching to a different session"
                    ));
                }

                // Auto path: converge/mint WITHIN the saved harness's
                // provider (a stale grok pick converges to the newest grok
                // session, never to a claude one).
                return converge_or_mint(
                    adapter,
                    &default_cmd,
                    project_id.as_deref(),
                    project_path,
                );
            }
            None => {
                // Unknown harness (a custom command's token, or corrupt
                // data). We cannot verify or resume it. Explicit pick →
                // loud error; auto path → fall through and let the
                // workspace default govern (below).
                if explicit_selection {
                    return Err(format!(
                        "selected chat session {sid} belongs to unknown provider '{harness}'; \
                         cannot resume it"
                    ));
                }
                crate::log_debug!(
                    "[core/resume-chat] saved session {} for {} has unknown harness {:?}; \
                     falling through to the workspace default agent",
                    sid,
                    project_path,
                    harness
                );
            }
        }
    }

    // ── Cases 2–3: no usable saved session — the workspace default
    // agent governs.
    match provider_resume_for_command(&default_cmd.command) {
        Some(adapter) => {
            converge_or_mint(adapter, &default_cmd, project_id.as_deref(), project_path)
        }
        None => {
            // Unknown provider: fresh bare spawn, no resume, no premint —
            // the Slice-2 degraded behavior, now with a truthful harness
            // stamp (the command's first token) so the row never lies
            // 'claude' about some custom agent's spawn.
            let provider = default_cmd
                .command
                .rsplit('/')
                .next()
                .unwrap_or(&default_cmd.command)
                .to_string();
            persist_session_identity(project_id.as_deref(), None, &provider);
            Ok(ResumeChatArgs {
                command: default_cmd.command.clone(),
                args: default_cmd.args.clone(),
                cwd: project_path.to_string(),
                resume_session: String::new(),
                resumed_existing: false,
                provider,
                pending_session_discovery: false,
                env: default_cmd.env.clone(),
                readiness: default_cmd.readiness.clone(),
            })
        }
    }
}

/// Cases 2 (GH#24 converge-to-newest) and 3 (brand-new premint /
/// bare-spawn-with-pending-discovery) for a known provider.
fn converge_or_mint(
    adapter: &'static ProviderResume,
    default_cmd: &ResolvedAgentCommand,
    project_id: Option<&str>,
    project_path: &str,
) -> Result<ResumeChatArgs, String> {
    let (command, base_args, env, readiness) = spawn_command_for(adapter, default_cmd);

    // GH#24 convergence fix. The saved id's conversation is missing (a
    // never-run pre-allocation, a workspace remove+readd, a manual
    // clear) — but the workspace may still have a REAL prior session on
    // disk: the one a reused/live pinned-chat PTY is actually running,
    // or the user's last conversation. Resume the most-recently-active
    // one and persist it, instead of minting a throwaway premint id.
    //
    // Why this is the root fix: when the canonical PTY is REUSED
    // (`reused=true`), the agent is NOT re-spawned, so a freshly-minted
    // `--session-id <new>` never gets written to disk. The old code still
    // overwrote `workspace_sessions.session_id = <new>`, so the next
    // resolve saw "saved id, no JSONL" → minted AGAIN. Remote/companion
    // clients re-request resume-args on every reconnect, turning that into
    // an unbounded re-mint / re-resume loop. Resuming the real on-disk
    // session makes the resolver converge: the persisted id now passes the
    // adapter's exists check above on the next call.
    if let Some(existing) = adapter.newest_on_disk(project_path) {
        persist_session_identity(project_id, Some(&existing), adapter.provider);
        let args = adapter.resume_args(&base_args, &existing);
        return Ok(ResumeChatArgs {
            command,
            args,
            cwd: project_path.to_string(),
            resume_session: existing,
            resumed_existing: true,
            provider: adapter.provider.to_string(),
            pending_session_discovery: false,
            env,
            readiness,
        });
    }

    // Genuinely brand-new workspace: no saved session AND nothing on
    // disk.
    match adapter.premint_args(&base_args, "") {
        Some(_) => {
            // Premint provider (claude/grok): pre-allocate the session
            // UUID and pin it via the premint flag. Persist to
            // workspace_sessions.session_id BEFORE the spawn so
            // v2_spawn's auto-stamp hook sees the matching id in argv.
            let new_sid = uuid::Uuid::new_v4().to_string();
            persist_session_identity(project_id, Some(&new_sid), adapter.provider);
            let args = adapter
                .premint_args(&base_args, &new_sid)
                .expect("premint style checked above");
            Ok(ResumeChatArgs {
                command,
                args,
                cwd: project_path.to_string(),
                resume_session: new_sid,
                resumed_existing: false,
                provider: adapter.provider.to_string(),
                pending_session_discovery: false,
                env,
                readiness: readiness.clone(),
            })
        }
        None => {
            // The provider mints its own ids (pi/codex/gemini/cursor):
            // spawn bare, stamp only the harness now, and signal the
            // caller to adopt the discovered id post-hoc
            // (provider_resume::defer_adopt_discovered_session).
            persist_session_identity(project_id, None, adapter.provider);
            Ok(ResumeChatArgs {
                command,
                args: base_args,
                cwd: project_path.to_string(),
                resume_session: String::new(),
                resumed_existing: false,
                provider: adapter.provider.to_string(),
                pending_session_discovery: true,
                env,
                readiness,
            })
        }
    }
}

/// The spawn command + BASE args (before resume/premint grammar) for a
/// provider:
///
/// 1. If the workspace's resolved default agent already speaks this
///    provider, use its command + args (path-qualified binaries and
///    user preset customization survive).
/// 2. Else scan the enabled preset roster (display order) for one
///    whose command maps to the provider — the "canonical session's
///    agent differs from the workspace default" case keeps the user's
///    preset args for that agent.
/// 3. Else fall back to the adapter's bare binary.
///
/// CLAUDE INVARIANT: whenever the chosen command is claude, base args
/// are pinned to exactly `["--dangerously-skip-permissions"]` — the
/// pre-Slice-3 hardcode. This resolver never consumed Claude preset
/// args and still doesn't (byte-identical argv guarantee); the flag
/// stays claude-only, other providers get their preset args verbatim.
fn spawn_command_for(
    adapter: &'static ProviderResume,
    default_cmd: &ResolvedAgentCommand,
) -> (
    String,
    Vec<String>,
    Option<std::collections::BTreeMap<String, String>>,
    Option<String>,
) {
    // 1. Workspace/global default already speaks this provider.
    if provider_resume_for_command(&default_cmd.command).map(|a| a.provider)
        == Some(adapter.provider)
    {
        return (
            default_cmd.command.clone(),
            claude_pinned_or(&default_cmd.command, &default_cmd.args),
            default_cmd.env.clone(),
            default_cmd.readiness.clone(),
        );
    }

    // 2. Enabled preset roster, display order (same ordering
    //    agent_resolve uses, so both sides pick the same row). Carries
    //    the matched row's migration-0070 env JSON so a harness-picked
    //    preset spawns with ITS OWN env, not the workspace default's.
    let roster: Vec<(String, Option<String>, Option<String>)> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.prepare(
            "SELECT command, env, readiness FROM agent_presets \
             WHERE enabled = 1 ORDER BY sort_order, label",
        )
        .and_then(|mut stmt| {
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?
                .flatten()
                .collect::<Vec<(String, Option<String>, Option<String>)>>();
            Ok(rows)
        })
        .unwrap_or_default()
    };
    for (command_str, env_json, readiness) in roster {
        if provider_resume_for_command(&command_str).map(|a| a.provider) == Some(adapter.provider) {
            let (command, args) =
                crate::workspace::agent_resolve::parse_command_string(&command_str);
            if !command.is_empty() {
                let args = claude_pinned_or(&command, &args);
                // Malformed env JSON degrades to None (never a panic in
                // a spawn path); values are never logged.
                let env = env_json
                    .as_deref()
                    .and_then(|raw| serde_json::from_str(raw).ok());
                return (command, args, env, readiness);
            }
        }
    }

    // 3. Adapter default binary — no preset row, no preset env, no
    //    readiness metadata (the static provider table speaks for the
    //    adapter's own studied binary).
    let command = adapter.command.to_string();
    let args = claude_pinned_or(&command, &[]);
    (command, args, None, None)
}

/// See [`spawn_command_for`]'s CLAUDE INVARIANT.
fn claude_pinned_or(command: &str, args: &[String]) -> Vec<String> {
    let is_claude = command
        .rsplit('/')
        .next()
        .map(|name| name == "claude")
        .unwrap_or(false);
    if is_claude {
        vec!["--dangerously-skip-permissions".to_string()]
    } else {
        args.to_vec()
    }
}

/// Drop resume / premint / fork tokens and the following id. Fresh
/// continue must not inherit a preset that already names a session.
fn strip_session_argv(args: &[String]) -> Vec<String> {
    const FLAGS: &[&str] = &[
        "--resume",
        "-r",
        "--continue",
        "-c",
        "--session",
        "--session-id",
        "--fork-session",
    ];
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let tok = args[i].as_str();
        if FLAGS.contains(&tok) || tok == "resume" {
            i += 1;
            if i < args.len() && !args[i].starts_with('-') {
                i += 1;
            }
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

/// Never-chatted argv for `target_provider`. Does not resume a saved id
/// and does not converge to the newest file on disk.
///
/// Premint harnesses (claude, grok) persist a new uuid and return
/// `--session-id <new>`. Self-minting harnesses (pi, codex, gemini,
/// cursor, hermes) clear the saved id — [`persist_session_identity`]
/// with `None` would keep it — and return bare preset args with
/// `pending_session_discovery`.
pub fn resolve_never_chatted_chat_args(
    project_path: &str,
    target_provider: &str,
) -> Result<ResumeChatArgs, String> {
    let provider = target_provider.trim();
    if provider.is_empty() {
        return Err("target provider required".to_string());
    }
    let adapter = provider_resume_for_provider(provider)
        .ok_or_else(|| format!("unknown harness: {provider}"))?;

    let (project_id, default_cmd) = {
        let db = crate::db::shared();
        let conn = db.lock();
        let project_id: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE path = ?1",
                params![project_path],
                |row| row.get(0),
            )
            .ok();
        let default_cmd = resolve_agent_command(&conn, project_path);
        (project_id, default_cmd)
    };
    if project_id.is_none() {
        return Err(format!("project not registered: {project_path}"));
    }

    let (command, base_args, env, readiness) = spawn_command_for(adapter, &default_cmd);
    let base_args = strip_session_argv(&base_args);

    match adapter.premint_args(&base_args, "") {
        Some(_) => {
            let new_sid = uuid::Uuid::new_v4().to_string();
            persist_session_identity(project_id.as_deref(), Some(&new_sid), adapter.provider);
            let args = adapter
                .premint_args(&base_args, &new_sid)
                .expect("premint style checked above");
            Ok(ResumeChatArgs {
                command,
                args,
                cwd: project_path.to_string(),
                resume_session: new_sid,
                resumed_existing: false,
                provider: adapter.provider.to_string(),
                pending_session_discovery: false,
                env,
                readiness,
            })
        }
        None => {
            clear_session_identity(project_id.as_deref(), adapter.provider);
            Ok(ResumeChatArgs {
                command,
                args: base_args,
                cwd: project_path.to_string(),
                resume_session: String::new(),
                resumed_existing: false,
                provider: adapter.provider.to_string(),
                pending_session_discovery: true,
                env,
                readiness,
            })
        }
    }
}

/// Stamp harness and clear `session_id`. [`persist_session_identity`]
/// with `None` keeps the previous id; fresh continue must not.
fn clear_session_identity(project_id: Option<&str>, harness: &str) {
    let Some(pid) = project_id else { return };
    let db = crate::db::shared();
    let conn = db.lock();
    let row_id = uuid::Uuid::new_v4().to_string();
    let _ = conn.execute(
        "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
         VALUES (?1, ?2, NULL, ?3, 'user', 'running', unixepoch()) \
         ON CONFLICT(project_id) DO UPDATE SET session_id = NULL, harness = ?3, last_activity_at = unixepoch()",
        params![row_id, pid, harness],
    );
}

/// Best-effort identity persist (mirrors the pre-Slice-3 upserts, now
/// with a TRUTHFUL harness instead of a hardcoded 'claude'):
///
/// - `session_id = Some(_)` → stamp id + harness (converge + premint
///   cases).
/// - `session_id = None`    → stamp harness only, PRESERVING any
///   existing session_id (bare-spawn / unknown-provider cases — the
///   post-hoc adoption helper writes the id later).
///
/// A `None` project_id (unregistered workspace) no-ops, matching the
/// old behavior.
fn persist_session_identity(project_id: Option<&str>, session_id: Option<&str>, harness: &str) {
    let Some(pid) = project_id else { return };
    let db = crate::db::shared();
    let conn = db.lock();
    let row_id = uuid::Uuid::new_v4().to_string();
    let _ = match session_id {
        Some(sid) => conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'user', 'running', unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET session_id = ?3, harness = ?4, last_activity_at = unixepoch()",
            params![row_id, pid, sid, harness],
        ),
        None => conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, NULL, ?3, 'user', 'running', unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET harness = ?3, last_activity_at = unixepoch()",
            params![row_id, pid, harness],
        ),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// HOME guard: controlled on-disk provider stores + a hermetic
    /// `~/.k2/settings.json` (absent → global default_agent = "claude").
    /// Serialized via the ONE crate-wide env lock (held by the inner
    /// `test_env::TempHome`, which restores HOME and removes the dir on
    /// drop). Take it before the DB lock (lock order: env first).
    struct HomeGuard {
        home: PathBuf,
        _temp: crate::test_env::TempHome,
    }

    impl HomeGuard {
        fn new(_label: &str) -> Self {
            let temp = crate::test_env::TempHome::new();
            Self {
                home: temp.path().to_path_buf(),
                _temp: temp,
            }
        }
    }

    fn insert_project(path: &str, default_agent: Option<&str>) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path, default_agent) VALUES (?1, 'test', ?2, ?3)",
            params![id, path, default_agent],
        )
        .expect("insert project");
        id
    }

    fn insert_preset(command: &str, sort_order: i64) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, ?2, ?3, '', 1, ?4, 0)",
            params![id, format!("test-{id}"), command, sort_order],
        )
        .expect("insert preset");
        id
    }

    fn set_saved_session(project_id: &str, session_id: &str, harness: &str) {
        let db = crate::db::shared();
        let conn = db.lock();
        let row_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'user', 'running', unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET session_id = ?3, harness = ?4",
            params![row_id, project_id, session_id, harness],
        )
        .unwrap();
    }

    fn saved_row(project_id: &str) -> Option<(Option<String>, String)> {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::db::schema::WorkspaceSession::get(&conn, project_id)
            .unwrap()
            .map(|r| (r.session_id, r.harness))
    }

    fn write_claude_session(home: &std::path::Path, project_path: &str, session_id: &str) {
        let hash = crate::chat_history::claude_project_hash(
            crate::chat_history::resolve_root_project_path(project_path),
        );
        let dir = home.join(".claude").join("projects").join(&hash);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{session_id}.jsonl")),
            b"{\"cwd\":\"/x\"}\n",
        )
        .unwrap();
    }

    fn write_grok_session(home: &std::path::Path, project_path: &str, session_id: &str) {
        let dir = home
            .join(".grok")
            .join("sessions")
            .join("%2Ffixture")
            .join(session_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("summary.json"),
            serde_json::json!({
                "info": { "id": session_id, "cwd": project_path },
                "last_active_at": "2026-07-03T10:00:00Z",
            })
            .to_string(),
        )
        .unwrap();
    }

    // ── CLAUDE REGRESSION GUARD: byte-identical argv, all 3 cases ────
    //
    // Pre-Slice-3, this resolver hardcoded `claude` and produced:
    //   case 1: ["--dangerously-skip-permissions", "--resume", <saved>]
    //   case 2: ["--dangerously-skip-permissions", "--resume", <newest>]
    //   case 3: ["--dangerously-skip-permissions", "--session-id", <new>]
    // These pins fail on ANY drift — order, extra flags, missing flags.

    #[test]
    fn claude_argv_case1_saved_and_on_disk_is_byte_identical() {
        let guard = HomeGuard::new("claude-c1");
        crate::db::init_for_tests();
        let path = format!("/fixture/claude-c1-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        let sid = "11111111-2222-3333-4444-555555555555";
        write_claude_session(&guard.home, &path, sid);
        set_saved_session(&project_id, sid, "claude");

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(out.command, "claude");
        assert_eq!(
            out.args,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--resume".to_string(),
                sid.to_string()
            ],
            "case-1 claude argv must be byte-identical to pre-Slice-3"
        );
        assert_eq!(out.resume_session, sid);
        assert!(out.resumed_existing);
        assert_eq!(out.provider, "claude");
        assert!(!out.pending_session_discovery);
    }

    #[test]
    fn claude_argv_case2_converge_is_byte_identical_and_persists() {
        let guard = HomeGuard::new("claude-c2");
        crate::db::init_for_tests();
        let path = format!("/fixture/claude-c2-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        // Saved id is stale (not on disk); a real newest session exists.
        let newest = "aaaaaaaa-1111-1111-1111-111111111111";
        write_claude_session(&guard.home, &path, newest);
        set_saved_session(
            &project_id,
            "bbbbbbbb-0000-0000-0000-000000000000",
            "claude",
        );

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(out.command, "claude");
        assert_eq!(
            out.args,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--resume".to_string(),
                newest.to_string()
            ],
            "case-2 claude argv must be byte-identical to pre-Slice-3"
        );
        assert!(out.resumed_existing);
        // GH#24: the converged id is persisted; harness stays truthful.
        assert_eq!(
            saved_row(&project_id),
            Some((Some(newest.to_string()), "claude".to_string()))
        );
    }

    #[test]
    fn claude_argv_case3_mint_is_byte_identical_and_preallocates() {
        let _guard = HomeGuard::new("claude-c3");
        crate::db::init_for_tests();
        let path = format!("/fixture/claude-c3-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(out.command, "claude");
        assert_eq!(
            out.args,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--session-id".to_string(),
                out.resume_session.clone()
            ],
            "case-3 claude argv must be byte-identical to pre-Slice-3"
        );
        assert!(!out.resumed_existing);
        assert!(!out.pending_session_discovery);
        assert!(!out.resume_session.is_empty());
        // Pre-allocated BEFORE spawn (v2_spawn auto-stamp contract).
        assert_eq!(
            saved_row(&project_id),
            Some((Some(out.resume_session.clone()), "claude".to_string()))
        );
    }

    #[test]
    fn explicit_selection_of_missing_claude_session_errors_with_same_message() {
        let _guard = HomeGuard::new("claude-explicit");
        crate::db::init_for_tests();
        let path = format!("/fixture/claude-explicit-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        let missing = "cccccccc-0000-0000-0000-000000000000";
        set_saved_session(&project_id, missing, "claude");

        let err = resolve_resume_chat_args_ex(&path, true).expect_err("must error");
        assert!(
            err.contains(missing) && err.contains("no conversation on disk"),
            "pre-Slice-3 error contract must hold, got: {err}"
        );
        // SSOT untouched.
        assert_eq!(
            saved_row(&project_id),
            Some((Some(missing.to_string()), "claude".to_string()))
        );
    }

    // ── Grok: premint + resume-by-harness ────────────────────────────

    #[test]
    fn grok_default_brand_new_premints_and_stamps_harness() {
        let _guard = HomeGuard::new("grok-mint");
        crate::db::init_for_tests();
        let preset = insert_preset("grok --always-approve", 950);
        let path = format!("/fixture/grok-mint-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, Some(&preset));

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(out.command, "grok");
        assert_eq!(
            out.args,
            vec![
                "--always-approve".to_string(),
                "--session-id".to_string(),
                out.resume_session.clone()
            ],
            "grok premints via --session-id (new sessions only, per the storage study), \
             keeping its preset args — no claude flags"
        );
        assert!(!out.resumed_existing);
        assert!(!out.pending_session_discovery);
        assert_eq!(out.provider, "grok");
        assert_eq!(
            saved_row(&project_id),
            Some((Some(out.resume_session.clone()), "grok".to_string())),
            "harness must be stamped 'grok', not hardcoded 'claude'"
        );
    }

    #[test]
    fn grok_saved_session_resumes_via_stored_harness_even_when_default_is_claude() {
        let guard = HomeGuard::new("grok-harness");
        crate::db::init_for_tests();
        // Roster carries a grok preset; the workspace default stays
        // claude (level 4) — the CANONICAL SESSION's harness must win.
        insert_preset("grok --always-approve", 951);
        let path = format!("/fixture/grok-harness-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        let sid = "01920000-eeee-7000-8000-000000000001";
        write_grok_session(&guard.home, &path, sid);
        set_saved_session(&project_id, sid, "grok");

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(
            out.command, "grok",
            "stored harness must pick the agent for an existing canonical session"
        );
        assert_eq!(
            out.args,
            vec![
                "--always-approve".to_string(),
                "--resume".to_string(),
                sid.to_string()
            ]
        );
        assert!(out.resumed_existing);
        assert_eq!(out.provider, "grok");
    }

    #[test]
    fn grok_stale_saved_converges_within_grok_never_to_claude() {
        let guard = HomeGuard::new("grok-converge");
        crate::db::init_for_tests();
        insert_preset("grok --always-approve", 952);
        let path = format!("/fixture/grok-converge-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        // A CLAUDE session exists on disk (newest overall) — but the
        // saved harness is grok, so convergence must stay grok-scoped.
        write_claude_session(&guard.home, &path, "dddddddd-0000-0000-0000-000000000000");
        let grok_real = "01920000-ffff-7000-8000-000000000001";
        write_grok_session(&guard.home, &path, grok_real);
        set_saved_session(&project_id, "01920000-ffff-7000-8000-00000000dead", "grok");

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(out.command, "grok");
        assert_eq!(out.resume_session, grok_real);
        assert!(out.args.iter().any(|a| a == grok_real));
        assert_eq!(
            saved_row(&project_id),
            Some((Some(grok_real.to_string()), "grok".to_string()))
        );
    }

    // ── Non-premint providers: bare spawn + pending discovery ────────

    #[test]
    fn pi_default_brand_new_spawns_bare_with_pending_discovery() {
        let _guard = HomeGuard::new("pi-pending");
        crate::db::init_for_tests();
        let preset = insert_preset("pi", 953);
        let path = format!("/fixture/pi-pending-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, Some(&preset));

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(out.command, "pi");
        assert!(
            out.args.is_empty(),
            "pi mints its own ids — bare spawn, no invented flags, got: {:?}",
            out.args
        );
        assert!(out.resume_session.is_empty());
        assert!(!out.resumed_existing);
        assert!(
            out.pending_session_discovery,
            "caller must be told to adopt the id post-hoc"
        );
        assert_eq!(
            saved_row(&project_id),
            Some((None, "pi".to_string())),
            "harness stamped up front; session_id stays NULL until adoption"
        );
    }

    #[test]
    fn codex_saved_session_uses_subcommand_grammar() {
        let guard = HomeGuard::new("codex-resume");
        crate::db::init_for_tests();
        insert_preset("codex --yolo", 954);
        let path = format!("/fixture/codex-resume-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        let sid = "01920000-abcd-7000-8000-000000000001";
        // Fabricate a codex rollout with a session_meta header.
        let day_dir = guard.home.join(".codex/sessions/2026/07/03");
        std::fs::create_dir_all(&day_dir).unwrap();
        std::fs::write(
            day_dir.join(format!("rollout-2026-07-03T10-00-00-{sid}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": { "id": sid, "cwd": path, "timestamp": "2026-07-03T10:00:00Z" }
                })
            ),
        )
        .unwrap();
        set_saved_session(&project_id, sid, "codex");

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(out.command, "codex");
        assert_eq!(
            out.args,
            vec!["--yolo".to_string(), "resume".to_string(), sid.to_string()],
            "codex resume keeps the Settings → LLMs flags in front of `resume <id>`"
        );
        assert!(out.resumed_existing);
        assert_eq!(out.provider, "codex");
    }

    // ── Unknown provider: Slice-2 degraded bare spawn ────────────────

    #[test]
    fn unknown_provider_degrades_to_bare_spawn_with_truthful_harness() {
        let _guard = HomeGuard::new("unknown");
        crate::db::init_for_tests();
        let preset = insert_preset("aider --chat", 955);
        let path = format!("/fixture/unknown-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, Some(&preset));

        let out = resolve_resume_chat_args_ex(&path, false).expect("resolve");
        assert_eq!(out.command, "aider");
        assert_eq!(
            out.args,
            vec!["--chat".to_string()],
            "preset args pass through verbatim"
        );
        assert!(out.resume_session.is_empty());
        assert!(!out.resumed_existing);
        assert!(
            !out.pending_session_discovery,
            "no adapter → nothing to discover"
        );
        assert_eq!(out.provider, "aider");
        assert_eq!(
            saved_row(&project_id),
            Some((None, "aider".to_string())),
            "harness records the actual command token, never a fake 'claude'"
        );
    }

    #[test]
    fn unknown_harness_explicit_selection_errors_auto_falls_through() {
        let _guard = HomeGuard::new("unknown-harness");
        crate::db::init_for_tests();
        let path = format!("/fixture/unknown-harness-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        set_saved_session(&project_id, "some-foreign-id", "aider");

        // Explicit pick of a session we can't verify → loud error.
        let err = resolve_resume_chat_args_ex(&path, true).expect_err("must error");
        assert!(err.contains("unknown provider"), "got: {err}");

        // Auto path: falls through to the default (claude) → case-3 mint.
        let out = resolve_resume_chat_args_ex(&path, false).expect("auto resolve");
        assert_eq!(out.command, "claude");
        assert!(out.args.iter().any(|a| a == "--session-id"));
    }

    fn assert_never_chatted_argv(args: &[String], source: &str) {
        for arg in args {
            assert_ne!(
                arg, source,
                "fresh argv must not contain the source id: {args:?}"
            );
            assert_ne!(
                arg, "--resume",
                "fresh argv must not contain --resume: {args:?}"
            );
            assert_ne!(
                arg, "resume",
                "fresh argv must not contain a resume token: {args:?}"
            );
            assert_ne!(
                arg, "--fork-session",
                "fresh argv must not contain --fork-session: {args:?}"
            );
        }
    }

    // ── Fresh continue: never resume or converge ─────────────────────

    #[test]
    fn never_chatted_claude_premints_when_source_file_exists() {
        let guard = HomeGuard::new("fresh-claude");
        crate::db::init_for_tests();
        let path = format!("/fixture/fresh-claude-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        let source = "11111111-2222-3333-4444-555555555555";
        write_claude_session(&guard.home, &path, source);
        set_saved_session(&project_id, source, "claude");

        let resumed = resolve_resume_chat_args_ex(&path, false).expect("resume");
        assert!(resumed.resumed_existing);
        assert!(resumed.args.iter().any(|arg| arg == "--resume"));
        assert!(resumed.args.iter().any(|arg| arg == source));

        let out = resolve_never_chatted_chat_args(&path, "claude").expect("fresh");
        assert_eq!(out.command, "claude");
        assert!(!out.resumed_existing);
        assert!(!out.pending_session_discovery);
        assert_ne!(out.resume_session, source);
        assert_eq!(
            out.args,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--session-id".to_string(),
                out.resume_session.clone(),
            ]
        );
        assert_never_chatted_argv(&out.args, source);
        assert_eq!(
            saved_row(&project_id),
            Some((Some(out.resume_session.clone()), "claude".to_string()))
        );
    }

    #[test]
    fn never_chatted_grok_keeps_preset_and_does_not_resume_source() {
        let guard = HomeGuard::new("fresh-grok");
        crate::db::init_for_tests();
        insert_preset("grok --always-approve", 960);
        let path = format!("/fixture/fresh-grok-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        let source = "01920000-eeee-7000-8000-0000000000aa";
        write_grok_session(&guard.home, &path, source);
        set_saved_session(&project_id, source, "grok");

        let resumed = resolve_resume_chat_args_ex(&path, false).expect("resume");
        assert_eq!(resumed.resume_session, source);
        assert!(resumed.args.iter().any(|arg| arg == "--resume"));

        let out = resolve_never_chatted_chat_args(&path, "grok").expect("fresh");
        assert_eq!(out.command, "grok");
        assert!(!out.resumed_existing);
        assert!(!out.pending_session_discovery);
        assert_ne!(out.resume_session, source);
        assert_eq!(
            out.args,
            vec![
                "--always-approve".to_string(),
                "--session-id".to_string(),
                out.resume_session.clone(),
            ]
        );
        assert_never_chatted_argv(&out.args, source);
        assert_eq!(
            saved_row(&project_id),
            Some((Some(out.resume_session.clone()), "grok".to_string()))
        );
    }

    #[test]
    fn never_chatted_pi_clears_source_and_strips_session_from_preset() {
        let guard = HomeGuard::new("fresh-pi");
        crate::db::init_for_tests();
        let source = "01920000-abcd-7000-8000-00000000beef";
        let preset = insert_preset(&format!("pi --no-color --session {source}"), 961);
        let path = format!("/fixture/fresh-pi-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, Some(&preset));
        write_claude_session(&guard.home, &path, source);
        set_saved_session(&project_id, source, "claude");

        let out = resolve_never_chatted_chat_args(&path, "pi").expect("fresh");
        assert_eq!(out.command, "pi");
        assert_eq!(out.args, vec!["--no-color".to_string()]);
        assert!(out.resume_session.is_empty());
        assert!(!out.resumed_existing);
        assert!(out.pending_session_discovery);
        assert_never_chatted_argv(&out.args, source);
        assert_eq!(saved_row(&project_id), Some((None, "pi".to_string())));
    }

    #[test]
    fn never_chatted_codex_strips_resume_subcommand_and_keeps_preset_flags() {
        let guard = HomeGuard::new("fresh-codex");
        crate::db::init_for_tests();
        let source = "01920000-abcd-7000-8000-00000000c0de";
        insert_preset(&format!("codex --yolo resume {source} --fork-session"), 962);
        let path = format!("/fixture/fresh-codex-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        write_claude_session(&guard.home, &path, source);
        set_saved_session(&project_id, source, "claude");

        let out = resolve_never_chatted_chat_args(&path, "codex").expect("fresh");
        assert_eq!(out.command, "codex");
        assert_eq!(out.args, vec!["--yolo".to_string()]);
        assert!(out.pending_session_discovery);
        assert!(out.resume_session.is_empty());
        assert_never_chatted_argv(&out.args, source);
        assert_eq!(saved_row(&project_id), Some((None, "codex".to_string())));
    }

    #[test]
    fn never_chatted_self_mint_harnesses_stay_bare() {
        let guard = HomeGuard::new("fresh-self-mint");
        crate::db::init_for_tests();
        let path = format!("/fixture/fresh-self-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        let source = "01920000-abcd-7000-8000-00000000a11a";
        write_claude_session(&guard.home, &path, source);
        set_saved_session(&project_id, source, "claude");

        for provider in ["pi", "codex", "gemini", "cursor", "hermes"] {
            set_saved_session(&project_id, source, "claude");
            let out = resolve_never_chatted_chat_args(&path, provider)
                .unwrap_or_else(|err| panic!("{provider}: {err}"));
            assert!(!out.resumed_existing, "{provider}");
            assert!(
                out.pending_session_discovery,
                "{provider} must pending-discover"
            );
            assert!(out.resume_session.is_empty(), "{provider}");
            assert_eq!(out.provider, provider);
            assert_never_chatted_argv(&out.args, source);
            assert_eq!(
                saved_row(&project_id),
                Some((None, provider.to_string())),
                "{provider} must clear the source id"
            );
        }
    }

    #[test]
    fn never_chatted_unknown_harness_does_not_clear_the_source() {
        let guard = HomeGuard::new("fresh-unknown");
        crate::db::init_for_tests();
        let path = format!("/fixture/fresh-unknown-{}", uuid::Uuid::new_v4());
        let project_id = insert_project(&path, None);
        let source = "01920000-abcd-7000-8000-0000000000ff";
        write_claude_session(&guard.home, &path, source);
        set_saved_session(&project_id, source, "claude");

        let err = resolve_never_chatted_chat_args(&path, "aider").expect_err("unknown");
        assert!(err.contains("unknown harness"), "got: {err}");
        assert_eq!(
            saved_row(&project_id),
            Some((Some(source.to_string()), "claude".to_string()))
        );
    }

    // ── JSON wire shape stays additive ───────────────────────────────

    #[test]
    fn to_json_keeps_existing_keys_and_adds_provider_fields() {
        let out = ResumeChatArgs {
            command: "claude".into(),
            args: vec!["--resume".into(), "X".into()],
            cwd: "/w".into(),
            resume_session: "X".into(),
            resumed_existing: true,
            provider: "claude".into(),
            pending_session_discovery: false,
            env: Some([("SECRET_KEY".to_string(), "must-not-leak".to_string())].into()),
            readiness: Some("settle:2000".to_string()),
        };
        let j = out.to_json();
        assert_eq!(j["command"], "claude");
        assert_eq!(j["resumeSession"], "X");
        assert_eq!(j["resumedExisting"], true);
        assert_eq!(j["provider"], "claude");
        assert_eq!(j["pendingSessionDiscovery"], false);
        // W2: env is DELIBERATELY absent from the wire shape — values
        // may hold credentials and only daemon spawn sites consume them.
        assert!(j.get("env").is_none(), "env must never be serialized");
        assert!(
            !j.to_string().contains("must-not-leak"),
            "env values must never appear in the wire JSON"
        );
    }
}
