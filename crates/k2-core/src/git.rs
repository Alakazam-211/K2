use git2::{DiffOptions, Repository, StatusOptions, BranchType};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

// ── Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub is_repo: bool,
    pub current_branch: String,
    pub ahead: i32,
    pub behind: i32,
    pub changed_files: i32,
    pub untracked_files: i32,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchList {
    pub current: String,
    pub local: Vec<String>,
    pub remote: Vec<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub is_main: bool,
    pub is_bare: bool,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub staged: bool,
}

// ── Git Info ──────────────────────────────────────────────────────────────

pub fn get_git_info(path: &str) -> GitInfo {
    let default = GitInfo {
        is_repo: false,
        current_branch: String::new(),
        ahead: 0,
        behind: 0,
        changed_files: 0,
        untracked_files: 0,
    };

    let repo = match Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return default,
    };

    // Get current branch
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => {
            return GitInfo {
                is_repo: true,
                ..default
            };
        }
    };

    let current_branch = head
        .shorthand()
        .unwrap_or("")
        .to_string();

    // Count changed and untracked files. We deliberately do NOT
    // `recurse_untracked_dirs(true)` here even though it would
    // give a more precise per-file untracked count. The recurse
    // makes libgit2 walk into every untracked directory — most
    // notably `node_modules/`, `target/`, `dist/`, `.next/`, etc.
    // — which on a JS/TS workspace with 100k+ files inside
    // node_modules turns each poll into a multi-hundred-thousand
    // stat-and-attribute-load loop. With this hook polled every
    // 5 seconds (`useGit::useGitInfo`), that pegs the Tauri main
    // process at ~200% CPU. Without recurse, libgit2 reports the
    // untracked directory as a single entry — which is exactly
    // what the sidebar's "is this workspace dirty?" indicator
    // needs anyway. Per-file detail is available on demand via
    // `get_changed_files` when the user opens the commit panel.
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);

    let (changed_files, untracked_files) = match repo.statuses(Some(&mut opts)) {
        Ok(statuses) => {
            let mut changed = 0i32;
            let mut untracked = 0i32;
            for entry in statuses.iter() {
                let s = entry.status();
                if s.contains(git2::Status::WT_NEW) || s.contains(git2::Status::INDEX_NEW) {
                    // Check if it's purely untracked (not staged as new)
                    if s.contains(git2::Status::WT_NEW) && !s.contains(git2::Status::INDEX_NEW) {
                        untracked += 1;
                    } else {
                        changed += 1;
                    }
                } else {
                    changed += 1;
                }
            }
            (changed, untracked)
        }
        Err(_) => (0, 0),
    };

    // Get ahead/behind counts
    let (ahead, behind) = get_ahead_behind(&repo, &current_branch);

    GitInfo {
        is_repo: true,
        current_branch,
        ahead,
        behind,
        changed_files,
        untracked_files,
    }
}

/// Calculate ahead/behind relative to upstream tracking branch.
fn get_ahead_behind(repo: &Repository, branch_name: &str) -> (i32, i32) {
    let local_branch = match repo.find_branch(branch_name, BranchType::Local) {
        Ok(b) => b,
        Err(_) => return (0, 0),
    };

    let upstream = match local_branch.upstream() {
        Ok(u) => u,
        Err(_) => return (0, 0),
    };

    let local_oid = match local_branch.get().target() {
        Some(o) => o,
        None => return (0, 0),
    };

    let upstream_oid = match upstream.get().target() {
        Some(o) => o,
        None => return (0, 0),
    };

    match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((ahead, behind)) => (ahead as i32, behind as i32),
        Err(_) => (0, 0),
    }
}

// ── List Branches ─────────────────────────────────────────────────────────

pub fn list_branches(path: &str) -> BranchList {
    let default = BranchList {
        current: String::new(),
        local: Vec::new(),
        remote: Vec::new(),
    };

    let repo = match Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return default,
    };

    let current = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_default();

    let mut local = Vec::new();
    let mut remote = Vec::new();

    if let Ok(branches) = repo.branches(None) {
        for branch_result in branches {
            if let Ok((branch, branch_type)) = branch_result {
                let name = branch
                    .name()
                    .ok()
                    .flatten()
                    .unwrap_or("")
                    .to_string();

                if name.is_empty() {
                    continue;
                }

                match branch_type {
                    BranchType::Local => local.push(name),
                    BranchType::Remote => {
                        // Skip HEAD pointers
                        if !name.ends_with("/HEAD") {
                            remote.push(name);
                        }
                    }
                }
            }
        }
    }

    BranchList {
        current,
        local,
        remote,
    }
}

// ── List Worktrees ────────────────────────────────────────────────────────

pub fn list_worktrees(path: &str) -> Vec<WorktreeInfo> {
    // Use git CLI for worktree list since git2 worktree support is limited
    let output = match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(path)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in stdout.lines() {
        if let Some(wt_path) = line.strip_prefix("worktree ") {
            current_path = Some(wt_path.to_string());
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            // branch refs/heads/main -> main
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        } else if line == "bare" {
            is_bare = true;
        } else if line.is_empty() {
            if let Some(ref wt_path) = current_path {
                worktrees.push(WorktreeInfo {
                    path: wt_path.clone(),
                    branch: current_branch.take().unwrap_or_else(|| "(detached)".to_string()),
                    is_main: worktrees.is_empty(),
                    is_bare,
                });
            }
            current_path = None;
            current_branch = None;
            is_bare = false;
        }
    }

    // Handle last entry if no trailing newline
    if let Some(wt_path) = current_path {
        worktrees.push(WorktreeInfo {
            path: wt_path,
            branch: current_branch.unwrap_or_else(|| "(detached)".to_string()),
            is_main: worktrees.is_empty(),
            is_bare,
        });
    }

    worktrees
}

// ── Create Worktree ───────────────────────────────────────────────────────

pub fn create_worktree(
    project_path: &str,
    branch_name: &str,
) -> Result<WorktreeCreateResult, String> {
    let worktrees_dir = Path::new(project_path).join(".worktrees");

    // Create worktrees directory if needed
    if !worktrees_dir.exists() {
        std::fs::create_dir_all(&worktrees_dir)
            .map_err(|e| format!("Failed to create worktrees directory: {}", e))?;
    }

    // Auto-add .worktrees/ to .gitignore
    let gitignore_path = Path::new(project_path).join(".gitignore");
    let gitignore_entry = ".worktrees/";

    let needs_gitignore_update = if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
        !content.lines().any(|line| line.trim() == gitignore_entry)
    } else {
        true
    };

    if needs_gitignore_update {
        use std::fs::OpenOptions;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .map_err(|e| format!("Failed to update .gitignore: {}", e))?;
        // Add newline before entry if file doesn't end with one
        if gitignore_path.exists() {
            let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
            if !content.is_empty() && !content.ends_with('\n') {
                use std::io::Write;
                writeln!(file).ok();
            }
        }
        use std::io::Write;
        writeln!(file, "{}", gitignore_entry)
            .map_err(|e| format!("Failed to write .gitignore: {}", e))?;
    }

    let safe_branch = branch_name.replace('/', "-");
    let worktree_path = worktrees_dir.join(&safe_branch);
    let wt_path_str = worktree_path.to_string_lossy().to_string();

    // Always create a new branch from HEAD
    let output = Command::new("git")
        .args(["worktree", "add", "-b", branch_name, &wt_path_str, "HEAD"])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("Failed to run git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr));
    }

    Ok(WorktreeCreateResult {
        path: wt_path_str,
        branch: branch_name.to_string(),
    })
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct WorktreeCreateResult {
    pub path: String,
    pub branch: String,
}

// ── Checkout Existing Branch as Worktree ─────────────────────────────

pub fn checkout_worktree(
    project_path: &str,
    branch_name: &str,
) -> Result<WorktreeCreateResult, String> {
    let worktrees_dir = Path::new(project_path).join(".worktrees");

    // Create worktrees directory if needed
    if !worktrees_dir.exists() {
        std::fs::create_dir_all(&worktrees_dir)
            .map_err(|e| format!("Failed to create worktrees directory: {}", e))?;
    }

    // Auto-add .worktrees/ to .gitignore
    let gitignore_path = Path::new(project_path).join(".gitignore");
    let gitignore_entry = ".worktrees/";

    let needs_gitignore_update = if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
        !content.lines().any(|line| line.trim() == gitignore_entry)
    } else {
        true
    };

    if needs_gitignore_update {
        use std::fs::OpenOptions;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .map_err(|e| format!("Failed to update .gitignore: {}", e))?;
        if gitignore_path.exists() {
            let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
            if !content.is_empty() && !content.ends_with('\n') {
                use std::io::Write;
                writeln!(file).ok();
            }
        }
        use std::io::Write;
        writeln!(file, "{}", gitignore_entry)
            .map_err(|e| format!("Failed to write .gitignore: {}", e))?;
    }

    let safe_branch = branch_name.replace('/', "-");
    let worktree_path = worktrees_dir.join(&safe_branch);
    let wt_path_str = worktree_path.to_string_lossy().to_string();

    // Checkout existing branch (no -b flag)
    let output = Command::new("git")
        .args(["worktree", "add", &wt_path_str, branch_name])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("Failed to run git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr));
    }

    Ok(WorktreeCreateResult {
        path: wt_path_str,
        branch: branch_name.to_string(),
    })
}

// ── Remove Worktree ───────────────────────────────────────────────────────

pub fn remove_worktree(
    project_path: &str,
    worktree_path: &str,
    force: bool,
) -> Result<(), String> {
    let wt = Path::new(worktree_path);

    if wt.exists() {
        // Rename to a temp name so the UI path disappears instantly
        let temp_name = format!(
            ".k2so-delete-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let temp_path = wt.parent().unwrap_or(wt).join(&temp_name);

        if std::fs::rename(wt, &temp_path).is_ok() {
            // Prune git metadata (detach the worktree ref)
            let _ = Command::new("git")
                .args(["worktree", "prune"])
                .current_dir(project_path)
                .output();

            // Move to Trash in background thread (recoverable).
            // 0.37.6: route through the centralized safe_delete
            // helper so every trashing call site goes through one
            // chokepoint we can audit + tweak.
            let trash_path = temp_path.to_path_buf();
            std::thread::spawn(move || {
                let _ = crate::safe_delete::trash(&trash_path);
            });

            return Ok(());
        }
    }

    // **0.37.6:** the rename-to-trash fast path above failed (e.g.
    // cross-volume, permission, lock held). Pre-0.37.6 we fell
    // through to `git worktree remove` here — which **permanently
    // deletes** the worktree directory with no recycle bin. That's
    // the path that destroyed a user's bouncy-blobs game in 0.37.5
    // and is exactly what the user directive forbids: "deletion
    // systems should only be sending things to the recycle bin.
    // Never full-on deletion."
    //
    // New flow: skip `git worktree remove` entirely. Send the
    // worktree directory to the OS recycle bin via the safe-delete
    // helper, then `git worktree prune` to clean up the dangling
    // ref. Recoverable: user can drag the worktree back from Trash
    // if the deletion was a mistake.
    //
    // If the trash itself fails (no trash service / permission),
    // we surface the error and STOP — better to leave the worktree
    // in place and let the user investigate than to permanently
    // destroy their work.
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(project_path)
        .output();
    if Path::new(worktree_path).exists() {
        crate::safe_delete::trash(worktree_path).map_err(|e| {
            format!(
                "worktree trash failed: {} (worktree NOT removed; the dir is intact at {})",
                e, worktree_path
            )
        })?;
    }
    let _ = force; // explicitly unused in trash path; kept on signature for back-compat

    Ok(())
}

// ── Get Changed Files ─────────────────────────────────────────────────────

pub fn get_changed_files(path: &str) -> Vec<ChangedFile> {
    let repo = match Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    // Don't recurse into untracked directories — same rationale as
    // `get_git_info` above. Walking inside node_modules/, target/,
    // etc. produces hundreds of thousands of irrelevant entries and
    // burns CPU. Without recurse, libgit2 reports the untracked dir
    // as a single entry (e.g. `node_modules/`) which is what every
    // git GUI / `git status` CLI does by default.
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);

    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut files = Vec::new();

    for entry in statuses.iter() {
        let file_path = entry.path().unwrap_or("").to_string();
        let s = entry.status();

        // Determine if the change is staged (in index) vs unstaged (working tree)
        let is_staged = s.intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED
                | git2::Status::INDEX_TYPECHANGE,
        );

        let status = if s.contains(git2::Status::WT_NEW) {
            "untracked"
        } else if s.contains(git2::Status::INDEX_NEW) {
            "added"
        } else if s.contains(git2::Status::WT_DELETED) || s.contains(git2::Status::INDEX_DELETED) {
            "deleted"
        } else {
            "modified"
        };

        files.push(ChangedFile {
            path: file_path,
            status: status.to_string(),
            staged: is_staged,
        });
    }

    files
}

// ── Diff Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct DiffLine {
    pub kind: String, // "add", "remove", "context"
    pub content: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffSummary {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    pub old_path: Option<String>,
}

// ── Diff Functions ───────────────────────────────────────────────────────

/// Get unified diff hunks for a single file (working tree vs HEAD).
pub fn diff_file(repo_path: &str, file_path: &str) -> Result<Vec<DiffHunk>, String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    // Make file_path relative to the repo workdir for pathspec matching
    let rel_path = if let Some(workdir) = repo.workdir() {
        let abs = std::path::Path::new(file_path);
        abs.strip_prefix(workdir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file_path.to_string())
    } else {
        file_path.to_string()
    };

    let mut opts = DiffOptions::new();
    opts.pathspec(&rel_path);
    opts.context_lines(3);

    let head_tree = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_tree().ok());

    let diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| format!("Failed to compute diff: {e}"))?;

    collect_hunks(&diff)
}

/// Get a summary of all changed files (working tree vs HEAD).
pub fn diff_summary(repo_path: &str) -> Result<Vec<FileDiffSummary>, String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    let mut opts = DiffOptions::new();
    opts.context_lines(0);

    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

    let diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| format!("Failed to compute diff: {e}"))?;

    collect_summary(&diff)
}

/// Get a diff summary between two branches (for merge preview).
pub fn diff_between_branches(
    repo_path: &str,
    base_branch: &str,
    head_branch: &str,
) -> Result<Vec<FileDiffSummary>, String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    let base_tree = resolve_branch_tree(&repo, base_branch)?;
    let head_tree = resolve_branch_tree(&repo, head_branch)?;

    let mut opts = DiffOptions::new();
    opts.context_lines(0);

    let diff = repo
        .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut opts))
        .map_err(|e| format!("Failed to compute branch diff: {e}"))?;

    collect_summary(&diff)
}

/// Get the content of a file at a specific git reference (branch, commit, HEAD, etc.).
pub fn file_content_at_ref(repo_path: &str, file_path: &str, git_ref: &str) -> Result<String, String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    let obj = repo
        .revparse_single(git_ref)
        .map_err(|e| format!("Cannot resolve ref '{git_ref}': {e}"))?;

    let commit = obj
        .peel_to_commit()
        .map_err(|e| format!("Not a commit: {e}"))?;

    let tree = commit.tree().map_err(|e| format!("Failed to get tree: {e}"))?;

    let entry = tree
        .get_path(Path::new(file_path))
        .map_err(|_| format!("File '{file_path}' not found at '{git_ref}'"))?;

    let blob = repo
        .find_blob(entry.id())
        .map_err(|e| format!("Failed to read blob: {e}"))?;

    let content = std::str::from_utf8(blob.content())
        .map_err(|_| "File is binary".to_string())?;

    Ok(content.to_string())
}

/// Helper: resolve a branch name to its tree.
fn resolve_branch_tree<'a>(repo: &'a Repository, branch: &str) -> Result<git2::Tree<'a>, String> {
    let reference = repo
        .find_branch(branch, BranchType::Local)
        .map_err(|e| format!("Branch '{branch}' not found: {e}"))?;

    let commit = reference
        .get()
        .peel_to_commit()
        .map_err(|e| format!("Failed to get commit for branch '{branch}': {e}"))?;

    commit.tree().map_err(|e| format!("Failed to get tree: {e}"))
}

/// Helper: collect DiffHunks from a git2::Diff.
fn collect_hunks(diff: &git2::Diff) -> Result<Vec<DiffHunk>, String> {
    let mut hunks: Vec<DiffHunk> = Vec::new();

    diff.print(git2::DiffFormat::Patch, |_delta, hunk, line| {
        if let Some(h) = hunk {
            // Check if we need a new hunk
            let need_new = hunks.last().map_or(true, |last: &DiffHunk| {
                last.old_start != h.old_start() || last.new_start != h.new_start()
            });

            if need_new {
                hunks.push(DiffHunk {
                    old_start: h.old_start(),
                    old_count: h.old_lines(),
                    new_start: h.new_start(),
                    new_count: h.new_lines(),
                    lines: Vec::new(),
                });
            }
        }

        if let Some(current_hunk) = hunks.last_mut() {
            let kind = match line.origin() {
                '+' => "add",
                '-' => "remove",
                ' ' => "context",
                _ => return true,
            };

            let content = String::from_utf8_lossy(line.content()).to_string();
            current_hunk.lines.push(DiffLine {
                kind: kind.to_string(),
                content,
            });
        }

        true
    })
    .map_err(|e| format!("Failed to print diff: {e}"))?;

    Ok(hunks)
}

/// Helper: collect FileDiffSummary from a git2::Diff.
fn collect_summary(diff: &git2::Diff) -> Result<Vec<FileDiffSummary>, String> {
    let stats = diff.stats().map_err(|e| format!("Failed to get diff stats: {e}"))?;
    let _ = stats; // stats gives totals, we need per-file

    let mut summaries: Vec<FileDiffSummary> = Vec::new();

    for i in 0..diff.deltas().len() {
        let Some(delta) = diff.get_delta(i) else { continue };
        let new_file = delta.new_file();
        let old_file = delta.old_file();

        let path = new_file
            .path()
            .or_else(|| old_file.path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let status = match delta.status() {
            git2::Delta::Added => "added",
            git2::Delta::Deleted => "deleted",
            git2::Delta::Modified => "modified",
            git2::Delta::Renamed => "renamed",
            git2::Delta::Copied => "copied",
            git2::Delta::Conflicted => "conflicted",
            _ => "modified",
        };

        let old_path = if delta.status() == git2::Delta::Renamed {
            old_file.path().map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };

        // Count additions/deletions for this file
        let mut additions = 0u32;
        let mut deletions = 0u32;
        if let Ok(patch) = git2::Patch::from_diff(diff, i) {
            if let Some(patch) = patch {
                let (_, adds, dels) = patch.line_stats().unwrap_or((0, 0, 0));
                additions = adds as u32;
                deletions = dels as u32;
            }
        }

        summaries.push(FileDiffSummary {
            path,
            status: status.to_string(),
            additions,
            deletions,
            old_path,
        });
    }

    Ok(summaries)
}

// ── Index lock retry helper ──────────────────────────────────────────────

/// Maximum retries for index operations when the index is locked by another process.
const INDEX_LOCK_RETRIES: u32 = 5;
const INDEX_LOCK_DELAY_MS: u64 = 50;

/// Check if a git2 error is an index lock contention error.
fn is_index_locked(err: &git2::Error) -> bool {
    let msg = err.message();
    msg.contains("index.lock") || msg.contains("is locked") || err.code() == git2::ErrorCode::Locked
}

/// Retry getting the repository index with lock contention handling.
fn get_index_with_retry(repo: &Repository) -> Result<git2::Index, String> {
    for attempt in 0..INDEX_LOCK_RETRIES {
        match repo.index() {
            Ok(idx) => return Ok(idx),
            Err(e) if is_index_locked(&e) && attempt < INDEX_LOCK_RETRIES - 1 => {
                std::thread::sleep(std::time::Duration::from_millis(INDEX_LOCK_DELAY_MS));
            }
            Err(e) => return Err(format!("Failed to get index: {e}")),
        }
    }
    Err("Index locked after retries".to_string())
}

/// Retry writing the index with lock contention handling.
fn write_index_with_retry(index: &mut git2::Index) -> Result<(), String> {
    for attempt in 0..INDEX_LOCK_RETRIES {
        match index.write() {
            Ok(()) => return Ok(()),
            Err(e) if is_index_locked(&e) && attempt < INDEX_LOCK_RETRIES - 1 => {
                std::thread::sleep(std::time::Duration::from_millis(INDEX_LOCK_DELAY_MS));
            }
            Err(e) => return Err(format!("Failed to write index: {e}")),
        }
    }
    Err("Index locked after retries".to_string())
}

// ── Staging Functions ────────────────────────────────────────────────────

/// Stage a file (git add).
pub fn stage_file(repo_path: &str, file_path: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;
    let mut index = get_index_with_retry(&repo)?;

    let full_path = Path::new(repo_path).join(file_path);
    if full_path.exists() {
        index.add_path(Path::new(file_path)).map_err(|e| format!("Failed to stage file: {e}"))?;
    } else {
        index.remove_path(Path::new(file_path)).map_err(|e| format!("Failed to stage deletion: {e}"))?;
    }

    write_index_with_retry(&mut index)
}

/// Unstage a file (git reset HEAD -- file).
pub fn unstage_file(repo_path: &str, file_path: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    let head = repo.head().map_err(|e| format!("Failed to get HEAD: {e}"))?;
    let head_commit = head.peel_to_commit().map_err(|e| format!("Failed to get HEAD commit: {e}"))?;
    let head_tree = head_commit.tree().map_err(|e| format!("Failed to get HEAD tree: {e}"))?;

    let mut index = get_index_with_retry(&repo)?;

    // Check if the file exists in HEAD
    match head_tree.get_path(Path::new(file_path)) {
        Ok(entry) => {
            // File exists in HEAD — restore index entry to HEAD version
            let blob = repo.find_blob(entry.id()).map_err(|e| format!("Failed to find blob: {e}"))?;
            let index_entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: entry.filemode() as u32,
                uid: 0,
                gid: 0,
                file_size: blob.size() as u32,
                id: entry.id(),
                flags: 0,
                flags_extended: 0,
                path: file_path.as_bytes().to_vec(),
            };
            index.add(&index_entry).map_err(|e| format!("Failed to unstage: {e}"))?;
        }
        Err(_) => {
            // File doesn't exist in HEAD — remove from index
            index.remove_path(Path::new(file_path)).map_err(|e| format!("Failed to unstage: {e}"))?;
        }
    }

    write_index_with_retry(&mut index)
}

/// Stage all changes (git add -A).
pub fn stage_all(repo_path: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;
    let mut index = get_index_with_retry(&repo)?;

    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("Failed to stage all: {e}"))?;

    // Also remove deleted files from index
    index
        .update_all(["*"].iter(), None)
        .map_err(|e| format!("Failed to update index: {e}"))?;

    write_index_with_retry(&mut index)
}

// ── Commit Function ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    pub oid: String,
    pub message: String,
}

/// Create a commit from the current index.
pub fn commit(repo_path: &str, message: &str) -> Result<CommitResult, String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    let sig = repo.signature().map_err(|e| format!("Failed to get signature (set git user.name and user.email): {e}"))?;

    let mut index = get_index_with_retry(&repo)?;
    let tree_oid = index.write_tree().map_err(|e| format!("Failed to write tree: {e}"))?;
    let tree = repo.find_tree(tree_oid).map_err(|e| format!("Failed to find tree: {e}"))?;

    let head = repo.head().map_err(|e| format!("Failed to get HEAD: {e}"))?;
    let parent = head.peel_to_commit().map_err(|e| format!("Failed to get parent commit: {e}"))?;

    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
        .map_err(|e| format!("Failed to create commit: {e}"))?;

    Ok(CommitResult {
        oid: oid.to_string(),
        message: message.to_string(),
    })
}

// ── Merge Functions ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub success: bool,
    pub conflicts: Vec<String>,
    pub merged_files: u32,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeStatus {
    pub in_progress: bool,
    pub conflicts: Vec<String>,
}

/// Merge a branch into the current HEAD (used for merging worktree branches into main).
pub fn merge_branch(repo_path: &str, branch_name: &str) -> Result<MergeResult, String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    // Validate repository state — reject if another merge/rebase is already in progress
    match repo.state() {
        git2::RepositoryState::Clean => {}
        git2::RepositoryState::Merge => {
            return Err("A merge is already in progress. Resolve conflicts or abort the current merge first.".to_string());
        }
        git2::RepositoryState::Rebase
        | git2::RepositoryState::RebaseInteractive
        | git2::RepositoryState::RebaseMerge => {
            return Err("A rebase is in progress. Complete or abort the rebase first.".to_string());
        }
        git2::RepositoryState::CherryPick => {
            return Err("A cherry-pick is in progress. Complete or abort it first.".to_string());
        }
        other => {
            return Err(format!("Repository is in an unsupported state: {:?}", other));
        }
    }

    let branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(|e| format!("Branch '{branch_name}' not found: {e}"))?;

    let annotated = repo
        .reference_to_annotated_commit(branch.get())
        .map_err(|e| format!("Failed to get annotated commit: {e}"))?;

    let (analysis, _) = repo
        .merge_analysis(&[&annotated])
        .map_err(|e| format!("Merge analysis failed: {e}"))?;

    if analysis.is_up_to_date() {
        return Ok(MergeResult {
            success: true,
            conflicts: Vec::new(),
            merged_files: 0,
        });
    }

    if analysis.is_fast_forward() {
        // Fast-forward: just move HEAD
        let target_oid = annotated.id();
        let mut reference = repo.head().map_err(|e| format!("Failed to get HEAD: {e}"))?;
        reference
            .set_target(target_oid, &format!("Fast-forward merge {branch_name}"))
            .map_err(|e| format!("Failed to fast-forward: {e}"))?;

        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .map_err(|e| format!("Failed to checkout after fast-forward: {e}"))?;

        return Ok(MergeResult {
            success: true,
            conflicts: Vec::new(),
            merged_files: 0,
        });
    }

    // Normal merge
    repo.merge(&[&annotated], None, None)
        .map_err(|e| format!("Merge failed: {e}"))?;

    // Check for conflicts
    let index = repo.index().map_err(|e| format!("Failed to get index: {e}"))?;
    let conflicts: Vec<String> = index
        .conflicts()
        .map_err(|e| format!("Failed to check conflicts: {e}"))?
        .filter_map(|c| c.ok())
        .filter_map(|c| {
            c.our
                .as_ref()
                .or(c.their.as_ref())
                .and_then(|entry| String::from_utf8(entry.path.clone()).ok())
        })
        .collect();

    if conflicts.is_empty() {
        // Auto-merge succeeded — create merge commit
        let sig = repo.signature().map_err(|e| format!("Failed to get signature: {e}"))?;
        let mut index = repo.index().map_err(|e| format!("Failed to get index: {e}"))?;
        let tree_oid = index.write_tree().map_err(|e| format!("Failed to write tree: {e}"))?;
        let tree = repo.find_tree(tree_oid).map_err(|e| format!("Failed to find tree: {e}"))?;

        let head_commit = repo.head()
            .map_err(|e| format!("Failed to get HEAD: {e}"))?
            .peel_to_commit()
            .map_err(|e| format!("Failed to get HEAD commit: {e}"))?;
        let branch_commit = repo.find_commit(annotated.id())
            .map_err(|e| format!("Failed to find branch commit: {e}"))?;

        let msg = format!("Merge branch '{branch_name}'");
        repo.commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&head_commit, &branch_commit])
            .map_err(|e| format!("Failed to create merge commit: {e}"))?;

        repo.cleanup_state().map_err(|e| format!("Failed to cleanup merge state: {e}"))?;

        let summary = diff_summary(repo_path).unwrap_or_default();
        Ok(MergeResult {
            success: true,
            conflicts: Vec::new(),
            merged_files: summary.len() as u32,
        })
    } else {
        Ok(MergeResult {
            success: false,
            conflicts,
            merged_files: 0,
        })
    }
}

/// Check if a merge is currently in progress.
pub fn merge_status(repo_path: &str) -> Result<MergeStatus, String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    let in_progress = repo.state() == git2::RepositoryState::Merge;

    let conflicts = if in_progress {
        let index = repo.index().map_err(|e| format!("Failed to get index: {e}"))?;
        let conflict_paths: Vec<String> = index
            .conflicts()
            .map_err(|e| format!("Failed to check conflicts: {e}"))?
            .filter_map(|c| c.ok())
            .filter_map(|c| {
                c.our
                    .as_ref()
                    .or(c.their.as_ref())
                    .and_then(|entry| String::from_utf8(entry.path.clone()).ok())
            })
            .collect();
        conflict_paths
    } else {
        Vec::new()
    };

    Ok(MergeStatus {
        in_progress,
        conflicts,
    })
}

/// Abort an in-progress merge.
pub fn abort_merge(repo_path: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
        .map_err(|e| format!("Failed to reset working tree: {e}"))?;

    repo.cleanup_state().map_err(|e| format!("Failed to cleanup merge state: {e}"))
}

/// Resolve a conflict by choosing "ours" or "theirs".
pub fn resolve_conflict(repo_path: &str, file_path: &str, resolution: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;
    let workdir = repo.workdir().ok_or("No working directory")?;

    match resolution {
        "ours" => {
            // Checkout our version
            let mut cb = git2::build::CheckoutBuilder::new();
            cb.path(file_path).force().use_ours(true);
            repo.checkout_index(None, Some(&mut cb))
                .map_err(|e| format!("Failed to checkout 'ours': {e}"))?;
        }
        "theirs" => {
            // Checkout their version
            let mut cb = git2::build::CheckoutBuilder::new();
            cb.path(file_path).force().use_theirs(true);
            repo.checkout_index(None, Some(&mut cb))
                .map_err(|e| format!("Failed to checkout 'theirs': {e}"))?;
        }
        _ => return Err(format!("Invalid resolution: {resolution} (use 'ours' or 'theirs')")),
    }

    // Stage the resolved file
    stage_file(workdir.to_string_lossy().as_ref(), file_path)
}

/// Delete a local branch.
pub fn delete_branch(repo_path: &str, branch_name: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;

    let mut branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(|e| format!("Branch '{branch_name}' not found: {e}"))?;

    branch.delete().map_err(|e| format!("Failed to delete branch '{branch_name}': {e}"))
}

/// Collect a JSON snapshot of the repo's current state for the AI
/// commit workflow. Runs a few tiny `git` subprocesses with a 5-second
/// per-command timeout — libgit2 would need 5 separate code paths for
/// the same info, and the AI-commit terminal wants the raw CLI output
/// anyway. Returns an empty object-fields if `project_path` isn't a
/// repo or if a command times out.
pub fn gather_git_context(project_path: &str) -> serde_json::Value {
    use std::io::Read;

    let run = |args: &[&str]| -> String {
        let mut child = match Command::new("git")
            .args(args)
            .current_dir(project_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return String::new(),
        };

        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return String::new();
                    }
                    return child
                        .stdout
                        .take()
                        .and_then(|mut out| {
                            let mut buf = String::new();
                            out.read_to_string(&mut buf).ok()?;
                            Some(buf.trim().to_string())
                        })
                        .unwrap_or_default();
                }
                Ok(None) => {
                    if start.elapsed() > std::time::Duration::from_secs(5) {
                        let _ = child.kill();
                        return String::new();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return String::new(),
            }
        }
    };

    serde_json::json!({
        "branch": run(&["rev-parse", "--abbrev-ref", "HEAD"]),
        "status": run(&["status", "--short"]),
        "diffStat": run(&["diff", "--stat"]),
        "stagedStat": run(&["diff", "--cached", "--stat"]),
        "recentLog": run(&["log", "--oneline", "-5"]),
    })
}
