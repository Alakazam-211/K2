//! Processes a test's stub programs start, ended when the test ends.
//!
//! A fake `frpc` runs in its own session (`setsid`, as the connector starts
//! the real one), so it is outside the test's process group: when the test
//! process exits, or a test runner kills its group, the stub lives on. The
//! tunnel tests left 85 of them on the Mac compute minis in ten days.
//!
//! [`StubProcs`] is the drop guard for that. Each stub records its own pid
//! first thing ([`StubProcs::record_line`]), and the test may record pids
//! it learns itself ([`StubProcs::record`]). On drop, pass or fail
//! (including during a panic), every recorded pid that is still alive gets
//! SIGTERM (its process group too when it leads one, so a stub's `sleep`
//! goes with it), a short wait, then SIGKILL. Only recorded pids are ever
//! signalled, never a pattern match, and only if the process started after
//! the guard did (a pid reused by an unrelated process is left alone).
//!
//! Then the check: no process of ours whose argv or working directory is
//! under one of the guard's watched folders may still be running. If one
//! is, the test fails, naming it. That check finds processes by folder,
//! but it only reports them; it never signals them.
//!
//! Listing processes: Linux reads `/proc`; macOS asks libproc (works under
//! the compute node's Seatbelt sandbox), then `ps -axo pid=,command=`.
//! When neither can list processes, the check falls back to the recorded
//! pids (`kill -0`) and says so on stderr.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// How long ended processes get to go away, and how long the final check
/// waits for an unrecorded straggler (a `/bin/true` mid-exit) to finish.
/// Kept short on purpose: a stub some in-process supervisor would kill "on
/// its next poll" (1 s) still outlives a test runner that exits the test
/// process right away, which is how the fake frpcs leaked.
const TERM_WAIT: Duration = Duration::from_secs(2);
const KILL_WAIT: Duration = Duration::from_secs(2);
const CHECK_GRACE: Duration = Duration::from_millis(300);

/// The drop guard: see the module docs.
#[must_use = "the stubs are ended when the guard drops"]
pub struct StubProcs {
    /// Folder of the pid file (removed on drop).
    dir: PathBuf,
    /// Stubs append `$$` here, one per line.
    pid_file: PathBuf,
    /// Pids the test recorded itself.
    recorded: Mutex<Vec<i32>>,
    /// Folders whose processes must all be gone when the test ends.
    roots: Mutex<Vec<PathBuf>>,
    /// Nothing started before this is ever signalled.
    created: SystemTime,
}

impl Default for StubProcs {
    fn default() -> Self {
        Self::new()
    }
}

impl StubProcs {
    pub fn new() -> Self {
        let dir = super::unique_temp_path("stub-pids");
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        let pid_file = dir.join("pids");
        std::fs::write(&pid_file, "").unwrap_or_else(|e| panic!("create {}: {e}", pid_file.display()));
        let me = StubProcs {
            dir: dir.clone(),
            pid_file,
            recorded: Mutex::new(Vec::new()),
            roots: Mutex::new(Vec::new()),
            created: SystemTime::now() - Duration::from_secs(3),
        };
        me.watch(&dir);
        me
    }

    /// The file a stub appends its pid to.
    pub fn pid_file(&self) -> &Path {
        &self.pid_file
    }

    /// One `sh` line that appends the running shell's pid to the pid file.
    /// A stub puts it first, right after its `#!` line; a stub that `exec`s
    /// keeps that pid.
    pub fn record_line(&self) -> String {
        #[cfg(windows)]
        {
            // Git Bash: `$$` is an MSYS pid; the Windows pid is under /proc.
            format!("cat /proc/$$/winpid >> '{}'\n", self.pid_file.display())
        }
        #[cfg(not(windows))]
        {
            format!("echo $$ >> '{}'\n", self.pid_file.display())
        }
    }

    /// `script` (which starts with a `#!` line) with [`Self::record_line`]
    /// right after that line.
    pub fn with_record_line(&self, script: &str) -> String {
        assert!(script.starts_with("#!"), "a stub script starts with #!:\n{script}");
        let (shebang, rest) = script.split_once('\n').unwrap_or((script, ""));
        format!("{shebang}\n{}{rest}", self.record_line())
    }

    /// Record a pid the test started (or learned) itself.
    pub fn record(&self, pid: i32) {
        self.recorded.lock().unwrap_or_else(|p| p.into_inner()).push(pid);
    }

    /// Watch `root`: when the test ends, no process whose argv or cwd is
    /// under it may still run.
    pub fn watch(&self, root: impl AsRef<Path>) {
        let root = root.as_ref().to_path_buf();
        let mut roots = self.roots.lock().unwrap_or_else(|p| p.into_inner());
        if let Ok(c) = std::fs::canonicalize(&root) {
            if c != root && !roots.contains(&c) {
                roots.push(c);
            }
        }
        if !roots.contains(&root) {
            roots.push(root);
        }
    }

    /// Every recorded pid: the test's own, then the stubs' (deduplicated).
    pub fn pids(&self) -> Vec<i32> {
        let mut out = self.recorded.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Ok(text) = std::fs::read_to_string(&self.pid_file) {
            out.extend(text.lines().filter_map(|l| l.trim().parse::<i32>().ok()));
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|p| *p > 0 && *p != std::process::id() as i32 && seen.insert(*p));
        out
    }

    /// End every recorded pid still alive: TERM, wait, KILL. A recorded
    /// stub that already died but led a process group (a `setsid` stub
    /// whose `sleep` outlived it) has that group ended too. Repeats while
    /// new pids appear (a supervisor that respawned during the wait).
    pub fn end_all(&self) {
        let mut done = std::collections::HashSet::new();
        for _round in 0..5 {
            let (mut live, mut remnants) = (Vec::new(), Vec::new());
            for p in self.pids().into_iter().filter(|p| !done.contains(p)) {
                if alive(p) {
                    if started_after(p, self.created) {
                        live.push(p);
                    }
                } else if group_exists(p) {
                    // While a group with this id exists, the pid can't be
                    // reused: its members are the dead stub's children.
                    remnants.push(p);
                }
            }
            if live.is_empty() && remnants.is_empty() {
                return;
            }
            for &p in live.iter().chain(&remnants) {
                signal(p, Sig::Term);
            }
            wait_gone(&live, TERM_WAIT);
            for &p in live.iter().chain(&remnants) {
                signal(p, Sig::Kill);
            }
            wait_gone(&live, KILL_WAIT);
            done.extend(live);
            done.extend(remnants);
        }
    }

    /// Processes of ours whose argv or cwd is under a watched folder
    /// (`pid: command`), waiting up to [`CHECK_GRACE`] for them to go.
    pub fn survivors(&self) -> Vec<String> {
        let roots: Vec<String> = self
            .roots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|r| r.display().to_string())
            .collect();
        let deadline = Instant::now() + CHECK_GRACE;
        loop {
            let found = match list_processes() {
                Some(procs) => procs
                    .into_iter()
                    .filter(|p| p.pid != std::process::id() as i32 && p.belongs(&roots))
                    .map(|p| format!("pid {}: {} (cwd {})", p.pid, p.argv.join(" "), p.cwd.as_deref().unwrap_or("?")))
                    .collect::<Vec<_>>(),
                None => {
                    eprintln!(
                        "[stub-procs] can't list processes here; checking only the {} recorded pid(s) with kill -0",
                        self.pids().len()
                    );
                    self.pids()
                        .into_iter()
                        .filter(|p| alive(*p) && started_after(*p, self.created))
                        .map(|p| format!("pid {p}: recorded stub (process list unavailable)"))
                        .collect()
                }
            };
            if found.is_empty() || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for StubProcs {
    fn drop(&mut self) {
        self.end_all();
        let left = self.survivors();
        let _ = std::fs::remove_dir_all(&self.dir);
        if left.is_empty() {
            return;
        }
        let roots = self.roots.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let msg = format!(
            "{} process(es) under this test's folders {roots:?} are still running after the test ended \
             (a stub that never recorded its pid, or a leak):\n  {}",
            left.len(),
            left.join("\n  ")
        );
        if std::thread::panicking() {
            eprintln!("[stub-procs] {msg}");
        } else {
            panic!("{msg}");
        }
    }
}

/// One listed process.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
struct Proc {
    pid: i32,
    argv: Vec<String>,
    cwd: Option<String>,
}

impl Proc {
    fn belongs(&self, roots: &[String]) -> bool {
        let under = |s: &str| {
            roots.iter().any(|r| s == r || s.contains(&format!("{}/", r.trim_end_matches('/'))))
        };
        self.cwd.as_deref().is_some_and(under) || self.argv.iter().any(|a| under(a))
    }
}

/// Every live (not zombie) process of our uid, or `None` when this
/// platform or sandbox won't list them.
#[cfg(target_os = "linux")]
fn list_processes() -> Option<Vec<Proc>> {
    let rd = std::fs::read_dir("/proc").ok()?;
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let Some(pid) = ent.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        if !alive(pid) {
            continue;
        }
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let argv = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| p.display().to_string());
        out.push(Proc { pid, argv, cwd });
    }
    // An empty list means /proc is hidden from us (we are running, at least).
    (!out.is_empty()).then_some(out)
}

#[cfg(target_os = "macos")]
fn list_processes() -> Option<Vec<Proc>> {
    if let Some(pids) = crate::mac_proc::all_pids() {
        // SAFETY: getuid has no failure mode.
        let me = unsafe { libc::getuid() };
        let mut out = Vec::new();
        for pid in pids {
            let Some(info) = crate::mac_proc::bsd_info(pid) else { continue };
            if info.uid != me || info.zombie {
                continue;
            }
            let argv = crate::mac_proc::argv(pid).unwrap_or_default();
            out.push(Proc { pid, argv, cwd: mac_cwd(pid) });
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    // libproc said nothing: `ps` (denied under a sandbox: then None).
    let o = crate::bg_proc::command("/bin/ps").args(["-axww", "-o", "pid=,command="]).output().ok()?;
    if !o.status.success() {
        return None;
    }
    let out: Vec<Proc> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| {
            let (pid, cmd) = l.trim_start().split_once(char::is_whitespace)?;
            Some(Proc { pid: pid.parse().ok()?, argv: vec![cmd.trim().to_string()], cwd: None })
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn list_processes() -> Option<Vec<Proc>> {
    None
}

/// macOS: the working directory, from libproc.
#[cfg(target_os = "macos")]
fn mac_cwd(pid: i32) -> Option<String> {
    // SAFETY: plain old data; all zeroes is a valid value.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: the buffer is a proc_vnodepathinfo of exactly `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size)
    };
    if n != size {
        return None;
    }
    // SAFETY: vip_path is MAXPATHLEN bytes, NUL-terminated by the kernel.
    let p = unsafe { std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char) };
    Some(p.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
}

/// The pid runs and is not a zombie.
#[cfg(target_os = "linux")]
fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks existence.
    if unsafe { libc::kill(pid, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
        return false;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !matches!(
            stat.rfind(')').and_then(|i| stat[i + 1..].split_whitespace().next()),
            Some("Z") | Some("X") | None
        ),
        // Hidden from /proc (another user's): it exists.
        Err(_) => true,
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks existence.
    if pid <= 0 || unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "macos")]
    if let Some(info) = crate::mac_proc::bsd_info(pid) {
        return !info.zombie;
    }
    true
}

#[cfg(windows)]
fn alive(pid: i32) -> bool {
    pid > 0 && crate::win_proc::pid_alive(pid as u32)
}

#[cfg(not(any(unix, windows)))]
fn alive(_pid: i32) -> bool {
    false
}

/// The process started at or after `t` (unknown start: assume it did, the
/// signal can only reach a process of ours anyway).
fn started_after(pid: i32, t: SystemTime) -> bool {
    match start_time(pid) {
        Some(s) => s >= t,
        None => true,
    }
}

#[cfg(target_os = "linux")]
fn start_time(pid: i32) -> Option<SystemTime> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let ticks: u64 = stat[stat.rfind(')')? + 1..].split_whitespace().nth(19)?.parse().ok()?;
    let btime: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 {
        return None;
    }
    let ms = btime * 1000 + ticks * 1000 / hz as u64;
    Some(SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
}

#[cfg(target_os = "macos")]
fn start_time(pid: i32) -> Option<SystemTime> {
    let info = crate::mac_proc::bsd_info(pid)?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(info.start_sec) + Duration::from_micros(info.start_usec))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn start_time(_pid: i32) -> Option<SystemTime> {
    None
}

enum Sig {
    Term,
    Kill,
}

/// Signal the pid (if it still runs), and the process group with its id
/// (a `setsid` stub's `sleep` is in it) if there is one.
#[cfg(unix)]
fn signal(pid: i32, sig: Sig) {
    let s = match sig {
        Sig::Term => libc::SIGTERM,
        Sig::Kill => libc::SIGKILL,
    };
    if pid <= 0 {
        return;
    }
    // SAFETY: plain signal sends to a positive pid / the group it leads.
    unsafe {
        if group_exists(pid) {
            libc::kill(-pid, s);
        }
        if alive(pid) {
            libc::kill(pid, s);
        }
    }
}

#[cfg(windows)]
fn signal(pid: i32, _sig: Sig) {
    if alive(pid) {
        crate::win_proc::terminate(pid as u32);
    }
}

#[cfg(not(any(unix, windows)))]
fn signal(_pid: i32, _sig: Sig) {}

/// A process group with id `pid` exists and is ours to signal.
#[cfg(unix)]
fn group_exists(pid: i32) -> bool {
    // SAFETY: signal 0 to a group only checks it.
    pid > 0 && unsafe { libc::kill(-pid, 0) } == 0
}

#[cfg(not(unix))]
fn group_exists(_pid: i32) -> bool {
    false
}

fn wait_gone(pids: &[i32], max: Duration) {
    let deadline = Instant::now() + max;
    while pids.iter().any(|p| alive(*p)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A `setsid` stub (as the connector starts frpc) that records its pid
    /// and ignores SIGTERM is still ended by the guard, its `sleep` too,
    /// and nothing under the folder is left.
    #[test]
    fn the_guard_ends_a_detached_stub_that_ignores_term() {
        let procs = StubProcs::new();
        let dir = super::super::unique_temp_path("stub-procs-selftest");
        std::fs::create_dir_all(&dir).expect("mk dir");
        procs.watch(&dir);
        let script = dir.join("frpc");
        super::super::write_executable(
            &script,
            &procs.with_record_line("#!/bin/sh\ntrap '' TERM\nwhile :; do sleep 1; done\n"),
        );
        let mut cmd = crate::bg_proc::command(&script);
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
        let mut child = cmd.spawn().expect("spawn stub");
        let pid = child.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !procs.pids().contains(&pid) {
            assert!(Instant::now() < deadline, "the stub never recorded its pid");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The check sees it before the guard ends it.
        let seen = procs.survivors_now();
        assert!(seen.iter().any(|s| s.starts_with(&format!("pid {pid}:"))), "{seen:?}");
        procs.end_all();
        let status = child.wait().expect("wait the stub");
        assert!(!status.success(), "ended by a signal: {status:?}");
        assert!(procs.survivors().is_empty(), "{:?}", procs.survivors());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(test, unix))]
impl StubProcs {
    /// [`Self::survivors`] without the grace wait.
    fn survivors_now(&self) -> Vec<String> {
        let roots: Vec<String> =
            self.roots.lock().unwrap().iter().map(|r| r.display().to_string()).collect();
        list_processes()
            .expect("this test needs a process list")
            .into_iter()
            .filter(|p| p.belongs(&roots))
            .map(|p| format!("pid {}: {}", p.pid, p.argv.join(" ")))
            .collect()
    }
}
