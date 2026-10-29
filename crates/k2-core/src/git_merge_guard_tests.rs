//! `merge_branch`'s fast-forward never writes over uncommitted changes on
//! the target checkout: it refuses with `main_has_uncommitted_changes`
//! (naming the files) and leaves everything as it was. A clean
//! fast-forward still works, and uncommitted changes to files the
//! fast-forward doesn't touch are kept.

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-merge-guard-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    dunce::canonicalize(&d).expect("canonical temp")
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=K2 Test", "-c", "user.email=k2-test@example.com", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf8 git output")
}

fn git(dir: &Path, args: &[&str]) {
    git_out(dir, args);
}

fn s(p: &Path) -> String {
    p.to_str().expect("utf8").to_string()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// `main` with a.txt, b.txt, c.txt; branch `feature` one commit ahead
/// that changes a.txt and adds new.txt. Checked out on `main`.
fn ff_repo(tag: &str) -> (PathBuf, String, String) {
    let d = temp(tag);
    git(&d, &["init", "-q"]);
    std::fs::write(d.join("a.txt"), "a1\n").expect("a");
    std::fs::write(d.join("b.txt"), "b1\n").expect("b");
    std::fs::write(d.join("c.txt"), "c1\n").expect("c");
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    git(&d, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(d.join("a.txt"), "a2 from feature\n").expect("a2");
    std::fs::write(d.join("new.txt"), "new from feature\n").expect("new");
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "feature work"]);
    let feature = git_out(&d, &["rev-parse", "feature"]).trim().to_string();
    git(&d, &["checkout", "-q", "main"]);
    let main = git_out(&d, &["rev-parse", "main"]).trim().to_string();
    (d, main, feature)
}

fn head(d: &Path) -> String {
    git_out(d, &["rev-parse", "HEAD"]).trim().to_string()
}

fn assert_dirty_refusal(r: Result<MergeResult, String>, files: &[&str]) {
    let e = match r {
        Ok(m) => panic!("fast-forward over uncommitted changes must be refused, got Ok(success={})", m.success),
        Err(e) => e,
    };
    assert!(e.starts_with("main_has_uncommitted_changes:"), "expected main_has_uncommitted_changes, got: {e}");
    for f in files {
        assert!(e.contains(f), "refusal names {f}: {e}");
    }
}

#[test]
fn fast_forward_refuses_an_unstaged_change_it_would_overwrite() {
    let (d, main, _) = ff_repo("unstaged");
    std::fs::write(d.join("a.txt"), "my unsaved work\n").expect("dirty a");
    assert_dirty_refusal(merge_branch(&s(&d), "feature"), &["a.txt"]);
    assert_eq!(read(&d.join("a.txt")), "my unsaved work\n", "uncommitted change kept");
    assert_eq!(head(&d), main, "HEAD did not move");
    assert!(!d.join("new.txt").exists(), "nothing checked out");
}

#[test]
fn fast_forward_refuses_a_staged_change_it_would_overwrite() {
    let (d, main, _) = ff_repo("staged");
    std::fs::write(d.join("a.txt"), "my staged work\n").expect("dirty a");
    git(&d, &["add", "a.txt"]);
    assert_dirty_refusal(merge_branch(&s(&d), "feature"), &["a.txt"]);
    assert_eq!(read(&d.join("a.txt")), "my staged work\n");
    assert_eq!(git_out(&d, &["diff", "--cached", "--name-only"]).trim(), "a.txt", "still staged");
    assert_eq!(head(&d), main);
}

#[test]
fn fast_forward_refuses_an_untracked_file_it_would_overwrite() {
    let (d, main, _) = ff_repo("untracked");
    std::fs::write(d.join("new.txt"), "my untracked notes\n").expect("untracked new");
    assert_dirty_refusal(merge_branch(&s(&d), "feature"), &["new.txt"]);
    assert_eq!(read(&d.join("new.txt")), "my untracked notes\n");
    assert_eq!(head(&d), main);
}

#[test]
fn clean_fast_forward_still_works() {
    let (d, _, feature) = ff_repo("clean");
    let r = merge_branch(&s(&d), "feature").expect("clean fast-forward");
    assert!(r.success && r.conflicts.is_empty(), "success, no conflicts");
    assert_eq!(head(&d), feature, "HEAD moved to feature");
    assert_eq!(git_out(&d, &["symbolic-ref", "HEAD"]).trim(), "refs/heads/main", "still on main");
    assert_eq!(read(&d.join("a.txt")), "a2 from feature\n");
    assert_eq!(read(&d.join("new.txt")), "new from feature\n");
    assert_eq!(git_out(&d, &["status", "--porcelain"]), "", "clean tree after the fast-forward");
}

#[test]
fn fast_forward_keeps_uncommitted_changes_it_does_not_touch() {
    let (d, _, feature) = ff_repo("unrelated");
    std::fs::write(d.join("b.txt"), "b unsaved\n").expect("dirty b");
    std::fs::write(d.join("c.txt"), "c staged\n").expect("dirty c");
    git(&d, &["add", "c.txt"]);
    merge_branch(&s(&d), "feature").expect("fast-forward around unrelated changes");
    assert_eq!(head(&d), feature);
    assert_eq!(read(&d.join("a.txt")), "a2 from feature\n");
    assert_eq!(read(&d.join("new.txt")), "new from feature\n");
    assert_eq!(read(&d.join("b.txt")), "b unsaved\n", "unstaged change kept");
    assert_eq!(read(&d.join("c.txt")), "c staged\n", "staged change kept");
    let mut status: Vec<String> = git_out(&d, &["status", "--porcelain"]).lines().map(str::to_string).collect();
    status.sort();
    assert_eq!(status, vec![" M b.txt".to_string(), "M  c.txt".to_string()]);
}
