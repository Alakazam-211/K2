//! K2's writes on the person's git (0.46.18, S4–S6): each one, compared
//! with plain git doing the same thing on a twin repo (`git ls-files -s`,
//! `git status --porcelain=v2`, `git fsck`), with no hooks run and no
//! git at all giving `git_not_installed` before anything is touched.

use super::*;
use crate::git_fixture::{self as fx, write};
use std::path::{Path, PathBuf};

fn s(p: &Path) -> &str {
    p.to_str().expect("utf8 path")
}

/// Two identical repos: K2 writes to the first, plain git to the second.
fn twins(tag: &str, build: impl Fn(&Path)) -> (PathBuf, PathBuf) {
    let a = fx::init(&format!("{tag}-k2"));
    let b = fx::init(&format!("{tag}-git"));
    build(&a);
    build(&b);
    (a, b)
}

fn same_state(what: &str, a: &Path, b: &Path) {
    assert_eq!(fx::ls_files_stage(a), fx::ls_files_stage(b), "{what}: index entries differ from git's");
    let st = |d: &Path| fx::git(d, &["status", "--porcelain=v2", "-uall"]);
    assert_eq!(st(a), st(b), "{what}: status differs from git's");
    fx::git(a, &["fsck", "--full", "--no-progress"]);
}

fn base(d: &Path) {
    write(d, ".gitignore", "*.log\n");
    write(d, "a.txt", "a\n");
    write(d, "gone.txt", "bye\n");
    write(d, "keep.txt", "k\n");
    fx::commit_all(d, "base");
    write(d, "a.txt", "a changed\n");
    std::fs::remove_file(d.join("gone.txt")).expect("rm");
    write(d, "x/one.txt", "1\n");
    write(d, "x/deep/two.txt", "2\n");
    write(d, "x/deep/noise.log", "ignored\n");
    write(d, "x-other/three.txt", "3\n");
    #[cfg(unix)]
    write(d, "star*.txt", "star\n");
    #[cfg(unix)]
    write(d, "starX.txt", "x\n");
}

#[test]
fn stage_unstage_and_stage_all_match_git() {
    let (a, b) = twins("write-stage", base);
    stage_file(s(&a), "a.txt").expect("stage modified");
    fx::git(&b, &["add", "--", "a.txt"]);
    same_state("stage a modified file", &a, &b);

    stage_file(s(&a), "gone.txt").expect("stage a deletion");
    fx::git(&b, &["rm", "-q", "--cached", "--", "gone.txt"]);
    same_state("stage a deletion", &a, &b);

    stage_file(s(&a), "x/").expect("stage a folder");
    fx::git(&b, &["add", "--", "x/"]);
    same_state("stage a folder (gitignore honoured)", &a, &b);
    assert!(!fx::git(&a, &["ls-files"]).contains("noise.log"), "ignored file stays out");
    assert!(!fx::git(&a, &["ls-files"]).contains("x-other"), "a sibling folder stays out");

    #[cfg(unix)]
    {
        stage_file(s(&a), "star*.txt").expect("stage a glob-named file");
        fx::git(&b, &["add", "--", "star\\*.txt"]);
        same_state("a glob-named file stages only itself", &a, &b);
        assert!(!fx::git(&a, &["ls-files"]).contains("starX.txt"), "the glob didn't match a sibling");
    }

    unstage_file(s(&a), "a.txt").expect("unstage");
    fx::git(&b, &["reset", "-q", "--", "a.txt"]);
    same_state("unstage", &a, &b);

    stage_all(s(&a)).expect("stage all");
    fx::git(&b, &["add", "-A"]);
    same_state("stage all", &a, &b);
}

#[test]
fn commit_matches_git_and_needs_an_identity() {
    let (a, b) = twins("write-commit", base);
    stage_all(s(&a)).expect("stage all");
    fx::git(&b, &["add", "-A"]);
    let c = commit(s(&a), "a message\n\nwith a body").expect("commit");
    fx::git(&b, &["commit", "-q", "-m", "a message\n\nwith a body"]);
    assert_eq!(c.oid, fx::git(&a, &["rev-parse", "HEAD"]));
    assert_eq!(fx::git(&a, &["rev-parse", "HEAD^{tree}"]), fx::git(&b, &["rev-parse", "HEAD^{tree}"]), "same tree as git's");
    assert_eq!(fx::git(&a, &["log", "-1", "--format=%an <%ae>"]), "K2 Test <k2-test@example.com>");
    assert_eq!(fx::git(&a, &["log", "-1", "--format=%B"]), "a message\n\nwith a body");
    same_state("commit", &a, &b);

    // No identity in the repo: refused with K2's words, nothing committed.
    // (HOME points at an empty folder in the child below, so the machine's
    // own ~/.gitconfig can't supply one.)
    let d = fx::tmp("write-noident");
    fx::git(&d, &["init", "-q", "-b", "main"]);
    write(&d, "f.txt", "f\n");
    fx::git(&d, &["add", "f.txt"]);
    let out = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "git::write_tests::child_commit_without_identity", "--ignored", "--nocapture", "--test-threads=1"])
        .env("HOME", fx::tmp("write-noident-home"))
        .env("XDG_CONFIG_HOME", fx::tmp("write-noident-xdg"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("EMAIL")
        .env(CHILD_REPO_ENV, &d)
        .output()
        .expect("child");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && said.contains("1 passed"), "child:\n{said}");
}

const CHILD_REPO_ENV: &str = "K2_GIT_WRITE_CHILD_REPO";
const CHILD_MARK_ENV: &str = "K2_GIT_WRITE_CHILD_MARK";

#[test]
#[ignore = "run by commit_matches_git_and_needs_an_identity in its own process"]
fn child_commit_without_identity() {
    let d = PathBuf::from(std::env::var_os(CHILD_REPO_ENV).expect("run by the parent test"));
    let e = commit(s(&d), "x").expect_err("no identity → refused");
    assert!(e.starts_with("Failed to get signature (set git user.name and user.email)"), "{e}");
    assert!(fx::git_cmd(&d).args(["rev-parse", "--verify", "-q", "HEAD"]).output().expect("rev-parse").stdout.is_empty(), "nothing committed");
}

/// No hook ever runs through K2's writes (a repo's own `core.hooksPath`
/// included), and a repo-local signing setting doesn't prompt.
#[cfg(unix)]
#[test]
fn writes_run_no_hooks() {
    let d = fx::init("write-hooks");
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "base");
    let marks = fx::tmp("write-hooks-marks");
    let hooks = d.join("my-hooks");
    for h in ["pre-commit", "commit-msg", "post-commit", "prepare-commit-msg", "post-index-change", "pre-merge-commit", "post-merge", "reference-transaction"] {
        crate::test_env::write_executable(&hooks.join(h), &format!("#!/bin/sh\n: > '{}'\n", marks.join(h).display()));
        crate::test_env::write_executable(&d.join(".git/hooks").join(h), &format!("#!/bin/sh\n: > '{}'\n", marks.join(format!("{h}-default")).display()));
    }
    fx::git(&d, &["config", "core.hooksPath", s(&hooks)]);
    fx::git(&d, &["config", "commit.gpgsign", "true"]);
    // Armed: plain git runs them.
    write(&d, "a.txt", "armed\n");
    fx::git_cmd(&d).args(["-c", "commit.gpgsign=false", "commit", "-qam", "armed"]).output().expect("plain commit");
    assert!(marks.join("pre-commit").exists(), "fixture: plain git runs the repo's hooks");
    for e in std::fs::read_dir(&marks).expect("marks") {
        std::fs::remove_file(e.expect("entry").path()).expect("clear");
    }

    write(&d, "a.txt", "k2\n");
    write(&d, "b.txt", "b\n");
    stage_file(s(&d), "a.txt").expect("stage");
    stage_all(s(&d)).expect("stage all");
    unstage_file(s(&d), "b.txt").expect("unstage");
    commit(s(&d), "k2 commit").expect("commit (unsigned, no prompt)");
    fx::git(&d, &["branch", "side"]);
    delete_branch(s(&d), "side").expect("delete branch");
    let ran: Vec<String> =
        std::fs::read_dir(&marks).expect("marks").map(|e| e.expect("entry").file_name().to_string_lossy().to_string()).collect();
    assert!(ran.is_empty(), "hooks ran through K2's writes: {ran:?}");
    assert!(fx::git(&d, &["log", "-1", "--format=%G?"]) == "N", "unsigned, as libgit2 made them");
}

#[test]
fn delete_branch_refuses_a_checked_out_branch_and_drops_its_config() {
    let d = fx::init("write-delbranch");
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "base");
    fx::git(&d, &["branch", "gone"]);
    fx::git(&d, &["config", "branch.gone.description", "temp"]);
    delete_branch(s(&d), "gone").expect("delete");
    assert!(fx::git(&d, &["branch", "--list", "gone"]).is_empty());
    assert!(fx::git_cmd(&d).args(["config", "--get", "branch.gone.description"]).output().expect("config").stdout.is_empty(), "its config went too");
    assert!(delete_branch(s(&d), "never").expect_err("missing").contains("not found"));
    assert!(delete_branch(s(&d), "-D").expect_err("option-like").contains("not found"));
    let wt = fx::tmp("write-delbranch-wt").join("co");
    fx::git(&d, &["worktree", "add", "-q", "-b", "busy", s(&wt)]);
    let e = delete_branch(s(&d), "busy").expect_err("checked out in a worktree");
    assert!(e.starts_with("Failed to delete branch 'busy'"), "{e}");
    assert!(!fx::git(&d, &["branch", "--list", "busy"]).is_empty(), "still there");
    assert!(delete_branch(s(&d), "main").is_err(), "the checked-out branch");
}

/// No git on this machine: every write refuses with `git_not_installed`
/// and runs no program (a fake `git` first on PATH records any run). Runs
/// in a child process so no other test sees the missing git.
#[cfg(unix)]
#[test]
fn every_write_refuses_without_git_and_runs_nothing() {
    let d = fx::init("write-nogit");
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "base");
    fx::git(&d, &["checkout", "-q", "-b", "side"]);
    write(&d, "b.txt", "side\n");
    fx::commit_all(&d, "side work");
    fx::git(&d, &["checkout", "-q", "main"]);
    write(&d, "a.txt", "dirty\n");
    let fake = fx::tmp("write-nogit-bin");
    let mark = fake.join("git-ran");
    crate::test_env::write_executable(&fake.join("git"), &format!("#!/bin/sh\n: > '{}'\nexit 1\n", mark.display()));
    let empty = fx::tmp("write-nogit-empty");
    let path = crate::test_env::path_list_prepend(&fake, std::env::var_os("PATH").unwrap_or_default());
    let before = fx::ls_files_stage(&d);
    let out = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "git::write_tests::child_writes_without_git", "--ignored", "--nocapture", "--test-threads=1"])
        .env("PATH", &path)
        .env(crate::git_safe::PROBE_PATH_ENV, &empty)
        .env(crate::git_safe::TEST_APPLE_DIR_ENV, &empty)
        .env(CHILD_REPO_ENV, &d)
        .env(CHILD_MARK_ENV, &mark)
        .output()
        .expect("child");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && said.contains("1 passed"), "child:\n{said}");
    assert!(!mark.exists(), "a git program ran");
    assert_eq!(fx::ls_files_stage(&d), before, "the index is untouched");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "dirty\n");
}

#[cfg(unix)]
#[test]
#[ignore = "run by every_write_refuses_without_git_and_runs_nothing in its own process"]
fn child_writes_without_git() {
    let d = PathBuf::from(std::env::var_os(CHILD_REPO_ENV).expect("run by the parent test"));
    assert!(!crate::git_safe::availability().is_ready(), "the probe sees no git");
    let refused = |what: &str, r: Result<(), String>| {
        let e = r.expect_err(what);
        assert!(e.starts_with("git_not_installed: "), "{what}: {e}");
        assert!(e.contains("this needs git"), "{what}: a plain hint: {e}");
        assert_eq!(refusal::code_of(&e), Some(refusal::GIT_NOT_INSTALLED));
    };
    refused("stage", stage_file(s(&d), "a.txt"));
    refused("unstage", unstage_file(s(&d), "a.txt"));
    refused("stage all", stage_all(s(&d)));
    refused("commit", commit(s(&d), "x").map(|_| ()));
    refused("delete branch", delete_branch(s(&d), "side"));
    let fresh = fx::tmp("write-nogit-init");
    refused("init", init_repo(s(&fresh)));
    assert!(!fresh.join(".git").exists());
    let e = crate::git_changes::discard(s(&d), &["a.txt".to_string()], false, &|_| panic!("nothing goes to the Trash"))
        .expect_err("discard");
    assert_eq!(e.code, crate::git_changes::codes::GIT_NOT_INSTALLED, "{e}");
    assert!(e.hint.contains("this needs git"), "{e}");
    // Reads still work.
    assert_eq!(crate::git::get_changed_files(s(&d)).len(), 1);
}
