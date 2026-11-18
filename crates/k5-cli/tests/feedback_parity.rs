//! `k5-cli feedback list` and `k5-cli feedback show` against the shell
//! CLI's `cli/k2 feedback list|show` (and `tickets`, the same command), on
//! the parity rig ([`common::parity`]): the same requests, stdout, stderr
//! and exit code for every local path of the two.
//!
//! - The words: `--all`, `--status <s>` (each status, an empty one, a bad
//!   one, none after it), `--json`, an unknown flag, an extra word, words
//!   that are not ASCII.
//! - The answers: rows of every shape (long titles, unicode, numbers,
//!   `null`s, `true`), none, no list, many rows; `--json`; the daemon's
//!   error (an object with each code, text, a number, `null`), a refusal
//!   with a permission card (the closing line), not JSON (short, long,
//!   empty), not an object, Python's raises, the bulletin, migrating,
//!   trailing newlines.
//! - The owner from disk, a passport over TCP (the workspace from
//!   `/cli/whoami`, on the cell socket or TCP), re-minted, healed, dead;
//!   `HOOK_SOCK` with the owner on disk (PRD 13.9 #4); PROJECT_PATH; the
//!   gates; the passport rule's would-refuse mark.
//! - No answer: an empty reply, a 5xx with no body and none in time are
//!   the line and exit 1; a reset (56), not HTTP (000), TMPDIR unusable
//!   (75), an empty 200 and no daemon (7) are the `daemon_unreachable`
//!   line and exit 1.
//! - What the binary hands to the shell: `feedback` alone, help, another
//!   verb, a port that is no port.
//!
//! Neither verb sends a request on the left of a pipe (`list` holds its
//! answer in `$(…)`, `show`'s Python reads it itself), so no case dies of
//! SIGPIPE; every case runs with stdout and stderr on pipes.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "feedback";
const LIST: &str = r#"{"items":[
  {"id":"fb_0123456789","status":"waiting","priority":2,"kind":"question","title":"Ship the release? é 日本"},
  {"id":"fb_x","status":"needs_discussion","priority":10,"kind":"decision","title":"LONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONG"},
  {"id":12345678901,"status":null,"priority":1.5,"kind":true,"title":["a",1]},
  {"status":"answered"},
  {}]}"#;
const PROJ_WS: &str = r#"{"workspace":"acme","path":"/srv/acme ws é"}"#;

fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"fb_{i:06}","status":"waiting","priority":{},"kind":"question","title":"Row {i} {}"}}"#,
                i % 4,
                "é".repeat(i % 30)
            )
        })
        .collect();
    format!(r#"{{"items":[{}]}}"#, rows.join(","))
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn no_tmp(c: Case) -> Case {
    Case {
        no_tmpdir: true,
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

#[test]
fn list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let long_text = format!("not json {}", "x".repeat(400));
    let many = many(300);
    let v = vec![
        case("list", &["list"], &[], vec![ok(LIST)], owner_and_port, 0, 1),
        case(
            "--json",
            &["list", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--all",
            &["list", "--all"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--status planned",
            &["list", "--status", "planned", "--all"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "each status",
            &[
                "list",
                "--status",
                "needs_discussion",
                "--status",
                "dismissed",
            ],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty status is none",
            &["list", "--status", "", "--all", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no rows",
            &["list"],
            &[],
            vec![ok(r#"{"items":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no list",
            &["list"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many rows",
            &["list"],
            &[],
            vec![ok(&many)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many rows, --json",
            &["list", "--json"],
            &[],
            vec![ok(&many)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json keeps odd values",
            &["list", "--json"],
            &[],
            vec![ok(
                r#"{"items":[{"id":1e16,"t":NaN,"u":"😀 \u0000"}],"items":[],"n":-0,"big":123456789012345678901234567890}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trailing newlines go",
            &["list"],
            &[],
            vec![ok("{\"items\":[{\"id\":\"a\"}]}\n\n\n")],
            owner_and_port,
            0,
            1,
        ),
        // The daemon's error.
        case(
            "not_found: exit 4",
            &["list"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"nothing é"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "ambiguous_id: exit 4",
            &["list"],
            &[],
            vec![ok(r#"{"error":{"code":"ambiguous_id","hint":"a, b"}}"#)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "usage: exit 2",
            &["list"],
            &[],
            vec![ok(r#"{"error":{"code":"usage","hint":"no"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "another code: exit 1",
            &["list", "--json"],
            &[],
            vec![ok(r#"{"error":{"code":["usage"],"x":1}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error as text",
            &["list"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"it broke — badly"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error of null is none",
            &["list"],
            &[],
            vec![ok(r#"{"error":null,"items":[{"id":"x"}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal with a permission card: the closing line",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin",
            &["list"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            &["list"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(LIST),
            ],
            owner_and_port,
            0,
            2,
        ),
        // Not JSON.
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("nope é")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long",
            &["list"],
            &[],
            vec![ok(&long_text)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, quotes",
            &["list"],
            &[],
            vec![ok("it's \"x\"\t\u{1}")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty 200",
            &["list"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "only newlines",
            &["list"],
            &[],
            vec![ok("\n\n")],
            owner_and_port,
            1,
            1,
        ),
        // Python's raises.
        traceback(case(
            "not an object",
            &["list"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an error that is a number",
            &["list"],
            &[],
            vec![ok(r#"{"error":3}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "items of null",
            &["list"],
            &[],
            vec![ok(r#"{"items":null}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a row that is no object, part way",
            &["list"],
            &[],
            vec![ok(r#"{"items":[{"id":"a"},"x"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "items as an empty object: no rows",
            &["list"],
            &[],
            vec![ok(r#"{"items":{}}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "items as an object: its keys, then a raise",
            &["list"],
            &[],
            vec![ok(r#"{"items":{"k":1}}"#)],
            owner_and_port,
            1,
            1,
        )),
        // The words.
        case(
            "an unknown flag",
            &["list", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("a dash", &["list", "-"], &[], vec![], owner_and_port, 2, 0),
        case(
            "an extra word",
            &["list", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an empty word",
            &["list", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--status alone",
            &["list", "--status"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad status",
            &["list", "--status", "nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad status after a bad flag",
            &["list", "--status", "nope", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a word that is not ASCII",
            &["list", "日本 é"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "PROJECT_PATH",
            &["list"],
            &[("K2_PROJECT_PATH", "/srv/acme ws é")],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "in a session by its markers: the would-refuse mark",
            &["list"],
            &[("K2_CELL", "canonical")],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // The gates.
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
        case("no token", &["list"], &[], vec![], port_only, 1, 0),
        case(
            "the gate comes before the words",
            &["list", "--bogus"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &["list"],
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
fn list_with_a_passport() {
    let rig = Rig::new(VERB, "lpass");
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(rig.stub.port, &[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let v = vec![
        with(
            h(&[]),
            case(
                "whoami's path is the workspace",
                &["list"],
                &[],
                vec![ok(PROJ_WS), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "whoami not JSON",
                &["list", "--all"],
                &[],
                vec![ok("nope"), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_PROJECT_PATH", "/set")]),
            case(
                "PROJECT_PATH: no whoami",
                &["list"],
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
                &["list"],
                &[],
                vec![ok(PROJ_WS), ok(LIST)],
                healed_on_disk,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_PROJECT_PATH", "/set")]),
            case(
                "a refused passport heals once",
                &["list", "--json"],
                &[],
                vec![heal(&rig), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_PROJECT_PATH", "/set")]),
            case(
                "a dead passport: the probe, then the restart line",
                &["list"],
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
                cell(&[("K2_PROJECT_PATH", "/set")]),
                case(
                    "HOOK_SOCK and the owner on disk (PRD 13.9 #4)",
                    &["list"],
                    &[],
                    vec![ok(LIST)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
        with(
            cell(&[]),
            case(
                "the cell socket for whoami, TCP for the list",
                &["list"],
                &[],
                vec![ok(PROJ_WS), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: TCP",
                &["list"],
                &[],
                vec![ok(PROJ_WS), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: an open tool, the owner",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn list_with_no_answer() {
    let rig = Rig::new(VERB, "lnone");
    let v = vec![
        waited(case(
            "an empty reply: the line, exit 1",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "a 5xx with no body: the line, exit 1",
            &["list"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a reset (56): daemon_unreachable",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP (000): daemon_unreachable",
            &["list"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset (75): daemon_unreachable",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            &["list"],
            &[],
            vec![ok(LIST)],
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
fn list_with_no_daemon() {
    let rig = Rig::new(VERB, "ldead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens (7): daemon_unreachable",
        &["list"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// No answer within 30 s: the line, exit 1. Slow.
#[test]
fn list_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "lhang");
    rig.check(&waited(case(
        "no answer in time (28)",
        &["list"],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        1,
        1,
    )));
}

/// `tickets` is the same command.
#[test]
fn tickets_list_is_feedback_list() {
    let rig = Rig::new("tickets", "list");
    let v = vec![
        case(
            "tickets list",
            &["list"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tickets list --status waiting --json",
            &["list", "--status", "waiting", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tickets list x",
            &["list", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn feedback_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let v = vec![
        shell(case(
            "feedback alone",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "--help",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "list -h",
            &["list", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "list --status --help",
            &["list", "--status", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "template",
            &["template"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "a port that is no port",
            &["list"],
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
