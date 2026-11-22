//! What a call by one of the brand's aliases does before the command
//! runs: the notice, or the tombstone. Both are built from the brand
//! table's two names ([`crate::called_as::Names`]), never spelled here.
//!
//! - An alias in mode `notice`: the command runs, after one line on
//!   stderr ([`text`]): `note: <alias> is now <name> (the same command).
//!   Type <name> from now on.`
//! - An alias in mode `tombstone`: nothing runs. One line on stderr
//!   ([`tombstone_text`]), `<alias> is now <name>. Run <name> instead.`,
//!   and exit 127 ([`Exit::Tombstone`]). It never repeats the arguments:
//!   they may hold a secret.
//! - An alias in mode `silent`, or the command's own name: nothing.
//!
//! `src/main.rs` asks [`on_call`] right after it reads the name it was
//! called by. The default brand has no alias in either mode, so nothing
//! shows there; turning one on is a change to a brand file, not to code.
//!
//! The notice shows once. When the front hands the call to the shell CLI
//! it sets `K5_CLI_NOTICE_DONE=1` ([`crate::called_as::NOTICE_DONE_VAR`]);
//! the shell reads that into `_K5_NOTICE_DONE` (`cli/src/prelude.sh`) and
//! never prints a second one.
//!
//! It is not shown when stdout and stderr are the same pipe or file and
//! stdout is not a terminal: `out=$(xy ... 2>&1)` would get the notice in
//! the answer it reads. Two streams count as the same when their device
//! and inode are ([`merged`]). The tombstone always shows: nothing else
//! does.
//!
//! On Windows that check can't tell: the file ids that would (volume
//! serial and file index) are not stable in Rust's std there, and this
//! crate adds no Windows API crate for it. There the notice shows only
//! when stderr is a console (`GetConsoleMode` on it succeeds), never into
//! a pipe or a file (K2's ruling, R1 slice 10). That code is built for
//! Windows only and has not been run there; [`should_show`] holds the
//! rule and is tested everywhere with a stand-in for the console check.

use crate::called_as::CalledAs;
use crate::exit::Exit;
use k2_client::brand::{AliasMode, CLI_NAME};

/// The notice for a call by `alias` of the command called `name`: one
/// line, for stderr.
pub fn text(alias: &str, name: &str) -> String {
    format!("note: {alias} is now {name} (the same command). Type {name} from now on.")
}

/// The tombstone's line for a call by `alias` of the command called
/// `name`, for stderr. It takes no arguments, so it can't repeat them.
pub fn tombstone_text(alias: &str, name: &str) -> String {
    format!("{alias} is now {name}. Run {name} instead.")
}

/// What a call does before the command runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Before {
    /// Nothing: the command runs.
    Run,
    /// This notice on stderr when it is due ([`should_show`]), then the
    /// command runs.
    Notice(String),
    /// This line on stderr, then exit 127; nothing runs.
    Tombstone(String),
}

/// What a call by `who` does before the command called `name` runs.
pub fn before(who: CalledAs, name: &str) -> Before {
    match who {
        CalledAs::Alias {
            name: alias,
            mode: AliasMode::Notice,
        } => Before::Notice(text(alias, name)),
        CalledAs::Alias {
            name: alias,
            mode: AliasMode::Tombstone,
        } => Before::Tombstone(tombstone_text(alias, name)),
        CalledAs::Alias {
            mode: AliasMode::Silent,
            ..
        }
        | CalledAs::Primary => Before::Run,
    }
}

/// [`before`] for this process, the compiled command's name: the notice
/// printed when due and `None` (the command runs), or the tombstone's
/// line printed and the exit to end with.
pub fn on_call(who: CalledAs) -> Option<Exit> {
    match before(who, CLI_NAME) {
        Before::Run => None,
        Before::Notice(line) => {
            if should_show(who, streams(), stderr_is_console) {
                crate::out::stderr_line(&line);
            }
            None
        }
        Before::Tombstone(line) => {
            crate::out::stderr_line(&line);
            Some(Exit::Tombstone)
        }
    }
}

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

    use crate::called_as::{called_as, Names, Os};
    use std::ffi::OsStr;

    /// The command after the rename: the old name in mode notice, the old
    /// shim's name silent.
    const K5: Names = Names {
        name: "k5",
        aliases: &[("k2", AliasMode::Notice), ("k2so", AliasMode::Silent)],
    };

    /// The release after that: the old name in mode tombstone.
    const K5_TOMBSTONE: Names = Names {
        name: "k5",
        aliases: &[("k2", AliasMode::Tombstone)],
    };

    /// A brand with no aliases.
    const XY: Names = Names {
        name: "xy",
        aliases: &[],
    };

    /// What a call by `argv0` does before the command of `names` runs.
    fn before_for(names: &Names, argv0: &str) -> Before {
        let who = called_as(OsStr::new(argv0), |_| None, Os::Unix, names);
        before(who, names.name)
    }

    /// With the names k2 and k5 the notice is the ruled line, byte for
    /// byte, and the tombstone is K2's ruled line.
    #[test]
    fn the_lines_are_the_ruled_text() {
        assert_eq!(
            text("k2", "k5"),
            "note: k2 is now k5 (the same command). Type k5 from now on."
        );
        assert_eq!(tombstone_text("k2", "k5"), "k2 is now k5. Run k5 instead.");
        for line in [text("k2", "k5"), tombstone_text("k2", "k5")] {
            assert!(!line.contains('\n'), "{line:?}");
        }
    }

    /// Each mode of the table does what it says; the command's own name
    /// and a name the table does not know do nothing.
    #[test]
    fn each_mode_from_the_table() {
        let notice = Before::Notice(text("k2", "k5"));
        assert_eq!(before_for(&K5, "k2"), notice);
        assert_eq!(before_for(&K5, "/usr/local/bin/k2"), notice);
        assert_eq!(before_for(&K5, "k2so"), Before::Run);
        assert_eq!(before_for(&K5, "k5"), Before::Run);
        assert_eq!(before_for(&K5, "some-copy"), Before::Run);
        let tomb = Before::Tombstone(tombstone_text("k2", "k5"));
        assert_eq!(before_for(&K5_TOMBSTONE, "k2"), tomb);
        assert_eq!(before_for(&K5_TOMBSTONE, "k5"), Before::Run);
        for argv0 in ["xy", "k2", "k5", "k2so"] {
            assert_eq!(before_for(&XY, argv0), Before::Run, "{argv0}");
        }
        // The names are the table's, not ours.
        let other = Names {
            name: "xy",
            aliases: &[("xyold", AliasMode::Notice), ("xy1", AliasMode::Tombstone)],
        };
        assert_eq!(
            before_for(&other, "xyold"),
            Before::Notice(
                "note: xyold is now xy (the same command). Type xy from now on.".to_string()
            )
        );
        assert_eq!(
            before_for(&other, "xy1"),
            Before::Tombstone("xy1 is now xy. Run xy instead.".to_string())
        );
    }

    /// The compiled brand turns neither on: no alias in mode notice or
    /// tombstone, so whatever the name, nothing prints and the command
    /// runs. This fails on purpose at the switch, when the brand file
    /// gives an alias one of those modes.
    #[test]
    fn the_compiled_brand_shows_neither() {
        let names = Names::COMPILED;
        for (alias, mode) in names.aliases {
            assert_eq!(*mode, AliasMode::Silent, "alias {alias}");
        }
        let mut argv0s = vec![names.name, "some-copy", ""];
        argv0s.extend(names.aliases.iter().map(|(alias, _)| *alias));
        for argv0 in &argv0s {
            assert_eq!(before_for(&names, argv0), Before::Run, "{argv0:?}");
            for value in &argv0s {
                let who = called_as(
                    OsStr::new(argv0),
                    |_| Some(value.to_string()),
                    Os::Unix,
                    &names,
                );
                assert_eq!(on_call(who), None, "{argv0:?}, variable {value:?}");
            }
        }
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
