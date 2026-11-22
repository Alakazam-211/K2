//! A Mac no-lan job's egress proxy, from the node's side
//! (prd-compute-mac-no-lan-v1 §5.2–§5.6): whether this node can give one,
//! starting it on a pool port before the job, the job's proxy env, the
//! watch that turns its decisions into live job-log lines and restarts a
//! dead proxy once, and the end summary and host list.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::frames::LogStream;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::adopt::RunFacts;
use super::run::TailCmd;
use super::{Node, Tenant};
use crate::egress::fold::{self, HostList};
use crate::egress::proc::{self as ep, EgressFacts, EgressSpec};
use crate::journal::JournalWriter;
use crate::wall::Wall;

/// A running job's proxy, as the run facts keep it (adoption after an
/// update reads it back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressRun {
    pub dir: PathBuf,
    pub facts: EgressFacts,
    /// Unix seconds: the proxy's own deadline (a restart keeps it).
    pub deadline: i64,
    /// How far the job log has read its `egress.jsonl`.
    #[serde(default)]
    pub offset: u64,
    /// Restarts after a death mid-job (at most one).
    #[serde(default)]
    pub restarts: u32,
}

impl EgressRun {
    pub fn port(&self) -> u16 {
        self.facts.port
    }
}

/// The proxy's names in the job's env (§5.4), set only on this path.
pub const PROXY_VARS: &[&str] = &["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "NO_PROXY", "no_proxy", "NODE_USE_ENV_PROXY", ENV_JOB_PROXY];
/// Set only behind a Mac no-lan proxy: a script can tell it is there.
pub const ENV_JOB_PROXY: &str = "K2_JOB_PROXY";
/// Loopback never goes through the proxy.
pub const NO_PROXY: &str = "localhost,127.0.0.1,::1,[::1]";

/// The job's env with the proxy's names set (over the job's own `--env` of
/// the same names), and which of the job's own it replaced.
pub fn with_proxy_env(mut env: Vec<(String, String)>, plan_env: &std::collections::BTreeMap<String, String>, port: u16) -> (Vec<(String, String)>, Vec<String>) {
    let url = format!("http://127.0.0.1:{port}");
    let replaced: Vec<String> = PROXY_VARS.iter().filter(|n| plan_env.contains_key(**n)).map(|n| n.to_string()).collect();
    env.retain(|(k, _)| !PROXY_VARS.contains(&k.as_str()));
    for (k, v) in [
        ("HTTPS_PROXY", url.as_str()),
        ("https_proxy", url.as_str()),
        ("HTTP_PROXY", url.as_str()),
        ("http_proxy", url.as_str()),
        ("NO_PROXY", NO_PROXY),
        ("no_proxy", NO_PROXY),
        ("NODE_USE_ENV_PROXY", "1"),
        (ENV_JOB_PROXY, url.as_str()),
    ] {
        env.push((k.to_string(), v.to_string()));
    }
    (env, replaced)
}

/// The names that point a client at a proxy (either case).
pub const PROXY_URL_VARS: &[&str] = &["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"];
/// What `NO_PROXY` must name so loopback never goes through a proxy.
pub const LOOPBACK_NO_PROXY: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// 0.46.17: when the job's env names a proxy (its own `--env`, or the Mac
/// no-lan proxy's), `NO_PROXY` and `no_proxy` both name loopback, so a
/// client never sends a request for a server the job started on
/// 127.0.0.1 to the proxy (where it hangs or fails). The job's own
/// entries stay (both spellings' entries, merged, in order); `*` is left
/// as it is. Returns the env and whether it changed. Without a proxy
/// nothing changes.
pub fn with_loopback_no_proxy(mut env: Vec<(String, String)>) -> (Vec<(String, String)>, bool) {
    if !env.iter().any(|(k, v)| PROXY_URL_VARS.contains(&k.as_str()) && !v.trim().is_empty()) {
        return (env, false);
    }
    let mut entries: Vec<String> = Vec::new();
    for (_, v) in env.iter().filter(|(k, _)| k == "NO_PROXY" || k == "no_proxy") {
        for e in v.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if !entries.iter().any(|x| x.eq_ignore_ascii_case(e)) {
                entries.push(e.to_string());
            }
        }
    }
    if entries.iter().any(|e| e == "*") {
        return (env, false);
    }
    for lo in LOOPBACK_NO_PROXY {
        if !entries.iter().any(|x| x.eq_ignore_ascii_case(lo)) {
            entries.push(lo.to_string());
        }
    }
    let want = entries.join(",");
    let before: Vec<Option<String>> = ["NO_PROXY", "no_proxy"].iter().map(|n| env.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone())).collect();
    if before.iter().all(|v| v.as_deref() == Some(want.as_str())) {
        return (env, false);
    }
    env.retain(|(k, _)| k != "NO_PROXY" && k != "no_proxy");
    env.push(("NO_PROXY".into(), want.clone()));
    env.push(("no_proxy".into(), want));
    (env, true)
}

/// The job-log line when a job's own `--env` named one of the proxy's.
pub fn replaced_line(replaced: &[String]) -> Option<String> {
    (!replaced.is_empty()).then(|| format!("network: your --env {} was replaced by the node's no-lan proxy", replaced.join(", ")))
}

/// How long a proxy may take to listen.
const SPAWN_WAIT: Duration = Duration::from_secs(10);
/// How often the watch reads the proxy's log.
const WATCH_EVERY: Duration = Duration::from_secs(1);

impl Node {
    /// Can a Mac no-lan job here get its proxy? `Ok(binary)` or why not.
    /// Linux never asks (its no-lan is the kernel's address filter).
    pub(crate) fn no_lan_proxy_ready(&self) -> Result<PathBuf, String> {
        if !cfg!(target_os = "macos") {
            return Err("not a Mac".into());
        }
        if !matches!(self.wall, Wall::Seatbelt) {
            return Err("this node has no Seatbelt job wall".into());
        }
        if self.local.lock().unwrap().policy.mac_no_lan != crate::config::MAC_NO_LAN_PROXY {
            return Err("the node's owner set mac_no_lan = \"none\"".into());
        }
        self.opts.job_holder.clone().ok_or_else(|| "this k2-node isn't an installed one (no job-egress binary)".into())
    }

    /// `status.json`'s `no_lan` fact.
    pub(crate) fn no_lan_fact(&self) -> &'static str {
        if !cfg!(target_os = "macos") {
            "n/a"
        } else if self.no_lan_proxy_ready().is_ok() {
            "proxy"
        } else {
            "none"
        }
    }

    pub(super) fn egress_dir(&self, t: &Tenant, key: &str) -> PathBuf {
        t.paths.state().join("egress").join(key)
    }

    /// The proxy's own Seatbelt profile (a Seatbelt node only).
    fn egress_sandbox(&self, dir: &std::path::Path) -> Option<String> {
        matches!(self.wall, Wall::Seatbelt).then(|| ep::sandbox_profile(dir, &self.seatbelt_deny().0))
    }

    /// Start a job's proxy on a pool port (its port block's first, then
    /// the next free one; a port something else holds is skipped). The port
    /// stays reserved until [`Node::egress_release`].
    pub(super) async fn egress_start(&self, bin: PathBuf, dir: PathBuf, key: &str, block: Option<u16>, deadline: i64) -> Result<EgressRun, String> {
        let lan = crate::admin::jobs::lan_deny_extra();
        let sandbox = self.egress_sandbox(&dir);
        let mut skipped = std::collections::BTreeSet::new();
        for _ in 0..8 {
            let port = {
                let mut used = self.egress_ports.lock().unwrap();
                let avoid: std::collections::BTreeSet<u16> = used.union(&skipped).copied().collect();
                let Some(p) = ep::port_order(block, &avoid).into_iter().next() else {
                    return Err(format!("no free proxy port ({} in use)", used.len()));
                };
                used.insert(p);
                p
            };
            let spec = EgressSpec { v: ep::EGRESS_V, port, deadline, job_key: key.to_string(), lan: lan.clone(), test: None };
            let (b, d, s) = (bin.clone(), dir.clone(), sandbox.clone());
            let r = tokio::task::spawn_blocking(move || ep::spawn(&b, &d, &spec, s.as_deref(), true, SPAWN_WAIT)).await.unwrap_or_else(|e| Err(ep::SpawnError::Failed(format!("job-egress: {e}"))));
            match r {
                Ok(facts) => {
                    self.write_status();
                    return Ok(EgressRun { dir, facts, deadline, offset: 0, restarts: 0 });
                }
                Err(ep::SpawnError::PortTaken) => {
                    crate::nlog!("egress proxy for {key}: 127.0.0.1:{port} is taken; trying the next pool port");
                    self.egress_ports.lock().unwrap().remove(&port);
                    skipped.insert(port);
                }
                Err(e) => {
                    self.egress_ports.lock().unwrap().remove(&port);
                    return Err(e.to_string());
                }
            }
        }
        Err(format!("every proxy port tried was taken ({})", skipped.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")))
    }

    /// Mark an adopted job's proxy port as in use again.
    pub(super) fn egress_reserve(&self, port: u16) {
        self.egress_ports.lock().unwrap().insert(port);
    }

    /// The job is over: stop its proxy (if it still runs), free the port,
    /// remove its folder.
    pub(super) fn egress_release(&self, r: &EgressRun) {
        if ep::alive(&r.facts) {
            ep::terminate(&r.facts);
        }
        self.egress_ports.lock().unwrap().remove(&r.port());
        let _ = std::fs::remove_dir_all(&r.dir);
        self.write_status();
    }

    /// Tell the proxy its job's process group (it ends when the group
    /// does).
    pub(super) fn egress_job_started(&self, r: &EgressRun, pgid: i32) {
        let start = (pgid > 1).then(|| crate::util::process_start_time(pgid as u32)).flatten();
        if let Err(e) = ep::write_job(&r.dir, pgid, start) {
            crate::nlog!("egress proxy {}: job.json: {e}", r.dir.display());
        }
    }

    /// The job's proxy while it runs: its decisions as live lines (the
    /// first few per host), a restart if it dies mid-job (once), and at
    /// the end ([`TailCmd::Finish`]) the whole host list. On
    /// [`TailCmd::Detach`] it records how far it read and leaves the proxy
    /// running for the next k2-node.
    pub(super) fn spawn_egress_watch(
        self: &Arc<Self>,
        mut run: EgressRun,
        journal: Arc<Mutex<JournalWriter>>,
        t: Arc<Tenant>,
        facts: Option<Arc<Mutex<RunFacts>>>,
        job: String,
        gen: u32,
        mut cmd: watch::Receiver<TailCmd>,
    ) -> tokio::task::JoinHandle<Option<(HostList, EgressRun)>> {
        let node = self.clone();
        tokio::spawn(async move {
            let mut live = fold::Live::default();
            // After an adoption: the lines already shown count.
            if run.offset > 0 {
                let b = std::fs::read(run.dir.join(ep::LOG_FILE)).unwrap_or_default();
                for d in fold::parse_lines(&String::from_utf8_lossy(&b[..(run.offset as usize).min(b.len())])) {
                    let _ = live.line(&d);
                }
            }
            let mut tail = ep::Tail::new(&run.dir, run.offset);
            let mut watching = true;
            let line = |j: &Arc<Mutex<JournalWriter>>, m: &str| {
                let _ = j.lock().unwrap().append(LogStream::Sys, format!("[k2-node] {m}\n").as_bytes());
            };
            loop {
                let c = *cmd.borrow();
                let mut shown = false;
                for d in tail.read_new() {
                    if let Some(l) = live.line(&d) {
                        line(&journal, &l);
                        shown = true;
                    }
                }
                run.offset = tail.offset;
                if let Some(f) = &facts {
                    if let Some(e) = f.lock().unwrap().egress.as_mut() {
                        e.offset = run.offset;
                    }
                }
                if shown {
                    t.ship.notify_one();
                }
                match c {
                    TailCmd::Detach => return None,
                    TailCmd::Finish => break,
                    TailCmd::Run => {}
                }
                if watching && !ep::alive(&run.facts) {
                    let ended = ep::read_exit(&run.dir);
                    if ended.as_ref().is_some_and(|x| x.why == "job_ended" || x.why == "deadline") {
                        watching = false;
                    } else if run.restarts == 0 {
                        let how = ended.map(|x| x.error.unwrap_or(x.why)).unwrap_or_else(|| "it was killed".into());
                        line(&journal, &format!("network: this job's proxy stopped ({how}); restarting it on the same port"));
                        run.restarts = 1;
                        let spec = EgressSpec { v: ep::EGRESS_V, port: run.port(), deadline: run.deadline, job_key: String::new(), lan: crate::admin::jobs::lan_deny_extra(), test: None };
                        let (bin, dir, sandbox) = (node.opts.job_holder.clone(), run.dir.clone(), node.egress_sandbox(&run.dir));
                        let r = match bin {
                            Some(b) => tokio::task::spawn_blocking(move || ep::spawn(&b, &dir, &spec, sandbox.as_deref(), false, SPAWN_WAIT)).await.unwrap_or_else(|e| Err(ep::SpawnError::Failed(e.to_string()))),
                            None => Err(ep::SpawnError::Failed("no job-egress binary".into())),
                        };
                        match r {
                            Ok(f) => {
                                run.facts = f;
                                if let Some(rf) = &facts {
                                    let snapshot = {
                                        let mut g = rf.lock().unwrap();
                                        g.egress = Some(run.clone());
                                        g.clone()
                                    };
                                    node.save_run(&t, &job, gen, &snapshot);
                                }
                            }
                            Err(e) => {
                                line(&journal, &format!("network: the proxy didn't start again ({e}); this job has no network from now on"));
                                watching = false;
                            }
                        }
                        t.ship.notify_one();
                    } else {
                        line(&journal, "network: this job's proxy stopped again; it isn't restarted twice, so this job has no network from now on");
                        t.ship.notify_one();
                        watching = false;
                    }
                }
                tokio::select! {
                    _ = tokio::time::sleep(WATCH_EVERY) => {}
                    _ = cmd.changed() => {}
                }
            }
            // The job is over: the proxy goes (it closes its tunnels and
            // writes their bytes), then the last lines and the whole list.
            if ep::alive(&run.facts) {
                ep::terminate(&run.facts);
                let t0 = std::time::Instant::now();
                while ep::alive(&run.facts) && t0.elapsed() < Duration::from_secs(8) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            for d in tail.read_new() {
                if let Some(l) = live.line(&d) {
                    line(&journal, &l);
                }
            }
            let dir = run.dir.clone();
            let all = tokio::task::spawn_blocking(move || ep::read_all(&dir)).await.unwrap_or_default();
            Some((fold::fold(&all), run))
        })
    }
}

/// The end of a no-lan job's log: the summary, then the host list (no
/// prefix). The summary also goes into the result's notes.
pub fn end_lines(l: &HostList) -> (String, Vec<String>) {
    (fold::summary_line(l), fold::host_lines(l))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proxy_env_is_set_over_the_jobs_own_and_says_so() {
        let base = vec![("PATH".to_string(), "/usr/bin".to_string()), ("HTTPS_PROXY".to_string(), "http://corp:3128".to_string()), ("https_proxy".to_string(), "x".to_string())];
        let plan = std::collections::BTreeMap::from([("HTTPS_PROXY".to_string(), "http://corp:3128".to_string()), ("https_proxy".to_string(), "x".to_string()), ("FOO".to_string(), "1".to_string())]);
        let (env, replaced) = with_proxy_env(base, &plan, 19883);
        let get = |k: &str| env.iter().filter(|(n, _)| n == k).map(|(_, v)| v.as_str()).collect::<Vec<_>>();
        for k in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", ENV_JOB_PROXY] {
            assert_eq!(get(k), vec!["http://127.0.0.1:19883"], "{k} once, the proxy's");
        }
        assert_eq!(get("NO_PROXY"), vec!["localhost,127.0.0.1,::1,[::1]"]);
        assert_eq!(get("no_proxy"), vec!["localhost,127.0.0.1,::1,[::1]"]);
        assert_eq!(get("NODE_USE_ENV_PROXY"), vec!["1"]);
        assert_eq!(get("PATH"), vec!["/usr/bin"]);
        assert_eq!(replaced, vec!["HTTPS_PROXY".to_string(), "https_proxy".to_string()]);
        assert_eq!(replaced_line(&replaced).unwrap(), "network: your --env HTTPS_PROXY, https_proxy was replaced by the node's no-lan proxy");
        let (_, none) = with_proxy_env(vec![], &Default::default(), 19880);
        assert!(none.is_empty() && replaced_line(&none).is_none());
    }

    /// 0.46.17: a proxy in the job's env never takes loopback: NO_PROXY and
    /// no_proxy name localhost, 127.0.0.1 and ::1, keeping the job's own.
    #[test]
    fn a_proxy_in_the_env_never_takes_loopback() {
        let e = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        let get = |env: &[(String, String)], k: &str| env.iter().filter(|(n, _)| n == k).map(|(_, v)| v.clone()).collect::<Vec<_>>();
        // No proxy: nothing changes.
        let (env, changed) = with_loopback_no_proxy(e(&[("PATH", "/usr/bin"), ("NO_PROXY", "corp.internal")]));
        assert!(!changed);
        assert_eq!(env, e(&[("PATH", "/usr/bin"), ("NO_PROXY", "corp.internal")]));
        // A job's own proxy without NO_PROXY: both spellings name loopback.
        let (env, changed) = with_loopback_no_proxy(e(&[("http_proxy", "http://proxy.example:3128")]));
        assert!(changed);
        assert_eq!(get(&env, "NO_PROXY"), vec!["localhost,127.0.0.1,::1"]);
        assert_eq!(get(&env, "no_proxy"), vec!["localhost,127.0.0.1,::1"]);
        // Its own entries stay, both spellings merged; loopback added once.
        let (env, changed) = with_loopback_no_proxy(e(&[("HTTPS_PROXY", "http://p:1"), ("NO_PROXY", "a.example, LOCALHOST"), ("no_proxy", "b.example")]));
        assert!(changed);
        assert_eq!(get(&env, "NO_PROXY"), vec!["a.example,LOCALHOST,b.example,127.0.0.1,::1"]);
        assert_eq!(get(&env, "no_proxy"), vec!["a.example,LOCALHOST,b.example,127.0.0.1,::1"]);
        // `*` (no proxy for anything) is left alone; ALL_PROXY counts.
        let (_, changed) = with_loopback_no_proxy(e(&[("ALL_PROXY", "socks5://p:1"), ("no_proxy", "*")]));
        assert!(!changed);
        let (env, changed) = with_loopback_no_proxy(e(&[("all_proxy", "socks5://p:1")]));
        assert!(changed && get(&env, "no_proxy") == vec!["localhost,127.0.0.1,::1"]);
        // The Mac no-lan proxy's env already names loopback: unchanged.
        let (proxied, _) = with_proxy_env(vec![], &Default::default(), 19883);
        let (after, changed) = with_loopback_no_proxy(proxied.clone());
        assert!(!changed, "{after:?}");
        assert_eq!(after, proxied);
    }

    /// `K2_JOB_PROXY` is K2-reserved: a job's `--env` can't fake it.
    #[test]
    fn the_job_proxy_name_is_reserved() {
        assert_eq!(k2_node_proto::env::env_name_refusal(ENV_JOB_PROXY), Some("env_k2_reserved"));
        assert_eq!(k2_node_proto::env::env_name_refusal("HTTPS_PROXY"), None, "a job may still pass its own HTTPS_PROXY (Linux, on)");
    }
}
