//! `remove_worktree` only ever removes a linked worktree of the project's
//! repository. Any other folder (inside or outside the repo, the main
//! worktree, a link that points elsewhere) is refused and left untouched.
//! Trashing goes to the test trash (`safe_delete` in test builds).

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-wt-guard-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    dunce::canonicalize(&d).expect("canonical temp")
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=K2 Test", "-c", "user.email=k2-test@example.com", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn repo(tag: &str) -> PathBuf {
    let d = temp(tag);
    git(&d, &["init", "-q"]);
    std::fs::write(d.join("a.txt"), "one\n").expect("a");
    std::fs::create_dir_all(d.join("src")).expect("src");
    std::fs::write(d.join("src/lib.txt"), "lib\n").expect("lib");
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    d
}

fn s(p: &Path) -> String {
    p.to_str().expect("utf8").to_string()
}

/// A folder with one file in it, standing in for something precious.
fn precious(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("mkdir precious");
    let f = dir.join("keep.txt");
    std::fs::write(&f, "precious\n").expect("write precious");
    f
}

fn assert_refused(r: Result<(), String>, code: &str) {
    let e = r.expect_err("remove_worktree must refuse");
    assert!(e.starts_with(&format!("{code}:")), "expected a `{code}` refusal, got: {e}");
}

fn listed(project: &Path) -> Vec<PathBuf> {
    list_worktrees(&s(project)).iter().map(|w| dunce::canonicalize(&w.path).expect("listed worktree exists")).collect()
}

#[test]
fn refuses_a_folder_outside_the_repo() {
    let r = repo("outside");
    let elsewhere = temp("elsewhere");
    let keep = precious(&elsewhere);
    assert_refused(remove_worktree(&s(&r), &s(&elsewhere), true), "not_a_worktree_of_project");
    assert_eq!(std::fs::read_to_string(&keep).expect("still there"), "precious\n");
}

#[test]
fn refuses_a_plain_folder_inside_the_repo() {
    let r = repo("inside");
    let src = r.join("src");
    assert_refused(remove_worktree(&s(&r), &s(&src), false), "not_a_worktree_of_project");
    assert_eq!(std::fs::read_to_string(src.join("lib.txt")).expect("still there"), "lib\n");
}

#[test]
fn refuses_the_main_worktree() {
    let r = repo("main");
    let wt = create_worktree(&s(&r), "feature/main-guard").expect("create worktree");
    // From the project itself, and from a linked worktree naming the main one.
    assert_refused(remove_worktree(&s(&r), &s(&r), true), "not_a_worktree_of_project");
    assert_refused(remove_worktree(&wt.path, &s(&r), true), "not_a_worktree_of_project");
    assert_eq!(std::fs::read_to_string(r.join("a.txt")).expect("main intact"), "one\n");
    assert!(Path::new(&wt.path).join("a.txt").is_file(), "linked worktree intact");
}

#[test]
fn refuses_a_link_inside_the_repo_that_points_outside() {
    let r = repo("link");
    let elsewhere = temp("link-target");
    let keep = precious(&elsewhere);
    std::fs::create_dir_all(r.join(".worktrees")).expect("mkdir .worktrees");
    let link = r.join(".worktrees").join("sneaky");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&elsewhere, &link).expect("symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&elsewhere, &link).expect("symlink");
    assert_refused(remove_worktree(&s(&r), &s(&link), true), "not_a_worktree_of_project");
    assert_eq!(std::fs::read_to_string(&keep).expect("target intact"), "precious\n");
    assert!(std::fs::symlink_metadata(&link).is_ok(), "the link itself is left alone");
}

#[test]
fn refuses_when_the_project_is_not_a_repo() {
    let plain = temp("plain");
    let child = plain.join("child");
    let keep = precious(&child);
    let e = remove_worktree(&s(&plain), &s(&child), true).expect_err("must refuse");
    // `not_a_repo` normally; `not_a_worktree_of_project` if the temp dir
    // happens to sit inside some repository on the test machine.
    assert!(
        e.starts_with("not_a_repo:") || e.starts_with("not_a_worktree_of_project:"),
        "expected a refusal, got: {e}"
    );
    assert_eq!(std::fs::read_to_string(&keep).expect("still there"), "precious\n");
}

#[test]
fn removes_a_real_linked_worktree() {
    let r = repo("real");
    let wt = create_worktree(&s(&r), "feature/real").expect("create worktree");
    let wt_path = dunce::canonicalize(&wt.path).expect("canonical worktree");
    assert!(listed(&r).contains(&wt_path), "worktree listed before remove");
    remove_worktree(&s(&r), &wt.path, false).expect("remove a real linked worktree");
    assert!(!wt_path.exists(), "worktree folder moved away");
    assert!(!listed(&r).contains(&wt_path), "git no longer lists the worktree");
    assert_eq!(std::fs::read_to_string(r.join("a.txt")).expect("main intact"), "one\n");
}

#[test]
fn a_missing_worktree_path_is_a_no_op_prune() {
    let r = repo("missing");
    remove_worktree(&s(&r), &s(&r.join(".worktrees/gone")), false).expect("missing path → Ok");
    assert_eq!(std::fs::read_to_string(r.join("a.txt")).expect("main intact"), "one\n");
}

#[test]
fn refusal_codes_read_back() {
    assert_eq!(refusal::code_of("not_a_worktree_of_project: x"), Some(refusal::NOT_A_WORKTREE_OF_PROJECT));
    assert_eq!(refusal::code_of("not_a_repo: x"), Some(refusal::NOT_A_REPO));
    assert_eq!(refusal::code_of("main_has_uncommitted_changes: x"), Some(refusal::MAIN_HAS_UNCOMMITTED_CHANGES));
    assert_eq!(refusal::code_of("Merge failed: not_a_repo: x"), None);
    assert_eq!(refusal::code_of("not_a_repository"), None);
}
