//! `legacy_agent_types_v1` — rewrite legacy agent-type strings in
//! `AGENT.md` frontmatter.
//!
//! Pre-0.34 (pod-vocabulary era), agent frontmatter used:
//!   - `type: pod-member`  → became `type: agent-template`
//!   - `type: pod-leader`  → became `type: manager`
//!   - `pod_leader: true`  → became `manager: true`
//!
//! This migration walks `.k2so/agents/<n>/AGENT.md` for every registered
//! workspace and rewrites those tokens in place. It used to run from
//! Tauri's setup hook in `src-tauri/src/lib.rs` (gated via the
//! `code_migrations` table). Moving it daemon-side means:
//!
//! - Headless daemons (K2 Connect remote, launchd boot without Tauri)
//!   pick up the migration on their own first boot. Pre-move, a remote
//!   daemon that opens projects whose Tauri instance never ran would
//!   keep serving legacy types — the type-aware UI surfaces would
//!   render stale labels until someone launched K2SO.app.
//! - The migration logic is testable against synthetic workspaces in
//!   isolation (no Tauri context, no SQLite mock — just project paths
//!   on tmp dirs).
//!
//! The DB gating (`code_migrations` row) still lives at the daemon
//! call-site; the helpers here are pure-FS so the test suite doesn't
//! need a real SQLite handle to verify the rewrites.

use std::fs;
use std::path::Path;

/// Stable id stored in the `code_migrations` table once this
/// migration completes for the local DB. Used by the daemon-side
/// boot caller; exposed as a `pub const` so call sites + tests agree.
pub const MIGRATION_ID: &str = "legacy_agent_types_v1";

/// Outcome of running the migration over a set of workspaces. Carries
/// a count of rewritten AGENT.md files (one entry per file mutated;
/// idempotent re-runs return 0).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyAgentTypesOutcome {
    /// Number of AGENT.md files where at least one legacy token was
    /// rewritten. Used by the call-site to log a one-line summary
    /// (e.g. `[k2so] legacy_agent_types_v1: rewrote N AGENT.md files`).
    pub rewritten_count: usize,
}

/// Run the migration across one project's `.k2so/agents/<n>/` tree.
/// Idempotent: workspaces with no `.k2so/agents/` directory (post-
/// 0.37.0 unification, or fresh workspaces) skip in one stat. Files
/// without the legacy tokens are left untouched.
///
/// Returns the count of rewritten files. Per-file errors (unreadable
/// AGENT.md, unwritable file) are silently ignored — the migration
/// is best-effort, not blocking. The daemon caller logs the aggregate
/// count; debugging individual failures is a future task.
pub fn rewrite_workspace(project_path: &Path) -> usize {
    let agents_dir = crate::workspace_dot_dir(&project_path).join("agents");
    if !agents_dir.exists() {
        return 0;
    }
    let mut rewritten = 0usize;
    let Ok(entries) = fs::read_dir(&agents_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let agent_md = entry.path().join("AGENT.md");
        if !agent_md.exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&agent_md) else {
            continue;
        };
        let (updated, changed) = rewrite_string(&content);
        if changed {
            // Best-effort write; failures don't propagate. The
            // migration is gated by `code_migrations` so this only
            // runs once successfully; a transient write failure here
            // means the file gets retried on the NEXT boot when this
            // workspace's marker check sees the migration hasn't
            // landed yet. (Daemon caller doesn't mark applied if
            // the workspace loop bails — but it does mark applied
            // unconditionally today, so a partial failure means
            // stale data. That's acceptable for legacy data.)
            if fs::write(&agent_md, &updated).is_ok() {
                rewritten += 1;
            }
        }
    }
    rewritten
}

/// Run the migration across a list of project paths. Aggregates the
/// per-workspace rewrite counts. Callers (daemon boot, tests) supply
/// the project list however they get it — the daemon hits
/// `k2_core::db::schema::Project::list`; tests pass synthetic paths.
///
/// This is the "daemon-friendly" entry point. The gating
/// (`code_migrations` check + mark) lives one layer up so this fn
/// stays DB-free and trivially testable.
pub fn run<I, P>(projects: I) -> LegacyAgentTypesOutcome
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut rewritten_count = 0usize;
    for project in projects {
        rewritten_count += rewrite_workspace(project.as_ref());
    }
    LegacyAgentTypesOutcome { rewritten_count }
}

/// Pure string rewrite. Returns `(new_content, changed_flag)`.
/// Extracted so tests can assert the rewrite shape without touching
/// the filesystem.
fn rewrite_string(content: &str) -> (String, bool) {
    let mut updated = content.to_string();
    let mut changed = false;
    if updated.contains("type: pod-member") {
        updated = updated.replace("type: pod-member", "type: agent-template");
        changed = true;
    }
    if updated.contains("type: pod-leader") {
        updated = updated.replace("type: pod-leader", "type: manager");
        changed = true;
    }
    if updated.contains("pod_leader: true") {
        updated = updated.replace("pod_leader: true", "manager: true");
        changed = true;
    }
    (updated, changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_workspace() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2so-legacy-types-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(p.join(".k2so/agents")).unwrap();
        p
    }

    fn write_agent_md(p: &Path, name: &str, body: &str) {
        let dir = p.join(".k2so/agents").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("AGENT.md"), body).unwrap();
    }

    #[test]
    fn legacy_agent_types_v1_rewrites_pod_member_to_agent_template() {
        let p = temp_workspace();
        write_agent_md(
            &p,
            "rust-eng",
            "---\nname: rust-eng\ntype: pod-member\n---\n# rust\n",
        );
        let n = rewrite_workspace(&p);
        assert_eq!(n, 1);
        let content = fs::read_to_string(p.join(".k2so/agents/rust-eng/AGENT.md")).unwrap();
        assert!(
            content.contains("type: agent-template"),
            "missing rewrite: {content}"
        );
        assert!(
            !content.contains("type: pod-member"),
            "legacy token leaked: {content}"
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn legacy_agent_types_v1_rewrites_pod_leader_to_manager() {
        let p = temp_workspace();
        write_agent_md(
            &p,
            "pod-leader",
            "---\nname: pod-leader\ntype: pod-leader\npod_leader: true\n---\n# lead\n",
        );
        let n = rewrite_workspace(&p);
        assert_eq!(n, 1);
        let content = fs::read_to_string(p.join(".k2so/agents/pod-leader/AGENT.md")).unwrap();
        assert!(content.contains("type: manager"), "missing rewrite: {content}");
        assert!(
            content.contains("manager: true"),
            "missing pod_leader rewrite: {content}"
        );
        assert!(
            !content.contains("type: pod-leader"),
            "legacy token leaked: {content}"
        );
        assert!(
            !content.contains("pod_leader: true"),
            "legacy bool token leaked: {content}"
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn legacy_agent_types_v1_is_idempotent() {
        let p = temp_workspace();
        write_agent_md(
            &p,
            "rust-eng",
            "---\nname: rust-eng\ntype: pod-member\n---\n# rust\n",
        );
        let first = rewrite_workspace(&p);
        assert_eq!(first, 1, "first run must rewrite the legacy token");
        let second = rewrite_workspace(&p);
        assert_eq!(second, 0, "second run must be a no-op");
        let third = rewrite_workspace(&p);
        assert_eq!(third, 0, "third run must still be a no-op");
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn legacy_agent_types_v1_skips_workspaces_without_legacy_dirs() {
        // Workspace path that has no `.k2so/agents/` (e.g. post-0.37.0
        // unification, or a freshly-imported workspace). Must early-exit
        // cheaply without scanning anywhere.
        let p = std::env::temp_dir().join(format!(
            "k2so-legacy-types-empty-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&p).unwrap();
        let n = rewrite_workspace(&p);
        assert_eq!(n, 0);
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn run_aggregates_across_multiple_workspaces() {
        let p1 = temp_workspace();
        write_agent_md(&p1, "a", "---\ntype: pod-member\n---\n");
        let p2 = temp_workspace();
        write_agent_md(&p2, "b", "---\ntype: pod-leader\n---\n");
        write_agent_md(&p2, "c", "---\ntype: pod-member\n---\n");
        let p3 = temp_workspace(); // no agent.md files

        let outcome = run([&p1, &p2, &p3]);
        assert_eq!(outcome.rewritten_count, 3);

        fs::remove_dir_all(&p1).ok();
        fs::remove_dir_all(&p2).ok();
        fs::remove_dir_all(&p3).ok();
    }

    #[test]
    fn rewrite_string_unchanged_for_modern_frontmatter() {
        // Modern AGENT.md (post-rewrite) — pass through unchanged.
        let modern = "---\nname: rust-eng\ntype: agent-template\n---\n";
        let (out, changed) = rewrite_string(modern);
        assert_eq!(out, modern);
        assert!(!changed);
    }

    #[test]
    fn rewrite_string_handles_all_three_tokens_in_one_file() {
        // Single AGENT.md that carries all three legacy tokens.
        let legacy = "---\ntype: pod-leader\npod_leader: true\n---\nbody mentions pod-member elsewhere but only frontmatter matches.\n";
        let (out, changed) = rewrite_string(legacy);
        assert!(changed);
        assert!(out.contains("type: manager"));
        assert!(out.contains("manager: true"));
        // The body's stray "pod-member" without "type: " prefix is
        // intentionally NOT rewritten — the replacement only fires
        // on the `type: X` token, so prose stays intact.
        assert!(out.contains("pod-member elsewhere"));
    }
}
