//! Code in (§11.1, CN29): the `git` CLI only. Per project a bare mirror
//! `repos/<project_key>.git`; history comes from the project's remote when
//! the node can reach it, and only the commits it lacks come from the
//! controller as a bundle (`refs/k2-compute/<job_id>`). `--dirty` adds a
//! tar of `k2-dirty/patch.diff` + `k2-dirty/untracked/…`. Each job gets
//! its own detached worktree.
//!
//! **Hardened git (V49).** Every git call runs with no hooks and no
//! fsmonitor (`core.hooksPath=/dev/null`, `core.fsmonitor=false`), an
//! explicit `--git-dir`, and never with its cwd inside a tree a job
//! wrote. A non-owner server's history comes only from an `https://`
//! remote whose name resolves to public addresses only, pinned to the
//! address checked (`http.curloptResolve`), with every other protocol,
//! redirects, proxies and credential helpers off ([`Git::fetch_public`]);
//! anything else is bundles only. So a server can't make the node copy
//! another server's mirror (a local path, `file://`), or dial the LAN,
//! the tailnet or this machine on its behalf.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::util::{run_cmd, CmdOut};

const FETCH_LIMIT: Duration = Duration::from_secs(600);
const LOCAL_LIMIT: Duration = Duration::from_secs(300);
/// A mirror holds at most this many packs before the node folds them into
/// one ([`Git::keep_packs_few`]). Well under the 32 below.
const MIRROR_PACKS_MAX: usize = 20;

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

/// A git token for one fetch (git access, GH#78): `http.<url>.extraHeader`
/// through `GIT_CONFIG_*` in the git child's environment. Never on the
/// command line (other users can read argv), never in the mirror's config,
/// never in a URL (so not in `FETCH_HEAD`). Git sends it only to URLs that
/// match `url` (`git_env` sets no other `GIT_CONFIG_*`).
pub fn auth_env(env: &mut Vec<(String, String)>, url: &str, cred: &k2_node_proto::gitcred::Cred) {
    env.retain(|(k, _)| !k.starts_with("GIT_CONFIG_COUNT") && !k.starts_with("GIT_CONFIG_KEY_") && !k.starts_with("GIT_CONFIG_VALUE_"));
    env.push(("GIT_CONFIG_COUNT".into(), "1".into()));
    env.push(("GIT_CONFIG_KEY_0".into(), format!("http.{url}.extraHeader")));
    env.push(("GIT_CONFIG_VALUE_0".into(), cred.basic_header()));
}

fn check(what: &str, out: CmdOut) -> Result<CmdOut, String> {
    if out.ok {
        Ok(out)
    } else {
        let msg = out.stderr.trim();
        Err(format!("{what}: {}", if msg.is_empty() { "failed" } else { msg }))
    }
}

/// Before every git call: no hooks, no fsmonitor daemon (V49).
pub const HARDEN: &[&str] = &["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"];

/// "git version 2.43.0" → (2, 43).
pub fn parse_git_version(s: &str) -> Option<(u32, u32)> {
    let v = s.split_whitespace().nth(2)?;
    let mut it = v.split('.');
    Some((it.next()?.parse().ok()?, it.next()?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?))
}

/// `https://[user@]host[:port]/…` → (host, port). Anything else is
/// refused: other schemes, local paths, `file://`, ssh, an empty host.
pub fn https_host_port(url: &str) -> Result<(String, u16), String> {
    let rest = url.strip_prefix("https://").ok_or("only https:// remotes are fetched for this server (bundles otherwise)")?;
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("the remote url has spaces or control characters".into());
    }
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
    let (host, port) = if let Some(r) = hostport.strip_prefix('[') {
        let (h, tail) = r.split_once(']').ok_or("bad IPv6 host")?;
        let port = match tail.strip_prefix(':') {
            Some(p) => p.parse::<u16>().map_err(|_| "bad port")?,
            None if tail.is_empty() => 443,
            None => return Err("bad host".into()),
        };
        (format!("[{h}]"), port)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| "bad port")?),
            None => (hostport.to_string(), 443),
        }
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.is_empty() || !bare.chars().all(|c| c.is_ascii_alphanumeric() || "-._:".contains(c)) {
        return Err(format!("{host:?} is not a host name"));
    }
    if port == 0 {
        return Err("port 0".into());
    }
    Ok((host.to_ascii_lowercase(), port))
}

/// Every address `host` resolved to must be public; returns the one to
/// pin. None at all is refused too.
pub fn check_public(host: &str, addrs: &[IpAddr]) -> Result<IpAddr, String> {
    if addrs.is_empty() {
        return Err(format!("{host} has no address"));
    }
    if let Some(a) = addrs.iter().find(|a| !is_public_ip(a)) {
        return Err(format!("{host} resolves to {a}, which isn't a public address (bundles only)"));
    }
    Ok(addrs[0])
}

/// A globally routable unicast address: not loopback, private, link-local,
/// CGNAT/tailnet (100.64/10), "this network" (0/8), multicast, broadcast,
/// documentation, benchmarking, reserved, ULA, or an IPv6 form of any of
/// those (mapped, compatible, 6to4 and NAT64 of a private v4).
pub fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_unspecified()
                || a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_multicast()
                || a.is_broadcast()
                || a.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                || o[0] >= 240)
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_public_ip(&IpAddr::V4(v4));
            }
            // ::a.b.c.d (compatible), 2002::/16 (6to4), 64:ff9b::/96 (NAT64).
            if s[..6] == [0, 0, 0, 0, 0, 0] && !a.is_loopback() && !a.is_unspecified() {
                return false;
            }
            if s[0] == 0x2002 {
                let v4 = std::net::Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8);
                return is_public_ip(&IpAddr::V4(v4));
            }
            if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
                let v4 = std::net::Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
                return is_public_ip(&IpAddr::V4(v4));
            }
            !(a.is_unspecified()
                || a.is_loopback()
                || a.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || (s[0] == 0x0100 && s[1..4] == [0, 0, 0]))
        }
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
        let mut all: Vec<&str> = HARDEN.to_vec();
        all.extend_from_slice(args);
        run_cmd(&self.program, &all, &self.env, cwd, limit).await
    }

    /// `git --version` → (major, minor).
    pub async fn version(&self) -> Option<(u32, u32)> {
        let out = run_cmd(&self.program, &["--version"], &self.env, None, LOCAL_LIMIT).await.ok().filter(|o| o.ok)?;
        parse_git_version(&out.stdout)
    }

    /// A non-owner server's fetch (V49): `https://` only, the host's every
    /// address public, the connection pinned to the checked address (no
    /// re-resolve, no rebinding), no redirect, proxy, credential helper or
    /// other protocol. `Err` = refused or failed; the caller asks the
    /// server for a bundle instead.
    pub async fn fetch_public(&self, mirror: &Path, url: &str) -> Result<String, String> {
        self.fetch_public_auth(mirror, url, None).await
    }

    /// [`Git::fetch_public`] with a git token for `url` (git access, GH#78):
    /// the same guards, plus the token as a header scoped to that URL
    /// ([`auth_env`]).
    pub async fn fetch_public_auth(&self, mirror: &Path, url: &str, auth: Option<&k2_node_proto::gitcred::Cred>) -> Result<String, String> {
        let (host, port) = https_host_port(url)?;
        let want = (host.clone(), port);
        let addrs: Vec<IpAddr> = tokio::task::spawn_blocking(move || {
            use std::net::ToSocketAddrs;
            (want.0.trim_start_matches('[').trim_end_matches(']'), want.1).to_socket_addrs().map(|a| a.map(|s| s.ip()).collect::<Vec<_>>())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("resolve {host}: {e}"))?;
        let pick = check_public(&host, &addrs)?;
        match self.version().await {
            Some(v) if v >= (2, 37) => {}
            v => return Err(format!("git {v:?} can't pin the address it connects to (needs 2.37); bundles only")),
        }
        let resolve_host = host.trim_start_matches('[').trim_end_matches(']').to_string();
        let addr = match pick {
            IpAddr::V4(a) => a.to_string(),
            IpAddr::V6(a) => format!("[{a}]"),
        };
        let pin = format!("http.curloptResolve={resolve_host}:{port}:{addr}");
        let m = mirror.display().to_string();
        let args = [
            "-c", "protocol.allow=never",
            "-c", "protocol.https.allow=always",
            "-c", "http.followRedirects=false",
            "-c", "http.proxy=",
            "-c", "credential.helper=",
            "-c", &pin,
            "--git-dir", &m,
            "fetch", "-q", "--no-tags", "--no-recurse-submodules", url, "+refs/heads/*:refs/remotes/origin/*",
        ];
        let mut env = self.env.clone();
        env.push(("GIT_PROTOCOL_FROM_USER".into(), "0".into()));
        if let Some(c) = auth {
            auth_env(&mut env, url, c);
        }
        let mut all: Vec<&str> = HARDEN.to_vec();
        all.extend_from_slice(&args);
        check("git fetch", run_cmd(&self.program, &all, &env, None, FETCH_LIMIT).await?)?;
        Ok(format!("{host}:{port} ({addr})"))
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
        self.fetch_remote_auth(mirror, url, None).await
    }

    /// [`Git::fetch_remote`] with a git token for `url` (git access,
    /// GH#78): the token rides as an HTTP header scoped to exactly `url`,
    /// in the git child's environment only ([`auth_env`]); redirects are
    /// off so it never follows one to another host.
    pub async fn fetch_remote_auth(&self, mirror: &Path, url: &str, auth: Option<&k2_node_proto::gitcred::Cred>) -> Result<(), String> {
        if url.starts_with('-') {
            return Err("remote url can't start with '-'".into());
        }
        let m = mirror.display().to_string();
        let args = ["--git-dir", &m, "fetch", "-q", "--no-tags", url, "+refs/heads/*:refs/remotes/origin/*"];
        let out = match auth {
            None => self.git(&args, None, FETCH_LIMIT).await?,
            Some(c) => {
                let mut env = self.env.clone();
                auth_env(&mut env, url, c);
                let mut all: Vec<&str> = HARDEN.to_vec();
                all.extend_from_slice(&["-c", "http.followRedirects=false"]);
                all.extend_from_slice(&args);
                run_cmd(&self.program, &all, &env, None, FETCH_LIMIT).await?
            }
        };
        check("git fetch", out)?;
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

    /// Fold a mirror's packs into one when they pile up. Mirrors run with
    /// `gc.auto 0` and every bundle or fetch adds a pack. A job's user doesn't
    /// own its mirror, so gix (`cargo package`) opens it at reduced trust,
    /// which caps its pack index table at 32: past that it fails with "The
    /// slotmap turned out to be too small". Unreachable objects stay (`-k`),
    /// and a job already reading an old pack retries on the new one. Returns
    /// `(before, after)` when it repacked.
    pub async fn keep_packs_few(&self, mirror: &Path) -> Result<Option<(usize, usize)>, String> {
        let before = pack_count(mirror);
        if before <= MIRROR_PACKS_MAX {
            return Ok(None);
        }
        let m = mirror.display().to_string();
        check("git repack", self.git(&["--git-dir", &m, "repack", "-a", "-d", "-k", "-q", "--no-write-bitmap-index"], None, FETCH_LIMIT).await?)?;
        Ok(Some((before, pack_count(mirror))))
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
///
/// How many packs a mirror holds (0 when it has none yet).
fn pack_count(mirror: &Path) -> usize {
    std::fs::read_dir(mirror.join("objects/pack"))
        .map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "idx")).count())
        .unwrap_or(0)
}

/// Never writes through a link already in `dst` (V49): the worktree is
/// the server's checkout, so `dst/foo` may be a symlink to anywhere the
/// node user can write (its key, another server's mirror). A link where
/// a folder goes is refused; a link where a file goes is replaced by the
/// file, never followed.
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
        let existing = std::fs::symlink_metadata(&to).ok();
        if ft.is_dir() {
            match existing {
                Some(m) if m.file_type().is_symlink() || !m.is_dir() => {
                    return Err(format!("untracked folder {} would go through a link or over a file", to.display()));
                }
                Some(_) => {}
                None => std::fs::create_dir(&to).map_err(|e| format!("create {}: {e}", to.display()))?,
            }
            copy_tree(&from, &to)?;
            continue;
        }
        if existing.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
            std::fs::remove_file(&to).map_err(|e| format!("remove link {}: {e}", to.display()))?;
        }
        if ft.is_symlink() {
            let target = std::fs::read_link(&from).map_err(|e| format!("readlink {}: {e}", from.display()))?;
            let _ = std::fs::remove_file(&to);
            std::os::unix::fs::symlink(target, &to).map_err(|e| format!("symlink {}: {e}", to.display()))?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", to.display()))?;
        }
        // A FIFO, socket or device from the blob is skipped (a FIFO would
        // block the copy forever).
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

    /// Every bundle adds a pack (`gc.auto 0`). Past the limit the mirror folds
    /// them into one and keeps everything: every commit, an unreachable one, a
    /// live worktree. At or under the limit nothing runs.
    #[tokio::test]
    async fn a_mirror_folds_its_packs_before_they_reach_gix_s_limit() {
        let (repo, _first, _second) = repo_with_commits("sync-packs");
        let home = crate::util::temp_dir("sync-packs-home");
        let g = git(&home);
        let mirror = home.join("m.git");
        g.ensure_mirror(&mirror).await.unwrap();
        let m = mirror.to_str().unwrap();
        // A real bundle is big enough to stay a pack; these small ones too.
        sh_git(&mirror, &["--git-dir", m, "config", "fetch.unpackLimit", "1"]);
        let bundle = home.join("b.bundle");
        let mut shas = Vec::new();
        for i in 0..=MIRROR_PACKS_MAX {
            std::fs::write(repo.join("a.txt"), format!("{i}\n")).unwrap();
            sh_git(&repo, &["commit", "-q", "-am", &format!("c{i}")]);
            let sha = sh_git(&repo, &["rev-parse", "HEAD"]);
            sh_git(&repo, &["update-ref", "refs/k2-compute/job1", &sha]);
            let _ = std::fs::remove_file(&bundle);
            sh_git(&repo, &["bundle", "create", bundle.to_str().unwrap(), "refs/k2-compute/job1"]);
            g.fetch_bundle(&mirror, &bundle).await.unwrap();
            shas.push(sha);
            if i == 0 {
                assert_eq!(g.keep_packs_few(&mirror).await.unwrap(), None, "one pack: nothing to do");
            }
        }
        assert_eq!(pack_count(&mirror), MIRROR_PACKS_MAX + 1, "one pack per bundle");

        let wt = home.join("jobs/j/src");
        g.worktree_add(&mirror, &wt, shas.last().unwrap()).await.unwrap();
        // Each bundle moved the one ref: only the last commit is at a tip.
        assert_eq!(g.tips(&mirror).await.unwrap(), vec![shas.last().unwrap().clone()]);

        assert_eq!(g.keep_packs_few(&mirror).await.unwrap(), Some((MIRROR_PACKS_MAX + 1, 1)));
        assert_eq!(pack_count(&mirror), 1);
        assert_eq!(g.keep_packs_few(&mirror).await.unwrap(), None, "folded: nothing more to do");
        for sha in &shas {
            assert!(g.has_commit(&mirror, sha).await, "{sha} is still in the mirror");
        }
        assert_eq!(sh_git(&wt, &["status", "--porcelain"]), "", "the live worktree still reads");
        assert_eq!(&sh_git(&wt, &["rev-parse", "HEAD"]), shas.last().unwrap());
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

    /// V49: only https to a public address; never a local path, file://,
    /// ssh, git://, or a private/LAN/tailnet/loopback/multicast address.
    #[test]
    fn tenant_fetch_urls_and_addresses() {
        assert_eq!(https_host_port("https://github.com/a/b.git").unwrap(), ("github.com".into(), 443));
        assert_eq!(https_host_port("https://u:p@Git.Example.com:8443/x").unwrap(), ("git.example.com".into(), 8443));
        assert_eq!(https_host_port("https://[2606:4700::1111]/x").unwrap(), ("[2606:4700::1111]".into(), 443));
        for bad in [
            "/var/lib/k2node-t/1/repos/pk.git",
            "file:///var/lib/k2node/repos/pk.git",
            "http://github.com/a/b",
            "ssh://git@github.com/a/b",
            "git@github.com:a/b.git",
            "git://github.com/a/b",
            "ext::sh -c touch% /tmp/pwned",
            "https:///x",
            "https://a b/x",
            "https://host:0/x",
            "https://host:99999/x",
            "-u/x",
        ] {
            assert!(https_host_port(bad).is_err(), "{bad}");
        }
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        for public in ["1.1.1.1", "140.82.112.3", "8.8.8.8", "2606:4700::1111", "2a01:4f9::1"] {
            assert!(is_public_ip(&ip(public)), "{public}");
        }
        for private in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.20", "169.254.169.254", "100.64.0.1", "100.127.255.254",
            "0.0.0.0", "0.1.2.3", "224.0.0.251", "239.1.1.1", "255.255.255.255", "192.0.2.1", "198.18.0.1", "240.0.0.1", "192.0.0.8",
            "::1", "::", "fc00::1", "fd7a:115c:a1e0::1", "fe80::1", "ff02::1", "2001:db8::1", "::ffff:192.168.1.1", "::ffff:127.0.0.1",
            "::10.0.0.1", "2002:c0a8:0101::1", "64:ff9b::a00:1", "100::1",
        ] {
            assert!(!is_public_ip(&ip(private)), "{private}");
        }
        assert!(check_public("h", &[ip("1.1.1.1"), ip("10.0.0.1")]).is_err(), "every address must be public (no rebinding pick)");
        assert!(check_public("h", &[]).is_err());
        assert_eq!(check_public("h", &[ip("1.1.1.1")]).unwrap(), ip("1.1.1.1"));
        assert_eq!(parse_git_version("git version 2.43.0\n"), Some((2, 43)));
        assert_eq!(parse_git_version("git version 2.39.5 (Apple Git-154)"), Some((2, 39)));
    }

    /// V49: a server's fetch of a local path or `file://` URL (another
    /// server's mirror) is refused before git runs, and the mirror stays
    /// without the commit.
    #[tokio::test]
    async fn fetch_public_refuses_local_and_lan_remotes() {
        let (repo, _first, second) = repo_with_commits("sync-public");
        let home = crate::util::temp_dir("sync-public-home");
        let g = git(&home);
        let mirror = home.join("m.git");
        g.ensure_mirror(&mirror).await.unwrap();
        let file_url = format!("file://{}", repo.display());
        for url in [repo.display().to_string(), file_url, "https://127.0.0.1/x.git".into(), "https://192.168.1.20/x.git".into(), "https://localhost/x.git".into()] {
            let e = g.fetch_public(&mirror, &url).await.unwrap_err();
            assert!(e.contains("https") || e.contains("public"), "{url}: {e}");
        }
        assert!(!g.has_commit(&mirror, &second).await, "nothing came in");
    }

    /// Git access (GH#78): the token reaches only the granted repo's URL,
    /// as basic auth, and lands nowhere in the mirror. A loopback listener
    /// stands in for the forge: it records each request's Authorization
    /// header and answers 404, so git fails after one request.
    #[tokio::test]
    async fn an_authenticated_fetch_sends_the_token_only_to_its_repo() {
        use std::io::{Read, Write};
        let token = "ghs_k2testSYNCFETCH0123456789";
        let cred = k2_node_proto::gitcred::Cred { username: "x-access-token".into(), password: token.into(), expires_at: 1_900_000_000 };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>)>>> = Default::default();
        let s2 = seen.clone();
        std::thread::spawn(move || {
            for c in listener.incoming() {
                let Ok(mut c) = c else { continue };
                let mut buf = vec![0u8; 16 * 1024];
                let n = c.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string();
                let auth = req.lines().find_map(|l| l.strip_prefix("Authorization: ").or_else(|| l.strip_prefix("authorization: ")).map(str::to_string));
                s2.lock().unwrap().push((path, auth));
                let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });
        let home = crate::util::temp_dir("sync-auth-home");
        let g = git(&home);
        let mirror = home.join("m.git");
        g.ensure_mirror(&mirror).await.unwrap();
        let granted = format!("http://127.0.0.1:{port}/acme/app.git");
        let other = format!("http://127.0.0.1:{port}/acme/other.git");
        assert!(g.fetch_remote_auth(&mirror, &granted, Some(&cred)).await.is_err(), "the stand-in answers 404");
        // The header is scoped to the granted URL: a fetch of another repo
        // with the same env shape carries nothing.
        let mut env = g.env.clone();
        auth_env(&mut env, &granted, &cred);
        let g_other = Git { program: g.program.clone(), env };
        assert!(g_other.fetch_remote(&mirror, &other).await.is_err());
        let seen = seen.lock().unwrap().clone();
        let want = "Basic eC1hY2Nlc3MtdG9rZW46Z2hzX2sydGVzdFNZTkNGRVRDSDAxMjM0NTY3ODk=";
        let to_app: Vec<_> = seen.iter().filter(|(p, _)| p.starts_with("/acme/app.git/")).collect();
        let to_other: Vec<_> = seen.iter().filter(|(p, _)| p.starts_with("/acme/other.git/")).collect();
        assert!(!to_app.is_empty() && !to_other.is_empty(), "both fetches reached the stand-in: {seen:?}");
        assert!(to_app.iter().all(|(_, a)| a.as_deref() == Some(want)), "the granted repo gets the token: {seen:?}");
        assert!(to_other.iter().all(|(_, a)| a.is_none()), "another repo gets no token: {seen:?}");
        // Nothing of the token in the mirror (config, FETCH_HEAD, logs).
        let mut hits = Vec::new();
        crate::util::walk_files(&mirror, &mut |p: &Path| {
            if std::fs::read(p).is_ok_and(|b| b.windows(token.len()).any(|w| w == token.as_bytes()) || b.windows(20).any(|w| w == &want.as_bytes()[6..26])) {
                hits.push(p.to_path_buf());
            }
        });
        assert!(hits.is_empty(), "the token was written to {hits:?}");
    }

    /// The dirty blob's untracked files never go through a link the
    /// checkout planted (a server's repo can hold `foo -> /anywhere`).
    #[test]
    fn copy_tree_never_writes_through_a_link() {
        let d = crate::util::temp_dir("copy-links");
        let (src, dst, outside) = (d.join("src"), d.join("dst"), d.join("outside"));
        for p in [src.join("dir"), dst.clone(), outside.clone()] {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(src.join("dir/f"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, dst.join("dir")).unwrap();
        assert!(copy_tree(&src, &dst).is_err(), "a folder through a link is refused");
        assert!(!outside.join("f").exists(), "nothing written outside");
        std::fs::remove_file(dst.join("dir")).unwrap();
        std::fs::remove_dir_all(src.join("dir")).unwrap();
        std::fs::write(src.join("key.pem"), "new").unwrap();
        std::fs::write(outside.join("key.pem"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.join("key.pem"), dst.join("key.pem")).unwrap();
        copy_tree(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(outside.join("key.pem")).unwrap(), "secret", "the link's target is untouched");
        assert!(!std::fs::symlink_metadata(dst.join("key.pem")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(dst.join("key.pem")).unwrap(), "new");
    }

    #[test]
    fn sha_validation() {
        assert!(valid_sha(&"a".repeat(40)));
        assert!(!valid_sha(&"A".repeat(40)));
        assert!(!valid_sha("HEAD"));
        assert!(!valid_sha(&"g".repeat(40)));
    }
}
