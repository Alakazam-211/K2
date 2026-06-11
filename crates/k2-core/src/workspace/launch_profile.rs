//! Agent launch profile — the per-agent block in `agent.md`
//! frontmatter that tells the daemon HOW to spawn a session for
//! that agent when one doesn't already exist.
//!
//! G3 of Phase 3.2. This is the data layer that G4 builds on:
//! `DaemonWakeProvider::wake` for an offline agent looks up the
//! agent's `LaunchProfile`, hands it to `spawn_session_stream`, and
//! the queued signal lands in the fresh session's PTY as the target's
//! first byte of input.
//!
//! The profile lives in `.k2so/agents/<name>/AGENT.md` as an
//! OPTIONAL `launch:` block in the YAML frontmatter. The flat
//! `parse_frontmatter` helper (used elsewhere for `name:`, `role:`,
//! `type:`) doesn't understand nested blocks, so this module uses
//! `serde_yaml` to parse the full frontmatter.
//!
//! **Schema** (everything inside `launch:` is optional):
//!
//! ```yaml
//! ---
//! name: bar
//! role: Example agent
//! launch:
//!   command: bash                # shell cmd string; absent → default shell
//!   args: ["-c", "echo hi"]      # extra args appended after escaping
//!   cwd: "."                     # project-relative; `~` expands to HOME
//!   cols: 120
//!   rows: 40
//!   env:
//!     FOO: bar
//!     BAZ: qux
//! coordination_level: moderate   # none | minimal | moderate | chatty (G5)
//! ---
//! ```
//!
//! **Backwards compatible.** Every existing `agent.md` without a
//! `launch:` block parses cleanly and yields `None` from
//! `load_launch_profile`. Callers handle the `None` case (no auto-
//! launch; stay in queue-only mode).
//!
//! **Pure parser.** No I/O in the serialization layer. The loader
//! (`load_launch_profile`) does read the file, but parsing itself is
//! pure and directly testable via `parse_launch_profile_from_yaml`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace::agent_identity::agent_dir;

/// Coordination-level tag. Controls per-session message budgets in
/// G5. Five levels by design — agent authors pick one, budgets map
/// to concrete emit counts in `awareness::budget`. `#[non_exhaustive]`
/// so we can add a level without breaking downstream crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum CoordinationLevel {
    /// Budget = 0 — agent may NEVER emit to the bus. For agents
    /// that are pure consumers or background compactors.
    None,
    /// Budget = 2 emits per session. Silent-by-default agents.
    Minimal,
    /// Budget = 5 emits per session. The default.
    Moderate,
    /// Budget = 10 emits per session. Chatty-by-design agents
    /// (pair programmers, narrators).
    Chatty,
}

impl Default for CoordinationLevel {
    fn default() -> Self {
        Self::Moderate
    }
}

impl CoordinationLevel {
    /// Concrete per-session emit budget this level maps to. Consumed
    /// by the G5 budget tracker. Locked in this fn so the budget
    /// numbers live next to the level vocabulary — changing one
    /// without the other would desync the activity_feed audit and
    /// agent expectations.
    pub fn budget(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Minimal => 2,
            Self::Moderate => 5,
            Self::Chatty => 10,
        }
    }
}

/// Everything the daemon needs to spawn a session for an agent.
/// Every field optional — caller supplies defaults for absent
/// values when invoking the spawn path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchProfile {
    /// Shell command string. Passed verbatim as the `-ilc` arg of
    /// the user's default shell (see
    /// `terminal::session_stream_pty::spawn_session_stream`). If
    /// `None`, the daemon launches the default interactive shell.
    #[serde(default)]
    pub command: Option<String>,
    /// Additional arguments appended after the command. Each is
    /// shell-escaped before concatenation so `cmd "has spaces"`
    /// stays one argument. `None` or empty = no extra args.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Working directory. Project-relative by default; `~` is
    /// expanded to the current user's HOME. `None` = project root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Initial PTY column count. Defaults to 80 when unset.
    #[serde(default)]
    pub cols: Option<u16>,
    /// Initial PTY row count. Defaults to 24 when unset.
    #[serde(default)]
    pub rows: Option<u16>,
    /// Environment overrides merged on top of the daemon's inherited
    /// env when spawning. Profile takes precedence over inherited
    /// values for overlapping keys. Use `BTreeMap` for stable
    /// serialization order in tests + human-readable diffs.
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
}

/// The full shape of fields this module cares about from the
/// frontmatter. Fields outside this shape (like `name`, `role`,
/// `type`) are ignored via `#[serde(default)]` + extras-ignoring
/// behavior, so we don't reject a frontmatter with extra keys.
#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    launch: Option<LaunchProfile>,
    #[serde(default)]
    coordination_level: Option<CoordinationLevel>,
}

/// Parse results for a single agent's frontmatter. Either or both
/// fields may be absent — absence is not an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentConfig {
    pub launch: Option<LaunchProfile>,
    pub coordination_level: Option<CoordinationLevel>,
}

/// Pure parser. Takes the frontmatter YAML body (between the two
/// `---` fences, NOT including the fences) and returns the agent
/// config. Malformed YAML returns an `Err`; absent fields return
/// defaults (`None`) inside `Ok`.
pub fn parse_agent_config_from_yaml(yaml: &str) -> Result<AgentConfig, serde_yaml::Error> {
    let shape: FrontmatterShape = serde_yaml::from_str(yaml)?;
    Ok(AgentConfig {
        launch: shape.launch,
        coordination_level: shape.coordination_level,
    })
}

/// Extract the frontmatter YAML body from an `agent.md` string.
/// Returns `None` if the file doesn't start with `---` or the
/// second `---` fence is missing. Matches the tolerance of
/// `parse_frontmatter` — empty / malformed files simply yield no
/// config rather than erroring.
pub fn extract_frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))?;
    let end_idx = rest.find("\n---")?;
    Some(&rest[..end_idx])
}

/// Load a named agent's config from disk. Returns `Ok(None)` if
/// the agent directory doesn't exist or its `AGENT.md` file is
/// missing — absent = "agent not declared in this project," not an
/// error. Returns `Err` only if the YAML is malformed (the caller
/// can decide to ignore the profile or surface the error).
///
/// Filename is the canonical `AGENT.md` (all-caps) per the 0.33.0
/// naming convention — other agent-system modules
/// (`agents::commands`, `skill_content`, `scheduler`) all use the
/// same casing, so keeping this loader consistent prevents drift.
pub fn load_agent_config(
    project_path: &str,
    agent_name: &str,
) -> Result<Option<AgentConfig>, String> {
    let dir = agent_dir(project_path, agent_name);
    if !dir.exists() {
        return Ok(None);
    }
    let md_path: PathBuf = dir.join("AGENT.md");
    let Ok(content) = fs::read_to_string(&md_path) else {
        return Ok(None);
    };
    let Some(yaml) = extract_frontmatter(&content) else {
        // Empty or no frontmatter is fine; return a default config
        // so callers can still check for a missing launch block.
        return Ok(Some(AgentConfig::default()));
    };
    parse_agent_config_from_yaml(yaml)
        .map(Some)
        .map_err(|e| format!("parse {:?} frontmatter: {e}", md_path))
}

/// Convenience wrapper: just the launch profile, ignoring any
/// coordination_level (G4's scheduler-wake path is all that cares).
pub fn load_launch_profile(
    project_path: &str,
    agent_name: &str,
) -> Result<Option<LaunchProfile>, String> {
    Ok(load_agent_config(project_path, agent_name)?
        .and_then(|cfg| cfg.launch))
}

/// Convenience wrapper: just the coordination level, defaulting to
/// `Moderate` when the agent has no config or no coord level field.
/// G5's budget tracker uses this to apply per-emit limits.
pub fn load_coordination_level(
    project_path: &str,
    agent_name: &str,
) -> CoordinationLevel {
    load_agent_config(project_path, agent_name)
        .ok()
        .flatten()
        .and_then(|cfg| cfg.coordination_level)
        .unwrap_or_default()
}

/// Expand `~` (and `~/`-prefixed paths) in `cwd` to the user's
/// HOME. Relative paths are resolved against `project_root`. Returns
/// the absolute path as a `PathBuf` — the spawn path feeds this
/// to `CommandBuilder.cwd()`.
pub fn resolve_cwd(project_root: &Path, cwd: Option<&str>) -> PathBuf {
    let Some(cwd) = cwd else {
        return project_root.to_path_buf();
    };
    if cwd == "~" {
        return dirs::home_dir().unwrap_or_else(|| project_root.to_path_buf());
    }
    if let Some(rest) = cwd.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    let path = Path::new(cwd);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pure YAML parsing ─────────────────────────────────────────

    #[test]
    fn empty_frontmatter_returns_defaults() {
        let cfg = parse_agent_config_from_yaml("").unwrap();
        assert_eq!(cfg.launch, None);
        assert_eq!(cfg.coordination_level, None);
    }

    #[test]
    fn frontmatter_with_only_unrelated_fields_returns_defaults() {
        let yaml = "name: bar\nrole: Example agent\ntype: manager\n";
        let cfg = parse_agent_config_from_yaml(yaml).unwrap();
        assert_eq!(cfg.launch, None);
        assert_eq!(cfg.coordination_level, None);
    }

    #[test]
    fn minimal_launch_block_parses() {
        let yaml = "launch:\n  command: bash\n";
        let cfg = parse_agent_config_from_yaml(yaml).unwrap();
        let launch = cfg.launch.expect("launch block should parse");
        assert_eq!(launch.command.as_deref(), Some("bash"));
        assert_eq!(launch.args, None);
        assert_eq!(launch.cols, None);
        assert_eq!(launch.rows, None);
    }

    #[test]
    fn full_launch_block_parses_with_every_field() {
        let yaml = r#"
name: test
launch:
  command: "my prog"
  args:
    - "-c"
    - "echo hi"
  cwd: "~/proj"
  cols: 120
  rows: 40
  env:
    FOO: bar
    BAZ: qux
coordination_level: chatty
"#;
        let cfg = parse_agent_config_from_yaml(yaml).unwrap();
        let launch = cfg.launch.expect("launch should parse");
        assert_eq!(launch.command.as_deref(), Some("my prog"));
        assert_eq!(
            launch.args.as_deref(),
            Some(&["-c".to_string(), "echo hi".to_string()][..])
        );
        assert_eq!(launch.cwd.as_deref(), Some("~/proj"));
        assert_eq!(launch.cols, Some(120));
        assert_eq!(launch.rows, Some(40));
        let env = launch.env.as_ref().expect("env should parse");
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(env.get("BAZ").map(String::as_str), Some("qux"));

        assert_eq!(cfg.coordination_level, Some(CoordinationLevel::Chatty));
    }

    #[test]
    fn malformed_yaml_returns_err() {
        // Bad indentation that actual YAML parsers reject.
        let yaml = "launch:\n  command: bash\n args: [1, 2\n";
        assert!(parse_agent_config_from_yaml(yaml).is_err());
    }

    #[test]
    fn coordination_level_parses_all_levels() {
        for (text, want) in [
            ("none", CoordinationLevel::None),
            ("minimal", CoordinationLevel::Minimal),
            ("moderate", CoordinationLevel::Moderate),
            ("chatty", CoordinationLevel::Chatty),
        ] {
            let yaml = format!("coordination_level: {text}\n");
            let cfg = parse_agent_config_from_yaml(&yaml).unwrap();
            assert_eq!(cfg.coordination_level, Some(want), "text={text}");
        }
    }

    #[test]
    fn coordination_level_rejects_unknown_tag() {
        let yaml = "coordination_level: screaming\n";
        assert!(parse_agent_config_from_yaml(yaml).is_err());
    }

    // ── Frontmatter extraction ────────────────────────────────────

    #[test]
    fn extract_frontmatter_handles_lf_and_crlf() {
        let lf = "---\nname: bar\n---\nbody here\n";
        assert_eq!(extract_frontmatter(lf), Some("name: bar"));

        let crlf = "---\r\nname: bar\r\n---\r\nbody\r\n";
        // CRLF input: the YAML body retains its CR, parser tolerates.
        let yaml = extract_frontmatter(crlf).unwrap();
        assert!(yaml.contains("name: bar"));
    }

    #[test]
    fn extract_frontmatter_missing_fences_returns_none() {
        assert_eq!(extract_frontmatter(""), None);
        assert_eq!(extract_frontmatter("no frontmatter here"), None);
        assert_eq!(extract_frontmatter("---\nno closing fence\n"), None);
    }

    // ── Budget mapping ────────────────────────────────────────────

    #[test]
    fn budget_mapping_matches_pi_messenger_spec() {
        assert_eq!(CoordinationLevel::None.budget(), 0);
        assert_eq!(CoordinationLevel::Minimal.budget(), 2);
        assert_eq!(CoordinationLevel::Moderate.budget(), 5);
        assert_eq!(CoordinationLevel::Chatty.budget(), 10);
    }

    #[test]
    fn default_coordination_level_is_moderate() {
        assert_eq!(CoordinationLevel::default(), CoordinationLevel::Moderate);
    }

    // ── cwd resolution ────────────────────────────────────────────

    #[test]
    fn resolve_cwd_handles_none_and_absolute() {
        let root = PathBuf::from("/tmp/project");
        assert_eq!(resolve_cwd(&root, None), root);
        assert_eq!(
            resolve_cwd(&root, Some("/absolute/path")),
            PathBuf::from("/absolute/path")
        );
    }

    #[test]
    fn resolve_cwd_relative_joins_project_root() {
        let root = PathBuf::from("/tmp/project");
        assert_eq!(
            resolve_cwd(&root, Some("sub/dir")),
            PathBuf::from("/tmp/project/sub/dir")
        );
        assert_eq!(resolve_cwd(&root, Some(".")), PathBuf::from("/tmp/project/."));
    }

    #[test]
    fn resolve_cwd_expands_tilde() {
        let root = PathBuf::from("/tmp/project");
        let home = dirs::home_dir().expect("test needs HOME");
        assert_eq!(resolve_cwd(&root, Some("~")), home);
        assert_eq!(
            resolve_cwd(&root, Some("~/nested/dir")),
            home.join("nested/dir")
        );
    }

    // ── load_agent_config end-to-end (filesystem) ─────────────────

    fn scratch_project() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2so-launch-profile-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_agent(project: &Path, name: &str, content: &str) {
        let agent_dir = project.join(".k2so/agents").join(name);
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("AGENT.md"), content).unwrap();
    }

    #[test]
    fn load_agent_config_returns_none_for_missing_agent() {
        let project = scratch_project();
        let cfg =
            load_agent_config(project.to_str().unwrap(), "nonexistent").unwrap();
        assert!(cfg.is_none());
    }

    #[test]
    fn load_agent_config_returns_some_default_when_no_frontmatter() {
        let project = scratch_project();
        write_agent(&project, "plain", "just a body, no frontmatter\n");
        let cfg =
            load_agent_config(project.to_str().unwrap(), "plain").unwrap();
        let cfg = cfg.expect("agent dir exists");
        assert_eq!(cfg.launch, None);
        assert_eq!(cfg.coordination_level, None);
    }

    #[test]
    fn load_agent_config_returns_parsed_profile() {
        let project = scratch_project();
        write_agent(
            &project,
            "bar",
            "---\nname: bar\nlaunch:\n  command: bash\n  cols: 132\ncoordination_level: minimal\n---\n\nBody here.\n",
        );
        let cfg = load_agent_config(project.to_str().unwrap(), "bar")
            .unwrap()
            .expect("agent exists");
        let launch = cfg.launch.expect("launch block present");
        assert_eq!(launch.command.as_deref(), Some("bash"));
        assert_eq!(launch.cols, Some(132));
        assert_eq!(cfg.coordination_level, Some(CoordinationLevel::Minimal));
    }

    #[test]
    fn load_launch_profile_convenience_wrapper() {
        let project = scratch_project();
        write_agent(
            &project,
            "shell",
            "---\nlaunch:\n  command: zsh\n---\nbody\n",
        );
        let profile = load_launch_profile(project.to_str().unwrap(), "shell")
            .unwrap()
            .expect("profile present");
        assert_eq!(profile.command.as_deref(), Some("zsh"));
    }

    #[test]
    fn load_coordination_level_defaults_when_absent() {
        let project = scratch_project();
        write_agent(&project, "default", "---\nname: default\n---\n");
        let level =
            load_coordination_level(project.to_str().unwrap(), "default");
        assert_eq!(level, CoordinationLevel::Moderate);

        // Also: missing agent dir → default
        let level =
            load_coordination_level(project.to_str().unwrap(), "ghost");
        assert_eq!(level, CoordinationLevel::Moderate);
    }

    #[test]
    fn lowercase_agent_md_is_not_loaded() {
        // Canonical filename is AGENT.md (0.33.0 naming rule).
        // A stray lowercase agent.md should NOT be picked up —
        // either the project is misconfigured, or we'd be silently
        // routing around the convention. Returning `Ok(None)` keeps
        // the daemon's behavior predictable: agent has no config,
        // queue-only delivery, no auto-launch.
        //
        // Note: on case-insensitive filesystems (macOS default) this
        // test can't distinguish AGENT.md from agent.md; the test
        // would write AGENT.md anyway. We skip the assertion when
        // the underlying FS merges the names.
        let project = scratch_project();
        let agent_dir = project.join(".k2so/agents").join("lower-only");
        fs::create_dir_all(&agent_dir).unwrap();
        let lower = agent_dir.join("agent.md");
        fs::write(&lower, "---\nlaunch:\n  command: lower\n---\n").unwrap();
        let upper = agent_dir.join("AGENT.md");
        // If FS is case-insensitive, the file just written as
        // agent.md is accessible via AGENT.md too; can't test.
        if upper.exists() {
            return;
        }
        // Case-sensitive FS path (Linux CI).
        let config =
            load_agent_config(project.to_str().unwrap(), "lower-only")
                .unwrap();
        assert!(
            config.is_none(),
            "lowercase agent.md should NOT be loaded when AGENT.md is absent"
        );
    }
}
