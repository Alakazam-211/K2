//! `k5-cli activity` against the shell CLI's `cli/k2 activity`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code for every local path `cmd_feed` has.
//!
//! - The words: a limit, `--limit`, `--agent` (escaped), `--json`, the
//!   last limit winning, a value flag with no value, an unknown flag, a
//!   limit that is not a number (or empty).
//! - The answers: events (many, unicode, long text, odd types, a route
//!   with both ends, `at` as an int, a float, a bool, under another `TZ`),
//!   none, an error object (hint, code, neither) or text, a list, not
//!   JSON (short, long, empty), `--json`, a refusal with a permission
//!   card (the closing line), the bulletin, migrating, Python's raise part
//!   way.
//! - The workspace: `PROJECT_PATH`, a PWD through a link, and with a
//!   passport `_cli_use_passport_project`'s `/cli/whoami` first (over TCP,
//!   or the cell socket and then TCP; a path, none, not JSON, no answer).
//! - The owner from disk, a passport over TCP, a re-minted, a healed and a
//!   dead passport, PRD 13.9 #4's owner token with a passport, the gates.
//! - No answer (an empty reply, a 5xx with no body, none in time): the
//!   line, then death by SIGPIPE (141); a reset, not HTTP, TMPDIR
//!   unusable and no daemon: the reader on an empty input, exit 1.
//! - What the binary hands to the shell: `--help`, a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "activity";
const FEED: &str = r#"{"feed":[
    {"at":1763400000,"agent":"acme","type":"msg","from":"acme","to":"beta","summary":"hello é 日本 🚀"},
    {"at":1763400061.9,"agent":null,"type":"done","summary":null},
    {"at":true,"type":"x"},
    {"agent":"a","from":"x"}]}"#;
const PROJ_WS: &str = r#"{"workspace":"acme","path":"/srv/acme ws é"}"#;

fn many(n: usize) -> String {
    let items: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"at":{},"agent":"agent-{i}","type":"t{}","summary":"{}"}}"#,
                1_700_000_000 + i * 3607,
                i % 7,
                "s".repeat(i % 90)
            )
        })
        .collect();
    format!(r#"{{"feed":[{}]}}"#, items.join(","))
}

fn reply(body: String) -> Reply {
    Reply::Answer(200, body)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn in_pipe(c: Case) -> Case {
    Case {
        compare: Compare::Bash32PipeReader {
            stdout: "",
            stderr: "Error: the K2 daemon gave no usable answer: ''\n",
        },
        ..c
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn words(v: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(v.into_boxed_slice())
}

#[test]
fn activity_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let traceback = |c: Case| Case {
        compare: Compare::Traceback,
        ..c
    };
    let v = vec![
        case("the feed", &[], &[], vec![ok(FEED)], owner_and_port, 0, 1),
        case("a limit", &["5"], &[], vec![ok(FEED)], owner_and_port, 0, 1),
        case(
            "the last limit wins",
            &["5", "--limit", "7", "9"],
            &[],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--agent, escaped",
            &["--agent", "rev iewer&é#", "--limit", "3"],
            &[],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty --agent sends none",
            &["--agent", ""],
            &[],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--agent takes a flag as its value",
            &["--agent", "--json"],
            &[],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["--json"],
            &[],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json, an odd answer",
            &["--json"],
            &[],
            vec![ok(
                r#"{"feed":[{"x":1e16,"y":-0,"z":NaN,"w":"🚀"}],"k":{}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no events",
            &[],
            &[],
            vec![ok(r#"{"feed":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case("no feed", &[], &[], vec![ok("{}")], owner_and_port, 0, 1),
        case(
            "many events",
            &["500"],
            &[],
            vec![reply(many(500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a long summary",
            &[],
            &[],
            vec![reply(format!(
                r#"{{"feed":[{{"at":1,"summary":"{}"}}]}}"#,
                "ü".repeat(12000)
            ))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd types",
            &[],
            &[],
            vec![ok(
                r#"{"feed":[{"at":0,"agent":[1,"a"],"type":{"k":null},"summary":2.5,"from":1,"to":true}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "another TZ",
            &[],
            &[("TZ", "XST-5:30")],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an error object's hint",
            &[],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"nope é"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object's code",
            &[],
            &[],
            vec![ok(r#"{"error":{"code":"gated","hint":""}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object with neither",
            &[],
            &[],
            vec![ok(r#"{"error":{"x":[1]}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a text error",
            &[],
            &[],
            vec![ok(r#"{"error":"workspace not registered"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a false error",
            &[],
            &[],
            vec![ok(r#"{"error":false,"feed":[]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a null error",
            &[],
            &[],
            vec![ok(r#"{"error":null,"feed":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case("a list", &[], &[], vec![ok("[1]")], owner_and_port, 1, 1),
        case(
            "a refusal with a permission card: the closing line",
            &[],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case("not JSON", &[], &[], vec![ok("x'\n")], owner_and_port, 1, 1),
        case(
            "not JSON, long",
            &[],
            &[],
            vec![reply("é".repeat(300))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer",
            &[],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin line when it worked",
            &[],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            &[],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(FEED),
            ],
            owner_and_port,
            0,
            2,
        ),
        traceback(case(
            "an event that is no object: Python raises part way",
            &[],
            &[],
            vec![ok(r#"{"feed":[{"at":5,"agent":"a"},7]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an at that is a string",
            &[],
            &[],
            vec![ok(r#"{"feed":[{"at":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a feed that is null",
            &[],
            &[],
            vec![ok(r#"{"feed":null}"#)],
            owner_and_port,
            1,
            1,
        )),
        // The words.
        case(
            "--agent with no name",
            &["--agent"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--limit with no number",
            &["--json", "--limit"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["-x", "--agent"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("a lone dash", &["-"], &[], vec![], owner_and_port, 2, 0),
        case(
            "a limit that is no number",
            &["ten"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an empty limit",
            &["--limit", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a long word as the limit",
            words(vec![leak("9".repeat(5000))]),
            &[],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        ),
        // The workspace.
        Case {
            cwd: Some(link_to_proj(&rig)),
            ..case(
                "PWD through a link",
                &[],
                &[],
                vec![ok(FEED)],
                owner_and_port,
                0,
                1,
            )
        },
        case(
            "PROJECT_PATH",
            &[],
            &[("K2_PROJECT_PATH", "/srv/acme ws")],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        ),
        // The gates.
        case("no port", &[], &[], vec![], owner_only, 1, 0),
        case("no token", &[], &[], vec![], port_only, 1, 0),
        case(
            "the gate comes before the words",
            &["--bogus"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
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
fn activity_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(rig.stub.port, &[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            h(&[]),
            case(
                "whoami's path is the workspace",
                &["--agent", "a"],
                &[],
                vec![ok(PROJ_WS), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a path of spaces is none",
                &[],
                &[],
                vec![ok(r#"{"path":"  \t "}"#), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a path with spaces round it",
                &[],
                &[],
                vec![ok(r#"{"path":"  /w x  "}"#), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a path that is no string",
                &[],
                &[],
                vec![ok(r#"{"path":5}"#), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "whoami not JSON",
                &[],
                &[],
                vec![ok("nope"), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "whoami a list",
                &[],
                &[],
                vec![ok("[1]"), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "whoami gives no answer",
                &[],
                &[],
                vec![Reply::Close, ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "whoami's error body has no path",
                &[],
                &[],
                vec![Reply::Answer(403, REFUSED.into()), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_PROJECT_PATH", "/set")]),
            case(
                "PROJECT_PATH: no whoami",
                &[],
                &[],
                vec![ok(FEED)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &[],
                &[],
                vec![ok(PROJ_WS), ok(FEED)],
                healed_on_disk,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_PROJECT_PATH", "/set")]),
            case(
                "a refused passport heals once",
                &[],
                &[],
                vec![heal(&rig), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_PROJECT_PATH", "/set")]),
            case(
                "a dead passport: the probe, then the restart line",
                &[],
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
                h(&[("K2_HOOK_SOCK", "{nosock}"), ("K2_PROJECT_PATH", "/set")]),
                case(
                    "HOOK_SOCK gone, the owner on disk (PRD 13.9 #4)",
                    &[],
                    &[],
                    vec![ok(FEED)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone, no owner on disk: whoami over TCP",
                &[],
                &[],
                vec![ok(PROJ_WS), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket for whoami, TCP for the feed",
                &[],
                &[],
                vec![ok(PROJ_WS), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "whoami not served there: TCP",
                &[],
                &[],
                vec![not_served(), ok(PROJ_WS), ok(FEED)],
                port_only,
                0,
                3,
            ),
        ),
        with(
            cell(&[]),
            case(
                "whoami only newlines there: TCP",
                &[],
                &[],
                vec![ok("\n\n"), ok(PROJ_WS), ok(FEED)],
                port_only,
                0,
                3,
            ),
        ),
        with(
            cell(&[]),
            case(
                "whoami no answer there: TCP",
                &[],
                &[],
                vec![Reply::Close, ok(PROJ_WS), ok(FEED)],
                port_only,
                0,
                3,
            ),
        ),
        with(
            cell(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: TCP",
                &[],
                &[],
                vec![ok(PROJ_WS), ok(FEED)],
                port_only,
                0,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn activity_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let v = vec![
        in_pipe(case(
            "an empty reply: the line, then SIGPIPE",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        in_pipe(case(
            "a 5xx with no body: the line, then SIGPIPE",
            &[],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            141,
            1,
        )),
        case(
            "a reset: the reader on nothing, exit 1",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &[],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            &[],
            &[],
            vec![ok(FEED)],
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
fn activity_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens",
        &[],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// No answer within 30 s: the line, then SIGPIPE. Slow.
#[test]
fn activity_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&in_pipe(case(
        "no answer in time (28)",
        &[],
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
fn activity_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
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
            "-h after a flag",
            &["--json", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        // The shell reads it as the agent `--help`.
        shell(case(
            "--agent --help",
            &["--agent", "--help"],
            &[],
            vec![ok(FEED)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "a port that is no port: curl's 3, the reader's 1",
            &[],
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
