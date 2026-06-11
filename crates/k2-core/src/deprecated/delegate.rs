//! Agent delegation — DEPRECATED in Phase 2.1 (0.39.0g).
//!
//! Pre-Phase-2.1, K2SO owned the spawn lifecycle for delegated sub-agents:
//! the daemon (or the UI Delegate button) called
//! [`k2so_agents_delegate`] which created a worktree, registered it,
//! moved the work item into the agent's `active/`, generated CLAUDE.md,
//! and returned a launch JSON the caller used to spawn `claude` in the
//! new worktree.
//!
//! Phase 2.1 (PRD A23) reframed delegation as a harness responsibility:
//! K2SO no longer spawns sub-agent processes; the parent harness owns
//! the spawn and K2SO just tracks the workspace state. The four public
//! frontmatter helpers ([`add_worktree_to_frontmatter`],
//! [`strip_worktree_from_frontmatter`], [`update_assigned_by`],
//! [`shorten_slug`]) are still used by the review queue (`reviews.rs`)
//! and the legacy `agent_launch` path; they're preserved with
//! `#[deprecated]` annotations so callers see compiler warnings at
//! every use site. The top-level [`k2so_agents_delegate`] is kept
//! for the daemon's `agents_routes.rs` delegate route + the
//! `agent_launch` inbox-delegate branch — both will be retired in
//! Phase 2.5d.
//!
//! Relocated to `deprecated/` in Phase 2.5c (0.39.0h) so the module
//! tree advertises this surface as retired.

use std::fs;
use std::path::PathBuf;

use crate::skills::content::generate_agent_claude_md_content;
use crate::workspace::agent_identity::{agent_dir, resolve_project_id};
use crate::workspace::scheduler::{agent_work_dir, get_workspace_state};
use crate::workspace::work_item::{atomic_write, read_work_item};
use crate::log_debug;

/// Shorten a slug to a maximum length, breaking at word boundaries.
/// Strips common filler prefixes (`bug-`, `feature-`, `task-`).
#[deprecated(
    since = "0.39.0h",
    note = "Harness owns spawn lifecycle post-Phase-2.1; see Phase 2.1 PRD A23"
)]
pub fn shorten_slug(slug: &str, max_len: usize) -> String {
    let stripped = slug
        .strip_prefix("bug-")
        .or_else(|| slug.strip_prefix("feature-"))
        .or_else(|| slug.strip_prefix("task-"))
        .unwrap_or(slug);

    if stripped.len() <= max_len {
        return stripped.to_string();
    }

    let truncated = &stripped[..max_len];
    match truncated.rfind('-') {
        Some(pos) if pos > max_len / 2 => truncated[..pos].to_string(),
        _ => truncated.to_string(),
    }
}

/// Overwrite the `assigned_by:` frontmatter field (creating the field
/// is not this fn's job — caller ensures it exists).
#[deprecated(
    since = "0.39.0h",
    note = "Harness owns spawn lifecycle post-Phase-2.1; see Phase 2.1 PRD A23"
)]
pub fn update_assigned_by(content: &str, new_value: &str) -> String {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            let frontmatter = &content[3..3 + end];
            let rest = &content[3 + end..];
            let updated_fm: String = frontmatter
                .lines()
                .map(|line| {
                    if line.starts_with("assigned_by:") {
                        format!("assigned_by: {}", new_value)
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            return format!("---{}{}", updated_fm, rest);
        }
    }
    content.to_string()
}

/// Stamp `worktree_path:` and `branch:` onto a work item's frontmatter.
/// Called when moving an item from `inbox/` to `active/` during
/// delegate.
#[deprecated(
    since = "0.39.0h",
    note = "Harness owns spawn lifecycle post-Phase-2.1; see Phase 2.1 PRD A23"
)]
pub fn add_worktree_to_frontmatter(
    content: &str,
    worktree_path: &str,
    branch: &str,
) -> String {
    if content.starts_with("---") {
        if let Some(end_idx) = content[3..].find("---") {
            let frontmatter = &content[3..3 + end_idx];
            let body = &content[3 + end_idx + 3..];
            return format!(
                "---\n{}worktree_path: {}\nbranch: {}\n---{}",
                frontmatter, worktree_path, branch, body
            );
        }
    }
    content.to_string()
}

/// Strip `worktree_path:` and `branch:` from frontmatter. Called on
/// rejection/retry so the re-queued work item doesn't reference a
/// worktree that was cleaned up.
#[deprecated(
    since = "0.39.0h",
    note = "Harness owns spawn lifecycle post-Phase-2.1; see Phase 2.1 PRD A23"
)]
pub fn strip_worktree_from_frontmatter(content: &str) -> String {
    if content.starts_with("---") {
        if let Some(end_idx) = content[3..].find("---") {
            let frontmatter = &content[3..3 + end_idx];
            let body = &content[3 + end_idx + 3..];
            let cleaned: String = frontmatter
                .lines()
                .filter(|line| {
                    !line.starts_with("worktree_path:") && !line.starts_with("branch:")
                })
                .collect::<Vec<_>>()
                .join("\n");
            return format!("---\n{}\n---{}", cleaned.trim(), body);
        }
    }
    content.to_string()
}

/// Delegate a work item from the workspace inbox to a specific agent.
///
/// Creates the worktree, registers it in the DB, moves the work file
/// into the agent's `active/` folder, writes `CLAUDE.md` into the
/// worktree root, and returns the launch JSON (same shape as the
/// manual "Launch" path: `command`, `args`, `cwd`, `claudeMdPath`,
/// `agentName`, `worktreePath`, `branch`, `taskFile`).
///
/// Intended callers:
/// - Manager's UI "Delegate" button (via `#[tauri::command]` wrapper
///   still in `src-tauri/src/commands/k2so_agents.rs`).
/// - `k2so_agents_build_launch` when an agent's inbox has work but
///   no active worktree yet.
#[deprecated(
    since = "0.39.0h",
    note = "Harness owns spawn lifecycle post-Phase-2.1; see Phase 2.1 PRD A23"
)]
pub fn k2so_agents_delegate(
    project_path: String,
    target_agent: String,
    source_file: String,
) -> Result<serde_json::Value, String> {
    let source = PathBuf::from(&source_file);
    if !source.exists() {
        return Err(format!("Source file does not exist: {}", source_file));
    }

    let agent_d = agent_dir(&project_path, &target_agent);
    if !agent_d.exists() {
        return Err(format!("Target agent '{}' does not exist", target_agent));
    }

    let content = fs::read_to_string(&source).map_err(|e| e.to_string())?;
    let item = read_work_item(&source, "inbox")
        .ok_or_else(|| "Could not parse work item".to_string())?;

    // 1. Create a worktree for this task
    let full_slug = item.filename.trim_end_matches(".md");
    let task_slug = shorten_slug(full_slug, 40);
    let branch_name = format!("agent/{}/{}", target_agent, task_slug);
    let worktree = crate::git::create_worktree(&project_path, &branch_name)
        .map_err(|e| format!("Failed to create worktree: {}", e))?;

    // Register the worktree as a workspace in the DB so it appears
    // in the sidebar. Matches the git_create_worktree schema used by
    // the Tauri sidebar: (id, project_id, name, type, branch,
    // tab_order, worktree_path).
    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Ok(project_id) = conn.query_row(
            "SELECT id FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, String>(0),
        ) {
            let ws_id = uuid::Uuid::new_v4().to_string();
            let max_order: i32 = conn
                .query_row(
                    "SELECT COALESCE(MAX(tab_order), -1) + 1 FROM workspaces WHERE project_id = ?1",
                    rusqlite::params![project_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if let Err(e) = conn.execute(
                "INSERT INTO workspaces (id, project_id, name, type, branch, tab_order, worktree_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    ws_id,
                    project_id,
                    worktree.branch,
                    "worktree",
                    worktree.branch,
                    max_order,
                    worktree.path
                ],
            ) {
                log_debug!("[delegate] Failed to register worktree in DB: {}", e);
            }
        }
    }

    // 2. Move work item to agent's active/ folder with worktree info
    let active_dir = agent_work_dir(&project_path, &target_agent, "active");
    fs::create_dir_all(&active_dir).ok();
    let updated = update_assigned_by(&content, "delegated");
    let updated = add_worktree_to_frontmatter(&updated, &worktree.path, &worktree.branch);
    let active_file = active_dir.join(&item.filename);
    atomic_write(&active_file, &updated)?;
    // **0.37.6:** route to recycle bin. The work item content is
    // already preserved in `active_file` via the atomic_write
    // above, but the SOURCE was a user-authored task description —
    // sending to Trash gives the user a recovery path if they want
    // to undelegate.
    crate::safe_delete::trash(&source)
        .map_err(|e| format!("Failed to trash source: {}", e))?;

    // 3. Generate a task-specific CLAUDE.md and write it to the
    //    worktree root.
    let claude_md =
        generate_agent_claude_md_content(&project_path, &target_agent, Some(&item))?;
    let claude_md_path = PathBuf::from(&worktree.path).join("CLAUDE.md");
    atomic_write(&claude_md_path, &claude_md)?;

    // 4. Build the launch command for the frontend.
    let source_type = &item.source;
    let capability = if let Some(ws_state) = get_workspace_state(&project_path) {
        ws_state.capability_for_source(source_type).to_string()
    } else {
        "gated".to_string()
    };

    let completion_protocol = if capability == "auto" {
        format!(
            "When done:\n\
            1. Commit all your changes to branch `{branch}`\n\
            2. Run: `k2so agent complete --agent {agent} --file {filename}`\n\
            This will automatically merge your branch into main and clean up the worktree.\n\
            3. Notify the workspace manager that you're done:\n\
            Run `k2so agents running` to find the manager's terminal ID (look for `.k2so/agents/manager` in the CWD),\n\
            then run: `k2so terminal write <manager-terminal-id> \"Completed: {title}. Branch {branch} merged.\"`",
            agent = target_agent, branch = worktree.branch, filename = item.filename, title = item.title,
        )
    } else {
        format!(
            "When done:\n\
            1. Commit all your changes to branch `{branch}`\n\
            2. Run: `k2so agent complete --agent {agent} --file {filename}`\n\
            This will move your work to done and flag it for human review.\n\
            3. Notify the workspace manager that your work is ready for review:\n\
            Run `k2so agents running` to find the manager's terminal ID (look for `.k2so/agents/manager` in the CWD),\n\
            then run: `k2so terminal write <manager-terminal-id> \"Ready for review: {title}. Branch: {branch}\"`",
            agent = target_agent, branch = worktree.branch, filename = item.filename, title = item.title,
        )
    };

    let task_instructions = format!(
        "\n\n## Your Current Assignment\n\n\
        You are working in a dedicated worktree at `{wt_path}` on branch `{branch}`.\n\n\
        **{title}** (priority: {priority})\n\n\
        Read the full task file at `.k2so/agents/{agent}/work/active/{filename}` for details and acceptance criteria.\n\n\
        ## Completion Protocol\n\n\
        {completion_protocol}",
        agent = target_agent,
        wt_path = worktree.path,
        branch = worktree.branch,
        title = item.title,
        priority = item.priority,
        filename = item.filename,
        completion_protocol = completion_protocol,
    );

    let full_system_prompt = format!("{}\n{}", claude_md, task_instructions);

    let kickoff = format!(
        "Read your task file at `{}` and begin implementing the fix. \
        Commit your work as you go.",
        agent_work_dir(&project_path, &target_agent, "active")
            .join(&item.filename)
            .to_string_lossy()
    );

    Ok(serde_json::json!({
        "command": "claude",
        "args": ["--dangerously-skip-permissions", "--append-system-prompt", full_system_prompt, kickoff],
        "cwd": worktree.path,
        "claudeMdPath": claude_md_path.to_string_lossy(),
        "agentName": target_agent,
        "worktreePath": worktree.path,
        "branch": worktree.branch,
        "taskFile": item.filename,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_slug_strips_common_prefixes() {
        assert_eq!(shorten_slug("bug-fix-auth", 40), "fix-auth");
        assert_eq!(shorten_slug("feature-new-ui", 40), "new-ui");
        assert_eq!(shorten_slug("task-do-x", 40), "do-x");
    }

    #[test]
    fn shorten_slug_truncates_on_word_boundary() {
        let slug = "the-quick-brown-fox-jumps-over-the-lazy-dog";
        let short = shorten_slug(slug, 20);
        // Should not chop a word in half.
        assert!(!short.contains("fo") || short.ends_with("fox"));
        assert!(short.len() <= 20);
    }

    #[test]
    fn update_assigned_by_rewrites_field() {
        let content = "---\ntitle: t\nassigned_by: user\n---\nbody";
        let updated = update_assigned_by(content, "delegated");
        assert!(updated.contains("assigned_by: delegated"));
        assert!(!updated.contains("assigned_by: user"));
    }

    #[test]
    fn add_worktree_to_frontmatter_appends_fields() {
        let content = "---\ntitle: t\n---\nbody";
        let stamped = add_worktree_to_frontmatter(content, "/tmp/wt", "agent/foo/x");
        assert!(stamped.contains("worktree_path: /tmp/wt"));
        assert!(stamped.contains("branch: agent/foo/x"));
    }

    #[test]
    fn strip_worktree_removes_fields() {
        let content = "---\ntitle: t\nworktree_path: /tmp/wt\nbranch: x\n---\nbody";
        let stripped = strip_worktree_from_frontmatter(content);
        assert!(!stripped.contains("worktree_path"));
        assert!(!stripped.contains("branch:"));
        assert!(stripped.contains("title: t"));
    }
}
