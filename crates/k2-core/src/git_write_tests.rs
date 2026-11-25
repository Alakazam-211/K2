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
/// included), and a repo-local signing setting doesn't prompt. A repo-local
/// `core.hooksPath` names a program, so the workspace is trusted first.
#[cfg(unix)]
#[test]
fn writes_run_no_hooks() {
    let _home = trust_env();
    let d = fx::init("write-hooks");
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "base");
    let marks = fx::tmp("write-hooks-marks");
    let hooks = d.join("my-hooks");
    std::fs::create_dir_all(&hooks).expect("mkdir hooks");
    std::fs::create_dir_all(d.join(".git/hooks")).expect("mkdir .git/hooks");
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

    assert!(stage_file(s(&d), "a.txt").expect_err("untrusted").starts_with("git_config_runs_programs: "));
    crate::cli_folder_trust::gate::decide(s(&d), crate::cli_folder_trust::gate::Decision::Always, "owner", None).expect("trust");
    write(&d, "a.txt", "k2\n");
    write(&d, "b.txt", "b\n");
    stage_file(s(&d), "a.txt").expect("stage");
    stage_all(s(&d)).expect("stage all");
    unstage_file(s(&d), "b.txt").expect("unstage");
    commit(s(&d), "k2 commit").expect("commit (unsigned, no prompt)");
    // The fixture's own git (hooks off) makes the branch K2 then deletes.
    fx::git(&d, &["-c", if cfg!(windows) { "core.hooksPath=NUL" } else { "core.hooksPath=/dev/null" }, "branch", "side"]);
    delete_branch(s(&d), "side").expect("delete branch");
    let ran: Vec<String> =
        std::fs::read_dir(&marks).expect("marks").map(|e| e.expect("entry").file_name().to_string_lossy().to_string()).collect();
    assert!(ran.is_empty(), "hooks ran through K2's writes: {ran:?}");
    assert!(fx::git(&d, &["log", "-1", "--format=%G?"]) == "N", "unsigned, as libgit2 made them");
}

/// A 3-way merge on git: K2's merge message, unsigned, no editor, and the
/// conflict markers in plain `merge` style even when the person's repo
/// asks for diff3; a merge driver from the repo's attributes is the
/// person's git's business, but hooks never run.
#[test]
fn merge_commit_and_conflicts_on_git() {
    let d = fx::init("write-merge");
    write(&d, "a.txt", "a1\n");
    write(&d, "b.txt", "b1\n");
    fx::commit_all(&d, "base");
    fx::git(&d, &["checkout", "-q", "-b", "feature"]);
    write(&d, "b.txt", "b2 from feature\n");
    fx::commit_all(&d, "feature");
    fx::git(&d, &["checkout", "-q", "main"]);
    write(&d, "a.txt", "a2 from main\n");
    fx::commit_all(&d, "main");
    fx::git(&d, &["config", "merge.conflictStyle", "diff3"]);
    fx::git(&d, &["config", "commit.gpgsign", "true"]);
    let r = merge_branch(s(&d), "feature").expect("merge");
    assert!(r.success && r.conflicts.is_empty(), "{r:?}");
    assert_eq!(fx::git(&d, &["log", "-1", "--format=%s %G?"]), "Merge branch 'feature' N");
    assert_eq!(fx::git(&d, &["rev-list", "--count", "--merges", "HEAD"]), "1");
    assert_eq!(std::fs::read_to_string(d.join("b.txt")).expect("b"), "b2 from feature\n");

    // A conflict: listed, markers in merge style, state in progress.
    let root = fx::git(&d, &["rev-list", "--max-parents=0", "HEAD"]);
    fx::git(&d, &["checkout", "-q", "-b", "other", &root]);
    write(&d, "a.txt", "a3 from other\n");
    fx::commit_all(&d, "other");
    fx::git(&d, &["checkout", "-q", "main"]);
    let r = merge_branch(s(&d), "other").expect("merge runs");
    assert_eq!((r.success, r.conflicts.clone()), (false, vec!["a.txt".to_string()]));
    let body = std::fs::read_to_string(d.join("a.txt")).expect("a");
    assert!(body.contains("<<<<<<<") && body.contains(">>>>>>>"), "{body}");
    assert!(!body.contains("|||||||"), "merge style, not diff3: {body}");
    assert!(merge_status(s(&d)).expect("status").in_progress);
    let e = merge_branch(s(&d), "other").expect_err("a merge is in progress");
    assert!(e.contains("already in progress"), "{e}");
    resolve_conflict(s(&d), "a.txt", "ours").expect("ours");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "a2 from main\n");
    assert!(merge_status(s(&d)).expect("status").conflicts.is_empty());
    abort_merge(s(&d)).expect("abort");
    assert!(!merge_status(s(&d)).expect("status").in_progress);
    assert_eq!(fx::git(&d, &["status", "--porcelain"]), "", "clean after abort");
    assert!(merge_branch(s(&d), "nope").expect_err("missing").contains("not found"));
    assert!(merge_branch(s(&d), "--abort").expect_err("option-like").contains("not found"));
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
        assert!(e.contains("this needs git, which isn't installed on this machine"), "{what}: a plain message: {e}");
        // And how to install it, for this machine.
        let how = if cfg!(target_os = "macos") {
            "Install Apple's command line tools"
        } else {
            "Install it with: "
        };
        assert!(e.contains(how) && e.ends_with("then try again"), "{what}: an install hint: {e}");
        assert_eq!(refusal::code_of(&e), Some(refusal::GIT_NOT_INSTALLED));
    };
    refused("stage", stage_file(s(&d), "a.txt"));
    refused("unstage", unstage_file(s(&d), "a.txt"));
    refused("stage all", stage_all(s(&d)));
    refused("commit", commit(s(&d), "x").map(|_| ()));
    refused("delete branch", delete_branch(s(&d), "side"));
    refused("merge", merge_branch(s(&d), "side").map(|_| ()));
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

// ── A repository's own programs need trust (0.46.18) ─────────────────

/// A marker program: records that it ran, then passes its input through.
#[cfg(unix)]
fn marker_prog(marks: &Path, name: &str) -> String {
    std::fs::create_dir_all(marks).expect("mkdir marks");
    let p = marks.join(format!("{name}.sh"));
    crate::test_env::write_executable(&p, &format!("#!/bin/sh\n: > '{}'\ncat\n", marks.join(format!("{name}.ran")).display()));
    p.display().to_string()
}

#[cfg(unix)]
fn ran(marks: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(marks)
        .expect("marks")
        .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".ran"))
        .collect();
    v.sort();
    v
}

/// An isolated HOME (folder trust and K2's own records live there) and a
/// test database.
#[cfg(unix)]
fn trust_env() -> crate::test_env::TempHome {
    let t = crate::test_env::TempHome::new();
    crate::db::init_for_tests();
    t
}

#[cfg(unix)]
fn assert_needs_trust(e: &str, keys: &[&str], secret: &str) {
    assert!(e.starts_with("git_config_runs_programs: "), "the typed code: {e}");
    assert_eq!(refusal::code_of(e), Some(refusal::GIT_CONFIG_RUNS_PROGRAMS));
    for k in keys {
        assert!(e.to_ascii_lowercase().contains(k), "names {k}: {e}");
    }
    assert!(!e.contains(secret), "never a key's value: {e}");
    assert!(e.contains("Trust the workspace first") && e.contains("k2 workspace trust"), "points to trust: {e}");
}

/// A repo whose `.gitattributes` sends every .txt through filter `x`, with
/// one commit, and a.txt changed in the work tree.
#[cfg(unix)]
fn filtered_repo(tag: &str) -> PathBuf {
    let d = fx::init(tag);
    write(&d, ".gitattributes", "*.txt filter=x\n");
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "base");
    write(&d, "a.txt", "changed\n");
    d
}

#[cfg(unix)]
#[test]
fn a_repo_filter_needs_trust_before_stage_and_then_runs() {
    let _home = trust_env();
    let d = filtered_repo("guard-filter");
    let marks = fx::tmp("guard-filter-marks");
    let prog = marker_prog(&marks, "filter");
    fx::git(&d, &["config", "filter.x.clean", &prog]);
    fx::git(&d, &["config", "filter.x.smudge", &prog]);
    let before = fx::ls_files_stage(&d);
    let e = stage_file(s(&d), "a.txt").expect_err("an untrusted repo filter refuses the stage");
    assert_needs_trust(&e, &["filter.x.clean", "filter.x.smudge"], &prog);
    assert!(ran(&marks).is_empty(), "nothing ran: {:?}", ran(&marks));
    assert_eq!(fx::ls_files_stage(&d), before, "the index is untouched");
    assert!(stage_all(s(&d)).is_err() && commit(s(&d), "x").is_err(), "every write is refused");
    assert!(ran(&marks).is_empty());

    // Trusted: as before, the person's git runs its filter.
    crate::cli_folder_trust::gate::decide(s(&d), crate::cli_folder_trust::gate::Decision::Always, "owner", None).expect("trust");
    stage_file(s(&d), "a.txt").expect("stage once trusted");
    assert!(!ran(&marks).is_empty(), "the filter ran once trusted");
}

#[cfg(unix)]
#[test]
fn a_filter_behind_an_include_needs_trust_too() {
    let _home = trust_env();
    let d = filtered_repo("guard-include");
    let marks = fx::tmp("guard-include-marks");
    let prog = marker_prog(&marks, "filter");
    write(&d, ".git/extra.cfg", format!("[filter \"x\"]\n\tclean = {prog}\n"));
    fx::git(&d, &["config", "include.path", "extra.cfg"]);
    let e = stage_file(s(&d), "a.txt").expect_err("refused");
    assert_needs_trust(&e, &["filter.x.clean"], &prog);
    assert!(ran(&marks).is_empty());
}

#[cfg(unix)]
#[test]
fn a_case_variant_key_is_the_same_key() {
    let _home = trust_env();
    let d = filtered_repo("guard-case");
    let marks = fx::tmp("guard-case-marks");
    let prog = marker_prog(&marks, "filter");
    let cfg = d.join(".git/config");
    let mut text = std::fs::read_to_string(&cfg).expect("config");
    text.push_str(&format!("[Filter \"x\"]\n\tCLEAN = {prog}\n"));
    std::fs::write(&cfg, text).expect("write config");
    let e = stage_file(s(&d), "a.txt").expect_err("refused");
    assert_needs_trust(&e, &["filter.x.clean"], &prog);
    assert!(ran(&marks).is_empty());
}

#[cfg(unix)]
#[test]
fn a_merge_driver_needs_trust_before_merge() {
    let _home = trust_env();
    let d = fx::init("guard-merge");
    write(&d, ".gitattributes", "a.txt merge=x\n");
    write(&d, "a.txt", "base\n");
    fx::commit_all(&d, "base");
    fx::git(&d, &["checkout", "-q", "-b", "side"]);
    write(&d, "a.txt", "side\n");
    fx::commit_all(&d, "side");
    fx::git(&d, &["checkout", "-q", "main"]);
    write(&d, "a.txt", "main\n");
    fx::commit_all(&d, "main");
    let marks = fx::tmp("guard-merge-marks");
    let prog = marker_prog(&marks, "driver");
    fx::git(&d, &["config", "merge.x.driver", &format!("{prog} %O %A %B")]);
    let head = fx::git(&d, &["rev-parse", "HEAD"]);
    let e = merge_branch(s(&d), "side").map(|_| ()).expect_err("refused");
    assert_needs_trust(&e, &["merge.x.driver"], &prog);
    assert!(ran(&marks).is_empty(), "the driver never ran");
    assert_eq!(fx::git(&d, &["rev-parse", "HEAD"]), head);
    assert!(!d.join(".git/MERGE_HEAD").exists());
    let e = delete_branch(s(&d), "side").expect_err("delete is a write too");
    assert!(e.starts_with("git_config_runs_programs: "), "{e}");
}

/// A merge already in progress (made before the filter was set) can be
/// neither resolved nor aborted through K2 untrusted.
#[cfg(unix)]
#[test]
fn resolve_and_abort_are_refused_too() {
    let _home = trust_env();
    let d = fx::init("guard-resolve");
    write(&d, "a.txt", "base\n");
    fx::commit_all(&d, "base");
    fx::git(&d, &["checkout", "-q", "-b", "side"]);
    write(&d, "a.txt", "side\n");
    fx::commit_all(&d, "side");
    fx::git(&d, &["checkout", "-q", "main"]);
    write(&d, "a.txt", "main\n");
    fx::commit_all(&d, "main");
    let out = fx::git_cmd(&d).args(["merge", "-q", "side"]).output().expect("run git merge");
    assert!(!out.status.success(), "the fixture merge conflicts");
    assert!(d.join(".git/MERGE_HEAD").exists());
    write(&d, ".gitattributes", "*.txt filter=x\n");
    let marks = fx::tmp("guard-resolve-marks");
    let prog = marker_prog(&marks, "filter");
    fx::git(&d, &["config", "filter.x.smudge", &prog]);
    let e = resolve_conflict(s(&d), "a.txt", "theirs").expect_err("resolve is refused");
    assert_needs_trust(&e, &["filter.x.smudge"], &prog);
    let e = abort_merge(s(&d)).expect_err("abort is refused");
    assert_needs_trust(&e, &["filter.x.smudge"], &prog);
    assert!(d.join(".git/MERGE_HEAD").exists(), "the merge is still there");
    assert!(ran(&marks).is_empty());
}

#[cfg(unix)]
#[test]
fn discard_is_refused_before_anything_goes_to_the_trash() {
    let _home = trust_env();
    let d = filtered_repo("guard-discard");
    let marks = fx::tmp("guard-discard-marks");
    let prog = marker_prog(&marks, "filter");
    fx::git(&d, &["config", "filter.x.smudge", &prog]);
    let e = crate::git_changes::discard(s(&d), &["a.txt".to_string()], false, &|_| panic!("nothing goes to the Trash"))
        .expect_err("refused");
    assert_eq!(e.code, "git_config_runs_programs", "{e}");
    assert!(e.hint.contains("filter.x.smudge") && !e.hint.contains(&prog), "{e}");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "changed\n");
    assert!(ran(&marks).is_empty());
}

/// A repo with no program in its own config needs no trust; one K2 made
/// on this machine needs none either, but a copy of it elsewhere (a clone
/// from another server) is not K2-made.
#[cfg(unix)]
#[test]
fn clean_and_k2_made_repos_need_no_trust_but_a_copy_does() {
    let _home = trust_env();
    let d = filtered_repo("guard-clean");
    stage_file(s(&d), "a.txt").expect("no program, no trust needed");

    let made = fx::tmp("guard-made");
    write(&made, ".gitattributes", "*.txt filter=x\n");
    write(&made, "a.txt", "a\n");
    init_repo(s(&made)).expect("K2's own init");
    fx::git(&made, &["config", "user.name", "K2 Test"]);
    fx::git(&made, &["config", "user.email", "k2-test@example.com"]);
    let marks = fx::tmp("guard-made-marks");
    let prog = marker_prog(&marks, "filter");
    fx::git(&made, &["config", "filter.x.clean", &prog]);
    stage_file(s(&made), "a.txt").expect("K2 made this repo here");

    let copy = fx::tmp("guard-copy").join("repo");
    let out = std::process::Command::new("cp").arg("-R").arg(&made).arg(&copy).output().expect("cp");
    assert!(out.status.success(), "cp: {}", String::from_utf8_lossy(&out.stderr));
    for e in std::fs::read_dir(&marks).expect("marks") {
        let p = e.expect("entry").path();
        if p.extension().is_some_and(|x| x == "ran") {
            std::fs::remove_file(p).expect("clear mark");
        }
    }
    write(&copy, "a.txt", "changed in the copy\n");
    let e = stage_file(s(&copy), "a.txt").expect_err("a copy from elsewhere isn't K2-made");
    assert_needs_trust(&e, &["filter.x.clean"], &prog);
    assert!(ran(&marks).is_empty());
}

/// A repo whose `.gitattributes` sends every .txt through filter `driver`,
/// with one commit, a.txt changed, and `config` (bytes) as `.git/config`.
#[cfg(unix)]
fn repo_with_config(tag: &str, driver: &str, config: &[u8]) -> PathBuf {
    let d = fx::init(tag);
    write(&d, ".gitattributes", format!("*.txt filter={driver}\n"));
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "base");
    write(&d, "a.txt", "changed\n");
    std::fs::write(d.join(".git/config"), config).expect("write config");
    d
}

/// Config git reads and a line-by-line text reader misreads still names
/// its filter: a non-UTF-8 byte anywhere in the file, a UTF-8 BOM before
/// the first section, and a `]` inside a quoted subsection with the key on
/// the header's own line. Untrusted, the stage is refused and the filter
/// never runs; trusted, the person's git runs it (the config is real).
#[cfg(unix)]
#[test]
fn a_filter_in_config_git_reads_any_way_needs_trust() {
    let _home = trust_env();
    let marks = fx::tmp("guard-forms-marks");
    let prog = marker_prog(&marks, "filter");
    let base = "[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tautocrlf = false\n[user]\n\tname = K2 Test\n\temail = k2-test@example.com\n";
    let mut non_utf8 = base.as_bytes().to_vec();
    non_utf8.extend_from_slice(b"# caf\xe9\n");
    non_utf8.extend_from_slice(format!("[filter \"x\"]\n\tclean = {prog}\n").as_bytes());
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend_from_slice(format!("[filter \"x\"]\n\tclean = {prog}\n{base}").as_bytes());
    let bracket = format!("{base}[filter \"a]b\"] clean = {prog}\n").into_bytes();
    for (tag, driver, config, key) in [
        ("guard-non-utf8", "x", non_utf8, "filter.x.clean"),
        ("guard-bom", "x", bom, "filter.x.clean"),
        ("guard-bracket", "a]b", bracket, "filter.a]b.clean"),
    ] {
        let d = repo_with_config(tag, driver, &config);
        let before = fx::ls_files_stage(&d);
        let e = crate::git_write_guard::check(&d).expect_err(tag);
        assert_needs_trust(&e, &[key], &prog);
        let e = stage_file(s(&d), "a.txt").expect_err(tag);
        assert_needs_trust(&e, &[key], &prog);
        assert!(ran(&marks).is_empty(), "{tag}: the filter never ran: {:?}", ran(&marks));
        assert_eq!(fx::ls_files_stage(&d), before, "{tag}: the index is untouched");

        crate::cli_folder_trust::gate::decide(s(&d), crate::cli_folder_trust::gate::Decision::Always, "owner", None).expect("trust");
        stage_file(s(&d), "a.txt").unwrap_or_else(|e| panic!("{tag}: stage once trusted: {e}"));
        assert_eq!(ran(&marks), vec!["filter.ran".to_string()], "{tag}: git really runs this filter once trusted");
        std::fs::remove_file(marks.join("filter.ran")).expect("clear mark");
    }
}

/// An include K2 can't read or parse counts as naming a program: one that
/// is a directory, one with a non-UTF-8 byte (its filter is still found),
/// and one that isn't valid config.
#[cfg(unix)]
#[test]
fn an_include_that_cant_be_read_or_parsed_needs_trust() {
    let _home = trust_env();
    let marks = fx::tmp("guard-inc-marks");
    let prog = marker_prog(&marks, "filter");
    let base = "[core]\n\trepositoryformatversion = 0\n\tbare = false\n[include]\n\tpath = extra.cfg\n";
    let mut non_utf8 = b"# caf\xe9\n".to_vec();
    non_utf8.extend_from_slice(format!("[filter \"x\"]\n\tclean = {prog}\n").as_bytes());
    let mut broken = format!("[filter \"x\"\n\tclean = {prog}\n").into_bytes();
    broken.extend_from_slice(b"\xff\n");
    for (tag, extra, key) in [
        ("guard-inc-dir", None, "include.path (can't be read)"),
        ("guard-inc-non-utf8", Some(non_utf8), "filter.x.clean"),
        ("guard-inc-broken", Some(broken), "include.path (can't be parsed)"),
    ] {
        let d = repo_with_config(tag, "x", base.as_bytes());
        match extra {
            Some(bytes) => std::fs::write(d.join(".git/extra.cfg"), bytes).expect("include"),
            None => std::fs::create_dir_all(d.join(".git/extra.cfg")).expect("include dir"),
        }
        let e = crate::git_write_guard::check(&d).expect_err(tag);
        assert_needs_trust(&e, &[key], &prog);
        let e = stage_file(s(&d), "a.txt").expect_err(tag);
        assert_needs_trust(&e, &[key], &prog);
        assert!(ran(&marks).is_empty(), "{tag}: nothing ran");
    }
}

/// A repository git opens but K2 can't (sha256 objects; K2's gix is built
/// for sha1 only) is refused, not taken for "not a repository"; a folder
/// with no repository at all needs nothing.
#[cfg(unix)]
#[test]
fn a_repository_k2_cant_open_is_refused() {
    let _home = trust_env();
    let marks = fx::tmp("guard-open-marks");
    let prog = marker_prog(&marks, "filter");
    let d = fx::tmp("guard-sha256");
    fx::git(&d, &["init", "-q", "--object-format=sha256", "-b", "main"]);
    fx::git(&d, &["config", "filter.x.clean", &prog]);
    write(&d, ".gitattributes", "*.txt filter=x\n");
    write(&d, "sub/a.txt", "a\n");
    assert!(crate::gitx::discover(&d).is_err(), "K2's gix can't open a sha256 repository");
    let e = crate::git_write_guard::check(&d).expect_err("a repository K2 can't open is refused");
    assert_needs_trust(&e, &["can't open this repository"], &prog);
    let e = crate::git_write_guard::check(&d.join("sub")).expect_err("from a folder inside it too");
    assert!(e.starts_with("git_config_runs_programs: "), "{e}");
    assert!(stage_file(s(&d), "sub/a.txt").is_err(), "no stage");
    assert!(ran(&marks).is_empty(), "the filter never ran");

    let plain = fx::tmp("guard-no-repo");
    crate::git_write_guard::check(&plain).expect("no repository, nothing to check");
}
