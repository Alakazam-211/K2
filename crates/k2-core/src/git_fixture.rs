//! Test repositories made by the real `git` CLI (research §6.1), so K2's
//! gix reads are checked against what a person's git makes, never gix
//! against gix. The fixture's git runs with no global or system config
//! (`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`), so a builder's
//! own config can't leak in; the repo gets a local identity so K2's own
//! CLI-backed commits work in it.
//!
//! Tests need git: a missing git fails loudly, never skips.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `/dev/null` (`NUL` on Windows): the empty config file.
pub const NULL_CONFIG: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// A plain `git` for fixtures, isolated from the machine's config.
pub fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .env("GIT_CONFIG_GLOBAL", NULL_CONFIG)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "K2 Test")
        .env("GIT_AUTHOR_EMAIL", "k2-test@example.com")
        .env("GIT_COMMITTER_NAME", "K2 Test")
        .env("GIT_COMMITTER_EMAIL", "k2-test@example.com")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    c
}

/// Run fixture git in `dir`; panics with git's stderr on failure. Returns
/// stdout, trimmed.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_cmd(dir).args(args).output().unwrap_or_else(|e| panic!("run git {args:?} (tests need git): {e}"));
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Like [`git`], but stdout untrimmed (exact output for oracle compares).
pub fn git_raw(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = git_cmd(dir).args(args).output().unwrap_or_else(|e| panic!("run git {args:?} (tests need git): {e}"));
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// A fresh temp dir.
pub fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-gitfx-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    d
}

/// `git init` (branch `main`) in a fresh temp dir, with a local identity.
pub fn init(tag: &str) -> PathBuf {
    let d = tmp(tag);
    init_at(&d);
    d
}

/// `git init` at `dir` (which exists), with a local identity.
pub fn init_at(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "K2 Test"]);
    git(dir, &["config", "user.email", "k2-test@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

pub fn write(dir: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir parent");
    }
    std::fs::write(p, body).expect("write file");
}

/// `git add -A && git commit`; returns the new commit id.
pub fn commit_all(dir: &Path, msg: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

/// `git add -- <rel>`.
pub fn stage(dir: &Path, rel: &str) {
    git(dir, &["add", "--", rel]);
}

/// `git rm --cached -- <rel>`.
pub fn stage_remove(dir: &Path, rel: &str) {
    git(dir, &["rm", "-q", "--cached", "--", rel]);
}

/// `git ls-files -s`: every index entry as `mode id stage\tpath`.
pub fn ls_files_stage(dir: &Path) -> String {
    git(dir, &["ls-files", "-s"])
}
