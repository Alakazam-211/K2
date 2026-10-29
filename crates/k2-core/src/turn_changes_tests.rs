//! Tests for `turn_changes` (workbench-v1 B7). Each test builds its own
//! temp repo; nothing outside it is read or written.

use super::*;
use std::fs;
use std::path::PathBuf;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-turnchg-{tag}-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&d).expect("mkdir temp");
    d
}

fn init(dir: &Path) -> Repository {
    let repo = Repository::init(dir).expect("git init");
    let mut cfg = repo.config().expect("config");
    cfg.set_str("user.name", "K2 Test").expect("user.name");
    cfg.set_str("user.email", "k2-test@example.com").expect("user.email");
    repo
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("mkdir parent");
    }
    fs::write(p, body).expect("write file");
}

fn commit_all(repo: &Repository, msg: &str) -> Oid {
    let mut idx = repo.index().expect("index");
    idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).expect("add_all");
    idx.update_all(["*"].iter(), None).expect("update_all");
    idx.write().expect("write index");
    let tree_id = idx.write_tree().expect("write_tree");
    let tree = repo.find_tree(tree_id).expect("tree");
    let sig = repo.signature().expect("sig");
    let parents: Vec<git2::Commit<'_>> = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
    let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs).expect("commit")
}

fn file<'a>(c: &'a ThreadChanges, rel: &str) -> &'a ThreadChangedFile {
    c.files.iter().find(|f| f.file == rel).unwrap_or_else(|| panic!("{rel} not in {:?}", c.files))
}

fn base_repo(tag: &str) -> (PathBuf, Repository) {
    let dir = tmp(tag);
    let repo = init(&dir);
    write(&dir, "a.txt", "one\ntwo\nthree\n");
    write(&dir, "b.txt", "keep\n");
    commit_all(&repo, "base");
    (dir, repo)
}

#[test]
fn nothing_changed_is_none() {
    let (dir, _repo) = base_repo("none");
    let m = mark(&dir).expect("a repo");
    assert!(changes_since(&dir, &m).is_none());
}

#[test]
fn a_file_dirty_before_and_untouched_is_not_counted() {
    let (dir, _repo) = base_repo("prior");
    write(&dir, "a.txt", "one\nTWO\nthree\n");
    let m = mark(&dir).expect("a repo");
    assert!(changes_since(&dir, &m).is_none(), "the turn didn't touch it");
}

#[test]
fn uncommitted_edits_add_and_delete_count_with_line_stats() {
    let (dir, _repo) = base_repo("edits");
    let m = mark(&dir).expect("a repo");
    write(&dir, "a.txt", "one\n2\nthree\nfour\n");
    write(&dir, "new/c.txt", "x\ny\n");
    fs::remove_file(dir.join("b.txt")).expect("rm");
    let c = changes_since(&dir, &m).expect("changes");
    assert_eq!(c.total, 3);
    assert_eq!(c.commits, 0);
    assert!(!c.truncated);
    let a = file(&c, "a.txt");
    assert_eq!((a.status.as_str(), a.additions, a.deletions), ("modified", Some(2), Some(1)));
    let n = file(&c, "new/c.txt");
    assert_eq!((n.status.as_str(), n.additions, n.deletions), ("added", Some(2), Some(0)));
    let b = file(&c, "b.txt");
    assert_eq!((b.status.as_str(), b.additions, b.deletions), ("deleted", Some(0), Some(1)));
    assert_eq!((c.additions, c.deletions), (4, 2));
    assert_eq!(summary_line(&c), format!("Changed 3 files (+4 \u{2212}2) on {}", c.branch.clone().expect("a branch")));
    // Paths are relative: never the temp dir.
    let root = dir.to_string_lossy().to_string();
    assert!(c.files.iter().all(|f| !f.file.contains(&root) && !f.file.starts_with('/')), "{:?}", c.files);
}

#[test]
fn commits_during_the_turn_count_their_files_and_the_commits() {
    let (dir, repo) = base_repo("commits");
    let m = mark(&dir).expect("a repo");
    let start = m.head().expect("a head");
    write(&dir, "a.txt", "one\ntwo\nthree\nmore\n");
    commit_all(&repo, "one");
    write(&dir, "d.txt", "d\n");
    let end = commit_all(&repo, "two");
    let c = changes_since(&dir, &m).expect("changes");
    assert_eq!(c.commits, 2);
    assert_eq!(c.total, 2);
    assert_eq!(c.from.as_deref(), Some(&start[..7]));
    assert_eq!(c.to.as_deref(), Some(&end.to_string()[..7]));
    assert_eq!(file(&c, "a.txt").additions, Some(1));
    assert_eq!(file(&c, "d.txt").status, "added");
    assert!(summary_line(&c).contains("\u{b7} 2 commits"), "{}", summary_line(&c));
}

#[test]
fn a_file_dirty_at_start_then_committed_unchanged_is_not_counted() {
    let (dir, repo) = base_repo("commit-same");
    write(&dir, "a.txt", "already\n");
    let m = mark(&dir).expect("a repo");
    commit_all(&repo, "commit what was there");
    let c = changes_since(&dir, &m).expect("the commit counts");
    assert_eq!(c.commits, 1);
    assert_eq!(c.total, 0, "the file's content didn't change in the turn: {:?}", c.files);
    assert_eq!(summary_line(&c), format!("No file changes \u{b7} 1 commit on {}", c.branch.clone().expect("a branch")));
}

#[test]
fn an_edit_put_back_is_nothing() {
    let (dir, _repo) = base_repo("putback");
    let m = mark(&dir).expect("a repo");
    write(&dir, "a.txt", "changed\n");
    write(&dir, "a.txt", "one\ntwo\nthree\n");
    assert!(changes_since(&dir, &m).is_none());
}

#[test]
fn more_than_fifty_files_truncate_but_total_counts_them_all() {
    let (dir, _repo) = base_repo("many");
    let m = mark(&dir).expect("a repo");
    for i in 0..60 {
        write(&dir, &format!("f/{i:02}.txt"), "x\n");
    }
    let c = changes_since(&dir, &m).expect("changes");
    assert_eq!(c.total, 60);
    assert_eq!(c.files.len(), THREAD_CHANGES_MAX_FILES);
    assert!(c.truncated);
    assert_eq!(c.additions, 60);
}

#[test]
fn binary_files_count_without_line_stats() {
    let (dir, _repo) = base_repo("binary");
    let m = mark(&dir).expect("a repo");
    fs::write(dir.join("img.bin"), [0u8, 1, 2, 0, 255, 0]).expect("write");
    let c = changes_since(&dir, &m).expect("changes");
    let f = file(&c, "img.bin");
    assert_eq!((f.additions, f.deletions), (None, None));
}

#[test]
fn not_a_repo_has_no_mark() {
    let dir = tmp("plain");
    assert!(mark(&dir).is_none());
    assert!(!is_k2_worktree(&dir));
}

#[test]
fn only_a_checkout_under_dot_worktrees_is_a_k2_worktree() {
    let (dir, _repo) = base_repo("wt");
    assert!(!is_k2_worktree(&dir), "a main checkout isn't a worktree agent");
    let wt = dir.join(".worktrees").join("k2-fix");
    fs::create_dir_all(&wt).expect("mkdir");
    let wrepo = init(&wt);
    write(&wt, "x.txt", "x\n");
    commit_all(&wrepo, "x");
    assert!(is_k2_worktree(&wt));
}

#[test]
fn the_doc_round_trips_with_changes() {
    let c = ThreadChanges {
        files: vec![ThreadChangedFile { file: "a.rs".into(), old_file: None, status: "modified".into(), additions: Some(1), deletions: None }],
        total: 1,
        additions: 1,
        deletions: 0,
        commits: 0,
        from: Some("abc1234".into()),
        to: None,
        branch: Some("k2/fix".into()),
        truncated: false,
    };
    let doc = crate::overlay::OverlayDoc::changes_card("d1".into(), "k2".into(), "ws/fix".into(), summary_line(&c), c.clone());
    assert_eq!(doc.kind, "changes");
    let v = serde_json::to_value(&doc).expect("json");
    assert_eq!(v["changes"]["files"][0]["file"], "a.rs");
    assert_eq!(v["changes"]["files"][0]["oldFile"], serde_json::Value::Null);
    assert_eq!(v["changes"]["truncated"], false);
    let back: crate::overlay::OverlayDoc = serde_json::from_value(v).expect("parse");
    assert_eq!(back.changes, Some(c));
    // Every other doc leaves the field out.
    let text = crate::overlay::OverlayDoc::text("d2".into(), "a".into(), "b".into(), "hi".into(), "thread");
    assert!(serde_json::to_value(&text).expect("json").get("changes").is_none());
}
