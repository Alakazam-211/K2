//! `enable_fanout_for_enabled_agents_v1` — **NEUTERED** (no-op).
//!
//! ## What this used to do (and why it's gone)
//!
//! Originally (PRD `.k2/prds/k2-canonical-agents.md` §5.7) this one-shot
//! migration auto-checked the per-workspace harness-fanout box
//! (`.k2/.harness-fanout-enabled`) for EVERY existing workspace whose
//! `agent_mode != 'off'`, so upgraders accustomed to the old always-on
//! fan-out kept it.
//!
//! ## Why it was neutered (product-owner decision, supersedes §5.7)
//!
//! That is a **retroactive, fleet-wide application** of fan-out to
//! existing workspaces — exactly what the owner does NOT want. Harness
//! fan-out (symlinking CLAUDE.md / GEMINI.md / … onto K2's generated
//! canon) is **destructive-by-design** and must NEVER be applied in bulk
//! without per-workspace, user-confirmed intent. The ONLY surface that
//! applies/removes fan-out is the per-workspace "Canonical Agent"
//! checkbox (with its confirmation modal). There is no auto-apply on new
//! agents either: the former global "default for new agents" flag was
//! removed entirely, so turning a workspace into an agent does nothing to
//! its fan-out marker.
//!
//! ## Why a no-op instead of a deletion
//!
//! The original migration may already have RUN on some dev/user boxes
//! (it wrote the `code_migrations` sentinel + the markers). Deleting the
//! module would re-key the sentinel and is module-removal churn for no
//! gain. We instead make `run` a no-op so:
//!   - boxes that already ran it keep their state (we never auto-revert a
//!     marker — that would be its own surprising bulk teardown);
//!   - boxes that have NOT run it will record the sentinel and apply
//!     nothing, so no fresh bulk sweep ever happens again.
//!
//! There is intentionally no "undo" of markers a prior run wrote: ripping
//! fan-out away from workspaces that already have it would be the mirror
//! image of the bug we're fixing (a surprising bulk mutation). Users who
//! want it off uncheck the per-workspace box.

/// Stable id stored in the `code_migrations` table once this migration
/// completes for the local DB. Unchanged so already-run boxes stay
/// idempotent.
pub const MIGRATION_ID: &str = "enable_fanout_for_enabled_agents_v1";

/// Outcome — always zero now (this migration applies nothing).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnableFanoutOutcome {
    /// Always 0 — the neutered migration never writes a marker.
    pub enabled_count: usize,
}

/// **NO-OP.** Consumes the rows (so the daemon call-site is unchanged)
/// and applies nothing. See the module docs for the rationale: harness
/// fan-out is never applied retroactively in bulk; the per-workspace box
/// is the sole apply surface.
pub fn run<'a, I>(_rows: I) -> EnableFanoutOutcome
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    // Intentionally drain nothing and write nothing. The argument is kept
    // for call-site compatibility with the daemon boot runner.
    EnableFanoutOutcome { enabled_count: 0 }
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
    fn neutered_migration_never_applies_fanout_to_any_workspace() {
        // Every flavor of enabled agent that the ORIGINAL migration would
        // have swept in must now be left untouched — this is the core
        // guarantee: no retroactive bulk application.
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

        assert_eq!(
            outcome.enabled_count, 0,
            "neutered migration must enable ZERO workspaces (no retroactive bulk sweep)",
        );
        for ws in [&agent_ws, &manager_ws, &custom_ws, &off_ws] {
            assert!(
                !crate::workspace::onboarding::harness_fanout_enabled(ws.to_str().unwrap()),
                "no existing workspace may have fan-out applied by the migration: {ws:?}",
            );
        }

        for ws in [agent_ws, manager_ws, custom_ws, off_ws] {
            fs::remove_dir_all(&ws).ok();
        }
    }

    #[test]
    fn neutered_migration_does_not_clear_a_preexisting_marker() {
        // A box that already ran the OLD migration (or where the user
        // checked the box) must NOT be auto-reverted — that would be the
        // mirror-image surprising bulk mutation.
        let ws = temp_workspace();
        let path = ws.to_str().unwrap();
        crate::workspace::onboarding::set_harness_fanout_enabled(path, true).unwrap();
        assert!(crate::workspace::onboarding::harness_fanout_enabled(path));

        let outcome = run([(path, "manager")]);
        assert_eq!(outcome.enabled_count, 0);
        assert!(
            crate::workspace::onboarding::harness_fanout_enabled(path),
            "neutered migration must never clear an existing marker (no bulk teardown either)",
        );
        fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn empty_input_returns_zero() {
        let outcome = run(std::iter::empty());
        assert_eq!(outcome.enabled_count, 0);
    }
}
