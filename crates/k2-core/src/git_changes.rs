//! Changes tab v1 (C1): the daemon-side git reads and writes behind the
//! Changes tab and `k2 changes`.
//!
//! - [`snapshot`]: one call for the whole changes list (status groups,
//!   branch, ahead/behind, per-file +/- counts, a `rev`, a 1,000-row cap),
//!   optionally against a base ref.
//! - [`diff_sides`]: both full sides of one file for a side-by-side view,
//!   with size caps and a binary flag.
//! - [`discard`]: throw away working-tree changes. Nothing is ever
//!   unlinked: every working copy that would be lost goes to the Trash
//!   first (Convention - Security Constraints).
//! - [`current_rev`] / [`bump_rev`]: the opaque per-repo change counter
//!   the daemon's `git_changed` event and the snapshot share.
//! - [`RepoDirs`] / [`classify_git_path`]: which `.git` paths mean "the
//!   Changes view moved" (used by the daemon's file watcher).
//!
//! Contract: `.k2/prds/prd-changes-tab-v1.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

use serde::Serialize;

use crate::gitx::diff::{ChangeKind, FileChange};
use crate::gitx::status::{self as gstatus, Flags};

/// Snapshot row cap (Orca's `git-status-limit`).
pub const MAX_FILES: usize = 1000;
/// Largest blob / file read for a diff side or a line count.
pub const MAX_BLOB_BYTES: u64 = 10 * 1024 * 1024;
/// Most lines per diff side before `tooLarge`.
pub const MAX_LINES: usize = 120_000;
/// Bytes sniffed for a NUL (git's own binary heuristic).
pub const BINARY_SNIFF: usize = 8000;
/// Most files one discard call may name.
pub const MAX_DISCARD_FILES: usize = 1000;

// ── Errors ───────────────────────────────────────────────────────────

/// Stable error codes on the wire (`{"error", "code"}`).
pub mod codes {
    pub const BAD_REQUEST: &str = "bad_request";
    pub const BAD_PATH: &str = "bad_path";
    pub const NOT_A_REPO: &str = "not_a_repo";
    pub const BAD_REF: &str = "bad_ref";
    pub const NOT_FOUND: &str = "not_found";
    pub const CONFLICTED: &str = "conflicted";
    /// Discard needs the person's git to restore or unstage, and none runs
    /// here (`crate::git::refusal::GIT_NOT_INSTALLED`).
    pub const GIT_NOT_INSTALLED: &str = "git_not_installed";
    /// The repository's own git settings run programs and the workspace
    /// isn't trusted (`crate::git::refusal::GIT_CONFIG_RUNS_PROGRAMS`).
    pub const GIT_CONFIG_RUNS_PROGRAMS: &str = "git_config_runs_programs";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesError {
    pub code: &'static str,
    pub hint: String,
}

impl ChangesError {
    pub fn new(code: &'static str, hint: impl Into<String>) -> Self {
        Self { code, hint: hint.into() }
    }
    fn internal(hint: impl Into<String>) -> Self {
        Self::new(codes::BAD_REQUEST, hint)
    }
}

impl std::fmt::Display for ChangesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.hint, self.code)
    }
}

// ── rev ──────────────────────────────────────────────────────────────

fn revs() -> &'static Mutex<HashMap<String, u64>> {
    static REVS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    REVS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn boot_nonce() -> &'static str {
    static NONCE: OnceLock<String> = OnceLock::new();
    NONCE.get_or_init(|| uuid::Uuid::new_v4().simple().to_string()[..8].to_string())
}

/// The key a repo's rev lives under: its workdir, canonical when it
/// exists, no trailing `/`. Every caller (routes, watcher) goes through
/// this so `/var/…` and `/private/var/…` spellings share one counter.
pub fn rev_key(workdir: &Path) -> String {
    let canon = dunce::canonicalize(&workdir).unwrap_or_else(|_| workdir.to_path_buf());
    trim_slash(&canon.to_string_lossy())
}

fn format_rev(n: u64) -> String {
    format!("{}:{n}", boot_nonce())
}

/// The repo's current rev (opaque). `0` until the first change is seen.
pub fn current_rev(key: &str) -> String {
    let n = revs().lock().unwrap_or_else(|p| p.into_inner()).get(key).copied().unwrap_or(0);
    format_rev(n)
}

/// Advance the repo's rev and return the new value.
pub fn bump_rev(key: &str) -> String {
    let mut map = revs().lock().unwrap_or_else(|p| p.into_inner());
    let n = map.entry(key.to_string()).or_insert(0);
    *n += 1;
    format_rev(*n)
}

/// Rev key for the repo that holds `path`, when it is a non-bare repo.
pub fn rev_key_for_path(path: &str) -> Option<String> {
    repo_key_and_root(path).map(|(k, _)| k)
}

/// `(rev_key, repo_root)` for the non-bare repo holding `path`.
/// `repo_root` is the workdir as gix reports it, no trailing `/`.
pub fn repo_key_and_root(path: &str) -> Option<(String, String)> {
    let repo = crate::gitx::discover(path).ok()?;
    let workdir = repo.workdir()?;
    Some((rev_key(workdir), trim_slash(&workdir.to_string_lossy())))
}

/// Answers "is this working-tree path git-ignored?" for one repo, so the
/// watcher can drop edits to build output without re-opening the repo
/// per path. Built per watcher batch, so an edited `.gitignore` counts
/// from the next batch on, and no repository stays open between batches.
pub struct IgnoreChecker {
    excludes: crate::gitx::Excludes,
    workdir: PathBuf,
}

impl IgnoreChecker {
    pub fn open(workdir: &Path) -> Option<Self> {
        let repo = crate::gitx::open(workdir).ok()?;
        let workdir = repo.workdir()?.to_path_buf();
        Some(Self { excludes: crate::gitx::Excludes::new(repo)?, workdir })
    }
    /// `rel` is repo-relative. Errors count as not ignored.
    pub fn is_ignored(&mut self, rel: &str) -> bool {
        let is_dir = self.workdir.join(rel).is_dir();
        self.excludes.is_ignored(rel, is_dir)
    }
}

fn trim_slash(s: &str) -> String {
    if s == "/" {
        return s.to_string();
    }
    s.trim_end_matches('/').to_string()
}

// ── Paths ────────────────────────────────────────────────────────────

/// Turn a client file path into a repo-relative `/` path. Accepts a
/// relative path or an absolute path inside `workdir`. Refuses empty,
/// NUL, `..`, an absolute path outside the repo, and anything under
/// `.git/`. A trailing `/` (an untracked directory row) is dropped.
pub fn normalize_rel_path(workdir: &Path, input: &str) -> Result<String, ChangesError> {
    let bad = |why: &str| ChangesError::new(codes::BAD_PATH, format!("bad file path '{input}': {why}"));
    if input.trim().is_empty() {
        return Err(bad("empty"));
    }
    if input.contains('\0') {
        return Err(bad("contains NUL"));
    }
    let trimmed = input.trim_end_matches('/');
    let p = Path::new(trimmed);
    let rel: PathBuf = if p.is_absolute() {
        let canon_wd = dunce::canonicalize(&workdir).unwrap_or_else(|_| workdir.to_path_buf());
        if let Ok(r) = p.strip_prefix(workdir) {
            r.to_path_buf()
        } else if let Ok(r) = p.strip_prefix(&canon_wd) {
            r.to_path_buf()
        } else if let Some(r) = canonical_prefix(p).as_deref().and_then(|c| c.strip_prefix(&canon_wd).ok()) {
            // Another spelling of the same place (`/var/…` vs `/private/var/…`).
            r.to_path_buf()
        } else {
            return Err(bad("outside the repository"));
        }
    } else {
        p.to_path_buf()
    };
    let mut parts: Vec<String> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => return Err(bad("'..' is not allowed")),
            Component::RootDir | Component::Prefix(_) => return Err(bad("not relative to the repository")),
        }
    }
    if parts.is_empty() {
        return Err(bad("names the repository root"));
    }
    if parts[0].eq_ignore_ascii_case(".git") {
        return Err(bad("inside .git"));
    }
    Ok(parts.join("/"))
}

/// `p` with its nearest existing ancestor canonicalized and the missing
/// tail re-attached as written (the leaf need not exist).
fn canonical_prefix(p: &Path) -> Option<PathBuf> {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(c) = dunce::canonicalize(&cur) {
            let mut out = c;
            for t in tail.iter().rev() {
                out.push(t);
            }
            return Some(out);
        }
        tail.push(cur.file_name()?);
        cur = cur.parent()?;
    }
}

/// Refuse a working-tree path whose real location (after resolving
/// symlinked parent directories) is outside the repo. The leaf itself is
/// never followed by callers.
fn ensure_inside(workdir: &Path, full: &Path) -> Result<(), ChangesError> {
    let canon_wd = dunce::canonicalize(&workdir).unwrap_or_else(|_| workdir.to_path_buf());
    // Nearest existing ancestor of the leaf's parent.
    let mut probe = full.parent().map(Path::to_path_buf);
    while let Some(dir) = probe {
        if let Ok(real) = dunce::canonicalize(&dir) {
            if real.starts_with(&canon_wd) {
                return Ok(());
            }
            return Err(ChangesError::new(
                codes::BAD_PATH,
                format!("'{}' resolves outside the repository", full.display()),
            ));
        }
        probe = dir.parent().map(Path::to_path_buf);
    }
    Err(ChangesError::new(codes::BAD_PATH, format!("'{}' has no existing parent", full.display())))
}

fn open_gix(path: &str) -> Result<(gix::Repository, PathBuf), ChangesError> {
    let repo = crate::gitx::discover(path)
        .map_err(|e| ChangesError::new(codes::NOT_A_REPO, format!("not a git repository: {path} ({e})")))?;
    let workdir = repo
        .workdir()
        .map(Path::to_path_buf)
        .ok_or_else(|| ChangesError::new(codes::NOT_A_REPO, format!("bare repository: {path}")))?;
    Ok((repo, workdir))
}

// ── Content helpers ──────────────────────────────────────────────────

/// True when `bytes` look binary: a NUL in the first [`BINARY_SNIFF`]
/// bytes, or not UTF-8.
pub fn looks_binary(bytes: &[u8]) -> bool {
    let n = bytes.len().min(BINARY_SNIFF);
    bytes[..n].contains(&0) || std::str::from_utf8(bytes).is_err()
}

fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let nl = bytes.iter().filter(|b| **b == b'\n').count();
    nl + usize::from(*bytes.last().unwrap_or(&b'\n') != b'\n')
}

// ── Snapshot ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRow {
    pub path: String,
    pub old_path: Option<String>,
    pub group: &'static str,
    pub status: &'static str,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
    pub dir: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeCounts {
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
    pub base: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BaseInfo {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub oid: String,
    pub merge_base: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub is_repo: bool,
    pub workspace_path: String,
    pub repo_root: String,
    pub rev: String,
    pub branch: Option<String>,
    pub head_oid: Option<String>,
    pub detached: bool,
    pub unborn: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub state: &'static str,
    pub base: Option<BaseInfo>,
    pub counts: ChangeCounts,
    pub truncated: bool,
    pub limit: usize,
    pub files: Vec<ChangeRow>,
}

/// The snapshot answer for a path that is not inside a (non-bare) repo.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotRepo {
    pub is_repo: bool,
    pub workspace_path: String,
    pub rev: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum SnapshotResult {
    Repo(Box<Snapshot>),
    NotRepo(NotRepo),
}

impl SnapshotResult {
    /// The repo snapshot, or `None` for a non-repo path.
    pub fn repo(self) -> Option<Snapshot> {
        match self {
            SnapshotResult::Repo(s) => Some(*s),
            SnapshotResult::NotRepo(_) => None,
        }
    }
}

fn group_order(g: &str) -> u8 {
    match g {
        "conflicted" => 0,
        "staged" => 1,
        "unstaged" => 2,
        "base" => 3,
        _ => 4,
    }
}

fn change_letter(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Added => "A",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Typechange => "T",
        ChangeKind::Conflicted => "C",
        ChangeKind::Modified => "M",
    }
}

/// `(additions, deletions, binary)` per path of `changes`, for the paths
/// in `wanted` only; sides over [`MAX_BLOB_BYTES`] get no counts.
fn diff_stats(
    repo: &gix::Repository,
    workdir: &Path,
    changes: &[FileChange],
    wanted: &HashSet<String>,
) -> HashMap<String, (Option<u32>, Option<u32>, bool)> {
    let mut out = HashMap::new();
    let Some(mut reader) = crate::gitx::diff::WorktreeReader::new(repo) else { return out };
    for c in changes {
        if !wanted.contains(&c.path) {
            continue;
        }
        let old_path = c.old_path.as_deref().unwrap_or(&c.path);
        let too_big = crate::gitx::diff::side_size(repo, workdir, old_path, &c.old) > MAX_BLOB_BYTES
            || crate::gitx::diff::side_size(repo, workdir, &c.path, &c.new) > MAX_BLOB_BYTES;
        if too_big {
            out.insert(c.path.clone(), (None, None, false));
            continue;
        }
        out.insert(c.path.clone(), crate::gitx::diff::change_stats(repo, &mut reader, c, MAX_BLOB_BYTES));
    }
    out
}

/// The Changes snapshot for the workspace at `path` (see the contract).
pub fn snapshot(path: &str, base: Option<&str>) -> Result<SnapshotResult, ChangesError> {
    let ws = trim_slash(path);
    let not_repo = |rev: String| SnapshotResult::NotRepo(NotRepo { is_repo: false, workspace_path: ws.clone(), rev });
    let Ok(repo) = crate::gitx::discover(path) else {
        return Ok(not_repo(current_rev(&ws)));
    };
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
        return Ok(not_repo(current_rev(&ws)));
    };
    let key = rev_key(&workdir);
    // Read the rev BEFORE status: a change landing mid-read bumps it
    // later, so the client refetches.
    let rev = current_rev(&key);

    // ── HEAD / branch / upstream ──
    let head = crate::gitx::head_info(&repo);
    let (branch, unborn, detached) = (head.branch.clone(), head.unborn, head.detached);
    let head_oid = head.oid.map(|o| o.to_string());
    let (mut upstream, mut ahead, mut behind) = (None, 0usize, 0usize);
    if let (Some(name), Some(local)) = (branch.as_deref(), head.oid) {
        if let Some((up, tip)) = crate::gitx::upstream_of(&repo, name) {
            upstream = Some(up);
            if let Some((a, b)) = crate::gitx::ahead_behind(&repo, local, tip) {
                ahead = a;
                behind = b;
            }
        }
    }

    // ── base ──
    let mut base_info: Option<BaseInfo> = None;
    let mut base_tree: Option<gix::ObjectId> = None;
    if let Some(spec) = base.filter(|s| !s.trim().is_empty()) {
        let bad_ref = |e: String| ChangesError::new(codes::BAD_REF, format!("cannot resolve base '{spec}': {e}"));
        crate::git::refuse_message_search_revspec(spec).map_err(|m| ChangesError::new(codes::BAD_REF, m))?;
        let base_oid = crate::gitx::resolve_commit(&repo, spec).map_err(bad_ref)?;
        let (merge_base, a, b) = match head.oid {
            Some(hc) => {
                let mb = crate::gitx::merge_base(&repo, hc, base_oid);
                let (a, b) = crate::gitx::ahead_behind(&repo, hc, base_oid).unwrap_or((0, 0));
                (mb, a, b)
            }
            None => (None, 0, 0),
        };
        // No common ancestor (unrelated histories / unborn HEAD): compare
        // against the base commit itself.
        let tree_commit = merge_base.unwrap_or(base_oid);
        base_tree = Some(
            crate::gitx::commit_tree_id(&repo, tree_commit)
                .ok_or_else(|| ChangesError::internal(format!("read the tree of {tree_commit}")))?,
        );
        base_info = Some(BaseInfo {
            ref_name: spec.to_string(),
            oid: base_oid.to_string(),
            merge_base: merge_base.map(|o| o.to_string()),
            ahead: a,
            behind: b,
        });
    }

    // ── status rows ──
    let statuses = gstatus::statuses(&repo, gstatus::Options::changes())
        .map_err(|e| ChangesError::internal(format!("git status failed: {e}")))?;
    let mut rows: Vec<ChangeRow> = Vec::new();
    let row = |path: String, old_path: Option<String>, group: &'static str, status: &'static str, dir: bool| ChangeRow {
        path,
        old_path,
        group,
        status,
        additions: None,
        deletions: None,
        binary: false,
        dir,
    };
    for entry in statuses {
        let s = entry.flags;
        let path = entry.path;
        if s.contains(Flags::CONFLICTED) {
            rows.push(row(path, None, "conflicted", "C", false));
            continue;
        }
        if s.contains(Flags::WT_NEW) {
            let dir = path.ends_with('/');
            rows.push(row(path, None, "untracked", "U", dir));
            continue;
        }
        if base_tree.is_some() {
            continue; // base view lists tracked paths from the base diff below
        }
        let staged = if s.contains(Flags::INDEX_NEW) {
            Some("A")
        } else if s.contains(Flags::INDEX_DELETED) {
            Some("D")
        } else if s.contains(Flags::INDEX_RENAMED) {
            Some("R")
        } else if s.contains(Flags::INDEX_TYPECHANGE) {
            Some("T")
        } else if s.contains(Flags::INDEX_MODIFIED) {
            Some("M")
        } else {
            None
        };
        if let Some(letter) = staged {
            let old = if letter == "R" { entry.renamed_from.clone() } else { None };
            rows.push(row(path.clone(), old, "staged", letter, false));
        }
        let unstaged = if s.contains(Flags::WT_DELETED) {
            Some("D")
        } else if s.contains(Flags::WT_TYPECHANGE) {
            Some("T")
        } else if s.contains(Flags::WT_MODIFIED) {
            Some("M")
        } else {
            None
        };
        if let Some(letter) = unstaged {
            // A staged rename's worktree edit lives at the new path.
            rows.push(row(path, None, "unstaged", letter, false));
        }
    }

    // Base view: tracked paths that differ between merge-base and the
    // working tree (committed + staged + unstaged).
    let mut base_diff: Option<Vec<FileChange>> = None;
    if let Some(tree) = base_tree {
        let diff = crate::gitx::diff::tree_to_worktree(&repo, Some(tree), true)
            .map_err(|e| ChangesError::internal(format!("base diff failed: {e}")))?;
        let conflicted: HashSet<String> =
            rows.iter().filter(|r| r.group == "conflicted").map(|r| r.path.clone()).collect();
        for d in &diff {
            if conflicted.contains(&d.path) || d.kind == ChangeKind::Conflicted {
                continue;
            }
            rows.push(row(d.path.clone(), d.old_path.clone(), "base", change_letter(d.kind), false));
        }
        base_diff = Some(diff);
    }

    rows.sort_by(|a, b| group_order(a.group).cmp(&group_order(b.group)).then_with(|| a.path.cmp(&b.path)));
    let mut counts = ChangeCounts::default();
    for r in &rows {
        match r.group {
            "staged" => counts.staged += 1,
            "unstaged" => counts.unstaged += 1,
            "untracked" => counts.untracked += 1,
            "conflicted" => counts.conflicted += 1,
            _ => counts.base += 1,
        }
    }
    counts.total = rows.len();
    let truncated = rows.len() > MAX_FILES;
    rows.truncate(MAX_FILES);

    // ── +/- counts for the rows we return ──
    let want = |g: &str| -> HashSet<String> { rows.iter().filter(|r| r.group == g).map(|r| r.path.clone()).collect() };
    let mut stats: HashMap<(&'static str, String), (Option<u32>, Option<u32>, bool)> = HashMap::new();
    let wanted_staged = want("staged");
    let wanted_unstaged = want("unstaged");
    let wanted_base = want("base");
    if !wanted_staged.is_empty() {
        let d = crate::gitx::diff::tree_to_index(&repo, crate::gitx::head_tree_id(&repo), true)
            .map_err(|e| ChangesError::internal(format!("staged diff failed: {e}")))?;
        for (p, v) in diff_stats(&repo, &workdir, &d, &wanted_staged) {
            stats.insert(("staged", p), v);
        }
    }
    if !wanted_unstaged.is_empty() {
        let d = crate::gitx::diff::index_to_worktree(&repo)
            .map_err(|e| ChangesError::internal(format!("unstaged diff failed: {e}")))?;
        for (p, v) in diff_stats(&repo, &workdir, &d, &wanted_unstaged) {
            stats.insert(("unstaged", p), v);
        }
    }
    if let Some(d) = &base_diff {
        for (p, v) in diff_stats(&repo, &workdir, d, &wanted_base) {
            stats.insert(("base", p), v);
        }
    }
    for r in rows.iter_mut() {
        match r.group {
            "untracked" if !r.dir => {
                let full = workdir.join(&r.path);
                if let Ok(meta) = std::fs::symlink_metadata(&full) {
                    if meta.is_file() && meta.len() <= MAX_BLOB_BYTES {
                        if let Ok(bytes) = std::fs::read(&full) {
                            if looks_binary(&bytes) {
                                r.binary = true;
                            } else {
                                r.additions = Some(count_lines(&bytes) as u32);
                                r.deletions = Some(0);
                            }
                        }
                    }
                }
            }
            "staged" | "unstaged" | "base" => {
                if let Some((a, d, bin)) = stats.get(&(r.group, r.path.clone())) {
                    r.additions = *a;
                    r.deletions = *d;
                    r.binary = *bin;
                }
            }
            _ => {}
        }
    }

    Ok(SnapshotResult::Repo(Box::new(Snapshot {
        is_repo: true,
        workspace_path: ws,
        repo_root: trim_slash(&workdir.to_string_lossy()),
        rev,
        branch,
        head_oid,
        detached,
        unborn,
        upstream,
        ahead,
        behind,
        state: crate::gitx::state_name(&repo),
        base: base_info,
        counts,
        truncated,
        limit: MAX_FILES,
        files: rows,
    })))
}

// ── Both-sides diff ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffMode {
    /// HEAD → working tree.
    Worktree,
    /// index → working tree.
    Unstaged,
    /// HEAD → index.
    Staged,
    /// merge-base(ref, HEAD) → working tree.
    Base(String),
}

impl DiffMode {
    pub fn parse(mode: Option<&str>, staged: bool, base: Option<&str>) -> Result<Self, ChangesError> {
        if let Some(b) = base.filter(|b| !b.trim().is_empty()) {
            return Ok(DiffMode::Base(b.to_string()));
        }
        if staged {
            return Ok(DiffMode::Staged);
        }
        match mode.unwrap_or("worktree") {
            "" | "worktree" | "head" => Ok(DiffMode::Worktree),
            "unstaged" | "index" => Ok(DiffMode::Unstaged),
            "staged" => Ok(DiffMode::Staged),
            other => Err(ChangesError::new(
                codes::BAD_REQUEST,
                format!("unknown mode '{other}' (worktree, unstaged, staged; or base=<ref>)"),
            )),
        }
    }
    fn wire(&self) -> &'static str {
        match self {
            DiffMode::Worktree => "worktree",
            DiffMode::Unstaged => "unstaged",
            DiffMode::Staged => "staged",
            DiffMode::Base(_) => "base",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffSide {
    pub label: String,
    pub exists: bool,
    pub text: Option<String>,
    pub size: u64,
    pub binary: bool,
    pub too_large: bool,
}

impl DiffSide {
    fn missing(label: &str) -> Self {
        Self { label: label.to_string(), exists: false, text: Some(String::new()), size: 0, binary: false, too_large: false }
    }
    fn too_large(label: &str, size: u64) -> Self {
        Self { label: label.to_string(), exists: true, text: None, size, binary: false, too_large: true }
    }
    fn from_bytes(label: &str, bytes: &[u8]) -> Self {
        let size = bytes.len() as u64;
        if size > MAX_BLOB_BYTES {
            return Self::too_large(label, size);
        }
        if looks_binary(bytes) {
            return Self { label: label.to_string(), exists: true, text: None, size, binary: true, too_large: false };
        }
        if count_lines(bytes) > MAX_LINES {
            return Self::too_large(label, size);
        }
        let text = String::from_utf8_lossy(bytes).into_owned();
        Self { label: label.to_string(), exists: true, text: Some(text), size, binary: false, too_large: false }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffSides {
    pub path: String,
    pub old_path: Option<String>,
    pub mode: &'static str,
    pub base_ref: Option<String>,
    pub rev: String,
    pub old: DiffSide,
    pub new: DiffSide,
    pub binary: bool,
    pub too_large: bool,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

fn blob_side(repo: &gix::Repository, label: &str, oid: gix::ObjectId) -> Result<DiffSide, ChangesError> {
    let size = crate::gitx::object_size(repo, oid).ok_or_else(|| ChangesError::internal(format!("read blob {oid}")))?;
    if size > MAX_BLOB_BYTES {
        return Ok(DiffSide::too_large(label, size));
    }
    let bytes = crate::gitx::blob_bytes(repo, oid).ok_or_else(|| ChangesError::internal(format!("read blob {oid}")))?;
    Ok(DiffSide::from_bytes(label, &bytes))
}

fn tree_side(repo: &gix::Repository, tree: Option<gix::ObjectId>, rel: &str, label: &str) -> Result<DiffSide, ChangesError> {
    let Some(tree) = tree else { return Ok(DiffSide::missing(label)) };
    match crate::gitx::tree_entry(repo, tree, rel) {
        Some((kind, id)) if kind.is_blob() || kind == crate::gitx::EntryKind::Link => blob_side(repo, label, id),
        Some((_, id)) => Ok(DiffSide {
            // A submodule (commit) or tree: show its id as text.
            label: label.to_string(),
            exists: true,
            text: Some(format!("{id}\n")),
            size: 0,
            binary: false,
            too_large: false,
        }),
        None => Ok(DiffSide::missing(label)),
    }
}

fn index_side(repo: &gix::Repository, rel: &str) -> Result<DiffSide, ChangesError> {
    let index = repo.index_or_empty().map_err(|e| ChangesError::internal(format!("read index: {e}")))?;
    match crate::gitx::index_entry(&index, rel) {
        Some((_, id)) => blob_side(repo, "INDEX", id),
        None => Ok(DiffSide::missing("INDEX")),
    }
}

fn worktree_side(workdir: &Path, rel: &str) -> Result<DiffSide, ChangesError> {
    let full = workdir.join(rel);
    ensure_inside(workdir, &full)?;
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(m) => m,
        Err(_) => return Ok(DiffSide::missing("WORKTREE")),
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&full).map_err(|e| ChangesError::internal(format!("read link: {e}")))?;
        return Ok(DiffSide::from_bytes("WORKTREE", target.to_string_lossy().as_bytes()));
    }
    if meta.is_dir() {
        return Err(ChangesError::new(codes::BAD_REQUEST, format!("'{rel}' is a directory")));
    }
    if meta.len() > MAX_BLOB_BYTES {
        return Ok(DiffSide::too_large("WORKTREE", meta.len()));
    }
    let bytes = std::fs::read(&full).map_err(|e| ChangesError::internal(format!("read {rel}: {e}")))?;
    Ok(DiffSide::from_bytes("WORKTREE", &bytes))
}

/// Both full sides of one file (see the contract table for `mode`).
pub fn diff_sides(path: &str, file_path: &str, old_path: Option<&str>, mode: DiffMode) -> Result<DiffSides, ChangesError> {
    let (repo, workdir) = open_gix(path)?;
    let rel = normalize_rel_path(&workdir, file_path)?;
    let old_rel = match old_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => Some(normalize_rel_path(&workdir, p)?),
        None => None,
    };
    let rev = current_rev(&rev_key(&workdir));
    let head_tree = crate::gitx::head_tree_id(&repo);
    let tree_rel = old_rel.clone().unwrap_or_else(|| rel.clone());

    let (old, new, base_ref) = match &mode {
        DiffMode::Worktree => (tree_side(&repo, head_tree, &tree_rel, "HEAD")?, worktree_side(&workdir, &rel)?, None),
        DiffMode::Unstaged => (index_side(&repo, &rel)?, worktree_side(&workdir, &rel)?, None),
        DiffMode::Staged => (tree_side(&repo, head_tree, &tree_rel, "HEAD")?, index_side(&repo, &rel)?, None),
        DiffMode::Base(spec) => {
            let bad_ref = |e: String| ChangesError::new(codes::BAD_REF, format!("cannot resolve base '{spec}': {e}"));
            crate::git::refuse_message_search_revspec(spec).map_err(|m| ChangesError::new(codes::BAD_REF, m))?;
            let base_commit = crate::gitx::resolve_commit(&repo, spec).map_err(bad_ref)?;
            let head_id = crate::gitx::head_info(&repo).oid;
            let tree_commit = head_id.and_then(|h| crate::gitx::merge_base(&repo, h, base_commit)).unwrap_or(base_commit);
            let tree = crate::gitx::commit_tree_id(&repo, tree_commit)
                .ok_or_else(|| ChangesError::internal(format!("read the tree of {tree_commit}")))?;
            (tree_side(&repo, Some(tree), &tree_rel, spec)?, worktree_side(&workdir, &rel)?, Some(spec.clone()))
        }
    };
    if !old.exists && !new.exists {
        return Err(ChangesError::new(codes::NOT_FOUND, format!("'{rel}' exists on neither side")));
    }
    let (mut additions, mut deletions) = (None, None);
    if let (Some(a), Some(b)) = (&old.text, &new.text) {
        let (adds, dels) = crate::gitx::diff::line_stats(a.as_bytes(), b.as_bytes());
        additions = Some(adds);
        deletions = Some(dels);
    }
    Ok(DiffSides {
        path: rel,
        old_path: old_rel,
        mode: mode.wire(),
        base_ref,
        rev,
        binary: old.binary || new.binary,
        too_large: old.too_large || new.too_large,
        old,
        new,
        additions,
        deletions,
    })
}

// ── Discard ──────────────────────────────────────────────────────────

static TRASH_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Tests send discarded files to `dir` instead of the OS Trash.
/// `K2_GIT_TRASH_DIR` does the same for a whole daemon process (smoke /
/// headless runs), like `K2_HIRE_TRASH_DIR`.
pub fn set_trash_override(dir: Option<PathBuf>) {
    *TRASH_OVERRIDE.write().unwrap_or_else(|p| p.into_inner()) = dir;
}

fn trash_dir_override() -> Option<PathBuf> {
    if let Some(d) = TRASH_OVERRIDE.read().unwrap_or_else(|p| p.into_inner()).clone() {
        return Some(d);
    }
    std::env::var_os("K2_GIT_TRASH_DIR").filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Move `path` to the Trash (or the override dir). Never unlinks.
pub fn trash_path(path: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    if let Some(dir) = trash_dir_override() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("trash dir {}: {e}", dir.display()))?;
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let dest = dir.join(format!("{name}-{}", uuid::Uuid::new_v4().simple()));
        return std::fs::rename(path, &dest).map_err(|e| format!("move {} to {}: {e}", path.display(), dest.display()));
    }
    crate::safe_delete::trash(path)
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct DiscardSkip {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct DiscardFail {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct DiscardOutcome {
    pub restored: Vec<String>,
    pub trashed: Vec<String>,
    pub unstaged: Vec<String>,
    pub skipped: Vec<DiscardSkip>,
    pub failed: Vec<DiscardFail>,
    /// Repo rev key (the route bumps it after a discard).
    #[serde(skip)]
    pub rev_key: String,
}

enum Action {
    TrashUntracked,
    RestoreFromIndex,
    ResetToHead,
    RemoveFromIndex,
}

/// Throw away working-tree changes for `files` (see the contract table).
/// `trash` sends one path to the Trash; production passes [`trash_path`].
pub fn discard(
    path: &str,
    files: &[String],
    include_staged: bool,
    trash: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<DiscardOutcome, ChangesError> {
    if files.is_empty() {
        return Err(ChangesError::new(codes::BAD_REQUEST, "no files to discard"));
    }
    if files.len() > MAX_DISCARD_FILES {
        return Err(ChangesError::new(
            codes::BAD_REQUEST,
            format!("too many files ({} > {MAX_DISCARD_FILES})", files.len()),
        ));
    }
    let (repo, workdir) = open_gix(path)?;
    let mut out = DiscardOutcome { rev_key: rev_key(&workdir), ..Default::default() };

    // ── validate everything first (all-or-nothing) ──
    let mut seen = HashSet::new();
    let mut rels = Vec::new();
    for f in files {
        let rel = normalize_rel_path(&workdir, f)?;
        ensure_inside(&workdir, &workdir.join(&rel))?;
        if seen.insert(rel.clone()) {
            rels.push(rel);
        }
    }
    let head_tree = crate::gitx::head_tree_id(&repo);
    let index = repo.index_or_empty().map_err(|e| ChangesError::internal(format!("read index: {e}")))?;
    let opts = gstatus::Options { untracked: gstatus::Untracked::Files, staged_renames: false, submodules: true };
    let by_path: HashMap<String, Flags> = gstatus::statuses(&repo, opts)
        .map_err(|e| ChangesError::internal(format!("git status failed: {e}")))?
        .into_iter()
        .map(|r| (r.path, r.flags))
        .collect();
    let mut excludes = crate::gitx::Excludes::new(repo.clone());
    let mut ignored = |rel: &str, is_dir: bool| excludes.as_mut().is_some_and(|x| x.is_ignored(rel, is_dir));
    let in_index = |rel: &str| index.entries().iter().any(|e| e.path(&index) == rel.as_bytes());
    let mut plan: Vec<(String, Action)> = Vec::new();
    let mut conflicted = Vec::new();
    for rel in rels {
        let full = workdir.join(&rel);
        let meta = std::fs::symlink_metadata(&full).ok();
        if meta.as_ref().is_some_and(|m| m.is_dir()) {
            // A submodule's folder: its gitlink is the index entry itself.
            if in_index(&rel) {
                out.skipped.push(DiscardSkip { path: rel, reason: "submodule".into() });
                continue;
            }
            let prefix = format!("{rel}/");
            let tracked = index.entries().iter().any(|e| e.path(&index).starts_with(prefix.as_bytes()));
            if tracked {
                return Err(ChangesError::new(
                    codes::BAD_REQUEST,
                    format!("'{rel}' is a directory with tracked files; name the files instead"),
                ));
            }
            if ignored(&rel, true) {
                out.skipped.push(DiscardSkip { path: rel, reason: "ignored".into() });
            } else {
                plan.push((rel, Action::TrashUntracked));
            }
            continue;
        }
        let Some(status) = by_path.get(&rel).copied() else {
            // Not in the status: clean, ignored, or not there at all.
            let tracked = in_index(&rel);
            let reason = if tracked {
                "nothing to discard"
            } else if meta.is_none() {
                "not found"
            } else if ignored(&rel, false) {
                "ignored"
            } else {
                "nothing to discard"
            };
            out.skipped.push(DiscardSkip { path: rel, reason: reason.into() });
            continue;
        };
        if status.contains(Flags::CONFLICTED) {
            conflicted.push(rel);
            continue;
        }
        if status.contains(Flags::WT_NEW) {
            plan.push((rel, Action::TrashUntracked));
            continue;
        }
        let wt_dirty = status.intersects(Flags::WT_MODIFIED | Flags::WT_DELETED | Flags::WT_TYPECHANGE);
        if !include_staged {
            if wt_dirty {
                plan.push((rel, Action::RestoreFromIndex));
            } else {
                out.skipped.push(DiscardSkip { path: rel, reason: "nothing to discard".into() });
            }
            continue;
        }
        match head_tree.and_then(|t| crate::gitx::tree_entry(&repo, t, &rel)) {
            Some((crate::gitx::EntryKind::Commit, _)) => {
                out.skipped.push(DiscardSkip { path: rel, reason: "submodule".into() });
            }
            Some(_) => plan.push((rel, Action::ResetToHead)),
            None => plan.push((rel, Action::RemoveFromIndex)),
        }
    }
    if !conflicted.is_empty() {
        return Err(ChangesError::new(
            codes::CONFLICTED,
            format!("resolve the merge conflict first: {}", conflicted.join(", ")),
        ));
    }
    // Restoring and unstaging run the person's git: refuse before anything
    // moves to the Trash when it isn't installed.
    if plan.iter().any(|(_, a)| !matches!(a, Action::TrashUntracked)) {
        crate::git_safe::require_git().map_err(|e| {
            let hint = e.strip_prefix(&format!("{}: ", crate::git::refusal::GIT_NOT_INSTALLED)).unwrap_or(&e).to_string();
            ChangesError::new(codes::GIT_NOT_INSTALLED, hint)
        })?;
        crate::git_write_guard::check(&workdir).map_err(|e| {
            let hint = e.strip_prefix(&format!("{}: ", crate::git::refusal::GIT_CONFIG_RUNS_PROGRAMS)).unwrap_or(&e).to_string();
            ChangesError::new(codes::GIT_CONFIG_RUNS_PROGRAMS, hint)
        })?;
    }

    // ── act ──
    // Every working copy that would be lost is in the Trash before git
    // writes; git then restores from the index (or HEAD) and edits its own
    // index.
    let git = |what: &str, args: &[&str]| crate::git_safe::run_write_ok(&workdir, what, args).map(|_| ());
    for (rel, action) in plan {
        let full = workdir.join(&rel);
        let exists = std::fs::symlink_metadata(&full).is_ok();
        let fail = |out: &mut DiscardOutcome, e: String| out.failed.push(DiscardFail { path: rel.clone(), error: e });
        let trash_first = |out: &mut DiscardOutcome| -> bool {
            if !exists {
                return true;
            }
            match trash(&full) {
                Ok(()) => {
                    out.trashed.push(rel.clone());
                    true
                }
                Err(e) => {
                    out.failed.push(DiscardFail { path: rel.clone(), error: e });
                    false
                }
            }
        };
        match action {
            Action::TrashUntracked => match trash(&full) {
                Ok(()) => out.trashed.push(rel.clone()),
                Err(e) => fail(&mut out, e),
            },
            Action::RestoreFromIndex => {
                if !trash_first(&mut out) {
                    continue;
                }
                match git(&format!("checkout {rel}"), &["checkout", "-q", "--", &rel]) {
                    Ok(()) => out.restored.push(rel.clone()),
                    Err(e) => fail(&mut out, e),
                }
            }
            Action::ResetToHead => {
                if !trash_first(&mut out) {
                    continue;
                }
                let head_id = head_tree.and_then(|t| crate::gitx::tree_entry(&repo, t, &rel)).map(|e| e.1);
                let staged = crate::gitx::index_entry(&index, &rel).map(|e| e.1) != head_id;
                match git(&format!("checkout HEAD {rel}"), &["checkout", "-q", "HEAD", "--", &rel]) {
                    Ok(()) => {
                        if staged {
                            out.unstaged.push(rel.clone());
                        }
                        out.restored.push(rel.clone());
                    }
                    Err(e) => fail(&mut out, e),
                }
            }
            Action::RemoveFromIndex => {
                if !trash_first(&mut out) {
                    continue;
                }
                match git(&format!("unstage {rel}"), &["rm", "-q", "--cached", "--ignore-unmatch", "--", &rel]) {
                    Ok(()) => out.unstaged.push(rel.clone()),
                    Err(e) => fail(&mut out, e),
                }
            }
        }
    }
    Ok(out)
}

// ── Watcher helpers ──────────────────────────────────────────────────

/// A repo's on-disk dirs, each in its given and canonical spelling (the
/// file watcher reports real paths, e.g. `/private/var/…` on macOS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoDirs {
    pub workdir: PathBuf,
    /// This checkout's git dir (`<repo>/.git` or `<main>/.git/worktrees/<name>`).
    pub gitdir: PathBuf,
    /// The shared git dir (refs, packed-refs). Equals `gitdir` for a main checkout.
    pub commondir: PathBuf,
    workdir_forms: Vec<String>,
    gitdir_forms: Vec<String>,
    commondir_forms: Vec<String>,
}

/// Every spelling of `p`: as given, canonical, and — when it sits under
/// the canonical `alias.0` — re-rooted at `alias.1` (the workspace path as
/// the caller spells it; git may report `/private/var/…` for `/var/…`).
fn forms(p: &Path, alias: Option<&(String, String)>) -> Vec<String> {
    let mut v = forms_raw(p, alias);
    // Windows: one '/' spelling (classify_git_path and worktree_rel fold
    // the path they are given the same way).
    if cfg!(windows) {
        let mut out: Vec<String> = Vec::new();
        for f in v.drain(..) {
            let f = f.replace('\\', "/");
            if !out.contains(&f) {
                out.push(f);
            }
        }
        v = out;
    }
    v
}

fn forms_raw(p: &Path, alias: Option<&(String, String)>) -> Vec<String> {
    let mut v = vec![trim_slash(&p.to_string_lossy())];
    let mut push = |s: String| {
        if !v.contains(&s) {
            v.push(s);
        }
    };
    if let Ok(c) = dunce::canonicalize(&p) {
        let c = trim_slash(&c.to_string_lossy());
        if let Some((canon_root, raw_root)) = alias {
            if c == *canon_root {
                push(raw_root.clone());
            } else if let Some(rest) = c.strip_prefix(&format!("{canon_root}/")) {
                push(format!("{raw_root}/{rest}"));
            }
        }
        push(c);
    }
    v
}

impl RepoDirs {
    pub fn workdir_forms(&self) -> &[String] {
        &self.workdir_forms
    }
}

/// The dirs of the non-bare repo holding `path`.
pub fn repo_dirs(path: &str) -> Option<RepoDirs> {
    let repo = crate::gitx::discover(path).ok()?;
    let workdir = repo.workdir()?.to_path_buf();
    // Canonical, as libgit2 gave them: a main checkout and its linked
    // worktree then name the shared git dir the same way, so a ref move
    // reported in that spelling reaches both (on macOS /var is /private/var).
    let canon = |p: &Path| dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let gitdir = canon(repo.git_dir());
    // The shared git dir (a linked worktree's main `.git`; the git dir
    // itself for a main checkout).
    let commondir = canon(repo.common_dir());
    let alias = dunce::canonicalize(&path)
        .ok()
        .map(|c| (trim_slash(&c.to_string_lossy()), trim_slash(path)))
        .filter(|(c, raw)| c != raw);
    Some(RepoDirs {
        workdir_forms: forms(&workdir, alias.as_ref()),
        gitdir_forms: forms(&gitdir, alias.as_ref()),
        commondir_forms: forms(&commondir, alias.as_ref()),
        workdir,
        gitdir,
        commondir,
    })
}

/// What a `.git` path change means for one checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSignal {
    /// This checkout's index / HEAD / merge state moved.
    Local,
    /// A shared ref moved (refs/**, packed-refs): every checkout of the repo.
    Shared,
}

/// Local-signal files directly under a checkout's git dir.
const LOCAL_FILES: &[&str] = &["index", "HEAD", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"];

fn strip_under<'a>(path: &'a str, dir_forms: &[String]) -> Option<&'a str> {
    dir_forms.iter().find_map(|d| path.strip_prefix(d.as_str()).and_then(|r| r.strip_prefix('/')))
}

/// Classify one changed path against a checkout's dirs. `.lock` files
/// count as their target (git writes `index.lock` then renames it).
pub fn classify_git_path(path: &str, dirs: &RepoDirs) -> Option<GitSignal> {
    let p = path.replace('\\', "/");
    let p = p.strip_suffix(".lock").unwrap_or(&p);
    if let Some(rest) = strip_under(p, &dirs.gitdir_forms) {
        if LOCAL_FILES.contains(&rest) {
            return Some(GitSignal::Local);
        }
    }
    if let Some(rest) = strip_under(p, &dirs.commondir_forms) {
        if rest == "packed-refs" || rest.starts_with("refs/") {
            return Some(GitSignal::Shared);
        }
    }
    None
}

/// True when `path` is inside any `.git` dir (a git-internal path that is
/// never a working-tree edit).
pub fn is_git_internal(path: &str) -> bool {
    let p = path.replace('\\', "/");
    p.contains("/.git/") || p.ends_with("/.git")
}

/// The working-tree path relative to `dirs.workdir`, when inside it.
pub fn worktree_rel<'a>(path: &'a str, dirs: &RepoDirs) -> Option<std::borrow::Cow<'a, str>> {
    use std::borrow::Cow;
    match fold_slashes(path) {
        Cow::Borrowed(p) => strip_under(p, &dirs.workdir_forms).filter(|r| !r.is_empty()).map(Cow::Borrowed),
        Cow::Owned(p) => strip_under(&p, &dirs.workdir_forms).filter(|r| !r.is_empty()).map(|r| Cow::Owned(r.to_string())),
    }
}

/// `path` with Windows' '\' folded to '/' (gix and the dir forms use
/// '/' there; a file watcher reports '\'). Unchanged elsewhere.
fn fold_slashes(path: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(windows) && path.contains('\\') {
        std::borrow::Cow::Owned(path.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(path)
    }
}

#[cfg(test)]
#[path = "git_changes_tests.rs"]
mod tests;
