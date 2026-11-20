//! `k5-cli capabilities` against the shell CLI's `cmd_capabilities`
//! (`cli/src/explain.sh`), on the parity rig ([`common::parity`]): the
//! same requests, stdout, stderr and exit code.
//!
//! - The answer: the box's rows (a missing capability with and without
//!   its refusal id), the compute nodes' rows, none, the box setup line
//!   (ready, failing); odd types; one answer above 256 KiB.
//! - The words: `--json`, `--node <name>` (escaped, empty, missing),
//!   a word it doesn't take.
//! - Where Python raises: not JSON, an empty answer, a row that is no
//!   object, a doctor that is no object.
//! - Refusals: a 400 (exit 2), a 404, a registry id with its `fix:` line
//!   (not with `--json`), a permission card.
//! - No answer: no daemon, a reset, a closed connection.
//! - The session: an agent on its cell socket and over TCP, `HOOK_SOCK`
//!   with no passport (refused before anything is sent), the gates.
//! - `_CP_404_EXIT` exported changes nothing (a 404 is 3).
//! - What the binary hands to the shell: a `CODEX_` setting in a session,
//!   a line with a help flag that is no help page. `capabilities --help`
//!   is a help page.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "capabilities";
const CAPS: &str = r#"{"box":[{"ok":true,"cap":"podman","detail":"5.0 é"},{"ok":false,"cap":"tls-certificates-for-apps","detail":null,"missing":{"reason":"no cert","fix":"add one","code":"app_box_not_ready"}},{"cap":"x","missing":{"reason":["r"],"fix":2.5}}],"compute":[{"ok":true,"cap":"node-1","detail":"linux"}],"doctor":{"ready":false,"failing":["podman","dns é"]}}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn the_answer() {
    let rig = Rig::new(VERB, "answer");
    let v = vec![
        // covers: capabilities
        case("the rows", &[], &[], vec![ok(CAPS)], owner_and_port, 0, 1),
        // covers: capabilities --json
        case(
            "--json",
            &["--json"],
            &[],
            vec![ok(CAPS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--node, escaped",
            &["--node", "n 1&é", "--json"],
            &[],
            vec![ok(CAPS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--node empty: no query",
            &["--node", ""],
            &[],
            vec![ok(CAPS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "nothing in it",
            &[],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ready, no failing",
            &[],
            &[],
            vec![ok(r#"{"box":[],"compute":[],"doctor":{"ready":true}}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "odd types",
            &[],
            &[],
            vec![ok(
                r#"{"box":[{"ok":"yes","cap":7,"detail":{"a":1}}],"compute":{"k":1},"doctor":{"ready":1,"failing":"ab"}}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        case(
            "the bulletin",
            &[],
            &[],
            vec![ok(&format!(
                r#"{{"box":[],{}}}"#,
                &BULLETIN[1..BULLETIN.len() - 1]
            ))],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One answer above 256 KiB: thousands of rows.
#[test]
fn a_large_answer() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(
                r#"{{"ok":{},"cap":"node-{n}","detail":"linux x86_64 {n} é","missing":{{"reason":"r{n}","fix":"f","code":"c{n}"}}}}"#,
                n % 2 == 0
            )
        })
        .collect();
    let body = format!(r#"{{"box":[],"compute":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&[][..], &["--json"]] {
        rig.check(&case(
            "a large answer",
            args,
            &[],
            vec![ok(&body)],
            owner_and_port,
            0,
            1,
        ));
    }
}

#[test]
fn the_words() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        case(
            "--node with no name",
            &["--node"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("a word", &["x"], &[], vec![], owner_and_port, 2, 0),
        case(
            "a word after --json",
            &["--json", "-j"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--help: the page",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case("-h: the page", &["-h"], &[], vec![], nothing, 0, 0),
        shell(case(
            "a help flag after a word",
            &["--json", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn where_python_raises() {
    let rig = Rig::new(VERB, "raise");
    let v = vec![
        traceback(case(
            "not JSON",
            &[],
            &[],
            vec![ok("not json")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case("empty", &[], &[], vec![ok("")], owner_and_port, 1, 1)),
        traceback(case(
            "a row that is no object",
            &[],
            &[],
            vec![ok(r#"{"box":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a compute row that is no object",
            &[],
            &[],
            vec![ok(r#"{"compute":["a"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a missing that is no object",
            &[],
            &[],
            vec![ok(r#"{"box":[{"missing":[1]}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a doctor that is no object",
            &[],
            &[],
            vec![ok(r#"{"doctor":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a list",
            &[],
            &[],
            vec![ok("[]")],
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
fn refusals_and_no_answer() {
    let rig = Rig::new(VERB, "refuse");
    let dead = dead_port();
    let refusal = r#"{"error":{"code":"app_box_not_ready","hint":"x"}}"#;
    let v = vec![
        // `_compute_fail` takes its exit for a 404 from the caller: an
        // exported `_CP_404_EXIT` changes nothing.
        case(
            "_CP_404_EXIT set: nothing reads it",
            &[],
            &[("_CP_404_EXIT", "5")],
            vec![Reply::Answer(404, r#"{"error":"x"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a refusal with its fix line",
            &[],
            &[],
            vec![Reply::Answer(409, refusal.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "with --json: no fix line",
            &["--json"],
            &[],
            vec![Reply::Answer(409, refusal.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 400",
            &[],
            &[],
            vec![Reply::Answer(400, r#"{"error":"bad"}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a 404",
            &["--node", "nope"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no_node"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a permission card",
            &[],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "no daemon",
            &[],
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ),
        case(
            "a reset",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a closed connection",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn the_session_and_the_gates() {
    let rig = Rig::new(VERB, "session");
    let v = vec![
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "an agent on its cell socket",
                &[],
                &[],
                vec![ok(CAPS)],
                nothing,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case("an agent over TCP", &[], &[], vec![ok(CAPS)], nothing, 0, 1),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
            case(
                "HOOK_SOCK and no passport: refused",
                &[],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "the socket gone, the passport refused",
                &[],
                &[],
                vec![Reply::Answer(401, REFUSED.into())],
                nothing,
                3,
                1,
            ),
        ),
        case("no port: the gate", &[], &[], vec![], owner_only, 1, 0),
        case("no token: the gate", &[], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &[],
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
fn hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let v = vec![with(
        with_passport(&[("K2_HOOK_SOCK", "{sock}"), ("CODEX_THREAD_ID", "t1")]),
        shell(case(
            "a CODEX_ setting",
            &[],
            &[],
            vec![ok(CAPS)],
            nothing,
            0,
            1,
        )),
    )];
    for c in v {
        rig.check(&c);
    }
}
