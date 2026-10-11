//! `SCM_RIGHTS` messages over a Unix stream socket, and `sd_notify`.
//!
//! A message is a 4-byte big-endian length plus that many bytes of JSON.
//! File descriptors ride on the `sendmsg` that carries the length, so the
//! receiver's first `recvmsg` of a message collects them.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// Largest JSON body either side sends.
const MAX_BODY: usize = 64 * 1024;
/// Most fds one message carries (lock + three listeners, with room).
const MAX_FDS: usize = 8;

pub(super) fn set_cloexec(fd: RawFd) {
    // SAFETY: plain fcntl on an fd we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags != -1 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Send one message; `fds` are duplicated into the receiver.
pub(super) fn send_msg(sock: &UnixStream, body: &[u8], fds: &[RawFd]) -> io::Result<()> {
    if body.len() > MAX_BODY || fds.len() > MAX_FDS {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "message too large"));
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);

    let mut sent = 0usize;
    let mut first = true;
    while sent < frame.len() {
        let mut iov = libc::iovec {
            iov_base: frame[sent..].as_ptr() as *mut libc::c_void,
            iov_len: frame.len() - sent,
        };
        // SAFETY: zeroed msghdr is a valid empty header.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        let space =
            // SAFETY: CMSG_SPACE is a pure size computation.
            unsafe { libc::CMSG_SPACE((fds.len() * std::mem::size_of::<RawFd>()) as u32) } as usize;
        let mut cbuf = vec![0u8; space.max(1)];
        if first && !fds.is_empty() {
            msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = space as _;
            // SAFETY: msg_control points at a buffer of CMSG_SPACE bytes.
            unsafe {
                let cmsg = libc::CMSG_FIRSTHDR(&msg);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len =
                    libc::CMSG_LEN((fds.len() * std::mem::size_of::<RawFd>()) as u32) as _;
                std::ptr::copy_nonoverlapping(
                    fds.as_ptr() as *const u8,
                    libc::CMSG_DATA(cmsg),
                    fds.len() * std::mem::size_of::<RawFd>(),
                );
            }
        }
        // SAFETY: valid socket fd and msghdr.
        let n = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        first = false;
        sent += n as usize;
    }
    Ok(())
}

/// Receive one message and every fd it carried (at most `max_fds`; any
/// extra fd is closed and the message refused).
pub(super) fn recv_msg(sock: &UnixStream, max_fds: usize) -> io::Result<(Vec<u8>, Vec<OwnedFd>)> {
    let mut fds: Vec<OwnedFd> = Vec::new();
    let mut header = [0u8; 4];
    recv_exact(sock, &mut header, &mut fds)?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut body = vec![0u8; len];
    recv_exact(sock, &mut body, &mut fds)?;
    if fds.len() > max_fds {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} fds, expected at most {max_fds}", fds.len()),
        ));
    }
    Ok((body, fds))
}

fn recv_exact(sock: &UnixStream, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<()> {
    let mut got = 0usize;
    while got < buf.len() {
        let mut iov = libc::iovec {
            iov_base: buf[got..].as_mut_ptr() as *mut libc::c_void,
            iov_len: buf.len() - got,
        };
        // SAFETY: CMSG_SPACE is a pure size computation.
        let space = unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) } as usize;
        let mut cbuf = vec![0u8; space];
        // SAFETY: zeroed msghdr is a valid empty header.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = space as _;
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_CMSG_CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;
        // SAFETY: valid socket fd and msghdr with owned buffers.
        let n = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, flags) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        // Collect fds before anything can return early (no leak).
        // SAFETY: walking the control buffer the kernel filled.
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(cmsg);
                    let bytes = (*cmsg).cmsg_len as usize
                        - (data as usize - cmsg as usize);
                    let count = bytes / std::mem::size_of::<RawFd>();
                    for i in 0..count {
                        let fd = std::ptr::read_unaligned((data as *const RawFd).add(i));
                        set_cloexec(fd);
                        fds.push(OwnedFd::from_raw_fd(fd));
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "fds truncated"));
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "peer closed the hand-off socket"));
        }
        got += n as usize;
    }
    Ok(())
}

/// One datagram to systemd's notify socket (`@abstract` or a path).
pub(super) fn sd_notify(socket: &std::ffi::OsStr, msg: &str) -> io::Result<()> {
    use std::os::unix::net::UnixDatagram;
    let s = socket.to_string_lossy();
    let sock = UnixDatagram::unbound()?;
    if let Some(name) = s.strip_prefix('@') {
        #[cfg(target_os = "linux")]
        {
            use std::os::linux::net::SocketAddrExt;
            let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
            sock.send_to_addr(msg.as_bytes(), &addr)?;
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = name;
            return Err(io::Error::new(io::ErrorKind::Unsupported, "abstract socket"));
        }
    }
    sock.send_to(msg.as_bytes(), &*s)?;
    Ok(())
}
