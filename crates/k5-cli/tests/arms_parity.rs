//! The router's small arms (`context`, `state`, `status`, `checkin`,
//! `trust-folders`, `commit`, `reviews`) against the shell CLI's, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - The pointer arms: their lines and exit, the gate with no port.
//! - `trust-folders` and `commit` (`-m`, `--merge`): sent once, the
//!   answer as it came; a refusal's body with exit 0 (a known T5 row).
//! - `reviews`: rows, none, and an answer the reader can't use (not
//!   JSON, empty, an error object: one error line, exit 1; a row it
//!   can't read: the traceback); the caller's stdin is never read.
//! - No answer: the line, exit 1 (`reviews`: then SIGPIPE).
//! - Help never acts: `commit -h` is the shell's usage, nothing sent.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

/// A request on the left of a pipe into the shell's reader, with no
/// answer: the binary's line, then SIGPIPE. macOS's `/bin/bash` 3.2 runs
/// the reader on the empty input, which prints `stdout` and `stderr`, and
/// exits 1 (`scripts/cli-parity.sh`'s named difference
/// `bash32-pipe-reader`, through [`Compare::Bash32PipeReader`]).
fn in_pipe(c: Case, stdout: &'static str, stderr: &'static str) -> Case {
    Case {
        compare: Compare::Bash32PipeReader { stdout, stderr },
        ..c
    }
}

fn one(name: &'static str, args: &'static [&'static str], body: &str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

#[test]
fn the_pointer_arms() {
    for (noun, cases) in [
        (
            "context",
            vec![
                stop("context", &["list"], 2),
                case("context: no port", &[], &[], vec![], owner_only, 1, 0),
            ],
        ),
        ("state", vec![stop("state", &["x"], 1)]),
        ("status", vec![stop("status", &[], 1)]),
        (
            "checkin",
            vec![
                stop("checkin", &[], 2),
                stop("checkin --done", &["--done"], 2),
                stop("checkin --status", &["x", "--status", "y"], 2),
                stop("checkin --status=x is plain", &["--status=x"], 2),
            ],
        ),
    ] {
        let rig = Rig::new(noun, "arm");
        for c in cases {
            rig.check(&c);
        }
    }
}

#[test]
fn trust_folders_and_commit() {
    let rig = Rig::new("trust-folders", "all");
    for c in [
        one("trust-folders", &["ignored"], "{\"folders\":[]}\n", 0),
        waited(case(
            "trust-folders: no answer",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
    ] {
        rig.check(&c);
    }
    let rig = Rig::new("commit", "all");
    for c in [
        one("commit", &[], r#"{"ok":true}"#, 0),
        one(
            "commit -m",
            &["x", "-m", "fix é & more"],
            r#"{"ok":true}"#,
            0,
        ),
        one(
            "commit --merge",
            &["--message", "m", "--merge"],
            r#"{"ok":true}"#,
            0,
        ),
        case(
            "a refusal: its body, exit 0 (known T5 row)",
            &[],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"Invalid or missing auth token"}"#.into(),
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a reset",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        // Help never acts: the usage, and no commit.
        shell(stop("commit -h: the usage, nothing sent", &["-h"], 0)),
        shell(stop(
            "commit -m x --help: the usage, nothing sent",
            &["-m", "x", "--help"],
            0,
        )),
    ] {
        rig.check(&c);
    }
}

#[test]
fn reviews() {
    let rig = Rig::new("reviews", "all");
    for c in [
        one(
            "rows",
            &[],
            r#"[{"agentName":"ada","branch":"fix/é","workItems":[1,2],"diffSummary":[{}]},{"agentName":7}]"#,
            0,
        ),
        one("none", &[], "[]", 0),
        Case {
            compare: Compare::Traceback,
            ..one(
                "a row it can't read: the traceback",
                &[],
                r#"[{"agentName":"a"},{"agentName":null}]"#,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..one("a list of strings: the traceback", &[], r#"["x"]"#, 1)
        },
        one("not JSON: the error line", &[], "<html>", 1),
        one("an empty answer: the error line", &[], "", 1),
        one("an error: its line", &[], r#"{"error":"nope é"}"#, 1),
        one(
            "an error object: its hint",
            &[],
            r#"{"error":{"hint":"h","code":"c"}}"#,
            1,
        ),
        one(
            "an error object: its code",
            &[],
            r#"{"error":{"hint":"","code":"c"}}"#,
            1,
        ),
        one("an object with no error", &[], r#"{"ok":true}"#, 1),
        one("null: none", &[], "null", 0),
        // The caller's own stdin is never read.
        Case {
            stdin: Some(b"MARKER-FROM-STDIN\n"),
            ..one("not JSON, with a stdin: never read", &[], "<html>", 1)
        },
        in_pipe(
            case(
                "no answer",
                &[],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
            "",
            "Error: k2 reviews: no answer from K2 on /cli/reviews\n",
        ),
    ] {
        rig.check(&c);
    }
}
