//! macOS launchd socket activation for `dev.k2.node-admin` (§5.2 option
//! A): the plist's `Sockets` dict names the socket `Listener`; launchd
//! creates it (mode, group) and hands the fd over on demand.

use std::os::unix::net::UnixListener;

extern "C" {
    // libSystem (<launch.h>), macOS 10.10+.
    fn launch_activate_socket(name: *const libc::c_char, fds: *mut *mut libc::c_int, cnt: *mut libc::size_t) -> libc::c_int;
}

/// The socket's key in the plist `Sockets` dict.
pub const SOCKET_NAME: &str = "Listener";

/// The listener launchd made for us, or `None` when not started by
/// launchd (then the caller binds the path itself).
pub fn activated_listener() -> Option<UnixListener> {
    let name = std::ffi::CString::new(SOCKET_NAME).ok()?;
    let mut fds: *mut libc::c_int = std::ptr::null_mut();
    let mut cnt: libc::size_t = 0;
    // SAFETY: valid C string and out-pointers; launchd mallocs `fds`.
    let rc = unsafe { launch_activate_socket(name.as_ptr(), &mut fds, &mut cnt) };
    if rc != 0 || fds.is_null() || cnt == 0 {
        return None;
    }
    // SAFETY: launchd returned `cnt` fds in a malloc'd array; take the
    // first, close any others, free the array.
    let fd = unsafe { *fds };
    for i in 1..cnt {
        unsafe { libc::close(*fds.add(i)) };
    }
    unsafe { libc::free(fds as *mut libc::c_void) };
    use std::os::fd::FromRawFd;
    // SAFETY: launchd handed us ownership of this listening fd.
    Some(unsafe { UnixListener::from_raw_fd(fd) })
}
