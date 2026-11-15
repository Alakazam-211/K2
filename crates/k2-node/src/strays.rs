//! macOS: processes a job left running outside its process group.
//!
//! At job end the node kills the job's process group (the holder does,
//! then [`crate::node`]'s reap). A process that `setsid`s out of that group
//! survives. A test's fake daemon (`frpc`) does exactly that, and on the
//! LAN minis 75 of them piled up on one node over ten days. macOS has no
//! cgroup to end them with, so after the group is gone the node finds
//! them by where they are: processes of the node's own uid whose working
//! directory, executable or any argv entry is under the ended job's
//! folder. It ends them: SIGTERM, a short grace, then SIGKILL. Each pid is
//! re-checked against the start time recorded when it was found, so a
//! reused pid is never hit. They go into the job's own log.
//!
//! Never touched: the node itself and its process group, other users'
//! processes (a tenant job's own user is the root executor's to stop), and
//! `sccache` (the slot's shared compile server, V63: other jobs use it).

use std::path::Path;
use std::time::Duration;

/// A process found under a job's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    pub pid: i32,
    /// [`crate::util::process_start_time`] when it was found.
    pub start: u64,
    /// Its command line, for the job's log (cut short).
    pub cmd: String,
}

/// Programs never ended here (shared beyond one job).
pub const SPARED: &[&str] = &["sccache"];

/// `root` and the path it resolves to (`/var` is `/private/var` on macOS,
/// and the kernel reports the resolved one), each with a trailing `/`.
pub fn roots(root: &Path) -> Vec<String> {
    let mut v = vec![root.to_path_buf()];
    if let Ok(c) = std::fs::canonicalize(root) {
        if c != root {
            v.push(c);
        }
    }
    v.into_iter()
        .map(|p| {
            let s = p.display().to_string();
            if s.ends_with('/') { s } else { format!("{s}/") }
        })
        .collect()
}

/// Is `path` the job folder or under it?
pub fn under(path: &str, roots: &[String]) -> bool {
    roots.iter().any(|r| path.starts_with(r.as_str()) || path == r.trim_end_matches('/'))
}

/// Does this process (its cwd, executable, argv) belong to the folder?
/// Pure, for tests: `argv` entries are checked whole, and so is any
/// `--flag=/path` value.
pub fn belongs(cwd: Option<&str>, exe: Option<&str>, argv: &[String], roots: &[String]) -> bool {
    if cwd.is_some_and(|c| under(c, roots)) || exe.is_some_and(|e| under(e, roots)) {
        return true;
    }
    argv.iter().any(|a| under(a, roots) || a.split_once('=').is_some_and(|(_, v)| under(v, roots)))
}

/// Is this program spared (its executable's or argv0's file name)?
pub fn spared(exe: Option<&str>, argv: &[String]) -> bool {
    let name = |s: &str| Path::new(s).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    exe.map(name).into_iter().chain(argv.first().map(|a| name(a))).any(|n| SPARED.contains(&n.as_str()))
}

/// `KERN_PROCARGS2` bytes → argv: `argc` (i32), the exec path, NUL
/// padding, then `argc` NUL-terminated strings (then the environment,
/// ignored).
pub fn parse_procargs2(b: &[u8]) -> Vec<String> {
    if b.len() < 4 {
        return Vec::new();
    }
    let argc = i32::from_ne_bytes([b[0], b[1], b[2], b[3]]).max(0) as usize;
    let mut i = 4;
    while i < b.len() && b[i] != 0 {
        i += 1;
    }
    while i < b.len() && b[i] == 0 {
        i += 1;
    }
    let mut out = Vec::with_capacity(argc);
    while out.len() < argc && i < b.len() {
        let end = b[i..].iter().position(|c| *c == 0).map(|p| i + p).unwrap_or(b.len());
        out.push(String::from_utf8_lossy(&b[i..end]).into_owned());
        i = end + 1;
    }
    out
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::CStr;

    pub fn all_pids() -> Vec<i32> {
        let mut pids: Vec<libc::c_int> = vec![0; 16384];
        let bytes = (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int;
        // SAFETY: a buffer of `bytes` bytes.
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut libc::c_void, bytes) };
        if n <= 0 {
            return Vec::new();
        }
        pids.truncate(n as usize);
        pids.retain(|p| *p > 1);
        pids
    }

    /// (uid, pgid) by `proc_pidinfo`.
    pub fn uid_pgid(pid: i32) -> Option<(u32, u32)> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: a zeroed out-struct of the size we pass.
        let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
        (n == size).then_some((info.pbi_uid, info.pbi_pgid))
    }

    pub fn cwd(pid: i32) -> Option<String> {
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        // SAFETY: a zeroed out-struct of the size we pass.
        let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
        if n != size {
            return None;
        }
        // SAFETY: vip_path is MAXPATHLEN bytes, NUL-terminated by the kernel.
        let p = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char) };
        Some(p.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
    }

    pub fn exe(pid: i32) -> Option<String> {
        let mut buf = vec![0u8; 4096];
        // SAFETY: a buffer of the size we pass.
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    pub fn procargs(pid: i32) -> Vec<u8> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        // SAFETY: a size query (null buffer).
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 || size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size];
        // SAFETY: a buffer of `size` bytes.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) } != 0 {
            return Vec::new();
        }
        buf.truncate(size);
        buf
    }
}

/// The node's own processes under `root` (never this process or its group).
#[cfg(target_os = "macos")]
pub fn find_under(root: &Path) -> Vec<Stray> {
    let roots = roots(root);
    // SAFETY: plain getters.
    let (me, my_group, uid) = unsafe { (libc::getpid(), libc::getpgrp(), libc::geteuid()) };
    let mut out = Vec::new();
    for pid in mac::all_pids() {
        if pid == me {
            continue;
        }
        match mac::uid_pgid(pid) {
            Some((u, g)) if u == uid && g as i32 != my_group => {}
            _ => continue,
        }
        let (cwd, exe) = (mac::cwd(pid), mac::exe(pid));
        let argv = parse_procargs2(&mac::procargs(pid));
        if spared(exe.as_deref(), &argv) || !belongs(cwd.as_deref(), exe.as_deref(), &argv, &roots) {
            continue;
        }
        let Some(start) = crate::util::process_start_time(pid as u32) else { continue };
        let mut cmd = if argv.is_empty() { exe.unwrap_or_default() } else { argv.join(" ") };
        if cmd.len() > 200 {
            let mut cut = 200;
            while !cmd.is_char_boundary(cut) {
                cut -= 1;
            }
            cmd.truncate(cut);
            cmd.push('…');
        }
        out.push(Stray { pid, start, cmd });
    }
    out
}

#[cfg(not(target_os = "macos"))]
pub fn find_under(_root: &Path) -> Vec<Stray> {
    Vec::new()
}

fn still(s: &Stray) -> bool {
    crate::util::process_start_time(s.pid as u32) == Some(s.start)
}

/// SIGTERM each (if still the same process), wait up to `grace` for them
/// to go, SIGKILL what's left. Returns what was ended, for the job's log.
pub fn end(strays: &[Stray], grace: Duration) -> Vec<String> {
    let mut ended = Vec::new();
    for s in strays.iter().filter(|s| still(s)) {
        // SAFETY: a pid re-checked by its start time just above.
        unsafe { libc::kill(s.pid, libc::SIGTERM) };
        ended.push(s);
    }
    let deadline = std::time::Instant::now() + grace;
    while ended.iter().any(|s| still(s)) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut lines = Vec::new();
    for s in ended {
        let how = if still(s) {
            // SAFETY: re-checked by its start time just above.
            unsafe { libc::kill(s.pid, libc::SIGKILL) };
            "KILL"
        } else {
            "TERM"
        };
        lines.push(format!("pid {} ({how}): {}", s.pid, s.cmd));
    }
    lines
}

/// Find and end what the job left under `root`: the lines for its log.
pub fn reap_under(root: &Path, grace: Duration) -> Vec<String> {
    let found = find_under(root);
    if found.is_empty() {
        return Vec::new();
    }
    end(&found, grace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_belongs_by_cwd_exe_argv_or_flag_value_and_never_by_prefix_alone() {
        let r = roots(Path::new("/var/k2node/jobs/abc-g1"));
        assert!(under("/var/k2node/jobs/abc-g1/tmp/x/frpc", &r));
        assert!(under("/var/k2node/jobs/abc-g1", &r));
        assert!(!under("/var/k2node/jobs/abc-g10/tmp", &r), "a sibling folder with the same prefix");
        let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(belongs(None, Some("/bin/sh"), &argv(&["/bin/sh", "/var/k2node/jobs/abc-g1/tmp/k2-adopt-frpc-1/frpc", "-c", "x"]), &r));
        assert!(belongs(Some("/var/k2node/jobs/abc-g1/src"), Some("/bin/sleep"), &argv(&["sleep", "1"]), &r));
        assert!(belongs(None, None, &argv(&["frpc", "--config=/var/k2node/jobs/abc-g1/f.toml"]), &r));
        assert!(!belongs(Some("/var/k2node"), Some("/bin/sleep"), &argv(&["sleep", "1"]), &r));
        assert!(spared(Some("/var/k2node/toolchains/bin/sccache"), &argv(&["sccache"])));
        assert!(!spared(Some("/bin/sh"), &argv(&["/bin/sh"])));
    }

    #[test]
    fn procargs2_is_argc_exec_path_padding_then_argv() {
        let mut b = 3i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/bin/sh\0\0\0\0/bin/sh\0/var/k2node/jobs/j-g1/tmp/frpc\0-c\0PATH=/usr/bin\0");
        assert_eq!(parse_procargs2(&b), vec!["/bin/sh", "/var/k2node/jobs/j-g1/tmp/frpc", "-c"]);
        assert!(parse_procargs2(&[1, 0]).is_empty());
    }

    /// The real thing: a `setsid` stray with its cwd in the job folder, one
    /// named by argv that ignores SIGTERM, and a process elsewhere that
    /// must live on.
    #[cfg(target_os = "macos")]
    #[test]
    fn strays_under_the_job_folder_end_and_others_live() {
        use std::os::unix::process::CommandExt;
        let dir = crate::util::temp_dir("strays");
        let root = dir.join("job");
        std::fs::create_dir_all(root.join("tmp/k2-fake")).unwrap();
        let marker = root.join("tmp/k2-fake/frpc");
        std::fs::write(&marker, "").unwrap();
        let spawn = |cwd: &Path, args: &[&str]| {
            let mut c = std::process::Command::new("/bin/sh");
            c.args(args).current_dir(cwd);
            // SAFETY: setsid only, like the tests' fake daemons.
            unsafe {
                c.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
            c.spawn().unwrap()
        };
        let mut by_cwd = spawn(&root, &["-c", "while :; do sleep 1; done"]);
        let mut by_argv = spawn(&dir, &["-c", "trap '' TERM; while :; do sleep 1; done", marker.to_str().unwrap()]);
        let mut other = spawn(&dir, &["-c", "while :; do sleep 1; done"]);
        std::thread::sleep(Duration::from_millis(200));
        let found: Vec<i32> = find_under(&root).iter().map(|s| s.pid).collect();
        assert!(found.contains(&(by_cwd.id() as i32)), "by cwd: {found:?}");
        assert!(found.contains(&(by_argv.id() as i32)), "by argv: {found:?}");
        assert!(!found.contains(&(other.id() as i32)), "elsewhere: {found:?}");
        let lines = reap_under(&root, Duration::from_millis(500));
        assert!(lines.iter().any(|l| l.starts_with(&format!("pid {} (TERM)", by_cwd.id()))), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with(&format!("pid {} (KILL)", by_argv.id()))), "SIGTERM ignored: {lines:?}");
        assert!(by_cwd.wait().is_ok() && by_argv.wait().is_ok());
        assert!(other.try_wait().unwrap().is_none(), "a process elsewhere lives on");
        let _ = other.kill();
        let _ = other.wait();
        assert!(find_under(&root).is_empty(), "nothing left");
    }

    #[test]
    fn a_reused_pid_is_never_signalled() {
        // A start time that can't match: the pid (this test process) is skipped.
        let me = Stray { pid: std::process::id() as i32, start: 1, cmd: "me".into() };
        assert!(end(&[me], Duration::from_millis(10)).is_empty());
    }
}
