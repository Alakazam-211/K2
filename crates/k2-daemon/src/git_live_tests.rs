//! Tests for `git_live` (Changes tab v1, C1): the debouncer, the watch
//! set's path mapping, and the live `git_changed` event from a real
//! watcher over temp repos (index, HEAD, refs, linked worktree, ignore).

use super::*;
use crate::session_events::SessionEvent;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::broadcast::error::TryRecvError;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-gitlive-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    d
}

/// Run git with no user/system config (hooks, signing, templates).
pub(crate) fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=K2 Test", "-c", "user.email=k2-test@example.com", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
}

fn repo_with_commit(tag: &str) -> PathBuf {
    let d = tmp(tag);
    git(&d, &["init", "-q"]);
    std::fs::write(d.join(".gitignore"), "*.tmp\n").expect("ignore");
    std::fs::write(d.join("a.txt"), "a\n").expect("a");
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    d
}

fn s(p: &Path) -> String {
    p.to_str().expect("utf8").trim_end_matches('/').to_string()
}

// ── Debouncer ────────────────────────────────────────────────────────

#[test]
fn debouncer_trailing_quiet_flushes_once() {
    let t0 = Instant::now();
    let mut d = Debouncer::default();
    d.touch("/w", "k", "/w", t0);
    d.touch("/w", "k", "/w", t0 + Duration::from_millis(100));
    d.touch("/w", "k", "/w", t0 + Duration::from_millis(200));
    assert_eq!(d.next_deadline(), Some(t0 + Duration::from_millis(500)));
    assert!(d.due(t0 + Duration::from_millis(499)).is_empty(), "still inside the quiet window");
    let due = d.due(t0 + Duration::from_millis(500));
    assert_eq!(due, vec![Due { workspace_path: "/w".into(), rev_key: "k".into(), repo_root: "/w".into() }]);
    assert!(d.is_empty());
    assert_eq!(d.next_deadline(), None);
    assert!(d.due(t0 + Duration::from_secs(5)).is_empty(), "one burst, one flush");
}

#[test]
fn debouncer_max_wait_caps_a_busy_burst() {
    let t0 = Instant::now();
    let mut d = Debouncer::default();
    let mut flushes = 0;
    // A change every 100 ms for 2.5 s never goes quiet for 300 ms.
    let mut t = t0;
    while t <= t0 + Duration::from_millis(2500) {
        d.touch("/busy", "k", "/busy", t);
        flushes += d.due(t).len();
        t += Duration::from_millis(100);
    }
    assert!((2..=3).contains(&flushes), "~1 flush per second under constant writes, got {flushes}");
}

#[test]
fn debouncer_keeps_workspaces_separate() {
    let t0 = Instant::now();
    let mut d = Debouncer::default();
    d.touch("/a", "ka", "/a", t0);
    d.touch("/b", "kb", "/b", t0 + Duration::from_millis(200));
    let first = d.due(t0 + Duration::from_millis(300));
    assert_eq!(first.iter().map(|x| x.workspace_path.as_str()).collect::<Vec<_>>(), vec!["/a"]);
    let second = d.due(t0 + Duration::from_millis(500));
    assert_eq!(second.iter().map(|x| x.workspace_path.as_str()).collect::<Vec<_>>(), vec!["/b"]);
}

// ── Watch set ────────────────────────────────────────────────────────

fn set(paths: &[String]) -> HashSet<String> {
    paths.iter().cloned().collect()
}

#[test]
fn affected_maps_git_signals_edits_and_skips_ignored_and_noise() {
    let d = repo_with_commit("affected");
    let ws = s(&d);
    let gs = GitWatchSet::build(&[ws.clone(), format!("{ws}/")]);
    assert_eq!(gs.repos.len(), 1, "duplicate spellings collapse");
    let g = s(&gs.repos[0].dirs.gitdir);
    let noisy = crate::fs_live::is_noisy;
    let hit = |paths: &[String]| gs.affected(&set(paths), &noisy).len();
    assert_eq!(hit(&[format!("{g}/index")]), 1);
    assert_eq!(hit(&[format!("{g}/index.lock")]), 1);
    assert_eq!(hit(&[format!("{g}/HEAD")]), 1);
    assert_eq!(hit(&[format!("{g}/refs/heads/main")]), 1);
    assert_eq!(hit(&[format!("{g}/objects/aa/bbbb")]), 0, "object writes alone are not a change");
    assert_eq!(hit(&[format!("{ws}/a.txt")]), 1, "a working-tree edit");
    assert_eq!(hit(&[format!("{ws}/scratch.tmp")]), 0, "git-ignored");
    assert_eq!(hit(&[format!("{ws}/node_modules/x/index.js")]), 0, "noisy");
    assert_eq!(hit(&[format!("{ws}/.k2/AGENTS.md")]), 0, "K2 runtime churn");
    assert_eq!(hit(&["/somewhere/else.txt".to_string()]), 0);
    assert_eq!(hit(&[]), 0);
    // Canonical spelling (FSEvents reports /private/var/... on macOS).
    let canon = s(&dunce::canonicalize(&d).expect("canon"));
    assert_eq!(hit(&[format!("{canon}/a.txt")]), 1);
}

#[test]
fn shared_ref_change_hits_main_and_linked_worktree() {
    let main = repo_with_commit("shared-main");
    let wt = tmp("shared-wt").join("co");
    git(&main, &["worktree", "add", "-q", "-b", "wt1", s(&wt).as_str()]);
    let gs = GitWatchSet::build(&[s(&main), s(&wt)]);
    assert_eq!(gs.repos.len(), 2);
    let common = s(&gs.repos[0].dirs.commondir);
    let noisy = crate::fs_live::is_noisy;
    let refs = gs.affected(&set(&[format!("{common}/refs/heads/main")]), &noisy);
    assert_eq!(refs.len(), 2, "a ref move touches every checkout");
    let wt_gitdir = s(&gs.repos[1].dirs.gitdir);
    let only_wt = gs.affected(&set(&[format!("{wt_gitdir}/index")]), &noisy);
    assert_eq!(only_wt.iter().map(|r| r.workspace_path.clone()).collect::<Vec<_>>(), vec![s(&wt)]);
    let only_main = gs.affected(&set(&[format!("{common}/index")]), &noisy);
    assert_eq!(only_main.iter().map(|r| r.workspace_path.clone()).collect::<Vec<_>>(), vec![s(&main)]);
}

#[test]
fn extra_watches_cover_git_dirs_outside_the_roots() {
    let main = repo_with_commit("extra-main");
    let wt = tmp("extra-wt").join("co");
    git(&main, &["worktree", "add", "-q", "-b", "wt2", s(&wt).as_str()]);
    // Only the worktree is a root: its git dir and the shared refs live
    // under `main/.git`, outside every root.
    let gs = GitWatchSet::build(&[s(&wt)]);
    let extra = gs.extra_watches(&[s(&wt)]);
    let common = s(&gs.repos[0].dirs.commondir);
    let gitdir = s(&gs.repos[0].dirs.gitdir);
    assert!(extra.contains(&(gitdir.clone(), false)), "{extra:?}");
    assert!(extra.contains(&(common.clone(), false)), "{extra:?}");
    assert!(extra.contains(&(format!("{common}/refs"), true)), "{extra:?}");
    // With the main repo as a root everything is already covered.
    assert!(gs.extra_watches(&[s(&main), s(&wt)]).is_empty());
}

#[test]
fn plan_adds_outside_worktrees_as_roots_and_every_worktree_as_git_workspace() {
    let plan = crate::fs_live::plan_from(
        vec!["/p".into(), "/q".into()],
        vec!["/p/.worktrees/x".into(), "/elsewhere/wt".into()],
    );
    assert_eq!(plan.roots, vec!["/elsewhere/wt".to_string(), "/p".into(), "/q".into()]);
    assert_eq!(plan.git_workspaces, vec!["/p".to_string(), "/q".into(), "/p/.worktrees/x".into(), "/elsewhere/wt".into()]);
}

// ── Live watcher → git_changed ───────────────────────────────────────

struct Live {
    stop: Arc<AtomicBool>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn start_watcher(roots: Vec<String>, git_workspaces: Vec<String>) -> Live {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = stop.clone();
    let plan = crate::fs_live::WatchPlan { roots, git_workspaces };
    std::thread::spawn(move || {
        let p = plan.clone();
        crate::fs_live::run_loop_with(&move || p.clone(), &stop_t).expect("watch loop");
    });
    // Let the watch stream start and any setup writes drain past MAX_WAIT.
    std::thread::sleep(Duration::from_millis(1800));
    Live { stop }
}

/// `git_changed` revs for `ws` seen within `window`.
fn collect(rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>, ws: &str, window: Duration) -> Vec<String> {
    let deadline = Instant::now() + window;
    let mut revs = Vec::new();
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(SessionEvent::GitChanged { workspace_path, rev, .. }) if workspace_path == ws => revs.push(rev),
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryRecvError::Closed) => break,
        }
    }
    revs
}

/// Wait until at least one `git_changed` for `ws` arrives (≤ `max`).
fn first_event(rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>, ws: &str, max: Duration) -> Option<String> {
    let deadline = Instant::now() + max;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(SessionEvent::GitChanged { workspace_path, rev, .. }) if workspace_path == ws => return Some(rev),
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryRecvError::Closed) => return None,
        }
    }
    None
}

#[test]
fn live_watcher_emits_git_changed_for_index_head_refs_and_edits() {
    let d = repo_with_commit("live");
    std::fs::write(d.join("b.txt"), "b\n").expect("b");
    std::fs::write(d.join("c.txt"), "c\n").expect("c");
    let ws = s(&d);
    let _live = start_watcher(vec![ws.clone()], vec![ws.clone()]);
    let mut rx = crate::session_events::subscribe();
    // Setup noise (the files above) has flushed; start clean.
    let _ = collect(&mut rx, &ws, Duration::from_millis(1300));

    // 1. index: stage an existing untracked file (only .git changes).
    git(&d, &["add", "b.txt"]);
    let r1 = first_event(&mut rx, &ws, Duration::from_secs(5)).expect("git_changed after staging (index)");

    // 2. debounce: a burst of index writes → far fewer events than ops.
    let _ = collect(&mut rx, &ws, Duration::from_millis(1300));
    for _ in 0..3 {
        git(&d, &["add", "c.txt"]);
        git(&d, &["reset", "-q", "c.txt"]);
    }
    let burst = collect(&mut rx, &ws, Duration::from_millis(2500));
    assert!((1..=3).contains(&burst.len()), "6 index writes → 1..=3 debounced events, got {}", burst.len());
    assert!(!burst.contains(&r1), "every flush is a new rev");

    // 3. HEAD + refs: commit.
    git(&d, &["commit", "-q", "-m", "two"]);
    first_event(&mut rx, &ws, Duration::from_secs(5)).expect("git_changed after commit (HEAD/refs)");

    // 4. a new branch ref only.
    let _ = collect(&mut rx, &ws, Duration::from_millis(1300));
    git(&d, &["branch", "side"]);
    first_event(&mut rx, &ws, Duration::from_secs(5)).expect("git_changed after a ref change");

    // 5. an ignored edit is silent; a tracked edit is not.
    let _ = collect(&mut rx, &ws, Duration::from_millis(1300));
    std::fs::write(d.join("build.tmp"), "ignored\n").expect("tmp");
    let quiet = collect(&mut rx, &ws, Duration::from_millis(1500));
    assert!(quiet.is_empty(), "a git-ignored edit must not emit, got {quiet:?}");
    std::fs::write(d.join("a.txt"), "edited\n").expect("edit");
    first_event(&mut rx, &ws, Duration::from_secs(5)).expect("git_changed after a working-tree edit");
}

#[test]
fn live_watcher_covers_a_linked_worktree_outside_every_root() {
    let main = repo_with_commit("live-wt-main");
    let wt = tmp("live-wt").join("co");
    git(&main, &["worktree", "add", "-q", "-b", "wt3", s(&wt).as_str()]);
    std::fs::write(wt.join("new.txt"), "n\n").expect("new");
    let ws = s(&wt);
    // The main repo is NOT watched: the worktree's index lives under
    // main/.git/worktrees/wt3, reachable only through the extra watch.
    let _live = start_watcher(vec![ws.clone()], vec![ws.clone()]);
    let mut rx = crate::session_events::subscribe();
    let _ = collect(&mut rx, &ws, Duration::from_millis(1300));
    git(&wt, &["add", "new.txt"]);
    first_event(&mut rx, &ws, Duration::from_secs(5)).expect("git_changed for the worktree's own index");
}

#[test]
fn changed_now_bumps_rev_and_emits_at_once() {
    let d = repo_with_commit("now");
    let ws = s(&d);
    let mut rx = crate::session_events::subscribe();
    let before = k2_core::git_changes::current_rev(&k2_core::git_changes::rev_key(&d));
    let rev = changed_now(&format!("{ws}/")).expect("a repo");
    assert_ne!(rev, before);
    let got = first_event(&mut rx, &ws, Duration::from_millis(200)).expect("immediate event");
    assert_eq!(got, rev);
    assert!(changed_now(&s(&tmp("now-norepo"))).is_none());
}
