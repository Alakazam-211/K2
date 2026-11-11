//! macOS process facts from libproc and sysctl, not `ps`: is a pid a
//! zombie, its parent, its owner, when it started, and its argv.
//!
//! `/bin/ps` is setuid root, and a process under a Seatbelt sandbox (the
//! macOS compute node runs jobs under `sandbox-exec`) can't exec a setuid
//! binary, so every `ps` call there fails. These calls work in and out of
//! a sandbox. Callers keep `ps` as the fallback for when one of them fails.
//!
//! The argv read is for processes of our own uid only, and it never
//! decodes the environment strings that follow argv in the kernel's reply.

/// What `proc_pidinfo(PROC_PIDTBSDINFO)` says about a pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BsdInfo {
    pub ppid: i32,
    /// The effective uid.
    pub uid: u32,
    /// Exited but not yet reaped by its parent.
    pub zombie: bool,
    /// Start time since the epoch: seconds, then microseconds.
    pub start_sec: u64,
    pub start_usec: u64,
}

/// The pid's BSD info, or `None` when there is no such process or the
/// kernel won't tell us (another user's process, unless we are root).
/// A zombie is still found: a nonzero arg asks the kernel to look among
/// zombies too (with 0 a zombie reads as no such process).
#[cfg(target_os = "macos")]
pub fn bsd_info(pid: i32) -> Option<BsdInfo> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: proc_bsdinfo is plain old data; all zeroes is a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is a proc_bsdinfo of exactly `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            1,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if n != size || info.pbi_pid != pid as u32 {
        return None;
    }
    Some(BsdInfo {
        ppid: info.pbi_ppid as i32,
        uid: info.pbi_uid,
        zombie: info.pbi_status == libc::SZOMB,
        start_sec: info.pbi_start_tvsec,
        start_usec: info.pbi_start_tvusec,
    })
}

/// Why [`argv`] gave no argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgvError {
    /// The process belongs to another uid: not read, by rule.
    OtherUser,
    /// No such process, or the kernel refused or sent something unreadable.
    Unreadable,
}

/// The argv of a process of our own uid, from `sysctl(KERN_PROCARGS2)`.
#[cfg(target_os = "macos")]
pub fn argv(pid: i32) -> Result<Vec<String>, ArgvError> {
    let info = bsd_info(pid).ok_or(ArgvError::Unreadable)?;
    // SAFETY: getuid has no failure mode.
    if info.uid != unsafe { libc::getuid() } {
        return Err(ArgvError::OtherUser);
    }
    let buf = procargs2(pid).ok_or(ArgvError::Unreadable)?;
    parse_procargs2_argv(&buf).ok_or(ArgvError::Unreadable)
}

/// The raw `KERN_PROCARGS2` reply for `pid` (argv and env together).
#[cfg(target_os = "macos")]
fn procargs2(pid: i32) -> Option<Vec<u8>> {
    let mut argmax: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `argmax` is a c_int and `len` holds its size.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            (&mut argmax as *mut libc::c_int).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut len = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: `buf` is `len` writable bytes; the kernel writes at most that
    // and sets `len` to what it wrote.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(len);
    Some(buf)
}

/// The argv out of a `KERN_PROCARGS2` reply. The layout: a native-endian
/// i32 argc, the exec path and its NUL, NUL padding, then argc
/// NUL-terminated argv strings, then the environment strings. Stops after
/// argv: the environment is never decoded. `None` when the reply is short.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_procargs2_argv(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let argc = usize::try_from(argc).ok()?;
    let rest = &buf[4..];
    let exec_end = rest.iter().position(|b| *b == 0)?;
    let rest = &rest[exec_end..];
    if argc == 0 {
        return Some(Vec::new());
    }
    let mut rest = &rest[rest.iter().position(|b| *b != 0)?..];
    let mut argv = Vec::with_capacity(argc);
    for _ in 0..argc {
        let end = rest.iter().position(|b| *b == 0)?;
        argv.push(String::from_utf8_lossy(&rest[..end]).into_owned());
        rest = &rest[end + 1..];
    }
    Some(argv)
}

/// A start time as `ps -o lstart=` prints it: `strftime("%c")` of the
/// local time, in the locale the environment names (`ps` calls
/// `setlocale(LC_ALL, "")`), trimmed. Pid records written before the
/// libproc switch hold this text, so a newer daemon can still match them
/// without running `ps`.
#[cfg(target_os = "macos")]
pub fn ps_lstart_text(start_sec: u64) -> Option<String> {
    extern "C" {
        fn tzset();
    }
    let t = libc::time_t::try_from(start_sec).ok()?;
    // SAFETY: tm is plain old data; all zeroes is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: tzset reads TZ; localtime_r writes only into `tm`.
    if unsafe {
        tzset();
        libc::localtime_r(&t, &mut tm)
    }
    .is_null()
    {
        return None;
    }
    let mut buf = [0u8; 256];
    // SAFETY: an empty name asks for the locale the environment names; a
    // null result (a bad name) leaves the C locale, as `ps` would.
    let loc = unsafe { libc::newlocale(libc::LC_ALL_MASK, c"".as_ptr(), std::ptr::null_mut()) };
    // SAFETY: `buf` holds `buf.len()` bytes; strftime writes at most that,
    // NUL included, and returns 0 when it would not fit.
    let n = unsafe {
        let out = buf.as_mut_ptr().cast();
        if loc.is_null() {
            libc::strftime(out, buf.len(), c"%c".as_ptr(), &tm)
        } else {
            let n = libc::strftime_l(out, buf.len(), c"%c".as_ptr(), &tm, loc);
            libc::freelocale(loc);
            n
        }
    };
    if n == 0 {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..n]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply in the kernel's layout: argc, exec path, padding, argv, env.
    fn reply(exec: &str, pad: usize, argv: &[&str], env: &[&str]) -> Vec<u8> {
        let mut b = (argv.len() as i32).to_ne_bytes().to_vec();
        b.extend_from_slice(exec.as_bytes());
        b.push(0);
        b.extend(std::iter::repeat_n(0u8, pad));
        for s in argv.iter().chain(env) {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b
    }

    #[test]
    fn procargs2_gives_argv_and_stops_before_env() {
        let b = reply("/opt/frpc", 7, &["/opt/frpc", "-c", "/run/k2/frpc.toml"], &["SECRET=x", "HOME=/h"]);
        assert_eq!(
            parse_procargs2_argv(&b),
            Some(vec!["/opt/frpc".to_string(), "-c".to_string(), "/run/k2/frpc.toml".to_string()])
        );
        // No padding at all also parses.
        let b = reply("/bin/sh", 0, &["/bin/sh", "/x/frpc"], &[]);
        assert_eq!(parse_procargs2_argv(&b), Some(vec!["/bin/sh".to_string(), "/x/frpc".to_string()]));
        // An argument with a space stays one argument.
        let b = reply("/f", 3, &["/f", "-c", "/Users/Ann Lee/frpc.toml"], &[]);
        assert_eq!(parse_procargs2_argv(&b).expect("argv")[2], "/Users/Ann Lee/frpc.toml");
        assert_eq!(parse_procargs2_argv(&reply("/f", 2, &[], &["A=b"])), Some(vec![]));
    }

    #[test]
    fn short_procargs2_replies_are_refused() {
        let full = reply("/opt/frpc", 4, &["/opt/frpc", "-c", "/run/k2/frpc.toml"], &[]);
        // Cut inside the last argv string: its NUL is gone.
        assert_eq!(parse_procargs2_argv(&full[..full.len() - 3]), None);
        assert_eq!(parse_procargs2_argv(&full[..3]), None, "no whole argc");
        assert_eq!(parse_procargs2_argv(&(-1i32).to_ne_bytes()), None, "negative argc");
        assert_eq!(parse_procargs2_argv(&reply("/f", 0, &[], &[])[..4]), None, "no exec path");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bsd_info_follows_a_real_child() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().expect("spawn sleep");
        let pid = child.id() as i32;
        let info = bsd_info(pid).expect("bsd info of our child");
        assert_eq!(info.ppid, std::process::id() as i32);
        // SAFETY: getuid has no failure mode.
        assert_eq!(info.uid, unsafe { libc::getuid() });
        assert!(!info.zombie);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_secs();
        assert!(info.start_sec <= now && now - info.start_sec < 60, "start {} now {now}", info.start_sec);
        assert_eq!(bsd_info(pid), Some(info), "stable");
        assert_eq!(argv(pid), Ok(vec!["sleep".to_string(), "30".to_string()]));
        child.kill().expect("kill");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let z = bsd_info(pid).expect("a zombie is still found");
        assert!(z.zombie, "{z:?}");
        assert_eq!(z.start_sec, info.start_sec);
        child.wait().expect("reap");
        assert_eq!(bsd_info(pid), None, "reaped");
        assert_eq!(argv(pid), Err(ArgvError::Unreadable));
        assert_eq!(bsd_info(-1), None);
    }

    /// `ps -o lstart=` and this text agree: `/bin/date -r <sec> +%c` is
    /// the same strftime call in the same locale (date is not setuid, so
    /// it runs under the compute node's sandbox where `ps` can't).
    #[cfg(target_os = "macos")]
    #[test]
    fn lstart_text_matches_date_c_format() {
        for sec in [0u64, 1_794_162_243, 1_794_374_867, 1_767_225_600] {
            let out = std::process::Command::new("/bin/date").arg("-r").arg(sec.to_string()).arg("+%c").output().expect("date");
            assert!(out.status.success(), "date: {out:?}");
            let want = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert_eq!(ps_lstart_text(sec).as_deref(), Some(want.as_str()), "sec {sec}");
        }
    }
}
