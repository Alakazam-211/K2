//! Workbench W1 (0.46.7): the git half of worktree sidecars
//! (`.k2/prds/research-t3code-ui-as-garden-v1-vs-live.md` §2 E1/E5/E6/E9,
//! §4.1; Rosson 2026-10-29).
//!
//! A worktree sidecar is a `k2 sidecar` that runs in its own git worktree:
//! folder `.worktrees/k2-<name>` (gitignored by [`crate::git::create_worktree`]),
//! branch `k2/<name>`, cut from a base (default: the workspace's default
//! branch, never whatever HEAD happens to be). When its work is ready a
//! person **Lands** it: K2 cherry-picks the branch's commits that are not on
//! the base yet (by patch-id) onto the base in the workspace's main
//! checkout. Then it is **Retired**: stopped, its worktree sent to the
//! Trash through the guarded [`crate::git::remove_worktree`], and its branch
//! deleted only when every commit landed or the person discarded it.
//!
//! Everything here is git CLI through [`crate::git_safe`] (hooks off, no
//! external diff). Land never force-checks-out and never resets: on a
//! conflict it runs `git cherry-pick --abort`, which puts the main checkout
//! back exactly as it was (main had no tracked changes, checked first), and
//! reports the conflicted files. One Land per repository at a time.
//!
//! The durable columns (migration 0251) are in [`store`]; the inbox state a
//! row shows is [`task_state`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::Serialize;

/// Refusal codes (the daemon answers `{"error":{"code","hint",…}}`).
pub mod codes {
    /// The workspace folder is not a git repository.
    pub const NOT_A_REPO: &str = "not_a_repo";
    /// `--base` names nothing git can resolve to a commit.
    pub const BASE_NOT_FOUND: &str = "base_not_found";
    /// Branch `k2/<name>` already exists.
    pub const BRANCH_EXISTS: &str = "branch_exists";
    /// Folder `.worktrees/k2-<name>` already exists.
    pub const WORKTREE_EXISTS: &str = "worktree_exists";
    /// The main checkout is not on the branch Land puts the work on.
    pub const WRONG_BRANCH: &str = "wrong_branch";
    /// A merge, rebase, cherry-pick or revert is in progress in the main checkout.
    pub const OPERATION_IN_PROGRESS: &str = "operation_in_progress";
    /// The main checkout has uncommitted changes to tracked files (B0's code).
    pub const MAIN_HAS_UNCOMMITTED_CHANGES: &str = crate::git::refusal::MAIN_HAS_UNCOMMITTED_CHANGES;
    /// The base or the branch moved since the person looked.
    pub const STALE: &str = "stale";
    /// A merge commit is among the commits to land (no `-m` in v1).
    pub const HAS_MERGE: &str = "has_merge";
    /// Every commit of the branch is already on the base.
    pub const ALREADY_LANDED: &str = "already_landed";
    /// The cherry-pick conflicted; it was aborted and main is unchanged.
    pub const CONFLICT: &str = "conflict";
    /// Retire found commits not on the base, or uncommitted files, and no discard.
    pub const UNLANDED_WORK: &str = "unlanded_work";
    /// git itself failed (the hint carries its message).
    pub const GIT_FAILED: &str = "git_failed";
}

/// One commit: full sha and subject line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    /// True for a merge commit (two or more parents).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub merge: bool,
}

/// One changed file of the branch against its base (`git diff --name-status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    /// `A`, `M`, `D`, `T` (renames show as `D` + `A`).
    pub status: String,
    pub path: String,
}

/// A coded refusal (or failure) with the files / commits it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WtError {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
    pub files: Vec<String>,
    pub commits: Vec<Commit>,
}

impl WtError {
    pub fn new(code: &'static str, status: u16, hint: impl Into<String>) -> Self {
        Self { code, status, hint: hint.into(), files: Vec::new(), commits: Vec::new() }
    }
    fn with_files(mut self, files: Vec<String>) -> Self {
        self.files = files;
        self
    }
    fn with_commits(mut self, commits: Vec<Commit>) -> Self {
        self.commits = commits;
        self
    }
}

impl std::fmt::Display for WtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.hint)
    }
}

/// The branch a worktree sidecar named `slug` works on.
pub fn branch_for(slug: &str) -> String {
    format!("k2/{slug}")
}

/// Its folder, relative to the workspace root.
pub fn dir_for(slug: &str) -> String {
    format!(".worktrees/k2-{slug}")
}

// ── git plumbing ─────────────────────────────────────────────────────

/// Run hardened git in `dir`: `Ok(stdout)` on success, `Err(stderr)`.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = crate::git_safe::git(dir)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("git {} failed ({})", args.join(" "), out.status) } else { err })
    }
}

fn git_failed(what: &str, e: String) -> WtError {
    WtError::new(codes::GIT_FAILED, 500, format!("{what}: {e}"))
}

/// The commit `rev` names, or `None`.
fn rev_parse(dir: &Path, rev: &str) -> Option<String> {
    if rev.starts_with('-') {
        return None;
    }
    git(dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn local_branch_exists(dir: &Path, branch: &str) -> bool {
    !branch.starts_with('-') && git(dir, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok()
}

/// The branch the checkout at `dir` is on (`None` when detached).
pub fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The workspace's default branch: `origin/HEAD`'s branch when it exists
/// locally, else `main`, else `master`, else the branch the main checkout
/// is on.
pub fn default_branch(root: &Path) -> Option<String> {
    if let Ok(s) = git(root, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]) {
        if let Some(b) = s.trim().strip_prefix("origin/") {
            if local_branch_exists(root, b) {
                return Some(b.to_string());
            }
        }
    }
    for b in ["main", "master"] {
        if local_branch_exists(root, b) {
            return Some(b.to_string());
        }
    }
    current_branch(root)
}

fn is_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|s| s.trim() == "true")
}

/// `git status --porcelain -z` paths (renames: the new path). Untracked
/// files count when `untracked`; ignored files never do.
fn status_paths(dir: &Path, untracked: bool) -> Result<Vec<String>, WtError> {
    let mode = if untracked { "--untracked-files=all" } else { "--untracked-files=no" };
    let raw = git(dir, &["status", "--porcelain=v1", "-z", mode]).map_err(|e| git_failed("git status", e))?;
    let mut out = Vec::new();
    let mut parts = raw.split('\0').filter(|p| !p.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (xy, path) = entry.split_at(3);
        let xy = xy.as_bytes();
        // A rename / copy is followed by its source path.
        if matches!(xy[0], b'R' | b'C') || matches!(xy[1], b'R' | b'C') {
            parts.next();
        }
        out.push(path.to_string());
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Files with uncommitted work in the checkout at `dir` (tracked changes,
/// staged or not, and untracked files; ignored files never).
pub fn dirty_files(dir: &Path) -> Result<Vec<String>, WtError> {
    status_paths(dir, true)
}

/// Tracked files with uncommitted changes (staged or not) at `dir`.
pub fn tracked_changes(dir: &Path) -> Result<Vec<String>, WtError> {
    status_paths(dir, false)
}

fn range(onto: &str, branch: &str) -> String {
    format!("refs/heads/{onto}...refs/heads/{branch}")
}

/// The commits of `branch` that are not on `onto` by patch-id, oldest
/// first (merge commits included and flagged).
pub fn unlanded_commits(root: &Path, onto: &str, branch: &str) -> Result<Vec<Commit>, WtError> {
    let raw = git(
        root,
        &["log", "--reverse", "--right-only", "--cherry-pick", "--format=%H%x1f%P%x1f%s%x1e", &range(onto, branch), "--"],
    )
    .map_err(|e| git_failed("git log", e))?;
    Ok(raw
        .split('\u{1e}')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let mut f = r.splitn(3, '\u{1f}');
            let sha = f.next()?.trim().to_string();
            let parents = f.next().unwrap_or("").split_whitespace().count();
            let subject = f.next().unwrap_or("").to_string();
            Some(Commit { sha, subject, merge: parents > 1 })
        })
        .collect())
}

/// The files `branch` changes against its merge base with `onto`.
pub fn changed_files(root: &Path, onto: &str, branch: &str) -> Result<Vec<FileChange>, WtError> {
    let mut args = vec!["diff"];
    args.extend(crate::git_safe::NO_EXT_DIFF);
    let r = range(onto, branch);
    args.extend(["--no-renames", "--name-status", "-z", r.as_str(), "--"]);
    let raw = git(root, &args).map_err(|e| git_failed("git diff", e))?;
    let parts: Vec<&str> = raw.split('\0').filter(|p| !p.is_empty()).collect();
    Ok(parts
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| FileChange { status: c[0].chars().next().map(String::from).unwrap_or_default(), path: c[1].to_string() })
        .collect())
}

// ── create (B1) ──────────────────────────────────────────────────────

/// What `--base` resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Base {
    /// As given, or the default branch.
    pub name: String,
    pub sha: String,
    /// The branch Land puts the work on: `name` when it is a local
    /// branch, else the default branch.
    pub onto: Option<String>,
}

/// Resolve `--base` (default: the workspace's default branch).
pub fn resolve_base(root: &Path, base: Option<&str>) -> Result<Base, WtError> {
    if !is_repo(root) {
        return Err(WtError::new(
            codes::NOT_A_REPO,
            409,
            format!("{} is not a git repository; worktree sidecars need one (git init, then commit once)", root.display()),
        ));
    }
    let default = default_branch(root);
    let name = match base.map(str::trim).filter(|s| !s.is_empty()) {
        Some(b) => b.to_string(),
        None => default.clone().ok_or_else(|| {
            WtError::new(codes::BASE_NOT_FOUND, 409, "this repository has no branch to start from yet; commit once, or pass --base <ref>")
        })?,
    };
    let sha = rev_parse(root, &name)
        .ok_or_else(|| WtError::new(codes::BASE_NOT_FOUND, 404, format!("--base '{name}' is not a branch, tag or commit in this repository")))?;
    let onto = if local_branch_exists(root, &name) { Some(name.clone()) } else { default };
    Ok(Base { name, sha, onto })
}

/// A created worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    /// Absolute (canonical) path of the worktree.
    pub path: String,
    /// Relative to the workspace root (`.worktrees/k2-<name>`).
    pub dir: String,
    pub branch: String,
    pub base: Base,
}

/// `git worktree add -b k2/<slug> .worktrees/k2-<slug> <base sha>` in the
/// workspace at `root`. Refuses an existing branch or folder.
pub fn create(root: &Path, slug: &str, base: Option<&str>) -> Result<Created, WtError> {
    let base = resolve_base(root, base)?;
    let branch = branch_for(slug);
    let dir = dir_for(slug);
    if local_branch_exists(root, &branch) {
        return Err(WtError::new(
            codes::BRANCH_EXISTS,
            409,
            format!("branch {branch} already exists; pick another name, or delete the branch first"),
        ));
    }
    let abs = root.join(&dir);
    if std::fs::symlink_metadata(&abs).is_ok() {
        return Err(WtError::new(codes::WORKTREE_EXISTS, 409, format!("{dir} already exists; pick another name")));
    }
    let made = crate::git::create_worktree_from(&root.to_string_lossy(), &branch, &base.sha)
        .map_err(|e| git_failed("git worktree add", e))?;
    let path = dunce::canonicalize(&made.path).map(|p| p.to_string_lossy().into_owned()).unwrap_or(made.path);
    Ok(Created { path, dir, branch, base })
}

// ── review (the Land sheet) ──────────────────────────────────────────

/// What a person sees before Land, and what the inbox derives from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub onto: String,
    pub onto_head: Option<String>,
    pub branch: String,
    pub tip: Option<String>,
    /// Commits to land, oldest first.
    pub commits: Vec<Commit>,
    /// Files the branch changes against its merge base with `onto`.
    pub files: Vec<FileChange>,
    /// Uncommitted files in the worktree (never landed).
    pub dirty: Vec<String>,
    /// The main checkout's branch and tracked changes (Land's preconditions).
    pub main_branch: Option<String>,
    pub main_dirty: Vec<String>,
    /// `None` when Land would run; else the code it would refuse with.
    pub blocked: Option<&'static str>,
}

/// Read the state of a worktree sidecar's branch against `onto`.
/// `worktree` may be gone (retired): `dirty` is then empty.
pub fn review(root: &Path, worktree: Option<&Path>, onto: &str, branch: &str) -> Result<Review, WtError> {
    let onto_head = rev_parse(root, &format!("refs/heads/{onto}"));
    let tip = rev_parse(root, &format!("refs/heads/{branch}"));
    let (commits, files) = if onto_head.is_some() && tip.is_some() {
        (unlanded_commits(root, onto, branch)?, changed_files(root, onto, branch)?)
    } else {
        (Vec::new(), Vec::new())
    };
    let dirty = match worktree.filter(|w| w.is_dir()) {
        Some(w) => dirty_files(w)?,
        None => Vec::new(),
    };
    let main_branch = current_branch(root);
    let main_dirty = tracked_changes(root)?;
    let blocked = if onto_head.is_none() || tip.is_none() {
        Some(codes::STALE)
    } else if main_branch.as_deref() != Some(onto) {
        Some(codes::WRONG_BRANCH)
    } else if operation_in_progress(root) {
        Some(codes::OPERATION_IN_PROGRESS)
    } else if !main_dirty.is_empty() {
        Some(codes::MAIN_HAS_UNCOMMITTED_CHANGES)
    } else if commits.iter().any(|c| c.merge) {
        Some(codes::HAS_MERGE)
    } else if commits.is_empty() {
        Some(codes::ALREADY_LANDED)
    } else {
        None
    };
    Ok(Review { onto: onto.to_string(), onto_head, branch: branch.to_string(), tip, commits, files, dirty, main_branch, main_dirty, blocked })
}

fn operation_in_progress(root: &Path) -> bool {
    match git2::Repository::open(root) {
        Ok(r) => r.state() != git2::RepositoryState::Clean,
        Err(_) => false,
    }
}

// ── land (B3) ────────────────────────────────────────────────────────

/// A successful Land.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Landed {
    pub onto: String,
    /// `onto`'s head before and after.
    pub from: String,
    pub to: String,
    /// The branch's commits that were applied (their original shas).
    pub commits: Vec<Commit>,
}

static REPO_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

fn repo_lock(root: &Path) -> Arc<Mutex<()>> {
    let key = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    REPO_LOCKS.get_or_init(|| Mutex::new(HashMap::new())).lock().entry(key).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
}

/// `want` names `have`: the full sha, or a prefix of at least 7.
pub fn sha_matches(have: &str, want: &str) -> bool {
    let want = want.trim();
    want.len() >= 7 && have.starts_with(&want.to_ascii_lowercase())
}

/// Cherry-pick `branch`'s commits that are not on `onto` (patch-id) onto
/// `onto`, in the main checkout at `root`. Refuses (nothing changed)
/// unless: `root` is on `onto`, no git operation is in progress, no tracked
/// file has uncommitted changes, `onto` is still at `expect_onto` (when
/// given) and `branch` is still at `expect_tip`, there is at least one
/// commit to land and none is a merge. A conflict aborts the cherry-pick
/// (main is back where it was) and returns [`codes::CONFLICT`] with the
/// files.
pub fn land(root: &Path, onto: &str, branch: &str, expect_onto: Option<&str>, expect_tip: &str) -> Result<Landed, WtError> {
    let lock = repo_lock(root);
    let _guard = lock.lock();

    if !is_repo(root) {
        return Err(WtError::new(codes::NOT_A_REPO, 409, format!("{} is not a git repository", root.display())));
    }
    let onto_head = rev_parse(root, &format!("refs/heads/{onto}"))
        .ok_or_else(|| WtError::new(codes::STALE, 409, format!("branch {onto} no longer exists")))?;
    let tip = rev_parse(root, &format!("refs/heads/{branch}"))
        .ok_or_else(|| WtError::new(codes::STALE, 409, format!("branch {branch} no longer exists")))?;
    let here = current_branch(root);
    if here.as_deref() != Some(onto) {
        return Err(WtError::new(
            codes::WRONG_BRANCH,
            409,
            format!(
                "the workspace's main checkout is on {}, not {onto}; switch it to {onto} first",
                here.as_deref().unwrap_or("a detached HEAD")
            ),
        ));
    }
    if operation_in_progress(root) {
        return Err(WtError::new(
            codes::OPERATION_IN_PROGRESS,
            409,
            "a merge, rebase or cherry-pick is in progress in the workspace's main checkout; finish or abort it first",
        ));
    }
    let main_dirty = tracked_changes(root)?;
    if !main_dirty.is_empty() {
        return Err(WtError::new(
            codes::MAIN_HAS_UNCOMMITTED_CHANGES,
            409,
            format!("{onto} has uncommitted changes in {}; commit or stash them first", main_dirty.join(", ")),
        )
        .with_files(main_dirty));
    }
    if let Some(want) = expect_onto.map(str::trim).filter(|s| !s.is_empty()) {
        if !sha_matches(&onto_head, want) {
            return Err(WtError::new(
                codes::STALE,
                409,
                format!("{onto} moved since you looked (now {}); review it again", &onto_head[..12.min(onto_head.len())]),
            ));
        }
    }
    if !sha_matches(&tip, expect_tip) {
        return Err(WtError::new(
            codes::STALE,
            409,
            format!("{branch} moved since you looked (now {}); review it again", &tip[..12.min(tip.len())]),
        ));
    }
    let commits = unlanded_commits(root, onto, branch)?;
    if commits.iter().any(|c| c.merge) {
        let merges: Vec<Commit> = commits.iter().filter(|c| c.merge).cloned().collect();
        return Err(WtError::new(
            codes::HAS_MERGE,
            409,
            format!("{branch} has a merge commit to land; rebase it onto {onto} in the worktree first (no merges in v1)"),
        )
        .with_commits(merges));
    }
    if commits.is_empty() {
        return Err(WtError::new(codes::ALREADY_LANDED, 409, format!("every commit of {branch} is already on {onto}")));
    }

    let mut args: Vec<&str> = vec!["cherry-pick"];
    args.extend(commits.iter().map(|c| c.sha.as_str()));
    if let Err(stderr) = git(root, &args) {
        let conflicted: Vec<String> = git(root, &["diff", "--name-only", "-z", "--diff-filter=U"])
            .map(|s| s.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        let picking = rev_parse(root, "CHERRY_PICK_HEAD").is_some() || operation_in_progress(root);
        if picking {
            if let Err(e) = git(root, &["cherry-pick", "--abort"]) {
                return Err(git_failed(
                    "the cherry-pick failed and `git cherry-pick --abort` failed too; the main checkout needs a person (git status)",
                    format!("{stderr} / abort: {e}"),
                )
                .with_files(conflicted));
            }
        }
        let back = rev_parse(root, "HEAD");
        if back.as_deref() != Some(onto_head.as_str()) {
            return Err(git_failed(
                "the cherry-pick failed and main did not go back to where it was; the main checkout needs a person (git status)",
                format!("{stderr} (HEAD {back:?}, was {onto_head})"),
            )
            .with_files(conflicted));
        }
        if !conflicted.is_empty() {
            return Err(WtError::new(
                codes::CONFLICT,
                409,
                format!(
                    "{branch} conflicts with {onto} in {}; nothing changed on {onto}. Rebase the branch onto {onto} in its worktree, then land again",
                    conflicted.join(", ")
                ),
            )
            .with_files(conflicted));
        }
        return Err(git_failed("git cherry-pick failed; nothing changed on main", stderr));
    }
    let to = rev_parse(root, "HEAD").ok_or_else(|| git_failed("read HEAD after landing", "no HEAD".into()))?;
    Ok(Landed { onto: onto.to_string(), from: onto_head, to, commits })
}

/// Delete branch `branch` (always `k2/…`) in the repository at `root`.
pub fn delete_branch(root: &Path, branch: &str) -> Result<(), WtError> {
    if !branch.starts_with("k2/") {
        return Err(WtError::new(codes::GIT_FAILED, 500, format!("refusing to delete {branch}: not a K2 worktree branch")));
    }
    if !local_branch_exists(root, branch) {
        return Ok(());
    }
    git(root, &["branch", "-D", branch]).map(|_| ()).map_err(|e| git_failed("git branch -D", e))
}

// ── inbox state (E9) ─────────────────────────────────────────────────

/// The inbox state of a worktree sidecar (Workbench's columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    /// Nothing done yet: no commits to land, no uncommitted files, idle.
    New,
    /// Its session is mid-turn (or waiting on a person).
    Working,
    /// Idle with commits to land or uncommitted files.
    Review,
    /// Its commits are on the base (and it may be retired).
    Landed,
    /// Retired without landing.
    Discarded,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Working => "working",
            Self::Review => "review",
            Self::Landed => "landed",
            Self::Discarded => "discarded",
        }
    }
}

/// Derive the state from the stored row, whether the session is busy, and
/// git (commits not on the base, uncommitted files).
pub fn task_state(meta: &store::WorktreeMeta, busy: bool, unlanded: usize, dirty: bool) -> TaskState {
    match meta.retired_how.as_deref() {
        Some(store::RETIRED_DISCARDED) => return TaskState::Discarded,
        Some(_) => return TaskState::Landed,
        None => {}
    }
    if busy {
        TaskState::Working
    } else if unlanded > 0 || dirty {
        TaskState::Review
    } else if meta.landed_at.is_some() {
        TaskState::Landed
    } else {
        TaskState::New
    }
}

// ── durable columns (migration 0251) ─────────────────────────────────

pub mod store {
    use rusqlite::{params, Connection, OptionalExtension};

    pub const RETIRED_LANDED: &str = "landed";
    pub const RETIRED_DISCARDED: &str = "discarded";

    /// The worktree columns of one sidecar tab row.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct WorktreeMeta {
        /// The `workspaces` row (type `worktree`); `None` once retired.
        pub worktree_ws_id: Option<String>,
        pub branch: String,
        /// Relative to the workspace root.
        pub dir: String,
        pub base_ref: Option<String>,
        pub landed_at: Option<i64>,
        pub landed_head: Option<String>,
        pub retired_at: Option<i64>,
        pub retired_how: Option<String>,
    }

    /// The row's worktree columns; `None` for a plain sidecar.
    pub fn get(conn: &Connection, project_id: &str, pane_group_id: &str) -> Option<WorktreeMeta> {
        conn.query_row(
            "SELECT worktree_ws_id, worktree_branch, worktree_dir, base_ref, landed_at, landed_head, retired_at, retired_how \
             FROM workspace_tab_sessions WHERE project_id = ?1 AND pane_group_id = ?2 AND worktree_branch IS NOT NULL",
            params![project_id, pane_group_id],
            |r| {
                Ok(WorktreeMeta {
                    worktree_ws_id: r.get(0)?,
                    branch: r.get(1)?,
                    dir: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    base_ref: r.get(3)?,
                    landed_at: r.get(4)?,
                    landed_head: r.get(5)?,
                    retired_at: r.get(6)?,
                    retired_how: r.get(7)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    fn one_row(n: usize, pane_group_id: &str) -> Result<(), String> {
        if n == 0 {
            return Err(format!("no tab row for pane group {pane_group_id}"));
        }
        Ok(())
    }

    /// Record a new worktree sidecar's worktree.
    pub fn stamp_new(
        conn: &Connection,
        project_id: &str,
        pane_group_id: &str,
        worktree_ws_id: &str,
        branch: &str,
        dir: &str,
        base_ref: Option<&str>,
    ) -> Result<(), String> {
        let n = conn
            .execute(
                "UPDATE workspace_tab_sessions SET worktree_ws_id = ?3, worktree_branch = ?4, worktree_dir = ?5, \
                 base_ref = ?6, landed_at = NULL, landed_head = NULL, retired_at = NULL, retired_how = NULL \
                 WHERE project_id = ?1 AND pane_group_id = ?2",
                params![project_id, pane_group_id, worktree_ws_id, branch, dir, base_ref],
            )
            .map_err(|e| format!("stamp worktree: {e}"))?;
        one_row(n, pane_group_id)
    }

    pub fn mark_landed(conn: &Connection, project_id: &str, pane_group_id: &str, head: &str, at: i64) -> Result<(), String> {
        let n = conn
            .execute(
                "UPDATE workspace_tab_sessions SET landed_at = ?3, landed_head = ?4 WHERE project_id = ?1 AND pane_group_id = ?2",
                params![project_id, pane_group_id, at, head],
            )
            .map_err(|e| format!("mark landed: {e}"))?;
        one_row(n, pane_group_id)
    }

    /// Retired: the `workspaces` row is gone, so its id is cleared.
    pub fn mark_retired(conn: &Connection, project_id: &str, pane_group_id: &str, how: &str, at: i64) -> Result<(), String> {
        let n = conn
            .execute(
                "UPDATE workspace_tab_sessions SET retired_at = ?3, retired_how = ?4, worktree_ws_id = NULL \
                 WHERE project_id = ?1 AND pane_group_id = ?2",
                params![project_id, pane_group_id, at, how],
            )
            .map_err(|e| format!("mark retired: {e}"))?;
        one_row(n, pane_group_id)
    }

    /// Worktree sidecars of the workspace that are not retired (the cap).
    pub fn open_count(conn: &Connection, project_id: &str) -> usize {
        conn.query_row(
            "SELECT COUNT(*) FROM workspace_tab_sessions WHERE project_id = ?1 AND worktree_branch IS NOT NULL AND retired_at IS NULL",
            params![project_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "worktree_task_tests.rs"]
mod tests;
