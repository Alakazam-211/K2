//! `k5-cli review`, `skills` and `whats-new` against the shell CLI's,
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - `review approve|reject|feedback`: each sent once, its answer as it
//!   came; a refusal's body with exit 0 (the shell's, a known T5 row);
//!   the stops; no verb at all.
//! - `skills`: list (none, rows, names that pad, a reader that can't read
//!   it, and one list above 256 KiB), create, profile, remove, regenerate,
//!   and their stops.
//! - `whats-new`: news, none, `--all`, `--reset`, `--mark-seen`, a reader
//!   that can't read the answer.
//! - No answer: on the left of a pipe (the line, then SIGPIPE), in
//!   `$(…)` and at the top (the line, exit 1).
//! - Help never acts, and the shell's forms: `-h` and `--help` on each
//!   verb that changes something print the usage, exit 0, and send
//!   nothing.

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
fn review_answers_as_the_shell_does() {
    let rig = Rig::new("review", "all");
    let v = vec![
        // covers: review
        one("approve", &["approve", "ada", "fix/x"], r#"{"ok":true}"#, 0),
        one(
            "reject with a reason",
            &["reject", "ada b", "x", "--reason", "too big é"],
            "{\"ok\":true}\n",
            0,
        ),
        one("reject", &["reject", "ada"], "{}", 0),
        one(
            "feedback",
            &["feedback", "ada", "-m", "first", "--message", "second"],
            r#"{"ok":true}"#,
            0,
        ),
        case(
            "a refusal: its body, exit 0 (known T5 row)",
            &["approve", "ada", "b"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"Invalid or missing auth token"}"#.into(),
            )],
            owner_and_port,
            0,
            1,
        ),
        stop("approve: no branch", &["approve", "ada"], 1),
        stop("reject: no word (shift fails)", &["reject"], 1),
        stop("reject: an empty agent", &["reject", ""], 1),
        stop("feedback: no message", &["feedback", "ada"], 1),
        waited(case(
            "no answer",
            &["approve", "ada", "b"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a reset",
            &["approve", "ada", "b"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        shell(stop("no verb (the whole help)", &[], 1)),
        // Help never acts: the usage, exit 0, nothing sent.
        shell(stop(
            "approve -h: the usage, nothing sent",
            &["approve", "ada", "b", "-h"],
            0,
        )),
        shell(stop(
            "reject --help: the usage, nothing sent",
            &["reject", "ada", "--help"],
            0,
        )),
        shell(stop(
            "feedback -h: the usage, nothing sent",
            &["feedback", "ada", "-m", "x", "-h"],
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn skills_answer_as_the_shell_does() {
    let rig = Rig::new("skills", "all");
    let many = {
        let rows: Vec<String> = (0..3000)
            .map(|i| {
                format!(
                    r#"{{"name":"skill-{i}","role":"role {i} {}","agentType":"skill"}}"#,
                    "é".repeat(i % 50)
                )
            })
            .collect();
        format!(r#"{{"agents":[{}]}}"#, rows.join(","))
    };
    assert!(many.len() > 256 * 1024);
    let v = vec![
        // covers: skills
        one(
            "list",
            &[],
            r#"{"agents":[{"name":"ab","role":"r","agent_type":"t"},{"name":"édition","agentType":null}]}"#,
            0,
        ),
        one("list: a bare list", &["list"], r#"[{"name":"x"}]"#, 0),
        one("list: none", &["list"], r#"{"agents":[]}"#, 0),
        one("list: many", &["list"], &many, 0),
        one(
            "list: an error",
            &["list"],
            r#"{"error":"no workspace"}"#,
            1,
        ),
        one(
            "list: a name that is a number",
            &["list"],
            r#"[{"name":"a"},{"name":5}]"#,
            1,
        ),
        one("list: not JSON", &["list"], "<html>", 1),
        one(
            "create",
            &[
                "create", "--x", "my skill", "--from", "base", "--role", "Writes.",
            ],
            r#"{"name":"my-skill"}"#,
            0,
        ),
        one(
            "create: an error",
            &["create", "a"],
            r#"{"error":"exists"}"#,
            1,
        ),
        one(
            "profile",
            &["profile", "a"],
            r##"{"profile":"# A\nbody é\n"}"##,
            0,
        ),
        one(
            "profile: no text",
            &["profile", "a"],
            r#"{"name":"a","x":[1,{"b":"é"}]}"#,
            0,
        ),
        one("remove", &["remove", "a"], r#"{"ok":true}"#, 0),
        one(
            "delete: an error",
            &["delete", "a"],
            r#"{"error":"no such skill"}"#,
            1,
        ),
        one("regenerate", &["regenerate"], r#"{"updated":3}"#, 0),
        one(
            "regenerate: an error",
            &["regenerate"],
            r#"{"error":"busy"}"#,
            1,
        ),
        one(
            "regenerate: a count that is not a number (exit 0)",
            &["regenerate"],
            r#"{"updated":"x"}"#,
            0,
        ),
        stop("create: no name", &["create", "--role", "x"], 1),
        stop("profile: no name", &["profile"], 1),
        stop("a verb it doesn't know", &["frob"], 1),
        in_pipe(
            case(
                "list: no answer",
                &["list"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
            "",
            "",
        ),
        // Help never acts: the usage, exit 0, nothing sent.
        shell(stop("list -h: the usage, nothing sent", &["list", "-h"], 0)),
        shell(stop(
            "create -h: the usage, nothing sent",
            &["create", "a", "-h"],
            0,
        )),
        shell(stop(
            "remove --help: the usage, nothing sent",
            &["remove", "a", "--help"],
            0,
        )),
        shell(stop(
            "regenerate -h: the usage, nothing sent",
            &["regenerate", "-h"],
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn whats_new_answers_as_the_shell_does() {
    let rig = Rig::new("whats-new", "all");
    let v = vec![
        // covers: whats-new
        one(
            "news",
            &[],
            r#"{"has_new":true,"content":"  New: things é\n","current_version":"0.46.17"}"#,
            0,
        ),
        one(
            "none",
            &["--bogus"],
            r#"{"has_new":false,"current_version":"0.46.17","last_seen_version":null}"#,
            0,
        ),
        one(
            "--all",
            &["--all"],
            r#"{"content":"","current_version":"1","last_seen_version":"0.9"}"#,
            0,
        ),
        one(
            "a version that is a number",
            &["--all"],
            r#"{"content":"x","current_version":5}"#,
            1,
        ),
        one("not JSON", &[], "<html>", 1),
        one("a list", &[], "[1]", 1),
        one("--reset", &["--all", "--reset"], r#"{"ok":true}"#, 0),
        one("--mark-seen", &["--mark-seen"], "{\"ok\":true}\n\n", 0),
        waited(case(
            "--reset: no answer",
            &["--reset"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        case(
            "show: a reset",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        shell(stop(
            "--reset -h prints help, sends nothing",
            &["--reset", "-h"],
            0,
        )),
        shell(stop(
            "--mark-seen --help prints help, sends nothing",
            &["--mark-seen", "--help"],
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
