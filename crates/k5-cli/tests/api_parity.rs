//! `k5-cli api` against the shell CLI's `cli/k2 api`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `on` and `off`: the switch's answers, the daemon's error, not JSON,
//!   Python's raise; a heal; no answer (the line, then SIGPIPE).
//! - `status`: the two reads (`/boot-status` with the token as Bearer,
//!   then `/cli/settings/get`), each answer kind, either one failing.
//! - The words: `${1:-status}`, the usage lines (exit 2), the gate first.
//! - Credentials: the owner from disk, a passport with no session; in a
//!   session with the owner token on disk the shell sends the owner token
//!   (COMPAT-58) and the binary the passport (`owner-token-with-passport`).
//! - What the binary hands to the shell: help, a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "api";
const BOOT_ON: &str = r#"{"version":"0.46.17","api":{"enabled":true}}"#;
const SET_OFF: &str = r#"{"apiEnabled":false,"other":1}"#;

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

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// No answer on the left of the pipe: the line, then SIGPIPE (141);
/// macOS's bash 3.2 runs the reader on an empty input instead.
fn in_pipe(c: Case) -> Case {
    Case {
        compare: Compare::Bash32PipeReader {
            stdout: "Error: daemon returned an unparseable response: ''\n",
            stderr: "",
        },
        ..c
    }
}

#[test]
fn api_on_and_off_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "switch");
    let v = vec![
        case(
            "on",
            &["on"],
            &[],
            vec![ok(r#"{"apiEnabled":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "off",
            &["off", "x"],
            &[],
            vec![ok(r#"{"apiEnabled":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "on, the answer says off",
            &["on"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the owner-only refusal",
            &["on"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"owner or admin only"}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card",
            &["off"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin",
            &["on"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not JSON",
            &["on"],
            &[],
            vec![ok("<html>\u{e9}</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty 200",
            &["on"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a list with no error in it",
            &["on"],
            &[],
            vec![ok(r#"["x"]"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a list with an error in it",
            &["on"],
            &[],
            vec![ok(r#"["error"]"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a number",
            &["off"],
            &[],
            vec![ok("5")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a string that says error",
            &["off"],
            &[],
            vec![ok(r#""an error""#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a reset (56): the reader's line",
            &["on"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        in_pipe(case(
            "an empty reply (52): the line, SIGPIPE",
            &["on"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        // The words and the gates.
        case(
            "an unknown verb",
            &["enable"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown verb and no port: the gate",
            &["enable"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case("no token", &["on"], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &["on"],
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
fn api_status_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "status");
    let v = vec![
        case(
            "status: forced on by the env flag",
            &["status"],
            &[],
            vec![ok(BOOT_ON), ok(SET_OFF)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "no verb is status",
            &[],
            &[],
            vec![
                ok(r#"{"api":{"enabled":false}}"#),
                ok(r#"{"apiEnabled":true}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "an empty verb is status",
            &[""],
            &[],
            vec![ok("{}"), ok("{}")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "neither is JSON",
            &["status"],
            &[],
            vec![ok("nope"), ok("")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the settings refused",
            &["status"],
            &[],
            vec![ok(BOOT_ON), Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            0,
            2,
        ),
        case(
            "boot-status closes, the settings answer",
            &["status"],
            &[],
            vec![Reply::Close, ok(SET_OFF)],
            owner_and_port,
            0,
            2,
        ),
        traceback(case(
            "api is null",
            &["status"],
            &[],
            vec![ok(r#"{"api":null}"#), ok("{}")],
            owner_and_port,
            1,
            2,
        )),
        traceback(case(
            "the settings are a list",
            &["status"],
            &[],
            vec![ok(BOOT_ON), ok("[]")],
            owner_and_port,
            1,
            2,
        )),
        waited(case(
            "the settings never answer: the line, exit 1",
            &["status"],
            &[],
            vec![ok(BOOT_ON), Reply::Answer(503, String::new())],
            owner_and_port,
            1,
            2,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn api_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let v = vec![
        with(
            h(&[]),
            case(
                "no session: the passport",
                &["status"],
                &[],
                vec![ok(BOOT_ON), ok(SET_OFF)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["on"],
                &[],
                vec![heal(&rig), ok(r#"{"apiEnabled":true}"#)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a session with no owner token on disk: the passport",
                &["off"],
                &[],
                vec![ok(r#"{"apiEnabled":false}"#)],
                port_only,
                0,
                1,
            ),
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "a session with the owner token on disk",
                    &["status"],
                    &[],
                    vec![ok(BOOT_ON), ok(SET_OFF)],
                    owner_and_port,
                    0,
                    2,
                ),
            )
        },
        case(
            "a session with no passport: the owner token from disk (an open verb)",
            &["on"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![ok(r#"{"apiEnabled":true}"#)],
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
fn api_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    let v = vec![
        case(
            "status, nothing listens: both read as nothing",
            &["status"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            0,
            0,
        ),
        case(
            "on, nothing listens (7): the reader's line",
            &["on"],
            &[("K2_PORT", port.as_str())],
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

/// `/boot-status` gives up after 10 s, read as nothing. Slow.
#[test]
fn api_status_with_a_boot_status_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&case(
        "boot-status hangs (28 after 10 s)",
        &["status"],
        &[],
        vec![Reply::Hang(Duration::from_secs(14)), ok(SET_OFF)],
        owner_and_port,
        0,
        2,
    ));
}

#[test]
fn api_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        // Help never acts (K2's rule): -h or --help anywhere prints the
        // usage on stdout, exit 0, and sends nothing (cmd_api's
        // `_wants_help`). The front hands each line to the shell's help.
        shell(case(
            "--help: the usage, nothing sent",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "on -h: the usage, nothing sent",
            &["on", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "on --help: the usage, nothing sent",
            &["on", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "off -h: the usage, nothing sent",
            &["off", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "off --help: the usage, nothing sent",
            &["off", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "-h before on: the usage, nothing sent",
            &["-h", "on"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "status -h: the usage, nothing sent",
            &["status", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "a port that is no port",
            &["on"],
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
