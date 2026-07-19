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
//!     - hermes: read-only WAL-capable SQLite queries against
//!       `~/.hermes/state.db` (`hermes_session_exists` /
//!       `detect_hermes_session`). The exists check deliberately
//!       matches ANY row for the id — a stored id can be superseded by
//!       a compression fork, and hermes' own `--resume` redirects to
//!       the chain tip;
//!     - pi / codex / gemini: `parse_*_sessions(Some(project))` for the
//!       exists check (a listed id = a real on-disk conversation) and
//!       `detect_*_session` for newest. APPROXIMATION: both walk the
//!       provider's whole per-project store (same cost class as the
//!       detect machinery itself); `detect_gemini_session` additionally
//!       requires the project registered in `~/.gemini/projects.json`.
//!
//! **Unknown provider ⇒ `None`.** A command that maps to no adapter
//! (arbitrary custom commands) makes callers degrade to a fresh bare
//! spawn with no resume — exactly the Slice-2 `// Slice 3:` gate
//! behavior.

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

/// Per-provider resume knowledge. One static record per Big-7
/// provider — all seven have on-disk discovery support as of Slice 6.
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
    ProviderResume {
        provider: "hermes",
        command: "hermes",
        // `hermes --resume <SESSION>` resumes by id (or title); its
        // `--continue` is most-recent GLOBAL (not cwd-scoped) — never
        // used. NO premint: `--pass-session-id` only injects into the
        // system prompt, ids are minted internally → post-hoc
        // discovery (pi/codex family) — hermes storage study.
        grammar: ResumeGrammar::Flag("--resume"),
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

/// The union of flag tokens that carry a session id in SOME supported
/// provider's grammar: `--session-id` (claude/grok premint),
/// `--resume` / `-r` (claude/grok/gemini/cursor/hermes resume + shared
/// aliases), `--session` (pi). Single source for the grammar-union
/// matcher ([`argv_references_session`]) and the per-provider extractor
/// ([`session_id_from_spawn_argv`]) — new grammars are added HERE and in
/// [`PROVIDERS`], nowhere else.
const SESSION_FLAG_TOKENS: &[&str] = &["--session-id", "--resume", "-r", "--session"];

/// Does a spawned argv claim ownership of `session_id`? Recognizes
/// EVERY grammar K2 (or a user) launches supported agents with:
///
/// - flag style — `--session-id <id>` (claude/grok premint),
///   `--resume <id>` / `-r <id>` (claude/grok/gemini/cursor),
///   `--session <id>` (pi);
/// - subcommand style — `resume <id>` as the LEADING argv pair
///   (codex; K2 always assembles it at position 0, see
///   [`ProviderResume::resume_args`]).
///
/// Supersedes the daemon's inline `--session-id`/`--resume`-only scans
/// (workspace_msg branch 1b / relookup, heartbeat
/// `find_live_for_resume`) — those missed subcommand grammar, so a
/// live `codex resume <id>` PTY was invisible to the wake/inject
/// fast paths. Strictly widens matching; claude argv produced by K2
/// matched before and still matches.
pub fn argv_references_session(args: &[String], session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    if args.len() >= 2 && args[0] == "resume" && args[1] == session_id {
        return true;
    }
    args.windows(2)
        .any(|w| SESSION_FLAG_TOKENS.contains(&w[0].as_str()) && w[1] == session_id)
}

/// First `<flag> <value>` pair whose flag satisfies `is_flag`.
fn flag_value(args: &[String], is_flag: impl Fn(&str) -> bool) -> Option<String> {
    args.windows(2)
        .find_map(|w| is_flag(w[0].as_str()).then(|| w[1].clone()))
}

/// EXTRACT the session id a spawn argv claims for `command`, in that
/// provider's grammar — the write-side dual of
/// [`argv_references_session`] (any id returned here satisfies the
/// matcher; the two share [`SESSION_FLAG_TOKENS`] / the adapter table).
/// This is what `v2_session_map::register` stamps
/// `workspace_tab_sessions.session_id` from, so it must recognize EVERY
/// grammar the daemon assembles ([`ProviderResume::resume_args`] /
/// [`ProviderResume::premint_args`]) — the pre-Slice-W3 inline
/// `--session-id`/`--resume` scan missed pi's `--session` and codex's
/// leading `resume <id>` subcommand, leaving those rows NULL (invisible
/// to the /v1 host-sessions list/resume index).
///
/// Command-aware:
/// - known subcommand-grammar provider (codex) → the LEADING
///   `resume <id>` pair only (a deeper positional `resume` is prompt
///   text, not identity);
/// - known flag-grammar provider → its resume flag, the shared
///   `--resume`/`-r` aliases, and its premint flag (if any);
/// - unknown command → the legacy generic `--session-id`/`--resume`
///   pair scan, byte-identical to the historical v2_session_map
///   behavior for arbitrary commands (deliberately NOT widened: a
///   random command's `--session`/leading-`resume` argv is not evidence
///   of a resumable conversation).
pub fn session_id_from_spawn_argv(command: &str, args: &[String]) -> Option<String> {
    match provider_resume_for_command(command) {
        Some(adapter) => match adapter.grammar {
            ResumeGrammar::Subcommand(sub) => {
                (args.len() >= 2 && args[0] == sub).then(|| args[1].clone())
            }
            ResumeGrammar::Flag(flag) => flag_value(args, |a| {
                a == flag || a == "--resume" || a == "-r" || Some(a) == adapter.premint_flag()
            }),
        },
        None => flag_value(args, |a| a == "--session-id" || a == "--resume"),
    }
}

impl ProviderResume {
    /// The premint flag token (`--session-id` for claude/grok), or
    /// `None` for self-minting providers. Callers that splice the flag
    /// into an EXISTING argv (v2_spawn's auto-inject) use this; fresh
    /// assemblies use [`Self::premint_args`].
    pub fn premint_flag(&self) -> Option<&'static str> {
        match self.premint {
            Some(PremintStyle::Flag(flag)) => Some(flag),
            None => None,
        }
    }

    /// Does `args` ALREADY carry a session identity in this provider's
    /// grammar (so a caller must not splice another resume/premint)?
    /// Flag style checks the provider's resume flag plus the shared
    /// `--resume`/`-r` aliases and the premint flag; subcommand style
    /// checks the leading subcommand.
    pub fn argv_carries_session_identity(&self, args: &[String]) -> bool {
        if let Some(flag) = self.premint_flag() {
            if args.iter().any(|a| a == flag) {
                return true;
            }
        }
        match self.grammar {
            ResumeGrammar::Flag(flag) => args
                .iter()
                .any(|a| a == flag || a == "--resume" || a == "-r"),
            ResumeGrammar::Subcommand(sub) => {
                args.first().map(|a| a == sub).unwrap_or(false)
            }
        }
    }

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
            // Deliberately ignores project_path: an id that exists in
            // state.db AT ALL is resumable — a stored id superseded by
            // a compression fork still resolves (hermes redirects
            // `--resume <old-id>` to the chain tip itself).
            "hermes" => crate::chat_history::hermes_session_exists(session_id),
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
            "hermes" => crate::chat_history::detect_hermes_session(project_path),
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
// Slice 5 — per-provider injection profile (wake/inject readiness)
// ─────────────────────────────────────────────────────────────────────

/// How the daemon's wake/injection path decides a freshly-spawned
/// agent TUI is READY to receive an injected message (2026-07 TUI
/// signal studies; consulted by `workspace_msg::deliver_post_wake` and
/// `wake_headless`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InjectionProfile {
    /// `true` — the provider's `ESC[?2004h` (bracketed paste) flip is a
    /// trustworthy "input box mounted" signal, so the injector may poll
    /// `bracketed_paste_active()` (with the settle floor below) and
    /// inject as soon as it flips:
    ///   - claude: ?2004h genuinely follows input-box mount (original
    ///     activity study) — the shipping poll works; DO NOT regress.
    ///   - grok: ?2004h at ~1.1s, trustworthy for "UI mounted" — but
    ///     the settle floor must still clear that mount window (see
    ///     [`injection_profile_for_provider`]); paste alone is not enough
    ///     without quiescence (dormant-wake / host-session first inject).
    ///   - cursor-agent: ?2004h is set only once the composer exists
    ///     (a TRUSTED dir; in an untrusted dir it never flips — the
    ///     poll correctly waits, then times out to best-effort).
    ///
    /// `false` — ?2004h LIES for this provider and must not be treated
    /// as readiness; the injector waits the full settle floor instead:
    ///   - codex: ?2004h at 0.4s while startup dialogs still block
    ///     input for 9-13s (dialogs eat keystrokes as hotkeys).
    ///   - gemini: same early-?2004h pathology as pi (set while the
    ///     trust/auth dialog still swallows input).
    ///   - pi: enables ?2004h at startup, before real readiness.
    ///   - hermes: prompt_toolkit toggles ?2004h per repaint and holds
    ///     it ON during the model wait — useless as a ready signal.
    pub ready_via_bracketed_paste: bool,
    /// Conservative minimum post-spawn settle before the first
    /// injection (the floor for polling providers; the whole wait for
    /// non-polling ones). Study-derived: hermes needs ~7s before its
    /// first message lands (prompt ~3.6s + ~3s agent init); codex/
    /// gemini get 2s headroom past their dialog storms; claude keeps
    /// the shipping 400ms floor exactly; grok uses ~1.5s (≥ ~1.1s
    /// ?2004h mount + first-frame headroom).
    pub post_spawn_settle: Duration,
}

/// Unknown-provider default = the claude-shaped behavior every provider
/// got before slice 5 (poll bracketed paste with the 400ms floor,
/// bounded by the caller's wake timeout) — safe degradation, no new
/// assumptions about agents we never studied.
pub const DEFAULT_INJECTION_PROFILE: InjectionProfile = InjectionProfile {
    ready_via_bracketed_paste: true,
    post_spawn_settle: Duration::from_millis(400),
};

/// Resolve the injection profile for a provider key
/// (`workspace_sessions.harness` / [`ProviderResume::provider`] value).
/// Unknown providers get [`DEFAULT_INJECTION_PROFILE`].
pub fn injection_profile_for_provider(provider: &str) -> InjectionProfile {
    match provider {
        // Poll-trustworthy providers (see field doc for evidence).
        "claude" => DEFAULT_INJECTION_PROFILE,
        // Grok: ?2004h is trustworthy (~1.1s study) but NOT at Claude's
        // 400ms floor — under-settling lands host-session / wake injects
        // in a still-painting first frame. Keep paste poll; raise settle
        // past mount + a little headroom, then shared quiescence finishes.
        "grok" => InjectionProfile {
            ready_via_bracketed_paste: true,
            post_spawn_settle: Duration::from_millis(1500),
        },
        "cursor" => InjectionProfile {
            ready_via_bracketed_paste: true,
            post_spawn_settle: Duration::from_millis(1000),
        },
        // ?2004h liars — readiness is the settle floor alone.
        "codex" | "gemini" => InjectionProfile {
            ready_via_bracketed_paste: false,
            post_spawn_settle: Duration::from_millis(2000),
        },
        "pi" => InjectionProfile {
            ready_via_bracketed_paste: false,
            post_spawn_settle: Duration::from_millis(1500),
        },
        "hermes" => InjectionProfile {
            ready_via_bracketed_paste: false,
            post_spawn_settle: Duration::from_millis(7000),
        },
        _ => DEFAULT_INJECTION_PROFILE,
    }
}

/// Ceiling for a preset-DECLARED settle floor (W4, 0.40.30). Defensive:
/// `readiness` is operator-editable TEXT, and a fat-fingered
/// `settle:999999999` must degrade to a long-but-bounded wait, never
/// park a wake/injection thread for days. 60s comfortably clears the
/// slowest studied agent (hermes, 7s) with an order of magnitude to
/// spare for slower community agents.
const MAX_DECLARED_SETTLE_MS: u64 = 60_000;

/// Parse one migration-0070 `agent_presets.readiness` value into a
/// profile. The declared vocabulary (see `0070_agent_preset_metadata.sql`):
///
/// - `bracketed-paste` — the ?2004h flip is trustworthy; poll it with
///   the default 400ms floor (claude's shipping behavior).
/// - `settle:<ms>` — ?2004h lies; wait `<ms>` (capped by
///   [`MAX_DECLARED_SETTLE_MS`]) instead of polling.
///
/// Anything else — unknown word, missing/non-numeric/negative ms —
/// returns `None` so [`resolve_injection_profile`] degrades to the
/// static provider table: a hand-corrupted row must never invent a
/// readiness dialect, and must never panic a spawn path.
fn parse_readiness_metadata(raw: &str) -> Option<InjectionProfile> {
    let raw = raw.trim();
    if raw == "bracketed-paste" {
        return Some(DEFAULT_INJECTION_PROFILE);
    }
    if let Some(ms_raw) = raw.strip_prefix("settle:") {
        if let Ok(ms) = ms_raw.trim().parse::<u64>() {
            return Some(InjectionProfile {
                ready_via_bracketed_paste: false,
                post_spawn_settle: Duration::from_millis(ms.min(MAX_DECLARED_SETTLE_MS)),
            });
        }
    }
    None
}

/// W4 (0.40.30) — THE injection-profile precedence chain, shared by
/// every injection site (the `/v1` host-session post-spawn injector,
/// `workspace_msg::deliver_post_wake`'s wake path, and
/// `wake_headless`'s settle computation). ONE resolution order, never
/// forked per call site:
///
/// 1. **Preset-declared `readiness` metadata** (migration 0070,
///    `agent_presets.readiness`) when present AND well-formed — the
///    operator/seed-declared truth for THIS preset's binary, which may
///    be a community agent the static table has never studied.
/// 2. **Static provider table** ([`injection_profile_for_provider`])
///    when the metadata is NULL or malformed — the 2026-07 study
///    results keyed by provider.
/// 3. **[`DEFAULT_INJECTION_PROFILE`]** for providers the table doesn't
///    know either (built into step 2's fallback arm) — the
///    claude-shaped shipping behavior, no new assumptions.
pub fn resolve_injection_profile(
    preset_readiness: Option<&str>,
    provider: &str,
) -> InjectionProfile {
    if let Some(raw) = preset_readiness {
        if let Some(profile) = parse_readiness_metadata(raw) {
            return profile;
        }
        if !raw.trim().is_empty() {
            crate::log_debug!(
                "[inject-profile] malformed preset readiness {:?}; degrading to the static provider table (provider={})",
                raw,
                provider
            );
        }
    }
    injection_profile_for_provider(provider)
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
            ("hermes", "hermes"),
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
        assert!(provider_resume_for_command("aider").is_none());
        assert!(provider_resume_for_command("bash").is_none());
        assert!(provider_resume_for_command("claudette").is_none());
        assert!(provider_resume_for_command("").is_none());
        assert!(provider_resume_for_provider("aider").is_none());
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
        for p in ["pi", "codex", "gemini", "cursor", "hermes"] {
            assert_eq!(
                provider_resume_for_provider(p).unwrap().premint_args(&[], "NEW"),
                None,
                "{p} mints its own ids — premint must be None"
            );
        }
    }

    // ── argv session-ownership matching (live-PTY scans) ────────────

    #[test]
    fn argv_references_session_matches_every_supported_grammar() {
        let sid = "11111111-2222-3333-4444-555555555555";
        // Flag styles (claude/grok/gemini/cursor/pi).
        for flag in ["--session-id", "--resume", "-r", "--session"] {
            let args = args(&["--dangerously-skip-permissions", flag, sid]);
            assert!(
                argv_references_session(&args, sid),
                "{flag} <id> must match"
            );
        }
        // Subcommand style (codex `resume <id>` at position 0).
        assert!(argv_references_session(&args(&["resume", sid]), sid));
        // A positional `resume` deeper in argv is NOT the codex
        // subcommand shape K2 assembles — no false positive.
        assert!(!argv_references_session(
            &args(&["--print", "resume", sid]),
            sid
        ));
        // Wrong id / empty id never match.
        assert!(!argv_references_session(&args(&["--resume", "other"]), sid));
        assert!(!argv_references_session(&args(&["--resume", sid]), ""));
        assert!(!argv_references_session(&[], sid));
    }

    // ── session_id_from_spawn_argv (registration stamping, Slice W3) ─

    #[test]
    fn spawn_argv_extraction_covers_every_premint_grammar() {
        let sid = "aaaaaaaa-0000-4000-8000-000000000001";
        // claude / grok premint (`--session-id <id>`).
        for cmd in ["claude", "grok", "/usr/local/bin/claude"] {
            assert_eq!(
                session_id_from_spawn_argv(cmd, &args(&["--session-id", sid])).as_deref(),
                Some(sid),
                "{cmd} premint argv must stamp"
            );
        }
    }

    #[test]
    fn spawn_argv_extraction_covers_every_flag_resume_grammar() {
        let sid = "aaaaaaaa-0000-4000-8000-000000000002";
        // Shared `--resume` (claude/grok/gemini/cursor/hermes) + `-r`.
        for cmd in ["claude", "grok", "gemini", "cursor-agent", "hermes"] {
            assert_eq!(
                session_id_from_spawn_argv(cmd, &args(&["--model", "x", "--resume", sid]))
                    .as_deref(),
                Some(sid),
                "{cmd} --resume argv must stamp"
            );
            assert_eq!(
                session_id_from_spawn_argv(cmd, &args(&["-r", sid])).as_deref(),
                Some(sid),
                "{cmd} -r argv must stamp"
            );
        }
        // Pi's `--session <id>` (its --resume is the interactive picker;
        // the shared aliases still count if a user typed them).
        assert_eq!(
            session_id_from_spawn_argv("pi", &args(&["--session", sid])).as_deref(),
            Some(sid),
            "pi --session was the missed grammar this slice exists for"
        );
        // A trailing flag with no value never stamps.
        assert_eq!(session_id_from_spawn_argv("pi", &args(&["--session"])), None);
        assert_eq!(session_id_from_spawn_argv("claude", &args(&["--resume"])), None);
    }

    #[test]
    fn spawn_argv_extraction_covers_the_subcommand_grammar() {
        let sid = "aaaaaaaa-0000-4000-8000-000000000003";
        // codex `resume <id>` — LEADING pair only.
        assert_eq!(
            session_id_from_spawn_argv("codex", &args(&["resume", sid])).as_deref(),
            Some(sid),
            "codex resume subcommand was the other missed grammar"
        );
        assert_eq!(
            session_id_from_spawn_argv("codex", &args(&["-c", "resume", sid])),
            None,
            "a deeper positional `resume` is not identity"
        );
        // codex has no premint and no flag grammar — a stray --resume in
        // codex argv is not the codex identity convention.
        assert_eq!(
            session_id_from_spawn_argv("codex", &args(&["--resume", sid])),
            None
        );
        // Bare self-minting spawn extracts nothing (post-hoc adoption's job).
        assert_eq!(session_id_from_spawn_argv("codex", &[]), None);
    }

    #[test]
    fn spawn_argv_extraction_unknown_command_keeps_legacy_scan() {
        let sid = "aaaaaaaa-0000-4000-8000-000000000004";
        // Unknown commands keep the historical --session-id/--resume pair
        // scan byte-identical…
        assert_eq!(
            session_id_from_spawn_argv("some-agent", &args(&["--session-id", sid])).as_deref(),
            Some(sid)
        );
        assert_eq!(
            session_id_from_spawn_argv("some-agent", &args(&["--resume", sid])).as_deref(),
            Some(sid)
        );
        // …and are deliberately NOT widened to the provider-only grammars.
        assert_eq!(
            session_id_from_spawn_argv("some-agent", &args(&["--session", sid])),
            None
        );
        assert_eq!(
            session_id_from_spawn_argv("some-agent", &args(&["resume", sid])),
            None
        );
        assert_eq!(session_id_from_spawn_argv("", &args(&["--resume", sid])).as_deref(), Some(sid));
    }

    #[test]
    fn spawn_argv_extraction_agrees_with_the_reference_matcher() {
        // DUALITY INVARIANT: any id the extractor stamps must satisfy the
        // live-PTY matcher (`argv_references_session`) — the read side of
        // the same grammar table. Exercise one argv per grammar.
        let sid = "aaaaaaaa-0000-4000-8000-000000000005";
        for (cmd, argv) in [
            ("claude", args(&["--session-id", sid])),
            ("grok", args(&["--always-approve", "--resume", sid])),
            ("gemini", args(&["-r", sid])),
            ("cursor-agent", args(&["--resume", sid])),
            ("hermes", args(&["--resume", sid])),
            ("pi", args(&["--session", sid])),
            ("codex", args(&["resume", sid])),
            ("unknown-cli", args(&["--session-id", sid])),
        ] {
            let extracted = session_id_from_spawn_argv(cmd, &argv)
                .unwrap_or_else(|| panic!("{cmd}: extraction must hit"));
            assert_eq!(extracted, sid, "{cmd}");
            assert!(
                argv_references_session(&argv, &extracted),
                "{cmd}: extracted id must satisfy argv_references_session"
            );
        }
    }

    #[test]
    fn argv_carries_session_identity_is_grammar_aware() {
        let claude = provider_resume_for_provider("claude").unwrap();
        assert!(claude.argv_carries_session_identity(&args(&["--session-id", "X"])));
        assert!(claude.argv_carries_session_identity(&args(&["--resume", "X"])));
        assert!(claude.argv_carries_session_identity(&args(&["-r", "X"])));
        assert!(!claude.argv_carries_session_identity(&args(&[
            "--dangerously-skip-permissions"
        ])));

        let pi = provider_resume_for_provider("pi").unwrap();
        assert!(pi.argv_carries_session_identity(&args(&["--session", "X"])));
        assert!(!pi.argv_carries_session_identity(&args(&["--model", "x"])));

        let codex = provider_resume_for_provider("codex").unwrap();
        assert!(codex.argv_carries_session_identity(&args(&["resume", "X"])));
        assert!(
            !codex.argv_carries_session_identity(&args(&["-c", "resume"])),
            "subcommand match is leading-position only"
        );
    }

    #[test]
    fn premint_flag_mirrors_the_premint_column() {
        assert_eq!(
            provider_resume_for_provider("claude").unwrap().premint_flag(),
            Some("--session-id")
        );
        assert_eq!(
            provider_resume_for_provider("grok").unwrap().premint_flag(),
            Some("--session-id")
        );
        for p in ["pi", "codex", "gemini", "cursor", "hermes"] {
            assert_eq!(
                provider_resume_for_provider(p).unwrap().premint_flag(),
                None
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

    /// Minimal `~/.hermes/state.db` fixture (study schema subset,
    /// WAL mode like the real thing). Returns the open writer
    /// connection so tests can hold it open while the adapter reads.
    fn seed_hermes_fixture(home: &std::path::Path) -> rusqlite::Connection {
        let db_dir = home.join(".hermes");
        std::fs::create_dir_all(&db_dir).unwrap();
        let conn = rusqlite::Connection::open(db_dir.join("state.db")).unwrap();
        conn.pragma_update(None, "journal_mode", "wal").unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY, source TEXT NOT NULL,
                 parent_session_id TEXT, started_at REAL NOT NULL,
                 ended_at REAL, end_reason TEXT, title TEXT, cwd TEXT,
                 archived INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE messages (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL, role TEXT NOT NULL,
                 content TEXT, timestamp REAL NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    fn insert_hermes_row(
        conn: &rusqlite::Connection,
        id: &str,
        source: &str,
        cwd: &str,
        started_at: f64,
        archived: i64,
    ) {
        conn.execute(
            "INSERT INTO sessions (id, source, cwd, started_at, archived) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, source, cwd, started_at, archived],
        )
        .unwrap();
    }

    #[test]
    fn hermes_adapter_exists_newest_and_source_filter_via_home() {
        let guard = HomeGuard::new("hermes-adapter");
        let project = "/Users/fixture/hermes-proj";
        let conn = seed_hermes_fixture(&guard.home);
        insert_hermes_row(&conn, "20260701_090000_aaaaaa", "cli", project, 1000.0, 0);
        insert_hermes_row(&conn, "20260702_100000_bbbbbb", "cli", project, 2000.0, 0);
        // Subagent + archived rows are newest — must lose newest_on_disk.
        insert_hermes_row(&conn, "20260703_110000_cccccc", "subagent", project, 3000.0, 0);
        insert_hermes_row(&conn, "20260703_120000_dddddd", "cli", project, 4000.0, 1);

        let hermes = provider_resume_for_provider("hermes").unwrap();
        assert_eq!(hermes.provider, "hermes");
        assert_eq!(hermes.command, "hermes");
        assert_eq!(hermes.grammar, ResumeGrammar::Flag("--resume"));
        assert_eq!(hermes.premint, None, "no premint — post-hoc discovery family");
        assert_eq!(
            hermes.resume_args(&args(&["--some-flag"]), "20260701_090000_aaaaaa"),
            args(&["--some-flag", "--resume", "20260701_090000_aaaaaa"])
        );

        assert!(hermes.session_file_exists("20260701_090000_aaaaaa", project));
        // Compression-chain rule: superseded/archived — even subagent —
        // ids exist; hermes redirects resume to the chain tip itself.
        assert!(hermes.session_file_exists("20260703_120000_dddddd", project));
        assert!(hermes.session_file_exists("20260703_110000_cccccc", project));
        assert!(!hermes.session_file_exists("20990101_000000_zzzzzz", project));
        assert!(!hermes.session_file_exists("", project));

        assert_eq!(
            hermes.newest_on_disk(project).as_deref(),
            Some("20260702_100000_bbbbbb"),
            "newest cli+unarchived by started_at — subagent/archived skipped"
        );
        assert_eq!(hermes.newest_on_disk("/Users/fixture/other-proj"), None);
    }

    #[test]
    fn hermes_adapter_degrades_when_db_absent() {
        // Fresh scratch HOME with no ~/.hermes at all — the adapter
        // must answer false/None, never error or block a spawn.
        let _guard = HomeGuard::new("hermes-absent");
        let hermes = provider_resume_for_provider("hermes").unwrap();
        assert!(!hermes.session_file_exists("20260701_090000_aaaaaa", "/x"));
        assert_eq!(hermes.newest_on_disk("/x"), None);
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
        assert_eq!(adopt_discovered_session("aider", "/nowhere"), None);
        // Must not spawn a thread / panic.
        defer_adopt_discovered_session("aider".into(), "/nowhere".into());
    }

    #[test]
    fn adopt_discovered_session_stamps_id_and_harness() {
        let guard = HomeGuard::new("adopt-stamp");
        crate::db::init_for_tests();
        let project_path = format!("/fixture/adopt-{}", uuid::Uuid::new_v4());
        let sid = "01920000-aaaa-7000-8000-00000000adop";
        write_grok_fixture(&guard.home, sid, &project_path, "2026-07-03T10:00:00Z", None);

        // Registered project + a pre-existing row with the legacy
        // 'claude' harness (what a bare-spawn row looks like before the
        // provider truth lands).
        let project_id = {
            let db = crate::db::shared();
            let conn = db.lock();
            let pid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, 'adopt', ?2)",
                rusqlite::params![pid, project_path],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
                 VALUES (?1, ?2, NULL, 'claude', 'user', 'running', unixepoch())",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), pid],
            )
            .unwrap();
            pid
        };

        assert_eq!(
            adopt_discovered_session("grok", &project_path).as_deref(),
            Some(sid),
            "must discover the newest on-disk grok session"
        );

        let db = crate::db::shared();
        let conn = db.lock();
        let row = crate::db::schema::WorkspaceSession::get(&conn, &project_id)
            .unwrap()
            .expect("row exists");
        assert_eq!(row.session_id.as_deref(), Some(sid), "session_id stamped");
        assert_eq!(row.harness, "grok", "harness stamped truthfully alongside the id");
    }

    // ── Slice 5 — injection profiles (per-provider readiness) ────────

    #[test]
    fn injection_profile_claude_is_exactly_the_shipping_behavior() {
        // DO NOT regress claude: poll bracketed paste with the 400ms
        // settle floor — byte-identical to the pre-slice-5 wake path.
        let p = injection_profile_for_provider("claude");
        assert!(p.ready_via_bracketed_paste);
        assert_eq!(p.post_spawn_settle, Duration::from_millis(400));
        assert_eq!(p, DEFAULT_INJECTION_PROFILE);
    }

    #[test]
    fn injection_profile_poll_trust_matches_the_studies() {
        // ?2004h trustworthy: claude (original study), grok (~1.1s,
        // "UI mounted"), cursor (trusted dir → composer exists).
        for provider in ["claude", "grok", "cursor"] {
            assert!(
                injection_profile_for_provider(provider).ready_via_bracketed_paste,
                "{provider} must poll bracketed paste for readiness"
            );
        }
        // ?2004h liars: codex/gemini/pi set it before real readiness,
        // hermes toggles it per repaint.
        for provider in ["codex", "gemini", "pi", "hermes"] {
            assert!(
                !injection_profile_for_provider(provider).ready_via_bracketed_paste,
                "{provider} must NOT trust bracketed paste as readiness"
            );
        }
    }

    #[test]
    fn injection_profile_settles_are_study_derived() {
        // hermes: ~7s first-message (prompt ~3.6s + ~3s agent init).
        assert_eq!(
            injection_profile_for_provider("hermes").post_spawn_settle,
            Duration::from_millis(7000)
        );
        // codex/gemini: 2s headroom past their dialog storms.
        assert_eq!(
            injection_profile_for_provider("codex").post_spawn_settle,
            Duration::from_millis(2000)
        );
        assert_eq!(
            injection_profile_for_provider("gemini").post_spawn_settle,
            Duration::from_millis(2000)
        );
        assert_eq!(
            injection_profile_for_provider("pi").post_spawn_settle,
            Duration::from_millis(1500)
        );
        assert_eq!(
            injection_profile_for_provider("cursor").post_spawn_settle,
            Duration::from_millis(1000)
        );
        // grok: ≥ study's ~1.1s ?2004h mount (not Claude's 400ms floor).
        assert_eq!(
            injection_profile_for_provider("grok").post_spawn_settle,
            Duration::from_millis(1500)
        );
        assert!(
            injection_profile_for_provider("grok").ready_via_bracketed_paste,
            "grok still polls ?2004h after the longer settle"
        );
    }

    #[test]
    fn injection_profile_unknown_provider_degrades_to_claude_shaped_default() {
        // Safe degradation: an unstudied agent keeps exactly the
        // pre-slice-5 behavior (poll + 400ms floor, bounded by the
        // caller's wake timeout).
        for provider in ["aider", "goose", "", "not-a-real-agent"] {
            assert_eq!(
                injection_profile_for_provider(provider),
                DEFAULT_INJECTION_PROFILE,
                "unknown provider {provider:?} must get the default profile"
            );
        }
    }

    // ── W4 (0.40.30) — the shared precedence chain ───────────────────

    #[test]
    fn resolve_profile_preset_settle_overrides_the_static_table() {
        // 'settle:2500' on a preset wins over BOTH a poll-trusting
        // static entry (claude) and the unknown-provider default.
        for provider in ["claude", "totally-custom-agent"] {
            let p = resolve_injection_profile(Some("settle:2500"), provider);
            assert!(
                !p.ready_via_bracketed_paste,
                "declared settle must disable the paste poll (provider={provider})"
            );
            assert_eq!(p.post_spawn_settle, Duration::from_millis(2500));
        }
        // Whitespace tolerance — operator-edited TEXT.
        let p = resolve_injection_profile(Some("  settle: 2500 "), "claude");
        assert_eq!(p.post_spawn_settle, Duration::from_millis(2500));
        assert!(!p.ready_via_bracketed_paste);
    }

    #[test]
    fn resolve_profile_preset_bracketed_paste_overrides_a_settle_provider() {
        // A preset declaring 'bracketed-paste' for a binary whose
        // provider the static table classes as a ?2004h liar takes the
        // declared (claude-shaped) poll profile — preset truth wins.
        let p = resolve_injection_profile(Some("bracketed-paste"), "hermes");
        assert_eq!(p, DEFAULT_INJECTION_PROFILE);
        let p = resolve_injection_profile(Some(" bracketed-paste "), "codex");
        assert_eq!(p, DEFAULT_INJECTION_PROFILE);
    }

    #[test]
    fn resolve_profile_malformed_readiness_degrades_to_the_static_table() {
        // Every malformed shape falls through to level 2 — the static
        // table still speaks the provider's studied dialect.
        for bad in ["settle:", "settle:abc", "settle:-5", "poll", "settle", "🚀"] {
            let p = resolve_injection_profile(Some(bad), "hermes");
            assert_eq!(
                p,
                injection_profile_for_provider("hermes"),
                "malformed readiness {bad:?} must degrade to the hermes table entry"
            );
            assert!(!p.ready_via_bracketed_paste);
            assert_eq!(p.post_spawn_settle, Duration::from_millis(7000));
        }
        // …and level 2's own unknown-arm still lands on the default.
        for bad in ["settle:abc", ""] {
            assert_eq!(
                resolve_injection_profile(Some(bad), "not-a-real-agent"),
                DEFAULT_INJECTION_PROFILE,
                "malformed readiness {bad:?} + unknown provider must degrade to the default"
            );
        }
    }

    #[test]
    fn resolve_profile_null_readiness_is_exactly_the_static_table() {
        // NULL metadata (the unstudied six + every legacy row) must be
        // byte-identical to the pre-W4 resolution for every provider.
        for provider in ["claude", "grok", "cursor", "codex", "gemini", "pi", "hermes", "x"] {
            assert_eq!(
                resolve_injection_profile(None, provider),
                injection_profile_for_provider(provider),
                "NULL readiness must resolve exactly the static entry for {provider}"
            );
        }
    }

    #[test]
    fn resolve_profile_declared_settle_is_capped_at_the_sanity_ceiling() {
        // A fat-fingered settle degrades to a bounded wait, never days.
        let p = resolve_injection_profile(Some("settle:999999999"), "claude");
        assert!(!p.ready_via_bracketed_paste);
        assert_eq!(p.post_spawn_settle, Duration::from_millis(60_000));
    }
}
