//! Tests for the changes card's turn tracking (workbench-v1 B7): a fake
//! [`Env`] over real temp git repos.

use super::*;
use std::fs;
use std::path::Path;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-turncard-{tag}-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&d).expect("mkdir temp");
    d
}

fn git_repo(dir: &Path) -> git2::Repository {
    let repo = git2::Repository::init(dir).expect("git init");
    let mut cfg = repo.config().expect("config");
    cfg.set_str("user.name", "K2 Test").expect("user.name");
    cfg.set_str("user.email", "k2-test@example.com").expect("user.email");
    fs::write(dir.join("a.txt"), "one\n").expect("write");
    let mut idx = repo.index().expect("index");
    idx.add_path(Path::new("a.txt")).expect("add");
    idx.write().expect("write index");
    let tree = repo.find_tree(idx.write_tree().expect("tree")).expect("tree");
    let sig = repo.signature().expect("sig");
    repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[]).expect("commit");
    drop(tree);
    repo
}

/// Every session is the worktree agent `ws/fix` at `cwd`, unless it is `canon`.
struct Fake {
    cwd: PathBuf,
}

impl Env for Fake {
    fn agent(&self, session_id: &str, _agent_name: &str) -> Option<WorktreeAgent> {
        (session_id != "canon").then(|| WorktreeAgent { cwd: self.cwd.clone(), address: "ws/fix".into() })
    }
}

#[test]
fn a_turn_that_changes_files_posts_one_card_on_the_sidecar_thread() {
    let dir = tmp("one");
    let _repo = git_repo(&dir);
    let env = Fake { cwd: dir.clone() };
    let mut t = Tracker::default();
    assert_eq!(t.on_row(&env, "s1", "tab-1", Some(100), false, false), None, "a start posts nothing");
    assert_eq!(t.open_turns(), 1);
    // More events of the same turn don't re-mark.
    fs::write(dir.join("a.txt"), "one\ntwo\n").expect("edit");
    assert_eq!(t.on_row(&env, "s1", "tab-1", Some(100), false, false), None);
    let post = t.on_row(&env, "s1", "tab-1", Some(100), true, false).expect("a card");
    assert_eq!(post.address, "ws/fix");
    assert_eq!(post.changes.total, 1);
    assert_eq!(post.changes.files[0].file, "a.txt");
    assert_eq!(post.changes.files[0].additions, Some(1));
    assert_eq!(t.open_turns(), 0);
    // The row keeps its old turnStartedAt after the end: no new mark, no second card.
    assert_eq!(t.on_row(&env, "s1", "tab-1", Some(100), false, false), None);
    assert_eq!(t.open_turns(), 0);
    assert_eq!(t.on_row(&env, "s1", "tab-1", Some(100), true, false), None);
}

#[test]
fn a_turn_that_changes_nothing_posts_nothing() {
    let dir = tmp("none");
    let _repo = git_repo(&dir);
    let env = Fake { cwd: dir };
    let mut t = Tracker::default();
    t.on_row(&env, "s1", "tab-1", Some(5), false, false);
    assert_eq!(t.on_row(&env, "s1", "tab-1", Some(5), true, false), None);
}

#[test]
fn the_next_turn_marks_again_and_counts_only_its_own_changes() {
    let dir = tmp("two");
    let _repo = git_repo(&dir);
    let env = Fake { cwd: dir.clone() };
    let mut t = Tracker::default();
    t.on_row(&env, "s1", "tab-1", Some(1), false, false);
    fs::write(dir.join("a.txt"), "first\n").expect("edit");
    assert!(t.on_row(&env, "s1", "tab-1", Some(1), true, false).is_some());
    t.on_row(&env, "s1", "tab-1", Some(2), false, false);
    fs::write(dir.join("b.txt"), "second\n").expect("new file");
    let post = t.on_row(&env, "s1", "tab-1", Some(2), true, false).expect("a card");
    let names: Vec<&str> = post.changes.files.iter().map(|f| f.file.as_str()).collect();
    assert_eq!(names, ["b.txt"], "a.txt was the last turn's");
}

#[test]
fn the_canonical_and_removed_rows_get_no_card() {
    let dir = tmp("canon");
    let _repo = git_repo(&dir);
    let env = Fake { cwd: dir.clone() };
    let mut t = Tracker::default();
    t.on_row(&env, "canon", "pid", Some(1), false, false);
    assert_eq!(t.open_turns(), 0, "never the canonical");
    fs::write(dir.join("a.txt"), "x\n").expect("edit");
    assert_eq!(t.on_row(&env, "canon", "pid", Some(1), true, false), None);
    t.on_row(&env, "s1", "tab-1", Some(1), false, false);
    assert_eq!(t.open_turns(), 1);
    assert_eq!(t.on_row(&env, "s1", "tab-1", None, false, true), None, "a removed row drops its turn");
    assert_eq!(t.open_turns(), 0);
}

#[test]
fn an_end_with_no_seen_start_posts_nothing() {
    let dir = tmp("late");
    let _repo = git_repo(&dir);
    let env = Fake { cwd: dir.clone() };
    let mut t = Tracker::default();
    fs::write(dir.join("a.txt"), "x\n").expect("edit");
    assert_eq!(t.on_row(&env, "s1", "tab-1", Some(9), true, false), None);
    assert_eq!(t.open_turns(), 0);
}

#[test]
fn a_main_checkout_or_the_canonical_is_never_a_worktree_agent() {
    let dir = tmp("main");
    let _repo = git_repo(&dir);
    assert_eq!(worktree_agent("p1", "tab-1", dir.clone()), None, "not under .worktrees/");
    assert_eq!(worktree_agent("p1", "p1", dir.join(".worktrees").join("x")), None, "the canonical");
}
