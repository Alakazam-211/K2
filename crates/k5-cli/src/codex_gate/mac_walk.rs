//! The walk of the processes above a Codex call on macOS, done here with
//! libproc and sysctl instead of in `python3`.
//!
//! It is the macOS branch of the shell's walk ([`super::ANCESTRY_PY`])
//! step for step: `proc_pidinfo(PROC_PIDTBSDINFO)` for the parent and the
//! short name, `sysctl(KERN_PROCARGS2)` for the argv, the short name alone
//! for a process whose argv the kernel won't give unless it could be a
//! Codex server, and the same limits (64 rows, no pid twice). It gives the
//! same rows, so the same answer, and it needs no `python3`: a Codex tab
//! on a Mac where `ps` can't run (a sandbox) and there is no `python3` is
//! still confirmed.
//!
//! Only the parent pid, the short name and argv are read; the environment
//! strings that follow argv in the kernel's reply are never decoded.

/// The walk's exit codes, as the Python's: every way was refused, or an
/// answer was missing, partial or unreadable.
pub const DENIED: i32 = 2;
pub const BROKEN: i32 = 3;

/// The rows `pid ppid command`, one a line with no newline after the
/// last, from `start` up to pid 1 (what the Python prints, as `$(...)`
/// keeps it), or the Python's exit code when it would fail.
pub fn rows(start: u32) -> Result<String, i32> {
    let argmax = arg_max()?;
    let mut rows: Vec<(u32, u32, String)> = Vec::new();
    let mut pid = start;
    while pid > 1 {
        if rows.len() >= 64 || rows.iter().any(|r| r.0 == pid) {
            return Err(BROKEN);
        }
        let (ppid, cmd) = info(pid, argmax)?;
        rows.push((pid, ppid, cmd.replace('\n', " ")));
        pid = ppid;
    }
    Ok(rows
        .iter()
        .map(|(pid, ppid, cmd)| format!("{pid} {ppid} {cmd}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The Python's `OSError` cases: a refusal is [`DENIED`], anything else
/// (no errno at all reads as EIO) is [`BROKEN`].
fn failed() -> i32 {
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EPERM | libc::EACCES) => DENIED,
        _ => BROKEN,
    }
}

/// errno is set to 0 before each call, so a stale one is never read as
/// this call's (the Python's `ctypes.get_errno() or errno.EIO`).
fn clear_errno() {
    // SAFETY: __error returns this thread's errno slot.
    unsafe { *libc::__error() = 0 };
}

/// `kern.argmax`: the size of a `KERN_PROCARGS2` reply. The Python reads
/// it before its `try:`, so a failure here is its uncaught error: exit 1,
/// [`BROKEN`] whatever the errno.
fn arg_max() -> Result<usize, i32> {
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
    if rc != 0 {
        return Err(BROKEN);
    }
    usize::try_from(argmax).map_err(|_| BROKEN)
}

/// One process: its parent and its command line.
fn info(pid: u32, argmax: usize) -> Result<(u32, String), i32> {
    let pid_c = libc::c_int::try_from(pid).map_err(|_| BROKEN)?;
    // SAFETY: proc_bsdinfo is plain old data; all zeroes is a valid value.
    let mut bsd: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    clear_errno();
    // SAFETY: the buffer is a proc_bsdinfo of exactly `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid_c,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut bsd as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if n != size {
        return Err(failed());
    }
    let ppid = bsd.pbi_ppid;
    let comm: Vec<u8> = bsd
        .pbi_comm
        .iter()
        .map(|c| *c as u8)
        .take_while(|b| *b != 0)
        .collect();
    let comm = String::from_utf8_lossy(&comm).into_owned();
    let mut buf = vec![0u8; argmax];
    let mut len = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid_c];
    clear_errno();
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
        // A process of another user: its name is enough to rule it out
        // unless it could be a Codex server (codex, or a script runner).
        let why = failed();
        let low = comm.to_lowercase();
        let runner = matches!(low.as_str(), "node" | "bun" | "deno");
        if comm.is_empty() || low.contains("codex") || runner {
            return Err(why);
        }
        return Ok((ppid, comm));
    }
    buf.truncate(len);
    Ok((ppid, argv_line(&buf).ok_or(BROKEN)?))
}

/// The argv of a `KERN_PROCARGS2` reply joined by blanks, as the Python
/// reads it: an i32 argc, the exec path to its first NUL, the NULs after
/// it, then argc NUL-separated words. `None` (the Python's error) when
/// the reply is short, has no NUL, or has fewer words than argc.
pub fn argv_line(data: &[u8]) -> Option<String> {
    let argc = i32::from_ne_bytes(data.get(..4)?.try_into().ok()?);
    let rest = &data[4..];
    let mut i = rest.iter().position(|b| *b == 0)?;
    while i < rest.len() && rest[i] == 0 {
        i += 1;
    }
    // A negative argc: the Python's slice leaves fewer words than argc.
    let argc = usize::try_from(argc).ok()?;
    let args: Vec<&[u8]> = rest[i..].split(|b| *b == 0).take(argc).collect();
    if args.len() != argc {
        return None;
    }
    Some(
        args.iter()
            .map(|a| String::from_utf8_lossy(a))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn reply(argc: i32, tail: &[u8]) -> Vec<u8> {
        let mut v = argc.to_ne_bytes().to_vec();
        v.extend_from_slice(tail);
        v
    }

    #[test]
    fn the_argv_of_a_reply_as_the_python_reads_it() {
        let words = reply(3, b"/bin/sh\0\0\0\0/bin/sh\0-c\0sleep 30\0HOME=/x\0");
        assert_eq!(argv_line(&words).as_deref(), Some("/bin/sh -c sleep 30"));
        // No argv at all.
        assert_eq!(argv_line(&reply(0, b"/bin/sh\0")).as_deref(), Some(""));
        // Nothing after the exec path's NULs: one empty word.
        assert_eq!(argv_line(&reply(1, b"/bin/sh\0\0")).as_deref(), Some(""));
        // Fewer words than argc, a negative argc, no NUL, a short reply.
        assert_eq!(argv_line(&reply(3, b"/bin/sh\0a\0b")), None);
        assert_eq!(argv_line(&reply(-1, b"/bin/sh\0a\0")), None);
        assert_eq!(argv_line(&reply(1, b"/bin/sh")), None);
        assert_eq!(argv_line(&[1, 0]), None);
        // Bytes that aren't UTF-8 read as the replacement character.
        assert_eq!(
            argv_line(&reply(1, b"x\0a\xffb\0")).as_deref(),
            Some("a\u{fffd}b")
        );
    }

    /// The native walk gives the shell's Python walk's rows byte for byte,
    /// from this process up to pid 1. Runs where `ps` can't (the macOS
    /// test nodes' sandbox) and where it can; needs this host's python3.
    #[test]
    fn the_walk_is_the_python_s_walk() {
        let start = std::process::id();
        let native = rows(start).expect("the native walk");
        let out = Command::new("python3")
            .args([
                "-c",
                super::super::ANCESTRY_PY,
                "k2-codex-ancestry",
                &start.to_string(),
            ])
            .stdin(Stdio::null())
            .output()
            .expect("python3 runs");
        assert_eq!(
            out.status.code(),
            Some(0),
            "the Python walk: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let python = String::from_utf8(out.stdout).expect("UTF-8 rows");
        assert_eq!(native, python.trim_end_matches('\n'));
        let first = native.lines().next().expect("a first row");
        assert!(
            first.starts_with(&format!("{start} ")),
            "the walk starts at this process: {first}"
        );
        assert!(native.lines().count() >= 2, "a parent above: {native}");
    }

    /// A Codex app-server above the start is seen by its argv: a child
    /// whose argv names one, walked from the child.
    #[test]
    fn a_shared_server_above_is_found_by_its_argv() {
        // `; exit 0`: the shell waits for sleep rather than becoming it.
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "sleep 30; exit 0",
                "codex",
                "app-server",
                "--listen",
                "unix://x",
            ])
            .stdin(Stdio::null())
            .spawn()
            .expect("sh starts");
        let pid = child.id();
        let head = format!("{pid} {} ", std::process::id());
        let tail = " -c sleep 30; exit 0 codex app-server --listen unix://x";
        let ours =
            |row: Option<&str>| row.is_some_and(|r| r.starts_with(&head) && r.ends_with(tail));
        // The argv is the shell's once its exec has landed.
        let deadline = Instant::now() + Duration::from_secs(10);
        let walked = loop {
            let walked = rows(pid).expect("the native walk from the child");
            if ours(walked.lines().next()) || Instant::now() > deadline {
                break walked;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        child.kill().expect("the child stops");
        child.wait().expect("the child is reaped");
        assert!(ours(walked.lines().next()), "the child's row: {walked}");
        assert!(super::super::has_shared_server(&walked, &pid.to_string()));
        assert!(!super::super::has_shared_server(
            &rows(std::process::id()).expect("the walk from here"),
            &std::process::id().to_string()
        ));
    }
}
