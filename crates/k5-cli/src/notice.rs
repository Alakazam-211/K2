//! The notice that tells a caller of `k2` to type `k5` from now on.
//!
//! It is off: nothing calls it yet, and `src/main.rs` never prints it. It
//! goes on with the switch (R1 slice 10) and 0.47. Then, called as `k2`,
//! the front prints [`TEXT`] once on stderr before it answers. When it
//! hands the call to the shell CLI it sets `K5_CLI_NOTICE_DONE=1`
//! ([`crate::called_as::NOTICE_DONE_VAR`]); the shell reads that into
//! `_K5_NOTICE_DONE` (`cli/src/prelude.sh`) and never prints a second one.
//!
//! It is not shown when stdout and stderr are the same pipe or file and
//! stdout is not a terminal: `out=$(k2 ... 2>&1)` would get the notice in
//! the answer it reads. Two streams count as the same when their device
//! and inode are ([`merged`]).
//!
//! On Windows they never count as merged, so the notice shows. The file
//! ids that would tell (volume serial and file index) are not stable in
//! Rust's std there, and this crate doesn't add a Windows API crate for a
//! check nobody calls yet. Look again when the switch turns it on.

use crate::called_as::CalledAs;

/// The notice, one line, on stderr. It never repeats the arguments.
pub const TEXT: &str = "note: k2 is now k5 (the same command). Type k5 from now on; k2 stops working in 0.48.";

/// Whether the notice shows: only when called as `k2`, and not when
/// stdout and stderr are merged into one capture.
pub fn should_show(called_as: CalledAs, merged: bool) -> bool {
    called_as == CalledAs::K2Name && !merged
}

/// Prints the notice when it is due. Nothing calls this yet.
pub fn show_if_due(called_as: CalledAs) {
    if should_show(called_as, stdout_and_stderr_merged()) {
        crate::out::stderr_line(TEXT);
    }
}

/// Whether this process's stdout and stderr are merged ([`merged`] on
/// fds 1 and 2). Never on Windows (see the module notes).
pub fn stdout_and_stderr_merged() -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        merged(std::io::stdout().as_fd(), std::io::stderr().as_fd())
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Whether `stdout` and `stderr` are the same pipe or file while `stdout`
/// is not a terminal. Same means the same device and inode, as `fstat`
/// gives them. One the system can't describe (a closed fd) is not merged.
#[cfg(unix)]
pub fn merged(stdout: std::os::fd::BorrowedFd<'_>, stderr: std::os::fd::BorrowedFd<'_>) -> bool {
    use std::io::IsTerminal;
    use std::os::unix::fs::MetadataExt;
    if stdout.is_terminal() {
        return false;
    }
    let stat = |fd: std::os::fd::BorrowedFd<'_>| -> std::io::Result<std::fs::Metadata> {
        std::fs::File::from(fd.try_clone_to_owned()?).metadata()
    };
    match (stat(stdout), stat(stderr)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_is_the_ruled_line() {
        assert_eq!(TEXT, "note: k2 is now k5 (the same command). Type k5 from now on; k2 stops working in 0.48.");
        assert!(!TEXT.contains('\n'));
    }

    #[test]
    fn only_k2_and_only_when_not_merged() {
        assert!(should_show(CalledAs::K2Name, false));
        assert!(!should_show(CalledAs::K2Name, true));
        assert!(!should_show(CalledAs::K5Name, false));
        assert!(!should_show(CalledAs::K5Name, true));
    }

    #[cfg(unix)]
    mod fds {
        use super::super::merged;
        use std::ffi::CStr;
        use std::fs::{File, OpenOptions};
        use std::io::{self, IsTerminal};
        use std::os::fd::{AsFd, FromRawFd, OwnedFd};
        use std::os::unix::fs::OpenOptionsExt;
        use std::path::PathBuf;

        fn temp_file(tag: &str) -> PathBuf {
            let p = std::env::temp_dir().join(format!("k5-notice-{tag}-{}", std::process::id()));
            File::create(&p).unwrap_or_else(|e| panic!("create {}: {e}", p.display()));
            p
        }

        #[test]
        fn one_pipe_on_both_is_merged() {
            let (_reader, writer) = io::pipe().unwrap();
            let dup = writer.try_clone().unwrap();
            assert!(merged(writer.as_fd(), dup.as_fd()));
            assert!(merged(writer.as_fd(), writer.as_fd()));
        }

        #[test]
        fn two_pipes_are_not() {
            let (_r1, one) = io::pipe().unwrap();
            let (_r2, two) = io::pipe().unwrap();
            assert!(!merged(one.as_fd(), two.as_fd()));
        }

        #[test]
        fn one_file_on_both_is_merged_and_two_files_are_not() {
            let a = temp_file("a");
            let b = temp_file("b");
            let fa = File::options().append(true).open(&a).unwrap();
            let fa_dup = fa.try_clone().unwrap();
            let fa_again = File::options().append(true).open(&a).unwrap();
            let fb = File::options().append(true).open(&b).unwrap();
            // `>f 2>&1`
            assert!(merged(fa.as_fd(), fa_dup.as_fd()));
            // `>f 2>>f`: opened twice, still the one file.
            assert!(merged(fa.as_fd(), fa_again.as_fd()));
            assert!(!merged(fa.as_fd(), fb.as_fd()));
            // A file and a pipe.
            let (_r, w) = io::pipe().unwrap();
            assert!(!merged(fa.as_fd(), w.as_fd()));
            assert!(!merged(w.as_fd(), fa.as_fd()));
            std::fs::remove_file(&a).unwrap();
            std::fs::remove_file(&b).unwrap();
        }

        #[test]
        fn dev_null_on_both_is_merged() {
            let null = OpenOptions::new().write(true).open("/dev/null").unwrap();
            let dup = null.try_clone().unwrap();
            assert!(!null.is_terminal());
            assert!(merged(null.as_fd(), dup.as_fd()));
        }

        /// A new pseudo terminal's far end, opened as a file.
        fn pty() -> (OwnedFd, File) {
            // SAFETY: posix_openpt hands back a new fd we own; grantpt,
            // unlockpt and ptsname read only that fd, and the name is
            // copied out before anything else calls ptsname.
            unsafe {
                let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
                assert!(fd >= 0, "posix_openpt: {}", io::Error::last_os_error());
                let master = OwnedFd::from_raw_fd(fd);
                assert_eq!(libc::grantpt(fd), 0, "grantpt: {}", io::Error::last_os_error());
                assert_eq!(libc::unlockpt(fd), 0, "unlockpt: {}", io::Error::last_os_error());
                let name = libc::ptsname(fd);
                assert!(!name.is_null(), "ptsname: {}", io::Error::last_os_error());
                let path = CStr::from_ptr(name).to_str().expect("a UTF-8 pty name").to_owned();
                let far = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NOCTTY)
                    .open(&path)
                    .unwrap_or_else(|e| panic!("open {path}: {e}"));
                (master, far)
            }
        }

        #[test]
        fn one_terminal_on_both_is_not_merged() {
            let (_master, tty) = pty();
            assert!(tty.is_terminal(), "the pty is not a terminal");
            let dup = tty.try_clone().unwrap();
            assert!(!merged(tty.as_fd(), dup.as_fd()));
            // A terminal on stdout, a pipe on stderr.
            let (_r, w) = io::pipe().unwrap();
            assert!(!merged(tty.as_fd(), w.as_fd()));
            // A pipe on stdout, the terminal on stderr.
            assert!(!merged(w.as_fd(), tty.as_fd()));
        }
    }
}
