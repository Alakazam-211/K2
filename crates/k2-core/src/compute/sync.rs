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

/// Hardened (no hooks, no fsmonitor, no ext diff: [`crate::git_safe`]),
/// with `-C` so a missing folder is git's own error message.
fn git(dir: &Path) -> Command {
    let mut c = crate::git_safe::command();
    c.arg("-C").arg(dir);
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

/// The checkout a job takes HEAD (and the `--dirty` diff) from when the
/// caller ran from `caller_dir` (its repo top level): a git worktree of the
/// workspace's own repo that lives inside the workspace folder.
///
/// `Ok(None)` = use the workspace's own checkout (the caller is in it, or
/// `caller_dir` doesn't exist on this machine). `Err` = the caller is in
/// some other repo or outside the workspace; the caller warns and falls
/// back. Never reads a repo outside `ws_dir`.
pub fn caller_checkout(ws_dir: &Path, ws_root: &Path, caller_dir: &Path) -> Result<Option<PathBuf>, String> {
    if !caller_dir.is_absolute() {
        return Err("your repo path isn't absolute".into());
    }
    let Ok(caller) = dunce::canonicalize(&caller_dir) else { return Ok(None) };
    let ws = dunce::canonicalize(&ws_dir).map_err(|e| format!("the workspace folder: {e}"))?;
    if !caller.starts_with(&ws) {
        return Err(format!("{} is outside the workspace", caller_dir.display()));
    }
    let root = dunce::canonicalize(&repo_root(&caller)?).map_err(|e| format!("your repo: {e}"))?;
    let ws_root = dunce::canonicalize(&ws_root).map_err(|e| format!("the workspace repo: {e}"))?;
    if root == ws_root {
        return Ok(None);
    }
    if !root.starts_with(&ws) {
        return Err(format!("{} is outside the workspace", root.display()));
    }
    let mine = dunce::canonicalize(&common_dir(&root)?).map_err(|e| format!("your git dir: {e}"))?;
    let theirs = dunce::canonicalize(&common_dir(&ws_root)?).map_err(|e| format!("the workspace git dir: {e}"))?;
    if mine != theirs {
        return Err(format!("{} is a different repo, not a worktree of the workspace's", root.display()));
    }
    Ok(Some(root))
}

/// `--repo <subdir>` (0.46.6): a git repo nested in the workspace folder
/// that a job ships instead of the workspace's own (a multi-repo
/// workspace whose root isn't a repo, e.g. `k2-cores/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedRepo {
    /// The checkout `--repo` named (canonical): HEAD and `--dirty` come
    /// from it (a worktree's are its own).
    pub root: PathBuf,
    /// The repo's main checkout when it is inside the workspace (canonical;
    /// else `root`): bundles are built from it, and its path names the
    /// repo for the warm-slot key, so a worktree and its main checkout
    /// share warm slots.
    pub home: PathBuf,
    /// `home` relative to the workspace folder, `/`-joined. Empty = the
    /// workspace's own repo (`--repo` named one of its worktrees).
    pub rel: String,
}

/// The one folder name a repo path may never go through: the workspace's
/// private K2 state (notes, wiki, PRDs). A job never ships it.
const K2_DIR: &str = ".k2";

/// Check a `--repo` value and find the repo (the jail, as for a caller's
/// worktree): relative to the workspace folder, plain names only (no
/// `..`, no `.`, no `.k2`), resolving inside the workspace with every
/// link followed (no link out), the top level of a git repo or of a
/// worktree, whose git dir is inside the workspace too (a worktree of a
/// repo elsewhere on this machine is refused). Never reads a repo outside
/// `ws_dir`.
pub fn nested_repo(ws_dir: &Path, rel: &str) -> Result<NestedRepo, String> {
    let trimmed = rel.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.len() > 1024 || trimmed.contains('\0') || trimmed.starts_with('/') {
        return Err(format!("--repo {rel:?}: a folder relative to the workspace, like k2-multiplayer-base"));
    }
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("--repo {rel:?}: plain folder names only (no '..', '.' or '//')"));
        }
        if part == K2_DIR {
            return Err(format!("--repo {rel:?}: never inside .k2/ (the workspace's private K2 folder)"));
        }
    }
    let ws = ws_dir.canonicalize().map_err(|e| format!("the workspace folder: {e}"))?;
    let named = ws.join(trimmed);
    let canon = named.canonicalize().map_err(|e| format!("--repo {trimmed}: {e}"))?;
    let inside = |p: &Path| -> Option<PathBuf> { p.strip_prefix(&ws).ok().map(Path::to_path_buf) };
    let through_k2 = |rel: &Path| rel.components().any(|c| c.as_os_str() == K2_DIR);
    match inside(&canon) {
        Some(r) if !r.as_os_str().is_empty() && !through_k2(&r) => {}
        Some(r) if r.as_os_str().is_empty() => return Err(format!("--repo {trimmed} is the workspace folder itself; drop --repo")),
        _ => return Err(format!("--repo {trimmed} leads outside the workspace (or into .k2/) through a link")),
    }
    if !canon.is_dir() {
        return Err(format!("--repo {trimmed} isn't a folder"));
    }
    let top = repo_root(&canon).map_err(|_| format!("--repo {trimmed} isn't a git repo"))?;
    let top = top.canonicalize().map_err(|e| format!("--repo {trimmed}: {e}"))?;
    if top != canon {
        return Err(format!("--repo {trimmed} is a folder inside the git repo at {}; name the repo's top folder", top.display()));
    }
    let common = common_dir(&canon)?.canonicalize().map_err(|e| format!("--repo {trimmed}: its git dir: {e}"))?;
    match inside(&common) {
        Some(r) if !through_k2(&r) => {}
        _ => return Err(format!("--repo {trimmed} is a worktree of a repo outside the workspace ({})", common.display())),
    }
    let home = main_checkout_of(&canon)
        .and_then(|h| h.canonicalize().ok())
        .filter(|h| inside(h).is_some_and(|r| !through_k2(&r)))
        .unwrap_or_else(|| canon.clone());
    let rel_home = inside(&home).unwrap_or_default();
    let rel = rel_home.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
    Ok(NestedRepo { root: canon, home, rel })
}

/// The nested repo a caller's checkout (`caller_dir`, its top level)
/// belongs to, when the workspace folder itself isn't a repo: `Ok(None)`
/// when the caller isn't in a repo inside the workspace (or the folder
/// isn't on this machine). The same jail as [`nested_repo`].
pub fn nested_repo_of_caller(ws_dir: &Path, caller_dir: &Path) -> Result<Option<NestedRepo>, String> {
    if !caller_dir.is_absolute() {
        return Ok(None);
    }
    let Ok(caller) = caller_dir.canonicalize() else { return Ok(None) };
    let ws = ws_dir.canonicalize().map_err(|e| format!("the workspace folder: {e}"))?;
    if !caller.starts_with(&ws) || caller == ws {
        return Ok(None);
    }
    let Ok(top) = repo_root(&caller) else { return Ok(None) };
    let top = top.canonicalize().map_err(|e| format!("your repo: {e}"))?;
    let home = main_checkout_of(&top).and_then(|h| h.canonicalize().ok()).filter(|h| h.starts_with(&ws) && *h != ws).unwrap_or(top);
    let Ok(rel) = home.strip_prefix(&ws) else { return Ok(None) };
    let rel = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
    nested_repo(ws_dir, &rel).map(Some)
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

/// What `--dirty` sends from `root`: (tracked files changed against HEAD,
/// untracked files not ignored). The same lists [`create_dirty_blob`] reads.
pub fn dirty_counts(root: &Path) -> Result<(usize, usize), String> {
    let mut c = git(root);
    c.args(["diff", "--no-ext-diff", "--no-textconv", "--name-only", "HEAD"]);
    let changed = run(c, "list changed files")?.lines().filter(|l| !l.is_empty()).count();
    let mut c = git(root);
    c.args(["ls-files", "--others", "--exclude-standard"]);
    let untracked = run(c, "list untracked files")?.lines().filter(|l| !l.is_empty() && !in_k2_dir(l)).count();
    Ok((changed, untracked))
}

/// An untracked path under a `.k2/` folder: K2's private state (notes,
/// wiki, vault files) never rides `--dirty` to a node, ignored or not.
fn in_k2_dir(rel: &str) -> bool {
    rel.split('/').any(|p| p == K2_DIR)
}

/// The main checkout of the repo `dir` belongs to, when `dir` is (inside)
/// a git worktree or the main checkout itself: the parent of the shared
/// git dir when that is a `.git` folder. `None` outside a repo, or for a
/// bare / separate-git-dir layout.
pub fn main_checkout_of(dir: &Path) -> Option<PathBuf> {
    let common = common_dir(dir).ok()?;
    if common.file_name().is_some_and(|n| n == ".git") {
        return common.parent().map(Path::to_path_buf);
    }
    None
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

/// [`project_key`] for a job's repo: the workspace's own (`repo` `None`
/// or empty: the same key as ever), or a nested one (`--repo`), whose
/// workspace-relative path is in the hash so two nested repos of one
/// workspace (even with no remote) never share a mirror or warm slots.
pub fn project_key_in(workspace_id: &str, remote: Option<&str>, repo: Option<&str>) -> String {
    match repo.filter(|r| !r.is_empty()) {
        None => project_key(workspace_id, remote),
        Some(r) => {
            let h = crypto::sha256_hex(&crypto::signed_bytes(
                "k2-compute-project-v2",
                &[workspace_id.as_bytes(), remote.unwrap_or("").as_bytes(), r.as_bytes()],
            ));
            h[..16].to_string()
        }
    }
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
        // A throwaway bare repo K2 just made (no work tree): the resolved
        // git without the `-c` hardening (`git_safe::plain`).
        let mut c = crate::git_safe::plain();
        c.args(["init", "--bare", "-q"]).arg(&tmp);
        run(c, "make the bundle repo")?;
        std::fs::write(tmp.join("objects/info/alternates"), format!("{}\n", objects.display()))
            .map_err(|e| format!("write alternates: {e}"))?;
        let gd = |args: &[&str]| {
            // The same throwaway bare repo, by --git-dir.
            let mut c = crate::git_safe::plain();
            c.arg("--git-dir").arg(&tmp).args(args);
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
        c.args(["diff", "--no-ext-diff", "--no-textconv", "--binary", "HEAD"]);
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
            if in_k2_dir(rel) {
                continue;
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
        let status = crate::bg_proc::command("tar")
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

    /// V28/V29: the node names mirrors, warm targets and (0.46.3) sccache
    /// and cargo caches by this key. Whatever id a workspace has, the key
    /// is the server's hash: 16 hex, never a path, a node-wide name or a
    /// value the id picks.
    #[test]
    fn a_workspace_id_cant_steer_the_cache_key() {
        for id in ["../../state", "sccache", "cache", "0123456789abcdef", "t/other-tenant/ws", "", "w\0x"] {
            let k = project_key(id, None);
            assert_eq!(k.len(), 16, "{id:?}");
            assert!(k.chars().all(|c| c.is_ascii_hexdigit()), "{id:?} → {k}");
            assert_ne!(k, id, "an id that looks like a key isn't one");
        }
        // Same id, another remote: another cache.
        assert_ne!(project_key("w", Some("https://github.com/a/b")), project_key("w", Some("https://github.com/a/c")));
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
        assert_eq!(dunce::canonicalize(&repo_root(&r).unwrap()).unwrap(), dunce::canonicalize(&r).unwrap());
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
    fn caller_checkout_picks_a_worktree_inside_the_workspace_only() {
        let d = temp("wt");
        let r = repo(&d);
        let main_head = g(&r, &["rev-parse", "HEAD"]);
        // A worktree inside the workspace, one commit ahead (ignored by the
        // main checkout, as K2's own .gitignore does for .claude/).
        std::fs::create_dir_all(r.join(".git/info")).unwrap();
        std::fs::write(r.join(".git/info/exclude"), ".claude/\n").unwrap();
        let wt = r.join(".claude/worktrees/x");
        g(&r, &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()]);
        std::fs::write(wt.join("a.txt"), "worktree\n").unwrap();
        g(&wt, &["commit", "-q", "-am", "wt"]);
        let wt_head = g(&wt, &["rev-parse", "HEAD"]);
        assert_ne!(wt_head, main_head);
        let ws_root = repo_root(&r).unwrap();

        let got = caller_checkout(&r, &ws_root, &wt).unwrap().expect("the worktree");
        assert_eq!(got, dunce::canonicalize(&wt).unwrap());
        assert_eq!(resolve_commit(&got, "HEAD").unwrap(), wt_head);
        // Its own dirty state, not the main checkout's.
        assert!(!is_dirty(&r).unwrap());
        std::fs::write(wt.join("a.txt"), "dirty in the worktree\n").unwrap();
        assert!(is_dirty(&got).unwrap());

        // The workspace's own checkout (or a folder in it): no override.
        assert_eq!(caller_checkout(&r, &ws_root, &r).unwrap(), None);
        // Not on this machine: no override.
        assert_eq!(caller_checkout(&r, &ws_root, &d.join("nope")).unwrap(), None);
        // Relative: refused.
        assert!(caller_checkout(&r, &ws_root, Path::new("ws")).is_err());
        // A worktree outside the workspace folder: refused.
        let outside = d.join("outside-wt");
        g(&r, &["worktree", "add", "-q", "-b", "out", outside.to_str().unwrap()]);
        assert!(caller_checkout(&r, &ws_root, &outside).unwrap_err().contains("outside the workspace"));
        // A different repo nested inside the workspace: refused.
        let nested = r.join("vendor/other");
        std::fs::create_dir_all(&nested).unwrap();
        g(&nested, &["init", "-q"]);
        std::fs::write(nested.join("b.txt"), "b\n").unwrap();
        g(&nested, &["add", "."]);
        g(&nested, &["commit", "-q", "-m", "b"]);
        assert!(caller_checkout(&r, &ws_root, &nested).unwrap_err().contains("different repo"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn nested(at: &Path, file: &str) -> String {
        std::fs::create_dir_all(at).unwrap();
        g(at, &["init", "-q"]);
        std::fs::write(at.join(file), "x\n").unwrap();
        g(at, &["add", "."]);
        g(at, &["commit", "-q", "-m", "init"]);
        g(at, &["rev-parse", "HEAD"])
    }

    /// `--repo`: a multi-repo workspace whose root isn't a repo. The jail
    /// is the caller-worktree one: inside the workspace, links followed,
    /// never `.k2/`, the top of a repo or worktree whose git dir is inside.
    #[test]
    fn nested_repos_are_jailed_to_the_workspace() {
        let d = temp("nested");
        let ws = d.join("k2-cores");
        std::fs::create_dir_all(ws.join(".k2/notes")).unwrap();
        let base_head = nested(&ws.join("k2-multiplayer-base"), "base.rs");
        nested(&ws.join("k2-multiplayer-markdown"), "md.rs");
        assert!(repo_root(&ws).is_err(), "the workspace root isn't a repo");
        let canon = |p: &Path| p.canonicalize().unwrap();

        let n = nested_repo(&ws, "k2-multiplayer-base").unwrap();
        assert_eq!((n.root.clone(), n.home.clone(), n.rel.as_str()), (canon(&ws.join("k2-multiplayer-base")), canon(&ws.join("k2-multiplayer-base")), "k2-multiplayer-base"));
        assert_eq!(resolve_commit(&n.root, "HEAD").unwrap(), base_head);
        assert_eq!(nested_repo(&ws, "k2-multiplayer-base/").unwrap(), n, "a trailing slash is fine");

        // A worktree of the nested repo, inside the workspace: its own
        // HEAD, the main checkout's name (so they share warm slots).
        let wt = ws.join(".worktrees/base-x");
        g(&ws.join("k2-multiplayer-base"), &["worktree", "add", "-q", "-b", "x", wt.to_str().unwrap()]);
        std::fs::write(wt.join("base.rs"), "y\n").unwrap();
        g(&wt, &["commit", "-q", "-am", "x"]);
        let w = nested_repo(&ws, ".worktrees/base-x").unwrap();
        assert_eq!((w.root.clone(), w.home.clone(), w.rel.as_str()), (canon(&wt), n.home.clone(), "k2-multiplayer-base"));
        assert_ne!(resolve_commit(&w.root, "HEAD").unwrap(), base_head);
        // From inside that worktree, no --repo: the same repo is found.
        assert_eq!(nested_repo_of_caller(&ws, &wt).unwrap(), Some(n.clone()));
        assert_eq!(nested_repo_of_caller(&ws, &canon(&ws.join("k2-multiplayer-markdown"))).unwrap().unwrap().rel, "k2-multiplayer-markdown");
        assert_eq!(nested_repo_of_caller(&ws, &ws).unwrap(), None, "the root is no repo");
        assert_eq!(nested_repo_of_caller(&ws, &d).unwrap(), None, "outside the workspace");

        // Refused: escapes, odd names, .k2, a folder inside a repo, the root.
        for bad in ["", "/etc", "../k2-cores/k2-multiplayer-base", "k2-multiplayer-base/../k2-multiplayer-markdown", "./k2-multiplayer-base", "a//b", ".k2", ".k2/notes", "k2-multiplayer-base/src", "nope"] {
            assert!(nested_repo(&ws, bad).is_err(), "{bad:?}");
        }
        std::fs::create_dir_all(ws.join("k2-multiplayer-base/src")).unwrap();
        assert!(nested_repo(&ws, "k2-multiplayer-base/src").unwrap_err().contains("inside the git repo"));
        // A repo hidden under .k2/: refused even through a link.
        nested(&ws.join(".k2/notes/repo"), "n.md");
        crate::test_env::symlink(ws.join(".k2/notes/repo"), ws.join("notes-link")).unwrap();
        assert!(nested_repo(&ws, ".k2/notes/repo").is_err());
        assert!(nested_repo(&ws, "notes-link").is_err(), "a link into .k2/");
        // A link out of the workspace to a real repo: refused.
        let outside = d.join("private-repo");
        nested(&outside, "secret.txt");
        crate::test_env::symlink(&outside, ws.join("escape")).unwrap();
        assert!(nested_repo(&ws, "escape").unwrap_err().contains("outside the workspace"));
        // A link inside the workspace to a nested repo: fine, by its real name.
        crate::test_env::symlink(ws.join("k2-multiplayer-base"), ws.join("base-link")).unwrap();
        assert_eq!(nested_repo(&ws, "base-link").unwrap().rel, "k2-multiplayer-base");
        // A worktree, inside the workspace, of a repo outside it: refused.
        let foreign = ws.join("foreign-wt");
        g(&outside, &["worktree", "add", "-q", "-b", "f", foreign.to_str().unwrap()]);
        assert!(nested_repo(&ws, "foreign-wt").unwrap_err().contains("outside the workspace"));
        assert!(nested_repo_of_caller(&ws, &foreign).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_nested_repo_has_its_own_project_key() {
        assert_eq!(project_key_in("w", Some("r"), None), project_key("w", Some("r")), "the workspace's own repo keeps its key");
        assert_eq!(project_key_in("w", None, Some("")), project_key("w", None));
        let a = project_key_in("w", None, Some("k2-multiplayer-base"));
        let b = project_key_in("w", None, Some("k2-multiplayer-markdown"));
        assert_ne!(a, b, "two nested repos with no remote never share warm slots");
        assert_ne!(a, project_key("w", None));
        assert_eq!(a.len(), 16);
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
        std::fs::create_dir_all(r.join(".k2/wiki")).unwrap();
        std::fs::write(r.join(".k2/wiki/notes.md"), "private").unwrap();
        assert!(is_dirty(&r).unwrap());
        // .gitignore, bin.dat, new/dir/u.txt: never the .k2 note.
        assert_eq!(dirty_counts(&r).unwrap(), (1, 3));
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
        assert!(!x.join("k2-dirty/untracked/.k2").exists(), "K2's private folder never rides --dirty");
        assert!(create_dirty_blob(&r, &d.join("small.tar"), 4).is_err(), "cap enforced");
        let _ = std::fs::remove_dir_all(&d);
    }
}
