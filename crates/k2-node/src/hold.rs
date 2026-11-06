//! The job holder (prd-compute-update-continuity-v1 §5.2): `k2-node
//! job-hold <dir>`, one small process per job that is not the k2-node
//! process, so a job outlives a k2-node restart and the next k2-node
//! adopts it.
//!
//! Used where no service manager holds the job: macOS (the Seatbelt wall;
//! launchd stops the node's whole process group, a `setsid` holder is out
//! of it) and test nodes. The Linux job wall needs none: each job is its
//! own systemd unit outside the node's cgroup.
//!
//! - The node **execs** the holder (never forks inside its threaded
//!   runtime). The holder forks, `setsid`s and forks again before it
//!   starts any thread, so it is in no process group or session of the
//!   node's (`k2-node` dispatches `job-hold` before its runtime starts).
//! - Its folder (`<node state>/hold/<job key>/`) is the node's only (the
//!   Seatbelt profile denies `state/` to jobs). The node writes `spec.json`;
//!   the holder reads it once and deletes it (the job's env may carry
//!   tokens), starts the job in its own process group with stdin
//!   `/dev/null` and stdout/stderr = the spool fds it inherited (it never
//!   opens a spool path), and writes `hold.json` (its pid, start time, the
//!   job's pgid). The node reads that **once** into the run facts and never
//!   trusts the file again.
//! - SIGTERM = cancel: SIGTERM to the job's group, SIGKILL after the grace.
//!   At its deadline it does the same itself (`timeout`): no job outlives
//!   its limit when no node is watching.
//! - When the job ends: SIGKILL whatever is left of its group, `exit.json`
//!   (code, signal, ended_at, timeout), exit. Files and signals only: any
//!   later node reads a v1 holder.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The holder files' format.
pub const HOLD_V: u32 = 1;
pub const SPEC_FILE: &str = "spec.json";
pub const FACTS_FILE: &str = "hold.json";
pub const EXIT_FILE: &str = "exit.json";
/// Holder files are small; a bigger one is refused.
const MAX_FILE: u64 = 1 << 20;
/// The holder's deadline is the job's plus this: the node, while it runs,
/// always stops a job at its own deadline first.
pub const DEADLINE_SLACK_SECS: i64 = 30;

/// What the node asks the holder to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldSpec {
    pub v: u32,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// Unix seconds; 0 = none.
    pub deadline: i64,
    pub grace_ms: u64,
}

/// What the holder says once it started the job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldFacts {
    pub v: u32,
    pub pid: u32,
    /// [`crate::util::process_start_time`] of the holder: a reused pid is
    /// never the holder.
    pub start: u64,
    /// The job's process group (= its pid).
    pub pgid: i32,
    /// The job leader's start time: a group whose leader has another
    /// start time is never signalled as this job's (a reused pgid).
    #[serde(default)]
    pub job_start: Option<u64>,
    pub started_at: i64,
}

/// SIGKILL the job group these facts name, unless its leader is alive
/// with another start time (the pgid was reused), or the pgid is 0, 1 or
/// this process's own. A leader that's gone with members left still gets
/// the kill: that group can only be this job's until its last member ends.
pub fn kill_job_group(f: &HoldFacts) {
    let own = unsafe { (libc::getpid(), libc::getpgrp()) };
    if f.pgid <= 1 || f.pgid == own.0 || f.pgid == own.1 {
        return;
    }
    if let (Some(want), Some(now)) = (f.job_start, crate::util::process_start_time(f.pgid as u32)) {
        if want != now {
            return;
        }
    }
    crate::runner::signal_group(f.pgid, libc::SIGKILL);
}

/// How the job ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldExit {
    pub v: u32,
    #[serde(default)]
    pub code: Option<i32>,
    #[serde(default)]
    pub signal: Option<i32>,
    pub ended_at: i64,
    /// The holder stopped it at its deadline.
    #[serde(default)]
    pub timeout: bool,
    /// The job never started.
    #[serde(default)]
    pub error: Option<String>,
}

fn now() -> i64 {
    crate::util::now()
}

/// Read a small regular file in `dir` without following a link.
pub fn read_small(dir: &Path, name: &str) -> Option<Vec<u8>> {
    let p = dir.join(name);
    let m = std::fs::symlink_metadata(&p).ok()?;
    if !m.file_type().is_file() || m.len() > MAX_FILE {
        return None;
    }
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: valid C path; the fd is owned right below.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    // SAFETY: a fresh fd we own.
    let f = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    if !f.metadata().ok()?.file_type().is_file() {
        return None;
    }
    let mut b = Vec::new();
    f.take(MAX_FILE).read_to_end(&mut b).ok()?;
    Some(b)
}

fn write_atomic(dir: &Path, name: &str, v: &impl Serialize) -> Result<(), String> {
    let b = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&dir.join(name), &b, 0o600)
}

/// How the job ended, if the holder said so.
pub fn read_exit(dir: &Path) -> Option<HoldExit> {
    read_small(dir, EXIT_FILE).and_then(|b| serde_json::from_slice(&b).ok())
}

/// Is `pid` still the holder that started at `start`?
pub fn alive(pid: u32, start: u64) -> bool {
    pid > 1 && crate::util::process_start_time(pid) == Some(start)
}

/// Ask the holder to stop its job (SIGTERM → its grace → SIGKILL). Only
/// when the pid is still that holder (pid + start time, checked right
/// before the signal).
pub fn terminate(pid: u32, start: u64) -> bool {
    if !alive(pid, start) {
        return false;
    }
    // SAFETY: a plain kill on a checked pid.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) == 0 }
}

// ── node side ────────────────────────────────────────────────────────

/// Start a holder for `spec` in `dir` (made 0700, emptied), with the
/// job's stdout/stderr as `out`/`err`, and read its facts once. Blocking
/// (≤ `wait`). The first child exits at once (it only forks); it is reaped
/// here.
pub fn spawn(bin: &Path, dir: &Path, spec: &HoldSpec, out: std::os::fd::OwnedFd, err: std::os::fd::OwnedFd, wait: Duration) -> Result<HoldFacts, String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("clear {}: {e}", dir.display()))?;
    }
    crate::util::mkdir_mode(dir, 0o700)?;
    write_atomic(dir, SPEC_FILE, spec)?;
    let mut first = std::process::Command::new(bin)
        .arg("job-hold")
        .arg(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir("/")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(out))
        .stderr(std::process::Stdio::from(err))
        .spawn()
        .map_err(|e| format!("start the job holder {}: {e}", bin.display()))?;
    let st = first.wait().map_err(|e| format!("job holder: {e}"))?;
    if !st.success() {
        return Err(format!("the job holder failed to start ({st})"));
    }
    let t0 = std::time::Instant::now();
    loop {
        if let Some(f) = read_small(dir, FACTS_FILE).and_then(|b| serde_json::from_slice::<HoldFacts>(&b).ok()) {
            if f.v != HOLD_V || f.pid <= 1 || f.pgid <= 1 {
                return Err(format!("the job holder's facts don't read: {f:?}"));
            }
            return Ok(f);
        }
        if let Some(x) = read_exit(dir) {
            return Err(x.error.unwrap_or_else(|| "the job holder ended before it started the job".into()));
        }
        if t0.elapsed() > wait {
            return Err(format!("the job holder didn't start the job within {} s", wait.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ── the holder process ───────────────────────────────────────────────

static TERM: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    TERM.store(true, Ordering::SeqCst);
}

/// `k2-node job-hold <dir>`. Must run before any thread or runtime starts.
/// Returns the first child's exit code (0 once the holder is on its own).
pub fn main(dir: &Path) -> i32 {
    let spec = match read_small(dir, SPEC_FILE).and_then(|b| serde_json::from_slice::<HoldSpec>(&b).ok()) {
        Some(s) if s.v <= HOLD_V && !s.argv.is_empty() => s,
        _ => {
            let _ = write_atomic(dir, EXIT_FILE, &HoldExit { v: HOLD_V, code: None, signal: None, ended_at: now(), timeout: false, error: Some("no readable job spec".into()) });
            return 1;
        }
    };
    let _ = std::fs::remove_file(dir.join(SPEC_FILE));
    // SAFETY: fork before any thread exists in this process (k2-node runs
    // `job-hold` before its runtime); each child only calls setsid, fork
    // and _exit until the holder proper.
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
    let code = hold(dir, spec);
    // SAFETY: the holder ends without running atexit handlers twice.
    unsafe { libc::_exit(code) }
}

fn hold(dir: &Path, spec: HoldSpec) -> i32 {
    // SAFETY: installing a handler that only stores an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_term as *const () as libc::sighandler_t);
    }
    let fail = |m: String| {
        let _ = write_atomic(dir, EXIT_FILE, &HoldExit { v: HOLD_V, code: None, signal: None, ended_at: now(), timeout: false, error: Some(m) });
        1
    };
    let mut cmd = std::process::Command::new(&spec.argv[0]);
    cmd.args(&spec.argv[1..]).env_clear().current_dir(&spec.cwd).stdin(std::process::Stdio::null());
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                // The holder's own handlers never reach the job.
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                crate::runner::raise_nofile();
                Ok(())
            });
        }
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("spawn {}: {e}", spec.argv[0])),
    };
    let pgid = child.id() as i32;
    let me = std::process::id();
    let Some(start) = crate::util::process_start_time(me) else {
        crate::runner::signal_group(pgid, libc::SIGKILL);
        let _ = child.wait();
        return fail("the holder can't read its own start time".into());
    };
    let job_start = crate::util::process_start_time(pgid as u32);
    if let Err(e) = write_atomic(dir, FACTS_FILE, &HoldFacts { v: HOLD_V, pid: me, start, pgid, job_start, started_at: now() }) {
        crate::runner::signal_group(pgid, libc::SIGKILL);
        let _ = child.wait();
        return fail(format!("hold.json: {e}"));
    }
    let mut stop_at: Option<std::time::Instant> = None;
    let mut timeout = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(_) => break None,
        }
        let due = spec.deadline > 0 && now() >= spec.deadline;
        if stop_at.is_none() && (TERM.load(Ordering::SeqCst) || due) {
            timeout = due && !TERM.load(Ordering::SeqCst);
            crate::runner::signal_group(pgid, libc::SIGTERM);
            stop_at = Some(std::time::Instant::now() + Duration::from_millis(spec.grace_ms.max(1)));
        }
        if stop_at.is_some_and(|t| std::time::Instant::now() >= t) {
            crate::runner::signal_group(pgid, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // Nothing the job started outlives it.
    crate::runner::signal_group(pgid, libc::SIGKILL);
    use std::os::unix::process::ExitStatusExt;
    let (code, signal) = status.map(|s| (s.code(), s.signal())).unwrap_or((None, None));
    match write_atomic(dir, EXIT_FILE, &HoldExit { v: HOLD_V, code, signal, ended_at: now(), timeout, error: None }) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    #[test]
    fn holder_files_round_trip_and_newer_fields_are_ignored() {
        let f = HoldFacts { v: 1, pid: 7, start: 9, pgid: 8, job_start: Some(3), started_at: 1 };
        let mut j = serde_json::to_value(&f).unwrap();
        j["later"] = serde_json::json!(true);
        assert_eq!(serde_json::from_value::<HoldFacts>(j).unwrap(), f);
        let x: HoldExit = serde_json::from_str(r#"{"v":1,"ended_at":5}"#).unwrap();
        assert_eq!((x.code, x.signal, x.timeout, x.error), (None, None, false, None));
    }

    #[test]
    fn a_link_is_never_read_as_a_holder_file() {
        let d = crate::util::temp_dir("hold-link");
        std::fs::write(d.join("real"), br#"{"v":1,"ended_at":5,"code":0}"#).unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join(EXIT_FILE)).unwrap();
        assert!(read_exit(&d).is_none());
        std::fs::remove_file(d.join(EXIT_FILE)).unwrap();
        std::fs::rename(d.join("real"), d.join(EXIT_FILE)).unwrap();
        assert_eq!(read_exit(&d).unwrap().code, Some(0));
    }

    /// A reused pgid (its leader alive with another start time) is never
    /// killed as the job's group.
    #[test]
    fn a_reused_job_group_is_never_killed() {
        let mut child = std::process::Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap();
        let pgid = child.id() as i32;
        let real = crate::util::process_start_time(pgid as u32).expect("its start time");
        kill_job_group(&HoldFacts { v: 1, pid: 0, start: 0, pgid, job_start: Some(real + 1), started_at: 0 });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(child.try_wait().unwrap().is_none(), "another start time: left alone");
        kill_job_group(&HoldFacts { v: 1, pid: 0, start: 0, pgid, job_start: Some(real), started_at: 0 });
        let st = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(st.signal(), Some(libc::SIGKILL), "its own group: killed");
    }

    #[test]
    fn a_reused_pid_is_never_the_holder() {
        let me = std::process::id();
        let start = crate::util::process_start_time(me).expect("own start time");
        assert!(alive(me, start));
        assert!(!alive(me, start + 1), "same pid, another start time");
        assert!(!alive(1, 0) && !alive(0, 0), "pid 0 and 1 are never a holder");
        assert!(!terminate(me, start + 1), "never signalled");
    }
}
