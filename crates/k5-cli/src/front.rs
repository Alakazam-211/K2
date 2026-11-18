//! What the front does with a command line: answer it itself, or hand it
//! to the shell CLI.
//!
//! The route table ([`crate::route`]) says who answers. Its `Native` rows
//! are the version (alone, or exactly `--version --check`), `cli`
//! (`cli status [--json]` and its help) and the families in
//! [`crate::families`]. A form the front's code does not take falls
//! through, as does everything else, byte for byte ([`crate::fallthrough`]).
//!
//! A family's help is the shell's: a `whoami`, `done`, `respond`, `inbox`,
//! `read`, `activity`, `sessions`, `connections` or `feedback` (`tickets`)
//! command line with `-h` or `--help` anywhere in it falls through, so the
//! shell prints its help
//! text (or its error for a flag before it, or whatever it does with such
//! a word where it reads a value), with its connection gate in front of
//! it, exactly as it always has. No copy of the text lives here.
//!
//! A study page ([`Route::Study`], when the `study` row is `Native`) and
//! a help form the shell only prints a file for ([`Route::Help`],
//! [`help_route`]) are pages: the binary prints them when the shell's
//! environment lets it ([`crate::pages`]), and hands them over otherwise.

use crate::pages::{self, Study};
use crate::route::{self, Noun, Owner};
use crate::text::{self, HelpForm};
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
    /// `done` with no `-h` or `--help` in it ([`crate::families::done`],
    /// which may still hand it to the shell).
    Done,
    /// `respond` with no `-h` or `--help` in it
    /// ([`crate::families::respond`], which may still hand it to the
    /// shell).
    Respond,
    /// `inbox` with no `-h` or `--help` in it ([`crate::families::inbox`],
    /// which may still hand it to the shell).
    Inbox,
    /// `read` with no `-h` or `--help` in it ([`crate::families::read`],
    /// which may still hand it to the shell).
    Read,
    /// `activity` with no `-h` or `--help` in it
    /// ([`crate::families::activity`], which may still hand it to the
    /// shell).
    Activity,
    /// `sessions` with no `-h` or `--help` in it
    /// ([`crate::families::sessions`], which may still hand it to the
    /// shell).
    Sessions,
    /// `connections` with a verb the route table sends here and no `-h`
    /// or `--help` in it ([`crate::families::connections`], which may
    /// still hand it to the shell).
    Connections,
    /// `feedback` or `tickets` with a verb the route table sends here and
    /// no `-h` or `--help` in it ([`crate::families::feedback`], which may
    /// still hand it to the shell).
    Feedback,
    /// A `k2 study` page ([`pages::study`]).
    Study(Study),
    /// A help form whose page is a file ([`text::HELP_FORMS`]).
    Help(&'static HelpForm),
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
    if noun.names("study") {
        return pages::study(words.get(1..).unwrap_or_default()).map_or(Route::Fallthrough, Route::Study);
    }
    if noun.names("cli") {
        return match words.get(1..).unwrap_or_default() {
            [] | ["--help" | "-h" | "help"] | ["status", "--help" | "-h"] => Route::CliHelp,
            ["status"] => Route::CliStatus { json: false },
            ["status", "--json"] => Route::CliStatus { json: true },
            _ => Route::Fallthrough,
        };
    }
    let help = words.iter().any(|w| matches!(*w, "-h" | "--help"));
    if noun.names("whoami") {
        return if help {
            Route::Fallthrough
        } else {
            Route::Whoami
        };
    }
    if noun.names("done") {
        return if help {
            Route::Fallthrough
        } else {
            Route::Done
        };
    }
    if noun.names("respond") {
        return if help {
            Route::Fallthrough
        } else {
            Route::Respond
        };
    }
    if noun.names("inbox") {
        return if help {
            Route::Fallthrough
        } else {
            Route::Inbox
        };
    }
    if noun.names("read") {
        return if help {
            Route::Fallthrough
        } else {
            Route::Read
        };
    }
    for (word, route) in [
        ("activity", Route::Activity),
        ("sessions", Route::Sessions),
        ("connections", Route::Connections),
        ("feedback", Route::Feedback),
    ] {
        if noun.names(word) {
            return if help { Route::Fallthrough } else { route };
        }
    }
    Route::Fallthrough
}

/// The help form `args` is, when the shell only prints a file for it
/// ([`text::help_form`]).
pub fn help_route(args: &[OsString]) -> Option<&'static HelpForm> {
    let words: Option<Vec<&str>> = args.iter().map(|a| a.to_str()).collect();
    text::help_form(&words?)
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
        assert_eq!(
            route(&args(&["cli", "status"])),
            Route::CliStatus { json: false }
        );
        assert_eq!(
            route(&args(&["cli", "status", "--json"])),
            Route::CliStatus { json: true }
        );
        for help in [
            &["cli"][..],
            &["cli", "--help"],
            &["cli", "-h"],
            &["cli", "help"],
            &["cli", "status", "--help"],
            &["cli", "status", "-h"],
        ] {
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
        for line in [
            &["whoami"][..],
            &["whoami", "--json"],
            &["whoami", "extra"],
            &["whoami", "--bogus"],
            &["whoami", "help"],
            &["whoami", "-help"],
        ] {
            assert_eq!(route(&args(line)), Route::Whoami, "{line:?}");
        }
        for line in [
            &["whoami", "--help"][..],
            &["whoami", "-h"],
            &["whoami", "--json", "-h"],
            &["whoami", "--bogus", "--help"],
            &["whoami", "x", "--help"],
        ] {
            assert_eq!(route(&args(line)), Route::Fallthrough, "{line:?}");
        }
        assert_eq!(route(&args(&["Whoami"])), Route::Fallthrough);
    }

    /// `done`'s front code: any `-h` or `--help` word goes to the shell,
    /// even as a flag's value. Whether the row is the binary's is the
    /// route table's (`native` reads only the words).
    #[test]
    fn the_done_forms() {
        let done = route::TABLE
            .iter()
            .find(|n| n.names("done"))
            .expect("a done row");
        for line in [
            &["done"][..],
            &["done", "--blocked", "x"],
            &["done", "--agent", "a"],
            &["done", "--bogus"],
            &["done", "help"],
        ] {
            assert_eq!(native(done, &args(line)), Route::Done, "{line:?}");
        }
        for line in [
            &["done", "--help"][..],
            &["done", "-h"],
            &["done", "--blocked", "--help"],
            &["done", "--agent", "-h"],
        ] {
            assert_eq!(native(done, &args(line)), Route::Fallthrough, "{line:?}");
        }
    }

    /// `respond`'s front code, likewise: `respond -- --help` sends the
    /// word `--help` in the shell, and goes there.
    #[test]
    fn the_respond_forms() {
        let respond = route::TABLE
            .iter()
            .find(|n| n.names("respond"))
            .expect("a respond row");
        for line in [
            &["respond"][..],
            &["respond", "hi"],
            &["respond", "--final", "x"],
            &["respond", "help"],
        ] {
            assert_eq!(native(respond, &args(line)), Route::Respond, "{line:?}");
        }
        for line in [
            &["respond", "--help"][..],
            &["respond", "-h"],
            &["respond", "hi", "--help"],
            &["respond", "--", "-h"],
        ] {
            assert_eq!(native(respond, &args(line)), Route::Fallthrough, "{line:?}");
        }
    }

    /// `inbox`'s front code, likewise: any `-h` or `--help` word goes to
    /// the shell (`inbox read --help` reads the item `--help` there).
    #[test]
    fn the_inbox_forms() {
        let inbox = route::TABLE
            .iter()
            .find(|n| n.names("inbox"))
            .expect("an inbox row");
        for line in [
            &["inbox"][..],
            &["inbox", "list"],
            &["inbox", "read", "a1"],
            &["inbox", "compose", "--title", "t"],
            &["inbox", "help"],
        ] {
            assert_eq!(native(inbox, &args(line)), Route::Inbox, "{line:?}");
        }
        for line in [
            &["inbox", "--help"][..],
            &["inbox", "-h"],
            &["inbox", "read", "--help"],
            &["inbox", "list", "--folder", "-h"],
        ] {
            assert_eq!(native(inbox, &args(line)), Route::Fallthrough, "{line:?}");
        }
    }

    /// `read`'s front code, likewise (the shell reads `read --help` as the
    /// workspace `--help`).
    #[test]
    fn the_read_forms() {
        let read = route::TABLE
            .iter()
            .find(|n| n.names("read"))
            .expect("a read row");
        for line in [
            &["read"][..],
            &["read", "ws"],
            &["read", "ws", "--lines", "5"],
            &["read", "help"],
        ] {
            assert_eq!(native(read, &args(line)), Route::Read, "{line:?}");
        }
        for line in [
            &["read", "--help"][..],
            &["read", "-h"],
            &["read", "ws", "--agent", "--help"],
        ] {
            assert_eq!(native(read, &args(line)), Route::Fallthrough, "{line:?}");
        }
    }

    /// `activity`, `sessions` and `connections`, likewise: any `-h` or
    /// `--help` word goes to the shell.
    #[test]
    fn the_r3_7_forms() {
        let row = |w: &str| {
            route::TABLE
                .iter()
                .find(|n| n.names(w))
                .unwrap_or_else(|| panic!("a {w} row"))
        };
        for (w, want, lines) in [
            (
                "activity",
                Route::Activity,
                &[&["activity"][..], &["activity", "5", "--json"]][..],
            ),
            (
                "sessions",
                Route::Sessions,
                &[
                    &["sessions"][..],
                    &["sessions", "--past"],
                    &["sessions", "show", "x"],
                ],
            ),
            (
                "connections",
                Route::Connections,
                &[&["connections", "list"][..], &["connections", "--json"]],
            ),
        ] {
            for line in lines {
                assert_eq!(native(row(w), &args(line)), want, "{line:?}");
            }
            for line in [&[w, "--help"][..], &[w, "list", "-h"], &[w, "x", "--help"]] {
                assert_eq!(native(row(w), &args(line)), Route::Fallthrough, "{line:?}");
            }
        }
    }

    /// `feedback` and `tickets` (one row), likewise.
    #[test]
    fn the_r3_8_forms() {
        let row = route::TABLE
            .iter()
            .find(|n| n.names("feedback"))
            .expect("a feedback row");
        assert!(row.names("tickets"));
        for line in [
            &["feedback", "list"][..],
            &["tickets", "list", "--json"],
            &["feedback", "show", "x"],
            &["tickets", "show", "x", "--html"],
        ] {
            assert_eq!(native(row, &args(line)), Route::Feedback, "{line:?}");
        }
        for line in [
            &["feedback", "list", "--help"][..],
            &["tickets", "show", "x", "-h"],
            &["feedback", "--help"],
        ] {
            assert_eq!(native(row, &args(line)), Route::Fallthrough, "{line:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_argument_that_is_not_utf8_falls_through() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(
            route(&[OsString::from_vec(b"--version\xff".to_vec())]),
            Route::Fallthrough
        );
        let odd = OsString::from_vec(b"st\xffatus".to_vec());
        assert_eq!(route(&[OsString::from("cli"), odd]), Route::Fallthrough);
        assert_eq!(
            route(&[
                OsString::from("whoami"),
                OsString::from_vec(b"--json\xff".to_vec())
            ]),
            Route::Fallthrough
        );
        assert_eq!(
            route(&[
                OsString::from("--version"),
                OsString::from_vec(b"--check\xff".to_vec())
            ]),
            Route::Fallthrough
        );
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
                    assert_ne!(
                        route(&args(&[*word])),
                        Route::Fallthrough,
                        "{word} is Native but falls through"
                    );
                }
            }
            for verb in noun.verbs.iter().filter(|v| v.owner == Owner::Native) {
                for word in verb.words {
                    assert_ne!(
                        route(&args(&[noun.words[0], *word])),
                        Route::Fallthrough,
                        "{:?} {word}",
                        noun.words
                    );
                }
            }
        }
    }

    #[test]
    fn the_line_says_k2() {
        assert_eq!(version_line("0.46.14"), "k2 0.46.14\n");
    }
}
