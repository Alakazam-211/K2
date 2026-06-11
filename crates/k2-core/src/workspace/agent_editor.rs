//! AIFileEditor surface for editing AGENT.md.
//!
//! Phase 2.5d: extracted from the monolithic `agents/commands.rs`. The
//! React `AgentPersonaEditor` consumes these four functions to render
//! the "Manage Persona" UI — fetch context, preview a regenerate,
//! commit a regenerate, save edits.
//!
//! Sibling [`crate::workspace::agent`] hosts the underlying CRUD;
//! [`crate::heartbeats::control`] hosts the per-agent heartbeat surface.

use std::fs;

use crate::workspace::agent_identity::{agent_dir, parse_frontmatter};
use crate::workspace::work_item::atomic_write;
use crate::workspace::agent::cleanup_agent_backups;


/// Get full context needed for the AIFileEditor agent editing session.
///
/// Returns a JSON bundle the React `AgentPersonaEditor` consumes
/// (agent name, role, type, manager-flag, current AGENT.md contents,
/// and the on-disk path so the editor can render an "open in finder"
/// link). The `agent_type` is normalized post-0.37.0 — old `pod-leader`
/// / `coordinator` types collapse to `manager`, `pod-member` collapses
/// to `agent-template`.
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_get_editor_context`.
pub fn k2so_agents_get_editor_context(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let agent_md = fs::read_to_string(dir.join("AGENT.md")).unwrap_or_default();
    let fm = parse_frontmatter(&agent_md);
    let is_manager = fm.get("pod_leader").map_or(false, |v| v == "true")
        || fm.get("coordinator").map_or(false, |v| v == "true")
        || fm.get("manager").map_or(false, |v| v == "true");
    let role = fm.get("role").cloned().unwrap_or_default();
    let agent_type = fm
        .get("type")
        .cloned()
        .map(|t| match t.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            "pod-member" => "agent-template".to_string(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "agent-template".to_string());

    Ok(serde_json::json!({
        "agentName": agent_name,
        "role": role,
        "agentType": agent_type,
        "isManager": is_manager,
        "agentMd": agent_md,
        "agentMdPath": dir.join("AGENT.md").to_string_lossy(),
        "agentDir": dir.to_string_lossy(),
    }))
}

/// Preview the agent's context bundle without writing to disk.
/// Returns `{ generated, onDisk, contextPath }`: the freshly-composed
/// system-prompt body, the current on-disk CLAUDE.md content (if any —
/// may contain user edits), and the CLAUDE.md path for caller-side
/// diff UIs. The JSON field is still `claudeMdPath` for back-compat
/// with the React AgentPersonaEditor; new UIs should read
/// `contextPath` once populated.
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_preview_agent_context`.
pub fn k2so_agents_preview_agent_context(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    let generated = crate::skills::content::generate_agent_claude_md_content(
        &project_path,
        &agent_name,
        None,
    )?;

    let dir = agent_dir(&project_path, &agent_name);
    let on_disk_path = dir.join("CLAUDE.md");
    let on_disk = if on_disk_path.exists() {
        Some(crate::workspace::work_item::safe_read_to_string(&on_disk_path).unwrap_or_default())
    } else {
        None
    };

    Ok(serde_json::json!({
        "generated": generated,
        "onDisk": on_disk,
        "contextPath": on_disk_path.to_string_lossy(),
        // Legacy field — React still reads `claudeMdPath` at some
        // call sites. Emit both during the rename window; drop the
        // legacy field once every UI call site has migrated.
        "claudeMdPath": on_disk_path.to_string_lossy(),
    }))
}

/// Regenerate an agent's context bundle: composes the full
/// `--append-system-prompt` body, writes it to the agent's CLAUDE.md,
/// and returns it. Equivalent to `preview_agent_context` followed by
/// an atomic write.
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_regenerate_agent_context`.
pub fn k2so_agents_regenerate_agent_context(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    let md = crate::skills::content::generate_agent_claude_md_content(
        &project_path,
        &agent_name,
        None,
    )?;
    let claude_md_path = agent_dir(&project_path, &agent_name).join("CLAUDE.md");
    atomic_write(&claude_md_path, &md)?;
    Ok(md)
}

/// Save an agent's AGENT.md file, creating a timestamped backup of
/// the previous version. Keeps the 20 most recent backups in
/// `<agent>/agent-backups/`.
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_save_agent_md`.
pub fn k2so_agents_save_agent_md(
    project_path: String,
    agent_name: String,
    content: String,
) -> Result<(), String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let agent_md_path = dir.join("AGENT.md");

    // Back up existing agent.md before overwriting
    if agent_md_path.exists() {
        let backup_dir = dir.join("agent-backups");
        fs::create_dir_all(&backup_dir).ok();

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup_name = format!("agent-{}.md", timestamp);
        let existing = fs::read_to_string(&agent_md_path).unwrap_or_default();
        fs::write(backup_dir.join(&backup_name), &existing).ok();

        // Keep only the 20 most recent backups
        cleanup_agent_backups(&backup_dir, 20);
    }

    atomic_write(&agent_md_path, &content)
}

#[cfg(test)]
mod tests {
    //! Phase 2 Tier 2.1 coverage for the AIFileEditor surface
    //! (k2so_agents_get_editor_context + k2so_agents_save_agent_md).
    //!
    //! These tests scaffold a scratch workspace with a unified-primary
    //! agent on disk (`.k2so/agent/AGENT.md`) and exercise the editor
    //! context fetcher + the save flow's timestamped backup behavior.
    //!
    //! The `preview_agent_context` / `regenerate_agent_context` paths
    //! require a fully composed wake context (touches `SKILL.md`
    //! generation + workspace state DB lookups + the harness fanout)
    //! which is exercised by integration tests; the unit tests here
    //! focus on the editor-specific behaviors: identity readout,
    //! frontmatter normalization, backup creation + retention.
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    /// Build a scratch workspace at $TMPDIR/<unique>/ with a unified-
    /// primary agent on disk. Returns (workspace_path, agent_name).
    fn scratch_workspace_with_primary(agent_name: &str, persona_type: &str, role: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-editor-test-{}-{}-{}",
            agent_name,
            std::process::id(),
            Uuid::new_v4(),
        ));
        std::fs::create_dir_all(dir.join(".k2so/agent")).expect("scaffold agent dir");
        let body = format!(
            "---\nname: {name}\ntype: {ptype}\nrole: {role}\n---\n\n# {name}\n\nInitial body.\n",
            name = agent_name,
            ptype = persona_type,
            role = role,
        );
        std::fs::write(dir.join(".k2so/agent/AGENT.md"), body)
            .expect("write seed AGENT.md");
        dir
    }

    #[test]
    fn get_editor_context_returns_identity_and_paths_for_existing_agent() {
        let dir = scratch_workspace_with_primary("cortana", "custom", "test pilot");
        let path = dir.to_string_lossy().into_owned();

        let ctx = k2so_agents_get_editor_context(path, "cortana".to_string())
            .expect("editor context");
        assert_eq!(ctx["agentName"], "cortana");
        assert_eq!(ctx["role"], "test pilot", "role pulled from frontmatter");
        // type=custom: not a manager, normalized agentType
        assert_eq!(ctx["agentType"], "custom");
        assert_eq!(ctx["isManager"], false);
        // The on-disk AGENT.md body should be returned verbatim.
        let agent_md = ctx["agentMd"].as_str().expect("agentMd string");
        assert!(agent_md.contains("Initial body."));
        // Both path fields should be set.
        assert!(ctx["agentMdPath"].as_str().unwrap().ends_with("AGENT.md"));
        assert!(ctx["agentDir"].as_str().unwrap().contains(".k2so/agent"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_editor_context_normalizes_legacy_pod_leader_type_string() {
        // Pre-0.37.0 unification: a `type: pod-leader` frontmatter field
        // collapses to "manager" in the agentType readout. Note: the
        // is_manager flag is computed from a SEPARATE pair of legacy
        // frontmatter keys (`pod_leader: true` / `coordinator: true` /
        // `manager: true`), NOT from the `type:` value — so the agent
        // here keeps isManager=false unless one of those flags is set.
        let dir = scratch_workspace_with_primary("captain", "pod-leader", "lead");
        let path = dir.to_string_lossy().into_owned();

        let ctx = k2so_agents_get_editor_context(path, "captain".to_string())
            .expect("ctx");
        assert_eq!(
            ctx["agentType"], "manager",
            "pod-leader should normalize to manager in the agentType readout"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_editor_context_sets_is_manager_when_manager_frontmatter_flag_present() {
        // The is_manager bit flips when any of the legacy "manager
        // flag" frontmatter keys is "true" — pod_leader, coordinator,
        // or manager. Build an AGENT.md that includes one of them and
        // verify isManager=true.
        let dir = std::env::temp_dir().join(format!(
            "k2so-editor-manager-flag-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        std::fs::create_dir_all(dir.join(".k2so/agent")).unwrap();
        let body = "---\nname: alpha\ntype: custom\nmanager: true\n---\n\n# alpha\n";
        std::fs::write(dir.join(".k2so/agent/AGENT.md"), body).unwrap();
        let path = dir.to_string_lossy().into_owned();

        let ctx = k2so_agents_get_editor_context(path, "alpha".to_string())
            .expect("ctx");
        assert_eq!(
            ctx["isManager"], true,
            "manager:true frontmatter key should flip isManager"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_editor_context_rejects_missing_agent() {
        // Fresh workspace, no agent on disk.
        let dir = std::env::temp_dir().join(format!(
            "k2so-editor-missing-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();

        let err = k2so_agents_get_editor_context(path, "ghost".to_string())
            .expect_err("missing agent must error");
        assert!(
            err.contains("does not exist"),
            "diagnostic should explain the agent isn't on disk, got {err:?}",
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_agent_md_creates_timestamped_backup_of_previous_version() {
        let dir = scratch_workspace_with_primary("scout", "custom", "scout role");
        let path = dir.to_string_lossy().into_owned();

        // Read the current body so we can assert the backup matches it.
        let agent_dir_path = dir.join(".k2so/agent");
        let pre_save = std::fs::read_to_string(agent_dir_path.join("AGENT.md")).unwrap();

        k2so_agents_save_agent_md(
            path.clone(),
            "scout".to_string(),
            "---\nname: scout\ntype: custom\nrole: scout role\n---\n\n# scout\n\nNew body.\n"
                .to_string(),
        )
        .expect("save");

        // 1. The new content landed.
        let new_body = std::fs::read_to_string(agent_dir_path.join("AGENT.md")).unwrap();
        assert!(new_body.contains("New body."));

        // 2. agent-backups/ exists with exactly one backup matching pre_save.
        let backup_dir = agent_dir_path.join("agent-backups");
        assert!(backup_dir.is_dir(), "agent-backups directory should be created");
        let backups: Vec<_> = std::fs::read_dir(&backup_dir)
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(
            backups.len(),
            1,
            "first save should create exactly one backup, got {} entries",
            backups.len(),
        );
        let backup_body = std::fs::read_to_string(backups[0].path()).unwrap();
        assert_eq!(
            backup_body, pre_save,
            "backup must preserve the previous AGENT.md byte-for-byte"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_agent_md_rejects_missing_agent() {
        let dir = std::env::temp_dir().join(format!(
            "k2so-editor-save-missing-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();

        let err = k2so_agents_save_agent_md(
            path,
            "ghost".to_string(),
            "anything".to_string(),
        )
        .expect_err("missing agent must error");
        assert!(
            err.contains("does not exist"),
            "diagnostic should explain the agent isn't on disk, got {err:?}",
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
