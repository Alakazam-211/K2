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
//! is `Bash`. So the family's own forms (no verb, `--help`, `-h`) stay
//! with the shell until every verb has moved and the whole row turns: one
//! family never has two help texts.
//!
//! The hard rule (PRD 13.8 (b) and 6.2), in `tests/route_table.rs`: every
//! word of the router's case has a row and every row is an arm of it,
//! and a `Native` or `Spec` row has a `--schema` entry or a named
//! reason not to.

use std::ffi::OsString;

/// Who answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// The binary, by its own code ([`crate::front`]). A form its code
    /// does not take still goes to the shell CLI.
    Native,
    /// The binary, from the command's `--schema` entry. Nothing is `Spec`
    /// yet: the front has no runner for it.
    Spec,
    /// The shell CLI, byte for byte ([`crate::fallthrough`]).
    Bash,
}

/// One arm of the router's top-level `case`.
#[derive(Debug)]
pub struct Noun {
    /// The arm's words, in the arm's order.
    pub words: &'static [&'static str],
    /// Who answers the noun's own forms and every verb not in `verbs`.
    pub owner: Owner,
    /// Verbs answered by someone other than `owner`.
    pub verbs: &'static [Verb],
}

/// A verb of a [`Noun`]: its second word.
#[derive(Debug)]
pub struct Verb {
    pub words: &'static [&'static str],
    pub owner: Owner,
}

impl Noun {
    /// Whether `word` is one of the row's words.
    pub fn names(&self, word: &str) -> bool {
        self.words.contains(&word)
    }
}

/// What [`find`] found.
#[derive(Debug, Clone, Copy)]
pub struct Found {
    /// The row of the first word, if it has one.
    pub noun: Option<&'static Noun>,
    /// Who answers this command line.
    pub owner: Owner,
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
        };
    };
    let verb = args.get(1).and_then(|w| w.to_str());
    let owner = verb
        .and_then(|w| noun.verbs.iter().find(|v| v.words.contains(&w)))
        .map_or(noun.owner, |v| v.owner);
    Found {
        noun: Some(noun),
        owner,
    }
}

const fn bash(words: &'static [&'static str]) -> Noun {
    Noun {
        words,
        owner: Owner::Bash,
        verbs: &[],
    }
}

const fn native(words: &'static [&'static str]) -> Noun {
    Noun {
        words,
        owner: Owner::Native,
        verbs: &[],
    }
}

/// The route table: one row per arm of `cli/src/router.sh`'s top-level
/// `case`, in its order. The `*)` arm has no row: a word with no row is
/// the shell's.
pub static TABLE: &[Noun] = &[
    bash(&["fed"]),
    bash(&["agent"]),
    bash(&["context"]),
    bash(&["preset"]),
    // Only `list` and `show`; every other verb, and `feedback` alone, is
    // the shell's. One verb row each, so either can turn back alone.
    Noun {
        words: &["feedback", "tickets"],
        owner: Owner::Bash,
        verbs: &[
            Verb {
                words: &["list"],
                owner: Owner::Native,
            },
            Verb {
                words: &["show"],
                owner: Owner::Native,
            },
        ],
    },
    bash(&["organization"]),
    bash(&["project"]),
    bash(&["mail"]),
    bash(&["calendar", "cal"]),
    bash(&["hostmail"]),
    bash(&["db"]),
    bash(&["store"]),
    bash(&["board"]),
    bash(&["dns"]),
    bash(&["domain"]),
    bash(&["cert"]),
    bash(&["terminal"]),
    bash(&["state"]),
    bash(&["commit"]),
    bash(&["reviews"]),
    bash(&["review"]),
    bash(&["trust-folders"]),
    // `list`, `profile`, `launch`, `update`, `create` and `open`, the
    // verbs a first run uses; every other verb, and `workspace` alone, is
    // the shell's. One verb row each, so each can turn back alone.
    Noun {
        words: &["workspace"],
        owner: Owner::Bash,
        verbs: &[
            Verb {
                words: &["list"],
                owner: Owner::Native,
            },
            Verb {
                words: &["profile"],
                owner: Owner::Native,
            },
            Verb {
                words: &["create"],
                owner: Owner::Native,
            },
            Verb {
                words: &["open"],
                owner: Owner::Native,
            },
        ],
    },
    bash(&["onboarding"]),
    bash(&["remind"]),
    bash(&["reminders"]),
    bash(&["heartbeat"]),
    bash(&["hooks"]),
    bash(&["daemon"]),
    bash(&["tunnel"]),
    bash(&["settings"]),
    bash(&["checkin"]),
    bash(&["status"]),
    native(&["done"]),
    native(&["whoami"]),
    // The reads and the writes each behind a switch (`families::setup`): a
    // form whose switch is off is the shell's.
    native(&["setup"]),
    bash(&["msg"]),
    bash(&["thread"]),
    bash(&["browser"]),
    native(&["read"]),
    bash(&["talk"]),
    native(&["respond"]),
    bash(&["whats-new"]),
    native(&["activity"]),
    native(&["inbox"]),
    bash(&["glossary"]),
    bash(&["help-deprecated"]),
    bash(&["help-internal"]),
    native(&["sessions"]),
    bash(&["sidecar"]),
    bash(&["root-helper"]),
    bash(&["front"]),
    bash(&["firewall"]),
    bash(&["desktop"]),
    bash(&["git-tools"]),
    bash(&["push"]),
    bash(&["notifications"]),
    bash(&["notify"]),
    bash(&["compute"]),
    bash(&["changes"]),
    bash(&["sim"]),
    bash(&["doc", "docs"]),
    bash(&["hostchat"]),
    bash(&["voice"]),
    bash(&["chat"]),
    bash(&["sources", "source"]),
    bash(&["llm"]),
    bash(&["usage"]),
    bash(&["vault"]),
    bash(&["migrate"]),
    bash(&["job", "jobs"]),
    bash(&["ci"]),
    bash(&["reserve"]),
    bash(&["release"]),
    // `list` (and `--json` and `--users`, which the shell reads as
    // `list`), `add` and `remove`; every other action, and `connections` alone, is
    // the shell's. One verb row each, so each can turn back alone.
    Noun {
        words: &["connections"],
        owner: Owner::Bash,
        verbs: &[
            Verb {
                words: &["list", "--json", "--users"],
                owner: Owner::Native,
            },
            Verb {
                words: &["add"],
                owner: Owner::Native,
            },
            Verb {
                words: &["remove"],
                owner: Owner::Native,
            },
        ],
    },
    bash(&["api"]),
    bash(&["api-key"]),
    bash(&["app"]),
    bash(&["skin"]),
    bash(&["skin-token"]),
    bash(&["publish"]),
    bash(&["clone"]),
    bash(&["users"]),
    bash(&["remote-session"]),
    bash(&["zen"]),
    bash(&["wiki"]),
    // Only `status`; `login`, `logout`, the old publish words and
    // `connect` alone are the shell's.
    Noun {
        words: &["connect"],
        owner: Owner::Bash,
        verbs: &[Verb {
            words: &["status"],
            owner: Owner::Native,
        }],
    },
    bash(&["skills"]),
    bash(&["app-update"]),
    // Each form behind its own switch (`families::update`): a form whose
    // switch is off is the shell's.
    native(&["update"]),
    native(&["version", "--version", "-v"]),
    native(&["cli"]),
    bash(&["--schema", "schema"]),
    native(&["study"]),
    bash(&["explain"]),
    bash(&["capabilities"]),
    bash(&["send-feedback"]),
    bash(&["approve"]),
    bash(&["permission"]),
    bash(&["help", "--help", "-h"]),
    bash(&["help-advanced"]),
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
    /// never verbs.
    #[test]
    fn only_a_shell_row_has_verb_rows() {
        for noun in TABLE {
            if noun.verbs.is_empty() {
                continue;
            }
            assert_eq!(noun.owner, Owner::Bash, "{:?} has verb rows", noun.words);
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
                        !["--help", "-h", "help"].contains(word),
                        "{:?} {word} is a help form",
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
        assert_eq!(owner(TABLE, &["docs", "list"]), Owner::Bash);
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
            },
            native(&["whoami"]),
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
}
