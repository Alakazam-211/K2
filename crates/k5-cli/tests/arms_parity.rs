//! The router's small arms (`context`, `state`, `status`, `checkin`,
//! `trust-folders`, `commit`, `reviews`) against the shell CLI's, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - The pointer arms: their lines and exit, the gate with no port.
//! - `trust-folders` and `commit` (`-m`, `--merge`): sent once, the
//!   answer as it came; a refusal's body with exit 0 (a known T5 row).
//! - `reviews`: rows, none, and a reader that can't read the answer
//!   (`|| cat` on an empty stdin: exit 0).
//! - No answer: the line, exit 1 (`reviews`: then SIGPIPE).
//! - Help never acts: `commit -h` is the shell's, which sends the commit
//!   (a bash bug K2 fixes: the row turns when the fix lands).

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
        // Help never acts: bash sends the commit (a bug K2 fixes; flip the row then).
        shell(one(
            "commit -h: bash sends it",
            &["-h"],
            r#"{"ok":true}"#,
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
        one(
            "a row it can't read: || cat",
            &[],
            r#"[{"agentName":"a"},{"agentName":null}]"#,
            0,
        ),
        one("not JSON: || cat", &[], "<html>", 0),
        waited(case(
            "no answer",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
    ] {
        rig.check(&c);
    }
}
