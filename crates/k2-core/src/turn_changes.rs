//! What one agent turn changed, read from git (workbench-v1 B7, vs-live E8;
//! Rosson 2026-10-23: "we're sticking with threads", so nothing here reads
//! the agent's transcript).
//!
//! [`mark`] takes a turn-start picture of a worktree: the HEAD commit and
//! the content of every file that differs from it (uncommitted, staged or
//! untracked; ignored files never). [`changes_since`] takes the same
//! picture at the turn's end and compares the two per file: a file counts
//! when its content at the end differs from its content at the start,
//! whether the agent left it uncommitted or committed it. A file edited and
//! put back the same counts as nothing. Commits made during the turn are
//! `start_head..end_head`.
//!
//! The answer is the catalog's `ThreadChanges` ([`ThreadChanges`]): paths
//! relative to the worktree, at most [`THREAD_CHANGES_MAX_FILES`] files by
//! path (`truncated` past it), `+/−` from a line diff of the two contents
//! (None for binary or very large files). Nothing changed is `None`: the
//! daemon posts no card.
//!
//! Bounds: a mark keeps file bytes only up to [`MARK_FILE_BYTES`] per file
//! and [`MARK_TOTAL_BYTES`] in all (past them, the file's blob id still
//! tells changed from unchanged; only its `+/−` is unknown), and looks at
//! no more than [`MAX_CANDIDATES`] paths.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use git2::{Oid, Repository, Status, StatusOptions};

use crate::overlay::{ThreadChangedFile, ThreadChanges, THREAD_CHANGES_MAX_FILES};

/// Bytes kept per dirty file at a turn's start (for its `+/−`).
pub const MARK_FILE_BYTES: u64 = 1024 * 1024;
/// Bytes kept in all per mark.
pub const MARK_TOTAL_BYTES: usize = 8 * 1024 * 1024;
/// Paths compared per turn, at most.
pub const MAX_CANDIDATES: usize = 2000;
/// Files whose `+/−` is counted per turn, at most (the rest still count).
const MAX_STATS: usize = 500;

/// One file's content at a mark: absent, or its blob id (and its bytes
/// when small enough to keep).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Absent,
    Blob { oid: Oid, bytes: Option<Vec<u8>> },
}

impl Content {
    fn oid(&self) -> Option<Oid> {
        match self {
            Content::Absent => None,
            Content::Blob { oid, .. } => Some(*oid),
        }
    }
}

/// A worktree at a turn's start.
#[derive(Debug, Clone)]
pub struct TurnMark {
    head: Option<Oid>,
    /// Files that differed from `head`, by their content then.
    dirty: BTreeMap<String, Content>,
}

impl TurnMark {
    /// The HEAD commit at the mark (tests, logs).
    pub fn head(&self) -> Option<String> {
        self.head.map(|o| o.to_string())
    }
}

fn open(path: &Path) -> Option<Repository> {
    let repo = Repository::open(path).ok()?;
    if repo.is_bare() {
        return None;
    }
    Some(repo)
}

fn head_oid(repo: &Repository) -> Option<Oid> {
    repo.head().ok().and_then(|h| h.peel_to_commit().ok()).map(|c| c.id())
}

/// Paths that differ from HEAD (index or worktree, untracked included,
/// ignored never). Directories are expanded (`recurse_untracked_dirs`).
fn dirty_paths(repo: &Repository) -> Vec<String> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true)
        .renames_head_to_index(false);
    let Ok(statuses) = repo.statuses(Some(&mut opts)) else { return Vec::new() };
    statuses
        .iter()
        .filter(|e| e.status() != Status::CURRENT && !e.status().contains(Status::IGNORED))
        .filter_map(|e| e.path().map(str::to_string))
        .take(MAX_CANDIDATES)
        .collect()
}

/// The file's content in the worktree now. `budget` is what's left of the
/// mark's byte allowance (None: keep bytes regardless, the turn's end).
fn disk_content(workdir: &Path, rel: &str, budget: Option<&mut usize>) -> Content {
    let p = workdir.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&p) else { return Content::Absent };
    if meta.is_dir() {
        return Content::Absent;
    }
    let bytes = if meta.file_type().is_symlink() {
        match std::fs::read_link(&p) {
            Ok(t) => t.to_string_lossy().into_owned().into_bytes(),
            Err(_) => return Content::Absent,
        }
    } else if meta.len() > MARK_FILE_BYTES * 8 {
        // Too big to read whole: its id comes from git's own hasher.
        return match Oid::hash_file(git2::ObjectType::Blob, &p) {
            Ok(oid) => Content::Blob { oid, bytes: None },
            Err(_) => Content::Absent,
        };
    } else {
        match std::fs::read(&p) {
            Ok(b) => b,
            Err(_) => return Content::Absent,
        }
    };
    let Ok(oid) = Oid::hash_object(git2::ObjectType::Blob, &bytes) else { return Content::Absent };
    let keep = bytes.len() as u64 <= MARK_FILE_BYTES
        && match budget {
            None => true,
            Some(left) if *left >= bytes.len() => {
                *left -= bytes.len();
                true
            }
            Some(_) => false,
        };
    Content::Blob { oid, bytes: keep.then_some(bytes) }
}

/// The file's content in commit `at` (Absent when it isn't there).
fn tree_content(repo: &Repository, at: Option<Oid>, rel: &str) -> Content {
    let Some(at) = at else { return Content::Absent };
    let Ok(commit) = repo.find_commit(at) else { return Content::Absent };
    let Ok(tree) = commit.tree() else { return Content::Absent };
    let Ok(entry) = tree.get_path(Path::new(rel)) else { return Content::Absent };
    if entry.kind() != Some(git2::ObjectType::Blob) {
        return Content::Absent;
    }
    let oid = entry.id();
    let bytes = repo.find_blob(oid).ok().filter(|b| (b.size() as u64) <= MARK_FILE_BYTES).map(|b| b.content().to_vec());
    Content::Blob { oid, bytes }
}

/// The turn-start picture of the worktree at `path`, or None when `path`
/// isn't in a (non-bare) git repository.
pub fn mark(path: &Path) -> Option<TurnMark> {
    let repo = open(path)?;
    let workdir = repo.workdir()?.to_path_buf();
    let head = head_oid(&repo);
    let mut budget = MARK_TOTAL_BYTES;
    let dirty = dirty_paths(&repo)
        .into_iter()
        .map(|rel| {
            let c = disk_content(&workdir, &rel, Some(&mut budget));
            (rel, c)
        })
        .collect();
    Some(TurnMark { head, dirty })
}

/// Paths that differ between two commits' trees.
fn tree_diff_paths(repo: &Repository, from: Option<Oid>, to: Option<Oid>) -> Vec<String> {
    let tree = |o: Option<Oid>| o.and_then(|o| repo.find_commit(o).ok()).and_then(|c| c.tree().ok());
    let (a, b) = (tree(from), tree(to));
    let Ok(diff) = repo.diff_tree_to_tree(a.as_ref(), b.as_ref(), None) else { return Vec::new() };
    let mut out = Vec::new();
    for d in diff.deltas() {
        for f in [d.old_file(), d.new_file()] {
            if let Some(p) = f.path().and_then(|p| p.to_str()) {
                out.push(p.to_string());
            }
        }
        if out.len() >= MAX_CANDIDATES {
            break;
        }
    }
    out
}

fn commits_between(repo: &Repository, from: Option<Oid>, to: Option<Oid>) -> u32 {
    let Some(to) = to else { return 0 };
    if Some(to) == from {
        return 0;
    }
    let Ok(mut walk) = repo.revwalk() else { return 0 };
    if walk.push(to).is_err() {
        return 0;
    }
    if let Some(from) = from {
        // A rewritten history (reset, rebase) still counts only what's new.
        let _ = walk.hide(from);
    }
    walk.take(10_000).filter(Result::is_ok).count() as u32
}

/// `(additions, deletions)` between two contents; None when either side is
/// binary or wasn't kept.
fn line_stats(old: &Content, new: &Content) -> Option<(u32, u32)> {
    let side = |c: &Content| -> Option<Vec<u8>> {
        match c {
            Content::Absent => Some(Vec::new()),
            Content::Blob { bytes, .. } => bytes.clone(),
        }
    };
    let (a, b) = (side(old)?, side(new)?);
    if crate::git_changes::looks_binary(&a) || crate::git_changes::looks_binary(&b) {
        return None;
    }
    let patch = git2::Patch::from_buffers(&a, None, &b, None, None).ok()?;
    let (_, add, del) = patch.line_stats().ok()?;
    Some((add as u32, del as u32))
}

fn short(oid: Option<Oid>) -> Option<String> {
    oid.map(|o| o.to_string().chars().take(7).collect())
}

/// What the turn that started at `start` changed in the worktree at `path`,
/// or None when nothing did (or `path` isn't a repository any more).
pub fn changes_since(path: &Path, start: &TurnMark) -> Option<ThreadChanges> {
    let repo = open(path)?;
    let workdir = repo.workdir()?.to_path_buf();
    let end_head = head_oid(&repo);
    let end_dirty = dirty_paths(&repo);
    let mut candidates: BTreeSet<String> = start.dirty.keys().cloned().collect();
    candidates.extend(end_dirty.iter().cloned());
    candidates.extend(tree_diff_paths(&repo, start.head, end_head));
    let end_dirty: BTreeSet<String> = end_dirty.into_iter().collect();

    let mut files: Vec<ThreadChangedFile> = Vec::new();
    let (mut additions, mut deletions, mut total) = (0u32, 0u32, 0u32);
    for rel in candidates.into_iter().take(MAX_CANDIDATES) {
        let before = match start.dirty.get(&rel) {
            Some(c) => c.clone(),
            None => tree_content(&repo, start.head, &rel),
        };
        let after = if end_dirty.contains(&rel) { disk_content(&workdir, &rel, None) } else { tree_content(&repo, end_head, &rel) };
        if before.oid() == after.oid() {
            continue;
        }
        total += 1;
        let status = match (&before, &after) {
            (Content::Absent, _) => "added",
            (_, Content::Absent) => "deleted",
            _ => "modified",
        };
        let stats = if (total as usize) <= MAX_STATS { line_stats(&before, &after) } else { None };
        if let Some((a, d)) = stats {
            additions += a;
            deletions += d;
        }
        if files.len() < THREAD_CHANGES_MAX_FILES {
            files.push(ThreadChangedFile {
                file: rel,
                old_file: None,
                status: status.to_string(),
                additions: stats.map(|s| s.0),
                deletions: stats.map(|s| s.1),
            });
        }
    }
    let commits = commits_between(&repo, start.head, end_head);
    if total == 0 && commits == 0 {
        return None;
    }
    let branch = repo.head().ok().filter(|h| h.is_branch()).and_then(|h| h.shorthand().map(str::to_string));
    Some(ThreadChanges {
        truncated: (total as usize) > files.len(),
        files,
        total,
        additions,
        deletions,
        commits,
        from: short(start.head),
        to: short(end_head),
        branch,
    })
}

/// The card's plain line: "Changed 3 files (+41 −12) · 2 commits".
pub fn summary_line(c: &ThreadChanges) -> String {
    let mut s = match c.total {
        0 => String::from("No file changes"),
        1 => String::from("Changed 1 file"),
        n => format!("Changed {n} files"),
    };
    if c.total > 0 && (c.additions > 0 || c.deletions > 0) {
        s.push_str(&format!(" (+{} \u{2212}{})", c.additions, c.deletions));
    }
    match c.commits {
        0 => {}
        1 => s.push_str(" \u{b7} 1 commit"),
        n => s.push_str(&format!(" \u{b7} {n} commits")),
    }
    if let Some(b) = c.branch.as_deref().filter(|b| !b.is_empty()) {
        s.push_str(&format!(" on {b}"));
    }
    s
}

/// A session's cwd is a K2 worktree when it sits under a `.worktrees/`
/// folder (k2-core's convention, `git::create_worktree`) and is a git
/// checkout. Only those sessions get changes cards.
pub fn is_k2_worktree(cwd: &Path) -> bool {
    let under = cwd.components().any(|c| c.as_os_str() == ".worktrees");
    under && open(cwd).is_some()
}

#[cfg(test)]
#[path = "turn_changes_tests.rs"]
mod tests;
