//! Code in (§11.1, CN29): the `git` CLI only. Per project a bare mirror
//! `repos/<project_key>.git`; history comes from the project's remote when
//! the node can reach it, and only the commits it lacks come from the
//! controller as a bundle (`refs/k2-compute/<job_id>`). `--dirty` adds a
//! tar of `k2-dirty/patch.diff` + `k2-dirty/untracked/…`. Each job gets
//! its own detached worktree.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::util::{run_cmd, CmdOut};

const FETCH_LIMIT: Duration = Duration::from_secs(600);
const LOCAL_LIMIT: Duration = Duration::from_secs(300);

/// Env for git: nothing inherited, no prompts, no system/global config.
pub fn git_env(git_home: &Path, path: &str) -> Vec<(String, String)> {
    vec![
        ("PATH".into(), path.into()),
        ("HOME".into(), git_home.display().to_string()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("LANG".into(), "C".into()),
    ]
}

pub struct Git {
    pub program: String,
    pub env: Vec<(String, String)>,
}

fn check(what: &str, out: CmdOut) -> Result<CmdOut, String> {
    if out.ok {
        Ok(out)
    } else {
        let msg = out.stderr.trim();
        Err(format!("{what}: {}", if msg.is_empty() { "failed" } else { msg }))
    }
}

pub fn valid_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

impl Git {
    /// `None` when there is no `git` on the job PATH (→ `tool_missing`).
    pub fn find(git_home: &Path, path: &str) -> Option<Self> {
        let env = git_env(git_home, path);
        let program = crate::sysinfo::which("git", &env)?;
        Some(Self { program, env })
    }

    async fn git(&self, args: &[&str], cwd: Option<&Path>, limit: Duration) -> Result<CmdOut, String> {
        run_cmd(&self.program, args, &self.env, cwd, limit).await
    }

    pub async fn ensure_mirror(&self, mirror: &Path) -> Result<(), String> {
        if mirror.join("HEAD").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(mirror).map_err(|e| format!("create {}: {e}", mirror.display()))?;
        let m = mirror.display().to_string();
        check("git init", self.git(&["init", "--bare", "-q", &m], None, LOCAL_LIMIT).await?)?;
        check("git config", self.git(&["--git-dir", &m, "config", "gc.auto", "0"], None, LOCAL_LIMIT).await?)?;
        Ok(())
    }

    pub async fn has_commit(&self, mirror: &Path, sha: &str) -> bool {
        let spec = format!("{sha}^{{commit}}");
        let m = mirror.display().to_string();
        matches!(self.git(&["--git-dir", &m, "cat-file", "-e", &spec], None, LOCAL_LIMIT).await, Ok(o) if o.ok)
    }

    pub async fn fetch_remote(&self, mirror: &Path, url: &str) -> Result<(), String> {
        if url.starts_with('-') {
            return Err("remote url can't start with '-'".into());
        }
        let m = mirror.display().to_string();
        check(
            "git fetch",
            self.git(&["--git-dir", &m, "fetch", "-q", "--no-tags", url, "+refs/heads/*:refs/remotes/origin/*"], None, FETCH_LIMIT)
                .await?,
        )?;
        Ok(())
    }

    /// Up to 50 distinct ref tips, newest first.
    pub async fn tips(&self, mirror: &Path) -> Result<Vec<String>, String> {
        let m = mirror.display().to_string();
        let out = check(
            "git for-each-ref",
            self.git(&["--git-dir", &m, "for-each-ref", "--sort=-committerdate", "--format=%(objectname)"], None, LOCAL_LIMIT)
                .await?,
        )?;
        let mut v: Vec<String> = Vec::new();
        for l in out.stdout.lines() {
            let l = l.trim().to_string();
            if valid_sha(&l) && !v.contains(&l) {
                v.push(l);
            }
            if v.len() >= 50 {
                break;
            }
        }
        Ok(v)
    }

    pub async fn fetch_bundle(&self, mirror: &Path, bundle: &Path) -> Result<(), String> {
        let m = mirror.display().to_string();
        let b = bundle.display().to_string();
        check(
            "git fetch bundle",
            self.git(&["--git-dir", &m, "fetch", "-q", "--no-tags", &b, "+refs/k2-compute/*:refs/k2-compute/*"], None, LOCAL_LIMIT)
                .await?,
        )?;
        Ok(())
    }

    pub async fn worktree_add(&self, mirror: &Path, dir: &Path, sha: &str) -> Result<(), String> {
        let m = mirror.display().to_string();
        let d = dir.display().to_string();
        check("git worktree add", self.git(&["--git-dir", &m, "worktree", "add", "-q", "--detach", &d, sha], None, LOCAL_LIMIT).await?)?;
        Ok(())
    }

    pub async fn tree_of(&self, mirror: &Path, sha: &str) -> Result<String, String> {
        let m = mirror.display().to_string();
        let spec = format!("{sha}^{{tree}}");
        let out = check("git rev-parse", self.git(&["--git-dir", &m, "rev-parse", &spec], None, LOCAL_LIMIT).await?)?;
        Ok(out.stdout.trim().to_string())
    }

    pub async fn worktree_remove(&self, mirror: &Path, dir: &Path) {
        let m = mirror.display().to_string();
        let d = dir.display().to_string();
        let _ = self.git(&["--git-dir", &m, "worktree", "remove", "--force", &d], None, LOCAL_LIMIT).await;
        let _ = std::fs::remove_dir_all(dir);
        let _ = self.git(&["--git-dir", &m, "worktree", "prune"], None, LOCAL_LIMIT).await;
    }

    /// Extract the dirty blob into `extract` and apply it to `worktree`.
    pub async fn apply_dirty(&self, worktree: &Path, blob: &Path, extract: &Path) -> Result<(), String> {
        std::fs::create_dir_all(extract).map_err(|e| format!("create {}: {e}", extract.display()))?;
        let tar = crate::sysinfo::which("tar", &self.env).ok_or("tar is not installed")?;
        let b = blob.display().to_string();
        let x = extract.display().to_string();
        check("tar", run_cmd(&tar, &["-xf", &b, "-C", &x], &self.env, None, LOCAL_LIMIT).await?)?;
        let root = extract.join("k2-dirty");
        if !root.is_dir() {
            return Err("dirty blob has no k2-dirty/ folder".into());
        }
        let patch = root.join("patch.diff");
        if patch.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            let p = patch.display().to_string();
            check(
                "git apply",
                self.git(&["apply", "--binary", "--whitespace=nowarn", &p], Some(worktree), LOCAL_LIMIT).await?,
            )?;
        }
        let untracked = root.join("untracked");
        if untracked.is_dir() {
            copy_tree(&untracked, worktree)?;
        }
        Ok(())
    }
}

/// Copy files and symlinks from `src` into `dst`, refusing escapes.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    for e in std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let e = e.map_err(|e| format!("read dir: {e}"))?;
        let name = e.file_name();
        if name == ".." || name == "." || name == ".git" {
            continue;
        }
        let from = e.path();
        let to: PathBuf = dst.join(&name);
        let ft = e.file_type().map_err(|e| format!("stat {}: {e}", from.display()))?;
        if ft.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| format!("create {}: {e}", to.display()))?;
            copy_tree(&from, &to)?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(&from).map_err(|e| format!("readlink {}: {e}", from.display()))?;
            let _ = std::fs::remove_file(&to);
            std::os::unix::fs::symlink(target, &to).map_err(|e| format!("symlink {}: {e}", to.display()))?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", to.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A plain git in a temp repo with fixed identity (no global config).
    pub fn sh_git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin")
            .env("HOME", repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo with two commits; returns (repo, first, second).
    pub fn repo_with_commits(tag: &str) -> (PathBuf, String, String) {
        let d = crate::util::temp_dir(tag).join("repo");
        std::fs::create_dir_all(&d).unwrap();
        sh_git(&d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("a.txt"), "one\n").unwrap();
        sh_git(&d, &["add", "."]);
        sh_git(&d, &["commit", "-q", "-m", "one"]);
        let first = sh_git(&d, &["rev-parse", "HEAD"]);
        std::fs::write(d.join("a.txt"), "two\n").unwrap();
        std::fs::write(d.join("bin.dat"), [0u8, 159, 146, 150, 1, 2]).unwrap();
        sh_git(&d, &["add", "."]);
        sh_git(&d, &["commit", "-q", "-m", "two"]);
        let second = sh_git(&d, &["rev-parse", "HEAD"]);
        (d, first, second)
    }

    fn git(home: &Path) -> Git {
        Git::find(home, "/usr/bin:/bin:/opt/homebrew/bin").expect("git on PATH")
    }

    #[tokio::test]
    async fn bundle_into_mirror_then_worktree_at_commit() {
        let (repo, first, second) = repo_with_commits("sync-bundle");
        let home = crate::util::temp_dir("sync-home");
        let g = git(&home);
        let mirror = home.join("m.git");
        g.ensure_mirror(&mirror).await.unwrap();
        assert!(!g.has_commit(&mirror, &second).await);
        assert!(g.tips(&mirror).await.unwrap().is_empty());
        // What the controller does: a temp ref, a bundle of have..sha.
        sh_git(&repo, &["update-ref", "refs/k2-compute/job1", &second]);
        let bundle = home.join("b.bundle");
        sh_git(&repo, &["bundle", "create", bundle.to_str().unwrap(), "refs/k2-compute/job1"]);
        g.fetch_bundle(&mirror, &bundle).await.unwrap();
        assert!(g.has_commit(&mirror, &second).await);
        assert!(g.has_commit(&mirror, &first).await);
        assert_eq!(g.tips(&mirror).await.unwrap(), vec![second.clone()]);
        let wt = home.join("jobs/j/src");
        g.worktree_add(&mirror, &wt, &first).await.unwrap();
        assert_eq!(std::fs::read_to_string(wt.join("a.txt")).unwrap(), "one\n");
        let tree = g.tree_of(&mirror, &first).await.unwrap();
        assert_eq!(tree, sh_git(&repo, &["rev-parse", &format!("{first}^{{tree}}")]));
        g.worktree_remove(&mirror, &wt).await;
        assert!(!wt.exists());
    }

    /// The dirty blob as the controller builds it.
    pub fn make_dirty_blob(repo: &Path, out: &Path) {
        let stage = out.parent().unwrap().join("stage");
        let root = stage.join("k2-dirty");
        std::fs::create_dir_all(root.join("untracked/sub")).unwrap();
        let diff = std::process::Command::new("git")
            .args(["diff", "--binary", "HEAD"])
            .current_dir(repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        std::fs::write(root.join("patch.diff"), diff.stdout).unwrap();
        std::fs::write(root.join("untracked/sub/new.txt"), "untracked\n").unwrap();
        let st = std::process::Command::new("tar")
            .args(["-cf", out.to_str().unwrap(), "-C", stage.to_str().unwrap(), "k2-dirty"])
            .status()
            .unwrap();
        assert!(st.success());
    }

    #[tokio::test]
    async fn dirty_blob_applies_modified_binary_and_untracked() {
        let (repo, _first, second) = repo_with_commits("sync-dirty");
        std::fs::write(repo.join("a.txt"), "dirty\n").unwrap();
        std::fs::write(repo.join("bin.dat"), [9u8, 0, 255, 7]).unwrap();
        let home = crate::util::temp_dir("sync-dirty-home");
        let blob = home.join("dirty.tar");
        make_dirty_blob(&repo, &blob);
        let g = git(&home);
        let mirror = home.join("m.git");
        g.ensure_mirror(&mirror).await.unwrap();
        g.fetch_remote(&mirror, repo.to_str().unwrap()).await.unwrap();
        assert!(g.has_commit(&mirror, &second).await, "fetch from a reachable remote");
        let wt = home.join("src");
        g.worktree_add(&mirror, &wt, &second).await.unwrap();
        g.apply_dirty(&wt, &blob, &home.join("x")).await.unwrap();
        assert_eq!(std::fs::read_to_string(wt.join("a.txt")).unwrap(), "dirty\n");
        assert_eq!(std::fs::read(wt.join("bin.dat")).unwrap(), vec![9u8, 0, 255, 7]);
        assert_eq!(std::fs::read_to_string(wt.join("sub/new.txt")).unwrap(), "untracked\n");
    }

    #[test]
    fn sha_validation() {
        assert!(valid_sha(&"a".repeat(40)));
        assert!(!valid_sha(&"A".repeat(40)));
        assert!(!valid_sha("HEAD"));
        assert!(!valid_sha(&"g".repeat(40)));
    }
}
