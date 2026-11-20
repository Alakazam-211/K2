//! Who answers a command: the binary itself (`Native`), the binary from
//! the command's `--schema` entry (`Spec`), or the shell CLI (`Bash`).
//!
//! [`TABLE`] has one row for each arm of the shell router's top-level
//! `case` (`cli/src/router.sh`, `case "${1:-help}" in`), in the router's
//! order. An arm with several words (`doc|docs)`, `version|--version|-v)`)
//! is one command, so it is one row naming all its words: the words of
//! one arm can never be owned by two sides, and the row reads the same as
//! the arm it stands for. `tests/cli/schema_drift.sh` counts an arm as one
//! command the same way.
//!
//! The first word is read as the router reads `${1:-help}`: no word, or
//! an empty one, is `help`. A word that is no row's goes where the
//! router's `*)` arm sends it, to the shell CLI. So does a word that is
//! not UTF-8: no row has one.
//!
//! A row can be narrowed by its second word ([`Verb`]): a family can move
//! one verb at a time. Only a `Bash` row has verb rows, and none of them
//! is `Bash`. So the family's own forms (`--help`, `-h`, an unknown verb)
//! stay with the shell until every verb has moved and the whole row
//! turns: one family never has two help texts.
//!
//! The one form a `Bash` row may move before that is its bare line: the
//! first word alone, or it and `--json` and nothing else ([`is_bare`]). A
//! family moves it with a verb row of no words ([`Verb::bare`]), which
//! may only be `Native`. A help word is never a bare line, so the help
//! stays where it was.
//!
//! The hard rule (PRD 13.8 (b) and 6.2), in `tests/route_table.rs`: every
//! word of the router's case has a row and every row is an arm of it,
//! and a `Native` or `Spec` row has a `--schema` entry or a named
//! reason not to.
//!
//! # One module per family
//!
//! A row is its family's: [`TABLE`] lists the families in the router's
//! order, and each row's words, owner, verb rows and code ([`Run`]) are
//! consts and a `run` function in that family's module under
//! [`crate::families`], named after its first word (`trust-folders` is
//! `trust_folders`, `--schema` is `schema`). A family the shell answers
//! whole is a stub (`shell_family!`). Making a family native, or turning
//! it back, edits only its own module and tests, never this list, so
//! lanes working on different families never touch the same lines. The
//! version, `cli` and `study` rows are the front's own code ([`FRONT`]).

use crate::exit::Exit;
use std::ffi::OsString;

/// Who answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// The binary, by its own code ([`crate::front`]). A form its code
    /// does not take still goes to the shell CLI.
    Native,
    /// The binary, from the command's `--schema` entry ([`crate::spec`]).
    /// Only a verb row may be `Spec`, and each such verb has its row of
    /// [`crate::spec::table::SPEC`] ([`crate::front::spec_problems`]).
    Spec,
    /// The shell CLI, byte for byte ([`crate::fallthrough`]).
    Bash,
}

/// A family's code for a command line the route sends to it (the whole
/// command line after `argv[0]`): its exit, or `None` to hand it to the
/// shell CLI, decided before anything is printed or sent.
pub type Run = fn(&[OsString]) -> Option<Exit>;

/// One arm of the router's top-level `case`: one family's row.
#[derive(Debug)]
pub struct Noun {
    /// The family's module under [`crate::families`], or [`FRONT`].
    pub family: &'static str,
    /// The arm's words, in the arm's order.
    pub words: &'static [&'static str],
    /// Who answers the noun's own forms and every verb not in `verbs`.
    pub owner: Owner,
    /// Verbs answered by someone other than `owner`.
    pub verbs: &'static [Verb],
    /// The family's code, for a form the route sends to the binary
    /// ([`crate::front::Route::Family`]).
    pub run: Run,
}

/// A row is itself: two rows are equal only when they are the same row
/// of the same table.
impl PartialEq for Noun {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Noun {}

/// A verb of a [`Noun`]: its second word. A row with no words is the
/// noun's bare line ([`Verb::bare`]).
#[derive(Debug)]
pub struct Verb {
    pub words: &'static [&'static str],
    pub owner: Owner,
}

impl Verb {
    /// The row of a family's bare line ([`is_bare`]): who answers the
    /// first word alone, or it and `--json`, when that is not the row's
    /// owner. It has no words, and only a `Bash` row may have it, as
    /// `Native`.
    pub const fn bare(owner: Owner) -> Verb {
        Verb { words: &[], owner }
    }

    /// Whether this is the bare line's row.
    pub fn is_bare(&self) -> bool {
        self.words.is_empty()
    }
}

impl Noun {
    /// Whether `word` is one of the row's words.
    pub fn names(&self, word: &str) -> bool {
        self.words.contains(&word)
    }

    /// The verb rows that name a second word: every row of [`Noun::verbs`]
    /// but the bare line's.
    pub fn verb_rows(&self) -> impl Iterator<Item = &'static Verb> {
        self.verbs.iter().filter(|v| !v.is_bare())
    }

    /// The bare line's row, when the family has one.
    pub fn bare_row(&self) -> Option<&'static Verb> {
        self.verbs.iter().find(|v| v.is_bare())
    }

    /// Who answers the bare line: its row's owner, or the row's.
    pub fn bare(&self) -> Owner {
        self.bare_row().map_or(self.owner, |v| v.owner)
    }
}

/// Whether `args` (the arguments after `argv[0]`) is a bare line: the
/// first word alone, or it and `--json` and nothing else.
pub fn is_bare(args: &[OsString]) -> bool {
    match args {
        [_] => true,
        [_, json] => json == "--json",
        _ => false,
    }
}

/// What [`find`] found.
#[derive(Debug, Clone, Copy)]
pub struct Found {
    /// The row of the first word, if it has one.
    pub noun: Option<&'static Noun>,
    /// Who answers this command line.
    pub owner: Owner,
    /// Whether the row's bare line row ([`Verb::bare`]) said so.
    pub by_bare: bool,
}

/// Who answers `args` (the arguments after `argv[0]`) by `table`.
pub fn find(table: &'static [Noun], args: &[OsString]) -> Found {
    let first = match args.first() {
        None => Some("help"),
        Some(w) if w.is_empty() => Some("help"),
        Some(w) => w.to_str(),
    };
    let Some(noun) = first.and_then(|w| table.iter().find(|n| n.names(w))) else {
        return Found {
            noun: None,
            owner: Owner::Bash,
            by_bare: false,
        };
    };
    if let Some(row) = noun.bare_row().filter(|_| is_bare(args)) {
        return Found {
            noun: Some(noun),
            owner: row.owner,
            by_bare: true,
        };
    }
    let verb = args.get(1).and_then(|w| w.to_str());
    let owner = verb
        .and_then(|w| noun.verb_rows().find(|v| v.words.contains(&w)))
        .map_or(noun.owner, |v| v.owner);
    Found {
        noun: Some(noun),
        owner,
        by_bare: false,
    }
}

/// The [`Noun::family`] of a row the front answers by its own code.
pub const FRONT: &str = "crate::front";

/// A row the front answers by its own code ([`crate::front`]): the
/// version, `cli` and `study`. Its `run` is never reached.
const fn front(words: &'static [&'static str]) -> Noun {
    Noun {
        family: FRONT,
        words,
        owner: Owner::Native,
        verbs: &[],
        run: hand_back,
    }
}

/// A [`Run`] that hands every command line to the shell CLI.
pub fn hand_back(_args: &[OsString]) -> Option<Exit> {
    None
}

/// The items of a family the shell CLI answers whole, for its module
/// under [`crate::families`]: the router arm's `WORDS`, `OWNER`
/// ([`Owner::Bash`]), no `VERBS`, and a `run` that hands every command
/// line to the shell. A lane that makes the family native writes the four
/// items out in its module in place of this: the words stay as they are,
/// and [`TABLE`] needs no edit.
macro_rules! shell_family {
    ($words:expr) => {
        /// The router arm's words: this family's row of
        /// [`crate::route::TABLE`].
        pub const WORDS: &[&str] = $words;
        /// The shell CLI answers every form.
        pub const OWNER: $crate::route::Owner = $crate::route::Owner::Bash;
        /// No verb rows.
        pub const VERBS: &[$crate::route::Verb] = &[];
        /// Hands every command line to the shell CLI.
        pub fn run(_args: &[::std::ffi::OsString]) -> Option<$crate::exit::Exit> {
            None
        }
    };
}
pub(crate) use shell_family;

/// A family's row, from its module under [`crate::families`].
macro_rules! family {
    ($m:ident) => {
        Noun {
            family: stringify!($m),
            words: crate::families::$m::WORDS,
            owner: crate::families::$m::OWNER,
            verbs: crate::families::$m::VERBS,
            run: crate::families::$m::run,
        }
    };
}

/// The route table: one row per arm of `cli/src/router.sh`'s top-level
/// `case`, in its order, each its family's ([`crate::families`]). The
/// `*)` arm has no row: a word with no row is the shell's.
pub static TABLE: &[Noun] = &[
    family!(fed),
    family!(agent),
    family!(context),
    family!(preset),
    family!(feedback),
    family!(organization),
    family!(project),
    family!(mail),
    family!(calendar),
    family!(hostmail),
    family!(db),
    family!(store),
    family!(board),
    family!(dns),
    family!(domain),
    family!(cert),
    family!(terminal),
    family!(state),
    family!(commit),
    family!(reviews),
    family!(review),
    family!(trust_folders),
    family!(workspace),
    family!(onboarding),
    family!(remind),
    family!(reminders),
    family!(heartbeat),
    family!(hooks),
    family!(daemon),
    family!(tunnel),
    family!(settings),
    family!(checkin),
    family!(status),
    family!(done),
    family!(whoami),
    family!(postal),
    family!(setup),
    family!(msg),
    family!(thread),
    family!(browser),
    family!(read),
    family!(talk),
    family!(respond),
    family!(whats_new),
    family!(activity),
    family!(inbox),
    family!(glossary),
    family!(focus_group),
    family!(help_deprecated),
    family!(help_internal),
    family!(sessions),
    family!(sidecar),
    family!(root_helper),
    family!(front),
    family!(firewall),
    family!(desktop),
    family!(git_tools),
    family!(push),
    family!(notifications),
    family!(notify),
    family!(compute),
    family!(changes),
    family!(sim),
    family!(doc),
    family!(hostchat),
    family!(voice),
    family!(chat),
    family!(sources),
    family!(llm),
    family!(usage),
    family!(vault),
    family!(migrate),
    family!(job),
    family!(ci),
    family!(reserve),
    family!(release),
    family!(connections),
    family!(api),
    family!(api_key),
    family!(app),
    family!(skin),
    family!(skin_token),
    family!(publish),
    family!(clone),
    family!(users),
    family!(remote),
    family!(remote_session),
    family!(zen),
    family!(wiki),
    family!(connect),
    family!(skills),
    family!(app_update),
    family!(update),
    front(&["version", "--version", "-v"]),
    front(&["cli"]),
    family!(schema),
    front(&["study"]),
    family!(explain),
    family!(capabilities),
    family!(send_feedback),
    family!(approve),
    family!(permission),
    family!(help),
    family!(help_advanced),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn owner(table: &'static [Noun], list: &[&str]) -> Owner {
        find(table, &args(list)).owner
    }

    const fn row(family: &'static str, words: &'static [&'static str], owner: Owner) -> Noun {
        Noun {
            family,
            words,
            owner,
            verbs: &[],
            run: hand_back,
        }
    }

    /// What is wrong with the families of `table`: a family in two rows,
    /// a word two rows claim, or a family's module not named after its
    /// first word (`trust-folders` is `trust_folders`, `--schema` is
    /// `schema`). The front's own rows have no module. Empty when none.
    fn claims(table: &[Noun]) -> Vec<String> {
        let mut out = Vec::new();
        let mut families = BTreeSet::new();
        let mut words = std::collections::BTreeMap::new();
        for noun in table {
            if noun.family != FRONT {
                if !families.insert(noun.family) {
                    out.push(format!("families::{} has two rows", noun.family));
                }
                let module = noun.words[0].trim_start_matches('-').replace('-', "_");
                if noun.family != module {
                    out.push(format!(
                        "{:?} is families::{}, not families::{module}",
                        noun.words, noun.family
                    ));
                }
            }
            for word in noun.words {
                if let Some(other) = words.insert(*word, noun.family) {
                    out.push(format!("{word} is claimed by {other} and {}", noun.family));
                }
            }
        }
        out
    }

    /// Each family claims its own words: no word is two families', no
    /// family has two rows, and each module is named after its first
    /// word. With the bite.
    #[test]
    fn no_two_families_claim_the_same_words() {
        assert_eq!(claims(TABLE), Vec::<String>::new());
        static TWO: &[Noun] = &[
            row("mail", &["mail", "email"], Owner::Bash),
            row("hostmail", &["hostmail", "email"], Owner::Bash),
            row("mail", &["mail2"], Owner::Bash),
            row("dns", &["domain"], Owner::Bash),
        ];
        assert_eq!(
            claims(TWO),
            [
                "email is claimed by mail and hostmail",
                "families::mail has two rows",
                "[\"mail2\"] is families::mail, not families::mail2",
                "[\"domain\"] is families::dns, not families::domain",
            ]
        );
    }

    /// The rows, in today's order. The scaffold that moved each row into
    /// its family's module kept every row and its place; a new arm of the
    /// router is a new line here as well as in [`TABLE`].
    #[test]
    fn the_order_is_today_s() {
        const TODAY: &[&[&str]] = &[
            &["fed"],
            &["agent"],
            &["context"],
            &["preset"],
            &["feedback", "tickets"],
            &["organization"],
            &["project"],
            &["mail"],
            &["calendar", "cal"],
            &["hostmail"],
            &["db"],
            &["store"],
            &["board"],
            &["dns"],
            &["domain"],
            &["cert"],
            &["terminal"],
            &["state"],
            &["commit"],
            &["reviews"],
            &["review"],
            &["trust-folders"],
            &["workspace"],
            &["onboarding"],
            &["remind"],
            &["reminders"],
            &["heartbeat"],
            &["hooks"],
            &["daemon"],
            &["tunnel"],
            &["settings"],
            &["checkin"],
            &["status"],
            &["done"],
            &["whoami"],
            &["postal"],
            &["setup"],
            &["msg"],
            &["thread"],
            &["browser"],
            &["read"],
            &["talk"],
            &["respond"],
            &["whats-new"],
            &["activity"],
            &["inbox"],
            &["glossary"],
            &["focus-group", "focus-groups"],
            &["help-deprecated"],
            &["help-internal"],
            &["sessions"],
            &["sidecar"],
            &["root-helper"],
            &["front"],
            &["firewall"],
            &["desktop"],
            &["git-tools"],
            &["push"],
            &["notifications"],
            &["notify"],
            &["compute"],
            &["changes"],
            &["sim"],
            &["doc", "docs"],
            &["hostchat"],
            &["voice"],
            &["chat"],
            &["sources", "source"],
            &["llm"],
            &["usage"],
            &["vault"],
            &["migrate"],
            &["job", "jobs"],
            &["ci"],
            &["reserve"],
            &["release"],
            &["connections"],
            &["api"],
            &["api-key"],
            &["app"],
            &["skin"],
            &["skin-token"],
            &["publish"],
            &["clone"],
            &["users"],
            &["remote"],
            &["remote-session"],
            &["zen"],
            &["wiki"],
            &["connect"],
            &["skills"],
            &["app-update"],
            &["update"],
            &["version", "--version", "-v"],
            &["cli"],
            &["--schema", "schema"],
            &["study"],
            &["explain"],
            &["capabilities"],
            &["send-feedback"],
            &["approve"],
            &["permission"],
            &["help", "--help", "-h"],
            &["help-advanced"],
        ];
        let rows: Vec<&[&str]> = TABLE.iter().map(|n| n.words).collect();
        assert_eq!(rows, TODAY);
        let front: Vec<&str> = TABLE
            .iter()
            .filter(|n| n.family == FRONT)
            .map(|n| n.words[0])
            .collect();
        assert_eq!(front, ["version", "cli", "study"]);
    }

    #[test]
    fn a_word_is_in_one_row_once_and_none_is_empty() {
        let mut seen = BTreeSet::new();
        for noun in TABLE {
            assert!(!noun.words.is_empty(), "a row with no words");
            for word in noun
                .words
                .iter()
                .chain(noun.verbs.iter().flat_map(|v| v.words))
            {
                assert!(!word.is_empty(), "an empty word in {:?}", noun.words);
            }
            for word in noun.words {
                assert!(seen.insert(*word), "{word} is in two rows");
            }
            let mut verbs = BTreeSet::new();
            for verb in noun.verbs {
                for word in verb.words {
                    assert!(
                        verbs.insert(*word),
                        "{word} is two verbs of {:?}",
                        noun.words
                    );
                }
            }
        }
    }

    /// One family never has two help texts: a row with verb rows is the
    /// shell's, its verb rows are not, and `--help`, `-h` and `help` are
    /// never verbs. Nor is `--json`, which a bare line may end with. A
    /// bare line's row is at most one, and `Native`.
    #[test]
    fn only_a_shell_row_has_verb_rows() {
        for noun in TABLE {
            if noun.verbs.is_empty() {
                continue;
            }
            assert_eq!(noun.owner, Owner::Bash, "{:?} has verb rows", noun.words);
            let bare: Vec<&Verb> = noun.verbs.iter().filter(|v| v.is_bare()).collect();
            assert!(bare.len() <= 1, "{:?} has two bare rows", noun.words);
            for row in bare {
                assert_eq!(
                    row.owner,
                    Owner::Native,
                    "{:?}'s bare row is not Native",
                    noun.words
                );
            }
            for verb in noun.verbs {
                assert_ne!(
                    verb.owner,
                    Owner::Bash,
                    "{:?} {:?} says what its row says",
                    noun.words,
                    verb.words
                );
                for word in verb.words {
                    assert!(
                        !["--help", "-h", "help", "--json"].contains(word),
                        "{:?} {word} is a help form or --json",
                        noun.words
                    );
                }
            }
        }
    }

    #[test]
    fn the_first_word_is_read_as_the_router_reads_it() {
        assert_eq!(owner(TABLE, &["version"]), Owner::Native);
        assert_eq!(owner(TABLE, &["--version", "x"]), Owner::Native);
        assert_eq!(owner(TABLE, &["-v"]), Owner::Native);
        assert_eq!(owner(TABLE, &["cli", "status"]), Owner::Native);
        // No word and an empty word are `help`.
        for list in [&[][..], &[""], &["", "version"]] {
            let found = find(TABLE, &args(list));
            assert!(found.noun.is_some_and(|n| n.names("help")), "{list:?}");
            assert_eq!(found.owner, Owner::Bash, "{list:?}");
        }
        // The `*)` arm: no row, the shell's.
        for list in [
            &["nosuchverb"][..],
            &["Version"],
            &[" version"],
            &["-V"],
            &["*"],
        ] {
            let found = find(TABLE, &args(list));
            assert!(found.noun.is_none(), "{list:?}");
            assert_eq!(found.owner, Owner::Bash, "{list:?}");
        }
        // An alias is read as its row: `--help` is the shell's help row,
        // `docs` is doc's (native).
        let found = find(TABLE, &args(&["--help", "list"]));
        assert!(found.noun.is_some_and(|n| n.names("help")));
        assert_eq!(found.owner, Owner::Bash);
        assert_eq!(owner(TABLE, &["docs", "list"]), Owner::Native);
    }

    #[cfg(unix)]
    #[test]
    fn a_word_that_is_not_utf8_has_no_row() {
        use std::os::unix::ffi::OsStringExt;
        let found = find(TABLE, &[OsString::from_vec(b"version\xff".to_vec())]);
        assert!(found.noun.is_none());
        assert_eq!(found.owner, Owner::Bash);
    }

    /// A family part moved: its verbs go native, everything else of it
    /// (no verb, `--help`, `-h`, an unknown verb) stays with the shell.
    #[test]
    fn a_verb_row_narrows_its_row() {
        static PART: &[Noun] = &[
            Noun {
                family: "mail",
                words: &["mail", "email"],
                owner: Owner::Bash,
                verbs: &[
                    Verb {
                        words: &["read", "show"],
                        owner: Owner::Native,
                    },
                    Verb {
                        words: &["list"],
                        owner: Owner::Spec,
                    },
                ],
                run: hand_back,
            },
            row("whoami", &["whoami"], Owner::Native),
        ];
        assert_eq!(owner(PART, &["mail", "read"]), Owner::Native);
        assert_eq!(owner(PART, &["email", "show", "--json"]), Owner::Native);
        assert_eq!(owner(PART, &["mail", "list"]), Owner::Spec);
        for list in [
            &["mail"][..],
            &["mail", "--help"],
            &["mail", "-h"],
            &["mail", "help"],
            &["mail", "send"],
            &["mail", ""],
            &["read"],
        ] {
            assert_eq!(owner(PART, list), Owner::Bash, "{list:?}");
        }
        // A whole row turned: its help is its own.
        for list in [&["whoami"][..], &["whoami", "--help"], &["whoami", "-h"]] {
            assert_eq!(owner(PART, list), Owner::Native, "{list:?}");
        }
    }

    /// A bare line's row: the first word alone, or it and `--json`, goes
    /// where the row says; every other form of the family, its help and
    /// its verb rows, stay where they were. With no bare row the bare
    /// line is the row's owner's.
    #[test]
    fn a_bare_row_moves_only_the_bare_line() {
        static BARE: &[Noun] = &[
            Noun {
                family: "compute",
                words: &["compute"],
                owner: Owner::Bash,
                verbs: &[
                    Verb {
                        words: &["nodes", "ls"],
                        owner: Owner::Native,
                    },
                    Verb {
                        words: &["stats"],
                        owner: Owner::Spec,
                    },
                    Verb::bare(Owner::Native),
                ],
                run: hand_back,
            },
            Noun {
                family: "mail",
                words: &["mail", "email"],
                owner: Owner::Bash,
                verbs: &[Verb {
                    words: &["read"],
                    owner: Owner::Native,
                }],
                run: hand_back,
            },
        ];
        let bare = |list: &[&str]| find(BARE, &args(list));
        // The bare line: the row of no words.
        for list in [&["compute"][..], &["compute", "--json"]] {
            let found = bare(list);
            assert_eq!(found.owner, Owner::Native, "{list:?}");
            assert!(found.by_bare, "{list:?}");
        }
        // The verb rows are unchanged, and not by the bare row.
        for (list, want) in [
            (&["compute", "nodes"][..], Owner::Native),
            (&["compute", "ls", "--json"], Owner::Native),
            (&["compute", "stats"], Owner::Spec),
        ] {
            let found = bare(list);
            assert_eq!(found.owner, want, "{list:?}");
            assert!(!found.by_bare, "{list:?}");
        }
        // Everything else stays the shell's: the help, an unknown or empty
        // verb, `--json` with more after it or before a verb.
        for list in [
            &["compute", "--help"][..],
            &["compute", "-h"],
            &["compute", "help"],
            &["compute", "x"],
            &["compute", ""],
            &["compute", "--json", "x"],
            &["compute", "--json", "--json"],
            &["compute", "-json"],
        ] {
            let found = bare(list);
            assert_eq!(found.owner, Owner::Bash, "{list:?}");
            assert!(!found.by_bare, "{list:?}");
        }
        // No bare row: the bare line is the row's owner's.
        for list in [&["mail"][..], &["email", "--json"]] {
            let found = bare(list);
            assert_eq!(found.owner, Owner::Bash, "{list:?}");
            assert!(!found.by_bare, "{list:?}");
        }
        assert_eq!(bare(&["mail", "read"]).owner, Owner::Native);
        let rows = |n: &Noun| n.verb_rows().map(|v| v.words).collect::<Vec<_>>();
        assert_eq!(rows(&BARE[0]), [&["nodes", "ls"][..], &["stats"]]);
        assert_eq!(BARE[0].bare(), Owner::Native);
        assert_eq!(BARE[1].bare(), Owner::Bash);
        assert!(BARE[1].bare_row().is_none());
    }

    #[test]
    fn a_bare_line_is_the_first_word_alone_or_with_json() {
        assert!(is_bare(&args(&["x"])));
        assert!(is_bare(&args(&["x", "--json"])));
        for list in [
            &[][..],
            &["x", "--json", "y"],
            &["x", "y"],
            &["x", ""],
            &["x", "--JSON"],
            &["x", "--json="],
        ] {
            assert!(!is_bare(&args(list)), "{list:?}");
        }
    }

    /// The table today: the rows with a bare row of their own.
    #[test]
    fn the_bare_rows_are_today_s() {
        let rows: Vec<&str> = TABLE
            .iter()
            .filter(|n| n.bare_row().is_some())
            .map(|n| n.words[0])
            .collect();
        assert_eq!(rows, ["hostmail", "chat"]);
    }
}
