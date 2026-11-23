//! S1: the simple reads moved from libgit2 to gix, checked against the git
//! CLI on CLI-made repos.

use crate::git_fixture::{self as fx, write};
use std::path::{Path, PathBuf};

fn canon(p: impl AsRef<Path>) -> PathBuf {
    dunce::canonicalize(p.as_ref()).unwrap_or_else(|e| panic!("canonicalize {}: {e}", p.as_ref().display()))
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf8 path")
}

#[test]
fn seed_build_head_matches_git() {
    let d = fx::init("s1-head");
    assert_eq!(crate::deploy::build::git_head(&d), None, "unborn: no head");
    write(&d, "a.txt", "a\n");
    let c = fx::commit_all(&d, "c1");
    assert_eq!(crate::deploy::build::git_head(&d), Some(c.clone()));
    std::fs::create_dir_all(d.join("sub")).expect("mkdir");
    assert_eq!(crate::deploy::build::git_head(&d.join("sub")), Some(c), "discovers upwards");
    assert_eq!(crate::deploy::build::git_head(&fx::tmp("s1-head-none")), None);
}

#[test]
fn head_branch_and_branch_names_without_the_cli() {
    let d = fx::init("s1-branch");
    assert_eq!(crate::git_tools::head_branch_without_cli(s(&d)), None, "unborn");
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "c1");
    assert_eq!(crate::git_tools::head_branch_without_cli(s(&d)).as_deref(), Some("main"));
    fx::git(&d, &["checkout", "-q", "-b", "feat/x"]);
    assert_eq!(crate::git_tools::head_branch_without_cli(s(&d)).as_deref(), Some("feat/x"));
    fx::git(&d, &["checkout", "-q", "--detach"]);
    assert_eq!(crate::git_tools::head_branch_without_cli(s(&d)), None, "detached is no branch");
    assert_eq!(crate::git_tools::valid_branch(" main ").expect("ok"), "main");
    for bad in ["", "-x", "a..b", "a b", "x.lock", "a~b"] {
        assert!(crate::git_tools::valid_branch(bad).is_err(), "{bad:?}");
    }
    // git agrees on the ones it can be asked about.
    for bad in ["a..b", "a b", "x.lock", "a~b"] {
        let ok = fx::git_cmd(&d).args(["check-ref-format", "--branch", bad]).output().expect("check-ref-format").status.success();
        assert!(!ok, "git accepts {bad:?}");
    }
}

#[test]
fn repo_dirs_match_git_for_main_and_linked_worktrees() {
    let d = fx::init("s1-dirs");
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "c1");
    let main = crate::git_changes::repo_dirs(s(&d)).expect("dirs");
    assert_eq!(canon(&main.gitdir), canon(fx::git(&d, &["rev-parse", "--path-format=absolute", "--git-dir"])));
    assert_eq!(canon(&main.commondir), canon(&main.gitdir));
    assert_eq!(canon(&main.workdir), canon(&d));
    let wt = fx::tmp("s1-dirs-wt").join("co");
    fx::git(&d, &["worktree", "add", "-q", "-b", "wt1", s(&wt)]);
    let linked = crate::git_changes::repo_dirs(s(&wt)).expect("wt dirs");
    assert_eq!(canon(&linked.gitdir), canon(fx::git(&wt, &["rev-parse", "--path-format=absolute", "--git-dir"])));
    assert_eq!(canon(&linked.commondir), canon(fx::git(&wt, &["rev-parse", "--path-format=absolute", "--git-common-dir"])));
    assert_eq!(canon(&linked.workdir), canon(&wt));
    let (_, root) = crate::git_changes::repo_key_and_root(s(&wt.join("."))).expect("key");
    assert_eq!(canon(root), canon(&wt));
    assert!(crate::git_changes::repo_key_and_root(s(&fx::tmp("s1-dirs-none"))).is_none());
}

#[test]
fn note_sides_match_git_blob_ids_and_texts() {
    let d = fx::init("s1-notes");
    write(&d, "f.txt", "head\n");
    fx::commit_all(&d, "c1");
    write(&d, "f.txt", "index\n");
    fx::stage(&d, "f.txt");
    write(&d, "f.txt", "work\r\nline\n");
    let new_id = crate::git_notes::side_blob_id(s(&d), "f.txt", "new").expect("new id");
    assert_eq!(new_id, fx::git(&d, &["hash-object", "--no-filters", "f.txt"]), "raw bytes, no filters");
    let old_id = crate::git_notes::side_blob_id(s(&d), "f.txt", "old").expect("old id");
    assert_eq!(old_id, fx::git(&d, &["rev-parse", "HEAD:f.txt"]));
    assert_eq!(crate::git_notes::side_texts(s(&d), "f.txt", "new"), vec!["work\r\nline\n".to_string(), "index\n".to_string()]);
    assert_eq!(crate::git_notes::side_texts(s(&d), "f.txt", "old"), vec!["head\n".to_string(), "index\n".to_string()]);
    assert!(crate::git_notes::side_blob_id(s(&d), "missing.txt", "old").is_none());
}

#[test]
fn ignore_checker_follows_gitignore_like_git() {
    let d = fx::init("s1-ignore");
    write(&d, ".gitignore", "*.log\nbuild/\n!keep.log\n");
    write(&d, "a.txt", "a\n");
    fx::commit_all(&d, "c1");
    write(&d, "build/out/x.o", "o\n");
    write(&d, "x.log", "l\n");
    write(&d, "keep.log", "k\n");
    write(&d, "src/y.txt", "y\n");
    let check = |c: &mut crate::git_changes::IgnoreChecker, rel: &str| -> bool {
        let k2 = c.is_ignored(rel);
        let git = fx::git_cmd(&d).args(["check-ignore", "-q", "--", rel]).output().expect("check-ignore").status.success();
        assert_eq!(k2, git, "{rel}: K2 says ignored={k2}, git says {git}");
        k2
    };
    let mut c = crate::git_changes::IgnoreChecker::open(&d).expect("checker");
    assert!(check(&mut c, "x.log"));
    assert!(!check(&mut c, "keep.log"));
    assert!(check(&mut c, "build"));
    assert!(check(&mut c, "build/out/x.o"), "inside an ignored folder");
    assert!(!check(&mut c, "src/y.txt"));
    assert!(!check(&mut c, "a.txt"));
    // An edited .gitignore counts from the next checker on.
    write(&d, ".gitignore", "src/\n");
    let mut c = crate::git_changes::IgnoreChecker::open(&d).expect("checker");
    assert!(check(&mut c, "src/y.txt"));
    assert!(!check(&mut c, "x.log"));
}

#[test]
fn an_operation_in_progress_is_seen() {
    let d = fx::init("s1-state");
    write(&d, "a.txt", "base\n");
    fx::commit_all(&d, "c1");
    let repo = crate::gitx::open(&d).expect("open");
    assert_eq!(crate::gitx::state_name(&repo), "clean");
    fx::git(&d, &["checkout", "-q", "-b", "other"]);
    write(&d, "a.txt", "theirs\n");
    fx::commit_all(&d, "theirs");
    fx::git(&d, &["checkout", "-q", "main"]);
    write(&d, "a.txt", "ours\n");
    fx::commit_all(&d, "ours");
    let m = fx::git_cmd(&d).args(["merge", "-q", "other"]).output().expect("merge");
    assert!(!m.status.success(), "fixture: the merge conflicts");
    let repo = crate::gitx::open(&d).expect("open");
    assert_eq!(crate::gitx::state_name(&repo), "merge");
    assert_eq!(crate::gitx::conflicted_paths(&repo.index_or_empty().expect("index")), vec!["a.txt".to_string()]);
}
