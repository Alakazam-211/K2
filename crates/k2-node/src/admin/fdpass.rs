//! Passing file descriptors over the executor's Unix socket (`SCM_RIGHTS`):
//! the node hands root its job's stdout/stderr pipes, so no path on disk
//! (which a job could swap for a symlink) is ever opened by root for it.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// At most this many fds in one message.
pub const MAX_FDS: usize = 4;

/// Held (shared) around every child spawn in the executor; held (alone)
/// while macOS receives fds and marks them close-on-exec. macOS has no
/// `MSG_CMSG_CLOEXEC`, so without it a child forked in between would
/// inherit a job's pipe (A-L1 / B-L3). Linux gets the flag atomically.
static SPAWN: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Take before spawning a child from the executor. Hold it only across
/// the fork/exec (the spawn), never while the child runs (A L1).
pub fn spawn_guard() -> std::sync::RwLockReadGuard<'static, ()> {
    SPAWN.read().unwrap_or_else(|p| p.into_inner())
}

/// `Command::output`, with the spawn under [`spawn_guard`] and the wait
/// outside it. stdin is `/dev/null` (every executor child takes no input).
/// The child is always reaped, also when reading its output fails.
pub fn spawn_output(cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = {
        let _g = spawn_guard();
        cmd.spawn()?
    };
    let mut out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    // stderr on its own thread: a child filling one pipe never blocks on the other.
    let err_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let r = err_pipe.map(|mut p| p.read_to_end(&mut b)).transpose();
        r.map(|_| b)
    });
    let mut stdout = Vec::new();
    let out_r = out_pipe.as_mut().map(|p| p.read_to_end(&mut stdout)).transpose();
    let err_r = err_reader.join().unwrap_or_else(|_| Err(std::io::Error::other("stderr reader panicked")));
    // Reaped whatever happened above.
    let status = child.wait()?;
    out_r?;
    let stderr = err_r?;
    Ok(std::process::Output { status, stdout, stderr })
}

/// Send one byte carrying `fds`.
pub fn send_fds(s: &UnixStream, fds: &[RawFd]) -> std::io::Result<()> {
    if fds.is_empty() || fds.len() > MAX_FDS {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "1 to 4 fds"));
    }
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr() as *mut libc::c_void, iov_len: 1 };
    let payload = std::mem::size_of_val(fds) as u32;
    // SAFETY: CMSG_SPACE is a pure size computation.
    let space = unsafe { libc::CMSG_SPACE(payload) } as usize;
    let mut cbuf = vec![0u8; space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = space as _;
    // SAFETY: msg points at a control buffer of CMSG_SPACE(payload) bytes,
    // so the first header and its data fit.
    unsafe {
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(payload) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr() as *const u8, libc::CMSG_DATA(c), payload as usize);
        if libc::sendmsg(s.as_raw_fd(), &msg, 0) != 1 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Receive one byte and the fds it carries (closed on drop).
pub fn recv_fds(s: &UnixStream) -> std::io::Result<Vec<OwnedFd>> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr() as *mut libc::c_void, iov_len: 1 };
    let payload = (MAX_FDS * std::mem::size_of::<RawFd>()) as u32;
    // SAFETY: pure size computation.
    let space = unsafe { libc::CMSG_SPACE(payload) } as usize;
    let mut cbuf = vec![0u8; space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = space as _;
    #[cfg(target_os = "linux")]
    let (flags, _spawns_wait) = (libc::MSG_CMSG_CLOEXEC, ());
    #[cfg(not(target_os = "linux"))]
    let (flags, _spawns_wait) = {
        // Wait for the message outside the lock, then receive it and mark
        // its fds close-on-exec while no child can be forked.
        let mut pfd = libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let wait_ms = s.read_timeout().ok().flatten().map(|d| d.as_millis().min(i32::MAX as u128) as i32).unwrap_or(-1);
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut pfd, 1, wait_ms) };
        if r == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "no fd message"));
        }
        if r < 0 {
            return Err(std::io::Error::last_os_error());
        }
        (libc::MSG_DONTWAIT, SPAWN.write().unwrap_or_else(|p| p.into_inner()))
    };
    // SAFETY: valid socket and message buffers.
    let n = unsafe { libc::recvmsg(s.as_raw_fd(), &mut msg, flags) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if n == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "no fd message"));
    }
    let mut out = Vec::new();
    // SAFETY: walking the control headers the kernel filled in.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c);
                let len = (*c).cmsg_len as usize - (data as usize - c as usize);
                let count = len / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    let mut fd: RawFd = 0;
                    std::ptr::copy_nonoverlapping(data.add(i * std::mem::size_of::<RawFd>()), &mut fd as *mut RawFd as *mut u8, std::mem::size_of::<RawFd>());
                    #[cfg(not(target_os = "linux"))]
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    out.push(OwnedFd::from_raw_fd(fd));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "too many fds"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn a_pipe_end_crosses_the_socket() {
        let (a, b) = UnixStream::pair().unwrap();
        let (mut r, w) = std::io::pipe().unwrap();
        send_fds(&a, &[w.as_raw_fd()]).unwrap();
        drop(w);
        let mut got = recv_fds(&b).unwrap();
        assert_eq!(got.len(), 1);
        let mut f = std::fs::File::from(got.remove(0));
        f.write_all(b"through root").unwrap();
        drop(f);
        let mut s = String::new();
        r.read_to_string(&mut s).unwrap();
        assert_eq!(s, "through root");
        assert!(send_fds(&a, &[]).is_err());
    }
}
