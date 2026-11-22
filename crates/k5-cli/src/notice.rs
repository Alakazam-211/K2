//! The notice that tells a caller of `k2` to type `k5` from now on.
//!
//! It is off: nothing calls it yet, and `src/main.rs` never prints it. It
//! goes on only once the decision to retire the `k2` name is made. Then, called as `k2`,
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
//! On Windows that check can't tell: the file ids that would (volume
//! serial and file index) are not stable in Rust's std there, and this
//! crate adds no Windows API crate for it. There the notice shows only
//! when stderr is a console (`GetConsoleMode` on it succeeds), never into
//! a pipe or a file (K2's ruling, R1 slice 10). That code is built for
//! Windows only and has not been run there; [`should_show`] holds the
//! rule and is tested everywhere with a stand-in for the console check.

use crate::called_as::CalledAs;
use k2_client::brand::AliasMode;

/// The notice, one line, on stderr. It never repeats the arguments.
pub const TEXT: &str = "note: k2 is now k5 (the same command). Type k5 from now on.";

/// What the merged check says about stdout and stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Streams {
    /// One pipe or file holds both ([`merged`]).
    Merged,
    /// They are apart, or stdout is a terminal.
    Apart,
    /// The check can't tell (Windows).
    Unknown,
}

/// Whether the notice shows: only when called by an alias in mode
/// `notice`; not when stdout and stderr are merged into one capture; and
/// when the merged check can't tell, only when `stderr_is_console` says
/// stderr is a console. That check runs only then.
pub fn should_show(
    called_as: CalledAs,
    streams: Streams,
    stderr_is_console: impl FnOnce() -> bool,
) -> bool {
    if !matches!(
        called_as,
        CalledAs::Alias {
            mode: AliasMode::Notice,
            ..
        }
    ) {
        return false;
    }
    match streams {
        Streams::Merged => false,
        Streams::Apart => true,
        Streams::Unknown => stderr_is_console(),
    }
}

/// Prints the notice when it is due. Nothing calls this yet.
pub fn show_if_due(called_as: CalledAs) {
    if should_show(called_as, streams(), stderr_is_console) {
        crate::out::stderr_line(TEXT);
    }
}

/// What the merged check says about this process's stdout and stderr
/// ([`merged`] on fds 1 and 2). [`Streams::Unknown`] on Windows.
pub fn streams() -> Streams {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        if merged(std::io::stdout().as_fd(), std::io::stderr().as_fd()) {
            Streams::Merged
        } else {
            Streams::Apart
        }
    }
    #[cfg(not(unix))]
    {
        Streams::Unknown
    }
}

/// Whether this process's stderr is a console: `GetConsoleMode` on its
/// handle succeeds. A pipe, a file, the null device or no handle at all
/// is not. Untested on Windows (see the module notes).
#[cfg(windows)]
pub fn stderr_is_console() -> bool {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleMode(console: *mut std::ffi::c_void, mode: *mut u32) -> i32;
    }
    let handle = std::io::stderr().as_raw_handle();
    let mut mode = 0u32;
    // SAFETY: GetConsoleMode only reads the handle (a null or invalid one
    // makes it fail) and writes one u32 through a pointer to a local.
    unsafe { GetConsoleMode(handle.cast(), &mut mode) != 0 }
}

/// Elsewhere the merged check always tells, so this is never asked.
#[cfg(not(windows))]
pub fn stderr_is_console() -> bool {
    false
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
        assert_eq!(
            TEXT,
            "note: k2 is now k5 (the same command). Type k5 from now on."
        );
        assert!(!TEXT.contains('\n'));
    }

    fn never_asked() -> bool {
        panic!("the console check ran although the merged check could tell")
    }

    const NOTICE: CalledAs = CalledAs::Alias {
        name: "ab1",
        mode: AliasMode::Notice,
    };

    #[test]
    fn only_a_notice_alias_and_only_when_not_merged() {
        assert!(should_show(NOTICE, Streams::Apart, never_asked));
        assert!(!should_show(NOTICE, Streams::Merged, never_asked));
        let others = [
            CalledAs::Primary,
            CalledAs::Alias {
                name: "ab1",
                mode: AliasMode::Silent,
            },
            CalledAs::Alias {
                name: "ab1",
                mode: AliasMode::Tombstone,
            },
        ];
        for who in others {
            for streams in [Streams::Apart, Streams::Merged, Streams::Unknown] {
                assert!(
                    !should_show(who, streams, never_asked),
                    "{who:?} {streams:?}"
                );
            }
        }
    }

    /// Windows' default: when the merged check can't tell, a console on
    /// stderr shows it and anything else (a pipe, a file) doesn't.
    #[test]
    fn when_it_can_t_tell_only_a_console_shows_it() {
        assert!(should_show(NOTICE, Streams::Unknown, || true));
        assert!(!should_show(NOTICE, Streams::Unknown, || false));
    }

    #[cfg(unix)]
    #[test]
    fn unix_always_tells() {
        assert_ne!(streams(), Streams::Unknown);
        assert!(!stderr_is_console());
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
                assert_eq!(
                    libc::grantpt(fd),
                    0,
                    "grantpt: {}",
                    io::Error::last_os_error()
                );
                assert_eq!(
                    libc::unlockpt(fd),
                    0,
                    "unlockpt: {}",
                    io::Error::last_os_error()
                );
                let name = libc::ptsname(fd);
                assert!(!name.is_null(), "ptsname: {}", io::Error::last_os_error());
                let path = CStr::from_ptr(name)
                    .to_str()
                    .expect("a UTF-8 pty name")
                    .to_owned();
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
