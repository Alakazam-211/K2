//! What the front does with a command line: answer it itself, or hand it
//! to the shell CLI.
//!
//! The route table ([`crate::route`]) says who answers. Its `Native` rows
//! are the version (alone, or exactly `--version --check`), `cli`
//! (`cli status [--json]` and its help) and `whoami`
//! ([`crate::families::whoami`]). A form the front's code does not take
//! falls through, as does everything else, byte for byte
//! ([`crate::fallthrough`]).
//!
//! `whoami`'s help is the shell's: a `whoami` command line with `-h` or
//! `--help` anywhere in it falls through, so the shell prints its help
//! text (or its error for a flag before it), with its connection gate in
//! front of it, exactly as it always has. No copy of the text lives here.

use crate::route::{self, Noun, Owner};
use std::ffi::OsString;

/// Where a command line goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `--version`, `-v` or `version`, with nothing after it.
    Version,
    /// Exactly `--version --check`: the version, then the server's
    /// ([`crate::check`]).
    VersionCheck,
    /// `cli status`, `cli status --json` ([`crate::cli_status`]).
    CliStatus { json: bool },
    /// `cli` alone, `cli --help`, `cli -h`, `cli help`, and `cli status`
    /// with `--help` or `-h`.
    CliHelp,
    /// `whoami` with no `-h` or `--help` in it ([`crate::families::whoami`]).
    Whoami,
    /// Anything else: the shell CLI runs it.
    Fallthrough,
}

/// The route for the arguments after `argv[0]`.
pub fn route(args: &[OsString]) -> Route {
    let found = route::find(route::TABLE, args);
    match (found.owner, found.noun) {
        (Owner::Native, Some(noun)) => native(noun, args),
        // Nothing is `Spec` yet: there is no runner for it.
        _ => Route::Fallthrough,
    }
}

/// The front's own code for a `Native` row. A form it does not take
/// falls through.
fn native(noun: &Noun, args: &[OsString]) -> Route {
    let words: Option<Vec<&str>> = args.iter().map(|a| a.to_str()).collect();
    let Some(words) = words else {
        return Route::Fallthrough;
    };
    if noun.names("version") {
        return match words.as_slice() {
            [_] => Route::Version,
            ["--version", "--check"] => Route::VersionCheck,
            _ => Route::Fallthrough,
        };
    }
    if noun.names("cli") {
        return match words.get(1..).unwrap_or_default() {
            [] | ["--help" | "-h" | "help"] | ["status", "--help" | "-h"] => Route::CliHelp,
            ["status"] => Route::CliStatus { json: false },
            ["status", "--json"] => Route::CliStatus { json: true },
            _ => Route::Fallthrough,
        };
    }
    if noun.names("whoami") {
        return if words.iter().any(|w| matches!(*w, "-h" | "--help")) { Route::Fallthrough } else { Route::Whoami };
    }
    Route::Fallthrough
}

/// What the version prints: the shell CLI's line, `k2 <version>`
/// (`cli/src/router.sh`, `echo "k2 $K2_CLI_VERSION"`). It says `k2` under
/// either name, as the shell does.
pub fn version_line(version: &str) -> String {
    format!("k2 {version}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_a_bare_version_is_answered_here() {
        for one in ["--version", "-v", "version"] {
            assert_eq!(route(&args(&[one])), Route::Version, "{one}");
        }
        let rows: &[&[&str]] = &[
            &[],
            &["help"],
            &["--help"],
            &["-V"],
            &["--Version"],
            &["versions"],
            &["--version="],
            &[" --version"],
            &["--version", "--json"],
            &["--version", "--check", "x"],
            &["-v", "--check"],
            &["version", "--check"],
            &["--check", "--version"],
            &["--version", "--Check"],
            &["version", "x"],
            &["-v", ""],
            &["--json", "--version"],
            &["msg", "--version"],
            &[""],
        ];
        for row in rows {
            assert_eq!(route(&args(row)), Route::Fallthrough, "{row:?}");
        }
    }

    #[test]
    fn only_the_exact_check_pair_asks_the_server() {
        assert_eq!(route(&args(&["--version", "--check"])), Route::VersionCheck);
    }

    #[test]
    fn the_cli_forms() {
        assert_eq!(route(&args(&["cli", "status"])), Route::CliStatus { json: false });
        assert_eq!(route(&args(&["cli", "status", "--json"])), Route::CliStatus { json: true });
        for help in [&["cli"][..], &["cli", "--help"], &["cli", "-h"], &["cli", "help"], &["cli", "status", "--help"], &["cli", "status", "-h"]] {
            assert_eq!(route(&args(help)), Route::CliHelp, "{help:?}");
        }
        // Any other form is the shell's: its `cli` arm points here.
        for other in [
            &["cli", "status", "--json", "x"][..],
            &["cli", "--json", "status"],
            &["cli", "status", "--bogus"],
            &["cli", "usage"],
            &["cli", ""],
            &["cli", "Status"],
        ] {
            assert_eq!(route(&args(other)), Route::Fallthrough, "{other:?}");
        }
    }

    /// `whoami` is the binary's, but for its help: any `-h` or `--help`
    /// word sends the whole line to the shell.
    #[test]
    fn the_whoami_forms() {
        for line in [&["whoami"][..], &["whoami", "--json"], &["whoami", "extra"], &["whoami", "--bogus"], &["whoami", "help"], &["whoami", "-help"]] {
            assert_eq!(route(&args(line)), Route::Whoami, "{line:?}");
        }
        for line in [&["whoami", "--help"][..], &["whoami", "-h"], &["whoami", "--json", "-h"], &["whoami", "--bogus", "--help"], &["whoami", "x", "--help"]] {
            assert_eq!(route(&args(line)), Route::Fallthrough, "{line:?}");
        }
        assert_eq!(route(&args(&["Whoami"])), Route::Fallthrough);
    }

    #[cfg(unix)]
    #[test]
    fn an_argument_that_is_not_utf8_falls_through() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(route(&[OsString::from_vec(b"--version\xff".to_vec())]), Route::Fallthrough);
        let odd = OsString::from_vec(b"st\xffatus".to_vec());
        assert_eq!(route(&[OsString::from("cli"), odd]), Route::Fallthrough);
        assert_eq!(route(&[OsString::from("whoami"), OsString::from_vec(b"--json\xff".to_vec())]), Route::Fallthrough);
        assert_eq!(route(&[OsString::from("--version"), OsString::from_vec(b"--check\xff".to_vec())]), Route::Fallthrough);
    }

    /// A `Native` row is one the front has code for, under each of its
    /// words; and no row is `Spec` while there is no runner for it.
    #[test]
    fn every_native_row_is_answered_here() {
        for noun in route::TABLE {
            let mut owners = std::iter::once(noun.owner).chain(noun.verbs.iter().map(|v| v.owner));
            assert!(owners.all(|o| o != Owner::Spec), "{:?} is Spec", noun.words);
            if noun.owner == Owner::Native {
                for word in noun.words {
                    assert_ne!(route(&args(&[*word])), Route::Fallthrough, "{word} is Native but falls through");
                }
            }
            for verb in noun.verbs.iter().filter(|v| v.owner == Owner::Native) {
                for word in verb.words {
                    assert_ne!(route(&args(&[noun.words[0], *word])), Route::Fallthrough, "{:?} {word}", noun.words);
                }
            }
        }
    }

    #[test]
    fn the_line_says_k2() {
        assert_eq!(version_line("0.46.14"), "k2 0.46.14\n");
    }
}
