//! The name the command was called by, and what the front hands the shell
//! CLI when a verb falls through to it.
//!
//! The name matters only for the aliases the brand file asks for (Rosson's
//! rule 1): every line the command prints carries its own name, the
//! brand's, whatever it was called by. So the one question is "is this
//! name one of my aliases, and in which mode?", asked of a table of names
//! ([`Names`]): the command's own and its aliases with their modes. The
//! front asks it of the compiled table, the brand file's
//! ([`Names::COMPILED`], from `k2_client::brand`); the tests ask it of
//! tables of their own, so no brand is spelled here.
//!
//! The rule ([`called_as`]):
//!
//! 1. `K5_CLI_CALLED_AS` wins when it is exactly the command's name or one
//!    of its aliases. Any other value, empty included, is ignored.
//! 2. Otherwise the last part of `argv[0]` decides: an alias's name is
//!    that alias ([`CalledAs::Alias`]), and any other name (the command's
//!    own, a copy under another name, a test's path) is the command
//!    ([`CalledAs::Primary`]).
//!
//! On Windows the program is `<name>.exe`, and `argv[0]` holds what the
//! person typed (`xy`, `xy.exe`, `XY`, `C:\bin\XY.EXE`). File names there
//! ignore case and the shell finds `xy.exe` for `xy`, so on Windows the
//! name is compared without case and a trailing `.exe` is dropped first;
//! both `\` and `/` end a folder name. Elsewhere the comparison is byte for
//! byte: `XY` and `xy.exe` are other programs.
//!
//! An alias in mode `notice` or `tombstone` shows only when `argv[0]`
//! names the alias, so its link must point at the command itself, not at
//! a shared shim that runs the command under its own name. That is the
//! install's and the packaging's part, and a condition of the switch that
//! turns such an alias on.

use k2_client::brand::{self, AliasMode};
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
/// The variable that tells the shell the passport rule would refuse the
/// call (`crate::passport`): `1`, or not set at all. Its daemon requests
/// then carry `X-K5-Would-Refuse: 1`.
pub const WOULD_REFUSE_VAR: &str = "K5_CLI_WOULD_REFUSE";
/// The variable that tells the shell the folder of the program people ran
/// (links resolved), where it looks first for the files shipped beside
/// the CLI (`k2-approve`, `k2-widget-shot.mjs`). The front runs the script
/// as `/dev/fd/N`, so the folder of the shell's `$0` says nothing. It is a
/// path, so it is not in [`CalledAs::fallthrough_env`]; the front sets it
/// beside the three fixed ones ([`crate::fallthrough::command`]).
pub const SCRIPT_DIR_VAR: &str = "K5_CLI_SCRIPT_DIR";
/// Every variable the front hands the shell. Each is one name, not a
/// setting read under three.
pub const FRONT_VARS: [&str; 5] = [
    CALLED_AS_VAR,
    NOTICE_DONE_VAR,
    VIA_VAR,
    WOULD_REFUSE_VAR,
    SCRIPT_DIR_VAR,
];

/// The names a command answers to: its own, and its aliases, each with
/// what it does when the command is called by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Names {
    /// The command's own name.
    pub name: &'static str,
    /// The other names it answers to, each with its mode.
    pub aliases: &'static [(&'static str, AliasMode)],
}

impl Names {
    /// The brand file's: the command's name and its aliases, as compiled.
    pub const COMPILED: Names = Names {
        name: brand::CLI_NAME,
        aliases: brand::CLI_ALIAS_MODES,
    };

    /// The alias called `name` exactly, as [`CalledAs::Alias`].
    fn alias(&self, name: &[u8]) -> Option<CalledAs> {
        self.aliases
            .iter()
            .find(|(alias, _)| alias.as_bytes() == name)
            .map(|&(name, mode)| CalledAs::Alias { name, mode })
    }
}

/// Which of the command's names it was called by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalledAs {
    /// Its own name, or a name it does not know.
    Primary,
    /// One of its aliases, with the alias's mode.
    Alias {
        /// The alias, as the table spells it.
        name: &'static str,
        /// What a call by it does.
        mode: AliasMode,
    },
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
    /// The name this process was called by, from its `argv[0]`, its
    /// environment and the compiled table.
    pub fn current(argv0: &OsStr) -> CalledAs {
        called_as(
            argv0,
            |name| std::env::var(name).ok(),
            Os::CURRENT,
            &Names::COMPILED,
        )
    }

    /// What the front sets for the shell CLI when it hands a verb on:
    /// the name that matched (the command's own for [`CalledAs::Primary`]),
    /// that the notice is dealt with, and that the call came through the
    /// front.
    pub fn fallthrough_env(self) -> [(&'static str, &'static str); 3] {
        let name = match self {
            CalledAs::Primary => brand::CLI_NAME,
            CalledAs::Alias { name, .. } => name,
        };
        [
            (CALLED_AS_VAR, name),
            (NOTICE_DONE_VAR, "1"),
            (VIA_VAR, VIA_RUST),
        ]
    }
}

/// The rule, over `argv[0]`, a lookup of the environment, the system's
/// way with file names, and a table of names.
pub fn called_as(
    argv0: &OsStr,
    lookup: impl Fn(&str) -> Option<String>,
    os: Os,
    names: &Names,
) -> CalledAs {
    if let Some(value) = lookup(CALLED_AS_VAR) {
        if value == names.name {
            return CalledAs::Primary;
        }
        if let Some(alias) = names.alias(value.as_bytes()) {
            return alias;
        }
    }
    let bytes = argv0.as_encoded_bytes();
    let base = match os {
        Os::Unix => bytes.rsplit(|b| *b == b'/').next(),
        Os::Windows => bytes.rsplit(|b| *b == b'/' || *b == b'\\').next(),
    }
    .unwrap_or_default();
    let alias = match os {
        Os::Unix => names.alias(base),
        Os::Windows => {
            let lower = base.to_ascii_lowercase();
            names.alias(lower.strip_suffix(b".exe").unwrap_or(&lower[..]))
        }
    };
    alias.unwrap_or(CalledAs::Primary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use AliasMode::{Notice, Silent, Tombstone};
    use CalledAs::Primary;

    /// A brand with no aliases.
    const XY: Names = Names {
        name: "xy",
        aliases: &[],
    };

    /// The command after the rename: the old name in mode notice, the old
    /// shim's name silent.
    const K5: Names = Names {
        name: "k5",
        aliases: &[("k2", Notice), ("k2so", Silent)],
    };

    /// The release after that: the old name in mode tombstone.
    const K5_TOMBSTONE: Names = Names {
        name: "k5",
        aliases: &[("k2", Tombstone)],
    };

    const K2_NOTICE: CalledAs = CalledAs::Alias {
        name: "k2",
        mode: Notice,
    };
    const K2SO_SILENT: CalledAs = CalledAs::Alias {
        name: "k2so",
        mode: Silent,
    };

    fn with(value: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
        move |name| {
            assert_eq!(name, CALLED_AS_VAR, "the rule reads no other variable");
            value.map(str::to_string)
        }
    }

    fn unix(names: &Names, argv0: &str, value: Option<&'static str>) -> CalledAs {
        called_as(OsStr::new(argv0), with(value), Os::Unix, names)
    }

    fn windows(names: &Names, argv0: &str, value: Option<&'static str>) -> CalledAs {
        called_as(OsStr::new(argv0), with(value), Os::Windows, names)
    }

    /// No aliases: every name is the command, and the variable set to a
    /// name it does not know is ignored.
    #[test]
    fn with_no_aliases_every_name_is_the_command() {
        for argv0 in [
            "xy",
            "k2",
            "k5",
            "k2so",
            "foo",
            "/usr/local/bin/xy",
            "/tmp/x/some-copy",
            "",
        ] {
            assert_eq!(unix(&XY, argv0, None), Primary, "argv0 {argv0:?}");
            assert_eq!(windows(&XY, argv0, None), Primary, "Windows {argv0:?}");
            assert_eq!(unix(&XY, argv0, Some("k5")), Primary, "{argv0:?}, k5");
            assert_eq!(unix(&XY, argv0, Some("xy")), Primary, "{argv0:?}, xy");
        }
    }

    /// The cases of `tests/cli/called_as.sh`, and a few more, against a
    /// table with two aliases.
    #[test]
    fn the_name_by_argv0_alone() {
        let rows: &[(&str, CalledAs)] = &[
            ("k5", Primary),
            ("k2", K2_NOTICE),
            ("k2so", K2SO_SILENT),
            ("/usr/local/bin/k2", K2_NOTICE),
            ("/tmp/x/k5", Primary),
            ("./k2", K2_NOTICE),
            ("/opt/k2so", K2SO_SILENT),
            ("/tmp/x/some-copy", Primary),
            ("", Primary),
            ("/", Primary),
            ("/usr/local/bin/k2/", Primary),
            ("k2/k5", Primary),
            ("K2", Primary),
            ("k2.exe", Primary),
            ("k2 ", Primary),
            ("k2-cli", Primary),
            ("C:\\bin\\k2", Primary),
        ];
        for (argv0, want) in rows {
            assert_eq!(unix(&K5, argv0, None), *want, "argv0 {argv0:?}");
        }
    }

    #[test]
    fn the_variable_wins_when_it_names_the_command_or_an_alias() {
        let rows: &[(&str, Option<&'static str>, CalledAs)] = &[
            ("k2", Some("k5"), Primary),
            ("k5", Some("k2"), K2_NOTICE),
            ("some-copy", Some("k2"), K2_NOTICE),
            ("k2", Some("k2so"), K2SO_SILENT),
            ("k2so", Some("k5"), Primary),
            ("k2", Some("k2"), K2_NOTICE),
            ("k5", Some("k5"), Primary),
            // Ignored: empty, or anything but a bare name in the table.
            ("k2", Some(""), K2_NOTICE),
            ("k5", Some(""), Primary),
            ("k5", Some("xy"), Primary),
            ("k2", Some("K5"), K2_NOTICE),
            ("k5", Some("k2 "), Primary),
            ("k5", Some("/usr/local/bin/k2"), Primary),
            ("some-copy", Some("k2-cli"), Primary),
            ("k5", Some("k2.exe"), Primary),
            ("k5", Some("K2"), Primary),
        ];
        for (argv0, value, want) in rows {
            assert_eq!(
                unix(&K5, argv0, *value),
                *want,
                "argv0 {argv0:?}, {CALLED_AS_VAR}={value:?}"
            );
            assert_eq!(
                windows(&K5, argv0, *value),
                *want,
                "Windows: argv0 {argv0:?}, {CALLED_AS_VAR}={value:?}"
            );
        }
    }

    #[test]
    fn a_tombstone_alias_is_an_alias_in_that_mode() {
        let tomb = CalledAs::Alias {
            name: "k2",
            mode: Tombstone,
        };
        assert_eq!(unix(&K5_TOMBSTONE, "/usr/bin/k2", None), tomb);
        assert_eq!(windows(&K5_TOMBSTONE, "K2.EXE", None), tomb);
        assert_eq!(unix(&K5_TOMBSTONE, "k5", Some("k2")), tomb);
        assert_eq!(unix(&K5_TOMBSTONE, "k2so", None), Primary);
    }

    #[test]
    fn on_windows_case_and_exe_are_ignored() {
        let rows: &[(&str, CalledAs)] = &[
            ("k2", K2_NOTICE),
            ("k2.exe", K2_NOTICE),
            ("K2.EXE", K2_NOTICE),
            ("K2", K2_NOTICE),
            ("k2.Exe", K2_NOTICE),
            ("C:\\bin\\k2.exe", K2_NOTICE),
            ("C:\\Program Files\\K2\\k2.exe", K2_NOTICE),
            ("C:/tools/k2", K2_NOTICE),
            ("\\\\server\\share\\K2.exe", K2_NOTICE),
            ("K2SO.EXE", K2SO_SILENT),
            ("k5.exe", Primary),
            ("K5", Primary),
            ("k2.exe.exe", Primary),
            ("k2.com", Primary),
            (".exe", Primary),
            ("k2\\", Primary),
            ("C:\\k2\\k5.exe", Primary),
        ];
        for (argv0, want) in rows {
            assert_eq!(windows(&K5, argv0, None), *want, "Windows argv0 {argv0:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_argv0_that_is_not_utf8_is_read_by_its_bytes() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            called_as(
                OsStr::from_bytes(b"/opt/\xff/k2"),
                with(None),
                Os::Unix,
                &K5
            ),
            K2_NOTICE
        );
        assert_eq!(
            called_as(OsStr::from_bytes(b"/opt/k2\xff"), with(None), Os::Unix, &K5),
            Primary
        );
    }

    /// The compiled table: the command's own name is the command, and
    /// each alias is itself in its own mode. No brand is named here.
    #[test]
    fn the_compiled_table_is_the_brand_file_s() {
        let names = Names::COMPILED;
        assert_eq!(names.name, k2_client::brand::CLI_NAME);
        assert_eq!(names.aliases, k2_client::brand::CLI_ALIAS_MODES);
        assert_eq!(unix(&names, names.name, None), Primary);
        assert_eq!(windows(&names, names.name, None), Primary);
        for &(name, mode) in names.aliases {
            let want = CalledAs::Alias { name, mode };
            assert_eq!(unix(&names, name, None), want, "alias {name}");
            assert_eq!(
                windows(&names, &format!("{}.EXE", name.to_uppercase()), None),
                want,
                "alias {name}"
            );
        }
    }

    #[test]
    fn the_fallthrough_env_names_the_caller() {
        let alias = CalledAs::Alias {
            name: "ab1",
            mode: Silent,
        };
        assert_eq!(
            alias.fallthrough_env(),
            [
                ("K5_CLI_CALLED_AS", "ab1"),
                ("K5_CLI_NOTICE_DONE", "1"),
                ("K5_CLI_VIA", "rust")
            ]
        );
        assert_eq!(
            Primary.fallthrough_env()[0],
            ("K5_CLI_CALLED_AS", k2_client::brand::CLI_NAME)
        );
    }

    /// The front's variables: the three fixed ones, the passport rule's
    /// count, then the folder.
    #[test]
    fn the_front_hands_on_five_names() {
        let fixed: Vec<&str> = Primary.fallthrough_env().iter().map(|(k, _)| *k).collect();
        assert_eq!(FRONT_VARS[..3], fixed[..]);
        assert_eq!(FRONT_VARS[3], "K5_CLI_WOULD_REFUSE");
        assert_eq!(FRONT_VARS[4], "K5_CLI_SCRIPT_DIR");
    }
}
