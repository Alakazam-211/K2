//! Spawning and stopping one job process (§9.2).
//!
//! - The env starts EMPTY; [`job_env`] sets the node's names, then the
//!   plan's `--env` pairs (checked again here).
//! - Own process group (and a cgroup leaf on Linux when delegated).
//! - Stop = SIGTERM to the group, a grace period, then SIGKILL (+
//!   `cgroup.kill`). After the main process exits the group is killed
//!   anyway, so a background grandchild can't outlive the job.
//! - macOS has no hard memory cap: an RSS sampler sums the group's RSS
//!   once a second and the job is killed over the cap.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use k2_node_proto::env::check_env_pairs;

use crate::cgroup::Leaf;

/// Inputs to the job env.
pub struct EnvInputs<'a> {
    pub node_home: &'a Path,
    pub job_home: &'a Path,
    pub job_tmp: &'a Path,
    pub target_dir: &'a Path,
    /// Warm caches (`sccache/`, `cargo-home/` inside): the project's
    /// `cache/<project_key>/` or, with no code, the job's own dir. Never
    /// node-wide and never named by anything the submitter picks (V28/V29):
    /// the project key is the controller's hash of its own workspace id.
    pub cache_dir: &'a Path,
    pub job_id: &'a str,
    pub user: &'a str,
    pub extra: &'a BTreeMap<String, String>,
    /// Toolchain bin dirs resolved for this job (first on PATH).
    pub tool_paths: &'a [PathBuf],
    /// Toolchain env resolved for this job (`RUSTUP_TOOLCHAIN`, `MISE_*`).
    pub tool_env: &'a [(String, String)],
    /// The job's server's sccache socket (`SCCACHE_SERVER_UDS`, V63): its
    /// slot's `tmp/sccache.sock`, so a build never attaches to another
    /// server's sccache on `127.0.0.1:4226`.
    pub sccache_socket: Option<&'a Path>,
    /// The job's own code checkout (`jobs/<id>/src`), when it has one: git
    /// may read it although the job user doesn't own it (see
    /// [`git_safe_directory`]).
    pub src_dir: Option<&'a Path>,
    /// The job's `--cache NAME` folder (`K2_JOB_CACHE`, protocol 6).
    pub job_cache: Option<&'a Path>,
}

/// Names only the node sets, beyond the proto's list: the sccache server
/// address (V63; the proto reserves them, so the server refuses them at
/// submit too).
pub use k2_node_proto::env::SCCACHE_SERVER_VARS as NODE_SET_SCCACHE;

/// The job's whole environment. `Err((code, name))` when an extra pair is
/// refused.
pub fn job_env(i: &EnvInputs<'_>) -> Result<Vec<(String, String)>, (String, String)> {
    check_env_pairs(i.extra.iter().map(|(k, v)| (k.as_str(), v.as_str())))?;
    if let Some(k) = i.extra.keys().find(|k| NODE_SET_SCCACHE.contains(&k.as_str())) {
        return Err(("env_node_reserved".to_string(), k.clone()));
    }
    let lang = if cfg!(target_os = "macos") { "en_US.UTF-8" } else { "C.UTF-8" };
    let tc = i.node_home.join("toolchains");
    let mut env: Vec<(String, String)> = vec![
        ("HOME".into(), i.job_home.display().to_string()),
        ("PATH".into(), tool_path(i.tool_paths, &crate::util::base_path(i.node_home))),
        // The registry and git checkouts cargo downloads: per project, so
        // one tenant can't plant a crate source another builds from. The
        // rustup proxies stay in toolchains/cargo/bin (on PATH, read-only).
        ("CARGO_HOME".into(), cache_paths(i.cache_dir).1.display().to_string()),
        ("RUSTUP_HOME".into(), tc.join("rustup").display().to_string()),
        ("CARGO_TARGET_DIR".into(), i.target_dir.display().to_string()),
        ("SCCACHE_DIR".into(), cache_paths(i.cache_dir).0.display().to_string()),
        ("TMPDIR".into(), i.job_tmp.display().to_string()),
        ("LANG".into(), lang.into()),
        ("CI".into(), "1".into()),
        ("K2_COMPUTE_JOB".into(), i.job_id.into()),
        ("USER".into(), i.user.into()),
        ("LOGNAME".into(), i.user.into()),
        ("SHELL".into(), "/bin/sh".into()),
    ];
    if let Some(sock) = i.sccache_socket {
        env.push(("SCCACHE_SERVER_UDS".into(), sock.display().to_string()));
    }
    if let Some(c) = i.job_cache {
        env.push((k2_node_proto::cache::ENV_JOB_CACHE.into(), c.display().to_string()));
    }
    for (k, v) in i.tool_env {
        env.retain(|(ek, _)| ek != k);
        env.push((k.clone(), v.clone()));
    }
    for (k, v) in i.extra {
        env.push((k.clone(), v.clone()));
    }
    if let Some(src) = i.src_dir {
        git_safe_directory(&mut env, src);
    }
    Ok(env)
}

/// Let git work in the job's own checkout. The checkout belongs to the
/// node (its mirror's worktree), the job runs as the job user, so git
/// refuses it as "dubious ownership" (k2n4, 2026-10-27). Trusted for this
/// job only, through git's own `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_n` /
/// `GIT_CONFIG_VALUE_n`, never a global config: appended after any entries
/// the job already sets (its `--env`, a toolchain), never over one. Both the
/// path as given and its real path are listed (git compares the real one;
/// macOS `/var` is `/private/var`). A `GIT_CONFIG_COUNT` that isn't a number
/// is the submitter's own (git refuses it anyway): left as it is.
pub fn git_safe_directory(env: &mut Vec<(String, String)>, src: &Path) {
    let get = |env: &[(String, String)], k: &str| env.iter().rev().find(|(ek, _)| ek == k).map(|(_, v)| v.clone());
    let count = match get(env, "GIT_CONFIG_COUNT") {
        None => 0usize,
        Some(c) => match c.trim().parse::<usize>() {
            Ok(n) => n,
            Err(_) => return,
        },
    };
    // Never reuse an index the job already names, inside its count or not.
    let used = |n: usize, env: &[(String, String)]| get(env, &format!("GIT_CONFIG_KEY_{n}")).is_some() || get(env, &format!("GIT_CONFIG_VALUE_{n}")).is_some();
    let mut paths = vec![src.display().to_string()];
    if let Ok(real) = std::fs::canonicalize(src) {
        let real = real.display().to_string();
        if !paths.contains(&real) {
            paths.push(real);
        }
    }
    let mut n = count;
    for p in paths {
        while used(n, env) {
            n += 1;
        }
        env.push((format!("GIT_CONFIG_KEY_{n}"), "safe.directory".into()));
        env.push((format!("GIT_CONFIG_VALUE_{n}"), p));
        n += 1;
    }
    env.retain(|(k, _)| k != "GIT_CONFIG_COUNT");
    env.push(("GIT_CONFIG_COUNT".into(), n.to_string()));
}

/// `(SCCACHE_DIR, CARGO_HOME)` inside a cache dir.
pub fn cache_paths(cache_dir: &Path) -> (PathBuf, PathBuf) {
    (cache_dir.join("sccache"), cache_dir.join("cargo-home"))
}

fn tool_path(first: &[PathBuf], base: &str) -> String {
    let mut parts: Vec<String> = first.iter().map(|p| p.display().to_string()).collect();
    parts.push(base.to_string());
    k2_node_proto::env::clean_path(&parts.join(":"))
}

/// Group RSS in bytes (macOS soft cap).
pub trait RssSampler: Send + Sync {
    fn group_rss(&self, pgid: i32) -> Option<u64>;

    /// The group's processes with their RSS and CPU (CE-14, the CE-10
    /// stall rule). `None`: this sampler can't say; the job's memory then
    /// comes from [`Self::group_rss`] and its CPU is unknown.
    fn group_procs(&self, _pgid: i32) -> Option<Vec<crate::jobstat::ProcUse>> {
        None
    }
}

/// `ps -axo pgid=,rss=` summed for one group.
pub struct PsSampler;

impl RssSampler for PsSampler {
    fn group_rss(&self, pgid: i32) -> Option<u64> {
        let out = std::process::Command::new("/bin/ps").args(["-axo", "pgid=,rss="]).env_clear().output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let mut kb = 0u64;
        for l in s.lines() {
            let mut it = l.split_whitespace();
            if let (Some(g), Some(r)) = (it.next(), it.next()) {
                if g.parse::<i32>().ok() == Some(pgid) {
                    kb += r.parse::<u64>().unwrap_or(0);
                }
            }
        }
        Some(kb * 1024)
    }

    fn group_procs(&self, pgid: i32) -> Option<Vec<crate::jobstat::ProcUse>> {
        crate::jobstat::ps_group(pgid)
    }
}

pub struct Spawned {
    pub child: tokio::process::Child,
    pub pgid: i32,
}

/// Start the job's command.
pub fn spawn(argv: &[String], env: &[(String, String)], cwd: &Path, leaf: Option<&Leaf>) -> Result<Spawned, String> {
    let program = argv.first().ok_or("empty argv")?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(&argv[1..])
        .env_clear()
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(false);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let procs = leaf.map(|l| l.procs_path.clone());
    unsafe {
        cmd.pre_exec(move || {
            if let Some(p) = procs.as_ref() {
                crate::cgroup::join_in_child(p)?;
            }
            raise_nofile();
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|e| format!("spawn {program}: {e}"))?;
    let pgid = child.id().ok_or("child has no pid")? as i32;
    Ok(Spawned { child, pgid })
}

/// Raise the open-files soft limit (builds open many files).
fn raise_nofile() {
    unsafe {
        let mut rl: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl) == 0 {
            let want: libc::rlim_t = 10240;
            let target = if rl.rlim_max < want { rl.rlim_max } else { want };
            if rl.rlim_cur < target {
                rl.rlim_cur = target;
                let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &rl);
            }
        }
    }
}

pub fn signal_group(pgid: i32, sig: i32) {
    if pgid > 1 {
        unsafe {
            libc::kill(-pgid, sig);
        }
    }
}

/// SIGTERM the group, wait `grace` for the main process, then SIGKILL.
pub async fn stop_group(child: &mut tokio::process::Child, pgid: i32, leaf: Option<&Leaf>, grace: Duration) {
    signal_group(pgid, libc::SIGTERM);
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        signal_group(pgid, libc::SIGKILL);
        if let Some(l) = leaf {
            l.kill();
        }
        let _ = child.wait().await;
    }
}

/// After the main process ended: nothing in the group may live on.
pub fn reap_group(pgid: i32, leaf: Option<&Leaf>) {
    signal_group(pgid, libc::SIGKILL);
    if let Some(l) = leaf {
        l.kill();
    }
}

/// Default sampler handle.
pub fn default_sampler() -> Arc<dyn RssSampler> {
    Arc::new(PsSampler)
}

/// The job root's folders bound over the job's /tmp and /var/tmp under
/// the Linux wall (its `TMPDIR` is the first).
pub const JOB_TMP: &str = "tmp";
pub const JOB_VAR_TMP: &str = "var-tmp";

/// Job dir parts.
pub struct JobDirs {
    pub root: PathBuf,
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub var_tmp: PathBuf,
    pub src: PathBuf,
    pub dirty: PathBuf,
    pub log: PathBuf,
    pub private_target: PathBuf,
}

impl JobDirs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            home: root.join("home"),
            tmp: root.join(JOB_TMP),
            var_tmp: root.join(JOB_VAR_TMP),
            src: root.join("src"),
            dirty: root.join("dirty"),
            log: root.join("log"),
            private_target: root.join("target"),
            root,
        }
    }

    pub fn create(&self) -> Result<(), String> {
        self.create_for(false)
    }

    /// `shared`: the Linux job wall, where the job runs as `k2job` with the
    /// node's group. The root is 01770: the job may create files in its
    /// cwd, but the sticky bit stops it renaming or replacing the node's
    /// own entries there (its `log` journal, `src/`); the dirs it writes
    /// are group-rwx (0770; still closed to everyone else).
    pub fn create_for(&self, shared: bool) -> Result<(), String> {
        crate::util::mkdir_mode(&self.root, if shared { 0o1770 } else { 0o700 })?;
        for d in [&self.home, &self.tmp, &self.var_tmp] {
            if shared {
                crate::util::mkdir_mode(d, 0o770)?;
            } else {
                std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
            }
        }
        Ok(())
    }

    /// A job without code keeps its caches here (deleted with the job).
    pub fn private_cache(&self) -> PathBuf {
        self.root.join("cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn inputs<'a>(d: &'a Path, extra: &'a BTreeMap<String, String>) -> EnvInputs<'a> {
        EnvInputs { node_home: d, job_home: d, job_tmp: d, target_dir: d, cache_dir: d, job_id: "j1", user: "k2node", extra, tool_paths: &[], tool_env: &[], sccache_socket: None, src_dir: None, job_cache: None }
    }

    /// V28/V29: warm caches come from the cache dir the node picked (the
    /// project key's), never a node-wide dir, and `--env` can't steer them.
    #[test]
    fn caches_follow_the_cache_dir_and_extras_cant_move_them() {
        let home = PathBuf::from("/n");
        let cache = PathBuf::from("/n/cache/0123456789abcdef");
        let extra = BTreeMap::from([("SCCACHE_DIR".to_string(), "/tmp/evil".to_string())]);
        let mut i = inputs(&home, &extra);
        i.cache_dir = &cache;
        assert_eq!(job_env(&i).unwrap_err().0, "env_node_reserved", "SCCACHE_DIR is the node's");
        let none = BTreeMap::new();
        let mut i = inputs(&home, &none);
        i.cache_dir = &cache;
        let env = job_env(&i).unwrap();
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get("SCCACHE_DIR"), "/n/cache/0123456789abcdef/sccache");
        assert_eq!(get("CARGO_HOME"), "/n/cache/0123456789abcdef/cargo-home");
        assert_eq!(get("RUSTUP_HOME"), "/n/toolchains/rustup");
        assert!(!env.iter().any(|(_, v)| v == "/n/cache/sccache"), "no node-wide sccache any more");
    }

    /// `--cache NAME`: the job finds its folder in `K2_JOB_CACHE`; without
    /// one the name is absent, and no `--env` can set or move it.
    #[test]
    fn the_job_cache_is_the_nodes_to_name() {
        let home = PathBuf::from("/n");
        let none = BTreeMap::new();
        let env = job_env(&inputs(&home, &none)).unwrap();
        assert!(!env.iter().any(|(k, _)| k == "K2_JOB_CACHE"));
        let c = PathBuf::from("/n/cache/job-caches/0123456789abcdef/fuzz");
        let mut i = inputs(&home, &none);
        i.job_cache = Some(&c);
        let env = job_env(&i).unwrap();
        assert!(env.contains(&("K2_JOB_CACHE".to_string(), "/n/cache/job-caches/0123456789abcdef/fuzz".to_string())));
        let evil = BTreeMap::from([("K2_JOB_CACHE".to_string(), "/etc".to_string())]);
        let mut i = inputs(&home, &evil);
        i.job_cache = Some(&c);
        assert_eq!(job_env(&i).unwrap_err().0, "env_k2_reserved");
    }

    /// V63: the job's sccache server is its server's own socket, and no
    /// `--env` moves it (to another server's socket, or to TCP).
    #[test]
    fn sccache_server_is_the_slots_socket_and_extras_cant_move_it() {
        let home = PathBuf::from("/n");
        let sock = PathBuf::from("/n-t/2/tmp/sccache.sock");
        let none = BTreeMap::new();
        let mut i = inputs(&home, &none);
        i.sccache_socket = Some(&sock);
        let env = job_env(&i).unwrap();
        assert!(env.contains(&("SCCACHE_SERVER_UDS".to_string(), "/n-t/2/tmp/sccache.sock".to_string())));
        for k in NODE_SET_SCCACHE {
            let extra = BTreeMap::from([(k.to_string(), "/n-t/1/tmp/sccache.sock".to_string())]);
            let mut i = inputs(&home, &extra);
            i.sccache_socket = Some(&sock);
            assert_eq!(job_env(&i).unwrap_err(), ("env_node_reserved".to_string(), k.to_string()));
        }
    }

    #[test]
    fn env_has_no_daemon_reach_and_refuses_bad_extras() {
        let d = PathBuf::from("/n");
        let extra = BTreeMap::from([("RUST_LOG".to_string(), "info".to_string())]);
        let env = job_env(&inputs(&d, &extra)).unwrap();
        for v in k2_node_proto::env::PROD_REACH_VARS {
            assert!(!env.iter().any(|(k, _)| k == v), "{v}");
        }
        let k2: Vec<_> = env.iter().filter(|(k, _)| k.starts_with("K2")).map(|(k, _)| k.as_str()).collect();
        assert_eq!(k2, vec!["K2_COMPUTE_JOB"]);
        assert!(env.contains(&("RUST_LOG".into(), "info".into())));
        let bad = BTreeMap::from([("K2_HOOK_TOKEN".to_string(), "x".to_string())]);
        assert_eq!(job_env(&inputs(&d, &bad)).unwrap_err().0, "env_reaches_daemon");
    }

    #[tokio::test]
    async fn child_env_is_exactly_the_job_env() {
        let d = crate::util::temp_dir("runner-env");
        let extra = BTreeMap::new();
        let env = job_env(&inputs(&d, &extra)).unwrap();
        let argv = vec!["/usr/bin/env".to_string()];
        let mut s = spawn(&argv, &env, &d, None).unwrap();
        let mut out = String::new();
        s.child.stdout.take().unwrap().read_to_string(&mut out).await.unwrap();
        assert!(s.child.wait().await.unwrap().success());
        let mut seen: Vec<String> = out.lines().filter_map(|l| l.split_once('=').map(|(k, _)| k.to_string())).collect();
        seen.sort();
        let mut want: Vec<String> = env.iter().map(|(k, _)| k.clone()).collect();
        want.sort();
        assert_eq!(seen, want, "nothing inherited from the node's own env");
    }

    #[tokio::test]
    async fn stop_group_kills_grandchildren() {
        let d = crate::util::temp_dir("runner-stop");
        let pidfile = d.join("gc.pid");
        let script = format!("sleep 1000 & echo $! > {}; trap '' TERM; wait", pidfile.display());
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script];
        let env = vec![("PATH".to_string(), "/usr/bin:/bin".to_string())];
        let mut s = spawn(&argv, &env, &d, None).unwrap();
        for _ in 0..100 {
            if pidfile.exists() && !std::fs::read_to_string(&pidfile).unwrap().trim().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let gc: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        stop_group(&mut s.child, s.pgid, None, Duration::from_millis(300)).await;
        reap_group(s.pgid, None);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let alive = unsafe { libc::kill(gc, 0) } == 0;
        assert!(!alive, "grandchild {gc} survived the job");
    }

    /// The job's own checkout is a git safe directory in the job's env
    /// only, appended after any `GIT_CONFIG_*` the job set, never over one.
    #[test]
    fn the_jobs_checkout_is_a_git_safe_directory_appended_never_over_the_jobs_own() {
        let d = crate::util::temp_dir("gitsafe");
        let src = d.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let real = std::fs::canonicalize(&src).unwrap().display().to_string();
        let given = src.display().to_string();
        let val = |env: &[(String, String)], k: &str| env.iter().rev().find(|(ek, _)| ek == k).map(|(_, v)| v.clone());
        let mut paths = vec![given.clone()];
        if real != given {
            paths.push(real.clone());
        }
        // Nothing set by the job: entries from 0.
        let mut env = vec![("HOME".to_string(), "/h".to_string())];
        git_safe_directory(&mut env, &src);
        assert_eq!(val(&env, "GIT_CONFIG_COUNT"), Some(paths.len().to_string()));
        for (n, p) in paths.iter().enumerate() {
            assert_eq!(val(&env, &format!("GIT_CONFIG_KEY_{n}")).as_deref(), Some("safe.directory"));
            assert_eq!(val(&env, &format!("GIT_CONFIG_VALUE_{n}")).as_ref(), Some(p));
        }
        assert_eq!(env.iter().filter(|(k, _)| k == "GIT_CONFIG_COUNT").count(), 1);
        // The job set two of its own (and a stray at 3): kept, ours after them.
        let mut env: Vec<(String, String)> = [
            ("GIT_CONFIG_COUNT", "2"),
            ("GIT_CONFIG_KEY_0", "user.name"),
            ("GIT_CONFIG_VALUE_0", "ci"),
            ("GIT_CONFIG_KEY_1", "core.autocrlf"),
            ("GIT_CONFIG_VALUE_1", "false"),
            ("GIT_CONFIG_KEY_3", "user.email"),
            ("GIT_CONFIG_VALUE_3", "ci@x"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        git_safe_directory(&mut env, &src);
        assert_eq!((val(&env, "GIT_CONFIG_KEY_0").as_deref(), val(&env, "GIT_CONFIG_VALUE_0").as_deref()), (Some("user.name"), Some("ci")));
        assert_eq!((val(&env, "GIT_CONFIG_KEY_1").as_deref(), val(&env, "GIT_CONFIG_VALUE_1").as_deref()), (Some("core.autocrlf"), Some("false")));
        assert_eq!((val(&env, "GIT_CONFIG_KEY_3").as_deref(), val(&env, "GIT_CONFIG_VALUE_3").as_deref()), (Some("user.email"), Some("ci@x")), "never over the job's own");
        assert_eq!(val(&env, "GIT_CONFIG_KEY_2").as_deref(), Some("safe.directory"));
        assert_eq!(val(&env, "GIT_CONFIG_VALUE_2").as_ref(), Some(&paths[0]));
        // A second path (macOS real path) skips the job's 3 and lands on 4.
        let count = if paths.len() == 2 { 5 } else { 3 };
        if paths.len() == 2 {
            assert_eq!(val(&env, "GIT_CONFIG_KEY_4").as_deref(), Some("safe.directory"));
            assert_eq!(val(&env, "GIT_CONFIG_VALUE_4").as_ref(), Some(&paths[1]));
        }
        assert_eq!(val(&env, "GIT_CONFIG_COUNT"), Some(count.to_string()));
        // A count that isn't a number is the job's own business: untouched.
        let mut env = vec![("GIT_CONFIG_COUNT".to_string(), "x".to_string())];
        git_safe_directory(&mut env, &src);
        assert_eq!(env, vec![("GIT_CONFIG_COUNT".to_string(), "x".to_string())]);
    }
}
