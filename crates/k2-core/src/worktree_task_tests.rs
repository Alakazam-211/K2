//! Worktree sidecars' git half: create from the default branch, the
//! review read, Land (cherry-pick) and every refusal leaving main exactly
//! as it was, branch deletion, the inbox state, and the 0251 columns.
//!
//! Each test makes its own throwaway repository (repo-local identity, so
//! the cherry-pick never depends on the machine's git config).

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-wt-task-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    dunce::canonicalize(&d).expect("canonical temp")
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on `main` with one commit, identity set in the repo itself.
fn repo(tag: &str) -> PathBuf {
    let d = temp(tag);
    sh(&d, &["init", "-q"]);
    sh(&d, &["config", "user.name", "K2 Test"]);
    sh(&d, &["config", "user.email", "k2-test@example.com"]);
    sh(&d, &["config", "commit.gpgsign", "false"]);
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").expect("a");
    sh(&d, &["add", "-A"]);
    sh(&d, &["commit", "-q", "-m", "init"]);
    d
}

fn commit_file(dir: &Path, file: &str, text: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), text).expect("write");
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-q", "-m", msg]);
    sh(dir, &["rev-parse", "HEAD"])
}

fn head(dir: &Path, rev: &str) -> String {
    sh(dir, &["rev-parse", rev])
}

fn err(r: Result<impl std::fmt::Debug, WtError>, code: &str) -> WtError {
    let e = r.expect_err(&format!("expected a `{code}` refusal"));
    assert_eq!(e.code, code, "wrong refusal: {e}");
    e
}

#[test]
fn create_cuts_from_the_default_branch_not_head() {
    let root = repo("create");
    let main_head = head(&root, "main");
    // The main checkout sits on another branch with its own commit.
    sh(&root, &["checkout", "-q", "-b", "side"]);
    commit_file(&root, "side.txt", "s\n", "side");

    let c = create(&root, "alpha", None).expect("create");
    assert_eq!(c.branch, "k2/alpha");
    assert_eq!(c.dir, ".worktrees/k2-alpha");
    assert_eq!(c.base.name, "main");
    assert_eq!(c.base.onto.as_deref(), Some("main"));
    assert_eq!(c.base.sha, main_head, "cut from main, not from side's HEAD");
    let wt = PathBuf::from(&c.path);
    assert_eq!(wt, dunce::canonicalize(root.join(".worktrees/k2-alpha")).expect("wt exists"));
    assert_eq!(head(&wt, "HEAD"), main_head);
    assert!(!wt.join("side.txt").exists(), "side's file must not be in the worktree");
    assert_eq!(current_branch(&wt).as_deref(), Some("k2/alpha"));
    let ignore = std::fs::read_to_string(root.join(".gitignore")).expect(".gitignore");
    assert!(ignore.lines().any(|l| l.trim() == ".worktrees/"), "{ignore}");

    // An explicit base that is a branch: Land goes back onto it.
    let c = create(&root, "beta", Some("side")).expect("create from side");
    assert_eq!(c.base.onto.as_deref(), Some("side"));
    assert_eq!(c.base.sha, head(&root, "side"));
    // A commit (not a branch): Land goes onto the default branch.
    let c = create(&root, "gamma", Some(&main_head[..10])).expect("create from a sha");
    assert_eq!(c.base.onto.as_deref(), Some("main"));
}

#[test]
fn create_refuses_existing_branch_folder_bad_base_and_non_repo() {
    let root = repo("create-refuse");
    sh(&root, &["branch", "k2/taken"]);
    let e = err(create(&root, "taken", None), codes::BRANCH_EXISTS);
    assert_eq!(e.status, 409);
    assert!(!root.join(".worktrees/k2-taken").exists(), "nothing was created");

    std::fs::create_dir_all(root.join(".worktrees/k2-busy")).expect("mkdir");
    std::fs::write(root.join(".worktrees/k2-busy/keep.txt"), "precious\n").expect("keep");
    err(create(&root, "busy", None), codes::WORKTREE_EXISTS);
    assert_eq!(std::fs::read_to_string(root.join(".worktrees/k2-busy/keep.txt")).expect("kept"), "precious\n");
    assert!(sh(&root, &["branch", "--list", "k2/busy"]).is_empty(), "no branch made");

    err(create(&root, "nobase", Some("no-such-ref")), codes::BASE_NOT_FOUND);
    err(create(&root, "dash", Some("--upload-pack=x")), codes::BASE_NOT_FOUND);

    let plain = temp("not-a-repo");
    err(create(&plain, "x", None), codes::NOT_A_REPO);
}

#[test]
fn land_cherry_picks_only_new_commits_and_review_reads_them() {
    let root = repo("land");
    let c = create(&root, "work", None).expect("create");
    let wt = PathBuf::from(&c.path);
    let first = commit_file(&wt, "b.txt", "b\n", "add b");
    // The same change is already on main (cherry-picked by hand): patch-id skips it.
    sh(&root, &["cherry-pick", &first]);
    let main_before = head(&root, "main");
    let second = commit_file(&wt, "c.txt", "c\n", "add c");
    std::fs::write(wt.join("scratch.txt"), "not committed\n").expect("scratch");

    let r = review(&root, Some(&wt), "main", "k2/work").expect("review");
    assert_eq!(r.commits.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(), vec![second.as_str()]);
    assert_eq!(r.commits[0].subject, "add c");
    assert_eq!(r.dirty, vec!["scratch.txt".to_string()]);
    assert_eq!(r.onto_head.as_deref(), Some(main_before.as_str()));
    assert_eq!(r.tip.as_deref(), Some(second.as_str()));
    assert_eq!(r.blocked, None, "{r:?}");
    assert!(r.files.iter().any(|f| f.path == "c.txt" && f.status == "A"), "{:?}", r.files);

    let l = land(&root, "main", "k2/work", Some(&main_before), &second[..12]).expect("land");
    assert_eq!(l.from, main_before);
    assert_eq!(l.commits.len(), 1);
    assert_eq!(head(&root, "main"), l.to);
    assert_ne!(l.to, main_before);
    assert_eq!(std::fs::read_to_string(root.join("c.txt")).expect("landed file"), "c\n");
    assert!(!root.join("scratch.txt").exists(), "uncommitted worktree files never land");
    assert!(tracked_changes(&root).expect("status").is_empty());
    // The worktree is untouched.
    assert_eq!(head(&wt, "HEAD"), second);

    let r = review(&root, Some(&wt), "main", "k2/work").expect("review after");
    assert!(r.commits.is_empty(), "{r:?}");
    assert_eq!(r.blocked, Some(codes::ALREADY_LANDED));
    err(land(&root, "main", "k2/work", None, &second), codes::ALREADY_LANDED);
}

#[test]
fn land_refusals_leave_main_exactly_as_it_was() {
    let root = repo("land-refuse");
    let c = create(&root, "w", None).expect("create");
    let wt = PathBuf::from(&c.path);
    let tip = commit_file(&wt, "b.txt", "b\n", "add b");
    let main_head = head(&root, "main");

    // Uncommitted tracked change on main: refused, the edit stays.
    std::fs::write(root.join("a.txt"), "one\ntwo\nmine\n").expect("edit main");
    let e = err(land(&root, "main", "k2/w", None, &tip), codes::MAIN_HAS_UNCOMMITTED_CHANGES);
    assert_eq!(e.files, vec!["a.txt".to_string()]);
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).expect("a"), "one\ntwo\nmine\n");
    assert_eq!(head(&root, "main"), main_head);
    assert!(!root.join("b.txt").exists());
    sh(&root, &["checkout", "--", "a.txt"]);

    // An untracked file on main does not block.
    std::fs::write(root.join("notes.txt"), "mine\n").expect("untracked");

    // Stale tip / stale onto: refused.
    err(land(&root, "main", "k2/w", None, "0000000000"), codes::STALE);
    err(land(&root, "main", "k2/w", Some("1111111111"), &tip), codes::STALE);
    err(land(&root, "main", "k2/w", None, "abc"), codes::STALE);

    // Main checkout on another branch: refused.
    sh(&root, &["checkout", "-q", "-b", "elsewhere"]);
    err(land(&root, "main", "k2/w", None, &tip), codes::WRONG_BRANCH);
    sh(&root, &["checkout", "-q", "main"]);

    // A merge commit in range: refused.
    sh(&wt, &["checkout", "-q", "-b", "k2/w-side"]);
    commit_file(&wt, "side.txt", "s\n", "side");
    sh(&wt, &["checkout", "-q", "k2/w"]);
    commit_file(&wt, "b2.txt", "b2\n", "b2");
    sh(&wt, &["merge", "-q", "--no-ff", "-m", "merge side", "k2/w-side"]);
    let merged_tip = head(&wt, "HEAD");
    let e = err(land(&root, "main", "k2/w", None, &merged_tip), codes::HAS_MERGE);
    assert_eq!(e.commits.len(), 1, "{e:?}");

    assert_eq!(head(&root, "main"), main_head, "no refusal moved main");
    assert_eq!(std::fs::read_to_string(root.join("notes.txt")).expect("notes"), "mine\n");
    assert!(tracked_changes(&root).expect("status").is_empty());
}

#[test]
fn land_conflict_aborts_and_names_the_files() {
    let root = repo("conflict");
    let c = create(&root, "clash", None).expect("create");
    let wt = PathBuf::from(&c.path);
    let ok = commit_file(&wt, "new.txt", "new\n", "a clean one first");
    let _ = ok;
    let tip = commit_file(&wt, "a.txt", "one\ntwo\nTHEIRS\n", "theirs");
    let main_head = commit_file(&root, "a.txt", "one\ntwo\nOURS\n", "ours");

    let e = err(land(&root, "main", "k2/clash", Some(&main_head), &tip), codes::CONFLICT);
    assert_eq!(e.status, 409);
    assert_eq!(e.files, vec!["a.txt".to_string()]);
    assert!(e.hint.contains("a.txt"), "{}", e.hint);
    // Back exactly where it was: same head, no picked commit, no pick in progress, clean.
    assert_eq!(head(&root, "HEAD"), main_head);
    assert!(!root.join("new.txt").exists(), "the clean first commit was undone by the abort");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).expect("a"), "one\ntwo\nOURS\n");
    assert!(!root.join(".git/CHERRY_PICK_HEAD").exists());
    assert!(!root.join(".git/sequencer").exists());
    assert!(tracked_changes(&root).expect("status").is_empty());
    let r = review(&root, Some(&wt), "main", "k2/clash").expect("review");
    assert_eq!(r.commits.len(), 2);
}

#[test]
fn delete_branch_only_touches_k2_branches() {
    let root = repo("del");
    sh(&root, &["branch", "k2/gone"]);
    sh(&root, &["branch", "keep"]);
    delete_branch(&root, "k2/gone").expect("delete");
    assert!(sh(&root, &["branch", "--list", "k2/gone"]).is_empty());
    err(delete_branch(&root, "keep"), codes::GIT_FAILED);
    assert!(!sh(&root, &["branch", "--list", "keep"]).is_empty(), "a non-K2 branch is never deleted");
    delete_branch(&root, "k2/never-was").expect("missing is a no-op");
}

#[test]
fn dirty_files_counts_untracked_but_never_ignored() {
    let root = repo("dirty");
    std::fs::write(root.join(".gitignore"), "target/\n").expect("ignore");
    sh(&root, &["add", "-A"]);
    sh(&root, &["commit", "-q", "-m", "ignore"]);
    std::fs::create_dir_all(root.join("target")).expect("target");
    std::fs::write(root.join("target/big.bin"), "x").expect("ignored");
    assert!(dirty_files(&root).expect("status").is_empty(), "ignored files are not work");
    std::fs::write(root.join("new file.txt"), "n\n").expect("untracked");
    std::fs::write(root.join("a.txt"), "changed\n").expect("tracked");
    assert_eq!(dirty_files(&root).expect("status"), vec!["a.txt".to_string(), "new file.txt".to_string()]);
    assert_eq!(tracked_changes(&root).expect("status"), vec!["a.txt".to_string()]);
}

#[test]
fn task_state_matrix() {
    let fresh = store::WorktreeMeta { branch: "k2/x".into(), dir: ".worktrees/k2-x".into(), ..Default::default() };
    assert_eq!(task_state(&fresh, false, 0, false), TaskState::New);
    assert_eq!(task_state(&fresh, true, 0, false), TaskState::Working);
    assert_eq!(task_state(&fresh, true, 2, true), TaskState::Working);
    assert_eq!(task_state(&fresh, false, 2, false), TaskState::Review);
    assert_eq!(task_state(&fresh, false, 0, true), TaskState::Review);
    let landed = store::WorktreeMeta { landed_at: Some(5), landed_head: Some("abc".into()), ..fresh.clone() };
    assert_eq!(task_state(&landed, false, 0, false), TaskState::Landed);
    assert_eq!(task_state(&landed, false, 1, false), TaskState::Review, "new work after a land is review again");
    let retired =
        store::WorktreeMeta { retired_at: Some(9), retired_how: Some(store::RETIRED_LANDED.into()), ..landed.clone() };
    assert_eq!(task_state(&retired, true, 3, true), TaskState::Landed);
    let discarded =
        store::WorktreeMeta { retired_at: Some(9), retired_how: Some(store::RETIRED_DISCARDED.into()), ..fresh.clone() };
    assert_eq!(task_state(&discarded, false, 3, true), TaskState::Discarded);
    assert_eq!(TaskState::Review.as_str(), "review");
    assert_eq!(serde_json::to_value(TaskState::Discarded).expect("json"), serde_json::json!("discarded"));
}

#[test]
fn store_columns_from_0251() {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    crate::db::run_migrations(&conn).expect("migrations");
    conn.execute("INSERT INTO projects (id, name, path) VALUES ('p1', 'p1', '/tmp/p1')", []).expect("project");
    for pg in ["pg-a", "pg-b", "pg-plain"] {
        conn.execute(
            "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, last_seen_at) VALUES ('p1', ?1, ?2, 0)",
            rusqlite::params![pg, format!("tab-{pg}")],
        )
        .expect("tab row");
    }
    assert_eq!(store::get(&conn, "p1", "pg-a"), None, "a plain sidecar has no worktree");
    store::stamp_new(&conn, "p1", "pg-a", "ws-a", "k2/a", ".worktrees/k2-a", Some("main")).expect("stamp a");
    store::stamp_new(&conn, "p1", "pg-b", "ws-b", "k2/b", ".worktrees/k2-b", None).expect("stamp b");
    assert_eq!(store::open_count(&conn, "p1"), 2);
    let a = store::get(&conn, "p1", "pg-a").expect("a");
    assert_eq!((a.worktree_ws_id.as_deref(), a.branch.as_str(), a.dir.as_str(), a.base_ref.as_deref()), (Some("ws-a"), "k2/a", ".worktrees/k2-a", Some("main")));
    store::mark_landed(&conn, "p1", "pg-a", "deadbeef", 100).expect("landed");
    store::mark_retired(&conn, "p1", "pg-a", store::RETIRED_LANDED, 101).expect("retired");
    let a = store::get(&conn, "p1", "pg-a").expect("a");
    assert_eq!((a.landed_at, a.landed_head.as_deref(), a.retired_at, a.retired_how.as_deref()), (Some(100), Some("deadbeef"), Some(101), Some("landed")));
    assert_eq!(a.worktree_ws_id, None, "the workspaces row id is cleared on retire");
    assert_eq!(a.branch, "k2/a", "the branch name is kept for the inbox");
    assert_eq!(store::open_count(&conn, "p1"), 1);
    assert!(store::stamp_new(&conn, "p1", "pg-none", "w", "k2/n", ".worktrees/k2-n", None).is_err(), "no row → loud error");
    assert!(store::mark_retired(&conn, "p1", "pg-none", store::RETIRED_DISCARDED, 1).is_err());
}

#[test]
fn names_and_sha_prefixes() {
    assert_eq!(branch_for("fix-login"), "k2/fix-login");
    assert_eq!(dir_for("fix-login"), ".worktrees/k2-fix-login");
    let full = "0123456789abcdef0123456789abcdef01234567";
    assert!(sha_matches(full, full));
    assert!(sha_matches(full, "0123456"));
    assert!(sha_matches(full, "0123456789ABC"));
    assert!(!sha_matches(full, "012345"), "shorter than 7 never matches");
    assert!(!sha_matches(full, "1123456"));
}

#[test]
fn interrupt_keys_per_harness() {
    for p in ["claude", "grok", "gemini", "codex", "pi"] {
        assert_eq!(crate::sidecar::interrupt_bytes(p), b"\x1b", "{p}");
    }
    for p in ["cursor", "hermes"] {
        assert_eq!(crate::sidecar::interrupt_bytes(p), b"\x03", "{p}");
    }
}
