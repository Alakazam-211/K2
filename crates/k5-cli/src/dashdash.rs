//! A bare `--` on a command line, as the shell CLI reads it.
//!
//! The words after `--` belong to the command, never to the CLI: a verb
//! that takes one (`compute run -- ls -h`, `msg acme -- --help`) reads
//! them as typed, and every help scan stops at the first one
//! (`_wants_help`, `cli/src/cli-tools-policy.sh`). A verb that takes none
//! refuses it before anything runs: one line on stderr, exit 2, nothing
//! sent (the shell's `_k2_takes_dashdash` and the loop after it).
//!
//! [`takes`] is that list, [`stray`] the refusal, and [`before`] the
//! words a help scan reads.

use k2_client::brand::CLI_NAME;
use std::ffi::OsString;

/// The first words whose whole family takes a `--`.
pub const WORDS: &[&str] = &[
    "ci",
    "job",
    "jobs",
    "doc",
    "docs",
    "wiki",
    "organization",
    "send-feedback",
    "respond",
    "done",
    // `msg who` and `msg verify` take none ([`takes`]).
    "msg",
    "talk",
    "thread",
    "postal",
    "remind",
    "chat",
    "focus-group",
    "focus-groups",
];

/// The first two words that take a `--`, where the family takes none.
pub const PAIRS: &[(&str, &str)] = &[
    ("compute", "run"),
    ("compute", "test"),
    ("compute", "rightsize"),
    ("vault", "run"),
    ("db", "run"),
    ("app", "exec"),
    ("app", "shell"),
    ("changes", "diff"),
    ("changes", "stage"),
    ("changes", "add"),
    ("changes", "unstage"),
    ("changes", "discard"),
    ("sim", "type"),
    ("zen", "garden"),
    ("zen", "gardens"),
    ("sidecar", "new"),
    ("sessions", "archives"),
    ("workspace", "resources"),
    ("workspace", "api-key"),
    ("heartbeat", "remind"),
    ("inbox", "respond"),
    ("tickets", "comment"),
    ("feedback", "comment"),
    ("project", "msg"),
    ("browser", "note"),
    ("terminal", "write"),
    ("approve", "verify"),
    ("approve", "show"),
    ("approve", "key-id"),
];

/// `_k2_takes_dashdash "$1" "$2"`: whether a line that starts with these
/// words may hold a bare `--`.
pub fn takes(first: &str, second: &str) -> bool {
    if first == "msg" && matches!(second, "who" | "verify") {
        return false;
    }
    WORDS.contains(&first) || PAIRS.contains(&(first, second))
}

/// What the shell does with a line's `--` before anything else runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stray {
    /// No bare `--`, or the verb takes one.
    No,
    /// The verb takes none: this line on stderr, exit 2, nothing sent.
    Refused(String),
    /// The verb takes none and its first word is not UTF-8: the shell
    /// prints its line, byte for byte.
    Shell,
}

/// The line the shell refuses a stray `--` with: `first` is the line's
/// first word.
pub fn refusal(first: &str) -> String {
    if first == "--" {
        format!("usage: name a command before '--' ({CLI_NAME} help)")
    } else {
        format!("usage: {first} takes no '--' ({CLI_NAME} help {first})")
    }
}

/// Whether `args` (after `argv[0]`) holds a `--` its verb doesn't take.
pub fn stray(args: &[OsString]) -> Stray {
    if !args.iter().any(|a| a == "--") {
        return Stray::No;
    }
    let word = |i: usize| args.get(i).map_or(Some(""), |a| a.to_str());
    let Some(first) = word(0) else {
        return Stray::Shell;
    };
    let taken = match word(1) {
        Some(second) => takes(first, second),
        // A second word that is not UTF-8 matches no pair, and is neither
        // `who` nor `verify`.
        None => WORDS.contains(&first),
    };
    if taken {
        Stray::No
    } else {
        Stray::Refused(refusal(first))
    }
}

/// The words a help scan reads: those before the first bare `--`.
pub fn before<'a, 'b>(words: &'a [&'b str]) -> &'a [&'b str] {
    let end = words.iter().position(|w| *w == "--").unwrap_or(words.len());
    &words[..end]
}

/// Whether `-h` or `--help` comes before the first bare `--`
/// (`_wants_help "$@"`).
pub fn wants_help(words: &[&str]) -> bool {
    before(words).iter().any(|w| matches!(*w, "-h" | "--help"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn shell_source(file: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src")
            .join(file);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// The patterns of `_k2_takes_dashdash`'s two `case`s that return 0:
    /// the first words, and the first two words.
    fn shell_lists() -> (BTreeSet<String>, BTreeSet<(String, String)>) {
        let text = shell_source("cli-tools-policy.sh");
        let head = "\n_k2_takes_dashdash() {\n";
        let start = text
            .find(head)
            .expect("_k2_takes_dashdash in cli-tools-policy.sh");
        let body = &text[start + head.len()..];
        let end = body.find("\n}\n").expect("the function's end");
        let body = &body[..end];
        let (mut words, mut pairs) = (BTreeSet::new(), BTreeSet::new());
        let mut block = 0;
        for line in body.lines() {
            let t = line.trim();
            if t.starts_with("case \"${1:-}\" in") {
                block = 1;
                continue;
            }
            if t.starts_with("case \"${1:-} ${2:-}\" in") {
                block = 2;
                continue;
            }
            // The nested `case "${2:-}" in who|verify) return 1 ;; esac`.
            if t.is_empty() || t.starts_with('#') || t.starts_with("case ") || t == "esac" {
                continue;
            }
            let Some(close) = t.find(')') else {
                continue;
            };
            let pattern = &t[..close];
            if pattern.contains(' ') && block == 1 || pattern.starts_with("return") {
                continue;
            }
            for alt in pattern.split('|') {
                let alt = alt.trim_matches('"');
                match block {
                    1 => {
                        words.insert(alt.to_string());
                    }
                    2 => {
                        let (a, b) = alt.split_once(' ').expect("two words");
                        pairs.insert((a.to_string(), b.to_string()));
                    }
                    _ => panic!("a pattern outside the two cases: {t}"),
                }
            }
        }
        (words, pairs)
    }

    #[test]
    fn the_list_is_the_shell_s() {
        let (words, pairs) = shell_lists();
        let ours: BTreeSet<String> = WORDS.iter().map(|w| w.to_string()).collect();
        assert_eq!(ours, words);
        let ours: BTreeSet<(String, String)> = PAIRS
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        assert_eq!(ours, pairs);
        // The one exception, as the shell spells it.
        assert!(shell_source("cli-tools-policy.sh").contains(
            "            case \"${2:-}\" in who|verify) return 1 ;; esac   # these take --text, no free text\n"
        ));
    }

    #[test]
    fn which_lines_take_one() {
        for (a, b) in [
            ("msg", "acme"),
            ("msg", ""),
            ("thread", "ask"),
            ("compute", "run"),
            ("browser", "note"),
            ("focus-groups", "x"),
        ] {
            assert!(takes(a, b), "{a} {b}");
        }
        for (a, b) in [
            ("msg", "who"),
            ("msg", "verify"),
            ("compute", "status"),
            ("vault", "list"),
            ("whoami", ""),
            ("browser", "redeliver"),
            ("--", "ls"),
            ("", "msg"),
        ] {
            assert!(!takes(a, b), "{a} {b}");
        }
    }

    #[test]
    fn the_refusal() {
        let k = |s: &str| s.replace("k2 ", &format!("{CLI_NAME} "));
        assert_eq!(stray(&args(&["whoami"])), Stray::No);
        assert_eq!(stray(&args(&["msg", "acme", "--", "-x"])), Stray::No);
        assert_eq!(stray(&args(&["msg", "--"])), Stray::No);
        assert_eq!(
            stray(&args(&["whoami", "--"])),
            Stray::Refused(k("usage: whoami takes no '--' (k2 help whoami)"))
        );
        assert_eq!(
            stray(&args(&["msg", "verify", "--", "x"])),
            Stray::Refused(k("usage: msg takes no '--' (k2 help msg)"))
        );
        assert_eq!(
            stray(&args(&["--", "ls"])),
            Stray::Refused(k("usage: name a command before '--' (k2 help)"))
        );
        assert_eq!(
            stray(&args(&["", "--"])),
            Stray::Refused(k("usage:  takes no '--' (k2 help )"))
        );
        // `--x` and `---` are not a bare `--`.
        assert_eq!(stray(&args(&["whoami", "--x", "---"])), Stray::No);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let odd = vec![OsString::from_vec(vec![b'w', 0xff]), OsString::from("--")];
            assert_eq!(stray(&odd), Stray::Shell);
            let odd = vec![
                OsString::from("msg"),
                OsString::from_vec(vec![0xff]),
                OsString::from("--"),
            ];
            assert_eq!(stray(&odd), Stray::No, "msg takes one, whatever follows");
            let odd = vec![
                OsString::from("compute"),
                OsString::from_vec(vec![0xff]),
                OsString::from("--"),
            ];
            assert_eq!(
                stray(&odd),
                Stray::Refused(refusal("compute")),
                "no pair matches a word that is not UTF-8"
            );
        }
        // The shell's lines, as its source spells them.
        let src = shell_source("cli-tools-policy.sh");
        assert!(src.contains("echo \"usage: name a command before '--' (k2 help)\" >&2"));
        assert!(src.contains("echo \"usage: ${1:-} takes no '--' (k2 help ${1:-})\" >&2"));
    }

    #[test]
    fn help_scans_stop_at_the_first_dashes() {
        assert!(wants_help(&["a", "-h", "--", "b"]));
        assert!(!wants_help(&["a", "--", "-h"]));
        assert!(!wants_help(&["--", "--help"]));
        assert!(wants_help(&["--help"]));
        assert_eq!(before(&["a", "--", "b", "--"]), &["a"]);
        assert_eq!(before(&["a", "b"]), &["a", "b"]);
        assert!(before(&[]).is_empty());
    }
}
