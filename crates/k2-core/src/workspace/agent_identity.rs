//! Workspace agent identity resolution + path helpers.
//!
//! Phase 2.5c extraction. Pre-2.5c these helpers lived at the
//! `agents/mod.rs` root because every `agents/*` submodule needed
//! them. Post-Phase-2.1 the workspace owns "which agent answers for
//! this workspace?" semantically, so the helpers belong under
//! [`crate::workspace`] — the folder that holds workspace lifecycle +
//! state + launch helpers.
//!
//! `agents/mod.rs` re-exports each public function via `pub use` so
//! every existing `crate::agents::find_primary_agent` /
//! `crate::agents::agent_dir` etc. call site keeps resolving without
//! churn during the deprecation window.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Resolve a project's primary-key id from its filesystem path. `None`
/// when the project hasn't been registered via `projects` yet.
pub fn resolve_project_id(conn: &rusqlite::Connection, path: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| r.get(0),
    )
    .ok()
}

/// Root of the legacy multi-agent tree for a given workspace:
/// `<project>/.k2so/agents/`. Post-0.37.0 unification this directory
/// is removed by the migration sweep and only sticks around for
/// fresh workspaces that haven't been onboarded yet (it gets created
/// briefly by older code paths that still scaffold it). Prefer
/// [`workspace_agent_path`] / [`agent_template_dir`] for new code.
pub fn agents_dir(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("agents")
}

/// Post-0.37.0: `<project>/.k2so/agent/` — the single workspace
/// agent's directory. Created by the unification migration; every
/// call site that historically did `agent_dir(project, primary_name)`
/// converges on this path after the migration runs.
pub fn workspace_agent_path(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("agent")
}

/// `<project>/.k2so/agent/AGENT.md` — the workspace agent's persona
/// file post-0.37.0. Convenience over `workspace_agent_path().join("AGENT.md")`.
#[allow(dead_code)]
pub fn workspace_agent_md_path(project_path: &str) -> PathBuf {
    workspace_agent_path(project_path).join("AGENT.md")
}

/// Post-0.37.0: `<project>/.k2so/agent-templates/<template_name>/` —
/// role personas for delegation/worktrees.
pub fn agent_template_dir(project_path: &str, template_name: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path)
        .join("agent-templates")
        .join(template_name)
}

/// Post-0.37.0: `<project>/.k2so/heartbeats/` — workspace-level
/// heartbeat directory. Each schedule lives at
/// `<this>/<schedule_name>/WAKEUP.md`. Pre-0.37.0 heartbeats lived
/// under each agent's directory (`agent_dir/heartbeats/<sched>/`).
/// The unification migration moves them to this workspace-level
/// path; new heartbeats scaffold here directly.
pub fn workspace_heartbeats_dir(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("heartbeats")
}

/// Post-Phase-2.5b: `<project>/.k2so/skills/` — the unified home for
/// every skill profile the workspace-agent owns. A workspace IS an
/// agent (hands, memory, self-built tools, proactivity via
/// heartbeats); skills are the documentation profiles that describe
/// how this workspace-agent operates. Replaces the pre-2.5b split
/// across `.k2so/agents/<name>/` and `.k2so/agent-templates/<role>/`
/// which carried over from the earlier sub-agent model. Migration
/// runs at daemon boot per upgraded workspace; see
/// [`crate::skills::consolidation::consolidate_skills_v1`].
pub fn skills_dir(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("skills")
}

/// Post-Phase-2.5b: `<project>/.k2so/skills/<skill_name>/` — a single
/// skill's directory. Each skill's body lives at `<this>/SKILL.md`.
pub fn skill_dir(project_path: &str, skill_name: &str) -> PathBuf {
    skills_dir(project_path).join(skill_name)
}

/// Resolve the on-disk directory for an agent name within a workspace.
///
/// In the workspace==agent model `agent_name` is mostly a routing
/// key for legacy call sites — the primary workspace-agent is keyed
/// on the workspace itself, so callers that pass a name still
/// resolve onto the unified path transparently.
///
/// **Layout-aware.** Probes in this order:
/// 1. `<project>/.k2so/agent/AGENT.md` exists — post-0.37.0 primary
///    workspace-agent. `agent_name` is ignored here; the primary is
///    keyed on workspace, not name. Historic call sites
///    (e.g. `agent_dir(project, "pod-leader")`) keep working without
///    changes during the deprecation window.
///
///    Probing for AGENT.md (not just the dir) matters because the
///    unification migration `mkdir -p`s `.k2so/agent/` BEFORE it
///    moves any files. During that window the dir exists but has
///    no AGENT.md yet, and the migration's own primary-detection
///    walk needs `agent_type_for` to read from the LEGACY
///    `.k2so/agents/<name>/AGENT.md` to determine the primary.
///    Gating the probe on the populated state avoids a
///    chicken-and-egg failure during migration.
/// 2. `<project>/.k2so/skills/<agent_name>/` exists with either
///    SKILL.md or AGENT.md — post-Phase-2.5b unified home for skill
///    profiles. Probed AFTER the workspace primary so a skill named
///    the same as the workspace-agent doesn't accidentally shadow
///    the primary persona.
/// 3. `<project>/.k2so/agent-templates/<agent_name>/AGENT.md`
///    exists — legacy template path. Pre-Phase-2.5b workspaces that
///    haven't been first-boot-migrated yet still answer here; on
///    upgraded workspaces this dir was trashed by
///    `consolidate_skills_v1` and the probe returns false.
/// 4. Legacy `<project>/.k2so/agents/<agent_name>/` — pre-0.37.0
///    workspaces that haven't been migrated yet (rare; the daemon's
///    boot sweep migrates every registered workspace).
pub fn agent_dir(project_path: &str, agent_name: &str) -> PathBuf {
    let primary = workspace_agent_path(project_path);
    if primary.join("AGENT.md").exists() {
        return primary;
    }
    // Phase 2.5b: probe the consolidated `.k2so/skills/<name>/` home
    // before the legacy `.k2so/agent-templates/` / `.k2so/agents/`
    // paths. The probe checks for either SKILL.md (new shape) or
    // AGENT.md (transitional shape for workspaces caught mid-migration).
    let skill = skill_dir(project_path, agent_name);
    if skill.join("SKILL.md").exists() || skill.join("AGENT.md").exists() {
        return skill;
    }
    let template = agent_template_dir(project_path, agent_name);
    if template.join("AGENT.md").exists() {
        return template;
    }
    agents_dir(project_path).join(agent_name)
}

/// Extract YAML-ish `key: value` frontmatter from a markdown blob.
/// Tolerant: empty keys/values skipped, missing closing fence returns
/// an empty map. Used by [`agent_type_for`] and
/// [`crate::workspace::scheduler`] consumers to read agent.md
/// metadata without pulling a full YAML dep.
pub fn parse_frontmatter(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if !content.starts_with("---") {
        return map;
    }
    if let Some(end) = content[3..].find("---") {
        let frontmatter = &content[3..3 + end];
        for line in frontmatter.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                if !key.is_empty() && !value.is_empty() {
                    map.insert(key, value);
                }
            }
        }
    }
    map
}

/// Determine an agent's type from its `agent.md` frontmatter. Returns
/// `"agent-template"` if no frontmatter or no `type:` field is found
/// (same default the scheduler uses elsewhere).
pub fn agent_type_for(project_path: &str, agent_name: &str) -> String {
    let md = agent_dir(project_path, agent_name).join("AGENT.md");
    if let Ok(content) = fs::read_to_string(&md) {
        let fm = parse_frontmatter(&content);
        if let Some(t) = fm.get("type") {
            return t.clone();
        }
    }
    "agent-template".to_string()
}

/// Find the workspace's primary scheduleable agent.
///
/// A workspace is one-of Custom / K2SO Agent / Workspace Manager
/// (mutually exclusive by design), but agent-mode swaps can leave
/// orphan directories from prior modes on disk. This fn uses
/// `projects.agent_mode` as the source of truth and only returns an
/// agent dir whose type matches the workspace's declared mode.
/// Agent-templates are never scheduleable.
///
/// **0.37.0 unification.** Post-migration the workspace's single
/// agent lives at `.k2so/agent/AGENT.md` and the legacy
/// `.k2so/agents/<name>/` tree is gone. We probe the unified path
/// first — if it has an AGENT.md, parse the frontmatter `name:`
/// and return it. Pre-0.37.0 workspaces (or freshly-created ones
/// that haven't been migrated yet) fall through to the legacy
/// scan. Without this probe, every heartbeat fire on a migrated
/// workspace silently failed at "no scheduleable agent in this
/// workspace" because `agents_root.exists()` returned false.
pub fn find_primary_agent(project_path: &str) -> Option<String> {
    // Post-0.37.0 unified primary.
    let unified_md = workspace_agent_path(project_path).join("AGENT.md");
    if let Ok(content) = fs::read_to_string(&unified_md) {
        let fm = parse_frontmatter(&content);
        if let Some(name) = fm.get("name") {
            if !name.is_empty() {
                return Some(name.clone());
            }
        }
    }

    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return None;
    }

    // Resolve the declared workspace mode from the DB. Prevents
    // alphabetical scan order from picking a stale orphan (e.g.
    // returning pod-leader before sarah when the workspace is actually
    // a Custom agent workspace for sarah).
    let declared_mode: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT agent_mode FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    };

    let type_for_mode = |mode: &str| match mode {
        "custom" => "custom",
        "manager" => "manager",
        "k2so" | "agent" => "k2so",
        _ => "",
    };

    // Pass 1: prefer the agent whose type matches the declared mode.
    if let Some(ref mode) = declared_mode {
        let wanted = type_for_mode(mode);
        if !wanted.is_empty() {
            if let Ok(entries) = fs::read_dir(&agents_root) {
                for entry in entries.flatten() {
                    if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    if agent_type_for(project_path, &name) == wanted {
                        return Some(name);
                    }
                }
            }
            // 0.39.0f: dropped the pre-0.37.0 `__lead__` sentinel
            // fallback. Post-unification the workspace's primary
            // agent ALWAYS materializes as `.k2so/agent/AGENT.md`
            // (handled by the unified probe above). If we reach
            // here with `wanted == "manager"` it means the legacy
            // `.k2so/agents/<n>/` scan didn't find a manager-type
            // dir AND the unified file is missing — i.e., the
            // workspace is in manager mode but its primary agent
            // hasn't been scaffolded yet. Return None so callers
            // can handle the missing-primary case (e.g., audit a
            // `skipped_no_primary` decision) instead of routing to
            // a sentinel name no consumer would recognize.
        }
    }

    // Pass 2 (fallback, no declared mode): first scheduleable dir wins.
    let Ok(entries) = fs::read_dir(&agents_root) else {
        return None;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let agent_type = agent_type_for(project_path, &name);
        if matches!(agent_type.as_str(), "custom" | "manager" | "k2so") {
            return Some(name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_reads_simple_kv() {
        let md = "---\nname: test\ntype: custom\n---\n# body\n";
        let fm = parse_frontmatter(md);
        assert_eq!(fm.get("name"), Some(&"test".to_string()));
        assert_eq!(fm.get("type"), Some(&"custom".to_string()));
    }

    #[test]
    fn parse_frontmatter_empty_when_no_fence() {
        let fm = parse_frontmatter("# heading only\n");
        assert!(fm.is_empty());
    }

    #[test]
    fn parse_frontmatter_skips_empty_keys_and_values() {
        // `: lonely` has empty key; `key:` has empty value. Both skipped.
        let fm = parse_frontmatter("---\n: lonely\nkey:\nrole: eng\n---\n");
        assert_eq!(fm.len(), 1);
        assert_eq!(fm.get("role"), Some(&"eng".to_string()));
    }

    #[test]
    fn agents_dir_and_agent_dir_are_consistent() {
        let root = agents_dir("/tmp/proj");
        assert_eq!(root, PathBuf::from("/tmp/proj/.k2so/agents"));
        let agent = agent_dir("/tmp/proj", "foo");
        assert_eq!(agent, root.join("foo"));
    }
}
