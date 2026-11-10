//! One file descriptor over a Unix socket (`SCM_RIGHTS`), with one marker
//! byte. deskd sends the frames pipe's read end right after `hello`, so the
//! daemon's frame reader reads a plain pipe whether deskd is a LaunchAgent
//! (macOS) or a child (Linux, fd 3).

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

/// The marker byte that carries the frames fd.
pub const FRAMES_FD_MARKER: u8 = b'P';

/// Sends `fd` with the one byte `marker`. Retries on EINTR.
pub fn send_fd(sock: &impl AsRawFd, fd: RawFd, marker: u8) -> io::Result<()> {
    let mut byte = [marker];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    // u64 storage keeps the control buffer aligned for cmsghdr.
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cbuf = vec![0u64; space.div_ceil(8)];
    // SAFETY: msghdr is plain data; every pointer below outlives the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    unsafe {
        let c = libc::CMSG_FIRSTHDR(&msg);
        if c.is_null() {
            return Err(io::Error::other("no room for a control message"));
        }
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::copy_nonoverlapping((&fd as *const RawFd).cast::<u8>(), libc::CMSG_DATA(c), std::mem::size_of::<RawFd>());
    }
    loop {
        // SAFETY: msg points at live buffers.
        let n = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) };
        if n == 1 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if n < 0 && e.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(if n < 0 { e } else { io::Error::other(format!("sendmsg sent {n} bytes")) });
    }
}

/// Receives one byte and the fd it carries. Fails if the byte arrives with
/// no fd, more than one fd, or at end of stream. Extra fds are closed.
pub fn recv_fd(sock: &impl AsRawFd) -> io::Result<(u8, OwnedFd)> {
    let mut byte = [0u8];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    let space = unsafe { libc::CMSG_SPACE((4 * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut cbuf = vec![0u64; space.div_ceil(8)];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    let n = loop {
        // SAFETY: msg points at live buffers.
        let n = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, 0) };
        if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        break n;
    };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut fds: Vec<OwnedFd> = Vec::new();
    // SAFETY: walking the control buffer recvmsg filled in.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c);
                let header = libc::CMSG_LEN(0) as usize;
                let count = ((*c).cmsg_len as usize).saturating_sub(header) / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    let mut fd: RawFd = -1;
                    std::ptr::copy_nonoverlapping(data.add(i * std::mem::size_of::<RawFd>()), (&mut fd as *mut RawFd).cast::<u8>(), std::mem::size_of::<RawFd>());
                    if fd >= 0 {
                        fds.push(OwnedFd::from_raw_fd(fd));
                    }
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the socket closed before the frames fd"));
    }
    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(io::Error::other("control message truncated"));
    }
    if fds.len() != 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("want one fd with the marker, got {}", fds.len())));
    }
    Ok((byte[0], fds.pop().expect("one fd")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    #[test]
    fn a_pipe_read_end_crosses_and_still_reads() {
        let (a, b) = UnixStream::pair().unwrap();
        let (rx, mut tx) = std::io::pipe().unwrap();
        send_fd(&a, rx.as_raw_fd(), FRAMES_FD_MARKER).unwrap();
        drop(rx);
        let (m, fd) = recv_fd(&b).unwrap();
        assert_eq!(m, FRAMES_FD_MARKER);
        tx.write_all(b"frame bytes").unwrap();
        drop(tx);
        let mut got = String::new();
        std::fs::File::from(fd).read_to_string(&mut got).unwrap();
        assert_eq!(got, "frame bytes");
    }

    #[test]
    fn a_plain_byte_without_an_fd_is_refused() {
        let (mut a, b) = UnixStream::pair().unwrap();
        a.write_all(b"P").unwrap();
        let e = recv_fd(&b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn end_of_stream_is_an_error() {
        let (a, b) = UnixStream::pair().unwrap();
        drop(a);
        assert_eq!(recv_fd(&b).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }
}
