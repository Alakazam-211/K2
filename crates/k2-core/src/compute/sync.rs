//! Code in, controller side (§11.1, CN29): the `git` CLI, never git2.
//!
//! The controller only READS the workspace repo. Bundles are built from a
//! throwaway bare repo whose `objects/info/alternates` points at the
//! workspace's object store, so no ref is ever written into the human's
//! repo. History normally doesn't cross the relay at all: the node
//! fetches from the project's remote itself, and a bundle carries only
//! `have..commit`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::proto::crypto::{self, Sha256Stream};

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir);
    c.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    c
}

fn run(mut c: Command, what: &str) -> Result<String, String> {
    let out = c.output().map_err(|e| format!("{what}: can't run git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{what}: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The repo's top-level directory for `dir`.
pub fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    let mut c = git(dir);
    c.args(["rev-parse", "--show-toplevel"]);
    run(c, "find the repo").map(PathBuf::from)
}

/// The shared git dir (a worktree's `.git` is a file pointing here).
fn common_dir(root: &Path) -> Result<PathBuf, String> {
    let mut c = git(root);
    c.args(["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    run(c, "find the git dir").map(PathBuf::from)
}

/// Full 40-hex commit for `rev`.
pub fn resolve_commit(root: &Path, rev: &str) -> Result<String, String> {
    if rev.starts_with('-') {
        return Err(format!("bad revision '{rev}'"));
    }
    let mut c = git(root);
    c.args(["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{rev}^{{commit}}")]);
    run(c, &format!("resolve '{rev}'"))
}

/// The tree of `commit`.
pub fn tree_of(root: &Path, commit: &str) -> Result<String, String> {
    let mut c = git(root);
    c.args(["rev-parse", "--verify", &format!("{commit}^{{tree}}")]);
    run(c, "read the tree")
}

/// True when the working tree has changes against HEAD (tracked or untracked).
pub fn is_dirty(root: &Path) -> Result<bool, String> {
    let mut c = git(root);
    c.args(["status", "--porcelain", "--untracked-files=normal"]);
    run(c, "read status").map(|s| !s.is_empty())
}

/// A remote URL the node may fetch from WITHOUT credentials: `https://`
/// with any userinfo stripped, or a `git@host:org/repo` / `ssh://` form
/// rewritten to `https://host/org/repo`. `None` for local paths and
/// anything else. Never hands a token to a node.
pub fn fetchable_remote(url: &str) -> Option<String> {
    let u = url.trim();
    if let Some(rest) = u.strip_prefix("https://") {
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        let host = rest.split('/').next().unwrap_or("");
        if host.is_empty() || !rest.contains('/') {
            return None;
        }
        return Some(format!("https://{rest}"));
    }
    let scp = if let Some(rest) = u.strip_prefix("ssh://") {
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        let host = host.split(':').next().unwrap_or(host);
        Some((host.to_string(), path.to_string()))
    } else if let Some((user_host, path)) = u.split_once(':') {
        let host = user_host.rsplit_once('@').map(|(_, h)| h).unwrap_or(user_host);
        (u.contains('@') && !path.starts_with('/') && !host.contains('/')).then(|| (host.to_string(), path.to_string()))
    } else {
        None
    };
    let (host, path) = scp?;
    if host.is_empty() || path.is_empty() || !host.contains('.') {
        return None;
    }
    Some(format!("https://{host}/{}", path.trim_start_matches('/')))
}

/// The origin's fetchable URL, if any.
pub fn origin_url(root: &Path) -> Option<String> {
    let mut c = git(root);
    c.args(["remote", "get-url", "origin"]);
    run(c, "read origin").ok().and_then(|u| fetchable_remote(&u))
}

/// Names the node's mirror and warm slots: hash of the workspace id and
/// the remote (16 hex).
pub fn project_key(workspace_id: &str, remote: Option<&str>) -> String {
    let h = crypto::sha256_hex(&crypto::signed_bytes("k2-compute-project-v1", &[workspace_id.as_bytes(), remote.unwrap_or("").as_bytes()]));
    h[..16].to_string()
}

/// The ref a bundle names its commit under (the node fetches
/// `refs/k2-compute/*`).
pub fn bundle_ref(job_id: &str) -> String {
    format!("refs/k2-compute/{job_id}")
}

fn is_hex40(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `git bundle` of `have..commit` into `out`. Haves the controller doesn't
/// have are dropped (the bundle then carries more history). Returns the
/// bundle size.
pub fn create_bundle(root: &Path, commit: &str, have: &[String], job_id: &str, out: &Path) -> Result<u64, String> {
    if !is_hex40(commit) || !job_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("bad bundle request".to_string());
    }
    let objects = common_dir(root)?.join("objects");
    let tmp = out.with_extension("tmp-repo");
    let _ = std::fs::remove_dir_all(&tmp);
    let result = (|| {
        let mut c = Command::new("git");
        c.args(["init", "--bare", "-q"]).arg(&tmp).env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
        run(c, "make the bundle repo")?;
        std::fs::write(tmp.join("objects/info/alternates"), format!("{}\n", objects.display()))
            .map_err(|e| format!("write alternates: {e}"))?;
        let gd = |args: &[&str]| {
            let mut c = Command::new("git");
            c.arg("--git-dir").arg(&tmp).args(args).env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
            c
        };
        let r = bundle_ref(job_id);
        run(gd(&["update-ref", &r, commit]), "name the commit")?;
        let mut args: Vec<String> = vec!["bundle".into(), "create".into(), "-q".into(), out.display().to_string(), r.clone()];
        for h in have.iter().filter(|h| is_hex40(h)).take(200) {
            let mut probe = gd(&["cat-file", "-e", &format!("{h}^{{commit}}")]);
            probe.stdout(Stdio::null()).stderr(Stdio::null());
            if probe.status().is_ok_and(|s| s.success()) {
                args.push(format!("^{h}"));
            }
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run(gd(&refs), "create the bundle")?;
        std::fs::metadata(out).map(|m| m.len()).map_err(|e| format!("bundle size: {e}"))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// SHA-256 hex and size of a file.
pub fn file_sha256(path: &Path) -> Result<(String, u64), String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut h = Sha256Stream::default();
    let mut buf = vec![0u8; 256 * 1024];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        n_total += n as u64;
    }
    Ok((h.finish_hex(), n_total))
}

/// The `--dirty` blob: a tar with `k2-dirty/patch.diff` (`git diff
/// --binary HEAD`, possibly empty) and `k2-dirty/untracked/<path>` for
/// every untracked, not-ignored file. Refuses over `cap` bytes of input.
/// Returns `(sha256, bytes)` of the tar.
pub fn create_dirty_blob(root: &Path, out: &Path, cap: u64) -> Result<(String, u64), String> {
    let staging = out.with_extension("staging");
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| {
        let base = staging.join("k2-dirty");
        std::fs::create_dir_all(base.join("untracked")).map_err(|e| format!("staging: {e}"))?;
        let mut c = git(root);
        c.args(["diff", "--binary", "HEAD"]);
        let diff = c.output().map_err(|e| format!("git diff: {e}"))?;
        if !diff.status.success() {
            return Err(format!("git diff: {}", String::from_utf8_lossy(&diff.stderr).trim()));
        }
        let mut total = diff.stdout.len() as u64;
        std::fs::write(base.join("patch.diff"), &diff.stdout).map_err(|e| format!("write patch: {e}"))?;
        let mut c = git(root);
        c.args(["ls-files", "--others", "--exclude-standard", "-z"]);
        let list = c.output().map_err(|e| format!("git ls-files: {e}"))?;
        if !list.status.success() {
            return Err(format!("git ls-files: {}", String::from_utf8_lossy(&list.stderr).trim()));
        }
        for rel in list.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            let rel = std::str::from_utf8(rel).map_err(|_| "an untracked path isn't UTF-8".to_string())?;
            if rel.split('/').any(|p| p == ".." || p.is_empty()) || rel.starts_with('/') {
                return Err(format!("unsafe untracked path '{rel}'"));
            }
            let src = root.join(rel);
            let dst = base.join("untracked").join(rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("staging dir: {e}"))?;
            }
            let meta = std::fs::symlink_metadata(&src).map_err(|e| format!("stat {rel}: {e}"))?;
            if meta.file_type().is_symlink() {
                #[cfg(unix)]
                {
                    let target = std::fs::read_link(&src).map_err(|e| format!("readlink {rel}: {e}"))?;
                    std::os::unix::fs::symlink(target, &dst).map_err(|e| format!("symlink {rel}: {e}"))?;
                }
                continue;
            }
            total += meta.len();
            if total > cap {
                return Err(format!("the working-tree changes are over {} MB", cap >> 20));
            }
            std::fs::copy(&src, &dst).map_err(|e| format!("copy {rel}: {e}"))?;
        }
        if total > cap {
            return Err(format!("the working-tree changes are over {} MB", cap >> 20));
        }
        let status = Command::new("tar")
            .arg("-cf")
            .arg(out)
            .arg("-C")
            .arg(&staging)
            .arg("k2-dirty")
            .env("COPYFILE_DISABLE", "1")
            .stdin(Stdio::null())
            .status()
            .map_err(|e| format!("tar: {e}"))?;
        if !status.success() {
            return Err("tar failed".to_string());
        }
        file_sha256(out)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-compute-sync-{tag}-{}-{}", std::process::id(), crate::compute::now()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn repo(d: &Path) -> PathBuf {
        let r = d.join("ws");
        std::fs::create_dir_all(&r).unwrap();
        g(&r, &["init", "-q"]);
        std::fs::write(r.join("a.txt"), "one\n").unwrap();
        g(&r, &["add", "."]);
        g(&r, &["commit", "-q", "-m", "one"]);
        r
    }

    #[test]
    fn fetchable_remote_strips_credentials_and_rewrites_ssh() {
        assert_eq!(fetchable_remote("https://github.com/a/b.git").as_deref(), Some("https://github.com/a/b.git"));
        assert_eq!(
            fetchable_remote("https://user:ghp_secret@github.com/a/b.git").as_deref(),
            Some("https://github.com/a/b.git"),
            "a token in the URL never reaches a node"
        );
        assert_eq!(fetchable_remote("git@github.com:Alakazam-211/K2.git").as_deref(), Some("https://github.com/Alakazam-211/K2.git"));
        assert_eq!(fetchable_remote("ssh://git@github.com:22/a/b.git").as_deref(), Some("https://github.com/a/b.git"));
        assert_eq!(fetchable_remote("/local/path"), None);
        assert_eq!(fetchable_remote("file:///x"), None);
        assert_eq!(fetchable_remote("http://github.com/a/b"), None, "plain http is not offered");
    }

    #[test]
    fn project_key_is_stable_and_distinct() {
        assert_eq!(project_key("w", Some("r")), project_key("w", Some("r")));
        assert_ne!(project_key("w", Some("r")), project_key("w2", Some("r")));
        assert_eq!(project_key("w", None).len(), 16);
    }

    #[test]
    fn bundle_carries_have_to_commit_and_never_writes_into_the_repo() {
        let d = temp("bundle");
        let r = repo(&d);
        let base = g(&r, &["rev-parse", "HEAD"]);
        std::fs::write(r.join("a.txt"), "two\n").unwrap();
        g(&r, &["commit", "-q", "-am", "two"]);
        let tip = g(&r, &["rev-parse", "HEAD"]);
        let refs_before = g(&r, &["for-each-ref"]);
        assert_eq!(repo_root(&r).unwrap().canonicalize().unwrap(), r.canonicalize().unwrap());
        assert_eq!(resolve_commit(&r, "HEAD").unwrap(), tip);
        assert!(resolve_commit(&r, "--upload-pack=x").is_err());
        let out = d.join("job.bundle");
        let n = create_bundle(&r, &tip, &[base.clone(), "f".repeat(40)], "job-1", &out).unwrap();
        assert!(n > 0);
        assert_eq!(g(&r, &["for-each-ref"]), refs_before, "no ref written into the workspace repo");
        // A mirror that has `base` fetches the bundle and gets `tip`.
        let mirror = d.join("mirror.git");
        g(&d, &["init", "-q", "--bare", mirror.to_str().unwrap()]);
        g(&mirror, &["fetch", "-q", r.to_str().unwrap(), &format!("{base}:refs/heads/base")]);
        g(&mirror, &["fetch", "-q", out.to_str().unwrap(), "+refs/k2-compute/*:refs/k2-compute/*"]);
        assert_eq!(g(&mirror, &["rev-parse", "refs/k2-compute/job-1"]), tip);
        // Without the prerequisite the bundle can't be fetched (it is thin).
        let empty = d.join("empty.git");
        g(&d, &["init", "-q", "--bare", empty.to_str().unwrap()]);
        let fail = Command::new("git")
            .arg("-C")
            .arg(&empty)
            .args(["fetch", "-q", out.to_str().unwrap(), "+refs/k2-compute/*:refs/k2-compute/*"])
            .output()
            .unwrap();
        assert!(!fail.status.success(), "a have..commit bundle needs the have");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn dirty_blob_has_patch_untracked_and_binary_and_respects_cap() {
        let d = temp("dirty");
        let r = repo(&d);
        std::fs::write(r.join("a.txt"), "changed\n").unwrap();
        std::fs::create_dir_all(r.join("new/dir")).unwrap();
        std::fs::write(r.join("new/dir/u.txt"), "untracked\n").unwrap();
        std::fs::write(r.join("bin.dat"), [0u8, 159, 146, 150, 255]).unwrap();
        std::fs::write(r.join(".gitignore"), "ignored.log\n").unwrap();
        std::fs::write(r.join("ignored.log"), "nope").unwrap();
        assert!(is_dirty(&r).unwrap());
        let out = d.join("dirty.tar");
        let (sha, bytes) = create_dirty_blob(&r, &out, 1 << 20).unwrap();
        assert_eq!(sha.len(), 64);
        assert!(bytes > 0);
        let x = d.join("x");
        std::fs::create_dir_all(&x).unwrap();
        let ok = Command::new("tar").arg("-xf").arg(&out).arg("-C").arg(&x).status().unwrap();
        assert!(ok.success());
        let patch = std::fs::read_to_string(x.join("k2-dirty/patch.diff")).unwrap();
        assert!(patch.contains("+changed"), "{patch}");
        assert_eq!(std::fs::read_to_string(x.join("k2-dirty/untracked/new/dir/u.txt")).unwrap(), "untracked\n");
        assert_eq!(std::fs::read(x.join("k2-dirty/untracked/bin.dat")).unwrap(), vec![0u8, 159, 146, 150, 255]);
        assert!(!x.join("k2-dirty/untracked/ignored.log").exists(), "ignored files stay home");
        assert!(create_dirty_blob(&r, &d.join("small.tar"), 4).is_err(), "cap enforced");
        let _ = std::fs::remove_dir_all(&d);
    }
}
