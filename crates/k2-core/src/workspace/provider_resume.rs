//! Agent de-generalization Slice 3 — the per-agent RESUME adapter.
//!
//! `resolve_resume_chat_args_ex` (the canonical pinned-chat resolver)
//! fuses four things: session identity, spawn command, resume flag
//! grammar, and the provider's on-disk session model. Pre-Slice-3 all
//! four were hardcoded Claude. This module carries the last three as a
//! per-provider record so the resolver (and the post-hoc adoption
//! helper) can speak each agent's dialect:
//!
//! - **Resume grammar** — flag style (`claude --resume <id>`,
//!   `pi --session <id>`) vs subcommand style (`codex resume <id>`).
//!   Mirrors the TS SSOT `src/shared/constants.ts::RESUMABLE_CLI_TOOLS`
//!   and `ChatHistory.tsx::PROVIDER_CONFIG` exactly, including the
//!   argv-assembly convention (`tabs.ts` / `ChatHistory.tsx`):
//!   flag style = `<preset args> <flag> <id>`; subcommand style =
//!   `<subcommand> <id>` with the preset args DROPPED.
//! - **Premint** — whether the provider accepts a K2-minted session id
//!   for a NEW session (`claude --session-id <uuid>`; grok
//!   `-s/--session-id <uuid>`, valid for new sessions per
//!   `.k2/notes/grok-session-storage-study.md`). Providers without a
//!   premint (pi/codex/gemini/cursor) mint their own ids — callers
//!   spawn bare and adopt the id post-hoc
//!   ([`defer_adopt_discovered_session`]).
//! - **On-disk session model** — `session_file_exists` /
//!   `newest_on_disk`, backed by `crate::chat_history`:
//!     - claude: `claude_session_file_exists` /
//!       `newest_claude_session_on_disk` (unchanged, byte-identical);
//!     - grok: the study-spec walkers (`grok_session_file_exists` /
//!       `newest_grok_session_on_disk` — subagents skipped, newest by
//!       `last_active_at`);
//!     - cursor: chat-dir `store.db` probe / `detect_cursor_session`;
//!     - pi / codex / gemini: `parse_*_sessions(Some(project))` for the
//!       exists check (a listed id = a real on-disk conversation) and
//!       `detect_*_session` for newest. APPROXIMATION: both walk the
//!       provider's whole per-project store (same cost class as the
//!       detect machinery itself); `detect_gemini_session` additionally
//!       requires the project registered in `~/.gemini/projects.json`.
//!
//! **Unknown provider ⇒ `None`.** A command that maps to no adapter
//! (hermes, arbitrary custom commands) makes callers degrade to a
//! fresh bare spawn with no resume — exactly the Slice-2 `// Slice 3:`
//! gate behavior.

use std::time::Duration;

/// How a provider resumes an EXISTING session by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeGrammar {
    /// `<command> <preset-args> <flag> <id>` (claude/grok/gemini/cursor
    /// `--resume`; pi `--session` — pi's `--resume` is its interactive
    /// picker and takes no id, don't confuse them).
    Flag(&'static str),
    /// `<command> <subcommand> <id>` with preset args dropped
    /// (codex `resume <id>`). Matches the TS argv convention in
    /// `tabs.ts`/`ChatHistory.tsx`.
    Subcommand(&'static str),
}

/// How a provider accepts a K2-pre-minted session id for a NEW session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PremintStyle {
    /// `<command> <preset-args> <flag> <uuid>` before first spawn
    /// (claude/grok `--session-id`). K2 mints a v4 UUID, persists it to
    /// `workspace_sessions.session_id` BEFORE the spawn, then passes it.
    Flag(&'static str),
}

/// Per-provider resume knowledge. One static record per Big-7 provider
/// with on-disk discovery support (hermes has none anywhere yet —
/// deliberately absent so it degrades like any unknown command).
#[derive(Debug, Clone, Copy)]
pub struct ProviderResume {
    /// Provider key — matches `chat_history::detect_active_session`'s
    /// provider strings AND the `workspace_sessions.harness` values
    /// this slice makes load-bearing.
    pub provider: &'static str,
    /// The provider's spawn binary (first token). Differs from
    /// `provider` only for cursor (`cursor-agent`).
    pub command: &'static str,
    pub grammar: ResumeGrammar,
    pub premint: Option<PremintStyle>,
}

/// The adapter table. Grammar column mirrors
/// `src/shared/constants.ts::RESUMABLE_CLI_TOOLS` (the authoritative TS
/// mirror); premint column per the claude behavior shipping today +
/// the grok storage study.
const PROVIDERS: &[ProviderResume] = &[
    ProviderResume {
        provider: "claude",
        command: "claude",
        grammar: ResumeGrammar::Flag("--resume"),
        premint: Some(PremintStyle::Flag("--session-id")),
    },
    ProviderResume {
        provider: "grok",
        command: "grok",
        grammar: ResumeGrammar::Flag("--resume"),
        // `-s/--session-id <uuid>` is valid for NEW sessions only
        // (with --resume it needs --fork-session) — study §CLI grammar.
        premint: Some(PremintStyle::Flag("--session-id")),
    },
    ProviderResume {
        provider: "cursor",
        command: "cursor-agent",
        grammar: ResumeGrammar::Flag("--resume"),
        premint: None,
    },
    ProviderResume {
        provider: "gemini",
        command: "gemini",
        grammar: ResumeGrammar::Flag("--resume"),
        premint: None,
    },
    ProviderResume {
        provider: "pi",
        command: "pi",
        grammar: ResumeGrammar::Flag("--session"),
        premint: None,
    },
    ProviderResume {
        provider: "codex",
        command: "codex",
        grammar: ResumeGrammar::Subcommand("resume"),
        premint: None,
    },
];

/// Basename of a command's first token — `~/bin/claude` → `claude`,
/// `codex -c x=y` → `codex`.
fn command_basename(command: &str) -> &str {
    let first = command.split_whitespace().next().unwrap_or("");
    first.rsplit('/').next().unwrap_or(first)
}

/// Resolve an adapter from a spawnable COMMAND (first token, basename
/// match so path-qualified binaries count). `None` = unknown provider;
/// callers must degrade to a fresh bare spawn (Slice-2 gate parity).
pub fn provider_resume_for_command(command: &str) -> Option<&'static ProviderResume> {
    let name = command_basename(command);
    if name.is_empty() {
        return None;
    }
    PROVIDERS.iter().find(|p| p.command == name)
}

/// Resolve an adapter from a stored PROVIDER string (the
/// `workspace_sessions.harness` value / `detect_active_session` key).
pub fn provider_resume_for_provider(provider: &str) -> Option<&'static ProviderResume> {
    PROVIDERS.iter().find(|p| p.provider == provider)
}

impl ProviderResume {
    /// Assemble resume argv for an existing session id, following the
    /// TS convention: flag style appends `<flag> <id>` to the base
    /// (preset) args; subcommand style REPLACES them with
    /// `<subcommand> <id>`.
    pub fn resume_args(&self, base_args: &[String], session_id: &str) -> Vec<String> {
        match self.grammar {
            ResumeGrammar::Flag(flag) => {
                let mut args = base_args.to_vec();
                args.push(flag.to_string());
                args.push(session_id.to_string());
                args
            }
            ResumeGrammar::Subcommand(sub) => {
                vec![sub.to_string(), session_id.to_string()]
            }
        }
    }

    /// Assemble premint argv for a K2-minted new-session id, or `None`
    /// when the provider mints its own ids (spawn bare + adopt
    /// post-hoc).
    pub fn premint_args(&self, base_args: &[String], session_id: &str) -> Option<Vec<String>> {
        match self.premint {
            Some(PremintStyle::Flag(flag)) => {
                let mut args = base_args.to_vec();
                args.push(flag.to_string());
                args.push(session_id.to_string());
                Some(args)
            }
            None => None,
        }
    }

    /// Does a resumable on-disk conversation exist for this
    /// `session_id` + `project_path` (project family: root +
    /// worktrees)?
    pub fn session_file_exists(&self, session_id: &str, project_path: &str) -> bool {
        if session_id.is_empty() {
            return false;
        }
        match self.provider {
            "claude" => crate::chat_history::claude_session_file_exists(session_id, project_path),
            "grok" => crate::chat_history::grok_session_file_exists(session_id, project_path),
            "cursor" => crate::chat_history::cursor_session_file_exists(session_id, project_path),
            // Exists-via-listing: a session surfaced by the provider's
            // parser is a real on-disk conversation (same walk the
            // ChatHistory browser does). Parser errors degrade to
            // "not found" — the resolver then converges/mints, never
            // errors a spawn.
            "gemini" => parsed_session_exists(
                crate::chat_history::parse_gemini_sessions(Some(project_path)),
                session_id,
            ),
            "pi" => parsed_session_exists(
                crate::chat_history::parse_pi_sessions(Some(project_path)),
                session_id,
            ),
            "codex" => parsed_session_exists(
                crate::chat_history::parse_codex_sessions(Some(project_path)),
                session_id,
            ),
            _ => false,
        }
    }

    /// The most-recently-active on-disk session id for this
    /// `project_path`, or `None` for a genuinely session-less
    /// workspace. This is both the GH#24 converge source and the
    /// post-hoc adoption probe (for claude it scans the projects dir
    /// directly — `detect_claude_session` keys on history.jsonl, which
    /// lags a fresh spawn; see `wake_headless::defer_stamp_adopted_session`'s
    /// GH#24 smoke-test note).
    pub fn newest_on_disk(&self, project_path: &str) -> Option<String> {
        match self.provider {
            "claude" => crate::chat_history::newest_claude_session_on_disk(project_path),
            "grok" => crate::chat_history::newest_grok_session_on_disk(project_path),
            "cursor" => crate::chat_history::detect_cursor_session(project_path),
            "gemini" => crate::chat_history::detect_gemini_session(project_path),
            "pi" => crate::chat_history::detect_pi_session(project_path),
            "codex" => crate::chat_history::detect_codex_session(project_path),
            _ => None,
        }
    }
}

fn parsed_session_exists(
    parsed: Result<Vec<crate::chat_history::ChatSession>, String>,
    session_id: &str,
) -> bool {
    parsed
        .map(|sessions| sessions.iter().any(|s| s.session_id == session_id))
        .unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────
// Post-hoc session adoption (non-premint providers + the claude eager
// read-back). Core generalization of the daemon's claude-only
// `wake_headless::defer_stamp_adopted_session`.
// ─────────────────────────────────────────────────────────────────────

/// How long to wait after spawn before probing the provider's on-disk
/// store. Matches the wake path's claude-tuned window; grok writes are
/// live within seconds (study gotcha #1), pi/codex write their headers
/// at session start.
const ADOPTION_PROBE_DELAY: Duration = Duration::from_secs(5);

/// Synchronous core: discover the session id the freshly-spawned
/// `provider` agent adopted on disk for `project_path`, and stamp it —
/// `workspace_sessions.session_id` + `harness` — via the identity
/// upsert. Returns the adopted id, or `None` when the provider is
/// unknown, nothing is on disk yet, or the workspace is unregistered.
///
/// Uses the adapter's `newest_on_disk` (NOT `detect_active_session`):
/// for claude the history.jsonl-keyed detect has no entry for a
/// freshly-spawned, not-yet-messaged chat inside the probe window
/// (GH#24 smoke-test finding), while the projects-dir scan catches the
/// new file immediately. For pi/codex/gemini/cursor `newest_on_disk`
/// IS their detect machinery.
pub fn adopt_discovered_session(provider: &str, project_path: &str) -> Option<String> {
    let adapter = provider_resume_for_provider(provider)?;
    let session_id = adapter.newest_on_disk(project_path)?;
    if session_id.is_empty() {
        return None;
    }
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id =
        crate::workspace::agent_identity::resolve_project_id(&conn, project_path)?;
    match crate::db::schema::WorkspaceSession::update_session_id_and_harness(
        &conn,
        &project_id,
        &session_id,
        adapter.provider,
    ) {
        Ok(_) => {
            crate::log_debug!(
                "[core/provider-resume] adopted {} session {} for {}",
                adapter.provider,
                session_id,
                project_path
            );
            Some(session_id)
        }
        Err(e) => {
            crate::log_debug!(
                "[core/provider-resume] adopt {} session for {} failed: {}",
                adapter.provider,
                project_path,
                e
            );
            None
        }
    }
}

/// Deferred, fire-and-forget wrapper around
/// [`adopt_discovered_session`]: sleep ~5s on a detached thread (the
/// provider writes its session file a beat after spawn), then probe +
/// stamp. Errors are logged, never surfaced — the resolver's lazy
/// converge fallback (`resume_chat.rs`) covers a miss.
///
/// This is the reusable core for every daemon spawn site
/// (Slice 3b wires wake_headless / heartbeat / agents_routes /
/// v2_spawn); Slice 3 wires `pinned_chat::ensure_pinned_chat`.
/// An unknown `provider` no-ops immediately (no thread).
pub fn defer_adopt_discovered_session(provider: String, project_path: String) {
    if provider_resume_for_provider(&provider).is_none() {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(ADOPTION_PROBE_DELAY);
        let _ = adopt_discovered_session(&provider, &project_path);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ── Table lookups ────────────────────────────────────────────────

    #[test]
    fn command_lookup_covers_the_resumable_roster() {
        for (cmd, provider) in [
            ("claude", "claude"),
            ("grok", "grok"),
            ("cursor-agent", "cursor"),
            ("gemini", "gemini"),
            ("pi", "pi"),
            ("codex", "codex"),
        ] {
            let a = provider_resume_for_command(cmd)
                .unwrap_or_else(|| panic!("adapter missing for {cmd}"));
            assert_eq!(a.provider, provider);
        }
        // Path-qualified binaries resolve by basename.
        assert_eq!(
            provider_resume_for_command("/usr/local/bin/claude").map(|a| a.provider),
            Some("claude")
        );
        // First token only — a full preset command string works too.
        assert_eq!(
            provider_resume_for_command("codex -c x=\"y\"").map(|a| a.provider),
            Some("codex")
        );
    }

    #[test]
    fn unknown_commands_and_providers_resolve_to_none() {
        assert!(provider_resume_for_command("hermes").is_none());
        assert!(provider_resume_for_command("bash").is_none());
        assert!(provider_resume_for_command("claudette").is_none());
        assert!(provider_resume_for_command("").is_none());
        assert!(provider_resume_for_provider("hermes").is_none());
        assert!(provider_resume_for_provider("cursor-agent").is_none(),
            "provider lookup keys on the provider string, not the binary");
        assert!(provider_resume_for_provider("").is_none());
    }

    // ── Grammar assembly (TS RESUMABLE_CLI_TOOLS parity) ─────────────

    #[test]
    fn flag_style_appends_to_base_args() {
        let claude = provider_resume_for_provider("claude").unwrap();
        assert_eq!(
            claude.resume_args(&args(&["--dangerously-skip-permissions"]), "SID"),
            args(&["--dangerously-skip-permissions", "--resume", "SID"])
        );
        let grok = provider_resume_for_provider("grok").unwrap();
        assert_eq!(
            grok.resume_args(&args(&["--always-approve"]), "SID"),
            args(&["--always-approve", "--resume", "SID"])
        );
        // Pi resumes with --session (its --resume is the interactive
        // picker — RESUMABLE_CLI_TOOLS comment).
        let pi = provider_resume_for_provider("pi").unwrap();
        assert_eq!(pi.resume_args(&[], "SID"), args(&["--session", "SID"]));
    }

    #[test]
    fn subcommand_style_drops_preset_args() {
        let codex = provider_resume_for_provider("codex").unwrap();
        assert_eq!(
            codex.resume_args(
                &args(&["-c", "model_reasoning_effort=high", "--dangerously-bypass-approvals-and-sandbox"]),
                "SID"
            ),
            args(&["resume", "SID"]),
            "codex resume replaces preset args — tabs.ts/ChatHistory.tsx convention"
        );
    }

    #[test]
    fn premint_only_for_claude_and_grok() {
        let claude = provider_resume_for_provider("claude").unwrap();
        assert_eq!(
            claude.premint_args(&args(&["--dangerously-skip-permissions"]), "NEW"),
            Some(args(&["--dangerously-skip-permissions", "--session-id", "NEW"]))
        );
        let grok = provider_resume_for_provider("grok").unwrap();
        assert_eq!(
            grok.premint_args(&args(&["--always-approve"]), "NEW"),
            Some(args(&["--always-approve", "--session-id", "NEW"]))
        );
        for p in ["pi", "codex", "gemini", "cursor"] {
            assert_eq!(
                provider_resume_for_provider(p).unwrap().premint_args(&[], "NEW"),
                None,
                "{p} mints its own ids — premint must be None"
            );
        }
    }

    // ── Grok on-disk adapter, $HOME-honoring end to end ──────────────
    //
    // The inner walkers are covered in chat_history.rs's unit tests;
    // this exercises the ADAPTER surface through a fabricated
    // `~/.grok/sessions` tree under a scratch $HOME.

    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        home: std::path::PathBuf,
        _lock: parking_lot::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new(label: &str) -> Self {
            let lock = crate::themes::HOME_LOCK.lock();
            let home = std::env::temp_dir().join(format!(
                "k2-provider-resume-{label}-{}-{}",
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

    fn write_grok_fixture(
        home: &std::path::Path,
        session_id: &str,
        cwd: &str,
        last_active_at: &str,
        session_kind: Option<&str>,
    ) {
        let dir = home
            .join(".grok")
            .join("sessions")
            .join("%2Ffixture")
            .join(session_id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut summary = serde_json::json!({
            "info": { "id": session_id, "cwd": cwd },
            "last_active_at": last_active_at,
            "updated_at": last_active_at,
        });
        if let Some(kind) = session_kind {
            summary["session_kind"] = serde_json::json!(kind);
        }
        std::fs::write(dir.join("summary.json"), summary.to_string()).unwrap();
    }

    #[test]
    fn grok_adapter_exists_newest_and_subagent_skip_via_home() {
        let guard = HomeGuard::new("grok-adapter");
        let project = "/Users/fixture/grok-proj";
        let older = "01920000-dddd-7000-8000-000000000001";
        let newest = "01920000-dddd-7000-8000-000000000002";
        let sub = "01920000-dddd-7000-8000-000000000003";
        write_grok_fixture(&guard.home, older, project, "2026-07-03T08:00:00Z", None);
        write_grok_fixture(&guard.home, newest, project, "2026-07-03T09:00:00Z", None);
        write_grok_fixture(&guard.home, sub, project, "2026-07-03T10:00:00Z", Some("subagent"));

        let grok = provider_resume_for_provider("grok").unwrap();
        assert!(grok.session_file_exists(older, project));
        assert!(grok.session_file_exists(newest, project));
        assert!(
            !grok.session_file_exists(sub, project),
            "subagent sessions must not satisfy the exists check"
        );
        assert!(!grok.session_file_exists("missing-id", project));
        assert_eq!(
            grok.newest_on_disk(project).as_deref(),
            Some(newest),
            "newest_on_disk must skip the newer subagent and pick the newest user session"
        );
        assert_eq!(grok.newest_on_disk("/Users/fixture/other-proj"), None);
    }

    #[test]
    fn claude_adapter_backs_onto_existing_probes() {
        let guard = HomeGuard::new("claude-adapter");
        let project = "/Users/fixture/claude-proj";
        let sid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let hash = crate::chat_history::claude_project_hash(project);
        let dir = guard.home.join(".claude").join("projects").join(&hash);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{sid}.jsonl")), b"{\"cwd\":\"/x\"}\n").unwrap();

        let claude = provider_resume_for_provider("claude").unwrap();
        assert!(claude.session_file_exists(sid, project));
        assert!(!claude.session_file_exists("nope", project));
        assert_eq!(claude.newest_on_disk(project).as_deref(), Some(sid));
    }

    #[test]
    fn adoption_helpers_noop_for_unknown_provider() {
        assert_eq!(adopt_discovered_session("hermes", "/nowhere"), None);
        // Must not spawn a thread / panic.
        defer_adopt_discovered_session("hermes".into(), "/nowhere".into());
    }
}
