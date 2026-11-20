//! What the front does with a command line: answer it itself, or hand it
//! to the shell CLI.
//!
//! The route table ([`crate::route`]) says who answers. Its `Native` rows
//! are the version (alone, or exactly `--version --check`), `cli`
//! (`cli status [--json]` and its help), `study`, and the families in
//! [`crate::families`], each run by its row's code ([`Route::Family`]). A
//! form the front's code does not take falls through, as does everything
//! else, byte for byte ([`crate::fallthrough`]).
//!
//! A family's help never reaches the family: a command line of a `Native`
//! row or verb row with `-h` or
//! `--help` anywhere in it (`setup`: or the word `help`; `vault run`: only
//! before a bare `--`, the rest being the child's) is a help page
//! when it is exactly a form the shell only prints a file for
//! ([`Route::Help`], [`help_route`]), and falls through
//! otherwise, so the shell prints its help text (or its error for a flag
//! before it, or whatever it does with such a word where it reads a
//! value), with its connection gate in front of it, exactly as it always
//! has. No copy of the text lives here: a page is the shell's own file.
//!
//! A family's bare line (the first word alone, or it and `--json`) that
//! its bare row sends to the binary ([`route::Verb::bare`]) is the
//! family's too, unless it is a page the front puts together
//! ([`Route::Composed`]: `chat` alone): that page stays the front's.
//!
//! A `Spec` verb row's command line is its spec verb ([`Route::Spec`]),
//! run from its `--schema` entry ([`crate::spec`]), unless it has `-h` or
//! `--help` in it (a page or the shell's, as for a family) or no spec row
//! starts it (the shell's).
//!
//! A study page ([`Route::Study`], when the `study` row is `Native`), a
//! help form the shell only prints a file for ([`Route::Help`],
//! [`help_route`]), the daily tier of the help (`k2`, `k2 help`,
//! `k2 --help`: [`Route::Daily`]), the other help pages bash puts
//! together from pieces ([`Route::Composed`]) and a first word the router
//! has no arm for ([`Route::Unknown`]: a retired verb's line, or
//! `Unknown command` and the daily tier) are pages: the binary prints them
//! when the shell's environment lets it ([`crate::pages`]), and hands them
//! over otherwise.

use crate::pages::{self, Composed, Study};
use crate::route::{self, Noun, Owner};
use crate::spec::{self, SpecVerb};
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
    /// A family's command line: a `Native` row, or a `Native` verb row,
    /// with no `-h` or `--help` in it (`setup`: nor the word `help`). The
    /// row's [`Noun::run`] answers it, and may still hand it to the shell.
    Family(&'static Noun),
    /// A spec verb: a `Spec` verb row, the row of [`spec::table::SPEC`]
    /// for its words, and no `-h` or `--help` in it ([`spec::run`], which
    /// may still hand it to the shell).
    Spec(&'static SpecVerb),
    /// A `k2 study` page ([`pages::study`]).
    Study(Study),
    /// A help form whose page is a file ([`text::HELP_FORMS`]).
    Help(&'static HelpForm),
    /// `k2`, `k2 help` or `k2 --help`: the daily tier of the help
    /// ([`pages::daily_page`]).
    Daily,
    /// A help page bash puts together from pieces ([`pages::composed`]).
    Composed(Composed),
    /// A first word the router has no arm for ([`pages::unknown_page`]).
    Unknown,
    /// Anything else: the shell CLI runs it.
    Fallthrough,
}

/// The route for the arguments after `argv[0]`.
pub fn route(args: &[OsString]) -> Route {
    let found = route::find(route::TABLE, args);
    // A page the front puts together stays the front's (below), even when
    // a bare row sends its line to the family.
    let page = found.by_bare && composed_line(args);
    match (found.owner, found.noun) {
        (Owner::Native, Some(noun)) if !page => native(noun, args),
        (Owner::Spec, Some(_)) => spec_route(spec::table::SPEC, args),
        // The daily tier and a help form the shell only prints a file for
        // are pages.
        _ if daily_route(args) => Route::Daily,
        (_, noun) => {
            let words: Option<Vec<&str>> = args.iter().map(|a| a.to_str()).collect();
            let Some(words) = words else {
                return Route::Fallthrough;
            };
            if let Some(c) = pages::composed(&words) {
                return Route::Composed(c);
            }
            if let Some(form) = text::help_form(&words) {
                return Route::Help(form);
            }
            // The router's `*)` arm: a first word no row names.
            if noun.is_none() && words.first().is_some_and(|w| !w.is_empty()) {
                return Route::Unknown;
            }
            Route::Fallthrough
        }
    }
}

/// Whether `args` is a page the front puts together ([`pages::composed`]).
fn composed_line(args: &[OsString]) -> bool {
    let words: Option<Vec<&str>> = args.iter().map(|a| a.to_str()).collect();
    words.is_some_and(|w| pages::composed(&w).is_some())
}

/// A `Spec` row's command line: its row of `table` (the longest whose
/// words start the line), or the shell's when it has none. Its help is a
/// page or the shell's, as a family's is.
fn spec_route(table: &'static [SpecVerb], args: &[OsString]) -> Route {
    let words: Option<Vec<&str>> = args.iter().map(|a| a.to_str()).collect();
    let Some(words) = words else {
        return Route::Fallthrough;
    };
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return help_route(args).map_or(Route::Fallthrough, Route::Help);
    }
    spec::find(table, &words).map_or(Route::Fallthrough, Route::Spec)
}

/// What is wrong between a route table and a spec table: a whole row that
/// is `Spec` (only a verb row may be), a `Spec` verb word with no spec row
/// for it, or a spec row the route table does not send to `Spec`. Empty
/// when they agree.
pub fn spec_problems(nouns: &'static [Noun], table: &[SpecVerb]) -> Vec<String> {
    let mut out = Vec::new();
    for noun in nouns {
        if noun.owner == Owner::Spec {
            out.push(format!("{:?} is Spec as a whole row", noun.words));
        }
        for verb in noun.verbs.iter().filter(|v| v.owner == Owner::Spec) {
            for word in verb.words {
                let line = [noun.words[0], *word];
                if !table
                    .iter()
                    .any(|r| r.words.len() >= 2 && r.words[..2] == line)
                {
                    out.push(format!("{line:?} is a Spec verb row with no spec row"));
                }
            }
        }
    }
    for row in table {
        let args: Vec<OsString> = row.words.iter().map(OsString::from).collect();
        if route::find(nouns, &args).owner != Owner::Spec {
            out.push(format!(
                "the spec row {:?} is not a Spec verb row",
                row.entry
            ));
        }
    }
    out
}

/// Whether `args` prints the daily tier ([`pages::DAILY_FORMS`]).
fn daily_route(args: &[OsString]) -> bool {
    let words: Option<Vec<&str>> = args.iter().map(|a| a.to_str()).collect();
    words.is_some_and(|w| pages::daily(&w))
}

/// The front's own code for a `Native` row. A form it does not take
/// falls through.
fn native(noun: &'static Noun, args: &[OsString]) -> Route {
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
        return pages::study(words.get(1..).unwrap_or_default())
            .map_or(Route::Fallthrough, Route::Study);
    }
    if noun.names("cli") {
        return match words.get(1..).unwrap_or_default() {
            [] | ["--help" | "-h" | "help"] | ["status", "--help" | "-h"] => Route::CliHelp,
            ["status"] => Route::CliStatus { json: false },
            ["status", "--json"] => Route::CliStatus { json: true },
            _ => Route::Fallthrough,
        };
    }
    // A family's help is a page or the shell's, never the family's.
    if help_words(&words).any(|w| matches!(*w, "-h" | "--help")) {
        return help_route(args).map_or(Route::Fallthrough, Route::Help);
    }
    // `cmd_setup`'s loop reads `help` as `--help`.
    if noun.names("setup") && words.iter().skip(1).any(|w| *w == "help") {
        return help_route(args).map_or(Route::Fallthrough, Route::Help);
    }
    Route::Family(noun)
}

/// The words of a line the front reads for `-h` and `--help`. Words after
/// a bare `--` belong to a child command and are never help flags, where
/// the shell reads them so: `vault run` (`cmd_vault` hands `run` its words
/// before its help scan, and `_vault_run` stops at `--`). Every other verb
/// is scanned whole, as the shell's `_wants_help "$@"` scans it, `--` and
/// all (a bash bug where the verb takes a command after `--`, as `compute
/// run` and `job` do; copied until the shell's fix lands).
fn help_words<'a>(words: &'a [&'a str]) -> impl Iterator<Item = &'a &'a str> + 'a {
    let stops_at_dashes = matches!(words, ["vault", "run", ..]);
    let end = if stops_at_dashes {
        words.iter().position(|w| *w == "--").unwrap_or(words.len())
    } else {
        words.len()
    };
    words[..end].iter()
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

    /// A family's route: its row of the table.
    fn family(name: &str) -> Route {
        Route::Family(
            route::TABLE
                .iter()
                .find(|n| n.family == name)
                .unwrap_or_else(|| panic!("a {name} row")),
        )
    }

    #[test]
    fn only_a_bare_version_is_answered_here() {
        for one in ["--version", "-v", "version"] {
            assert_eq!(route(&args(&[one])), Route::Version, "{one}");
        }
        let rows: &[&[&str]] = &[
            &["-h"],
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
            let r = route(&args(row));
            assert!(
                !matches!(r, Route::Version | Route::VersionCheck),
                "{row:?}: {r:?}"
            );
        }
        assert_eq!(route(&args(&["--version", "--json"])), Route::Fallthrough);
        assert_eq!(route(&args(&["-V"])), Route::Unknown, "no row has -V");
    }

    /// `k2`, `k2 help` and `k2 --help` are the daily tier; the other
    /// forms that print it stay the shell's.
    #[test]
    fn the_daily_forms() {
        for daily in [&[][..], &["help"], &["--help"]] {
            assert_eq!(route(&args(daily)), Route::Daily, "{daily:?}");
        }
        // The other forms that print the tier are composed pages.
        for other in [
            &["-h"][..],
            &[""],
            &["help", "nope"],
            &["--help", "nope"],
            &["-h", "nope"],
            &["help", ""],
            &["help", "app"],
        ] {
            assert_eq!(
                route(&args(other)),
                Route::Composed(Composed::Daily),
                "{other:?}"
            );
        }
        for shell in [&["help", "nope", "x"][..], &["", "x"], &["-h", "-h", "x"]] {
            assert_eq!(route(&args(shell)), Route::Fallthrough, "{shell:?}");
        }
        assert_eq!(
            route(&args(&["help", "mail"])),
            Route::Help(text::help_form(&["help", "mail"]).unwrap())
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let not_utf8 = vec![OsString::from_vec(vec![b'h', 0xff])];
            assert_eq!(route(&not_utf8), Route::Fallthrough);
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

    /// `whoami` is the binary's, but for its help: a line with an `-h` or
    /// `--help` word is a help page when it is exactly a help form, and
    /// the shell's otherwise.
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
            assert_eq!(route(&args(line)), family("whoami"), "{line:?}");
        }
        // Exactly a help form: the shell's own file, as a page.
        for line in [&["whoami", "--help"][..], &["whoami", "-h"]] {
            assert!(
                matches!(route(&args(line)), Route::Help(f) if f.page == "whoami"),
                "{line:?}"
            );
        }
        for line in [
            &["whoami", "--json", "-h"][..],
            &["whoami", "--bogus", "--help"],
            &["whoami", "x", "--help"],
        ] {
            assert_eq!(route(&args(line)), Route::Fallthrough, "{line:?}");
        }
        assert_eq!(route(&args(&["Whoami"])), Route::Unknown);
    }

    /// Words after a bare `--` are a child command's, never help flags,
    /// where the shell reads them so (`vault run`); other verbs are
    /// scanned whole, as the shell's `_wants_help` does.
    #[test]
    fn help_stops_at_dashes_for_vault_run() {
        assert_eq!(
            route(&args(&[
                "vault", "run", "--secret", "A", "--", "grep", "--help"
            ])),
            family("vault")
        );
        assert_eq!(
            route(&args(&["vault", "run", "--secret", "A", "--", "ls", "-h"])),
            family("vault")
        );
        // Before the `--`: still help.
        assert_eq!(
            route(&args(&["vault", "run", "--help", "--", "grep"])),
            help_route(&args(&["vault", "run", "--help", "--", "grep"]))
                .map_or(Route::Fallthrough, Route::Help)
        );
        assert_ne!(
            route(&args(&["vault", "run", "-h", "--", "grep"])),
            family("vault")
        );
        // Other verbs: the shell's scan reads past `--`.
        assert_ne!(
            route(&args(&["vault", "fetch", "--", "--help"])),
            family("vault")
        );
        assert_ne!(
            route(&args(&["compute", "run", "--", "grep", "--help"])),
            family("compute")
        );
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
            assert_eq!(native(done, &args(line)), family("done"), "{line:?}");
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
            assert_eq!(native(respond, &args(line)), family("respond"), "{line:?}");
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

    /// `inbox`'s front code, likewise: an `-h` or `--help` word never
    /// reaches the family (`inbox read --help` reads the item `--help` in
    /// the shell).
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
            assert_eq!(native(inbox, &args(line)), family("inbox"), "{line:?}");
        }
        for line in [&["inbox", "--help"][..], &["inbox", "-h"]] {
            assert!(
                matches!(native(inbox, &args(line)), Route::Help(f) if f.page == "inbox"),
                "{line:?}"
            );
        }
        for line in [
            &["inbox", "read", "--help"][..],
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
            assert_eq!(native(read, &args(line)), family("read"), "{line:?}");
        }
        for line in [
            &["read", "--help"][..],
            &["read", "-h"],
            &["read", "ws", "--agent", "--help"],
        ] {
            assert_eq!(native(read, &args(line)), Route::Fallthrough, "{line:?}");
        }
    }

    /// `activity`, `sessions` and `connections`, likewise: an `-h` or
    /// `--help` word never reaches the family.
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
                family("activity"),
                &[&["activity"][..], &["activity", "5", "--json"]][..],
            ),
            (
                "sessions",
                family("sessions"),
                &[
                    &["sessions"][..],
                    &["sessions", "--past"],
                    &["sessions", "show", "x"],
                ],
            ),
            (
                "connections",
                family("connections"),
                &[&["connections", "list"][..], &["connections", "--json"]],
            ),
        ] {
            for line in lines {
                assert_eq!(native(row(w), &args(line)), want, "{line:?}");
            }
            let page = native(row(w), &args(&[w, "--help"]));
            assert!(
                matches!(page, Route::Help(f) if f.page == w),
                "{w} --help: {page:?}"
            );
            for line in [&[w, "list", "-h"][..], &[w, "x", "--help"]] {
                assert_eq!(native(row(w), &args(line)), Route::Fallthrough, "{line:?}");
            }
        }
    }

    /// `update`, likewise: `-h` or `--help` anywhere is the shell's
    /// usage (`_k2_update_form`), so the line goes to the shell.
    #[test]
    fn the_update_forms() {
        let row = route::TABLE
            .iter()
            .find(|n| n.names("update"))
            .expect("an update row");
        for line in [
            &["update"][..],
            &["update", "--cli"],
            &["update", "--cli", "--check"],
            &["update", "--list"],
            &["update", "--app", "x"],
            &["update", "help"],
        ] {
            assert_eq!(native(row, &args(line)), family("update"), "{line:?}");
        }
        for line in [
            &["update", "--help"][..],
            &["update", "-h"],
            &["update", "--cli", "--help"],
            &["update", "--list", "-h"],
        ] {
            assert_eq!(native(row, &args(line)), Route::Fallthrough, "{line:?}");
        }
    }

    /// `setup`: `-h`, `--help` and the word `help` anywhere are a help
    /// page or the shell's; every other line is the family's.
    #[test]
    fn the_setup_forms() {
        let row = route::TABLE
            .iter()
            .find(|n| n.names("setup"))
            .expect("a setup row");
        for line in [
            &["setup"][..],
            &["setup", "--json"],
            &["setup", "skip", "dev_tools"],
            &["setup", "kessel", "--name", "x"],
            &["setup", "--bogus"],
        ] {
            assert_eq!(native(row, &args(line)), family("setup"), "{line:?}");
        }
        for page in [&["setup", "--help"][..], &["setup", "-h"]] {
            assert!(
                matches!(native(row, &args(page)), Route::Help(f) if f.page == "setup"),
                "{page:?}"
            );
        }
        for line in [
            &["setup", "help"][..],
            &["setup", "--json", "help"],
            &["setup", "kessel", "--name", "help"],
            &["setup", "kessel", "--name", "-h"],
        ] {
            assert_eq!(native(row, &args(line)), Route::Fallthrough, "{line:?}");
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
            assert_eq!(native(row, &args(line)), family("feedback"), "{line:?}");
        }
        for (line, page) in [
            (&["feedback", "list", "--help"][..], "feedback/list"),
            (&["feedback", "show", "-h"], "feedback/show"),
            (&["feedback", "--help"], "feedback"),
        ] {
            assert!(
                matches!(native(row, &args(line)), Route::Help(f) if f.page == page),
                "{line:?}"
            );
        }
        for line in [
            &["tickets", "show", "x", "-h"][..],
            &["tickets", "list", "--help"],
            &["feedback", "show", "x", "--help"],
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
    /// words.
    #[test]
    fn every_native_row_is_answered_here() {
        for noun in route::TABLE {
            if noun.owner == Owner::Native {
                for word in noun.words {
                    assert_ne!(
                        route(&args(&[*word])),
                        Route::Fallthrough,
                        "{word} is Native but falls through"
                    );
                }
            }
            if noun.bare_row().is_some_and(|v| v.owner == Owner::Native) {
                for line in [&[noun.words[0]][..], &[noun.words[0], "--json"]] {
                    assert_ne!(route(&args(line)), Route::Fallthrough, "{line:?}");
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

    /// Every `Spec` verb row has its spec row and every spec row is a
    /// `Spec` verb row; each routes to the runner, its help to a page or
    /// the shell.
    #[test]
    fn every_spec_row_has_its_route() {
        assert_eq!(
            spec_problems(route::TABLE, spec::table::SPEC),
            Vec::<String>::new()
        );
        for row in spec::table::SPEC {
            let args: Vec<OsString> = row.words.iter().map(OsString::from).collect();
            assert_eq!(route(&args), Route::Spec(row), "{}", row.entry);
        }
    }

    /// The check bites: a `Spec` verb row with no spec row, a spec row the
    /// table sends to the shell, and a whole `Spec` row.
    #[test]
    fn spec_problems_bite() {
        use crate::route::Verb;
        use crate::spec::{Call, Cred, Method, Output, Parse, Print, Style, Via};
        static NOUNS: &[Noun] = &[
            Noun {
                family: "compute",
                words: &["compute"],
                owner: Owner::Bash,
                verbs: &[Verb {
                    words: &["stats", "ls"],
                    owner: Owner::Spec,
                }],
                run: route::hand_back,
            },
            Noun {
                family: "vault",
                words: &["vault"],
                owner: Owner::Spec,
                verbs: &[],
                run: route::hand_back,
            },
        ];
        const fn row(words: &'static [&'static str]) -> SpecVerb {
            SpecVerb {
                entry: "x",
                words,
                parse: Parse {
                    style: Style::None,
                    usage: "u",
                    args: &[],
                    json_flag: false,
                },
                call: Call {
                    via: Via::Compute,
                    method: Method::Get,
                    path: "/cli/x",
                    tcp: false,
                    query: &[],
                },
                output: Output {
                    json: None,
                    text: Print::Body,
                },
                exits: &[],
                auth: Cred::Session,
            }
        }
        static TABLE: &[SpecVerb] = &[row(&["compute", "stats"]), row(&["compute", "nodes"])];
        assert_eq!(
            spec_problems(NOUNS, TABLE),
            [
                "[\"compute\", \"ls\"] is a Spec verb row with no spec row",
                "[\"vault\"] is Spec as a whole row",
                "the spec row \"x\" is not a Spec verb row",
            ]
        );
        static GOOD: &[SpecVerb] = &[row(&["compute", "stats"]), row(&["compute", "ls"])];
        assert_eq!(spec_problems(&NOUNS[..1], GOOD), Vec::<String>::new());
        let args = |l: &[&str]| -> Vec<OsString> { l.iter().map(OsString::from).collect() };
        assert_eq!(
            spec_route(GOOD, &args(&["compute", "stats", "j"])),
            Route::Spec(&GOOD[0])
        );
        assert_eq!(
            spec_route(GOOD, &args(&["compute", "nodes"])),
            Route::Fallthrough
        );
        assert_eq!(
            spec_route(GOOD, &args(&["compute", "stats", "--help", "x"])),
            Route::Fallthrough
        );
    }

    #[test]
    fn the_study_forms() {
        assert_eq!(route(&args(&["study"])), Route::Study(Study::Catalog));
        assert_eq!(
            route(&args(&["study", "list"])),
            Route::Study(Study::Catalog)
        );
        assert_eq!(route(&args(&["study", "-h"])), Route::Study(Study::Usage));
        assert_eq!(
            route(&args(&["study", "errors", "--json"])),
            Route::Study(Study::Page {
                topic: "errors",
                json: true
            })
        );
        assert_eq!(
            route(&args(&["study", "cron"])),
            Route::Study(Study::Page {
                topic: "heartbeats",
                json: false
            })
        );
        // The shell's: the list as JSON, unknown and bash topics, errors.
        for shell in [
            &["study", "--json"][..],
            &["study", "nope"],
            &["study", "app-api"],
            &["study", "errors", "db"],
            &["study", "--x"],
        ] {
            assert_eq!(route(&args(shell)), Route::Fallthrough, "{shell:?}");
        }
    }

    #[test]
    fn the_help_forms() {
        let Route::Help(form) = route(&args(&["mail", "send", "--help"])) else {
            panic!("mail send --help is not a page")
        };
        assert_eq!(form.page, "mail/send");
        assert!(matches!(route(&args(&["help", "mail"])), Route::Help(f) if f.page == "mail"));
        // A native row keeps its own help; anything not exactly a form is
        // the shell's.
        assert_eq!(route(&args(&["cli", "--help"])), Route::CliHelp);
        // The daily tier's forms are [`Route::Daily`] (the_daily_forms),
        // the pieces' pages [`Route::Composed`] (the_composed_forms).
        for shell in [
            &["mail", "send", "x", "--help"][..],
            &["help", "mail", "send"],
            &["chat", "--help", "x"],
            &["app", "--help"],
        ] {
            assert_eq!(route(&args(shell)), Route::Fallthrough, "{shell:?}");
        }
        for form in text::HELP_FORMS {
            let argv: Vec<OsString> = form.argv.iter().map(OsString::from).collect();
            assert!(
                matches!(route(&argv), Route::Help(f) if std::ptr::eq(f, form)),
                "{:?}",
                form.argv
            );
        }
    }

    #[test]
    fn the_composed_forms() {
        for (line, want) in [
            (&["help-deprecated"][..], Composed::Deprecated),
            (&["help", "deprecated"], Composed::Deprecated),
            (&["--help", "--deprecated"], Composed::Deprecated),
            (&["-h", "deprecated"], Composed::Deprecated),
            (&["chat"], Composed::Chat),
            (&["chat", "--help"], Composed::Chat),
            (&["chat", "-h"], Composed::Chat),
            (&["chat", "help"], Composed::Chat),
            (&["help", "chat"], Composed::Chat),
            (&["-h", "chat"], Composed::Chat),
        ] {
            assert_eq!(route(&args(line)), Route::Composed(want), "{line:?}");
        }
        // `cmd_help`'s other words are files or the shell's.
        for word in pages::HELP_WORDS {
            for first in pages::HELP_FIRST {
                let r = route(&args(&[first, word]));
                assert!(
                    !matches!(r, Route::Composed(Composed::Daily) | Route::Unknown),
                    "{first} {word}: {r:?}"
                );
            }
        }
    }

    /// A bare row's line goes to its family's code; its verb rows and its
    /// help are where they were, and a composed page stays the front's.
    #[test]
    fn the_bare_rows() {
        // compute: the bare row is Native, its verb rows unchanged.
        assert_eq!(route(&args(&["compute"])), family("compute"));
        assert_eq!(route(&args(&["compute", "--json"])), family("compute"));
        assert_eq!(route(&args(&["compute", "nodes"])), family("compute"));
        for shell in [
            &["compute", "x"][..],
            &["compute", ""],
            &["compute", "help"],
            &["compute", "--json", "x"],
        ] {
            assert_eq!(route(&args(shell)), Route::Fallthrough, "{shell:?}");
        }
        for help in [&["compute", "--help"][..], &["compute", "-h"]] {
            assert_ne!(route(&args(help)), family("compute"), "{help:?}");
        }
        // chat: `chat --json` is the family's; `chat` alone is still the
        // composed page, and so is its help.
        assert_eq!(route(&args(&["chat", "--json"])), family("chat"));
        assert_eq!(route(&args(&["chat"])), Route::Composed(Composed::Chat));
        assert_eq!(
            route(&args(&["chat", "--help"])),
            Route::Composed(Composed::Chat)
        );
        assert_eq!(route(&args(&["chat", "spaces"])), family("chat"));
        assert_eq!(route(&args(&["chat", "nope"])), Route::Fallthrough);
        // A family with no bare row: its bare line is not the family's.
        for (line, name) in [
            (&["app"][..], "app"),
            (&["connect"], "connect"),
            (&["hostmail", "--json"], "hostmail"),
        ] {
            assert_ne!(route(&args(line)), family(name), "{line:?}");
        }
    }

    /// The bare row's guard bites: with the composed page's check gone,
    /// `chat` alone would reach the family.
    #[test]
    fn a_composed_page_is_checked_before_the_bare_row() {
        let found = route::find(route::TABLE, &args(&["chat"]));
        assert_eq!(found.owner, Owner::Native);
        assert!(found.by_bare);
        assert!(composed_line(&args(&["chat"])));
        assert!(!composed_line(&args(&["chat", "--json"])));
        let chat = found.noun.expect("a chat row");
        assert_eq!(native(chat, &args(&["chat"])), family("chat"));
    }

    /// A first word the router has no arm for is the `*)` arm's; every
    /// word it has an arm for is never (so a typo never reaches a real
    /// verb, and a real verb never gets the unknown command's answer).
    #[test]
    fn the_unknown_forms() {
        for line in [
            &["nope"][..],
            &["nope", "x", "--help"],
            &["Help"],
            &["--nope"],
            &["-x"],
            &["who"],
            &["agents", "create"],
            &["work", "move", "x"],
            &["a b"],
        ] {
            assert_eq!(route(&args(line)), Route::Unknown, "{line:?}");
        }
        for noun in route::TABLE {
            for word in noun.words {
                for line in [&[*word][..], &[*word, "nope"], &[*word, "--help"]] {
                    assert_ne!(route(&args(line)), Route::Unknown, "{line:?}");
                }
            }
        }
        // Every retired verb is the `*)` arm's, but for the `agent` arm's
        // own (`agent create`), which that arm answers.
        for (key, _) in crate::retired::TABLE {
            let words: Vec<&str> = key.split(' ').collect();
            let r = route(&args(&words));
            if route::TABLE.iter().any(|n| n.names(words[0])) {
                assert_ne!(r, Route::Unknown, "{key}");
            } else {
                assert_eq!(r, Route::Unknown, "{key}");
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let odd = vec![OsString::from("nope"), OsString::from_vec(vec![0xff])];
            assert_eq!(route(&odd), Route::Fallthrough);
        }
    }

    #[test]
    fn the_line_says_k2() {
        assert_eq!(version_line("0.46.14"), "k2 0.46.14\n");
    }
}
