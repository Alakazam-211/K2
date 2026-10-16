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

use git2::{DiffFindOptions, DiffOptions, Oid, Repository, RepositoryState, Status, StatusOptions};
use serde::Serialize;

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
    let canon = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
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
/// `repo_root` is the workdir as git2 reports it, no trailing `/`.
pub fn repo_key_and_root(path: &str) -> Option<(String, String)> {
    let repo = Repository::discover(path).ok()?;
    let workdir = repo.workdir()?;
    Some((rev_key(workdir), trim_slash(&workdir.to_string_lossy())))
}

/// Answers "is this working-tree path git-ignored?" for one repo, so the
/// watcher can drop edits to build output without re-opening the repo
/// per path.
pub struct IgnoreChecker(Repository);

impl IgnoreChecker {
    pub fn open(workdir: &Path) -> Option<Self> {
        Repository::open(workdir).ok().map(Self)
    }
    /// `rel` is repo-relative. Errors count as not ignored.
    pub fn is_ignored(&self, rel: &str) -> bool {
        self.0.is_path_ignored(rel).unwrap_or(false)
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
        let canon_wd = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
        if let Ok(r) = p.strip_prefix(workdir) {
            r.to_path_buf()
        } else if let Ok(r) = p.strip_prefix(&canon_wd) {
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

/// Refuse a working-tree path whose real location (after resolving
/// symlinked parent directories) is outside the repo. The leaf itself is
/// never followed by callers.
fn ensure_inside(workdir: &Path, full: &Path) -> Result<(), ChangesError> {
    let canon_wd = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    // Nearest existing ancestor of the leaf's parent.
    let mut probe = full.parent().map(Path::to_path_buf);
    while let Some(dir) = probe {
        if let Ok(real) = std::fs::canonicalize(&dir) {
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

fn open_repo(path: &str) -> Result<(Repository, PathBuf), ChangesError> {
    let repo = Repository::discover(path)
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

fn state_name(s: RepositoryState) -> &'static str {
    match s {
        RepositoryState::Clean => "clean",
        RepositoryState::Merge => "merge",
        RepositoryState::Revert | RepositoryState::RevertSequence => "revert",
        RepositoryState::CherryPick | RepositoryState::CherryPickSequence => "cherry_pick",
        RepositoryState::Bisect => "bisect",
        RepositoryState::Rebase | RepositoryState::RebaseInteractive | RepositoryState::RebaseMerge => "rebase",
        RepositoryState::ApplyMailbox | RepositoryState::ApplyMailboxOrRebase => "apply_mailbox",
    }
}

fn delta_letter(d: git2::Delta) -> &'static str {
    match d {
        git2::Delta::Added | git2::Delta::Copied => "A",
        git2::Delta::Deleted => "D",
        git2::Delta::Renamed => "R",
        git2::Delta::Typechange => "T",
        git2::Delta::Conflicted => "C",
        git2::Delta::Untracked => "U",
        _ => "M",
    }
}

/// `(additions, deletions, binary)` per new-side path of `diff`, for the
/// paths in `wanted` only (patches are built lazily; big blobs skipped).
fn diff_stats(diff: &git2::Diff<'_>, wanted: &HashSet<String>) -> HashMap<String, (Option<u32>, Option<u32>, bool)> {
    let mut out = HashMap::new();
    for i in 0..diff.deltas().len() {
        let Some(delta) = diff.get_delta(i) else { continue };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if !wanted.contains(&path) {
            continue;
        }
        if delta.old_file().size() > MAX_BLOB_BYTES || delta.new_file().size() > MAX_BLOB_BYTES {
            out.insert(path, (None, None, false));
            continue;
        }
        match git2::Patch::from_diff(diff, i) {
            Ok(Some(patch)) => {
                let binary = patch.delta().flags().is_binary();
                if binary {
                    out.insert(path, (None, None, true));
                } else {
                    let (_, adds, dels) = patch.line_stats().unwrap_or((0, 0, 0));
                    out.insert(path, (Some(adds as u32), Some(dels as u32), false));
                }
            }
            _ => {
                out.insert(path, (None, None, delta.flags().is_binary()));
            }
        }
    }
    out
}

fn new_diff_opts() -> DiffOptions {
    let mut o = DiffOptions::new();
    o.context_lines(0).include_typechange(true);
    o
}

fn find_renames(diff: &mut git2::Diff<'_>) {
    let mut fo = DiffFindOptions::new();
    fo.renames(true);
    let _ = diff.find_similar(Some(&mut fo));
}

/// The Changes snapshot for the workspace at `path` (see the contract).
pub fn snapshot(path: &str, base: Option<&str>) -> Result<SnapshotResult, ChangesError> {
    let ws = trim_slash(path);
    let not_repo = |rev: String| SnapshotResult::NotRepo(NotRepo { is_repo: false, workspace_path: ws.clone(), rev });
    let Ok(repo) = Repository::discover(path) else {
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
    let (mut branch, mut head_oid, mut unborn) = (None, None, false);
    let mut head_commit: Option<git2::Commit<'_>> = None;
    match repo.head() {
        Ok(h) => {
            head_oid = h.target().map(|o| o.to_string());
            if h.is_branch() {
                branch = h.shorthand().map(str::to_string);
            }
            head_commit = h.peel_to_commit().ok();
        }
        Err(_) => {
            unborn = true;
            if let Ok(r) = repo.find_reference("HEAD") {
                branch = r.symbolic_target().map(|t| t.trim_start_matches("refs/heads/").to_string());
            }
        }
    }
    let detached = repo.head_detached().unwrap_or(false);
    let (mut upstream, mut ahead, mut behind) = (None, 0usize, 0usize);
    if let (Some(name), false) = (branch.as_deref(), unborn) {
        if let Ok(local) = repo.find_branch(name, git2::BranchType::Local) {
            if let Ok(up) = local.upstream() {
                upstream = up.name().ok().flatten().map(str::to_string);
                if let (Some(l), Some(u)) = (local.get().target(), up.get().target()) {
                    if let Ok((a, b)) = repo.graph_ahead_behind(l, u) {
                        ahead = a;
                        behind = b;
                    }
                }
            }
        }
    }

    // ── base ──
    let mut base_info: Option<BaseInfo> = None;
    let mut base_tree: Option<git2::Tree<'_>> = None;
    if let Some(spec) = base.filter(|s| !s.trim().is_empty()) {
        let bad_ref = |e: String| ChangesError::new(codes::BAD_REF, format!("cannot resolve base '{spec}': {e}"));
        let obj = repo.revparse_single(spec).map_err(|e| bad_ref(e.to_string()))?;
        let base_commit = obj.peel_to_commit().map_err(|e| bad_ref(e.to_string()))?;
        let base_oid = base_commit.id();
        let (merge_base, a, b) = match &head_commit {
            Some(hc) => {
                let mb = repo.merge_base(hc.id(), base_oid).ok();
                let (a, b) = repo.graph_ahead_behind(hc.id(), base_oid).unwrap_or((0, 0));
                (mb, a, b)
            }
            None => (None, 0, 0),
        };
        // No common ancestor (unrelated histories / unborn HEAD): compare
        // against the base commit itself.
        let tree_commit = match merge_base {
            Some(mb) => repo.find_commit(mb).map_err(|e| ChangesError::internal(e.to_string()))?,
            None => base_commit.clone(),
        };
        base_tree = Some(tree_commit.tree().map_err(|e| ChangesError::internal(e.to_string()))?);
        base_info = Some(BaseInfo {
            ref_name: spec.to_string(),
            oid: base_oid.to_string(),
            merge_base: merge_base.map(|o| o.to_string()),
            ahead: a,
            behind: b,
        });
    }

    // ── status rows ──
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .renames_head_to_index(true);
    let statuses = repo
        .statuses(Some(&mut opts))
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
    for entry in statuses.iter() {
        let s = entry.status();
        let path = entry.path().unwrap_or("").replace('\\', "/");
        if s.is_conflicted() {
            rows.push(row(path, None, "conflicted", "C", false));
            continue;
        }
        if s.is_wt_new() {
            let dir = path.ends_with('/');
            rows.push(row(path, None, "untracked", "U", dir));
            continue;
        }
        if base_tree.is_some() {
            continue; // base view lists tracked paths from the base diff below
        }
        let staged = if s.is_index_new() {
            Some("A")
        } else if s.is_index_deleted() {
            Some("D")
        } else if s.is_index_renamed() {
            Some("R")
        } else if s.is_index_typechange() {
            Some("T")
        } else if s.is_index_modified() {
            Some("M")
        } else {
            None
        };
        if let Some(letter) = staged {
            let (p, old) = match (letter, entry.head_to_index()) {
                ("R", Some(d)) => (
                    d.new_file().path().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or(path.clone()),
                    d.old_file().path().map(|p| p.to_string_lossy().replace('\\', "/")),
                ),
                _ => (path.clone(), None),
            };
            rows.push(row(p, old, "staged", letter, false));
        }
        let unstaged = if s.is_wt_deleted() {
            Some("D")
        } else if s.is_wt_typechange() {
            Some("T")
        } else if s.is_wt_modified() || s.is_wt_renamed() {
            Some("M")
        } else {
            None
        };
        if let Some(letter) = unstaged {
            // A staged rename's worktree edit lives at the new path.
            let p = match entry.index_to_workdir() {
                Some(d) => d.new_file().path().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or(path),
                None => path,
            };
            rows.push(row(p, None, "unstaged", letter, false));
        }
    }

    // Base view: tracked paths that differ between merge-base and the
    // working tree (committed + staged + unstaged).
    let mut base_diff: Option<git2::Diff<'_>> = None;
    if let Some(tree) = &base_tree {
        let mut o = new_diff_opts();
        let mut diff = repo
            .diff_tree_to_workdir_with_index(Some(tree), Some(&mut o))
            .map_err(|e| ChangesError::internal(format!("base diff failed: {e}")))?;
        find_renames(&mut diff);
        let conflicted: HashSet<String> =
            rows.iter().filter(|r| r.group == "conflicted").map(|r| r.path.clone()).collect();
        for d in diff.deltas() {
            let p = d
                .new_file()
                .path()
                .or_else(|| d.old_file().path())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if conflicted.contains(&p) || d.status() == git2::Delta::Untracked {
                continue;
            }
            let old = (d.status() == git2::Delta::Renamed)
                .then(|| d.old_file().path().map(|p| p.to_string_lossy().replace('\\', "/")))
                .flatten();
            rows.push(row(p, old, "base", delta_letter(d.status()), false));
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
    if !wanted_staged.is_empty() || !wanted_unstaged.is_empty() {
        let index = repo.index().map_err(|e| ChangesError::internal(format!("read index: {e}")))?;
        if !wanted_staged.is_empty() {
            let head_tree = head_commit.as_ref().and_then(|c| c.tree().ok());
            let mut o = new_diff_opts();
            let mut d = repo
                .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut o))
                .map_err(|e| ChangesError::internal(format!("staged diff failed: {e}")))?;
            find_renames(&mut d);
            for (p, v) in diff_stats(&d, &wanted_staged) {
                stats.insert(("staged", p), v);
            }
        }
        if !wanted_unstaged.is_empty() {
            let mut o = new_diff_opts();
            let d = repo
                .diff_index_to_workdir(Some(&index), Some(&mut o))
                .map_err(|e| ChangesError::internal(format!("unstaged diff failed: {e}")))?;
            for (p, v) in diff_stats(&d, &wanted_unstaged) {
                stats.insert(("unstaged", p), v);
            }
        }
    }
    if let Some(d) = &base_diff {
        for (p, v) in diff_stats(d, &wanted_base) {
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
        state: state_name(repo.state()),
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

fn blob_side(repo: &Repository, label: &str, oid: Oid) -> Result<DiffSide, ChangesError> {
    let blob = repo.find_blob(oid).map_err(|e| ChangesError::internal(format!("read blob: {e}")))?;
    if blob.size() as u64 > MAX_BLOB_BYTES {
        return Ok(DiffSide::too_large(label, blob.size() as u64));
    }
    Ok(DiffSide::from_bytes(label, blob.content()))
}

fn tree_side(repo: &Repository, tree: Option<&git2::Tree<'_>>, rel: &str, label: &str) -> Result<DiffSide, ChangesError> {
    let Some(tree) = tree else { return Ok(DiffSide::missing(label)) };
    match tree.get_path(Path::new(rel)) {
        Ok(entry) if entry.kind() == Some(git2::ObjectType::Blob) => blob_side(repo, label, entry.id()),
        Ok(entry) => Ok(DiffSide {
            // A submodule (commit) or tree: show its id as text.
            label: label.to_string(),
            exists: true,
            text: Some(format!("{}\n", entry.id())),
            size: 0,
            binary: false,
            too_large: false,
        }),
        Err(_) => Ok(DiffSide::missing(label)),
    }
}

fn index_side(repo: &Repository, rel: &str) -> Result<DiffSide, ChangesError> {
    let index = repo.index().map_err(|e| ChangesError::internal(format!("read index: {e}")))?;
    match index.get_path(Path::new(rel), 0) {
        Some(e) => blob_side(repo, "INDEX", e.id),
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
    let (repo, workdir) = open_repo(path)?;
    let rel = normalize_rel_path(&workdir, file_path)?;
    let old_rel = match old_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => Some(normalize_rel_path(&workdir, p)?),
        None => None,
    };
    let rev = current_rev(&rev_key(&workdir));
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let tree_rel = old_rel.clone().unwrap_or_else(|| rel.clone());

    let (old, new, base_ref) = match &mode {
        DiffMode::Worktree => (tree_side(&repo, head_tree.as_ref(), &tree_rel, "HEAD")?, worktree_side(&workdir, &rel)?, None),
        DiffMode::Unstaged => (index_side(&repo, &rel)?, worktree_side(&workdir, &rel)?, None),
        DiffMode::Staged => (tree_side(&repo, head_tree.as_ref(), &tree_rel, "HEAD")?, index_side(&repo, &rel)?, None),
        DiffMode::Base(spec) => {
            let bad_ref = |e: String| ChangesError::new(codes::BAD_REF, format!("cannot resolve base '{spec}': {e}"));
            let base_commit = repo
                .revparse_single(spec)
                .map_err(|e| bad_ref(e.to_string()))?
                .peel_to_commit()
                .map_err(|e| bad_ref(e.to_string()))?;
            let head_id = repo.head().ok().and_then(|h| h.target());
            let tree_commit = match head_id.and_then(|h| repo.merge_base(h, base_commit.id()).ok()) {
                Some(mb) => repo.find_commit(mb).map_err(|e| ChangesError::internal(e.to_string()))?,
                None => base_commit,
            };
            let tree = tree_commit.tree().map_err(|e| ChangesError::internal(e.to_string()))?;
            (tree_side(&repo, Some(&tree), &tree_rel, spec)?, worktree_side(&workdir, &rel)?, Some(spec.clone()))
        }
    };
    if !old.exists && !new.exists {
        return Err(ChangesError::new(codes::NOT_FOUND, format!("'{rel}' exists on neither side")));
    }
    let (mut additions, mut deletions) = (None, None);
    if let (Some(a), Some(b)) = (&old.text, &new.text) {
        if let Ok(patch) = git2::Patch::from_buffers(a.as_bytes(), None, b.as_bytes(), None, None) {
            if let Ok((_, adds, dels)) = patch.line_stats() {
                additions = Some(adds as u32);
                deletions = Some(dels as u32);
            }
        }
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
    let (repo, workdir) = open_repo(path)?;
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
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let index_snapshot = repo.index().map_err(|e| ChangesError::internal(format!("read index: {e}")))?;
    let mut plan: Vec<(String, Action)> = Vec::new();
    let mut conflicted = Vec::new();
    for rel in rels {
        let full = workdir.join(&rel);
        let meta = std::fs::symlink_metadata(&full).ok();
        if meta.as_ref().is_some_and(|m| m.is_dir()) {
            let prefix = format!("{rel}/");
            let tracked = index_snapshot.iter().any(|e| String::from_utf8_lossy(&e.path).starts_with(&prefix));
            if tracked {
                return Err(ChangesError::new(
                    codes::BAD_REQUEST,
                    format!("'{rel}' is a directory with tracked files; name the files instead"),
                ));
            }
            if repo.is_path_ignored(&rel).unwrap_or(false) {
                out.skipped.push(DiscardSkip { path: rel, reason: "ignored".into() });
            } else {
                plan.push((rel, Action::TrashUntracked));
            }
            continue;
        }
        let status = match repo.status_file(Path::new(&rel)) {
            Ok(s) => s,
            Err(_) => {
                out.skipped.push(DiscardSkip { path: rel, reason: "not found".into() });
                continue;
            }
        };
        if status.is_conflicted() {
            conflicted.push(rel);
            continue;
        }
        if status.is_ignored() {
            out.skipped.push(DiscardSkip { path: rel, reason: "ignored".into() });
            continue;
        }
        if status.is_wt_new() {
            plan.push((rel, Action::TrashUntracked));
            continue;
        }
        if status.is_empty() || status == Status::CURRENT {
            out.skipped.push(DiscardSkip { path: rel, reason: "nothing to discard".into() });
            continue;
        }
        let wt_dirty = status.intersects(Status::WT_MODIFIED | Status::WT_DELETED | Status::WT_TYPECHANGE | Status::WT_RENAMED);
        if !include_staged {
            if wt_dirty {
                plan.push((rel, Action::RestoreFromIndex));
            } else {
                out.skipped.push(DiscardSkip { path: rel, reason: "nothing to discard".into() });
            }
            continue;
        }
        let in_head = head_tree.as_ref().is_some_and(|t| t.get_path(Path::new(&rel)).is_ok());
        if in_head {
            let is_submodule = head_tree
                .as_ref()
                .and_then(|t| t.get_path(Path::new(&rel)).ok())
                .is_some_and(|e| e.kind() == Some(git2::ObjectType::Commit));
            if is_submodule {
                out.skipped.push(DiscardSkip { path: rel, reason: "submodule".into() });
            } else {
                plan.push((rel, Action::ResetToHead));
            }
        } else {
            plan.push((rel, Action::RemoveFromIndex));
        }
    }
    if !conflicted.is_empty() {
        return Err(ChangesError::new(
            codes::CONFLICTED,
            format!("resolve the merge conflict first: {}", conflicted.join(", ")),
        ));
    }
    drop(index_snapshot);

    // ── act ──
    let checkout = |repo: &Repository, rel: &str| -> Result<(), String> {
        let mut cb = git2::build::CheckoutBuilder::new();
        cb.force().disable_pathspec_match(true).path(rel);
        repo.checkout_index(None, Some(&mut cb)).map_err(|e| format!("checkout {rel}: {e}"))
    };
    for (rel, action) in plan {
        let full = workdir.join(&rel);
        let exists = std::fs::symlink_metadata(&full).is_ok();
        let fail = |out: &mut DiscardOutcome, e: String| out.failed.push(DiscardFail { path: rel.clone(), error: e });
        match action {
            Action::TrashUntracked => match trash(&full) {
                Ok(()) => out.trashed.push(rel.clone()),
                Err(e) => fail(&mut out, e),
            },
            Action::RestoreFromIndex => {
                if exists {
                    if let Err(e) = trash(&full) {
                        fail(&mut out, e);
                        continue;
                    }
                    out.trashed.push(rel.clone());
                }
                match checkout(&repo, &rel) {
                    Ok(()) => out.restored.push(rel.clone()),
                    Err(e) => fail(&mut out, e),
                }
            }
            Action::ResetToHead => {
                if exists {
                    if let Err(e) = trash(&full) {
                        fail(&mut out, e);
                        continue;
                    }
                    out.trashed.push(rel.clone());
                }
                let reset = (|| -> Result<bool, String> {
                    let tree = head_tree.as_ref().ok_or("no HEAD")?;
                    let entry = tree.get_path(Path::new(&rel)).map_err(|e| e.to_string())?;
                    let mut index = crate::git::get_index_with_retry(&repo)?;
                    let changed = index.get_path(Path::new(&rel), 0).map(|e| e.id) != Some(entry.id());
                    if changed {
                        let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
                        index
                            .add(&git2::IndexEntry {
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
                                path: rel.as_bytes().to_vec(),
                            })
                            .map_err(|e| e.to_string())?;
                        crate::git::write_index_with_retry(&mut index)?;
                    }
                    Ok(changed)
                })();
                match reset {
                    Ok(changed) => {
                        if changed {
                            out.unstaged.push(rel.clone());
                        }
                        match checkout(&repo, &rel) {
                            Ok(()) => out.restored.push(rel.clone()),
                            Err(e) => fail(&mut out, e),
                        }
                    }
                    Err(e) => fail(&mut out, e),
                }
            }
            Action::RemoveFromIndex => {
                if exists {
                    if let Err(e) = trash(&full) {
                        fail(&mut out, e);
                        continue;
                    }
                    out.trashed.push(rel.clone());
                }
                let removed = crate::git::get_index_with_retry(&repo).and_then(|mut index| {
                    index.remove_path(Path::new(&rel)).map_err(|e| e.to_string())?;
                    crate::git::write_index_with_retry(&mut index)
                });
                match removed {
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

fn forms(p: &Path) -> Vec<String> {
    let mut v = vec![trim_slash(&p.to_string_lossy())];
    if let Ok(c) = std::fs::canonicalize(p) {
        let c = trim_slash(&c.to_string_lossy());
        if !v.contains(&c) {
            v.push(c);
        }
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
    let repo = Repository::discover(path).ok()?;
    let workdir = repo.workdir()?.to_path_buf();
    let gitdir = repo.path().to_path_buf();
    let commondir = repo.commondir().to_path_buf();
    Some(RepoDirs {
        workdir_forms: forms(&workdir),
        gitdir_forms: forms(&gitdir),
        commondir_forms: forms(&commondir),
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
pub fn worktree_rel<'a>(path: &'a str, dirs: &RepoDirs) -> Option<&'a str> {
    strip_under(path, &dirs.workdir_forms).filter(|r| !r.is_empty())
}

#[cfg(test)]
#[path = "git_changes_tests.rs"]
mod tests;
