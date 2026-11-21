//! `k5-cli hostmail ooo` and `footer` against the shell CLI's `cli/k2
//! hostmail …`, on the parity rig ([`common::parity`]): the same requests
//! (urllib's, from `_mail_py`), stdout, stderr and exit code.
//!
//! - The words: each sub-command's help with no words, each usage line
//!   (a missing `@` file among them), exit 2 and nothing sent.
//! - ooo set (the text itself, from a file with its trailing newlines
//!   dropped, days, until), unset and show (on with its text, off);
//!   footer set, unset (with and without a domain) and show.
//! - `--text -`: the text on stdin, read past the gates; none; twice;
//!   not UTF-8 (the shell's raise); a NUL (`nul-in-stdin-text`).
//! - A refusal, an answer the Python raises on, and no answer.
//! - Help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "hostmail";

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

/// A text file in the workspace, its trailing newlines to be dropped.
fn note_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(
        r.proj.join("note.txt"),
        "Away until Monday.\nAsk Acme support.\n\n\n",
    )
    .unwrap();
}

const OOO_ON: &str = r#"{"ok":true,"address":"sales@acme.example","enabled":true,"days":3,"until":"2026-12-01","text":"Away.\nBack soon."}"#;

#[test]
fn hostmail_notices_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "nwords");
    let v = vec![
        case(
            "ooo alone: the help",
            &["ooo"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "footer alone: the help",
            &["footer"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        usage_case("ooo, an unknown sub-command", &["ooo", "on"]),
        usage_case("ooo set, no address", &["ooo", "set", "--text", "x"]),
        usage_case("ooo set, no text", &["ooo", "set", "a@x"]),
        usage_case(
            "ooo set, --text with no value",
            &["ooo", "set", "a@x", "--text"],
        ),
        usage_case("ooo set, @ alone", &["ooo", "set", "a@x", "--text", "@"]),
        usage_case(
            "ooo set, no such file, before a bad flag",
            &["ooo", "set", "a@x", "--text", "@missing.txt", "--bogus"],
        ),
        usage_case(
            "ooo set, a bad flag before a missing file",
            &["ooo", "set", "a@x", "--bogus", "--text", "@missing.txt"],
        ),
        usage_case(
            "ooo set, days 0",
            &["ooo", "set", "a@x", "--text", "x", "--days", "0"],
        ),
        usage_case(
            "ooo set, days 32",
            &["ooo", "set", "a@x", "--text", "x", "--days", "32"],
        ),
        usage_case(
            "ooo set, days a word",
            &["ooo", "set", "a@x", "--text", "x", "--days", "two"],
        ),
        usage_case(
            "ooo set, --until with no value",
            &["ooo", "set", "a@x", "--until"],
        ),
        usage_case(
            "ooo set, two addresses",
            &["ooo", "set", "a@x", "b@x", "--text", "x"],
        ),
        usage_case("ooo unset, no address", &["ooo", "unset"]),
        usage_case(
            "ooo show, an unknown flag",
            &["ooo", "show", "a@x", "--text", "x"],
        ),
        usage_case("footer, an unknown sub-command", &["footer", "on"]),
        usage_case("footer set, no text", &["footer", "set"]),
        usage_case("footer set, a word", &["footer", "set", "--text", "x", "y"]),
        usage_case(
            "footer set, --domain with no value",
            &["footer", "set", "--domain"],
        ),
        usage_case("footer unset, --text", &["footer", "unset", "--text", "x"]),
        usage_case(
            "footer show, --domain",
            &["footer", "show", "--domain", "acme.example"],
        ),
        usage_case("footer show, a word", &["footer", "show", "all"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn hostmail_notices_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "nanswer");
    let v = vec![
        // covers: hostmail ooo set
        ok_case(
            "ooo set",
            &[
                "ooo",
                "set",
                "sales@acme.example",
                "--text",
                "Away.\n\n",
                "--days",
                "007",
                "--until",
                "2026-12-01",
            ],
            r#"{"ok":true,"address":"sales@acme.example","days":7,"until":"2026-12-01"}"#,
        ),
        // covers: hostmail ooo set --json
        case(
            "ooo set --json, from a file",
            &[
                "ooo",
                "set",
                "--address",
                "sales@acme.example",
                "--text",
                "@note.txt",
                "--json",
            ],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            note_file,
            0,
            1,
        ),
        case(
            "ooo set, from a file",
            &["ooo", "set", "sales@acme.example", "--text", "@note.txt"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            note_file,
            0,
            1,
        ),
        // covers: hostmail ooo unset
        ok_case(
            "ooo unset",
            &["ooo", "unset", "sales@acme.example"],
            r#"{"ok":true,"address":"sales@acme.example"}"#,
        ),
        // covers: hostmail ooo unset --json
        ok_case(
            "ooo unset --json",
            &["ooo", "unset", "sales@acme.example", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail ooo show
        ok_case(
            "ooo show, on",
            &["ooo", "show", "sales@acme.example"],
            OOO_ON,
        ),
        // covers: hostmail ooo show --json
        ok_case(
            "ooo show --json",
            &["ooo", "show", "--json", "sales@acme.example"],
            OOO_ON,
        ),
        ok_case(
            "ooo show, on with no days or text",
            &["ooo", "show", "sales@acme.example"],
            r#"{"ok":true,"enabled":true,"days":null,"text":""}"#,
        ),
        ok_case(
            "ooo show, off",
            &["ooo", "show", "--address", "sales@acme.example"],
            r#"{"ok":true,"enabled":false}"#,
        ),
        // covers: hostmail footer set
        ok_case(
            "footer set",
            &["footer", "set", "--text", "Sent by Acme.\n"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail footer set --json
        ok_case(
            "footer set --json, a domain",
            &[
                "footer",
                "set",
                "--domain",
                "acme.example",
                "--text",
                "",
                "--json",
            ],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail footer unset
        ok_case("footer unset", &["footer", "unset"], r#"{"ok":true}"#),
        // covers: hostmail footer unset --json
        ok_case(
            "footer unset --json, a domain",
            &["footer", "unset", "--domain", "acme.example", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail footer show
        ok_case(
            "footer show, on",
            &["footer", "show"],
            r#"{"ok":true,"enabled":true,"text":"Sent by Acme."}"#,
        ),
        // covers: hostmail footer show --json
        ok_case(
            "footer show --json",
            &["footer", "show", "--json"],
            r#"{"ok":true,"enabled":false}"#,
        ),
        ok_case(
            "footer show, on with no text",
            &["footer", "show"],
            r#"{"ok":true,"enabled":true,"text":""}"#,
        ),
        // native: hostmail ooo set --text -
        Case {
            stdin: Some(b"Away from stdin.\nBack \xc3\xa9 soon.\n\n"),
            ..ok_case(
                "ooo set, the text on stdin",
                &["ooo", "set", "sales@acme.example", "--text", "-"],
                r#"{"ok":true,"address":"sales@acme.example"}"#,
            )
        },
        ok_case(
            "ooo set, the text on stdin: none (closed)",
            &["ooo", "set", "sales@acme.example", "--text", "-", "--json"],
            r#"{"ok":true}"#,
        ),
        Case {
            stdin: Some(b"read once"),
            ..ok_case(
                "ooo set, stdin twice: the second reads nothing",
                &["ooo", "set", "a@x", "--text", "-", "--text", "-"],
                r#"{"ok":true}"#,
            )
        },
        Case {
            stdin: Some(b"read once"),
            ..ok_case(
                "ooo set, stdin then the text itself",
                &["ooo", "set", "a@x", "--text", "-", "--text", "Away"],
                r#"{"ok":true}"#,
            )
        },
        Case {
            stdin: Some(b"never read"),
            ..usage_case(
                "ooo set --text -, a bad --days: the usage",
                &["ooo", "set", "a@x", "--text", "-", "--days", "99"],
            )
        },
        Case {
            stdin: Some(b"never read"),
            ..case(
                "ooo set --text -, no daemon: the gate first",
                &["ooo", "set", "a@x", "--text", "-"],
                &[],
                vec![],
                nothing,
                1,
                0,
            )
        },
        Case {
            stdin: Some(b"Sent by Acme.\n"),
            ..ok_case(
                "footer set, the text on stdin",
                &["footer", "set", "--text", "-"],
                r#"{"ok":true}"#,
            )
        },
        case(
            "an agent without the toggle",
            &["footer", "unset"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        compare(
            Compare::Traceback,
            case(
                "an answer that is a list",
                &["ooo", "show", "a@x"],
                &[],
                vec![ok("[1]")],
                owner_and_port,
                1,
                1,
            ),
        ),
        case(
            "closed, not sent again",
            &["footer", "set", "--text", "x"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// `--text -` with bytes that are not plain text. Not UTF-8: Python
/// holds each bad byte as its surrogate escape and `dumps(body).encode()`
/// raises, nothing sent (one byte; a run of them, named as a range; the
/// traceback compared as the rig compares one, its frames by Python's
/// version, and the exception's place by the unit tests of
/// `core::encode_body`). A
/// NUL: dropped by both, and from bash 4.4 bash warns on stderr naming
/// its own line, the named difference `nul-in-stdin-text`
/// (`scripts/cli-parity.sh`).
#[test]
fn text_on_stdin_that_is_not_plain() {
    let rig = Rig::new(VERB, "nstdin");
    let v = vec![
        compare(
            Compare::Traceback,
            Case {
                stdin: Some(b"Away \xff now.\n"),
                ..case(
                    "ooo set, stdin not UTF-8: the shell's raise, nothing sent",
                    &["ooo", "set", "sales@acme.example", "--text", "-"],
                    &[],
                    vec![],
                    owner_and_port,
                    1,
                    0,
                )
            },
        ),
        compare(
            Compare::Traceback,
            Case {
                stdin: Some(b"\xe2\x82 Sent by Acme \xff\xfe\n"),
                ..case(
                    "footer set, stdin with a run of bad bytes: the range",
                    &["footer", "set", "--text", "-", "--json"],
                    &[],
                    vec![],
                    owner_and_port,
                    1,
                    0,
                )
            },
        ),
        compare(
            Compare::NulInAnswer,
            Case {
                stdin: Some(b"Away \x00from\x00 Acme.\n\x00\n"),
                ..ok_case(
                    "ooo set, a NUL on stdin: dropped",
                    &["ooo", "set", "sales@acme.example", "--text", "-"],
                    r#"{"ok":true,"address":"sales@acme.example"}"#,
                )
            },
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// The verbs that change something, with `-h` or `--help` anywhere: the
/// help, and nothing sent.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "nhelp");
    const LINES: &[&[&str]] = &[
        &["ooo", "set", "a@x", "--text", "x", "-h"],
        &["ooo", "unset", "--help", "a@x"],
        &["footer", "set", "--text", "x", "--help"],
        &["footer", "unset", "-h"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case(
                "help, nothing sent",
                line,
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ));
    }
}
