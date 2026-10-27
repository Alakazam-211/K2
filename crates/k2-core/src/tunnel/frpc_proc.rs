//! An frpc process that outlives the daemon that started it (runtime
//! stage H, vs-live AR3/K37).
//!
//! Before stage H frpc was a piped child: its stdout/stderr went through
//! two pump threads in the daemon, so the daemon's exit closed those pipes
//! and frpc's next log line hit `EPIPE`/`SIGPIPE` and killed it — on top of
//! systemd's `KillMode=control-group` killing it outright. A daemon restart
//! therefore always cost the tunnel.
//!
//! Now:
//! - frpc writes straight into `~/.k2/frpc.log` (an `O_APPEND` file, no
//!   pipe), in its own session (`setsid`), and the connector **tails** the
//!   log to feed the relay watchdog and the proxy-registration tracker;
//! - every spawn records `frpc.pid` (pid, the process's start-time
//!   identity, the config path, the log offset the run started at) next to
//!   the rendered config;
//! - the next daemon **adopts** a live frpc whose pid file, identity,
//!   command line and rendered config all still match, instead of reaping
//!   it and dialing the relay again ([`try_adopt`]).
//!
//! An adopted frpc is not our child: we can't `wait()` it, so liveness is a
//! pid probe that also treats a zombie as dead (the previous daemon may be
//! draining and never reap it).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// A supervised frpc: our own child, or one adopted from a previous daemon.
pub(crate) enum FrpcProc {
    Spawned(Child),
    Adopted { pid: i32 },
}

impl FrpcProc {
    pub(crate) fn pid(&self) -> i32 {
        match self {
            FrpcProc::Spawned(c) => c.id() as i32,
            FrpcProc::Adopted { pid } => *pid,
        }
    }

    /// `Ok(Some(code))` once it has exited (`code` unknown for an adopted
    /// process), `Ok(None)` while it runs.
    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<Option<i32>>> {
        match self {
            FrpcProc::Spawned(c) => Ok(c.try_wait()?.map(|s| s.code())),
            FrpcProc::Adopted { pid } => Ok((!pid_alive(*pid)).then_some(None)),
        }
    }

    /// Kill and reap (or, adopted: SIGTERM, wait up to 1 s, SIGKILL).
    pub(crate) fn kill_and_wait(&mut self) {
        match self {
            FrpcProc::Spawned(c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            FrpcProc::Adopted { pid } => kill_foreign(*pid),
        }
    }
}

#[cfg(unix)]
fn kill_foreign(pid: i32) {
    if pid <= 0 {
        return;
    }
    // SAFETY: plain signal sends to a positive pid.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline && pid_alive(pid) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Windows: TerminateProcess, then wait up to 1 s for it to be gone.
#[cfg(windows)]
fn kill_foreign(pid: i32) {
    if pid <= 0 {
        return;
    }
    crate::win_proc::terminate(pid as u32);
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline && pid_alive(pid) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(any(unix, windows)))]
fn kill_foreign(_pid: i32) {}

/// The process exists and is not a zombie.
#[cfg(target_os = "linux")]
pub(crate) fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // `pid (comm) S …` — comm may hold spaces/parens: split after the last ')'.
    match stat.rfind(')').and_then(|i| stat[i + 1..].split_whitespace().next()) {
        Some("Z") | Some("X") | None => false,
        Some(_) => true,
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks existence.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output();
    match out {
        Ok(o) => !String::from_utf8_lossy(&o.stdout).trim_start().starts_with('Z'),
        Err(_) => true,
    }
}

#[cfg(windows)]
pub(crate) fn pid_alive(pid: i32) -> bool {
    pid > 0 && crate::win_proc::pid_alive(pid as u32)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn pid_alive(_pid: i32) -> bool {
    false
}

/// A stable identity for a live pid (its start time), so a recycled pid is
/// never mistaken for the frpc we recorded.
#[cfg(target_os = "linux")]
pub(crate) fn proc_identity(pid: i32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    // Fields after comm: state(3) … starttime is field 22 → index 19 here.
    rest.split_whitespace().nth(19).map(|s| format!("st:{s}"))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn proc_identity(pid: i32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| format!("ls:{s}"))
}

/// Windows: the creation time (GetProcessTimes).
#[cfg(windows)]
pub(crate) fn proc_identity(pid: i32) -> Option<String> {
    u32::try_from(pid).ok().and_then(crate::win_proc::start_key).map(|t| format!("ct:{t}"))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn proc_identity(_pid: i32) -> Option<String> {
    None
}

/// The process's argv.
#[cfg(target_os = "linux")]
pub(crate) fn proc_argv(pid: i32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).to_string())
            .collect(),
    )
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn proc_argv(pid: i32) -> Option<Vec<String>> {
    // `ps -o command=` joins argv with spaces; frpc's argv is
    // `<frpc> -c <cfg>` and the config path carries no spaces in practice,
    // but compare on the joined string to stay honest.
    let out = std::process::Command::new("ps")
        .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| vec![s])
}

/// Windows: the recorded command line as one string (like the `ps` form);
/// a quoted program keeps its trailing quote on the file name, which the
/// `frpc` prefix check ignores.
#[cfg(windows)]
pub(crate) fn proc_argv(pid: i32) -> Option<Vec<String>> {
    u32::try_from(pid).ok().and_then(crate::win_proc::command_line).map(|s| vec![s])
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn proc_argv(_pid: i32) -> Option<Vec<String>> {
    None
}

/// `<frpc> -c <cfg>` exactly (argv0's basename starts with `frpc`).
pub(crate) fn argv_is_our_frpc(argv: &[String], cfg: &Path) -> bool {
    let cfg = cfg.to_string_lossy();
    let is_frpc = |a0: &str| {
        Path::new(a0)
            .file_name()
            .map(|n| n.to_string_lossy().starts_with("frpc"))
            .unwrap_or(false)
    };
    match argv {
        [a0, flag, path] => is_frpc(a0) && flag == "-c" && path == cfg.as_ref(),
        // A `#!` frpc stand-in (tests): `<interpreter> <…/frpc> -c <cfg>`.
        [_interp, script, flag, path] => {
            is_frpc(script) && flag == "-c" && path == cfg.as_ref()
        }
        // The non-Linux `ps` form: one joined string.
        [joined] => {
            let mut it = joined.rsplitn(3, ' ');
            let (Some(path), Some(flag), Some(a0)) = (it.next(), it.next(), it.next()) else {
                return false;
            };
            is_frpc(a0) && flag == "-c" && path == cfg.as_ref()
        }
        _ => false,
    }
}

/// What a spawn records for the next daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PidRecord {
    pub pid: i32,
    pub identity: String,
    pub cfg: PathBuf,
    /// `frpc.log` length when this run started: the adopter replays the
    /// run's lines from here (proxy registration state).
    pub log_offset: u64,
}

/// `frpc.pid` beside the rendered config (the private runtime dir).
pub(crate) fn pid_path(cfg: &Path) -> PathBuf {
    cfg.with_file_name("frpc.pid")
}

pub(crate) fn write_pid_record(rec: &PidRecord) {
    let path = pid_path(&rec.cfg);
    let res = (|| -> std::io::Result<()> {
        let body = serde_json::to_vec(rec).map_err(std::io::Error::other)?;
        let tmp = path.with_extension(format!("pid.tmp.{}", std::process::id()));
        std::fs::write(&tmp, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = res {
        crate::log_debug!("[tunnel] WARN: write {}: {e}", path.display());
    }
}

pub(crate) fn read_pid_record(cfg: &Path) -> Option<PidRecord> {
    let raw = std::fs::read(pid_path(cfg)).ok()?;
    serde_json::from_slice(&raw).ok()
}

pub(crate) fn clear_pid_record(cfg: &Path) {
    let _ = std::fs::remove_file(pid_path(cfg));
}

/// Why an frpc was not adopted (logged; the caller then reaps + spawns).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoAdopt {
    NoRecord,
    Dead,
    Recycled,
    ForeignArgv,
    ConfigChanged,
}

/// Adopt the recorded frpc when every check passes: the pid record names
/// our config, the process lives (not a zombie), its start-time identity
/// and argv match, and the config on disk is byte-for-byte what this daemon
/// would render now (same relay, same `localPort`, same identity).
pub(crate) fn try_adopt(cfg_path: &Path, rendered: &str) -> Result<PidRecord, NoAdopt> {
    let rec = read_pid_record(cfg_path).ok_or(NoAdopt::NoRecord)?;
    if rec.cfg != cfg_path {
        return Err(NoAdopt::NoRecord);
    }
    if !pid_alive(rec.pid) {
        return Err(NoAdopt::Dead);
    }
    if proc_identity(rec.pid).as_deref() != Some(rec.identity.as_str()) {
        return Err(NoAdopt::Recycled);
    }
    match proc_argv(rec.pid) {
        Some(argv) if argv_is_our_frpc(&argv, cfg_path) => {}
        _ => return Err(NoAdopt::ForeignArgv),
    }
    match std::fs::read_to_string(cfg_path) {
        Ok(on_disk) if on_disk == rendered => Ok(rec),
        _ => Err(NoAdopt::ConfigChanged),
    }
}

/// Tail `log` from `offset`, handing each complete line to `on_line`, until
/// `pid` is gone (and the file is drained) or `stop` is set. Polls every
/// 100 ms; a truncated file restarts from 0.
pub(crate) fn spawn_tail(
    log: PathBuf,
    offset: u64,
    pid: i32,
    stop: Arc<AtomicBool>,
    mut on_line: impl FnMut(&str) + Send + 'static,
) {
    let _ = std::thread::Builder::new()
        .name("k2so-frpc-log-tail".into())
        .spawn(move || {
            let mut pos = offset;
            let mut partial = Vec::<u8>::new();
            let mut buf = vec![0u8; 16 * 1024];
            // frpc can write its last line and exit (and be reaped) between
            // a pass that read nothing and the liveness check: read the file
            // once more after the pid is gone before giving up, or its error
            // line ("open …: no such file or directory") is never seen.
            let mut drained_after_exit = false;
            loop {
                let mut read_any = false;
                if let Ok(mut f) = std::fs::File::open(&log) {
                    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                    if len < pos {
                        pos = 0; // truncated / rotated
                        partial.clear();
                    }
                    if f.seek(SeekFrom::Start(pos)).is_ok() {
                        loop {
                            match f.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    read_any = true;
                                    pos += n as u64;
                                    partial.extend_from_slice(&buf[..n]);
                                    while let Some(i) = partial.iter().position(|b| *b == b'\n') {
                                        let line: Vec<u8> = partial.drain(..=i).collect();
                                        let s = String::from_utf8_lossy(&line[..line.len() - 1]);
                                        on_line(s.trim_end_matches('\r'));
                                    }
                                }
                            }
                        }
                    }
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                if !read_any && !pid_alive(pid) {
                    if drained_after_exit {
                        return;
                    }
                    drained_after_exit = true;
                    continue;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
}

/// Current length of `log` (0 when missing).
pub(crate) fn log_len(log: &Path) -> u64 {
    std::fs::metadata(log).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_match_is_exact() {
        let cfg = Path::new("/run/user/1000/k2/frpc.toml");
        let ok = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(argv_is_our_frpc(
            &ok(&["/usr/local/bin/frpc", "-c", "/run/user/1000/k2/frpc.toml"]),
            cfg
        ));
        assert!(argv_is_our_frpc(
            &ok(&["/usr/local/bin/frpc -c /run/user/1000/k2/frpc.toml"]),
            cfg
        ));
        for bad in [
            ok(&["/usr/bin/sleep", "-c", "/run/user/1000/k2/frpc.toml"]),
            ok(&["/usr/local/bin/frpc", "-c", "/tmp/other.toml"]),
            ok(&["/usr/local/bin/frpc", "-c", "/run/user/1000/k2/frpc.toml", "x"]),
            ok(&["/usr/local/bin/frpc"]),
            ok(&[]),
        ] {
            assert!(!argv_is_our_frpc(&bad, cfg), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn identity_and_liveness_follow_the_real_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as i32;
        assert!(pid_alive(pid));
        let id1 = proc_identity(pid).expect("identity");
        assert_eq!(proc_identity(pid).as_deref(), Some(id1.as_str()), "stable");
        child.kill().expect("kill");
        // Not reaped yet → a zombie, which must count as dead.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!pid_alive(pid), "a zombie is not a live frpc");
        child.wait().expect("reap");
        assert!(!pid_alive(pid));
    }
}
