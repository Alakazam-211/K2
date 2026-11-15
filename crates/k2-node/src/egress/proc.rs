//! `k2-node job-egress <dir>`: one proxy process per Mac no-lan job
//! (prd-compute-mac-no-lan-v1 §5.2), and the node's side of it.
//!
//! - The node **execs** it (inside its own Seatbelt profile on a Mac,
//!   [`sandbox_profile`]); like the job holder ([`crate::hold`]) it forks,
//!   `setsid`s and forks again before any thread starts, so it is in no
//!   process group of the node's and outlives a k2-node restart or update.
//! - Its folder (`<node state>/egress/<job key>/`, which every job's
//!   profile denies) holds the node's `spec.json` (port, deadline, the
//!   node's prefixes at start; read once and deleted), its own `egress.json`
//!   (pid, start time, port: written once it listens), `egress.jsonl` (one
//!   line per decision, [`super::Decision`]), the node's `job.json` (the
//!   job's process group, once the job started) and `exit.json` (how it
//!   ended).
//! - It ends when the job's process group is gone (checked every
//!   [`WATCH_EVERY`]), on SIGTERM (the node), or at its deadline (the job's
//!   plus [`crate::hold::DEADLINE_SLACK_SECS`]). Open tunnels close with it.
//! - A port another process holds: `exit.json` says `port_taken` and the
//!   node tries the next pool port.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Decision, Log, Policy};

pub const EGRESS_V: u32 = 1;
pub const SPEC_FILE: &str = "spec.json";
pub const FACTS_FILE: &str = "egress.json";
pub const EXIT_FILE: &str = "exit.json";
pub const JOB_FILE: &str = "job.json";
pub const LOG_FILE: &str = "egress.jsonl";
/// How often the proxy looks for its job's process group.
pub const WATCH_EVERY: Duration = Duration::from_secs(2);
/// How much of `egress.jsonl` one tail read takes.
const TAIL_CHUNK: u64 = 1 << 20;

/// Test hooks a debug build honours (a release build refuses a spec with
/// any): a local upstream admitted as public, a scripted resolver, a short
/// idle limit, no own-LAN reading.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestHooks {
    #[serde(default)]
    pub public: Vec<SocketAddr>,
    /// Name → its answers, one per lookup (the last repeats).
    #[serde(default)]
    pub resolve: BTreeMap<String, Vec<Vec<IpAddr>>>,
    #[serde(default)]
    pub idle_ms: Option<u64>,
}

/// What the node asks the proxy to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressSpec {
    pub v: u32,
    pub port: u16,
    /// Unix seconds; 0 = none.
    pub deadline: i64,
    pub job_key: String,
    /// The node's own prefixes at start (the proxy reads them again
    /// itself every [`super::LAN_REFRESH`]).
    #[serde(default)]
    pub lan: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<TestHooks>,
}

/// What the proxy says once it listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressFacts {
    pub v: u32,
    pub pid: u32,
    /// [`crate::util::process_start_time`] of the proxy.
    pub start: u64,
    pub port: u16,
    pub started_at: i64,
}

/// How the proxy ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressExit {
    pub v: u32,
    pub ended_at: i64,
    /// `job_ended`, `stopped` (SIGTERM), `deadline`, or `error`.
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub port_taken: bool,
}

/// The job the proxy serves (written by the node once it started).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobWatch {
    pub pgid: i32,
    #[serde(default)]
    pub job_start: Option<u64>,
}

fn write_atomic(dir: &Path, name: &str, v: &impl Serialize) -> Result<(), String> {
    let b = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&dir.join(name), &b, 0o600)
}

fn read_json<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Option<T> {
    crate::hold::read_small(dir, name).and_then(|b| serde_json::from_slice(&b).ok())
}

pub fn read_facts(dir: &Path) -> Option<EgressFacts> {
    read_json(dir, FACTS_FILE)
}

pub fn read_exit(dir: &Path) -> Option<EgressExit> {
    read_json(dir, EXIT_FILE)
}

/// Tell the proxy which process group is its job.
pub fn write_job(dir: &Path, pgid: i32, job_start: Option<u64>) -> Result<(), String> {
    write_atomic(dir, JOB_FILE, &JobWatch { pgid, job_start })
}

/// Is the proxy these facts name still running?
pub fn alive(f: &EgressFacts) -> bool {
    crate::hold::alive(f.pid, f.start)
}

/// Ask it to stop (SIGTERM; it closes its tunnels and writes `exit.json`).
pub fn terminate(f: &EgressFacts) -> bool {
    crate::hold::terminate(f.pid, f.start)
}

/// The proxy's own Seatbelt profile (macOS): it parses the job's bytes, so
/// it gets less than the node has. Network is open (it is the way out);
/// file writes only in its folder; the node's `state/` and the other
/// `deny_dirs` (people's homes, the keychains) unreadable except its folder.
pub fn sandbox_profile(dir: &Path, deny_dirs: &[PathBuf]) -> String {
    let dir = crate::wall::sb_str(&crate::wall::real_path(dir));
    let mut p = String::from("(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write* (literal \"/dev/null\"))\n");
    for d in deny_dirs {
        p.push_str(&format!("(deny file-read* (subpath {}))\n", crate::wall::sb_str(&crate::wall::real_path(d))));
    }
    p.push_str(&format!("(allow file-read* file-write* (subpath {dir}))\n"));
    p
}

/// Why a proxy didn't start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// Something else holds the port: try the next one.
    PortTaken,
    Failed(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::PortTaken => write!(f, "the port is taken"),
            SpawnError::Failed(s) => write!(f, "{s}"),
        }
    }
}

/// Start a proxy for `spec` in `dir` and read its facts once. `fresh`:
/// the folder is made empty first (a restart keeps its log and `job.json`).
/// `sandbox`: the proxy's Seatbelt profile (macOS). Blocking (≤ `wait`).
pub fn spawn(bin: &Path, dir: &Path, spec: &EgressSpec, sandbox: Option<&str>, fresh: bool, wait: Duration) -> Result<EgressFacts, SpawnError> {
    let fail = |m: String| SpawnError::Failed(m);
    if fresh && dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| fail(format!("clear {}: {e}", dir.display())))?;
    }
    if !dir.is_dir() {
        crate::util::mkdir_mode(dir, 0o700).map_err(fail)?;
    }
    for f in [FACTS_FILE, EXIT_FILE] {
        let _ = std::fs::remove_file(dir.join(f));
    }
    write_atomic(dir, SPEC_FILE, spec).map_err(fail)?;
    let mut argv: Vec<std::ffi::OsString> = Vec::new();
    if let Some(p) = sandbox {
        argv.extend([crate::wall::SANDBOX_EXEC.into(), "-p".into(), p.into(), "--".into()]);
    }
    argv.extend([bin.as_os_str().to_owned(), "job-egress".into(), dir.as_os_str().to_owned()]);
    let mut first = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir("/")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| fail(format!("start {}: {e}", bin.display())))?;
    let st = first.wait().map_err(|e| fail(format!("job-egress: {e}")))?;
    if !st.success() {
        let mut err = String::new();
        if let Some(mut e) = first.stderr.take() {
            use std::io::Read;
            let _ = (&mut e).take(4096).read_to_string(&mut err);
        }
        return Err(fail(format!("job-egress failed to start ({st}){}", if err.trim().is_empty() { String::new() } else { format!(": {}", err.trim()) })));
    }
    let t0 = std::time::Instant::now();
    loop {
        if let Some(f) = read_facts(dir) {
            if f.v != EGRESS_V || f.pid <= 1 || f.port != spec.port {
                return Err(fail(format!("the proxy's facts don't read: {f:?}")));
            }
            return Ok(f);
        }
        if let Some(x) = read_exit(dir) {
            if x.port_taken {
                return Err(SpawnError::PortTaken);
            }
            return Err(fail(x.error.unwrap_or_else(|| "the proxy ended before it listened".into())));
        }
        if t0.elapsed() > wait {
            return Err(fail(format!("the proxy didn't listen within {} s", wait.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The pool ports to try, in order: the job's own (its port block's index)
/// first, then every other free one. `in_use`: the node's running proxies.
pub fn port_order(block: Option<u16>, in_use: &std::collections::BTreeSet<u16>) -> Vec<u16> {
    let first = block.filter(|b| *b < super::POOL_LEN).map(|b| super::POOL_FIRST + b);
    let mut out: Vec<u16> = first.into_iter().collect();
    out.extend(super::pool_ports().filter(|p| Some(*p) != first));
    out.retain(|p| !in_use.contains(p));
    out
}

/// Reads `egress.jsonl` on from an offset, whole lines only (a line the
/// proxy is still writing is read next time).
#[derive(Debug, Clone)]
pub struct Tail {
    pub path: PathBuf,
    pub offset: u64,
}

impl Tail {
    pub fn new(dir: &Path, offset: u64) -> Self {
        Self { path: dir.join(LOG_FILE), offset }
    }

    /// The new whole lines (bad ones skipped).
    pub fn read_new(&mut self) -> Vec<Decision> {
        use std::io::{Read, Seek, SeekFrom};
        let mut out = Vec::new();
        loop {
            let Ok(mut f) = std::fs::File::open(&self.path) else { return out };
            if f.seek(SeekFrom::Start(self.offset)).is_err() {
                return out;
            }
            let mut b = Vec::new();
            if f.take(TAIL_CHUNK).read_to_end(&mut b).is_err() {
                return out;
            }
            let Some(end) = b.iter().rposition(|c| *c == b'\n') else { return out };
            out.extend(super::fold::parse_lines(&String::from_utf8_lossy(&b[..=end])));
            self.offset += end as u64 + 1;
            if (b.len() as u64) < TAIL_CHUNK {
                return out;
            }
        }
    }
}

/// Every decision in a proxy folder's log.
pub fn read_all(dir: &Path) -> Vec<Decision> {
    Tail::new(dir, 0).read_new()
}

// ── the proxy process ───────────────────────────────────────────────

/// `k2-node job-egress <dir>`. Must run before any thread or runtime
/// starts. Returns the first process's exit code (0 once the proxy is on
/// its own).
pub fn main(dir: &Path) -> i32 {
    let spec = match read_json::<EgressSpec>(dir, SPEC_FILE) {
        Some(s) if s.v <= EGRESS_V && s.port > 0 => s,
        _ => {
            let _ = write_atomic(dir, EXIT_FILE, &exit("error", Some("no readable proxy spec".into()), false));
            return 1;
        }
    };
    let _ = std::fs::remove_file(dir.join(SPEC_FILE));
    if spec.test.is_some() && !cfg!(debug_assertions) {
        let _ = write_atomic(dir, EXIT_FILE, &exit("error", Some("test hooks in a release build's proxy spec".into()), false));
        return 1;
    }
    // SAFETY: fork before any thread exists in this process (k2-node runs
    // `job-egress` before its runtime); each child only calls setsid,
    // fork and _exit until the proxy proper.
    match unsafe { libc::fork() } {
        -1 => return 1,
        0 => {}
        _ => return 0,
    }
    // SAFETY: as above.
    unsafe {
        libc::setsid();
        match libc::fork() {
            -1 => libc::_exit(1),
            0 => {}
            _ => libc::_exit(0),
        }
    }
    let code = run(dir, spec);
    // SAFETY: the proxy ends without running atexit handlers twice.
    unsafe { libc::_exit(code) }
}

fn exit(why: &str, error: Option<String>, port_taken: bool) -> EgressExit {
    EgressExit { v: EGRESS_V, ended_at: crate::util::now(), why: why.into(), error, port_taken }
}

/// Has the job's process group ended (or been reused)?
fn group_gone(w: &JobWatch) -> bool {
    if w.pgid <= 1 {
        return true;
    }
    if let (Some(want), Some(now)) = (w.job_start, crate::util::process_start_time(w.pgid as u32)) {
        if want != now {
            return true;
        }
    }
    // SAFETY: signal 0 only checks the group exists.
    let r = unsafe { libc::kill(-w.pgid, 0) };
    r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

fn run(dir: &Path, spec: EgressSpec) -> i32 {
    let fail = |why: &str, e: String, taken: bool| {
        let _ = write_atomic(dir, EXIT_FILE, &exit(why, Some(e), taken));
        1
    };
    let listener = match std::net::TcpListener::bind(("127.0.0.1", spec.port)) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return fail("error", format!("127.0.0.1:{} is taken", spec.port), true),
        Err(e) => return fail("error", format!("listen on 127.0.0.1:{}: {e}", spec.port), false),
    };
    if let Err(e) = listener.set_nonblocking(true) {
        return fail("error", format!("listener: {e}"), false);
    }
    let me = std::process::id();
    let Some(start) = crate::util::process_start_time(me) else {
        return fail("error", "the proxy can't read its own start time".into(), false);
    };
    let log = match Log::open(&dir.join(LOG_FILE), super::LOG_CAP) {
        Ok(l) => Arc::new(l),
        Err(e) => return fail("error", e, false),
    };
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).max_blocking_threads(16).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return fail("error", format!("runtime: {e}"), false),
    };
    let policy = {
        let (resolve, own_lan): (super::Resolve, super::OwnLan) = match &spec.test {
            Some(t) => {
                let answers = t.resolve.clone();
                let calls = Arc::new(std::sync::Mutex::new(BTreeMap::<String, usize>::new()));
                let r: super::Resolve = Arc::new(move |host: &str| {
                    let seq = answers.get(host).ok_or_else(|| format!("no such host {host}"))?;
                    let mut c = calls.lock().unwrap_or_else(|e| e.into_inner());
                    let n = c.entry(host.to_string()).or_insert(0);
                    let a = seq.get((*n).min(seq.len().saturating_sub(1))).cloned().unwrap_or_default();
                    *n += 1;
                    Ok(a)
                });
                (r, Arc::new(Vec::new))
            }
            None => (super::system_resolve(), super::system_own_lan()),
        };
        let mut p = Policy::new(resolve, own_lan);
        if let Some(t) = &spec.test {
            p = p.with_test_public(t.public.clone());
            if let Some(ms) = t.idle_ms {
                p = p.with_timeouts(Duration::from_millis(ms), super::HEAD_TIMEOUT);
            }
        }
        Arc::new(p)
    };
    let why = rt.block_on(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => return Err(format!("listener: {e}")),
        };
        if let Err(e) = write_atomic(dir, FACTS_FILE, &EgressFacts { v: EGRESS_V, pid: me, start, port: spec.port, started_at: crate::util::now() }) {
            return Err(format!("{FACTS_FILE}: {e}"));
        }
        let (tx, rx) = tokio::sync::watch::channel(false);
        let server = tokio::spawn(super::serve(listener, policy, log.clone(), rx));
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|e| e.to_string())?;
        let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).map_err(|e| e.to_string())?;
        let mut tick = tokio::time::interval(WATCH_EVERY);
        let why = loop {
            tokio::select! {
                _ = term.recv() => break "stopped",
                _ = int.recv() => break "stopped",
                _ = tick.tick() => {
                    if spec.deadline > 0 && crate::util::now() >= spec.deadline {
                        break "deadline";
                    }
                    if read_json::<JobWatch>(dir, JOB_FILE).is_some_and(|w| group_gone(&w)) {
                        break "job_ended";
                    }
                }
            }
        };
        let _ = tx.send(true);
        let _ = server.await;
        log.finish();
        Ok(why)
    });
    let code = match why {
        Ok(w) => match write_atomic(dir, EXIT_FILE, &exit(w, None, false)) {
            Ok(()) => 0,
            Err(_) => 1,
        },
        Err(e) => fail("error", e, false),
    };
    // A lookup still running on a blocking thread never holds the exit.
    rt.shutdown_background();
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_newer_fields_are_ignored() {
        let s = EgressSpec { v: 1, port: 19883, deadline: 9, job_key: "j".into(), lan: vec!["10.0.0.0/8".into()], test: None };
        let j = serde_json::to_value(&s).unwrap();
        assert!(j.get("test").is_none(), "no hooks key in a real spec");
        let mut j2 = j.clone();
        j2["later"] = serde_json::json!(1);
        assert_eq!(serde_json::from_value::<EgressSpec>(j2).unwrap(), s);
        let x: EgressExit = serde_json::from_str(r#"{"v":1,"ended_at":3,"why":"job_ended"}"#).unwrap();
        assert_eq!((x.why.as_str(), x.port_taken, x.error), ("job_ended", false, None));
    }

    #[test]
    fn the_job_s_own_pool_port_comes_first() {
        let none = std::collections::BTreeSet::new();
        let o = port_order(Some(3), &none);
        assert_eq!((o[0], o[1], o.len()), (19883, 19880, 120));
        assert_eq!(port_order(None, &none)[0], 19880);
        assert_eq!(port_order(Some(500), &none)[0], 19880, "a block past the pool: the first free");
        let used: std::collections::BTreeSet<u16> = [19883, 19880].into();
        let o = port_order(Some(3), &used);
        assert_eq!((o[0], o.len()), (19881, 118));
    }

    #[test]
    fn the_tail_reads_whole_lines_only() {
        let d = crate::util::temp_dir("egress-tail");
        let mut t = Tail::new(&d, 0);
        assert!(t.read_new().is_empty(), "no log yet");
        let line = |h: &str| format!("{}\n", serde_json::to_string(&Decision::new("refused", h, 443)).unwrap());
        let half = line("b.example.com");
        std::fs::write(d.join(LOG_FILE), format!("{}{}", line("a.example.com"), &half[..10])).unwrap();
        let got = t.read_new();
        assert_eq!(got.iter().map(|d| d.host.as_str()).collect::<Vec<_>>(), vec!["a.example.com"]);
        std::fs::write(d.join(LOG_FILE), format!("{}{}", line("a.example.com"), half)).unwrap();
        assert_eq!(t.read_new().iter().map(|d| d.host.as_str()).collect::<Vec<_>>(), vec!["b.example.com"], "the rest of the half line, once whole");
        assert!(t.read_new().is_empty());
        // A second reader picks up from a saved offset (adoption).
        let mut t2 = Tail::new(&d, t.offset);
        std::fs::OpenOptions::new().append(true).open(d.join(LOG_FILE)).map(|mut f| std::io::Write::write_all(&mut f, line("c.example.com").as_bytes())).unwrap().unwrap();
        assert_eq!(t2.read_new().len(), 1);
        assert_eq!(read_all(&d).len(), 3);
    }

    #[test]
    fn the_proxy_profile_writes_only_its_folder() {
        let p = sandbox_profile(Path::new("/nonexistent-k2/state/egress/j1"), &[PathBuf::from("/nonexistent-k2/state"), PathBuf::from("/Users")]);
        let lines: Vec<&str> = p.lines().collect();
        assert_eq!(lines[..4], ["(version 1)", "(allow default)", "(deny file-write*)", "(allow file-write* (literal \"/dev/null\"))"]);
        assert!(lines.contains(&"(deny file-read* (subpath \"/nonexistent-k2/state\"))") && lines.contains(&"(deny file-read* (subpath \"/Users\"))"));
        assert_eq!(*lines.last().unwrap(), "(allow file-read* file-write* (subpath \"/nonexistent-k2/state/egress/j1\"))", "its folder last: Seatbelt takes the last match");
        assert!(!p.contains("network"), "the network stays open: it is the way out");
    }

    #[test]
    fn a_gone_or_reused_group_is_gone() {
        use std::os::unix::process::CommandExt;
        let mut c = std::process::Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap();
        let pgid = c.id() as i32;
        let start = crate::util::process_start_time(pgid as u32);
        assert!(!group_gone(&JobWatch { pgid, job_start: start }));
        assert!(group_gone(&JobWatch { pgid, job_start: start.map(|s| s + 1) }), "another start time: a reused pgid");
        assert!(group_gone(&JobWatch { pgid: 1, job_start: None }) && group_gone(&JobWatch { pgid: 0, job_start: None }));
        c.kill().unwrap();
        c.wait().unwrap();
        assert!(group_gone(&JobWatch { pgid, job_start: None }));
    }
}
