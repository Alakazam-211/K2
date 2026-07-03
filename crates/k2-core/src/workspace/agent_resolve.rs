//! Agent de-generalization Slice 2 — the DAEMON-side default-agent
//! resolver. The single seam that turns "which agent should this
//! workspace spawn?" into a concrete command + args for every
//! daemon-initiated FRESH launch (canonical session ensure, wake
//! auto-launch, heartbeat fires, `/cli/agents/launch`, restart
//! recovery).
//!
//! Mirrors the renderer seam (`src/renderer/lib/agent-resolve.ts`)
//! including its VALUE TOLERANCE: the stored default-agent value is
//! canonically an `agent_presets` preset id (UUID), but legacy
//! values that hold a command first-token (e.g. `"claude"`) keep
//! resolving via a command-token match. No data migration.
//!
//! Precedence (see [`resolve_launch_profile`]):
//!   1. AGENT.md `launch:` block — the existing per-workspace
//!      override ([`load_launch_profile`]) keeps top priority.
//!   2. `projects.default_agent` (per-workspace default, Slice 1).
//!   3. Global `AppSettings.default_agent` (`~/.k2/settings.json`).
//!   4. Literal `claude --dangerously-skip-permissions` — byte-for-
//!      byte the pre-S2 `default_launch_profile()` the daemon spawn
//!      sites hardcoded, so an unset install behaves identically.
//!
//! Levels 2–3 resolve against the `agent_presets` table (preset id
//! first, then command first-token), skipping DISABLED presets. An
//! unresolvable value (deleted preset, disabled preset, typo) falls
//! through to the next level instead of erroring — a stale setting
//! must never brick a spawn.
//!
//! FLAG-GRAMMAR SCOPE GUARD (Slice 3): this module resolves the
//! COMMAND only. Claude-specific flag grammar (`--resume`,
//! `--session-id`, `--append-system-prompt`, …) stays at the call
//! sites, gated on [`ResolvedAgentCommand::is_claude`]. A non-claude
//! default spawns the preset's own command + args bare — correct but
//! degraded (no resume) until the Slice 3 ProviderResume adapter.

use rusqlite::Connection;

use crate::workspace::launch_profile::{load_launch_profile, LaunchProfile};

/// Level-4 fallback command — matches the built-in Claude preset and
/// the pre-S2 hardcoded `default_launch_profile()` daemon spawns.
pub const FALLBACK_AGENT_COMMAND: &str = "claude";
/// Level-4 fallback args (see [`FALLBACK_AGENT_COMMAND`]).
pub const FALLBACK_AGENT_ARGS: &[&str] = &["--dangerously-skip-permissions"];

/// Which precedence level produced the resolved command. Carried for
/// log lines + test assertions; call sites must not branch on it
/// (branch on [`ResolvedAgentCommand::is_claude`] instead).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedAgentSource {
    /// `projects.default_agent` matched an enabled preset.
    WorkspaceDefault,
    /// Global `AppSettings.default_agent` matched an enabled preset.
    GlobalDefault,
    /// Nothing resolved — literal claude fallback.
    FallbackClaude,
}

/// A resolved spawnable agent command: first token + remaining args
/// (quote-aware split of the preset's full `command` string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAgentCommand {
    /// Executable (first token of the preset command), e.g. "claude".
    pub command: String,
    /// Remaining preset args, e.g. ["--dangerously-skip-permissions"].
    pub args: Vec<String>,
    /// The matched `agent_presets.id`; None for the literal fallback.
    pub preset_id: Option<String>,
    pub source: ResolvedAgentSource,
}

impl ResolvedAgentCommand {
    /// True when the resolved executable is claude (basename match, so
    /// a path-qualified `~/bin/claude` counts). Call sites gate every
    /// piece of Claude flag grammar on this.
    pub fn is_claude(&self) -> bool {
        self.command
            .rsplit('/')
            .next()
            .map(|name| name == FALLBACK_AGENT_COMMAND)
            .unwrap_or(false)
    }

    /// Adapt to the `LaunchProfile` shape the spawn helpers consume.
    /// Command + args only — cwd/cols/rows/env stay caller defaults,
    /// same as the pre-S2 `default_launch_profile()`.
    pub fn to_launch_profile(&self) -> LaunchProfile {
        LaunchProfile {
            command: Some(self.command.clone()),
            args: Some(self.args.clone()),
            cwd: None,
            cols: None,
            rows: None,
            env: None,
        }
    }
}

fn fallback_claude() -> ResolvedAgentCommand {
    ResolvedAgentCommand {
        command: FALLBACK_AGENT_COMMAND.to_string(),
        args: FALLBACK_AGENT_ARGS.iter().map(|s| s.to_string()).collect(),
        preset_id: None,
        source: ResolvedAgentSource::FallbackClaude,
    }
}

/// Split a preset `command` string into command + args, respecting
/// single/double quotes. Byte-for-byte port of the renderer seam's
/// `parseCommand` (src/renderer/lib/agent-resolve.ts) so the same
/// preset row launches identically from either side.
pub fn parse_command_string(raw: &str) -> (String, Vec<String>) {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for ch in raw.chars() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        } else if ch == ' ' {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }

    let mut iter = parts.into_iter();
    let command = iter.next().unwrap_or_default();
    (command, iter.collect())
}

/// Append `flag` to `args` unless already present. Call sites use this
/// to guarantee headless-critical flags (e.g.
/// `--dangerously-skip-permissions`) survive a user-customized preset
/// command without duplicating them.
pub fn ensure_flag(args: &mut Vec<String>, flag: &str) {
    if !args.iter().any(|a| a == flag) {
        args.push(flag.to_string());
    }
}

/// First whitespace-delimited token of a preset's command string —
/// the legacy-value match key (renderer parity).
fn command_first_token(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or("")
}

/// Enabled presets in display order (sort_order, label) — the same
/// order the renderer store iterates, so token-match ties resolve
/// identically on both sides. DB errors degrade to an empty roster
/// (→ fallback), never a panic in a spawn path.
fn enabled_presets(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = match conn
        .prepare("SELECT id, command FROM agent_presets WHERE enabled = 1 ORDER BY sort_order, label")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    });
    match rows {
        Ok(mapped) => mapped.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// Match a stored default-agent VALUE against the enabled roster —
/// preset id first (canonical), then command first-token (legacy
/// tolerance). None = unresolvable at this level; caller falls
/// through to the next precedence level.
fn match_preset<'a>(
    presets: &'a [(String, String)],
    value: &str,
) -> Option<&'a (String, String)> {
    presets
        .iter()
        .find(|(id, _)| id == value)
        .or_else(|| {
            presets
                .iter()
                .find(|(_, command)| command_first_token(command) == value)
        })
}

/// Resolve one precedence level's stored value against the roster.
/// Returns None when the value is empty, matches nothing enabled, or
/// the matched preset's command string is blank (unspawnable row).
fn resolve_value(
    presets: &[(String, String)],
    value: &str,
    source: ResolvedAgentSource,
) -> Option<ResolvedAgentCommand> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (id, command_str) = match_preset(presets, value)?;
    let (command, args) = parse_command_string(command_str);
    if command.is_empty() {
        return None;
    }
    Some(ResolvedAgentCommand {
        command,
        args,
        preset_id: Some(id.clone()),
        source,
    })
}

/// `projects.default_agent` for a workspace addressed by PATH or ID
/// (the daemon holds one or the other depending on the call site).
/// NULL / empty = inherit (level 3). Unregistered workspace = None.
fn project_default_agent(conn: &Connection, project_path_or_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT default_agent FROM projects WHERE path = ?1 OR id = ?1 LIMIT 1",
        rusqlite::params![project_path_or_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .filter(|s| !s.trim().is_empty())
}

/// Precedence levels 2–4 with an injected global value — the pure,
/// deterministic core (tests use this to avoid touching the caller's
/// real `~/.k2/settings.json`). Production sites call
/// [`resolve_agent_command`].
pub fn resolve_agent_command_with_global(
    conn: &Connection,
    project_path_or_id: &str,
    global_default: Option<&str>,
) -> ResolvedAgentCommand {
    let presets = enabled_presets(conn);

    // Level 2 — per-workspace default. Unresolvable (deleted preset,
    // disabled preset, typo) falls through rather than erroring.
    if let Some(ws_value) = project_default_agent(conn, project_path_or_id) {
        if let Some(resolved) =
            resolve_value(&presets, &ws_value, ResolvedAgentSource::WorkspaceDefault)
        {
            return resolved;
        }
        crate::log_debug!(
            "[agent-resolve] workspace default {:?} for {:?} matched no enabled preset; falling through",
            ws_value,
            project_path_or_id
        );
    }

    // Level 3 — global default.
    if let Some(global) = global_default {
        if let Some(resolved) =
            resolve_value(&presets, global, ResolvedAgentSource::GlobalDefault)
        {
            return resolved;
        }
        if !global.trim().is_empty() {
            crate::log_debug!(
                "[agent-resolve] global default {:?} matched no enabled preset; falling back to claude",
                global
            );
        }
    }

    // Level 4 — literal claude.
    fallback_claude()
}

/// Resolve the effective default agent for a workspace (levels 2–4):
/// `projects.default_agent` → global `AppSettings.default_agent` →
/// literal claude. AGENT.md-aware callers want
/// [`resolve_launch_profile`] instead.
pub fn resolve_agent_command(
    conn: &Connection,
    project_path_or_id: &str,
) -> ResolvedAgentCommand {
    let global = crate::app_settings::load().default_agent;
    resolve_agent_command_with_global(conn, project_path_or_id, Some(&global))
}

/// The full 4-level resolution (see module docs): the AGENT.md
/// `launch:` block keeps top priority when present; otherwise the
/// workspace/global default agent supplies the profile. `Err` only
/// for a malformed AGENT.md frontmatter (same contract callers had
/// with `load_launch_profile`).
pub fn resolve_launch_profile(
    conn: &Connection,
    project_path: &str,
    agent_name: &str,
) -> Result<LaunchProfile, String> {
    match load_launch_profile(project_path, agent_name)? {
        Some(profile) => Ok(profile),
        None => Ok(resolve_agent_command(conn, project_path).to_launch_profile()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Insert a custom (non-built-in) preset row with a unique id so
    /// tests never mutate the seeded built-ins the rest of the shared
    /// in-memory DB relies on.
    fn insert_preset(
        conn: &Connection,
        command: &str,
        enabled: bool,
        sort_order: i64,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, ?2, ?3, '', ?4, ?5, 0)",
            rusqlite::params![id, format!("test-{id}"), command, enabled as i64, sort_order],
        )
        .expect("insert preset");
        id
    }

    fn insert_project(conn: &Connection, path: &str, default_agent: Option<&str>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path, default_agent) VALUES (?1, 'test', ?2, ?3)",
            rusqlite::params![id, path, default_agent],
        )
        .expect("insert project");
        id
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-agent-resolve-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4(),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── parse_command_string (renderer parseCommand parity) ─────────

    #[test]
    fn parse_command_string_splits_simple_commands() {
        assert_eq!(
            parse_command_string("claude --dangerously-skip-permissions"),
            (
                "claude".to_string(),
                vec!["--dangerously-skip-permissions".to_string()]
            )
        );
        assert_eq!(parse_command_string("pi"), ("pi".to_string(), vec![]));
        assert_eq!(parse_command_string(""), (String::new(), vec![]));
        assert_eq!(
            parse_command_string("  spaced   out  "),
            ("spaced".to_string(), vec!["out".to_string()])
        );
    }

    #[test]
    fn parse_command_string_respects_quotes() {
        // The seeded Codex preset's exact shape.
        let (cmd, args) = parse_command_string(
            "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox",
        );
        assert_eq!(cmd, "codex");
        assert_eq!(
            args,
            vec![
                "-c".to_string(),
                "model_reasoning_effort=high".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
            ]
        );

        let (cmd, args) = parse_command_string("run 'a b' \"c d\"");
        assert_eq!(cmd, "run");
        assert_eq!(args, vec!["a b".to_string(), "c d".to_string()]);
    }

    #[test]
    fn ensure_flag_appends_once() {
        let mut args = vec!["--model".to_string(), "opus".to_string()];
        ensure_flag(&mut args, "--dangerously-skip-permissions");
        ensure_flag(&mut args, "--dangerously-skip-permissions");
        assert_eq!(
            args.iter()
                .filter(|a| *a == "--dangerously-skip-permissions")
                .count(),
            1
        );
    }

    #[test]
    fn is_claude_matches_bare_and_path_qualified() {
        let mut r = fallback_claude();
        assert!(r.is_claude());
        r.command = "/usr/local/bin/claude".into();
        assert!(r.is_claude());
        r.command = "claudette".into();
        assert!(!r.is_claude());
        r.command = "pi".into();
        assert!(!r.is_claude());
    }

    // ── Precedence: workspace > global > claude ─────────────────────

    #[test]
    fn workspace_default_beats_global_default() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let ws_preset = insert_preset(&conn, "zz-ws-agent --fast", true, 900);
        let global_preset = insert_preset(&conn, "zz-global-agent", true, 901);
        let path = format!("/tmp/agent-resolve-ws-beats-global-{ws_preset}");
        insert_project(&conn, &path, Some(&ws_preset));

        let r = resolve_agent_command_with_global(&conn, &path, Some(&global_preset));
        assert_eq!(r.command, "zz-ws-agent");
        assert_eq!(r.args, vec!["--fast".to_string()]);
        assert_eq!(r.preset_id.as_deref(), Some(ws_preset.as_str()));
        assert_eq!(r.source, ResolvedAgentSource::WorkspaceDefault);
    }

    #[test]
    fn null_workspace_default_inherits_global() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let global_preset = insert_preset(&conn, "zz-global-only --yolo", true, 902);
        let path = format!("/tmp/agent-resolve-inherit-{global_preset}");
        insert_project(&conn, &path, None);

        let r = resolve_agent_command_with_global(&conn, &path, Some(&global_preset));
        assert_eq!(r.command, "zz-global-only");
        assert_eq!(r.args, vec!["--yolo".to_string()]);
        assert_eq!(r.source, ResolvedAgentSource::GlobalDefault);
    }

    #[test]
    fn nothing_resolvable_falls_back_to_literal_claude() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let path = "/tmp/agent-resolve-unregistered-workspace";
        // Unregistered project + garbage global → level 4.
        let r = resolve_agent_command_with_global(&conn, path, Some("no-such-agent-anywhere"));
        assert_eq!(r.command, "claude");
        assert_eq!(r.args, vec!["--dangerously-skip-permissions".to_string()]);
        assert_eq!(r.preset_id, None);
        assert_eq!(r.source, ResolvedAgentSource::FallbackClaude);

        // Empty/absent global too.
        let r = resolve_agent_command_with_global(&conn, path, None);
        assert_eq!(r.source, ResolvedAgentSource::FallbackClaude);
        let r = resolve_agent_command_with_global(&conn, path, Some("  "));
        assert_eq!(r.source, ResolvedAgentSource::FallbackClaude);
    }

    // ── Value tolerance: id + legacy command-token matching ─────────

    #[test]
    fn value_matches_by_preset_id_and_by_legacy_command_token() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let preset = insert_preset(&conn, "zz-token-agent --flag one", true, 903);
        let path_id = format!("/tmp/agent-resolve-by-id-{preset}");
        insert_project(&conn, &path_id, Some(&preset));

        // Canonical: stored preset id.
        let by_id = resolve_agent_command_with_global(&conn, &path_id, None);
        assert_eq!(by_id.command, "zz-token-agent");
        assert_eq!(by_id.args, vec!["--flag".to_string(), "one".to_string()]);

        // Legacy: stored command first-token.
        let path_tok = format!("/tmp/agent-resolve-by-token-{preset}");
        insert_project(&conn, &path_tok, Some("zz-token-agent"));
        let by_token = resolve_agent_command_with_global(&conn, &path_tok, None);
        assert_eq!(by_token.command, "zz-token-agent");
        assert_eq!(by_token.preset_id.as_deref(), Some(preset.as_str()));
    }

    #[test]
    fn resolves_by_project_id_as_well_as_path() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let preset = insert_preset(&conn, "zz-by-id-agent", true, 904);
        let path = format!("/tmp/agent-resolve-project-id-{preset}");
        let project_id = insert_project(&conn, &path, Some(&preset));

        let r = resolve_agent_command_with_global(&conn, &project_id, None);
        assert_eq!(r.command, "zz-by-id-agent");
        assert_eq!(r.source, ResolvedAgentSource::WorkspaceDefault);
    }

    // ── Disabled presets + unresolvable fallthrough ──────────────────

    #[test]
    fn disabled_preset_is_skipped_and_falls_through_to_global() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let disabled = insert_preset(&conn, "zz-disabled-agent", false, 905);
        let global_preset = insert_preset(&conn, "zz-still-on", true, 906);
        let path = format!("/tmp/agent-resolve-disabled-{disabled}");
        insert_project(&conn, &path, Some(&disabled));

        let r = resolve_agent_command_with_global(&conn, &path, Some(&global_preset));
        assert_eq!(
            r.command, "zz-still-on",
            "disabled workspace preset must fall through to the global level"
        );
        assert_eq!(r.source, ResolvedAgentSource::GlobalDefault);
    }

    #[test]
    fn unresolvable_workspace_value_falls_through_then_to_claude() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let path = "/tmp/agent-resolve-garbage-everywhere";
        insert_project(&conn, path, Some("deleted-preset-uuid"));

        let r = resolve_agent_command_with_global(&conn, path, Some("also-not-a-preset"));
        assert_eq!(r.command, "claude");
        assert_eq!(r.source, ResolvedAgentSource::FallbackClaude);
    }

    #[test]
    fn disabled_preset_id_never_matches_even_when_token_would() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        // Disabled row whose token equals the stored value; nothing
        // enabled carries that token → unresolvable → fallback.
        let disabled = insert_preset(&conn, "zz-off-token", false, 907);
        let path = format!("/tmp/agent-resolve-off-token-{disabled}");
        insert_project(&conn, &path, Some("zz-off-token"));

        let r = resolve_agent_command_with_global(&conn, &path, None);
        assert_eq!(r.source, ResolvedAgentSource::FallbackClaude);
    }

    // ── Integration-style: the routed default_launch_profile seam ───

    #[test]
    fn resolve_launch_profile_honors_projects_default_agent_when_no_agent_md() {
        // The daemon's canonical_session / providers spawn sites now
        // call resolve_launch_profile where they used a hardcoded
        // default_launch_profile(). Prove the workspace default
        // governs the produced LaunchProfile end-to-end (DB row →
        // preset roster → parsed profile).
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let preset = insert_preset(&conn, "zz-profile-agent --safe 'a b'", true, 908);
        let dir = scratch_dir("no-agent-md");
        let path = dir.to_string_lossy().into_owned();
        insert_project(&conn, &path, Some(&preset));

        let profile = resolve_launch_profile(&conn, &path, "some-agent")
            .expect("no AGENT.md is not an error");
        assert_eq!(profile.command.as_deref(), Some("zz-profile-agent"));
        assert_eq!(
            profile.args,
            Some(vec!["--safe".to_string(), "a b".to_string()])
        );
        assert_eq!(profile.cwd, None, "cwd stays caller default (project root)");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn agent_md_launch_block_keeps_top_priority_over_workspace_default() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let preset = insert_preset(&conn, "zz-should-lose", true, 909);
        let dir = scratch_dir("agent-md-wins");
        let path = dir.to_string_lossy().into_owned();
        insert_project(&conn, &path, Some(&preset));

        // AGENT.md with an explicit launch block (level 1).
        let agent_dir = dir.join(".k2so/agents/pinned");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("AGENT.md"),
            "---\nname: pinned\nlaunch:\n  command: bash\n  args: [\"-c\", \"echo hi\"]\n---\nbody\n",
        )
        .unwrap();

        let profile = resolve_launch_profile(&conn, &path, "pinned").expect("parses");
        assert_eq!(
            profile.command.as_deref(),
            Some("bash"),
            "AGENT.md launch block must beat projects.default_agent"
        );
        assert_eq!(
            profile.args,
            Some(vec!["-c".to_string(), "echo hi".to_string()])
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_launch_profile_surfaces_malformed_agent_md_as_err() {
        let db = crate::db::init_for_tests();
        let conn = db.lock();
        let dir = scratch_dir("bad-yaml");
        let path = dir.to_string_lossy().into_owned();

        let agent_dir = dir.join(".k2so/agents/broken");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("AGENT.md"),
            "---\nlaunch:\n  command: bash\n args: [1, 2\n---\n",
        )
        .unwrap();

        let err = resolve_launch_profile(&conn, &path, "broken")
            .expect_err("malformed YAML must stay a loud Err, not a silent fallback");
        assert!(err.contains("frontmatter"), "diagnostic names the frontmatter: {err}");

        fs::remove_dir_all(&dir).ok();
    }
}
