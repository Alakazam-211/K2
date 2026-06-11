//! Review queue: workspace-manager approval path for agent work.
//!
//! When an agent moves an item to `done/`, its branch lands on the
//! review queue. The workspace manager (or the CLI) then:
//!
//! - [`review_queue`] — list of per-agent `ReviewItem`s with diff
//!   summary + worktree path, what the UI renders in the Review panel.
//! - [`review_approve`] — merge agent branch → main, remove worktree,
//!   delete branch, archive done items, unlock the agent.
//! - [`review_reject`] — discard worktree + branch, move done items
//!   back to `inbox/` (stripped of worktree frontmatter), optionally
//!   drop a feedback file so the next attempt has context. Unlock.
//! - [`review_request_changes`] — just a feedback file in inbox. No
//!   worktree teardown; the agent keeps their working branch.
//!
//! Moved to core so the daemon can serve `/cli/reviews` +
//! `/cli/review/{approve,reject,feedback}` headlessly.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::deprecated::delegate::strip_worktree_from_frontmatter;
use crate::workspace::agent_identity::parse_frontmatter;
use crate::workspace::scheduler::{agent_work_dir, get_workspace_state};
use crate::workspace::session::{k2so_agents_unlock, simple_date};
use crate::workspace::work_item::{atomic_write, read_work_item, WorkItem};

/// One file in the branch diff between main and the agent's worktree.
/// Mirrors `crate::git::FileDiffSummary` but drops the `old_path` field
/// since the review UI doesn't surface renames yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffFile {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

/// One entry on the review queue — an agent with done items plus its
/// associated worktree/branch and the diff summary vs main.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    pub agent_name: String,
    pub branch: String,
    pub worktree_path: Option<String>,
    pub work_items: Vec<WorkItem>,
    pub diff_summary: Vec<ReviewDiffFile>,
}

/// Enumerate the review queue from the workspace's unified inbox
/// (`.k2so/inbox/done/`).
///
/// **Post-Phase-2.5b model.** Pre-2.5b this walked
/// `.k2so/agents/<name>/work/done/` — a directory tree retired by
/// the workspace==agent unification migration. On every upgraded
/// workspace the old walk silently returned `Ok(vec![])`, breaking
/// the desktop Review Queue UI, `k2so reviews`, and the companion's
/// pending-review badge. The walk now enumerates the workspace's
/// post-2.5b unified inbox (`.k2so/inbox/done/*.md`), which is where
/// completed work waits for review.
///
/// **Grouping.** Each done item's frontmatter carries the `branch:`
/// field stamped by `delegate` (see
/// [`crate::deprecated::delegate::add_worktree_to_frontmatter`]).
/// The branch follows the `agent/<name>/<task>` convention, so the
/// agent name is recovered by parsing the second segment. Items
/// sharing a branch (multiple done items on the same worktree)
/// collapse into one [`ReviewItem`] — that matches what a reviewer
/// sees in the panel.
///
/// **Orphan bucket.** Done items missing a `branch:` frontmatter
/// land in a synthetic `"unbranched"` group (with an empty
/// `branch`/`worktree_path`/`diff_summary`). This keeps stranded
/// items visible to a human reviewer instead of silently dropping
/// them.
///
/// Returns an empty Vec when `.k2so/inbox/done/` doesn't exist
/// (fresh workspace or no completed work yet) — not an error.
pub fn review_queue(project_path: &str) -> Result<Vec<ReviewItem>, String> {
    let workspace = Path::new(project_path);
    let done_dir = crate::inbox::folder_path(workspace, "done");

    if !done_dir.exists() {
        return Ok(vec![]);
    }

    let worktrees = crate::git::list_worktrees(project_path);

    // Group done items by branch — items on the same worktree
    // collapse into a single ReviewItem so the panel renders one
    // entry per agent-branch (matching pre-2.5b behaviour) instead
    // of one entry per file.
    //
    // BTreeMap so the output order is deterministic across runs.
    // Value carries (agent_name, work_items) — agent_name is
    // derived from the branch's `agent/<name>/...` convention.
    let mut by_branch: BTreeMap<String, (String, Vec<WorkItem>)> = BTreeMap::new();
    // Items with no `branch:` frontmatter live here as a separate
    // synthetic bucket so a reviewer can still see + act on them.
    let mut unbranched: Vec<WorkItem> = Vec::new();

    let entries = match fs::read_dir(&done_dir) {
        Ok(e) => e,
        Err(e) => return Err(format!("read_dir {}: {}", done_dir.display(), e)),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !path.extension().map_or(false, |ext| ext == "md") {
            continue;
        }
        // Read content twice — once for the WorkItem (parsed body /
        // priority / type), once to pull the branch out of
        // frontmatter (WorkItem doesn't expose every frontmatter key).
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let item = match read_work_item(&path, "done") {
            Some(i) => i,
            None => continue,
        };

        let fm = parse_frontmatter(&content);
        let branch = fm.get("branch").cloned().unwrap_or_default();

        if branch.is_empty() {
            unbranched.push(item);
            continue;
        }

        let agent_name = agent_name_from_branch(&branch);
        by_branch
            .entry(branch)
            .or_insert_with(|| (agent_name, Vec::new()))
            .1
            .push(item);
    }

    let mut reviews = Vec::with_capacity(by_branch.len() + 1);
    for (branch, (agent_name, work_items)) in by_branch {
        let matching_worktree = worktrees.iter().find(|wt| !wt.is_main && wt.branch == branch);

        let diff_summary: Vec<ReviewDiffFile> = if matching_worktree.is_some() {
            crate::git::diff_between_branches(project_path, "main", &branch)
                .unwrap_or_default()
                .into_iter()
                .map(|f| ReviewDiffFile {
                    path: f.path,
                    status: f.status,
                    additions: f.additions,
                    deletions: f.deletions,
                })
                .collect()
        } else {
            vec![]
        };

        reviews.push(ReviewItem {
            agent_name,
            branch,
            worktree_path: matching_worktree.map(|wt| wt.path.clone()),
            work_items,
            diff_summary,
        });
    }

    if !unbranched.is_empty() {
        reviews.push(ReviewItem {
            agent_name: "unbranched".to_string(),
            branch: String::new(),
            worktree_path: None,
            work_items: unbranched,
            diff_summary: vec![],
        });
    }

    Ok(reviews)
}

/// Recover an agent name from a branch following the
/// `agent/<name>/<task>` convention stamped by `delegate`. Falls back
/// to the full branch string if it doesn't match the convention so a
/// hand-crafted branch (e.g. `manual-fix`) still produces a meaningful
/// `agent_name` field in the [`ReviewItem`].
fn agent_name_from_branch(branch: &str) -> String {
    if let Some(rest) = branch.strip_prefix("agent/") {
        if let Some((name, _)) = rest.split_once('/') {
            return name.to_string();
        }
        // `agent/foo` with no trailing task segment — still recognizable.
        return rest.to_string();
    }
    branch.to_string()
}

/// Approve + merge + tear down. Returns a short human-readable status
/// string (e.g., `"Approved and merged: 7 files"`).
pub fn review_approve(
    project_path: String,
    branch: String,
    agent_name: String,
) -> Result<String, String> {
    let result = crate::git::merge_branch(&project_path, &branch)?;

    if !result.success {
        return Err(format!("Merge conflicts: {}", result.conflicts.join(", ")));
    }

    // Find the agent's worktree by branch, remove it, drop DB row.
    let worktrees = crate::git::list_worktrees(&project_path);
    if let Some(wt) = worktrees.iter().find(|wt| wt.branch == branch) {
        let wt_path = wt.path.clone();
        let _ = crate::git::remove_worktree(&project_path, &wt_path, true);

        {
            let db = crate::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "DELETE FROM workspaces WHERE worktree_path = ?1",
                rusqlite::params![wt_path],
            );
        }
    }

    let _ = crate::git::delete_branch(&project_path, &branch);

    // Archive done items for this agent (they live in git now).
    let done_dir = agent_work_dir(&project_path, &agent_name, "done");
    if done_dir.exists() {
        if let Ok(entries) = fs::read_dir(&done_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }

    let _ = k2so_agents_unlock(project_path, agent_name);

    Ok(format!("Approved and merged: {} files", result.merged_files))
}

/// Reject the branch. Tears down the worktree, moves done items back
/// to `inbox/` (stripped of worktree frontmatter so a fresh worktree
/// is created on retry), optionally writes a `review-feedback-*.md`
/// with the rejection reason, unlocks the agent.
pub fn review_reject(
    project_path: String,
    agent_name: String,
    reason: Option<String>,
) -> Result<(), String> {
    let done_dir = agent_work_dir(&project_path, &agent_name, "done");
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");

    if !done_dir.exists() {
        return Ok(());
    }

    // Nuke the worktree + branch + DB workspace row for this agent.
    let worktrees = crate::git::list_worktrees(&project_path);
    let agent_prefix = format!("agent/{}/", agent_name);
    for wt in worktrees.iter().filter(|wt| wt.branch.starts_with(&agent_prefix)) {
        let wt_path = wt.path.clone();
        if let Err(e) = crate::git::remove_worktree(&project_path, &wt_path, true) {
            crate::log_debug!("[review-reject] Failed to remove worktree {}: {}", wt_path, e);
        }
        if let Err(e) = crate::git::delete_branch(&project_path, &wt.branch) {
            crate::log_debug!("[review-reject] Failed to delete branch {}: {}", wt.branch, e);
        }
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "DELETE FROM workspaces WHERE worktree_path = ?1",
                rusqlite::params![wt_path],
            );
        }
    }

    fs::create_dir_all(&inbox_dir).map_err(|e| format!("Failed to create inbox dir: {}", e))?;
    if let Ok(entries) = fs::read_dir(&done_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let filename = match path.file_name() {
                    Some(f) => f.to_owned(),
                    None => continue,
                };
                let target = inbox_dir.join(&filename);
                if let Ok(content) = fs::read_to_string(&path) {
                    let cleaned = strip_worktree_from_frontmatter(&content);
                    if let Err(e) = atomic_write(&target, &cleaned) {
                        crate::log_debug!("[review-reject] Failed to write cleaned work item: {}", e);
                    }
                } else if let Err(e) = fs::rename(&path, &target) {
                    crate::log_debug!("[review-reject] Failed to move work item: {}", e);
                }
                let _ = fs::remove_file(&path);
            }
        }
    }

    if let Some(reason) = reason {
        let now = simple_date();
        let content = format!(
            "---\ntitle: Review Feedback — Work Rejected\npriority: high\nassigned_by: reviewer\ncreated: {}\ntype: feedback\n---\n\n## Rejection Reason\n\n{}\n\n## Action Required\n\nReview the feedback above and address the issues in your next attempt.\nA fresh worktree will be created when you are relaunched.\n",
            now, reason
        );
        let filename = format!("review-feedback-{}.md", now);
        let path = inbox_dir.join(&filename);
        atomic_write(&path, &content)?;
    }

    let _ = k2so_agents_unlock(project_path, agent_name);

    Ok(())
}

/// Drop a feedback file in the agent's inbox without tearing down
/// their worktree. The agent keeps their branch; they address the
/// feedback and move the item to `done/` again.
pub fn review_request_changes(
    project_path: String,
    agent_name: String,
    feedback: String,
) -> Result<(), String> {
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");
    if !inbox_dir.exists() {
        fs::create_dir_all(&inbox_dir).map_err(|e| e.to_string())?;
    }

    let now = simple_date();
    let content = format!(
        "---\ntitle: Review Feedback — Changes Requested\npriority: high\nassigned_by: reviewer\ncreated: {}\ntype: feedback\n---\n\n## Requested Changes\n\n{}\n\n## Action Required\n\nAddress the feedback above, then move this item to done/ when complete.\n",
        now, feedback
    );
    let filename = format!("review-feedback-{}.md", now);
    let path = inbox_dir.join(&filename);
    atomic_write(&path, &content)?;

    Ok(())
}

/// Sub-agent completion. Reads the work item's frontmatter, consults
/// the workspace state's capability for the item's `source`, and
/// either auto-merges (`auto` mode — delegates to [`review_approve`])
/// or moves the file from `active/` to `done/` for human review
/// (`gated` mode). Returns JSON the CLI echoes back.
pub fn agent_complete(
    project_path: String,
    agent_name: String,
    filename: String,
) -> Result<String, String> {
    let active_dir = agent_work_dir(&project_path, &agent_name, "active");
    let item_path = active_dir.join(&filename);
    if !item_path.exists() {
        return Err(format!("Work item not found: {}", filename));
    }
    let content = fs::read_to_string(&item_path).unwrap_or_default();
    let fm = parse_frontmatter(&content);
    let source = fm
        .get("source")
        .cloned()
        .unwrap_or_else(|| "manual".to_string());

    let capability = if let Some(ws_state) = get_workspace_state(&project_path) {
        ws_state.capability_for_source(&source).to_string()
    } else {
        "gated".to_string()
    };

    let branch = fm.get("branch").cloned().unwrap_or_default();

    if capability == "auto" && !branch.is_empty() {
        match review_approve(project_path.clone(), branch.clone(), agent_name.clone()) {
            Ok(_) => Ok(serde_json::json!({
                "mode": "auto",
                "action": "merged",
                "branch": branch,
                "agent": agent_name,
            })
            .to_string()),
            Err(e) => Err(format!("Auto-merge failed: {}", e)),
        }
    } else {
        let done_dir = agent_work_dir(&project_path, &agent_name, "done");
        fs::create_dir_all(&done_dir).ok();
        let dest = done_dir.join(&filename);
        fs::rename(&item_path, &dest).map_err(|e| format!("Failed to move to done: {}", e))?;

        Ok(serde_json::json!({
            "mode": "gated",
            "action": "moved_to_done",
            "branch": branch,
            "agent": agent_name,
            "file": filename,
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_item_serializes_camel_case() {
        let item = ReviewItem {
            agent_name: "backend".to_string(),
            branch: "agent/backend/task".to_string(),
            worktree_path: Some("/tmp/wt".to_string()),
            work_items: vec![],
            diff_summary: vec![],
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"agentName\":\"backend\""));
        assert!(json.contains("\"worktreePath\":\"/tmp/wt\""));
        assert!(json.contains("\"workItems\":[]"));
        assert!(json.contains("\"diffSummary\":[]"));
    }

    #[test]
    fn agent_name_from_branch_extracts_second_segment() {
        assert_eq!(
            agent_name_from_branch("agent/backend-eng/build-auth"),
            "backend-eng",
        );
        assert_eq!(agent_name_from_branch("agent/qa/run-suite"), "qa");
        // No trailing task — single-segment after the prefix still resolves.
        assert_eq!(agent_name_from_branch("agent/cli-eng"), "cli-eng");
        // Hand-rolled branch (not delegate-stamped) — fallback to the
        // full string so the reviewer sees something meaningful.
        assert_eq!(agent_name_from_branch("hotfix/sentry"), "hotfix/sentry");
        assert_eq!(agent_name_from_branch("main"), "main");
    }

    /// Phase 2.5b regression — `review_queue` must read from
    /// `.k2so/inbox/done/`, NOT the retired `.k2so/agents/<n>/work/done/`
    /// tree. Pre-fix the walk returned `Ok(vec![])` on every upgraded
    /// workspace, silently breaking the review UI and the companion's
    /// pending-review badge.
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    static REVIEW_TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_scratch_ws() -> std::path::PathBuf {
        let n = REVIEW_TEST_COUNTER.fetch_add(1, AtomicOrdering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "k2so-review-queue-test-{}-{}-{}",
            std::process::id(),
            nanos,
            n
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join(".k2so")).unwrap();
        p
    }

    fn write_done_item(workspace: &Path, filename: &str, frontmatter: &str, body: &str) {
        let done = workspace.join(".k2so").join("inbox").join("done");
        fs::create_dir_all(&done).unwrap();
        let content = format!("---\n{}---\n\n{}\n", frontmatter, body);
        fs::write(done.join(filename), content).unwrap();
    }

    #[test]
    fn review_queue_empty_on_post_2_5b_workspace_with_no_done_items() {
        // Fresh post-2.5b workspace: `.k2so/` exists, no inbox/done/
        // subdir. Must return an empty Vec, NOT an error.
        let ws = unique_scratch_ws();
        let result = review_queue(&ws.to_string_lossy()).expect("review_queue ok");
        assert!(
            result.is_empty(),
            "post-2.5b workspace with no done items should return empty Vec; got {result:?}"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn review_queue_lists_items_from_workspace_inbox_done() {
        // Seed two done items on the same branch — they should
        // collapse into one ReviewItem (matching pre-2.5b grouping
        // by agent).
        let ws = unique_scratch_ws();
        write_done_item(
            &ws,
            "oauth-flow.md",
            "title: OAuth flow\npriority: high\ntype: task\nbranch: agent/backend-eng/oauth-flow\n",
            "Body 1",
        );
        write_done_item(
            &ws,
            "oauth-tests.md",
            "title: OAuth tests\npriority: normal\ntype: task\nbranch: agent/backend-eng/oauth-flow\n",
            "Body 2",
        );
        // Different agent, different branch — separate ReviewItem.
        write_done_item(
            &ws,
            "ui-polish.md",
            "title: UI polish\npriority: normal\ntype: task\nbranch: agent/frontend-eng/ui-polish\n",
            "Body 3",
        );

        let result = review_queue(&ws.to_string_lossy()).expect("review_queue ok");
        assert_eq!(
            result.len(),
            2,
            "two distinct branches should produce 2 ReviewItems; got {result:?}"
        );

        let backend = result
            .iter()
            .find(|r| r.agent_name == "backend-eng")
            .expect("backend-eng ReviewItem must be present");
        assert_eq!(backend.branch, "agent/backend-eng/oauth-flow");
        assert_eq!(
            backend.work_items.len(),
            2,
            "two done items on the same branch should collapse into one ReviewItem"
        );

        let frontend = result
            .iter()
            .find(|r| r.agent_name == "frontend-eng")
            .expect("frontend-eng ReviewItem must be present");
        assert_eq!(frontend.branch, "agent/frontend-eng/ui-polish");
        assert_eq!(frontend.work_items.len(), 1);

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn review_queue_does_not_walk_legacy_agents_dir_anymore() {
        // The legacy walk (`.k2so/agents/<n>/work/done/`) is the bug
        // we're fixing. Seed BOTH layouts on the same workspace; the
        // new code must surface ONLY the inbox items. If a stray
        // legacy item leaks through, this assertion fails and surfaces
        // the regression.
        let ws = unique_scratch_ws();

        // Legacy shape (must be ignored).
        let legacy_done = ws.join(".k2so/agents/old-agent/work/done");
        fs::create_dir_all(&legacy_done).unwrap();
        fs::write(
            legacy_done.join("legacy-task.md"),
            "---\ntitle: Legacy task\nbranch: agent/old-agent/legacy\n---\n\nlegacy body",
        )
        .unwrap();

        // New shape (must appear).
        write_done_item(
            &ws,
            "new-task.md",
            "title: New task\nbranch: agent/new-agent/new-task\n",
            "new body",
        );

        let result = review_queue(&ws.to_string_lossy()).expect("review_queue ok");
        assert_eq!(
            result.len(),
            1,
            "expected exactly 1 ReviewItem (the inbox one); got {result:?}"
        );
        assert_eq!(result[0].agent_name, "new-agent");
        assert_eq!(result[0].work_items.len(), 1);
        assert_eq!(result[0].work_items[0].title, "New task");

        // Belt-and-suspenders: no ReviewItem should carry the legacy
        // agent name or any item from the legacy tree.
        assert!(
            !result.iter().any(|r| r.agent_name == "old-agent"),
            "legacy `agents/old-agent/work/done/` items must not leak through"
        );
        for r in &result {
            assert!(
                !r.work_items.iter().any(|w| w.title == "Legacy task"),
                "no work item from the legacy tree should be surfaced"
            );
        }

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn review_queue_surfaces_unbranched_items_in_separate_bucket() {
        // Done items without a `branch:` frontmatter shouldn't
        // silently drop — they land in the synthetic "unbranched"
        // group so a reviewer can still see + act on them.
        let ws = unique_scratch_ws();
        write_done_item(
            &ws,
            "orphan.md",
            "title: Orphan item\npriority: normal\n",
            "no branch in frontmatter",
        );
        write_done_item(
            &ws,
            "normal.md",
            "title: Normal\nbranch: agent/x/normal\n",
            "branched",
        );

        let result = review_queue(&ws.to_string_lossy()).expect("review_queue ok");
        let unbranched = result
            .iter()
            .find(|r| r.agent_name == "unbranched")
            .expect("unbranched bucket must be present");
        assert!(unbranched.branch.is_empty());
        assert!(unbranched.worktree_path.is_none());
        assert_eq!(unbranched.work_items.len(), 1);
        assert_eq!(unbranched.work_items[0].title, "Orphan item");

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn review_diff_file_serializes_camel_case() {
        let f = ReviewDiffFile {
            path: "src/main.rs".to_string(),
            status: "M".to_string(),
            additions: 3,
            deletions: 1,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"path\":\"src/main.rs\""));
        assert!(json.contains("\"additions\":3"));
    }
}
