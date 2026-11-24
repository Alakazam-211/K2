//! Full-fat wake-launch argument builder.
//!
//! [`k2_agents_build_launch`] is the code path behind the UI's
//! "Launch agent" button and the scheduler-triggered heartbeat wake.
//! It returns the JSON the host (Tauri app or daemon) uses to spawn
//! `claude` with the right `cwd`, `args`, and `--resume` handling for
//! whichever branch of the decision tree applies:
//!
//! 1. **Resume an active worktree.** Agent has a work item in
//!    `active/` with a `worktree_path` frontmatter field → launch in
//!    that worktree, write the task CLAUDE.md, return args with a
//!    resume-context system prompt. The worktree must be under the
//!    workspace's own `.worktrees/`, reached with no link on the way;
//!    the folder-trust gate decides first, and CLAUDE.md is written
//!    only when it passes, never through a link.
//! 2. **Delegate from inbox.** Agent has work waiting in `inbox/` but
//!    no active worktree → call
//!    [`crate::deprecated::delegate::k2_agents_delegate`] (now
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

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::chat_history;
use crate::db::schema::WorkspaceSession;
use crate::skills::content::compose_agent_wake_context;
use crate::workspace::agent_identity::{agent_dir, parse_frontmatter, resolve_project_id};
use crate::workspace::scheduler::{agent_work_dir, priority_rank};
use crate::workspace::wake_prompts::{
    compose_wake_prompt_for_agent, compose_wake_prompt_from_path,
};
use crate::workspace::work_item::{read_work_item, WorkItem};

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
pub fn k2_agents_build_launch(
    project_path: String,
    agent_name: String,
    agent_cli_command: Option<String>,
    wakeup_override: Option<String>,
    skip_fork_session: Option<bool>,
    heartbeat_name: Option<String>,
) -> Result<serde_json::Value, String> {
    let command = agent_cli_command.unwrap_or_else(|| "claude".to_string());
    let skip_fork = skip_fork_session.unwrap_or(false);

    // Stamp-once heal: compose only if `.k2/AGENTS.md` is missing.
    // Must not rewrite the banner on host-session / Chat / cases 1–2.
    crate::workspace::skill_regen::ensure_agents_md_if_missing(&project_path);

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
                            // Only a folder under the workspace's own
                            // `.worktrees/`, reached by a walk that
                            // follows no link. Nothing is written yet.
                            let (base, rel) = worktree_rel(&project_path, wt_path)?;
                            let wt_dir = open_worktree(&base, &rel)?;
                            let claude_md = compose_agent_wake_context(
                                &project_path,
                                &agent_name,
                                Some(&item),
                            )?;
                            let claude_md_path = PathBuf::from(wt_path).join("CLAUDE.md");

                            let resume_context = format!(
                                "{}\n\n## Resuming Work\n\n\
                                You are in worktree `{wt_path}` on branch `{branch}`.\n\
                                Current task: **{title}** (priority: {priority})\n\
                                Task file: `.k2so/agents/{agent}/work/active/{filename}`\n\n\
                                Continue where you left off. When done: `k2 agent complete --agent {agent} --file {filename}`",
                                claude_md,
                                agent = agent_name, wt_path = wt_path, branch = branch,
                                title = item.title, priority = item.priority, filename = item.filename,
                            );

                            let resume_kickoff = format!(
                                "Continue working on your task: **{}**. Check your progress and pick up where you left off.",
                                item.title
                            );

                            let args = vec![
                                "--dangerously-skip-permissions".to_string(),
                                "--append-system-prompt".to_string(),
                                resume_context,
                                resume_kickoff,
                            ];
                            // The folder-trust gate decides before
                            // anything is written in the worktree.
                            ask_folder_gate(&project_path, &command, &args, wt_path)?;
                            write_task_claude_md(&wt_dir, &claude_md_path, &claude_md)?;

                            return Ok(serde_json::json!({
                                "command": command,
                                "args": args,
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
            #[allow(deprecated)] // deprecated-but-live delegate seam (Phase 2.1 PRD A23)
            return crate::deprecated::delegate::k2_agents_delegate(
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
    // — the SKILL scaffolding moved to k2-core in Unit 7b so the
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
    let mut missing_main: Option<String> = None;
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
                    // Only a Claude id is Claude's to touch. Another
                    // harness's pin (a custom preset's K2-minted Thread id,
                    // GL44; a grok/codex conversation) has no Claude JSONL
                    // by design and must survive this launch. And only a
                    // pre-allocated id that never started may be cleared:
                    // a real main chat that can't be found stays pinned.
                    if session.harness == "claude" {
                        let unstarted = crate::workspace::provider_resume::provider_resume_for_provider("claude")
                            .is_some_and(|adapter| {
                                crate::workspace::resume_chat::pin_counts_as_unstarted(&project_id, &sid, adapter, &project_path)
                            });
                        if unstarted {
                            let _ = WorkspaceSession::clear_session_id(&conn, &project_id);
                        } else if heartbeat_name.is_none() {
                            missing_main = Some(format!(
                                "{}: this workspace's main chat {sid} (claude) has no conversation on disk \
                                 in any claude folder. K2 kept it pinned and did not start another chat.",
                                crate::workspace::resume_chat::MAIN_CHAT_MISSING
                            ));
                        }
                    }
                }
            }
        }

        None
    })()
    .or_else(|| {
        chat_history::detect_active_session("claude", &agent_cwd.to_string_lossy().to_string())
            .ok()
            .flatten()
    })
    .or_else(|| {
        chat_history::detect_active_session("claude", &project_path)
            .ok()
            .flatten()
    });

    if let Some(refused) = missing_main {
        return Err(refused);
    }

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

/// Case 1 refusals: the worktree isn't under `<workspace>/.worktrees/`.
pub const WORKTREE_OUTSIDE_WORKSPACE: &str = "worktree_outside_workspace";
/// A link, or something that isn't a folder, on the way to the worktree.
pub const WORKTREE_NOT_A_FOLDER: &str = "worktree_not_a_folder";
/// Another user could change a folder on the way to the worktree.
pub const WORKTREE_FOLDER_UNSAFE: &str = "worktree_folder_unsafe";
/// The worktree folder isn't there.
pub const WORKTREE_MISSING: &str = "worktree_missing";
/// `<worktree>/CLAUDE.md` is a link (symbolic, or another name of a file).
pub const CLAUDE_MD_IS_A_LINK: &str = "claude_md_is_a_link";
/// The worktree or its CLAUDE.md couldn't be opened or written.
pub const WORKTREE_WRITE_FAILED: &str = "worktree_write_failed";

/// Where an active work item's `worktree_path` is: the workspace folder it
/// was matched against (as given, or canonical) and the names from there
/// down, starting with `.worktrees`. K2 makes every agent worktree at
/// `<workspace>/.worktrees/<branch>` ([`crate::git::create_worktree`]); a
/// path anywhere else, a relative one, or one with `..` is refused.
fn worktree_rel(project_path: &str, wt_path: &str) -> Result<(PathBuf, Vec<OsString>), String> {
    let refuse = || {
        format!(
            "{WORKTREE_OUTSIDE_WORKSPACE}: {wt_path} is not a folder under this workspace's .worktrees/ \
             ({project_path}/.worktrees); K2 wrote nothing"
        )
    };
    let wt = Path::new(wt_path);
    if !wt.is_absolute() {
        return Err(refuse());
    }
    let mut bases = vec![PathBuf::from(project_path)];
    if let Ok(c) = dunce::canonicalize(project_path) {
        bases.push(c);
    }
    for base in bases {
        let Ok(rest) = wt.strip_prefix(base.join(".worktrees")) else { continue };
        let mut rel = vec![OsString::from(".worktrees")];
        for c in rest.components() {
            match c {
                Component::Normal(n) => rel.push(n.to_os_string()),
                _ => return Err(refuse()),
            }
        }
        if rel.len() < 2 {
            return Err(refuse());
        }
        return Ok((base, rel));
    }
    Err(refuse())
}

fn walk_refusal(e: crate::hire::folder_walk::WalkError) -> String {
    use crate::hire::folder_walk::WalkError;
    match e {
        WalkError::NotAFolder(p) => format!(
            "{WORKTREE_NOT_A_FOLDER}: {} is a link or not a folder; K2 follows no link to a worktree and wrote nothing",
            p.display()
        ),
        WalkError::Unsafe(p) => format!(
            "{WORKTREE_FOLDER_UNSAFE}: another user can change {}; K2 wrote nothing",
            p.display()
        ),
        WalkError::Missing(p) => format!("{WORKTREE_MISSING}: {} is not there; K2 wrote nothing", p.display()),
        WalkError::AlreadyThere(p) => format!("{WORKTREE_WRITE_FAILED}: {}; K2 wrote nothing", p.display()),
        WalkError::Io(p, e) => format!("{WORKTREE_WRITE_FAILED}: {}: {e}; K2 wrote nothing", p.display()),
    }
}

/// The worktree folder, opened by a walk from the workspace down `rel`
/// that opens each part with `O_NOFOLLOW | O_DIRECTORY`
/// ([`crate::hire::folder_walk::walk_open`]).
#[cfg(unix)]
fn open_worktree(base: &Path, rel: &[OsString]) -> Result<std::os::fd::OwnedFd, String> {
    crate::hire::folder_walk::walk_open(base, rel).map_err(walk_refusal)
}

/// Not Unix: each part is looked at without following it.
#[cfg(not(unix))]
fn open_worktree(base: &Path, rel: &[OsString]) -> Result<PathBuf, String> {
    crate::hire::folder_walk::walk(base, rel, false).map_err(walk_refusal)?;
    let mut p = base.to_path_buf();
    for n in rel {
        p.push(n);
    }
    Ok(p)
}

/// The folder-trust gate for the worktree, asked the way a harness spawn
/// asks it ([`crate::cli_folder_trust::gate_harness_spawn`]): a held or
/// denied folder returns the gate's refusal string (its token turns into
/// the 409 body in the daemon's response).
fn ask_folder_gate(project_path: &str, command: &str, args: &[String], wt_path: &str) -> Result<(), String> {
    let project_id = {
        let db = crate::db::shared();
        let conn = db.lock();
        resolve_project_id(&conn, project_path)
    };
    let actor = crate::cli_folder_trust::gate::Actor {
        project_id,
        by: "launch".to_string(),
        tool: Some(command.to_string()),
        ..Default::default()
    };
    crate::cli_folder_trust::gate_harness_spawn(
        Some(command),
        args,
        &HashMap::new(),
        Some(Path::new(wt_path)),
        None,
        &actor,
    )
    .map(|_| ())
    .map_err(|r| r.to_error_string())
}

/// Write `<worktree>/CLAUDE.md` relative to the worktree's open folder:
/// `O_NOFOLLOW` (a link there is refused), and a file with another name
/// (a hard link) or that isn't a plain file is refused before it is cut.
#[cfg(unix)]
fn write_task_claude_md(dir: &std::os::fd::OwnedFd, at: &Path, body: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    let failed = |e: std::io::Error| format!("{WORKTREE_WRITE_FAILED}: {}: {e}", at.display());
    let name = std::ffi::CString::new("CLAUDE.md").map_err(|e| failed(std::io::Error::other(e)))?;
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;
    // SAFETY: a valid dirfd and C string; the fd is owned below.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, 0o644 as libc::c_uint) };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ELOOP) {
            return Err(format!("{CLAUDE_MD_IS_A_LINK}: {} is a link; K2 wrote nothing", at.display()));
        }
        return Err(failed(e));
    }
    // SAFETY: openat returned a new fd that nothing else owns.
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = f.metadata().map_err(failed)?;
    if !meta.file_type().is_file() {
        return Err(format!("{WORKTREE_WRITE_FAILED}: {} is not a plain file; K2 wrote nothing", at.display()));
    }
    if meta.nlink() > 1 {
        return Err(format!(
            "{CLAUDE_MD_IS_A_LINK}: {} has another name (a hard link); K2 wrote nothing",
            at.display()
        ));
    }
    f.set_len(0).map_err(failed)?;
    f.write_all(body.as_bytes()).map_err(failed)
}

/// Not Unix: the file is looked at without following it, then written.
#[cfg(not(unix))]
fn write_task_claude_md(_dir: &PathBuf, at: &Path, body: &str) -> Result<(), String> {
    if let Ok(m) = fs::symlink_metadata(at) {
        if m.file_type().is_symlink() {
            return Err(format!("{CLAUDE_MD_IS_A_LINK}: {} is a link; K2 wrote nothing", at.display()));
        }
    }
    fs::write(at, body).map_err(|e| format!("{WORKTREE_WRITE_FAILED}: {}: {e}", at.display()))
}

#[cfg(test)]
mod tests {
    //! Phase 2 Tier 2.1 coverage for the wake-launch orchestrator. The
    //! full happy-path traversal of all three cases (resume worktree /
    //! delegate inbox / fresh launch) touches scheduler + delegate +
    //! chat_history + DB + harness, so the unit-test surface here is
    //! intentionally narrow: missing-agent graceful handling +
    //! Case-3 cwd/command shape. Heavier integration coverage lives in
    //! the daemon-side `tests/it_sessions/agents_routes_integration.rs`.
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
            "k2-launch-missing-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();

        let result = k2_agents_build_launch(path, "ghost".to_string(), None, None, None, None);

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

        let result = k2_agents_build_launch(
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

        let result = k2_agents_build_launch(
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

        let result = k2_agents_build_launch(
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

    #[test]
    fn build_launch_with_existing_agents_md_does_not_rewrite_banner() {
        let dir = scratch_workspace_with_primary("scout", "custom");
        let path = dir.to_string_lossy().into_owned();
        crate::workspace::onboarding::set_agents_md_generate_enabled(&path, true).unwrap();
        crate::workspace::skill_regen::write_workspace_skill_file(&path);
        let canonical = crate::workspace_dot_dir(&path).join("AGENTS.md");
        let before = fs::read_to_string(&canonical).expect("canonical");
        let marker = "<!-- GENERATED at ";
        let i = before.find(marker).expect("banner");
        let rest = &before[i + marker.len()..];
        let stamp = rest[..rest.find(' ').expect("stamp")].to_string();

        let wt = dir.join(".worktrees").join("wt-resume");
        mkdirs_0755(&dir, &wt);
        let active = crate::workspace::scheduler::agent_work_dir(&path, "scout", "active");
        fs::create_dir_all(&active).unwrap();
        fs::write(
            active.join("task.md"),
            format!(
                "---\ntitle: Resume me\npriority: high\nworktree_path: {}\nbranch: feat\n---\n\nbody\n",
                wt.display()
            ),
        )
        .unwrap();

        let _ = k2_agents_build_launch(
            path.clone(),
            "scout".to_string(),
            Some("claude".to_string()),
            None,
            None,
            None,
        )
        .expect("case 1 launch");

        let after = fs::read_to_string(&canonical).expect("canonical after launch");
        let i = after.find(marker).expect("banner after");
        let rest = &after[i + marker.len()..];
        let after_stamp = &rest[..rest.find(' ').expect("stamp after")];
        assert_eq!(
            after_stamp,
            stamp.as_str(),
            "launch with existing .k2/AGENTS.md must not rewrite the banner"
        );

        fs::remove_dir_all(&dir).ok();
    }

    // ── Case 1: an active work item's worktree ───────────────────────

    /// Make `to` and set every folder from `root` down to it to 0755, so
    /// the tests never depend on the umask (the walk refuses a folder
    /// another user could write).
    fn mkdirs_0755(root: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for d in to.ancestors().take_while(|d| d.starts_with(root)) {
                fs::set_permissions(d, fs::Permissions::from_mode(0o755))
                    .unwrap_or_else(|e| panic!("chmod {}: {e}", d.display()));
            }
        }
    }

    /// A workspace under a temp HOME (so the folder-trust gate runs for
    /// real), with the agent's AGENT.md on disk.
    struct Ws {
        root: PathBuf,
        _home: crate::test_env::TempHome,
    }

    fn ws_under_temp_home(agent: &str) -> Ws {
        let home = crate::test_env::TempHome::new();
        crate::db::init_for_tests();
        let home_dir = dunce::canonicalize(home.path()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&home_dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let root = home_dir.join(format!("ws-{}", Uuid::new_v4().simple()));
        mkdirs_0755(&home_dir, &root);
        fs::create_dir_all(root.join(".k2so/agent")).unwrap();
        fs::write(
            root.join(".k2so/agent/AGENT.md"),
            format!("---\nname: {agent}\ntype: custom\nrole: test\n---\n\n# {agent}\n\nTest body.\n"),
        )
        .unwrap();
        Ws { root, _home: home }
    }

    /// An active work item whose `worktree_path:` is `wt`.
    fn put_active_item(root: &Path, agent: &str, wt: &Path) {
        let active = crate::workspace::scheduler::agent_work_dir(&root.to_string_lossy(), agent, "active");
        fs::create_dir_all(&active).unwrap();
        fs::write(
            active.join("task.md"),
            format!(
                "---\ntitle: Resume me\npriority: high\nworktree_path: {}\nbranch: feat\n---\n\nbody\n",
                wt.display()
            ),
        )
        .unwrap();
    }

    fn launch_in(root: &Path, agent: &str) -> Result<serde_json::Value, String> {
        k2_agents_build_launch(
            root.to_string_lossy().into_owned(),
            agent.to_string(),
            Some("claude".to_string()),
            None,
            None,
            None,
        )
    }

    const RISKY_HOOK: &str =
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"node .claude/setup.mjs"}]}]}}"#;

    #[test]
    fn case1_worktree_under_dot_worktrees_launches_and_writes_claude_md() {
        let ws = ws_under_temp_home("scout");
        let wt = ws.root.join(".worktrees").join("feat");
        mkdirs_0755(&ws.root, &wt);
        put_active_item(&ws.root, "scout", &wt);

        let v = launch_in(&ws.root, "scout").expect("a worktree under .worktrees/ launches");
        assert_eq!(v["cwd"], wt.to_string_lossy().as_ref());
        assert_eq!(v["worktreePath"], wt.to_string_lossy().as_ref());
        assert_eq!(v["branch"], "feat");
        let body = fs::read_to_string(wt.join("CLAUDE.md")).expect("CLAUDE.md written in the worktree");
        assert!(body.contains("scout"), "the task CLAUDE.md is the composed wake context: {body}");
    }

    #[test]
    fn case1_worktree_outside_the_workspace_is_refused_and_nothing_is_written() {
        let ws = ws_under_temp_home("scout");
        let outside = ws.root.parent().unwrap().join(format!("elsewhere-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&outside).unwrap();
        put_active_item(&ws.root, "scout", &outside);

        let e = launch_in(&ws.root, "scout").expect_err("a worktree outside the workspace is refused");
        assert!(e.starts_with("worktree_outside_workspace"), "named code, got {e:?}");
        assert!(
            fs::symlink_metadata(outside.join("CLAUDE.md")).is_err(),
            "nothing is written outside the workspace"
        );
        fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn case1_worktree_inside_the_workspace_but_not_under_dot_worktrees_is_refused() {
        let ws = ws_under_temp_home("scout");
        let wt = ws.root.join("src").join("wt");
        mkdirs_0755(&ws.root, &wt);
        put_active_item(&ws.root, "scout", &wt);

        let e = launch_in(&ws.root, "scout").expect_err("only <workspace>/.worktrees/ holds worktrees");
        assert!(e.starts_with("worktree_outside_workspace"), "named code, got {e:?}");
        assert!(fs::symlink_metadata(wt.join("CLAUDE.md")).is_err(), "nothing is written");
    }

    #[test]
    fn case1_dot_dot_in_the_worktree_path_is_refused() {
        let ws = ws_under_temp_home("scout");
        mkdirs_0755(&ws.root, &ws.root.join(".worktrees"));
        let wt = ws.root.join(".worktrees").join("..").join("src");
        mkdirs_0755(&ws.root, &ws.root.join("src"));
        put_active_item(&ws.root, "scout", &wt);

        let e = launch_in(&ws.root, "scout").expect_err("`..` leaves .worktrees/");
        assert!(e.starts_with("worktree_outside_workspace"), "named code, got {e:?}");
        assert!(fs::symlink_metadata(ws.root.join("src/CLAUDE.md")).is_err(), "nothing is written");
    }

    #[test]
    fn case1_held_folder_writes_nothing_and_the_launch_is_refused() {
        let ws = ws_under_temp_home("scout");
        let wt = ws.root.join(".worktrees").join("risky");
        mkdirs_0755(&ws.root, &wt.join(".claude"));
        fs::write(wt.join(".claude/settings.json"), RISKY_HOOK).unwrap();
        put_active_item(&ws.root, "scout", &wt);

        let e = launch_in(&ws.root, "scout").expect_err("a held folder is refused");
        assert!(
            e.starts_with(crate::cli_folder_trust::gate::PENDING),
            "the gate's refusal comes back, got {e:?}"
        );
        assert!(
            crate::cli_folder_trust::gate::refusal_in_text(&e).is_some(),
            "the error carries the refusal token the daemon turns into a 409: {e:?}"
        );
        assert!(fs::symlink_metadata(wt.join("CLAUDE.md")).is_err(), "a held folder gets no CLAUDE.md");
    }

    #[cfg(unix)]
    #[test]
    fn case1_link_at_claude_md_is_refused_and_its_target_is_untouched() {
        let ws = ws_under_temp_home("scout");
        let wt = ws.root.join(".worktrees").join("linked");
        mkdirs_0755(&ws.root, &wt);
        let target = ws.root.join("target.txt");
        fs::write(&target, "original").unwrap();
        std::os::unix::fs::symlink(&target, wt.join("CLAUDE.md")).unwrap();
        put_active_item(&ws.root, "scout", &wt);

        let e = launch_in(&ws.root, "scout").expect_err("a link at CLAUDE.md is refused");
        assert!(e.starts_with("claude_md_is_a_link"), "named code, got {e:?}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "original", "the link's target is untouched");
        assert!(fs::symlink_metadata(wt.join("CLAUDE.md")).unwrap().file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn case1_hard_link_at_claude_md_is_refused_and_its_other_name_is_untouched() {
        let ws = ws_under_temp_home("scout");
        let wt = ws.root.join(".worktrees").join("hard");
        mkdirs_0755(&ws.root, &wt);
        let other = ws.root.join("other.txt");
        fs::write(&other, "original").unwrap();
        fs::hard_link(&other, wt.join("CLAUDE.md")).unwrap();
        put_active_item(&ws.root, "scout", &wt);

        let e = launch_in(&ws.root, "scout").expect_err("a hard link at CLAUDE.md is refused");
        assert!(e.starts_with("claude_md_is_a_link"), "named code, got {e:?}");
        assert_eq!(fs::read_to_string(&other).unwrap(), "original", "the other name is untouched");
    }

    #[cfg(unix)]
    #[test]
    fn case1_link_on_the_way_to_the_worktree_is_refused_and_nothing_is_written() {
        let ws = ws_under_temp_home("scout");
        let outside = ws.root.parent().unwrap().join(format!("outside-{}", Uuid::new_v4().simple()));
        mkdirs_0755(&outside, &outside);
        mkdirs_0755(&ws.root, &ws.root.join(".worktrees"));
        std::os::unix::fs::symlink(&outside, ws.root.join(".worktrees").join("via-link")).unwrap();
        put_active_item(&ws.root, "scout", &ws.root.join(".worktrees").join("via-link"));

        let e = launch_in(&ws.root, "scout").expect_err("a link on the way is refused");
        assert!(e.starts_with("worktree_not_a_folder"), "named code, got {e:?}");
        assert!(fs::symlink_metadata(outside.join("CLAUDE.md")).is_err(), "nothing is written through the link");
        fs::remove_dir_all(&outside).ok();
    }

    #[cfg(unix)]
    #[test]
    fn case1_dot_worktrees_itself_a_link_is_refused() {
        let ws = ws_under_temp_home("scout");
        let outside = ws.root.parent().unwrap().join(format!("outside-{}", Uuid::new_v4().simple()));
        mkdirs_0755(&outside, &outside.join("feat"));
        std::os::unix::fs::symlink(&outside, ws.root.join(".worktrees")).unwrap();
        put_active_item(&ws.root, "scout", &ws.root.join(".worktrees").join("feat"));

        let e = launch_in(&ws.root, "scout").expect_err("a linked .worktrees is refused");
        assert!(e.starts_with("worktree_not_a_folder"), "named code, got {e:?}");
        assert!(fs::symlink_metadata(outside.join("feat/CLAUDE.md")).is_err(), "nothing is written through the link");
        fs::remove_dir_all(&outside).ok();
    }

    /// The umask, put back on drop (nextest runs each test in its own
    /// process).
    #[cfg(unix)]
    struct Umask(libc::mode_t);

    #[cfg(unix)]
    impl Drop for Umask {
        fn drop(&mut self) {
            // SAFETY: umask has no preconditions.
            unsafe { libc::umask(self.0) };
        }
    }

    #[cfg(unix)]
    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
    }

    /// With a umask that leaves group write on, a worktree K2 makes
    /// (`.worktrees` 0755, the worktree chmod go-w) launches end to end; a
    /// group-writable worktree K2 didn't make is still refused.
    #[cfg(unix)]
    #[test]
    fn case1_a_worktree_k2_made_launches_whatever_the_umask_and_one_others_can_write_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let ws = ws_under_temp_home("scout");
        let git = |args: &[&str]| {
            let st = std::process::Command::new("git").args(args).current_dir(&ws.root).status().expect("git");
            assert!(st.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);
        // SAFETY: umask has no preconditions.
        let _umask = Umask(unsafe { libc::umask(0o002) });
        let made = crate::git::create_worktree(&ws.root.to_string_lossy(), "feat-x").expect("a worktree");
        let wt = PathBuf::from(&made.path);
        assert_eq!(mode_of(&ws.root.join(".worktrees")), 0o755, ".worktrees as K2 makes it");
        assert_eq!(mode_of(&wt) & 0o022, 0, "the worktree has no group or other write");

        put_active_item(&ws.root, "scout", &wt);
        let out = launch_in(&ws.root, "scout").expect("a worktree K2 made launches");
        assert_eq!(out["worktreePath"], made.path.as_str(), "{out}");
        assert!(wt.join("CLAUDE.md").is_file(), "CLAUDE.md written in the worktree");

        let manual = ws.root.join(".worktrees").join("by-hand");
        fs::create_dir(&manual).unwrap();
        fs::set_permissions(&manual, fs::Permissions::from_mode(0o775)).unwrap();
        put_active_item(&ws.root, "scout", &manual);
        let e = launch_in(&ws.root, "scout").expect_err("a worktree others can write is refused");
        assert!(e.starts_with("worktree_folder_unsafe"), "named code, got {e:?}");
        assert!(fs::symlink_metadata(manual.join("CLAUDE.md")).is_err(), "nothing written there");
    }
}
