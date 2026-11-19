//! `k5-cli connections add` and `remove` against the shell CLI's
//! `cli/k2 connections add` and `remove`, on the parity rig ([`common::parity`]): the same requests,
//! stdout, stderr and exit code.
//!
//! - The answers: connected, already connected, a remote add that reaches
//!   a workspace (renamed or not), the daemon's warnings, the gate (its
//!   code, its hint, with and without Settings in it: JSON on stderr, exit
//!   3), the daemon's error (an object, text), a refusal with a permission
//!   card (the closing line), the bulletin, migrating, an older daemon's
//!   405 (the same call as a GET), not JSON, Python's raise part way.
//! - The words: no target or an empty one (usage, exit 1, nothing sent),
//!   a target that needs escaping, words after it.
//! - The owner from disk, a passport over TCP, a live cell socket (still
//!   TCP: the shell calls `cli_request_post` directly), a healed passport,
//!   the id tool's checks; the gates.
//! - Sent once: an empty 200 is the answer; an empty reply, a 5xx with no
//!   body and none in time are the line and death by SIGPIPE; a reset
//!   (56), not HTTP (1) and TMPDIR unusable (76) are curl's code under
//!   the reader's line; only a connect nothing accepted (7) is tried
//!   again. Each with one request.
//! - What the binary hands to the shell: help, a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "connections";
const UNPARSEABLE: &str = "Error: daemon returned an unparseable response: ''\n";
const ADDED: &str = r#"{"ok":true,"target":"beta"}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// No answer on the left of the pipe: the line, then SIGPIPE (141);
/// macOS's bash 3.2 runs the reader on an empty input instead.
fn in_pipe(c: Case) -> Case {
    Case {
        compare: Compare::Bash32GetPipe {
            stdout: UNPARSEABLE,
            stderr: "",
        },
        ..c
    }
}

#[test]
fn connections_add_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "add");
    let v = vec![
        case(
            "add",
            &["add", "beta"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a target that needs escaping",
            &["add", "a b/é&c=d#e"],
            &[],
            vec![ok(r#"{"target":"a b/é&c=d#e"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "words after the target are not read",
            &["add", "beta", "--json", "x"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "already connected",
            &["add", "beta"],
            &[],
            vec![ok(
                r#"{"ok":true,"noop":true,"target":"beta","message":"already é"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "already connected, odd values",
            &["add", "beta"],
            &[],
            vec![ok(r#"{"noop":1,"target":null,"message":[1,{"a":true}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a remote add that reaches a workspace, and the warnings",
            &["add", "seoca::quahog.example"],
            &[],
            vec![ok(
                r#"{"ok":true,"target":"seoca::quahog.example","bound":true,"handle":"quahog-site","displayName":"Quahog Site","bindWarning":"slow peer","warning":{"w":1}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the same name in another case is not shown twice",
            &["add", "seoca::quahog.example"],
            &[],
            vec![ok(
                r#"{"target":"x::","bound":1,"handle":"Quahog","displayName":" quahog "}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "bound with no handle: nothing reached",
            &["add", "x"],
            &[],
            vec![ok(
                r#"{"target":"x","bound":true,"handle":"","bindWarning":"no roster"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the gate: its code, exit 3",
            &["add", "beta"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"agents_create_connections_disabled","hint":"no é"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the gate: its hint with Settings in it",
            &["add", "beta"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"hint":"Agents can add connections: Settings"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the gate: a text error",
            &["add", "beta"],
            &[],
            vec![ok(
                r#"{"error":"this agent isn't allowed to create or remove connections"}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the gate: its code and no hint",
            &["add", "beta"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the daemon's error object",
            &["add", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no workspace named nope"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the daemon's error text",
            &["add", "nope"],
            &[],
            vec![ok(r#"{"error":"bad target"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error that is a list",
            &["add", "nope"],
            &[],
            vec![ok(r#"{"error":{"hint":["a",2]}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: the closing line",
            &["add", "beta"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin line when it worked",
            &["add", "beta"],
            &[],
            vec![ok(
                r#"{"ok":true,"target":"beta","bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            &["add", "beta"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(ADDED),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "an older daemon's 405: the same call as a GET",
            &["add", "beta"],
            &[],
            vec![
                Reply::Answer(405, "method not allowed for this route".into()),
                ok(ADDED),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "not JSON",
            &["add", "beta"],
            &[],
            vec![ok("<html>x</html>\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long",
            &["add", "beta"],
            &[],
            vec![ok(&"é".repeat(300))],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "not an object",
            &["add", "beta"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a hint that is a number",
            &["add", "beta"],
            &[],
            vec![ok(r#"{"error":{"hint":5}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "the gate and a hint that is a list",
            &["add", "beta"],
            &[],
            vec![ok(r#"{"error":{"code":"gated","hint":["x"]}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a target that is a number, after the first line",
            &["add", "beta"],
            &[],
            vec![ok(r#"{"bound":true,"handle":"h","target":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a display name that is a number",
            &["add", "beta"],
            &[],
            vec![ok(r#"{"bound":true,"handle":"h","displayName":7}"#)],
            owner_and_port,
            1,
            1,
        )),
        // The words.
        case("no target", &["add"], &[], vec![], owner_and_port, 1, 0),
        case(
            "an empty target",
            &["add", ""],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "PROJECT_PATH",
            &["add", "beta"],
            &[("K2_PROJECT_PATH", "/srv/acme ws")],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        // The gates come before the target.
        case("no port", &["add", "beta"], &[], vec![], owner_only, 1, 0),
        case("no token", &["add"], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &["add", "beta"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn connections_add_with_a_passport() {
    let rig = Rig::new(VERB, "addpass");
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
    let v = vec![
        with(
            h(&[]),
            case(
                "a host session: the passport over TCP",
                &["add", "beta"],
                &[],
                vec![ok(ADDED)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a live cell socket: still TCP",
                &["add", "beta"],
                &[],
                vec![ok(ADDED)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "the gate in a K2 terminal",
                &["add", "beta"],
                &[],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"agents_create_connections_disabled","hint":"Agents can add connections is off"}}"#.into(),
                )],
                port_only,
                3,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["add", "beta"],
                &[],
                vec![heal(&rig), ok(ADDED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &["add", "beta"],
                &[],
                vec![ok(ADDED)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["add", "beta"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["add", "beta"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// A POST that may have gone out is never sent again: one request each.
#[test]
fn connections_add_is_sent_once() {
    let rig = Rig::new(VERB, "addonce");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let v = vec![
        case(
            "an empty 200 is the answer",
            &["add", "beta"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        in_pipe(case(
            "an empty reply (52): the line, SIGPIPE",
            &["add", "beta"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        in_pipe(case(
            "a 5xx with no body: the line, SIGPIPE",
            &["add", "beta"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            141,
            1,
        )),
        case(
            "a reset (56): the reader's line",
            &["add", "beta"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP (000, curl 1): the reader's line",
            &["add", "beta"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset (76): the reader's line",
            &["add", "beta"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            &["add", "beta"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn connections_add_with_no_daemon() {
    let rig = Rig::new(VERB, "adddead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens (7): tried again, then the reader's line",
        &["add", "beta"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// No answer within 30 s (28): the line, SIGPIPE, one request. Slow.
#[test]
fn connections_add_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "addhang");
    rig.check(&in_pipe(case(
        "no answer in time (28)",
        &["add", "beta"],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        141,
        1,
    )));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn connections_add_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "addshell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "add --help",
            &["add", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "add beta -h",
            &["add", "beta", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "a port that is no port",
            &["add", "beta"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            1,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

const REMOVED: &str = r#"{"ok":true}"#;

#[test]
fn connections_remove_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "remove");
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
    let v = vec![
        case(
            "remove",
            &["remove", "beta"],
            &[],
            vec![ok(REMOVED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a target that needs escaping, words after it",
            &["remove", "seoca::quahog.example/é", "x"],
            &[],
            vec![ok(r#"{"ok":true,"warning":"not printed","target":"x"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the gate: exit 3",
            &["remove", "beta"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"gated","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the daemon's error",
            &["remove", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"not connected to nope"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error with no hint",
            &["remove", "nope"],
            &[],
            vec![ok(r#"{"error":{"code":"x","n":[1.5,null]}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: the closing line",
            &["remove", "beta"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "migrating, then the answer",
            &["remove", "beta"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(REMOVED),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "not JSON",
            &["remove", "beta"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "not an object",
            &["remove", "beta"],
            &[],
            vec![ok("\"s\"")],
            owner_and_port,
            1,
            1,
        )),
        case("no target", &["remove"], &[], vec![], owner_and_port, 1, 0),
        case(
            "no port",
            &["remove", "beta"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a K2 terminal: the passport over TCP",
                &["remove", "beta"],
                &[],
                vec![ok(REMOVED)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["remove", "beta"],
                &[],
                vec![heal(&rig), ok(REMOVED)],
                port_only,
                0,
                2,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["remove", "beta"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One request each, as for `add`.
#[test]
fn connections_remove_is_sent_once() {
    let rig = Rig::new(VERB, "removeonce");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let v = vec![
        case(
            "an empty 200 is the answer",
            &["remove", "beta"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        in_pipe(case(
            "an empty reply (52): the line, SIGPIPE",
            &["remove", "beta"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        in_pipe(case(
            "a 5xx with no body: the line, SIGPIPE",
            &["remove", "beta"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            141,
            1,
        )),
        case(
            "a reset (56): the reader's line",
            &["remove", "beta"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP (000, curl 1): the reader's line",
            &["remove", "beta"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset (76): the reader's line",
            &["remove", "beta"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn connections_remove_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "removeshell");
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "remove --help",
            &["remove", "beta", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )
    });
}
