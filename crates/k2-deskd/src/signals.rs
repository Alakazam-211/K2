//! SIGTERM / SIGINT: release every held key and button, hide the banner,
//! then exit (vs-live R3). launchd sends SIGTERM on `bootout`; the daemon
//! kills its Linux child the same way.
//!
//! The handler only writes one byte to a pipe (async-signal-safe); a
//! thread does the work.

use std::sync::atomic::{AtomicI32, Ordering};

static WAKE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_sig: libc::c_int) {
    let fd = WAKE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let b = [1u8];
        // SAFETY: write(2) is async-signal-safe.
        unsafe { libc::write(fd, b.as_ptr().cast(), 1) };
    }
}

pub fn on_sigterm(f: impl FnOnce() + Send + 'static) {
    let mut fds = [0i32; 2];
    // SAFETY: plain pipe(2).
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        eprintln!("k2-deskd: no signal pipe: {}", std::io::Error::last_os_error());
        return;
    }
    WAKE_FD.store(fds[1], Ordering::SeqCst);
    // SAFETY: installing a handler that only calls write(2).
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }
    let rfd = fds[0];
    std::thread::Builder::new()
        .name("k2-deskd-signal".into())
        .spawn(move || {
            let mut b = [0u8];
            loop {
                // SAFETY: reading our own pipe.
                let n = unsafe { libc::read(rfd, b.as_mut_ptr().cast(), 1) };
                if n == 1 {
                    break;
                }
                if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            f();
            std::process::exit(0);
        })
        .expect("spawn signal thread");
}
