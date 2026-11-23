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
    // makes the status walk into every untracked directory — most
    // notably `node_modules/`, `target/`, `dist/`, `.next/`, etc.
    // — which on a JS/TS workspace with 100k+ files inside
    // node_modules turns each poll into a multi-hundred-thousand
    // stat-and-attribute-load loop. With this hook polled every
    // 5 seconds (`useGit::useGitInfo`), that pegs the Tauri main
    // process at ~200% CPU. Without recurse, the status reports the
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
    // Worktree list stays on the git CLI (it reports prunable worktrees)
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
    /// A write that runs the person's `git` (stage, unstage, commit,
    /// discard, init, branch delete, merge, abort, resolve) while no git
    /// runs on this machine. The hint says how to install it. Reads never
    /// need git.
    pub const GIT_NOT_INSTALLED: &str = "git_not_installed";

    const ALL: [&str; 6] = [
        NOT_A_REPO,
        NOT_A_WORKTREE_OF_PROJECT,
        MAIN_HAS_UNCOMMITTED_CHANGES,
        NO_MERGE_IN_PROGRESS,
        NOT_IN_CONFLICT,
        GIT_NOT_INSTALLED,
    ];

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
/// ref, base or sha calls this right before handing it to gix or
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

// ── Writes: the person's git (0.46.18) ──────────────────────────────────
//
// Every write below runs the installed `git` through
// `git_safe::run_write` (no hooks, no fsmonitor, no signing prompt, no
// editor, literal pathspecs, none of the daemon's own GIT_* variables) and
// gix only for the reads around it. git edits its own index, so the
// cached-tree, split-index and same-second rules are git's. With no git
// installed each one refuses with `git_not_installed` and runs nothing.

/// The work tree of the repository holding `repo_path`.
fn workdir_of(repo_path: &str) -> Result<(gix::Repository, std::path::PathBuf), String> {
    let repo = crate::gitx::discover(repo_path).map_err(|e| format!("Not a git repository: {e}"))?;
    let wd = repo.workdir().map(Path::to_path_buf).ok_or_else(|| format!("Not a git repository: {repo_path} is a bare repository"))?;
    Ok((repo, wd))
}

/// `git init` at exactly `path` (a `.git` folder, no commit; the first
/// branch follows `init.defaultBranch`, else `main`). The caller checks
/// that no repository already holds `path`; a `.git` already there is
/// refused, never re-initialized.
pub fn init_repo(path: &str) -> Result<(), String> {
    let dir = Path::new(path);
    if std::fs::symlink_metadata(dir.join(".git")).is_ok() {
        return Err(format!("git init {path}: already a git repository"));
    }
    crate::git_safe::require_git()?;
    let configured = crate::git_safe::run_write(dir, &["config", "--get", "init.defaultBranch"])?;
    let branch = Some(configured.stdout.trim().to_string()).filter(|b| configured.ok() && !b.is_empty()).unwrap_or_else(|| "main".to_string());
    if !crate::gitx::valid_branch_name(&branch) {
        return Err(format!("git init {path}: init.defaultBranch \"{branch}\" isn't a valid branch name"));
    }
    crate::git_safe::run_write_ok(dir, &format!("git init {path}"), &["init", "-q"])?;
    crate::git_safe::run_write_ok(dir, &format!("git init {path}"), &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])?;
    Ok(())
}

// ── Staging Functions ────────────────────────────────────────────────────

/// Stage a file (git add). A folder (the Changes tab's collapsed untracked
/// `x/` row) is added recursively like `git add x/`: every file under it
/// that .gitignore does not exclude. A path gone from the work tree stages
/// its deletion.
pub fn stage_file(repo_path: &str, file_path: &str) -> Result<(), String> {
    let (_, workdir) = workdir_of(repo_path)?;
    let rel = file_path.trim_end_matches('/');
    if rel.is_empty() {
        return Err("Failed to stage file: empty path".to_string());
    }
    // `file_path` is repo-relative (the routes normalise it); resolve it
    // against the work tree, not the workspace folder (which may be a
    // sub-folder of the repo).
    let full = workdir.join(rel);
    if std::fs::symlink_metadata(&full).is_ok() {
        crate::git_safe::run_write_ok(&workdir, "Failed to stage file", &["add", "--", rel])?;
    } else {
        crate::git_safe::run_write_ok(&workdir, "Failed to stage deletion", &["rm", "-q", "--cached", "--ignore-unmatch", "--", rel])?;
    }
    Ok(())
}

/// Unstage a file (git reset HEAD -- file). With no commit yet, the path
/// just leaves the index.
pub fn unstage_file(repo_path: &str, file_path: &str) -> Result<(), String> {
    let (repo, workdir) = workdir_of(repo_path)?;
    let rel = file_path.trim_end_matches('/');
    if crate::gitx::head_info(&repo).unborn {
        crate::git_safe::run_write_ok(&workdir, "Failed to unstage file", &["rm", "-q", "--cached", "--ignore-unmatch", "-r", "--", rel])?;
    } else {
        crate::git_safe::run_write_ok(&workdir, "Failed to unstage", &["reset", "-q", "--", rel])?;
    }
    Ok(())
}

/// Stage all changes (git add -A).
pub fn stage_all(repo_path: &str) -> Result<(), String> {
    let (_, workdir) = workdir_of(repo_path)?;
    crate::git_safe::run_write_ok(&workdir, "Failed to stage all", &["add", "-A"])?;
    Ok(())
}

// ── Commit Function ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    pub oid: String,
    pub message: String,
}

/// Create a commit from the current index (the message kept as written;
/// an empty index change still commits, as before). The identity comes
/// from git config only.
pub fn commit(repo_path: &str, message: &str) -> Result<CommitResult, String> {
    let (_, workdir) = workdir_of(repo_path)?;
    let o = crate::git_safe::run_write(
        &workdir,
        &["commit", "-q", "--no-verify", "--allow-empty", "--allow-empty-message", "--cleanup=verbatim", "-m", message],
    )?;
    if !o.ok() {
        let said = o.said();
        if said.contains("user.useConfigOnly") || said.contains("tell me who you are") || said.contains("empty ident") {
            return Err(format!("Failed to get signature (set git user.name and user.email): {said}"));
        }
        return Err(format!("Failed to create commit: {said}"));
    }
    let repo = crate::gitx::discover(&workdir).map_err(|e| format!("Not a git repository: {e}"))?;
    let oid = crate::gitx::head_info(&repo).oid.ok_or("Failed to read the new commit")?;
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

/// Merge a branch into the current HEAD (used for merging worktree branches
/// into main). gix reads the state, the branch and the merge base; the
/// person's git does the merge (fast-forward or a merge commit). git2 used
/// to do it in-process; since 0.46.18 it needs git (`git_not_installed`
/// otherwise).
pub fn merge_branch(repo_path: &str, branch_name: &str) -> Result<MergeResult, String> {
    let (repo, workdir) = workdir_of(repo_path)?;

    // Validate repository state — reject if another merge/rebase is already in progress
    use gix::state::InProgress as P;
    match repo.state() {
        None => {}
        Some(P::Merge) => {
            return Err("A merge is already in progress. Resolve conflicts or abort the current merge first.".to_string());
        }
        Some(P::Rebase | P::RebaseInteractive) => {
            return Err("A rebase is in progress. Complete or abort the rebase first.".to_string());
        }
        Some(P::CherryPick | P::CherryPickSequence) => {
            return Err("A cherry-pick is in progress. Complete or abort it first.".to_string());
        }
        Some(other) => {
            return Err(format!("Repository is in an unsupported state: {:?}", other));
        }
    }

    if branch_name.starts_with('-') {
        return Err(format!("Branch '{branch_name}' not found"));
    }
    let target = crate::gitx::local_branch_tip(&repo, branch_name).ok_or_else(|| format!("Branch '{branch_name}' not found"))?;
    let head = crate::gitx::head_info(&repo).oid;
    let base = head.and_then(|h| crate::gitx::merge_base(&repo, h, target));

    if head == Some(target) || base == Some(target) {
        return Ok(MergeResult {
            success: true,
            conflicts: Vec::new(),
            merged_files: 0,
        });
    }
    crate::git_safe::require_git()?;

    if head.is_some() && base == head {
        // Fast-forward like `git merge --ff-only`: refuse when it would
        // overwrite uncommitted work (K2's own check names the files), then
        // git moves HEAD and the files. Uncommitted changes to files the
        // fast-forward doesn't touch stay as they are.
        let blocked = fast_forward_blockers(&repo, target)?;
        if !blocked.is_empty() {
            return Err(format!(
                "{}: merging '{branch_name}' would overwrite uncommitted changes in {}; commit or stash them first",
                refusal::MAIN_HAS_UNCOMMITTED_CHANGES,
                blocked.join(", ")
            ));
        }
        let o = crate::git_safe::run_write(&workdir, &["merge", "-q", "--ff-only", "--no-edit", branch_name])?;
        if !o.ok() {
            let said = o.said();
            if said.contains("overwritten") {
                return Err(format!(
                    "{}: merging '{branch_name}' would overwrite uncommitted changes ({said}); commit or stash them first",
                    refusal::MAIN_HAS_UNCOMMITTED_CHANGES
                ));
            }
            return Err(format!("Failed to fast-forward: {said}"));
        }
        return Ok(MergeResult {
            success: true,
            conflicts: Vec::new(),
            merged_files: 0,
        });
    }

    // Normal merge: a merge commit, or conflicts left in the work tree.
    let msg = format!("Merge branch '{branch_name}'");
    let o = crate::git_safe::run_write(
        &workdir,
        &["merge", "-q", "--no-ff", "--no-edit", "--no-verify", "--no-gpg-sign", "-m", &msg, branch_name],
    )?;
    let repo = crate::gitx::discover(&workdir).map_err(|e| format!("Not a git repository: {e}"))?;
    if o.ok() {
        let summary = diff_summary(repo_path).unwrap_or_default();
        return Ok(MergeResult {
            success: true,
            conflicts: Vec::new(),
            merged_files: summary.len() as u32,
        });
    }
    if repo.state() == Some(P::Merge) {
        let index = repo.index_or_empty().map_err(|e| format!("Failed to get index: {e}"))?;
        let conflicts = crate::gitx::conflicted_paths(&index);
        if !conflicts.is_empty() {
            return Ok(MergeResult {
                success: false,
                conflicts,
                merged_files: 0,
            });
        }
    }
    let said = o.said();
    if said.contains("overwritten") {
        return Err(format!(
            "{}: merging '{branch_name}' would overwrite uncommitted changes ({said}); commit or stash them first",
            refusal::MAIN_HAS_UNCOMMITTED_CHANGES
        ));
    }
    Err(format!("Merge failed: {said}"))
}

/// Paths with uncommitted changes (staged, unstaged or untracked) that a
/// fast-forward from HEAD to `target` would write over, sorted.
fn fast_forward_blockers(repo: &gix::Repository, target: gix::ObjectId) -> Result<Vec<String>, String> {
    let head_tree = crate::gitx::head_tree_id(repo).ok_or("Failed to read HEAD")?;
    let target_tree = crate::gitx::commit_tree_id(repo, target).ok_or("Failed to read the branch")?;
    let touched: std::collections::HashSet<String> = crate::gitx::diff::tree_to_tree(repo, Some(head_tree), Some(target_tree))
        .map_err(|e| format!("Failed to diff for fast-forward: {e}"))?
        .into_iter()
        .map(|c| c.path)
        .collect();
    let opts = gstatus::Options { untracked: gstatus::Untracked::Files, staged_renames: false, submodules: true };
    let rows = gstatus::statuses(repo, opts).map_err(|e| format!("Failed to read status: {e}"))?;
    let mut blocked: Vec<String> = rows.into_iter().map(|r| r.path).filter(|p| touched.contains(p)).collect();
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

/// Abort an in-progress merge (`git merge --abort`). Refused
/// (`no_merge_in_progress`) when the repository has no merge in progress:
/// a reset would otherwise throw away ordinary uncommitted work.
pub fn abort_merge(repo_path: &str) -> Result<(), String> {
    let (repo, workdir) = workdir_of(repo_path)?;
    if repo.state() != Some(gix::state::InProgress::Merge) {
        return Err(format!(
            "{}: there is no merge in progress in {repo_path}; nothing to abort",
            refusal::NO_MERGE_IN_PROGRESS
        ));
    }
    crate::git_safe::run_write_ok(&workdir, "Failed to abort the merge", &["merge", "--abort"])?;
    Ok(())
}

/// Resolve a conflict by choosing "ours" or "theirs". `file_path` must be
/// a file in conflict right now (its exact repo-relative path, so a
/// pattern never matches); anything else is refused (`not_in_conflict`)
/// and untouched. git checks out that side and stages the file.
pub fn resolve_conflict(repo_path: &str, file_path: &str, resolution: &str) -> Result<(), String> {
    let (repo, workdir) = workdir_of(repo_path)?;
    let index = repo.index_or_empty().map_err(|e| format!("Failed to get index: {e}"))?;
    if !crate::gitx::conflicted_paths(&index).iter().any(|p| p == file_path) {
        return Err(format!("{}: {file_path} is not in conflict; nothing to resolve", refusal::NOT_IN_CONFLICT));
    }
    let side = match resolution {
        "ours" => "--ours",
        "theirs" => "--theirs",
        _ => return Err(format!("Invalid resolution: {resolution} (use 'ours' or 'theirs')")),
    };
    crate::git_safe::run_write_ok(&workdir, &format!("Failed to checkout '{resolution}'"), &["checkout", "-q", side, "--", file_path])?;
    // Stage the resolved file
    crate::git_safe::run_write_ok(&workdir, "Failed to stage the resolved file", &["add", "--", file_path])?;
    Ok(())
}

/// Delete a local branch (merged or not). git refuses a branch checked out
/// in any worktree, and drops its `[branch "<name>"]` config with it.
pub fn delete_branch(repo_path: &str, branch_name: &str) -> Result<(), String> {
    let (repo, workdir) = workdir_of(repo_path)?;
    if branch_name.starts_with('-') || crate::gitx::local_branch_tip(&repo, branch_name).is_none() {
        return Err(format!("Branch '{branch_name}' not found"));
    }
    crate::git_safe::run_write_ok(&workdir, &format!("Failed to delete branch '{branch_name}'"), &["branch", "-D", branch_name])?;
    Ok(())
}

/// Collect a JSON snapshot of the repo's current state for the AI
/// commit workflow. Runs a few tiny `git` subprocesses with a 5-second
/// per-command timeout — gix would need 5 separate code paths for
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

#[cfg(test)]
#[path = "git_write_tests.rs"]
mod write_tests;
