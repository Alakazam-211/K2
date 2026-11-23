//! A caller's revision (ref, base or sha) can't be a commit-message
//! search: `:/text` and `<rev>^{/text}` are refused, anywhere in the
//! string, before anything resolves it. Branches, tags, shas, `~`/`^`
//! walks, `@{n}` and `<rev>:<path>` still work.
//!
//! The per-site tests use a repo whose only commit says "fix", so
//! `:/fix` would resolve if the guard were gone: each test then fails.

use super::*;
use crate::git_changes::{self, DiffMode};
use std::path::{Path, PathBuf};

const REFUSED: &str = "search commit messages";

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-revspec-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    dunce::canonicalize(&d).expect("canonical temp")
}

/// A repo with a.txt in one commit whose message is "fix".
fn repo_with_fix_commit(tag: &str) -> PathBuf {
    let d = temp(tag);
    crate::git_fixture::init_at(&d);
    std::fs::write(d.join("a.txt"), "a1\n").expect("write a.txt");
    crate::git_fixture::commit_all(&d, "fix");
    d
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf8 path")
}

#[test]
fn message_search_specs_are_refused_anywhere_in_the_string() {
    for spec in [":/foo", "HEAD^{/fix}", "main:/x", "abc:/"] {
        let err = refuse_message_search_revspec(spec).expect_err(spec);
        assert!(err.contains(REFUSED), "{spec}: {err}");
        assert!(err.contains("name a branch, tag or commit"), "{spec}: {err}");
    }
}

#[test]
fn plain_revision_specs_are_still_allowed() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    for spec in ["main", "HEAD~2", "v1.2^{commit}", sha, "origin/main", "HEAD@{1}", "main:src/lib.rs"] {
        refuse_message_search_revspec(spec).unwrap_or_else(|e| panic!("{spec} refused: {e}"));
    }
}

#[test]
fn file_content_at_ref_refuses_a_message_search_ref() {
    let d = repo_with_fix_commit("file-at-ref");
    assert_eq!(file_content_at_ref(s(&d), "a.txt", "HEAD").expect("HEAD still reads"), "a1\n");
    for spec in [":/fix", "HEAD^{/fix}"] {
        let err = file_content_at_ref(s(&d), "a.txt", spec).expect_err(spec);
        assert!(err.contains(REFUSED), "{spec}: {err}");
    }
}

#[test]
fn changes_snapshot_refuses_a_message_search_base() {
    let d = repo_with_fix_commit("snapshot");
    git_changes::snapshot(s(&d), Some("HEAD")).expect("HEAD still works as a base");
    for spec in [":/fix", "HEAD^{/fix}"] {
        let err = git_changes::snapshot(s(&d), Some(spec)).expect_err(spec);
        assert_eq!(err.code, git_changes::codes::BAD_REF, "{spec}");
        assert!(err.hint.contains(REFUSED), "{spec}: {}", err.hint);
    }
}

#[test]
fn diff_sides_refuses_a_message_search_base() {
    let d = repo_with_fix_commit("diff-sides");
    std::fs::write(d.join("a.txt"), "a2\n").expect("edit a.txt");
    git_changes::diff_sides(s(&d), "a.txt", None, DiffMode::Base("HEAD".into())).expect("HEAD still works as a base");
    for spec in [":/fix", "HEAD^{/fix}"] {
        let err = git_changes::diff_sides(s(&d), "a.txt", None, DiffMode::Base(spec.into())).expect_err(spec);
        assert_eq!(err.code, git_changes::codes::BAD_REF, "{spec}");
        assert!(err.hint.contains(REFUSED), "{spec}: {}", err.hint);
    }
}

#[test]
fn worktree_base_refuses_a_message_search_base() {
    let d = repo_with_fix_commit("wt-base");
    crate::worktree_task::resolve_base(&d, Some("HEAD")).expect("HEAD still works as a base");
    for spec in [":/fix", "HEAD^{/fix}"] {
        let err = crate::worktree_task::resolve_base(&d, Some(spec)).expect_err(spec);
        assert_eq!(err.code, crate::worktree_task::codes::BASE_NOT_FOUND, "{spec}");
        assert!(err.hint.contains(REFUSED), "{spec}: {}", err.hint);
    }
}

#[test]
fn compute_resolve_commit_refuses_a_message_search_rev() {
    let d = repo_with_fix_commit("compute-rev");
    let head = crate::compute::sync::resolve_commit(&d, "HEAD").expect("HEAD still resolves");
    assert_eq!(head.len(), 40, "{head}");
    for spec in [":/fix", "HEAD^{/fix}"] {
        let err = crate::compute::sync::resolve_commit(&d, spec).expect_err(spec);
        assert!(err.contains(REFUSED), "{spec}: {err}");
    }
}
