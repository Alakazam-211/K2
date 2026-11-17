//! The name the command was called by, and what the front hands the shell
//! CLI when a verb falls through to it.
//!
//! The rule is the shell CLI's (`cli/src/prelude.sh`, pinned by
//! `tests/cli/called_as.sh`):
//!
//! 1. `K5_CLI_CALLED_AS` wins when it is exactly `k2` or `k5`. Any other
//!    value, empty included, is ignored.
//! 2. Otherwise the last part of `argv[0]` decides: `k5` means k5, and any
//!    other name (`k2`, the old `k2so` shim, a copy under another name)
//!    counts as k2.
//!
//! On Windows the program is `k5.exe`, and `argv[0]` holds what the person
//! typed (`k5`, `k5.exe`, `K5`, `C:\bin\K5.EXE`). File names there ignore
//! case and the shell finds `k5.exe` for `k5`, so on Windows the name is
//! compared without case and a trailing `.exe` is dropped first; both
//! `\` and `/` end a folder name. Elsewhere the shell's rule holds byte for
//! byte: `K5` and `k5.exe` are other programs and count as k2.
//!
//! The two names are the values the shell compares against, not words for
//! people, so they are spelled here and not taken from
//! [`k2_client::brand`]. Today `brand::CLI_NAME` is `k2`, the name the
//! k2 side stands for; a test pins that, so the rename that moves
//! `CLI_NAME` to `k5` has to decide what calling the command `k2` means
//! then.

use std::ffi::OsStr;

/// The variable that tells the shell the name the front was called by.
pub const CALLED_AS_VAR: &str = "K5_CLI_CALLED_AS";
/// The variable that tells the shell the front has dealt with the notice
/// that asks callers of k2 to use k5 (shown it, or decided not to), so the
/// shell does not show it again.
pub const NOTICE_DONE_VAR: &str = "K5_CLI_NOTICE_DONE";
/// The variable that tells the shell the call came through the front. Its
/// requests to the daemon then say so.
pub const VIA_VAR: &str = "K5_CLI_VIA";
/// The one value of [`VIA_VAR`] the shell acts on.
pub const VIA_RUST: &str = "rust";
/// The variable that tells the shell the folder of the program people ran
/// (links resolved), where it looks first for the files shipped beside
/// the CLI (`k2-approve`, `k2-widget-shot.mjs`). The front runs the script
/// as `/dev/fd/N`, so the folder of the shell's `$0` says nothing. It is a
/// path, so it is not in [`CalledAs::fallthrough_env`]; the front sets it
/// beside those three ([`crate::fallthrough::command`]).
pub const SCRIPT_DIR_VAR: &str = "K5_CLI_SCRIPT_DIR";
/// Every variable the front hands the shell. Each is one name, not a
/// setting read under three.
pub const FRONT_VARS: [&str; 4] = [CALLED_AS_VAR, NOTICE_DONE_VAR, VIA_VAR, SCRIPT_DIR_VAR];

/// Which of the command's two names it was called by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalledAs {
    /// `k2`, or any name but `k5`.
    K2Name,
    /// `k5`.
    K5Name,
}

/// Whose rule reads `argv[0]`: see the module notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// The shell's rule, byte for byte.
    Unix,
    /// `\` too ends a folder name, case is ignored and `.exe` is dropped.
    Windows,
}

impl Os {
    /// The system this was built for.
    pub const CURRENT: Os = if cfg!(windows) { Os::Windows } else { Os::Unix };
}

impl CalledAs {
    /// The name, as [`CALLED_AS_VAR`] carries it: `k2` or `k5`.
    pub fn name(self) -> &'static str {
        match self {
            CalledAs::K2Name => "k2",
            CalledAs::K5Name => "k5",
        }
    }

    /// The name this process was called by, from its `argv[0]` and its
    /// environment.
    pub fn current(argv0: &OsStr) -> CalledAs {
        called_as(argv0, |name| std::env::var(name).ok(), Os::CURRENT)
    }

    /// What the front sets for the shell CLI when it hands a verb on:
    /// the name it was called by, that the notice is dealt with, and that
    /// the call came through the front.
    pub fn fallthrough_env(self) -> [(&'static str, &'static str); 3] {
        [(CALLED_AS_VAR, self.name()), (NOTICE_DONE_VAR, "1"), (VIA_VAR, VIA_RUST)]
    }
}

/// The rule, over `argv[0]`, a lookup of the environment and the system's
/// way with file names.
pub fn called_as(argv0: &OsStr, lookup: impl Fn(&str) -> Option<String>, os: Os) -> CalledAs {
    match lookup(CALLED_AS_VAR).as_deref() {
        Some("k2") => return CalledAs::K2Name,
        Some("k5") => return CalledAs::K5Name,
        _ => {}
    }
    let bytes = argv0.as_encoded_bytes();
    let base = match os {
        Os::Unix => bytes.rsplit(|b| *b == b'/').next(),
        Os::Windows => bytes.rsplit(|b| *b == b'/' || *b == b'\\').next(),
    }
    .unwrap_or_default();
    let is_k5 = match os {
        Os::Unix => base == b"k5",
        Os::Windows => {
            let lower = base.to_ascii_lowercase();
            lower.strip_suffix(b".exe").unwrap_or(&lower[..]) == b"k5"
        }
    };
    if is_k5 {
        CalledAs::K5Name
    } else {
        CalledAs::K2Name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CalledAs::{K2Name, K5Name};

    fn with(value: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
        move |name| {
            assert_eq!(name, CALLED_AS_VAR, "the rule reads no other variable");
            value.map(str::to_string)
        }
    }

    fn unix(argv0: &str, value: Option<&'static str>) -> CalledAs {
        called_as(OsStr::new(argv0), with(value), Os::Unix)
    }

    fn windows(argv0: &str, value: Option<&'static str>) -> CalledAs {
        called_as(OsStr::new(argv0), with(value), Os::Windows)
    }

    /// The cases of `tests/cli/called_as.sh`, and a few more.
    #[test]
    fn the_name_by_argv0_alone() {
        let rows: &[(&str, CalledAs)] = &[
            ("k2", K2Name),
            ("k5", K5Name),
            ("/usr/local/bin/k5", K5Name),
            ("/tmp/x/k2", K2Name),
            ("./k5", K5Name),
            ("k2so", K2Name),
            ("/tmp/x/some-copy", K2Name),
            ("", K2Name),
            ("/", K2Name),
            ("/usr/local/bin/k5/", K2Name),
            ("k5/k2", K2Name),
            ("K5", K2Name),
            ("k5.exe", K2Name),
            ("k5 ", K2Name),
            ("C:\\bin\\k5", K2Name),
        ];
        for (argv0, want) in rows {
            assert_eq!(unix(argv0, None), *want, "argv0 {argv0:?}");
        }
    }

    #[test]
    fn the_variable_wins_when_it_names_k2_or_k5() {
        let rows: &[(&str, Option<&'static str>, CalledAs)] = &[
            ("k2", Some("k5"), K5Name),
            ("k5", Some("k2"), K2Name),
            ("some-copy", Some("k5"), K5Name),
            ("k2so", Some("k5"), K5Name),
            ("k2", Some("k2"), K2Name),
            ("k5", Some("k5"), K5Name),
            // Ignored: empty, or anything but the two bare names.
            ("k5", Some(""), K5Name),
            ("k2", Some(""), K2Name),
            ("k5", Some("k2so"), K5Name),
            ("k2", Some("K5"), K2Name),
            ("k5", Some("k2 "), K5Name),
            ("k2", Some("/usr/local/bin/k5"), K2Name),
            ("some-copy", Some("k5-cli"), K2Name),
            ("k2", Some("k5.exe"), K2Name),
        ];
        for (argv0, value, want) in rows {
            assert_eq!(unix(argv0, *value), *want, "argv0 {argv0:?}, {CALLED_AS_VAR}={value:?}");
            assert_eq!(windows(argv0, *value), *want, "Windows: argv0 {argv0:?}, {CALLED_AS_VAR}={value:?}");
        }
    }

    #[test]
    fn on_windows_case_and_exe_are_ignored() {
        let rows: &[(&str, CalledAs)] = &[
            ("k5", K5Name),
            ("k5.exe", K5Name),
            ("K5.EXE", K5Name),
            ("K5", K5Name),
            ("k5.Exe", K5Name),
            ("C:\\Program Files\\K2\\k5.exe", K5Name),
            ("C:/tools/k5", K5Name),
            ("\\\\server\\share\\K5.exe", K5Name),
            ("k2.exe", K2Name),
            ("k2so.exe", K2Name),
            ("k5.exe.exe", K2Name),
            ("k5.com", K2Name),
            (".exe", K2Name),
            ("k5\\", K2Name),
            ("C:\\k5\\k2.exe", K2Name),
        ];
        for (argv0, want) in rows {
            assert_eq!(windows(argv0, None), *want, "Windows argv0 {argv0:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_argv0_that_is_not_utf8_is_read_by_its_bytes() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(called_as(OsStr::from_bytes(b"/opt/\xff/k5"), with(None), Os::Unix), K5Name);
        assert_eq!(called_as(OsStr::from_bytes(b"/opt/k5\xff"), with(None), Os::Unix), K2Name);
    }

    #[test]
    fn the_fallthrough_env_names_the_caller() {
        assert_eq!(
            K2Name.fallthrough_env(),
            [("K5_CLI_CALLED_AS", "k2"), ("K5_CLI_NOTICE_DONE", "1"), ("K5_CLI_VIA", "rust")]
        );
        assert_eq!(
            K5Name.fallthrough_env(),
            [("K5_CLI_CALLED_AS", "k5"), ("K5_CLI_NOTICE_DONE", "1"), ("K5_CLI_VIA", "rust")]
        );
        // What the front hands on, the shell reads back as the same name.
        for who in [K2Name, K5Name] {
            let env = who.fallthrough_env();
            let lookup = |name: &str| env.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
            assert_eq!(called_as(OsStr::new("some-copy"), lookup, Os::Unix), who);
        }
    }

    /// The front's variables: the three fixed ones, then the folder.
    #[test]
    fn the_front_hands_on_four_names() {
        let fixed: Vec<&str> = K2Name.fallthrough_env().iter().map(|(k, _)| *k).collect();
        assert_eq!(FRONT_VARS[..3], fixed[..]);
        assert_eq!(FRONT_VARS[3], "K5_CLI_SCRIPT_DIR");
    }

    /// The brand's command name is the k2 side today, and its aliases count
    /// as k2. When the rename moves `CLI_NAME`, this fails on purpose.
    #[test]
    fn the_brand_names_today() {
        assert_eq!(K2Name.name(), k2_client::brand::CLI_NAME);
        for alias in k2_client::brand::CLI_ALIASES {
            assert_eq!(unix(alias, None), K2Name, "alias {alias}");
            assert_eq!(windows(alias, None), K2Name, "alias {alias}");
        }
    }
}
