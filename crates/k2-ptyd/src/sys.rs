//! Small libc wrappers: fd flags, peer credentials, SCM_RIGHTS.

use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

pub fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

pub fn set_nonblocking(fd: RawFd, on: bool) -> io::Result<()> {
    // SAFETY: fcntl on a caller-owned fd.
    unsafe {
        let fl = cvt(libc::fcntl(fd, libc::F_GETFL))?;
        let fl = if on { fl | libc::O_NONBLOCK } else { fl & !libc::O_NONBLOCK };
        cvt(libc::fcntl(fd, libc::F_SETFL, fl))?;
    }
    Ok(())
}

pub fn set_cloexec(fd: RawFd, on: bool) -> io::Result<()> {
    // SAFETY: fcntl on a caller-owned fd.
    unsafe {
        let fl = cvt(libc::fcntl(fd, libc::F_GETFD))?;
        let fl = if on { fl | libc::FD_CLOEXEC } else { fl & !libc::FD_CLOEXEC };
        cvt(libc::fcntl(fd, libc::F_SETFD, fl))?;
    }
    Ok(())
}

/// The connecting peer's effective uid (P10).
pub fn peer_uid(fd: RawFd) -> io::Result<u32> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: getsockopt writes at most `len` bytes into `cred`.
        cvt(unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        })?;
        Ok(cred.uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: getpeereid writes two ids.
        cvt(unsafe { libc::getpeereid(fd, &mut uid, &mut gid) })?;
        Ok(uid)
    }
}

pub fn euid() -> u32 {
    // SAFETY: no preconditions.
    unsafe { libc::geteuid() }
}

/// `sendmsg` with an optional fd attached (SCM_RIGHTS). Returns bytes sent.
pub fn send_with_fd(sock: RawFd, data: &[u8], pass: Option<RawFd>) -> io::Result<usize> {
    let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut libc::c_void, iov_len: data.len() };
    // SAFETY: CMSG_SPACE is a pure size computation.
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cbuf = vec![0u8; space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if let Some(fd) = pass {
        msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = space as _;
        // SAFETY: msg_control points at `space` bytes, enough for one fd.
        unsafe {
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(c) as *mut RawFd, fd);
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let flags = libc::MSG_NOSIGNAL;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let flags = 0;
    // SAFETY: msg is fully initialised above.
    let n = unsafe { libc::sendmsg(sock, &msg, flags) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

/// `recvmsg` collecting any SCM_RIGHTS fds (marked CLOEXEC).
pub fn recv_with_fds(sock: RawFd, buf: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut libc::c_void, iov_len: buf.len() };
    // SAFETY: pure size computation; room for 8 fds.
    let space = unsafe { libc::CMSG_SPACE((8 * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut cbuf = vec![0u8; space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = space as _;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let flags = libc::MSG_CMSG_CLOEXEC;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let flags = 0;
    // SAFETY: msg points at valid buffers.
    let n = unsafe { libc::recvmsg(sock, &mut msg, flags) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut fds = Vec::new();
    // SAFETY: walking the control buffer the kernel filled in.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c) as *const RawFd;
                let payload = (*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                for i in 0..payload / std::mem::size_of::<RawFd>() {
                    let fd = std::ptr::read_unaligned(data.add(i));
                    let owned = OwnedFd::from_raw_fd(fd);
                    set_cloexec(fd, true)?;
                    fds.push(owned);
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    Ok((n as usize, fds))
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
