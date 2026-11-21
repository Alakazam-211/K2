//! `k5-cli glossary` against the shell CLI's `cli/k2 glossary`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - The words: no term, an empty one, a term (escaped, unicode, a help
//!   flag as the term, which the front hands to the shell), words after
//!   it. (A term that is not UTF-8 is
//!   escaped by [`k5_cli::transport::encode::quote_bytes`], whose own
//!   tests pin it; the rig's words are UTF-8.)
//! - The list's answers: terms (odd types, unicode padding, many: one
//!   answer above 256 KiB), none, empty, not JSON, an object, a raise part
//!   way, a refusal with a permission card (the closing line).
//! - One term's answers: found, an error (key, list item, substring), not
//!   JSON, empty, odd types.
//! - The owner from disk, a passport over TCP, a re-minted, a healed and
//!   a dead passport, PRD 13.9 #4's owner token with a passport, the gates
//!   (a help word lets the virtual desktop's guard by).
//! - No answer (an empty reply, a 5xx with no body, none in time): the
//!   line, then death by SIGPIPE (141), after the title for the list; a
//!   reset, not HTTP, TMPDIR unusable and no daemon: the reader on an
//!   empty input.
//! - What the binary hands to the shell: a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "glossary";
const LIST: &str = r#"[
    {"term":"cell","summary":"a pane é 日本 🚀"},
    {"term":"überlonger-than-sixteen","summary":null},
    {"term":["x",1],"summary":{"k":2.5}},
    {"summary":true},
    {}]"#;
const TERM: &str = r#"{"term":"réle","definition":"what an agent does 🚀"}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// A line the front hands to the shell: one with `-h` or `--help` in it
/// (no glossary line is a help page).
fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn many(n: usize) -> String {
    let items: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"term":"term-{i}","summary":"{}"}}"#,
                "s".repeat(i % 120)
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

#[test]
fn the_list() {
    let rig = Rig::new(VERB, "list");
    let big = many(5000);
    assert!(
        big.len() > 256 * 1024,
        "the large list is {} bytes",
        big.len()
    );
    let v = vec![
        // covers: glossary
        case("terms", &[], &[], vec![ok(LIST)], owner_and_port, 0, 1),
        case(
            "an empty term is none",
            &[""],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many terms, above 256 KiB",
            &[],
            &[],
            vec![Reply::Answer(200, big)],
            owner_and_port,
            0,
            1,
        ),
        case("no terms", &[], &[], vec![ok("[]")], owner_and_port, 0, 1),
        case("null", &[], &[], vec![ok("null")], owner_and_port, 0, 1),
        case(
            "an empty object",
            &[],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty body",
            &[],
            &[],
            vec![ok("")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not JSON",
            &[],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an object: Python raises, exit 1",
            &[],
            &[],
            vec![ok(r#"{"error":"nope"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a raise part way",
            &[],
            &[],
            vec![ok(r#"[{"term":"a"},7,{"term":"b"}]"#)],
            owner_and_port,
            1,
            1,
        ),
        case("a number", &[], &[], vec![ok("5")], owner_and_port, 1, 1),
        case(
            "a refusal with a permission card",
            &[],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin",
            &[],
            &[],
            vec![ok(&format!("[{{\"term\":\"a\"}},{BULLETIN}]"))],
            owner_and_port,
            0,
            1,
        ),
        // The gates.
        case("no port", &[], &[], vec![], owner_only, 1, 0),
        case("no token", &[], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        shell(case(
            "a help word lets the guard by: the connection gate",
            &["x", "--help"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            1,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn one_term() {
    let rig = Rig::new(VERB, "term");
    let v = vec![
        case(
            "a term",
            &["role"],
            &[],
            vec![ok(TERM)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "escaped, words after it unread",
            &["a b&c/é#?", "more", "--json"],
            &[],
            vec![ok(TERM)],
            owner_and_port,
            0,
            1,
        ),
        // The family's own help lines (a bash bug: help asks).
        // native: glossary --help
        case(
            "--help is a term",
            &["--help"],
            &[],
            vec![ok(TERM)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "-h alone is a term",
            &["-h"],
            &[],
            vec![ok(TERM)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--help: the virtual desktop's guard lets it by, the connection gate",
            &["--help"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        shell(case(
            "-h is a term",
            &["-h", "x"],
            &[],
            vec![ok(TERM)],
            owner_and_port,
            0,
            1,
        )),
        case(
            "no fields",
            &["x"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd types",
            &["x"],
            &[],
            vec![ok(r#"{"term":null,"definition":[1,"a",{"b":false}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an error key",
            &["x"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"unknown term"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error item",
            &["x"],
            &[],
            vec![ok(r#"["error"]"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error substring",
            &["x"],
            &[],
            vec![ok(r#""an error""#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a string",
            &["x"],
            &[],
            vec![ok(r#""fine""#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["x"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty body",
            &["x"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card",
            &["x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the gate comes first",
            &["x"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let v = vec![
        with(
            h(&[]),
            case("over TCP", &["x"], &[], vec![ok(TERM)], port_only, 0, 1),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "with a cell socket: TCP still",
                &[],
                &[],
                vec![ok(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &["x"],
                &[],
                vec![ok(TERM)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["x"],
                &[],
                vec![heal(&rig), ok(TERM)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a dead passport: the probe, then the restart line",
                &["x"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                1,
                2,
            ),
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                h(&[("K2_HOOK_SOCK", "{nosock}")]),
                case(
                    "HOOK_SOCK gone, the owner on disk (PRD 13.9 #4)",
                    &["x"],
                    &[],
                    vec![ok(TERM)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
        with(
            h(&[("K2_NO_OWNER_FALLBACK", "1")]),
            case(
                "the guard lets a passport by",
                &["x"],
                &[],
                vec![ok(TERM)],
                nothing,
                0,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

fn in_pipe(stdout: &'static str, c: Case) -> Case {
    Case {
        compare: Compare::Bash32PipeReader { stdout, stderr: "" },
        ..c
    }
}

#[test]
fn with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let mut v = vec![
        in_pipe(
            "",
            case(
                "a term, an empty reply: the line, then SIGPIPE",
                &["x"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
        ),
        in_pipe(
            "",
            case(
                "a term, a 5xx with no body",
                &["x"],
                &[],
                vec![Reply::Answer(502, String::new())],
                owner_and_port,
                141,
                1,
            ),
        ),
        case(
            "a term, a reset",
            &["x"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the list, a reset: no terms, exit 56",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "the list, not HTTP: no terms, exit 1",
            &[],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["x"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset",
            &["x"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            &["x"],
            &[],
            vec![ok(TERM)],
            owner_and_port,
            0,
            1,
        )),
    ];
    // The list prints its title before the request. macOS's bash 3.2 then
    // runs the reader on nothing (bash32-pipe-reader), which the rig
    // compares only where the binary's stdout is empty: these run on
    // Linux.
    if cfg!(target_os = "linux") {
        v.push(case(
            "the list, an empty reply: the title, the line, then SIGPIPE",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        ));
    }
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    for args in [&[][..], &["x"]] {
        rig.check(&case(
            "nothing listens",
            args,
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            if args.is_empty() { 7 } else { 1 },
            0,
        ));
    }
}

/// No answer within 30 s: the line, then SIGPIPE. Slow.
#[test]
fn with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&in_pipe(
        "",
        case(
            "no answer in time (28)",
            &["x"],
            &[],
            vec![Reply::Hang(Duration::from_secs(35))],
            owner_and_port,
            141,
            1,
        ),
    ));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "a port that is no port",
            &["x"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            1,
            0,
        )
    });
}
