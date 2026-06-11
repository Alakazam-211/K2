//! Full-fat wake-launch argument builder.
//!
//! [`k2so_agents_build_launch`] is the code path behind the UI's
//! "Launch agent" button and the scheduler-triggered heartbeat wake.
//! It returns the JSON the host (Tauri app or daemon) uses to spawn
//! `claude` with the right `cwd`, `args`, and `--resume` handling for
//! whichever branch of the decision tree applies:
//!
//! 1. **Resume an active worktree.** Agent has a work item in
//!    `active/` with a `worktree_path` frontmatter field → launch in
//!    that worktree, write the task CLAUDE.md, return args with a
//!    resume-context system prompt.
//! 2. **Delegate from inbox.** Agent has work waiting in `inbox/` but
//!    no active worktree → call
//!    [`crate::deprecated::delegate::k2so_agents_delegate`] (now
//!    deprecated alongside the rest of the agents-CRUD surface) on the
//!    highest-priority item, which creates a worktree + moves inbox →
//!    active in one step.
//! 3. **Fresh launch.** No active worktree, no inbox → launch in the
//!    project root with a compose_agent_wake_context system prompt,
//!    the agent's wakeup.md as the user message, and a
//!    checksum-derived resume session ID if one is on file.
//!
//! The daemon's lid-closed wake path uses a much simpler variant
//! (`k2_daemon::wake_headless::spawn_wake_headless`, moved out of
//! `k2_core::agents::wake` to the daemon in 0.37.0) that doesn't
//! branch on work queue state — lid-closed fires just want to hand
//! the agent its wakeup.md. This function is the supervised-launch
//! surface that needs the full decision tree.

use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::content::compose_agent_wake_context;
use crate::workspace::agent_identity::{agent_dir, parse_frontmatter, resolve_project_id};
use crate::workspace::scheduler::{agent_work_dir, priority_rank};
use crate::workspace::wake_prompts::{
    compose_wake_prompt_for_agent, compose_wake_prompt_from_path,
};
use crate::workspace::work_item::{read_work_item, WorkItem};
use crate::chat_history;
use crate::db::schema::WorkspaceSession;

/// See module docs. Returns the launch JSON for the chosen wake
/// branch. Errors only for filesystem / DB failures during the
/// chosen branch; empty inbox + missing active worktree + missing
/// wakeup.md are all handled gracefully.
// `heartbeat_name`: when Some, the launch is on behalf of a specific
// heartbeat fire. Resume target lookup prefers
// `agent_heartbeats.last_session_id` for that row over
// `agent_sessions.session_id`, so each heartbeat keeps its own
// dedicated chat thread that the user can audit independently from
// the Chat tab session. None = manual launch / Chat tab — use the
// per-agent global.
pub fn k2so_agents_build_launch(
    project_path: String,
    agent_name: String,
    agent_cli_command: Option<String>,
    wakeup_override: Option<String>,
    skip_fork_session: Option<bool>,
    heartbeat_name: Option<String>,
) -> Result<serde_json::Value, String> {
    let command = agent_cli_command.unwrap_or_else(|| "claude".to_string());
    let skip_fork = skip_fork_session.unwrap_or(false);

    // Case 1: resume active worktree
    let active_dir = agent_work_dir(&project_path, &agent_name, "active");
    if active_dir.exists() {
        if let Ok(entries) = fs::read_dir(&active_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    if let Some(item) = read_work_item(&path, "active") {
                        let content = fs::read_to_string(&path).unwrap_or_default();
                        let fm = parse_frontmatter(&content);
                        if let Some(wt_path) = fm.get("worktree_path") {
                            let branch = fm.get("branch").cloned().unwrap_or_default();
                            let claude_md = compose_agent_wake_context(
                                &project_path,
                                &agent_name,
                                Some(&item),
                            )?;
                            let claude_md_path = PathBuf::from(wt_path).join("CLAUDE.md");
                            fs::write(&claude_md_path, &claude_md).ok();

                            let resume_context = format!(
                                "{}\n\n## Resuming Work\n\n\
                                You are in worktree `{wt_path}` on branch `{branch}`.\n\
                                Current task: **{title}** (priority: {priority})\n\
                                Task file: `.k2so/agents/{agent}/work/active/{filename}`\n\n\
                                Continue where you left off. When done: `k2so work move --agent {agent} --file {filename} --from active --to done`",
                                claude_md,
                                agent = agent_name, wt_path = wt_path, branch = branch,
                                title = item.title, priority = item.priority, filename = item.filename,
                            );

                            let resume_kickoff = format!(
                                "Continue working on your task: **{}**. Check your progress and pick up where you left off.",
                                item.title
                            );

                            return Ok(serde_json::json!({
                                "command": command,
                                "args": ["--dangerously-skip-permissions", "--append-system-prompt", resume_context, resume_kickoff],
                                "cwd": wt_path,
                                "claudeMdPath": claude_md_path.to_string_lossy(),
                                "agentName": agent_name,
                                "worktreePath": wt_path,
                                "branch": branch,
                            }));
                        }
                    }
                }
            }
        }
    }

    // Case 2: delegate top-priority inbox item into a fresh worktree
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");
    if inbox_dir.exists() {
        let mut items: Vec<(PathBuf, WorkItem)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&inbox_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    if let Some(item) = read_work_item(&path, "inbox") {
                        items.push((path, item));
                    }
                }
            }
        }
        items.sort_by(|a, b| priority_rank(&a.1.priority).cmp(&priority_rank(&b.1.priority)));

        if let Some((top_path, _)) = items.into_iter().next() {
            let source_file = top_path.to_string_lossy().to_string();
            return crate::deprecated::delegate::k2so_agents_delegate(
                project_path,
                agent_name,
                source_file,
            );
        }
    }

    // Case 3: no work — launch in project root with general context.
    // The composed body is passed to --append-system-prompt below; no
    // file write needed (Phase 1a retired per-agent CLAUDE.md side-
    // effects).
    let claude_md = compose_agent_wake_context(&project_path, &agent_name, None)?;

    // Eagerly regen the workspace-root SKILL.md so the Claude session
    // (which launches from the workspace root) picks up the latest
    // CLI tools, PROJECT.md, and primary agent persona via the
    // symlinked ./CLAUDE.md → SKILL.md discovery path. Phase 2 Unit
    // 7c: now calls `workspace::write_workspace_skill_file` directly
    // — the SKILL scaffolding moved to k2so-core in Unit 7b so the
    // WorkspaceRegenProvider bridge that used to forward this call
    // back to src-tauri is no longer needed. Both daemon and Tauri
    // contexts hit the same body now.
    crate::workspace::skill_regen::write_workspace_skill_file(&project_path);

    // Check for previous session to resume. Lookup order:
    //   1. Heartbeat-scoped: agent_heartbeats.last_session_id (only when
    //      heartbeat_name was passed — keeps each heartbeat's chat thread
    //      separate from the user's Chat tab thread).
    //   2. Agent-global:    agent_sessions.session_id
    //   3. Filesystem scan: claude history under agent dir then project root
    // Each layer only returns a value if the underlying session file
    // still exists on disk — Claude prunes session JSONLs and stale ids
    // would cause "No conversation found" on resume; we clear the
    // stored id when that happens so the next wake starts fresh
    // instead of fighting the stale pointer indefinitely.
    let agent_cwd = agent_dir(&project_path, &agent_name);
    let resume_session = (|| -> Option<String> {
        let db = crate::db::shared();
        let conn = db.lock();
        let project_id = resolve_project_id(&conn, &project_path)?;

        // Layer 1: heartbeat-scoped resume target.
        if let Some(ref hb_name) = heartbeat_name {
            if let Ok(Some(hb)) =
                crate::db::schema::AgentHeartbeat::get_by_name(&conn, &project_id, hb_name)
            {
                if let Some(sid) = hb.last_session_id {
                    if !sid.is_empty() {
                        if chat_history::claude_session_file_exists(&sid, &project_path) {
                            return Some(sid);
                        }
                        // Stale — clear to fail forward.
                        let _ = crate::db::schema::AgentHeartbeat::save_session_id(
                            &conn,
                            &project_id,
                            hb_name,
                            "",
                        );
                    }
                }
            }
        }

        // Layer 2: workspace-global resume target.
        if let Ok(Some(session)) = WorkspaceSession::get(&conn, &project_id) {
            if let Some(sid) = session.session_id {
                if !sid.is_empty() {
                    if chat_history::claude_session_file_exists(&sid, &project_path) {
                        return Some(sid);
                    }
                    let _ = WorkspaceSession::clear_session_id(&conn, &project_id);
                }
            }
        }

        None
    })()
    .or_else(|| {
        chat_history::detect_active_session(
            "claude",
            &agent_cwd.to_string_lossy().to_string(),
        )
        .ok()
        .flatten()
    })
    .or_else(|| {
        chat_history::detect_active_session("claude", &project_path)
            .ok()
            .flatten()
    });

    let system_prompt = claude_md;
    // Heartbeats pass wakeup_override so each heartbeat row fires its
    // own workflow. Manual launches pass None and get the agent's
    // default wakeup.
    let wake_body = match wakeup_override.as_deref() {
        Some(p) => compose_wake_prompt_from_path(Path::new(p)),
        None => compose_wake_prompt_for_agent(&project_path, &agent_name),
    };

    let mut args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--append-system-prompt".to_string(),
        system_prompt,
    ];
    // --resume + --fork-session: restore the agent's conversation
    // history but mint a new session ID so Claude Code v2.1.90's stale-
    // session confirmation dialog doesn't block the wake. Heartbeats
    // pass skip_fork_session=true so wakes keep writing into the same
    // session (one growing chat per agent); the dismiss-stale-session
    // watcher in the host handles the dialog if it appears.
    if let Some(ref session_id) = resume_session {
        args.push("--resume".to_string());
        args.push(session_id.clone());
        if !skip_fork {
            args.push("--fork-session".to_string());
        }
    }

    // Wakes-since-compact counter: prepend `/compact` to the wake
    // message every WAKES_PER_COMPACT wakes so inherited conversation
    // history doesn't grow unbounded across heartbeats.
    const WAKES_PER_COMPACT: i64 = 20;
    let should_compact = (|| -> Option<bool> {
        let db = crate::db::shared();
        let conn = db.lock();
        let pid = resolve_project_id(&conn, &project_path)?;
        let n = WorkspaceSession::bump_wake_counter(&conn, &pid).ok()?;
        if n >= WAKES_PER_COMPACT {
            let _ = WorkspaceSession::reset_wake_counter(&conn, &pid);
            Some(true)
        } else {
            Some(false)
        }
    })()
    .unwrap_or(false);

    // The positional user message is the agent's wakeup.md content.
    // Fallback to a generic "begin" directive for agent-template
    // agents / fresh workspaces with no wakeup.md. When the compact
    // counter trips, prepend `/compact\n\n` so the slash command fires
    // first.
    let wake_message = wake_body.unwrap_or_else(|| "Begin your wake procedure now.".to_string());
    let wake_trigger = if should_compact {
        format!("/compact\n\n{}", wake_message)
    } else {
        wake_message
    };
    args.push(wake_trigger);

    let launch_cwd = project_path.clone();
    // Phase 1a: root CLAUDE.md is a symlink to canonical SKILL.md.
    let claude_md_path = PathBuf::from(&launch_cwd).join("CLAUDE.md");
    Ok(serde_json::json!({
        "command": command,
        "args": args,
        "cwd": launch_cwd,
        "claudeMdPath": claude_md_path.to_string_lossy(),
        "agentName": agent_name,
        "worktreePath": null,
        "branch": null,
        "resumeSession": resume_session,
        "didCompact": should_compact,
    }))
}

#[cfg(test)]
mod tests {
    //! Phase 2 Tier 2.1 coverage for the wake-launch orchestrator. The
    //! full happy-path traversal of all three cases (resume worktree /
    //! delegate inbox / fresh launch) touches scheduler + delegate +
    //! chat_history + DB + harness, so the unit-test surface here is
    //! intentionally narrow: missing-agent graceful handling +
    //! Case-3 cwd/command shape. Heavier integration coverage lives in
    //! the daemon-side `tests/agents_routes_integration.rs`.
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn scratch_workspace_with_primary(agent_name: &str, persona_type: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-agent-launch-test-{}-{}-{}",
            agent_name,
            std::process::id(),
            Uuid::new_v4(),
        ));
        // Unified-primary layout — the agent has an AGENT.md at
        // `.k2so/agent/`, so compose_agent_wake_context can resolve it.
        fs::create_dir_all(dir.join(".k2so/agent")).unwrap();
        let body = format!(
            "---\nname: {name}\ntype: {ptype}\nrole: test\n---\n\n# {name}\n\nTest body.\n",
            name = agent_name,
            ptype = persona_type,
        );
        fs::write(dir.join(".k2so/agent/AGENT.md"), body).unwrap();
        dir
    }

    #[test]
    fn build_launch_errors_when_no_agent_md_anywhere() {
        // Fresh workspace, no agent on disk at all. The function should
        // fall through to Case 3 (fresh launch) which calls
        // compose_agent_wake_context — which errors with
        // "Agent 'X' does not exist".
        let dir = std::env::temp_dir().join(format!(
            "k2so-launch-missing-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();

        let result = k2so_agents_build_launch(
            path,
            "ghost".to_string(),
            None,
            None,
            None,
            None,
        );

        match result {
            Ok(v) => panic!("expected error for missing agent, got {v:?}"),
            Err(e) => assert!(
                e.contains("does not exist"),
                "diagnostic should mention missing agent, got {e:?}",
            ),
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_launch_case3_returns_fresh_launch_shape_for_workspace_with_no_work() {
        // Unified-primary on disk, no inbox + no active worktree → fall
        // through to Case 3 (fresh launch in project root with
        // composed wake context).
        let dir = scratch_workspace_with_primary("scout", "custom");
        let path = dir.to_string_lossy().into_owned();

        let result = k2so_agents_build_launch(
            path.clone(),
            "scout".to_string(),
            Some("claude".to_string()),
            None,
            None,
            None,
        )
        .expect("fresh launch should succeed");

        // Case 3 contract:
        //   - command set to "claude"
        //   - cwd is the project root (NOT a worktree path)
        //   - worktreePath / branch are null
        //   - args includes --dangerously-skip-permissions
        assert_eq!(result["command"], "claude");
        assert_eq!(result["cwd"], path, "cwd is the project root in Case 3");
        assert!(
            result["worktreePath"].is_null(),
            "no worktree in fresh launch"
        );
        assert!(result["branch"].is_null(), "no branch in fresh launch");
        assert_eq!(result["agentName"], "scout");

        let args = result["args"].as_array().expect("args is array");
        assert!(
            args.iter().any(|v| v == "--dangerously-skip-permissions"),
            "fresh launch must include --dangerously-skip-permissions; got {args:?}",
        );
        assert!(
            args.iter().any(|v| v == "--append-system-prompt"),
            "fresh launch must include --append-system-prompt",
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_launch_honors_custom_agent_cli_command() {
        // Pass `agent_cli_command = Some("codex")` and verify it
        // overrides the default "claude" in the launch JSON.
        let dir = scratch_workspace_with_primary("scout", "custom");
        let path = dir.to_string_lossy().into_owned();

        let result = k2so_agents_build_launch(
            path,
            "scout".to_string(),
            Some("codex".to_string()),
            None,
            None,
            None,
        )
        .expect("launch ok");
        assert_eq!(
            result["command"], "codex",
            "agent_cli_command override should propagate to launch JSON"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_launch_command_defaults_to_claude_when_none_passed() {
        let dir = scratch_workspace_with_primary("scout", "custom");
        let path = dir.to_string_lossy().into_owned();

        let result = k2so_agents_build_launch(
            path,
            "scout".to_string(),
            None, // no override
            None,
            None,
            None,
        )
        .expect("launch ok");
        assert_eq!(
            result["command"], "claude",
            "None agent_cli_command must default to 'claude'"
        );

        fs::remove_dir_all(&dir).ok();
    }
}
