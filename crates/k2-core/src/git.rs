use git2::{Repository, StatusOptions, BranchType};
use serde::Serialize;

use crate::gitx::diff::{ChangeKind, FileChange};
use crate::gitx::status::{self as gstatus, Flags};
use std::path::Path;

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

    let repo = match crate::gitx::discover(path) {
        Ok(r) => r,
        Err(_) => return default,
    };

    // Current branch (`HEAD` when detached). No commit yet: a repo with
    // nothing else to say.
    let Some(current_branch) = crate::gitx::head_shorthand(&repo) else {
        return GitInfo {
            is_repo: true,
            ..default
        };
    };

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
    let opts = gstatus::Options { untracked: gstatus::Untracked::Collapsed, staged_renames: false, submodules: true };
    let (changed_files, untracked_files) = match gstatus::statuses(&repo, opts) {
        Ok(rows) => {
            let mut changed = 0i32;
            let mut untracked = 0i32;
            for r in &rows {
                if r.flags.contains(Flags::WT_NEW) && !r.flags.contains(Flags::INDEX_NEW) {
                    untracked += 1;
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
fn get_ahead_behind(repo: &gix::Repository, branch_name: &str) -> (i32, i32) {
    let Some(local) = crate::gitx::local_branch_tip(repo, branch_name) else { return (0, 0) };
    let Some((_, upstream)) = crate::gitx::upstream_of(repo, branch_name) else { return (0, 0) };
    match crate::gitx::ahead_behind(repo, local, upstream) {
        Some((ahead, behind)) => (ahead as i32, behind as i32),
        None => (0, 0),
    }
}

// ── List Branches ─────────────────────────────────────────────────────────

pub fn list_branches(path: &str) -> BranchList {
    let default = BranchList {
        current: String::new(),
        local: Vec::new(),
        remote: Vec::new(),
    };

    let repo = match crate::gitx::discover(path) {
        Ok(r) => r,
        Err(_) => return default,
    };

    let current = crate::gitx::head_shorthand(&repo).unwrap_or_default();

    let mut local = Vec::new();
    let mut remote = Vec::new();

    if let Ok(refs) = repo.references() {
        if let Ok(it) = refs.local_branches() {
            for r in it.flatten() {
                let name = crate::gitx::short_name(r.name());
                if !name.is_empty() {
                    local.push(name);
                }
            }
        }
        if let Ok(it) = refs.remote_branches() {
            for r in it.flatten() {
                let name = crate::gitx::short_name(r.name());
                // Skip HEAD pointers
                if !name.is_empty() && !name.ends_with("/HEAD") {
                    remote.push(name);
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
    let output = match crate::git_safe::git(path)
        .args(["worktree", "list", "--porcelain"])
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
    create_worktree_from(project_path, branch_name, "HEAD")
}

/// [`create_worktree`] with the new branch cut from `base` (a ref or a
/// commit the caller has resolved; Workbench W1 passes the base commit,
/// never whatever HEAD happens to be). `base` must not start with `-`.
pub fn create_worktree_from(
    project_path: &str,
    branch_name: &str,
    base: &str,
) -> Result<WorktreeCreateResult, String> {
    if base.is_empty() || base.starts_with('-') || branch_name.is_empty() || branch_name.starts_with('-') {
        return Err(format!("git worktree add refused: bad branch '{branch_name}' or base '{base}'"));
    }
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

    // Always create a new branch, from `base` (HEAD for the legacy route).
    let output = crate::git_safe::git(project_path)
        .args(["worktree", "add", "-b", branch_name, &wt_path_str, base])
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
    let output = crate::git_safe::git(project_path)
        .args(["worktree", "add", &wt_path_str, branch_name])
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

/// Refusal codes that [`remove_worktree`] and [`merge_branch`] put at the
/// start of their error text (`"<code>: <hint>"`); [`refusal::code_of`]
/// reads them back so the daemon can answer with a status and a `code`.
pub mod refusal {
    /// `project_path` is not inside a git repository.
    pub const NOT_A_REPO: &str = "not_a_repo";
    /// The folder to remove is not a linked worktree of the project's
    /// repository (any other folder, the main worktree, the project itself).
    pub const NOT_A_WORKTREE_OF_PROJECT: &str = "not_a_worktree_of_project";
    /// A fast-forward would overwrite uncommitted changes on the target.
    pub const MAIN_HAS_UNCOMMITTED_CHANGES: &str = "main_has_uncommitted_changes";
    /// Abort asked for, but the repository has no merge in progress.
    pub const NO_MERGE_IN_PROGRESS: &str = "no_merge_in_progress";
    /// Resolve asked for a file that is not in conflict.
    pub const NOT_IN_CONFLICT: &str = "not_in_conflict";

    const ALL: [&str; 5] =
        [NOT_A_REPO, NOT_A_WORKTREE_OF_PROJECT, MAIN_HAS_UNCOMMITTED_CHANGES, NO_MERGE_IN_PROGRESS, NOT_IN_CONFLICT];

    /// The refusal code an error from this module starts with, if any.
    pub fn code_of(err: &str) -> Option<&'static str> {
        ALL.into_iter().find(|c| err.strip_prefix(c).is_some_and(|rest| rest.starts_with(':')))
    }
}

/// `worktree_path`, canonical, when it is a linked worktree of the
/// repository that holds `project_path` (listed by `git worktree list`,
/// not the main worktree, not the project folder itself). `Ok(None)` when
/// nothing is at `worktree_path` (nothing to remove; only a prune is
/// left). Anything else is refused: this path only ever trashes a worktree.
fn checked_linked_worktree(project_path: &str, worktree_path: &str) -> Result<Option<std::path::PathBuf>, String> {
    let project = dunce::canonicalize(project_path)
        .map_err(|e| format!("{}: {project_path} is not a folder on this machine ({e})", refusal::NOT_A_REPO))?;
    let listed = list_worktrees(&project.to_string_lossy());
    if listed.is_empty() {
        return Err(format!("{}: {} is not a git repository", refusal::NOT_A_REPO, project.display()));
    }
    // Truly missing (not even a dangling link): nothing to trash.
    if std::fs::symlink_metadata(worktree_path).is_err() {
        return Ok(None);
    }
    let not_ours = |why: &str| {
        format!(
            "{}: {worktree_path} {why}; only a linked worktree of {} can be removed",
            refusal::NOT_A_WORKTREE_OF_PROJECT,
            project.display()
        )
    };
    let wt = dunce::canonicalize(worktree_path).map_err(|_| not_ours("can't be resolved"))?;
    if wt == project {
        return Err(not_ours("is the workspace itself"));
    }
    let entry = listed
        .iter()
        .find(|w| dunce::canonicalize(&w.path).is_ok_and(|p| p == wt))
        .ok_or_else(|| not_ours("is not a worktree of this repository"))?;
    if entry.is_main || entry.is_bare {
        return Err(not_ours("is the repository's main worktree"));
    }
    Ok(Some(wt))
}

pub fn remove_worktree(
    project_path: &str,
    worktree_path: &str,
    force: bool,
) -> Result<(), String> {
    // Only a linked worktree of this project's repository, by its real
    // (canonical) path; anything else is refused before any rename/trash.
    let checked = checked_linked_worktree(project_path, worktree_path)?;
    let wt_buf = checked.clone().unwrap_or_else(|| std::path::PathBuf::from(worktree_path));
    let wt = wt_buf.as_path();

    if checked.is_some() && wt.exists() {
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
            let _ = crate::git_safe::git(project_path)
                .args(["worktree", "prune"])
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
    let _ = crate::git_safe::git(project_path)
        .args(["worktree", "prune"])
        .output();
    if checked.is_some() && wt.exists() {
        crate::safe_delete::trash(wt).map_err(|e| {
            format!(
                "worktree trash failed: {} (worktree NOT removed; the dir is intact at {})",
                e,
                wt.display()
            )
        })?;
    }
    let _ = force; // explicitly unused in trash path; kept on signature for back-compat

    Ok(())
}

// ── Get Changed Files ─────────────────────────────────────────────────────

pub fn get_changed_files(path: &str) -> Vec<ChangedFile> {
    let repo = match crate::gitx::discover(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    // Don't recurse into untracked directories — same rationale as
    // `get_git_info` above. Walking inside node_modules/, target/,
    // etc. produces hundreds of thousands of irrelevant entries and
    // burns CPU. Collapsed, the untracked dir is a single entry (e.g.
    // `node_modules/`), which is what `git status` shows by default.
    let opts = gstatus::Options { untracked: gstatus::Untracked::Collapsed, staged_renames: false, submodules: true };
    let rows = match gstatus::statuses(&repo, opts) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut files = Vec::new();

    for r in rows {
        let file_path = r.path;
        let s = r.flags;

        // Determine if the change is staged (in index) vs unstaged (working tree)
        let is_staged = s.intersects(Flags::INDEX_ANY);

        let status = if s.contains(Flags::WT_NEW) {
            "untracked"
        } else if s.contains(Flags::INDEX_NEW) {
            "added"
        } else if s.contains(Flags::WT_DELETED) || s.contains(Flags::INDEX_DELETED) {
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

/// Largest side a summary or hunk diff reads (past it: no counts).
const DIFF_READ_CAP: u64 = 64 * 1024 * 1024;

fn discover_or_err(repo_path: &str) -> Result<gix::Repository, String> {
    crate::gitx::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))
}

/// Get unified diff hunks for a single file (working tree vs HEAD). A
/// folder path gives the hunks of every changed file under it.
pub fn diff_file(repo_path: &str, file_path: &str) -> Result<Vec<DiffHunk>, String> {
    let repo = discover_or_err(repo_path)?;

    // Make file_path relative to the repo workdir for path matching
    let rel_path = if let Some(workdir) = repo.workdir() {
        let abs = std::path::Path::new(file_path);
        abs.strip_prefix(workdir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file_path.to_string())
    } else {
        file_path.to_string()
    };
    let rel_path = rel_path.replace('\\', "/");
    let rel_path = rel_path.trim_end_matches('/');
    let under = format!("{rel_path}/");

    let changes = crate::gitx::diff::tree_to_worktree(&repo, crate::gitx::head_tree_id(&repo), false)
        .map_err(|e| format!("Failed to compute diff: {e}"))?;
    let mut reader = crate::gitx::diff::WorktreeReader::new(&repo).ok_or("No working directory")?;
    let mut hunks = Vec::new();
    for c in changes.iter().filter(|c| c.path == rel_path || c.path.starts_with(&under)) {
        let old_path = c.old_path.as_deref().unwrap_or(&c.path);
        let a = crate::gitx::diff::side_bytes(&repo, &mut reader, old_path, &c.old, DIFF_READ_CAP);
        let b = crate::gitx::diff::side_bytes(&repo, &mut reader, &c.path, &c.new, DIFF_READ_CAP);
        let (Some(a), Some(b)) = (a, b) else { continue };
        if crate::gitx::diff::is_binary(&a) || crate::gitx::diff::is_binary(&b) {
            continue; // "Binary files differ": no hunks, as libgit2 printed none
        }
        hunks.extend(crate::gitx::diff::hunks(&a, &b, 3).into_iter().map(to_diff_hunk));
    }
    Ok(hunks)
}

fn to_diff_hunk(h: crate::gitx::diff::Hunk) -> DiffHunk {
    use crate::gitx::diff::LineKind;
    DiffHunk {
        old_start: h.old_start,
        old_count: h.old_count,
        new_start: h.new_start,
        new_count: h.new_count,
        lines: h
            .lines
            .into_iter()
            .map(|l| DiffLine {
                kind: match l.kind {
                    LineKind::Add => "add",
                    LineKind::Remove => "remove",
                    LineKind::Context => "context",
                }
                .to_string(),
                content: String::from_utf8_lossy(&l.content).to_string(),
            })
            .collect(),
    }
}

/// Get a summary of all changed files (working tree vs HEAD).
pub fn diff_summary(repo_path: &str) -> Result<Vec<FileDiffSummary>, String> {
    let repo = discover_or_err(repo_path)?;
    let changes = crate::gitx::diff::tree_to_worktree(&repo, crate::gitx::head_tree_id(&repo), false)
        .map_err(|e| format!("Failed to compute diff: {e}"))?;
    collect_summary(&repo, &changes)
}

/// Get a diff summary between two branches (for merge preview).
pub fn diff_between_branches(
    repo_path: &str,
    base_branch: &str,
    head_branch: &str,
) -> Result<Vec<FileDiffSummary>, String> {
    let repo = discover_or_err(repo_path)?;

    let base_tree = resolve_branch_tree(&repo, base_branch)?;
    let head_tree = resolve_branch_tree(&repo, head_branch)?;

    let changes = crate::gitx::diff::tree_to_tree(&repo, Some(base_tree), Some(head_tree))
        .map_err(|e| format!("Failed to compute branch diff: {e}"))?;
    collect_summary(&repo, &changes)
}

/// Refuse a caller's revision spec that searches commit messages:
/// `:/text` or `<rev>^{/text}`. Every place that resolves a caller's
/// ref, base or sha calls this right before handing it to libgit2 or
/// `git rev-parse`. The whole string is checked, not only its start, so
/// `main:/x` and `HEAD~1^{/fix}` are refused too. `<rev>:<path>` (a colon
/// with no slash right after it) is still allowed.
pub fn refuse_message_search_revspec(spec: &str) -> Result<(), String> {
    if spec.contains(":/") || spec.contains("^{/") {
        return Err(
            "revision specs that search commit messages (`:/…`, `^{/…}`) aren't supported here; name a branch, tag or commit"
                .to_string(),
        );
    }
    Ok(())
}

/// Get the content of a file at a specific git reference (branch, commit, HEAD, etc.).
pub fn file_content_at_ref(repo_path: &str, file_path: &str, git_ref: &str) -> Result<String, String> {
    let repo = discover_or_err(repo_path)?;

    refuse_message_search_revspec(git_ref)?;
    let commit = crate::gitx::resolve_commit(&repo, git_ref).map_err(|e| format!("Cannot resolve ref '{git_ref}': {e}"))?;
    let tree = crate::gitx::commit_tree_id(&repo, commit).ok_or_else(|| "Failed to get tree".to_string())?;
    let rel = file_path.replace('\\', "/");
    let (kind, id) = crate::gitx::tree_entry(&repo, tree, &rel)
        .ok_or_else(|| format!("File '{file_path}' not found at '{git_ref}'"))?;
    if !(kind.is_blob() || kind == crate::gitx::EntryKind::Link) {
        return Err(format!("Failed to read blob: '{file_path}' at '{git_ref}' is not a file"));
    }
    let bytes = crate::gitx::blob_bytes(&repo, id).ok_or_else(|| "Failed to read blob".to_string())?;
    String::from_utf8(bytes).map_err(|_| "File is binary".to_string())
}

/// Helper: resolve a local branch name to its tree.
fn resolve_branch_tree(repo: &gix::Repository, branch: &str) -> Result<gix::ObjectId, String> {
    let tip = crate::gitx::local_branch_tip(repo, branch).ok_or_else(|| format!("Branch '{branch}' not found"))?;
    crate::gitx::commit_tree_id(repo, tip).ok_or_else(|| format!("Failed to get commit for branch '{branch}'"))
}

/// Helper: one FileDiffSummary per change (binary or past the read cap:
/// 0/0, as libgit2's line stats gave).
fn collect_summary(repo: &gix::Repository, changes: &[FileChange]) -> Result<Vec<FileDiffSummary>, String> {
    let mut reader = crate::gitx::diff::WorktreeReader::new(repo);
    let mut summaries: Vec<FileDiffSummary> = Vec::new();
    for c in changes {
        let status = match c.kind {
            ChangeKind::Added => "added",
            ChangeKind::Deleted => "deleted",
            ChangeKind::Renamed => "renamed",
            ChangeKind::Conflicted => "conflicted",
            ChangeKind::Modified | ChangeKind::Typechange => "modified",
        };
        let (additions, deletions) = match reader.as_mut() {
            Some(r) => match crate::gitx::diff::change_stats(repo, r, c, DIFF_READ_CAP) {
                (Some(a), Some(d), _) => (a, d),
                _ => (0, 0),
            },
            // A bare repository has no work-tree sides: blobs only.
            None => blob_only_stats(repo, c),
        };
        summaries.push(FileDiffSummary {
            path: c.path.clone(),
            status: status.to_string(),
            additions,
            deletions,
            old_path: c.old_path.clone(),
        });
    }
    Ok(summaries)
}

fn blob_only_stats(repo: &gix::Repository, c: &FileChange) -> (u32, u32) {
    use crate::gitx::diff::Side;
    let bytes = |s: &Side| match s {
        Side::Absent => Some(Vec::new()),
        Side::Object { id, .. } => crate::gitx::blob_bytes(repo, *id),
        Side::Worktree { .. } => None,
    };
    match (bytes(&c.old), bytes(&c.new)) {
        (Some(a), Some(b)) if !crate::gitx::diff::is_binary(&a) && !crate::gitx::diff::is_binary(&b) => {
            crate::gitx::diff::line_stats(&a, &b)
        }
        _ => (0, 0),
    }
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
pub(crate) fn get_index_with_retry(repo: &Repository) -> Result<git2::Index, String> {
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
pub(crate) fn write_index_with_retry(index: &mut git2::Index) -> Result<(), String> {
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

/// HEAD names a branch with no commits yet (or no HEAD at all).
fn is_unborn(e: &git2::Error) -> bool {
    matches!(e.code(), git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound)
}

/// `git init` at exactly `path` (libgit2: a `.git` folder, no commit; the
/// first branch follows `init.defaultBranch`, else `main`). The caller
/// checks that no repository already holds `path`.
pub fn init_repo(path: &str) -> Result<(), String> {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.no_reinit(true).mkdir(false).external_template(false);
    let branch = git2::Config::open_default()
        .ok()
        .and_then(|c| c.get_string("init.defaultBranch").ok())
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| "main".to_string());
    opts.initial_head(&branch);
    Repository::init_opts(path, &opts).map(|_| ()).map_err(|e| format!("git init {path}: {e}"))
}

// ── Staging Functions ────────────────────────────────────────────────────

/// Stage a file (git add). A folder (the Changes tab's collapsed untracked
/// `x/` row) is added recursively like `git add x/`: every file under it
/// that .gitignore does not exclude.
pub fn stage_file(repo_path: &str, file_path: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;
    let mut index = get_index_with_retry(&repo)?;

    let rel = file_path.trim_end_matches('/');
    // `file_path` is repo-relative (the routes normalise it); resolve it
    // against the work tree, not the workspace folder (which may be a
    // sub-folder of the repo).
    let workdir = repo.workdir().map(Path::to_path_buf).unwrap_or_else(|| Path::new(repo_path).to_path_buf());
    let dir = workdir.join(rel);
    if !rel.is_empty() && dir.is_dir() && !dir.is_symlink() {
        let prefix = format!("{rel}/");
        let spec = crate::git_changes::pathspec_literal(rel);
        let mut only_under = |p: &Path, _spec: &[u8]| -> i32 {
            // 0 = add, 1 = skip: never anything outside the folder.
            if p.to_str().is_some_and(|s| s.starts_with(&prefix)) { 0 } else { 1 }
        };
        index
            .add_all([spec.as_str()].iter(), git2::IndexAddOption::DEFAULT, Some(&mut only_under))
            .map_err(|e| format!("Failed to stage folder: {e}"))?;
        return write_index_with_retry(&mut index);
    }

    let full_path = workdir.join(file_path);
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

    let head = match repo.head() {
        Ok(h) => h,
        // No commits yet: unstaging = dropping the path from the index.
        Err(e) if is_unborn(&e) => {
            let mut index = get_index_with_retry(&repo)?;
            index.remove_path(Path::new(file_path)).map_err(|e| format!("Failed to unstage file: {e}"))?;
            return write_index_with_retry(&mut index);
        }
        Err(e) => return Err(format!("Failed to get HEAD: {e}")),
    };
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

    // A repo with no commits yet (fresh `git init`, the Changes tab's
    // Initialize repository): the first commit has no parent.
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().map_err(|e| format!("Failed to get parent commit: {e}"))?),
        Err(e) if is_unborn(&e) => None,
        Err(e) => return Err(format!("Failed to get HEAD: {e}")),
    };
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();

    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
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
        // Fast-forward like `git merge --ff-only`: refuse when it would
        // overwrite uncommitted work, update the files with a SAFE
        // checkout (never a forced one), then move HEAD. Uncommitted
        // changes to files the fast-forward doesn't touch stay as they are.
        let target_oid = annotated.id();
        let target_tree = repo
            .find_commit(target_oid)
            .and_then(|c| c.tree())
            .map_err(|e| format!("Failed to read branch '{branch_name}': {e}"))?;
        let blocked = fast_forward_blockers(&repo, &target_tree)?;
        if !blocked.is_empty() {
            return Err(format!(
                "{}: merging '{branch_name}' would overwrite uncommitted changes in {}; commit or stash them first",
                refusal::MAIN_HAS_UNCOMMITTED_CHANGES,
                blocked.join(", ")
            ));
        }
        repo.checkout_tree(target_tree.as_object(), Some(git2::build::CheckoutBuilder::new().safe()))
            .map_err(|e| {
                format!(
                    "{}: merging '{branch_name}' would overwrite uncommitted changes ({e}); commit or stash them first",
                    refusal::MAIN_HAS_UNCOMMITTED_CHANGES
                )
            })?;
        let mut reference = repo.head().map_err(|e| format!("Failed to get HEAD: {e}"))?;
        reference
            .set_target(target_oid, &format!("Fast-forward merge {branch_name}"))
            .map_err(|e| format!("Failed to fast-forward: {e}"))?;

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

/// Paths with uncommitted changes (staged, unstaged or untracked) that a
/// fast-forward from HEAD to `target` would write over, sorted.
fn fast_forward_blockers(repo: &Repository, target: &git2::Tree) -> Result<Vec<String>, String> {
    let head_tree = repo
        .head()
        .and_then(|h| h.peel_to_tree())
        .map_err(|e| format!("Failed to read HEAD: {e}"))?;
    let diff = repo
        .diff_tree_to_tree(Some(&head_tree), Some(target), None)
        .map_err(|e| format!("Failed to diff for fast-forward: {e}"))?;
    let mut touched = std::collections::HashSet::new();
    for d in diff.deltas() {
        for f in [d.old_file(), d.new_file()] {
            if let Some(p) = f.path().and_then(|p| p.to_str()) {
                touched.insert(p.to_string());
            }
        }
    }
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true).include_ignored(false);
    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| format!("Failed to read status: {e}"))?;
    let mut blocked: Vec<String> = statuses
        .iter()
        .filter(|e| e.status() != git2::Status::CURRENT && !e.status().contains(git2::Status::IGNORED))
        .filter_map(|e| e.path().map(str::to_string))
        .filter(|p| touched.contains(p))
        .collect();
    blocked.sort();
    blocked.dedup();
    Ok(blocked)
}

/// Check if a merge is currently in progress.
pub fn merge_status(repo_path: &str) -> Result<MergeStatus, String> {
    let repo = discover_or_err(repo_path)?;

    let in_progress = repo.state() == Some(gix::state::InProgress::Merge);

    let conflicts = if in_progress {
        let index = repo.index_or_empty().map_err(|e| format!("Failed to get index: {e}"))?;
        crate::gitx::conflicted_paths(&index)
    } else {
        Vec::new()
    };

    Ok(MergeStatus {
        in_progress,
        conflicts,
    })
}

/// Abort an in-progress merge. Refused (`no_merge_in_progress`) when the
/// repository has no merge in progress: the reset below would otherwise
/// throw away ordinary uncommitted work.
pub fn abort_merge(repo_path: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;
    if repo.state() != git2::RepositoryState::Merge {
        return Err(format!(
            "{}: there is no merge in progress in {repo_path}; nothing to abort",
            refusal::NO_MERGE_IN_PROGRESS
        ));
    }

    repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
        .map_err(|e| format!("Failed to reset working tree: {e}"))?;

    repo.cleanup_state().map_err(|e| format!("Failed to cleanup merge state: {e}"))
}

/// Resolve a conflict by choosing "ours" or "theirs". `file_path` must be
/// a file in conflict right now (its exact repo-relative path, so a
/// pattern never matches); anything else is refused (`not_in_conflict`)
/// and untouched.
pub fn resolve_conflict(repo_path: &str, file_path: &str, resolution: &str) -> Result<(), String> {
    let repo = Repository::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;
    let workdir = repo.workdir().ok_or("No working directory")?;

    let in_conflict = {
        let index = repo.index().map_err(|e| format!("Failed to get index: {e}"))?;
        let found = index
            .conflicts()
            .map_err(|e| format!("Failed to check conflicts: {e}"))?
            .filter_map(|c| c.ok())
            .any(|c| {
                [c.ancestor, c.our, c.their]
                    .iter()
                    .flatten()
                    .any(|entry| entry.path.as_slice() == file_path.as_bytes())
            });
        found
    };
    if !in_conflict {
        return Err(format!("{}: {file_path} is not in conflict; nothing to resolve", refusal::NOT_IN_CONFLICT));
    }

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
        let mut child = match crate::git_safe::git(project_path)
            .args(args)
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
        "diffStat": run(&["diff", "--no-ext-diff", "--no-textconv", "--stat"]),
        "stagedStat": run(&["diff", "--no-ext-diff", "--no-textconv", "--cached", "--stat"]),
        "recentLog": run(&["log", "--oneline", "-5"]),
    })
}

#[cfg(test)]
#[path = "git_worktree_guard_tests.rs"]
mod worktree_guard_tests;

#[cfg(test)]
#[path = "git_merge_guard_tests.rs"]
mod merge_guard_tests;

#[cfg(test)]
#[path = "git_revspec_guard_tests.rs"]
mod revspec_guard_tests;
