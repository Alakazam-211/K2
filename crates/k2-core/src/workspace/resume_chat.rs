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
                    let (command, base_args) = spawn_command_for(adapter, &default_cmd);
                    let args = adapter.resume_args(&base_args, sid);
                    return Ok(ResumeChatArgs {
                        command,
                        args,
                        cwd: project_path.to_string(),
                        resume_session: sid.clone(),
                        resumed_existing: true,
                        provider: adapter.provider.to_string(),
                        pending_session_discovery: false,
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
        Some(adapter) => converge_or_mint(adapter, &default_cmd, project_id.as_deref(), project_path),
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
    let (command, base_args) = spawn_command_for(adapter, default_cmd);

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
) -> (String, Vec<String>) {
    // 1. Workspace/global default already speaks this provider.
    if provider_resume_for_command(&default_cmd.command).map(|a| a.provider)
        == Some(adapter.provider)
    {
        return (
            default_cmd.command.clone(),
            claude_pinned_or(&default_cmd.command, &default_cmd.args),
        );
    }

    // 2. Enabled preset roster, display order (same ordering
    //    agent_resolve uses, so both sides pick the same row).
    let roster: Vec<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.prepare(
            "SELECT command FROM agent_presets WHERE enabled = 1 ORDER BY sort_order, label",
        )
        .and_then(|mut stmt| {
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .flatten()
                .collect::<Vec<String>>();
            Ok(rows)
        })
        .unwrap_or_default()
    };
    for command_str in roster {
        if provider_resume_for_command(&command_str).map(|a| a.provider)
            == Some(adapter.provider)
        {
            let (command, args) =
                crate::workspace::agent_resolve::parse_command_string(&command_str);
            if !command.is_empty() {
                let args = claude_pinned_or(&command, &args);
                return (command, args);
            }
        }
    }

    // 3. Adapter default binary.
    let command = adapter.command.to_string();
    let args = claude_pinned_or(&command, &[]);
    (command, args)
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
fn persist_session_identity(
    project_id: Option<&str>,
    session_id: Option<&str>,
    harness: &str,
) {
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
    /// Serialized via the crate-wide HOME_LOCK.
    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        home: PathBuf,
        _lock: parking_lot::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new(label: &str) -> Self {
            let lock = crate::themes::HOME_LOCK.lock();
            let home = std::env::temp_dir().join(format!(
                "k2-resume-chat-{label}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&home).unwrap();
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", &home);
            Self { original, home, _lock: lock }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.home);
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
        set_saved_session(&project_id, "bbbbbbbb-0000-0000-0000-000000000000", "claude");

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
        insert_preset(
            "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox",
            954,
        );
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
            vec!["resume".to_string(), sid.to_string()],
            "codex resume is subcommand-style and drops preset args (TS parity)"
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
        assert_eq!(out.args, vec!["--chat".to_string()], "preset args pass through verbatim");
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
        };
        let j = out.to_json();
        assert_eq!(j["command"], "claude");
        assert_eq!(j["resumeSession"], "X");
        assert_eq!(j["resumedExisting"], true);
        assert_eq!(j["provider"], "claude");
        assert_eq!(j["pendingSessionDiscovery"], false);
    }
}
