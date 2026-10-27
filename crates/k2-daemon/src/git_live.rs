//! Changes tab v1 (C1): the `git_changed` live event.
//!
//! `fs_live` hands every raw watcher path here ([`GitWatchSet::affected`]);
//! a `.git/index|HEAD|refs/**|packed-refs` change, or a non-ignored
//! working-tree edit, marks that workspace dirty. A [`Debouncer`] (300 ms
//! trailing, 1 s max) on its own flusher thread turns each burst into one
//! `git_changed {workspacePath, repoRoot, rev}` on the `session_events`
//! bus. Git routes that change the repo call [`changed_now`] so the
//! caller's own tab updates at once.
//!
//! Headless: everything runs in the daemon; no client needs to be
//! attached. Contract: `.k2/prds/prd-changes-tab-v1.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::git_changes::{self, GitSignal, RepoDirs};
use k2_core::log_debug;

/// Trailing quiet time before a burst is flushed.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// A burst never waits longer than this after its first change.
pub const MAX_WAIT: Duration = Duration::from_secs(1);

// ── Debouncer (pure) ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Pending {
    first: Instant,
    last: Instant,
    rev_key: String,
    repo_root: String,
}

/// One flushed burst: the workspace, its repo's rev key and repo root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub workspace_path: String,
    pub rev_key: String,
    pub repo_root: String,
}

/// Per-workspace trailing debounce with a max wait.
#[derive(Debug, Default)]
pub struct Debouncer {
    pending: HashMap<String, Pending>,
}

impl Debouncer {
    pub fn touch(&mut self, workspace_path: &str, rev_key: &str, repo_root: &str, now: Instant) {
        let p = self.pending.entry(workspace_path.to_string()).or_insert_with(|| Pending {
            first: now,
            last: now,
            rev_key: rev_key.to_string(),
            repo_root: repo_root.to_string(),
        });
        p.last = now;
    }

    fn deadline(p: &Pending) -> Instant {
        (p.last + DEBOUNCE).min(p.first + MAX_WAIT)
    }

    /// Remove and return every burst whose deadline has passed.
    pub fn due(&mut self, now: Instant) -> Vec<Due> {
        let ready: Vec<String> =
            self.pending.iter().filter(|(_, p)| Self::deadline(p) <= now).map(|(k, _)| k.clone()).collect();
        let mut out: Vec<Due> = ready
            .into_iter()
            .filter_map(|ws| {
                self.pending.remove(&ws).map(|p| Due { workspace_path: ws, rev_key: p.rev_key, repo_root: p.repo_root })
            })
            .collect();
        out.sort_by(|a, b| a.workspace_path.cmp(&b.workspace_path));
        out
    }

    /// The earliest pending deadline.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(Self::deadline).min()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

// ── Flusher thread ───────────────────────────────────────────────────

struct Shared {
    state: Mutex<Debouncer>,
    cv: Condvar,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    static FLUSHER: OnceLock<()> = OnceLock::new();
    let s = SHARED.get_or_init(|| Shared { state: Mutex::new(Debouncer::default()), cv: Condvar::new() });
    FLUSHER.get_or_init(|| {
        if let Err(e) = std::thread::Builder::new().name("k2-git-live".into()).spawn(flusher_loop) {
            log_debug!("[daemon/git_live] failed to spawn flusher: {e}");
        }
    });
    s
}

fn flusher_loop() {
    let s = shared();
    let mut guard = s.state.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        let now = Instant::now();
        let due = guard.due(now);
        if !due.is_empty() {
            drop(guard);
            for d in due {
                let rev = git_changes::bump_rev(&d.rev_key);
                crate::session_events::emit_git_changed(&d.workspace_path, &d.repo_root, &rev);
            }
            guard = s.state.lock().unwrap_or_else(|p| p.into_inner());
            continue;
        }
        guard = match guard.next_deadline() {
            Some(at) => {
                let wait = at.saturating_duration_since(now).max(Duration::from_millis(1));
                s.cv.wait_timeout(guard, wait).unwrap_or_else(|p| p.into_inner()).0
            }
            None => s.cv.wait(guard).unwrap_or_else(|p| p.into_inner()),
        };
    }
}

/// Mark a workspace's repo as changed; flushed after the debounce.
pub fn touch(workspace_path: &str, rev_key: &str, repo_root: &str) {
    let s = shared();
    s.state.lock().unwrap_or_else(|p| p.into_inner()).touch(workspace_path, rev_key, repo_root, Instant::now());
    s.cv.notify_one();
}

/// A git route just changed the repo at `workspace_path`: bump its rev
/// and emit `git_changed` now. Returns the new rev (`None` when the path
/// is not in a repo).
pub fn changed_now(workspace_path: &str) -> Option<String> {
    let (key, root) = git_changes::repo_key_and_root(workspace_path)?;
    let rev = git_changes::bump_rev(&key);
    let ws = trim_slash(workspace_path);
    crate::session_events::emit_git_changed(&ws, &root, &rev);
    Some(rev)
}

fn trim_slash(s: &str) -> String {
    if s == "/" {
        return s.to_string();
    }
    s.trim_end_matches('/').to_string()
}

// ── Watch set ────────────────────────────────────────────────────────

/// One watched workspace and its repo.
#[derive(Debug, Clone)]
pub struct WatchedRepo {
    pub workspace_path: String,
    pub dirs: RepoDirs,
    pub rev_key: String,
    pub repo_root: String,
}

/// Every workspace the watcher maps git changes for.
#[derive(Debug, Clone, Default)]
pub struct GitWatchSet {
    pub repos: Vec<WatchedRepo>,
}

fn canon(p: &Path) -> PathBuf {
    dunce::canonicalize(&p).unwrap_or_else(|_| p.to_path_buf())
}

impl GitWatchSet {
    /// Open the repo behind each workspace path (non-repos are skipped).
    pub fn build(workspaces: &[String]) -> Self {
        let mut seen = HashSet::new();
        let mut repos = Vec::new();
        for ws in workspaces {
            let ws = trim_slash(ws);
            if ws.is_empty() || !seen.insert(ws.clone()) {
                continue;
            }
            let Some(dirs) = git_changes::repo_dirs(&ws) else { continue };
            let rev_key = git_changes::rev_key(&dirs.workdir);
            let repo_root = trim_slash(&dirs.workdir.to_string_lossy());
            repos.push(WatchedRepo { workspace_path: ws, dirs, rev_key, repo_root });
        }
        Self { repos }
    }

    /// Extra watches for git dirs that live outside every recursively
    /// watched root: the checkout's git dir (index/HEAD, non-recursive),
    /// the common dir (packed-refs, non-recursive) and its `refs/`
    /// (recursive). `(path, recursive)`.
    pub fn extra_watches(&self, recursive_roots: &[String]) -> Vec<(String, bool)> {
        let roots: Vec<PathBuf> = recursive_roots.iter().map(|r| canon(Path::new(r))).collect();
        let covered = |p: &Path| {
            let c = canon(p);
            roots.iter().any(|r| c.starts_with(r))
        };
        let mut out: Vec<(String, bool)> = Vec::new();
        let mut add = |p: PathBuf, recursive: bool| {
            if !p.is_dir() || covered(&p) {
                return;
            }
            let s = trim_slash(&p.to_string_lossy());
            match out.iter_mut().find(|(q, _)| *q == s) {
                Some(existing) => existing.1 |= recursive,
                None => out.push((s, recursive)),
            }
        };
        for r in &self.repos {
            add(r.dirs.gitdir.clone(), false);
            add(r.dirs.commondir.clone(), false);
            add(r.dirs.commondir.join("refs"), true);
        }
        out.sort();
        out
    }

    /// The workspaces a batch of raw watcher paths touches. `.git` paths
    /// are classified per repo; other paths count when they are inside a
    /// workspace's repo (innermost), not noisy and not git-ignored.
    pub fn affected(&self, paths: &HashSet<String>, is_noisy: &dyn Fn(&str) -> bool) -> Vec<&WatchedRepo> {
        let mut hit: Vec<bool> = vec![false; self.repos.len()];
        let mut ignore_repos: HashMap<usize, Option<git_changes::IgnoreChecker>> = HashMap::new();
        for p in paths {
            if git_changes::is_git_internal(p) {
                for (i, r) in self.repos.iter().enumerate() {
                    if !hit[i] && matches!(git_changes::classify_git_path(p, &r.dirs), Some(GitSignal::Local | GitSignal::Shared)) {
                        hit[i] = true;
                    }
                }
                continue;
            }
            if is_noisy(p) {
                continue;
            }
            // Innermost repo whose workdir holds the path.
            let best = self
                .repos
                .iter()
                .enumerate()
                .filter_map(|(i, r)| git_changes::worktree_rel(p, &r.dirs).map(|rel| (i, rel)))
                .max_by_key(|(i, _)| self.repos[*i].dirs.workdir.as_os_str().len());
            let Some((i, rel)) = best else { continue };
            if hit[i] {
                continue;
            }
            let checker =
                ignore_repos.entry(i).or_insert_with(|| git_changes::IgnoreChecker::open(&self.repos[i].dirs.workdir));
            let ignored = checker.as_ref().is_some_and(|c| c.is_ignored(rel));
            if !ignored {
                hit[i] = true;
            }
        }
        self.repos.iter().zip(hit).filter(|(_, h)| *h).map(|(r, _)| r).collect()
    }

    /// Feed a batch of raw watcher paths into the debouncer.
    pub fn touch_affected(&self, paths: &HashSet<String>, is_noisy: &dyn Fn(&str) -> bool) {
        for r in self.affected(paths, is_noisy) {
            touch(&r.workspace_path, &r.rev_key, &r.repo_root);
        }
    }
}

#[cfg(test)]
#[path = "git_live_tests.rs"]
mod tests;
