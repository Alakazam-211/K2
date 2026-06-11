//! `enable_fanout_for_enabled_agents_v1` — one-shot migration that
//! opts existing agent-mode workspaces INTO harness fan-out on upgrade
//! to the canonical-agents feature.
//!
//! The canonical-agents feature flips user-visible harness fan-out OFF
//! by default (see `.k2so/prds/k2-canonical-agents.md` §4): a fresh
//! workspace no longer auto-symlinks CLAUDE.md / GEMINI.md / etc. The
//! switch is the per-workspace `.k2so/.harness-fanout-enabled` marker
//! (`workspace::onboarding::harness_fanout_enabled`).
//!
//! Without a migration, every EXISTING workspace that was relying on the
//! old always-on fan-out would silently stop getting its harness files
//! refreshed on the first upgraded boot. PRD §5.7: auto-check the
//! permission box for workspaces that **have an enabled agent**
//! (Workspace Manager, K2 Agent, or Custom Agent) — those users are
//! already accustomed to the programmatic behavior, nothing changes for
//! them, and they can uncheck it later. **New workspaces, and any
//! WITHOUT an enabled agent → stay false** (the gentle opt-in default).
//!
//! Keyed on "has an enabled agent" — `agent_mode != 'off'` — a cleaner
//! signal than guessing from already-unified symlinks on disk. There is
//! **no auto-revert**: already-unified workspaces are left as they are.
//!
//! The marker is a `.k2so/`-internal filesystem flag (daemon-first), so
//! this migration is filesystem-touching like `legacy_agent_types_v1`:
//! the pure logic here takes `(project_path, agent_mode)` rows and
//! writes the marker; the DB read + `code_migrations` gating live at the
//! daemon call-site so the test suite needs no SQLite handle.

use crate::workspace::onboarding::set_harness_fanout_enabled;

/// Stable id stored in the `code_migrations` table once this migration
/// completes for the local DB.
pub const MIGRATION_ID: &str = "enable_fanout_for_enabled_agents_v1";

/// Outcome — count of workspaces whose fan-out marker was newly set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnableFanoutOutcome {
    /// Number of workspaces that got the opt-in marker written by this
    /// run. Idempotent re-runs (marker already present) do not
    /// re-count.
    pub enabled_count: usize,
}

/// Whether a workspace's `agent_mode` string denotes an ENABLED agent
/// (Workspace Manager incl. legacy aliases, K2 Agent, or Custom Agent).
///
/// `off` (and anything unrecognized / empty) → not enabled. This is the
/// "has an enabled agent" signal PRD §5.7 keys on.
pub fn is_enabled_agent_mode(agent_mode: &str) -> bool {
    matches!(
        agent_mode,
        "agent" | "custom" | "manager" | "coordinator" | "pod"
    )
}

/// Run the migration over a set of `(project_path, agent_mode)` rows.
///
/// For each row whose mode is an enabled agent (`is_enabled_agent_mode`)
/// AND whose marker is not already set, write the
/// `.k2so/.harness-fanout-enabled` marker. Rows with `agent_mode == off`
/// (or unrecognized) are skipped — they stay at the off-by-default
/// posture. Never clears a marker; never auto-reverts.
///
/// Returns the count of newly-enabled workspaces. Per-row marker-write
/// errors are tolerated (best-effort, like the sibling FS migrations) —
/// they're surfaced via the returned count being lower than the input,
/// not via a hard failure that would block daemon boot.
pub fn run<'a, I>(rows: I) -> EnableFanoutOutcome
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut enabled_count = 0usize;
    for (project_path, agent_mode) in rows {
        if !is_enabled_agent_mode(agent_mode) {
            continue;
        }
        // Idempotent: skip if already enabled (don't re-count).
        if crate::workspace::onboarding::harness_fanout_enabled(project_path) {
            continue;
        }
        // PRD §4 legacy alias: a workspace that explicitly skipped
        // harness management keeps that posture — set_harness_fanout
        // writes the positive marker, but harness_fanout_enabled still
        // reports false while the skip flag is present, so re-counting
        // is guarded by the check above. We still write the marker so
        // that removing the skip flag later honors the migration intent.
        if set_harness_fanout_enabled(project_path, true).is_ok() {
            enabled_count += 1;
        }
    }
    EnableFanoutOutcome { enabled_count }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-fanout-mig-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        fs::create_dir_all(dir.join(".k2so")).unwrap();
        dir
    }

    #[test]
    fn is_enabled_agent_mode_matches_the_three_agent_kinds() {
        for m in ["agent", "custom", "manager", "coordinator", "pod"] {
            assert!(is_enabled_agent_mode(m), "{m} should count as enabled");
        }
        for m in ["off", "", "unknown", "Off"] {
            assert!(!is_enabled_agent_mode(m), "{m:?} must NOT count as enabled");
        }
    }

    #[test]
    fn enables_only_enabled_agent_workspaces() {
        let agent_ws = temp_workspace();
        let manager_ws = temp_workspace();
        let custom_ws = temp_workspace();
        let off_ws = temp_workspace();

        let outcome = run([
            (agent_ws.to_str().unwrap(), "agent"),
            (manager_ws.to_str().unwrap(), "manager"),
            (custom_ws.to_str().unwrap(), "custom"),
            (off_ws.to_str().unwrap(), "off"),
        ]);

        assert_eq!(outcome.enabled_count, 3, "agent + manager + custom get enabled; off does not");
        assert!(crate::workspace::onboarding::harness_fanout_enabled(agent_ws.to_str().unwrap()));
        assert!(crate::workspace::onboarding::harness_fanout_enabled(manager_ws.to_str().unwrap()));
        assert!(crate::workspace::onboarding::harness_fanout_enabled(custom_ws.to_str().unwrap()));
        assert!(
            !crate::workspace::onboarding::harness_fanout_enabled(off_ws.to_str().unwrap()),
            "an off (no enabled agent) workspace must stay at the off-by-default posture",
        );

        for ws in [agent_ws, manager_ws, custom_ws, off_ws] {
            fs::remove_dir_all(&ws).ok();
        }
    }

    #[test]
    fn idempotent_second_run_is_noop() {
        let ws = temp_workspace();
        let path = ws.to_str().unwrap();
        let first = run([(path, "manager")]);
        assert_eq!(first.enabled_count, 1);
        let second = run([(path, "manager")]);
        assert_eq!(second.enabled_count, 0, "already-enabled workspace must not re-count");
        fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn does_not_auto_revert_already_enabled_off_workspace() {
        // A workspace that's off but already opted-in (e.g. user checked
        // the box manually) must NOT be reverted — we never clear a
        // marker. The off row is simply skipped.
        let ws = temp_workspace();
        let path = ws.to_str().unwrap();
        set_harness_fanout_enabled(path, true).unwrap();
        assert!(crate::workspace::onboarding::harness_fanout_enabled(path));

        let outcome = run([(path, "off")]);
        assert_eq!(outcome.enabled_count, 0, "off row is skipped (not re-counted)");
        assert!(
            crate::workspace::onboarding::harness_fanout_enabled(path),
            "migration must NEVER auto-revert an already-enabled workspace",
        );
        fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn empty_input_returns_zero() {
        let outcome = run(std::iter::empty());
        assert_eq!(outcome.enabled_count, 0);
    }
}
