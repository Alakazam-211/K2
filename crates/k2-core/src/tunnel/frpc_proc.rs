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

/// macOS: libproc's process status, with `ps` as the fallback when
/// libproc can't say (`ps` is setuid, so it fails under a sandbox).
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks existence.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(info) = crate::mac_proc::bsd_info(pid) {
            return !info.zombie;
        }
    }
    let out = crate::bg_proc::command("ps")
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

/// macOS: `bt:<sec>.<usec>`, libproc's start time (no locale, no time
/// zone, microseconds). When libproc can't say, `ls:<lstart>`, the start
/// time as `ps -o lstart=` prints it: the only form before the libproc
/// switch, and still what other BSDs record.
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn proc_identity(pid: i32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if let Some(info) = crate::mac_proc::bsd_info(pid) {
            return Some(format!("bt:{}.{:06}", info.start_sec, info.start_usec));
        }
    }
    ps_lstart(pid).map(|s| format!("ls:{s}"))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn ps_lstart(pid: i32) -> Option<String> {
    let out = crate::bg_proc::command("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The live `pid` is the process a pid record's `identity` names.
///
/// macOS also matches an `ls:` record, written by a build before the
/// libproc switch or by the `ps` fallback: first by libproc's start time
/// printed as `ps` prints it (no `ps` run), then by `ps` itself. So a
/// daemon upgraded under a running tunnel still adopts its frpc.
pub(crate) fn identity_matches(pid: i32, recorded: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        if let Some(lstart) = recorded.strip_prefix("ls:") {
            let from_libproc =
                crate::mac_proc::bsd_info(pid).and_then(|i| crate::mac_proc::ps_lstart_text(i.start_sec));
            return from_libproc.as_deref() == Some(lstart) || ps_lstart(pid).as_deref() == Some(lstart);
        }
    }
    proc_identity(pid).as_deref() == Some(recorded)
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

/// macOS: `KERN_PROCARGS2` (our own uid only), with `ps` as the fallback
/// when the kernel won't answer. Another uid's process is never read: it
/// can't be our frpc.
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn proc_argv(pid: i32) -> Option<Vec<String>> {
    #[cfg(target_os = "macos")]
    {
        match crate::mac_proc::argv(pid) {
            Ok(argv) => return Some(argv),
            Err(crate::mac_proc::ArgvError::OtherUser) => return None,
            Err(crate::mac_proc::ArgvError::Unreadable) => {}
        }
    }
    // `ps -o command=` joins argv with spaces; frpc's argv is
    // `<frpc> -c <cfg>` and the config path carries no spaces in practice,
    // but compare on the joined string to stay honest. The uid column
    // keeps this fallback to our own processes too.
    let out = crate::bg_proc::command("ps")
        .args(["-ww", "-o", "uid=", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let (uid, command) = line.split_once(char::is_whitespace)?;
    // SAFETY: getuid has no failure mode.
    if uid.parse::<libc::uid_t>().ok()? != unsafe { libc::getuid() } {
        return None;
    }
    let command = command.trim();
    (!command.is_empty()).then(|| vec![command.to_string()])
}

/// Windows: the recorded command line split as Windows programs read it,
/// so a config under a profile folder with a space (`C:\Users\Ann Lee`)
/// still matches.
#[cfg(windows)]
pub(crate) fn proc_argv(pid: i32) -> Option<Vec<String>> {
    u32::try_from(pid)
        .ok()
        .and_then(crate::win_proc::command_line)
        .map(|s| crate::win_proc::split_command_line(&s))
}

/// Windows: kill every live frpc started as `<frpc> -c <cfg>`, what
/// `pkill -f "frpc -c <cfg>"` does on Unix (an frpc orphaned by a crashed
/// daemon keeps its proxy name on frps). Only frpc-named programs have
/// their command line read; none running costs a process snapshot.
#[cfg(windows)]
pub(crate) fn reap_ours(cfg: &Path) -> usize {
    let me = std::process::id();
    let pids: Vec<u32> = crate::win_proc::pids_named("frpc").into_iter().filter(|p| *p != me).collect();
    let mut reaped = 0;
    for (pid, line) in crate::win_proc::command_lines(&pids) {
        if argv_is_our_frpc(&crate::win_proc::split_command_line(&line), cfg) {
            kill_foreign(pid as i32);
            reaped += 1;
        }
    }
    reaped
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
        // The `ps` fallback form (not Linux or Windows): one joined string.
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

/// The pid record beside the rendered config (the private runtime dir):
/// `frpc.toml` → `frpc.pid`, the voice lane's `frpc-voice.toml` →
/// `frpc-voice.pid` (0.46.8), so the two frpcs never share a record.
pub(crate) fn pid_path(cfg: &Path) -> PathBuf {
    cfg.with_extension("pid")
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
    if !identity_matches(rec.pid, &rec.identity) {
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

/// Spawn `<frpc> -c <cfg>` writing stdout+stderr straight into the
/// append-mode `log` (no pipe: a pipe would kill frpc with `SIGPIPE` once
/// this daemon exits, runtime stage H), in its own session, and record its
/// pid file for the next daemon. Returns the child, its pid and the log
/// offset its run starts at. Shared by the main tunnel and the voice lane.
pub(crate) fn spawn_logged(frpc: &Path, cfg: &Path, log: &Path) -> Result<(Child, i32, u64), String> {
    use std::process::Stdio;
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| format!("open frpc log {}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| format!("clone log handle: {e}"))?;
    let offset = log_len(log);
    // frpc is a console program: bg_proc keeps it from opening a window.
    let mut cmd = crate::bg_proc::command(frpc);
    cmd.arg("-c").arg(cfg).stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    // Its own session: never in the daemon's process group or session, so
    // a signal to the daemon's group (or a terminal hangup) misses it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let child = cmd.spawn().map_err(|e| format!("spawn frpc ({}): {e}", frpc.display()))?;
    let pid = child.id() as i32;
    if let Some(identity) = proc_identity(pid) {
        write_pid_record(&PidRecord { pid, identity, cfg: cfg.to_path_buf(), log_offset: offset });
    }
    Ok((child, pid, offset))
}

/// Hand `on_line` the lines an adopted frpc already wrote this run (`log`
/// from `rec.log_offset` up to now); returns the offset to tail from.
pub(crate) fn replay_run(log: &Path, rec: &PidRecord, mut on_line: impl FnMut(&str)) -> u64 {
    let now = log_len(log);
    if let Ok(bytes) = std::fs::read(log) {
        let from = (rec.log_offset as usize).min(bytes.len());
        let to = (now as usize).min(bytes.len()).max(from);
        for line in String::from_utf8_lossy(&bytes[from..to]).lines() {
            on_line(line);
        }
    }
    now
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_voice_lane_has_its_own_pid_record_and_reap_pattern() {
        assert_eq!(pid_path(Path::new("/run/k2/frpc.toml")), PathBuf::from("/run/k2/frpc.pid"));
        assert_eq!(pid_path(Path::new("/run/k2/frpc-voice.toml")), PathBuf::from("/run/k2/frpc-voice.pid"));
        // Neither config path matches the other's argv, so neither frpc's
        // adopt or reap can take the other.
        let main = Path::new("/run/k2/frpc.toml");
        let voice = Path::new("/run/k2/frpc-voice.toml");
        let argv = |c: &Path| vec!["/usr/bin/frpc".to_string(), "-c".to_string(), c.display().to_string()];
        assert!(!argv_is_our_frpc(&argv(voice), main));
        assert!(!argv_is_our_frpc(&argv(main), voice));
        assert!(argv_is_our_frpc(&argv(voice), voice));
    }

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

    /// A live `<frpc> -c <cfg>` stand-in (a `#!/bin/sh` script named
    /// frpc, so argv is `/bin/sh <dir>/frpc -c <cfg>`) and its config.
    #[cfg(unix)]
    struct StandIn {
        dir: PathBuf,
        cfg: PathBuf,
        child: Child,
    }

    #[cfg(unix)]
    impl StandIn {
        const RENDERED: &'static str = "serverAddr = \"relay.example\"\n";

        fn start() -> StandIn {
            let dir = crate::test_env::unique_temp_path("frpc-adopt");
            std::fs::create_dir_all(&dir).expect("temp dir");
            let frpc = dir.join("frpc");
            crate::test_env::write_executable(&frpc, "#!/bin/sh\nwhile :; do sleep 1; done\n");
            let cfg = dir.join("frpc.toml");
            std::fs::write(&cfg, Self::RENDERED).expect("write cfg");
            // No inherited stdio: the loop's last `sleep 1` outlives a killed
            // shell and would hold the test's output open (a nextest leak).
            let child = std::process::Command::new(&frpc)
                .arg("-c")
                .arg(&cfg)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn stand-in");
            // Wait for the shell to be exec'd (argv names the script).
            let pid = child.id() as i32;
            let t = Instant::now();
            while !proc_argv(pid).is_some_and(|a| argv_is_our_frpc(&a, &cfg)) {
                assert!(t.elapsed() < Duration::from_secs(5), "stand-in argv: {:?}", proc_argv(pid));
                std::thread::sleep(Duration::from_millis(20));
            }
            StandIn { dir, cfg, child }
        }

        fn record(&self, identity: String) -> PidRecord {
            let rec = PidRecord { pid: self.child.id() as i32, identity, cfg: self.cfg.clone(), log_offset: 0 };
            write_pid_record(&rec);
            rec
        }
    }

    #[cfg(unix)]
    impl Drop for StandIn {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_live_frpc_with_this_builds_record_is_adopted() {
        let s = StandIn::start();
        let rec = s.record(proc_identity(s.child.id() as i32).expect("identity"));
        if cfg!(target_os = "macos") {
            assert!(rec.identity.starts_with("bt:"), "libproc identity: {}", rec.identity);
        }
        assert_eq!(try_adopt(&s.cfg, StandIn::RENDERED), Ok(rec.clone()));
        assert_eq!(try_adopt(&s.cfg, "other"), Err(NoAdopt::ConfigChanged));
        s.record(format!("{}0", rec.identity));
        assert_eq!(try_adopt(&s.cfg, StandIn::RENDERED), Err(NoAdopt::Recycled));
    }

    /// A pid record written before the libproc switch holds
    /// `ls:<ps -o lstart=>`. A daemon upgraded under a running tunnel must
    /// still adopt that frpc, with or without a working `ps`.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_live_frpc_with_an_old_ps_lstart_record_is_still_adopted() {
        let s = StandIn::start();
        let pid = s.child.id() as i32;
        let start = crate::mac_proc::bsd_info(pid).expect("bsd info").start_sec;
        // The old build's text: `ps` itself where it runs; under the compute
        // node's sandbox (no setuid exec) `/bin/date +%c`, the same strftime
        // call in the same locale.
        let lstart = |sec: u64| -> String {
            let out = std::process::Command::new("/bin/date").arg("-r").arg(sec.to_string()).arg("+%c").output().expect("date");
            assert!(out.status.success(), "date: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let old = match ps_lstart(pid) {
            Some(from_ps) => {
                assert_eq!(from_ps, lstart(start), "ps and strftime %c agree");
                from_ps
            }
            None => lstart(start),
        };
        let rec = s.record(format!("ls:{old}"));
        assert!(identity_matches(pid, &rec.identity));
        assert_eq!(try_adopt(&s.cfg, StandIn::RENDERED), Ok(rec));
        // Another start second is another process: never adopted.
        s.record(format!("ls:{}", lstart(start + 1)));
        assert_eq!(try_adopt(&s.cfg, StandIn::RENDERED), Err(NoAdopt::Recycled));
    }
}
