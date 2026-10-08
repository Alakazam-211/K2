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
    pub job_id: &'a str,
    pub user: &'a str,
    pub extra: &'a BTreeMap<String, String>,
}

/// The job's whole environment. `Err((code, name))` when an extra pair is
/// refused.
pub fn job_env(i: &EnvInputs<'_>) -> Result<Vec<(String, String)>, (String, String)> {
    check_env_pairs(i.extra.iter().map(|(k, v)| (k.as_str(), v.as_str())))?;
    let lang = if cfg!(target_os = "macos") { "en_US.UTF-8" } else { "C.UTF-8" };
    let tc = i.node_home.join("toolchains");
    let mut env: Vec<(String, String)> = vec![
        ("HOME".into(), i.job_home.display().to_string()),
        ("PATH".into(), crate::util::base_path(i.node_home)),
        ("CARGO_HOME".into(), tc.join("cargo").display().to_string()),
        ("RUSTUP_HOME".into(), tc.join("rustup").display().to_string()),
        ("CARGO_TARGET_DIR".into(), i.target_dir.display().to_string()),
        ("SCCACHE_DIR".into(), i.node_home.join("cache/sccache").display().to_string()),
        ("TMPDIR".into(), i.job_tmp.display().to_string()),
        ("LANG".into(), lang.into()),
        ("CI".into(), "1".into()),
        ("K2_COMPUTE_JOB".into(), i.job_id.into()),
        ("USER".into(), i.user.into()),
        ("LOGNAME".into(), i.user.into()),
        ("SHELL".into(), "/bin/sh".into()),
    ];
    for (k, v) in i.extra {
        env.push((k.clone(), v.clone()));
    }
    Ok(env)
}

/// Group RSS in bytes (macOS soft cap).
pub trait RssSampler: Send + Sync {
    fn group_rss(&self, pgid: i32) -> Option<u64>;
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

/// Job dir parts.
pub struct JobDirs {
    pub root: PathBuf,
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub src: PathBuf,
    pub dirty: PathBuf,
    pub log: PathBuf,
    pub private_target: PathBuf,
}

impl JobDirs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            home: root.join("home"),
            tmp: root.join("tmp"),
            src: root.join("src"),
            dirty: root.join("dirty"),
            log: root.join("log"),
            private_target: root.join("target"),
            root,
        }
    }

    pub fn create(&self) -> Result<(), String> {
        crate::util::mkdir_mode(&self.root, 0o700)?;
        for d in [&self.home, &self.tmp] {
            std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn inputs<'a>(d: &'a Path, extra: &'a BTreeMap<String, String>) -> EnvInputs<'a> {
        EnvInputs { node_home: d, job_home: d, job_tmp: d, target_dir: d, job_id: "j1", user: "k2node", extra }
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
}
