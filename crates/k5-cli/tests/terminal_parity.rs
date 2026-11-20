//! `k5-cli terminal spawn|write|read` against the shell CLI's
//! `cli/k2 terminal ...` on the parity rig ([`common::parity`]): the same
//! requests, stdout, stderr and exit code.
//!
//! - No verb or an unknown one; each verb's usage line.
//! - The `sessions-spawn` tool's session rule (no passport: exit 3).
//! - `spawn`: its words, every answer shape, a refusal (exit 3 and the
//!   closing line), no body, no answer.
//! - `write`: a session id, a workspace's name (its path, its sessions,
//!   a v2 one, the agent's), no live session, an agent name the shell's
//!   Python can't read, no answer to the lookup.
//! - `read`: the lines, an answer the Python can't read (asked again),
//!   `--lines`, no answer.
//! - Help on a line: the shell's (it takes `--help` as a word and acts,
//!   a known bash bug), so both sides send what the shell sends.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "terminal";
const UUID: &str = "0123abcd-0000-4000-8000-abcdef012345";

fn check(rig: &Rig, cases: Vec<Case>) {
    for c in cases {
        rig.check(&c);
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
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
fn usage_and_session() {
    let rig = Rig::new(VERB, "usage");
    check(
        &rig,
        vec![
            case("no verb", &[], &[], vec![], owner_and_port, 1, 0),
            case(
                "an unknown verb",
                &["list"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
            case(
                "spawn with no command",
                &["spawn", "--title", "t", "--x"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
            case(
                "write with no message",
                &["write", "ws"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
            case(
                "write with nothing",
                &["write"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
            case(
                "read with nothing",
                &["read"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
            case(
                "no passport in a session",
                &["read", "x"],
                &[("K2_HOOK_SOCK", "{nosock}"), ("K2_PORT", PORT)],
                vec![],
                owner_and_port,
                3,
                0,
            ),
            case(
                "no daemon: the gate",
                &["read", "x"],
                &[],
                vec![],
                nothing,
                1,
                0,
            ),
        ],
    );
}

#[test]
fn spawn() {
    let rig = Rig::new(VERB, "spawn");
    check(
        &rig,
        vec![
            // covers: terminal spawn
            case(
                "every word",
                &[
                    "spawn",
                    "--command",
                    "make test é",
                    "--title",
                    "Tests",
                    "--agent",
                    "qa",
                    "--cwd",
                    "/srv/x y",
                    "--wait",
                    "--sandbox",
                    "--extra",
                    "--no-sandbox",
                ],
                &[],
                vec![ok(r#"{"ok":true,"id":"t1"}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "the command as the title",
                &["spawn", "--command", "ls"],
                &[],
                vec![ok("[1]\n\n")],
                owner_and_port,
                0,
                1,
            ),
            case(
                "an answer that is not JSON",
                &["spawn", "--command", "ls"],
                &[],
                vec![ok("spawned")],
                owner_and_port,
                0,
                1,
            ),
            case(
                "an error",
                &["spawn", "--command", "ls"],
                &[],
                vec![Reply::Answer(
                    400,
                    r#"{"error":{"code":"bad","hint":"no such agent"}}"#.into(),
                )],
                owner_and_port,
                1,
                1,
            ),
            case(
                "a refusal",
                &["spawn", "--command", "ls"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                owner_and_port,
                3,
                1,
            ),
            case(
                "no body",
                &["spawn", "--command", "ls"],
                &[],
                vec![ok("\n")],
                owner_and_port,
                1,
                1,
            ),
            waited(case(
                "no answer",
                &["spawn", "--command", "ls"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            )),
        ],
    );
    let port = dead_port();
    rig.check(&case(
        "no daemon",
        &["spawn", "--command", "ls"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

#[test]
fn write() {
    let rig = Rig::new(VERB, "write");
    let resolved = ok(r#"{"path":"/srv/acme"}"#);
    let sessions = ok(&format!(
        r#"[{{"sessionId":"old","agentName":"qa"}},{{"sessionId":"{UUID}","isV2":true,"agentName":"lead"}},{{"sessionId":"s3","isV2":true,"agentName":"qa"}}]"#
    ));
    check(
        &rig,
        vec![
            case(
                "a session id",
                &["write", UUID, "hello", "there"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                1,
            ),
            // covers: terminal write
            case(
                "a workspace",
                &["write", "acme", "hi é"],
                &[],
                vec![resolved.clone(), sessions.clone(), ok("written\n")],
                owner_and_port,
                0,
                3,
            ),
            case(
                "a workspace and its agent",
                &["write", "acme", "--agent", "qa", "hi"],
                &[],
                vec![resolved.clone(), sessions.clone(), ok("")],
                owner_and_port,
                0,
                3,
            ),
            case(
                "an agent that is not there: the first v2",
                &["write", "acme", "--agent", "zed", "hi"],
                &[],
                vec![resolved.clone(), sessions.clone(), ok("{}")],
                owner_and_port,
                0,
                3,
            ),
            case(
                "--agent with nothing after it is the message",
                &["write", "acme", "--agent"],
                &[],
                vec![ok("{}"), ok(r#"[{"sessionId":"s1"}]"#), ok("{}")],
                owner_and_port,
                0,
                3,
            ),
            case(
                "no path: the name as it is",
                &["write", "acme", "hi"],
                &[],
                vec![ok("not json"), ok(r#"[{"sessionId":7}]"#), ok("{}")],
                owner_and_port,
                0,
                3,
            ),
            case(
                "no live session",
                &["write", "acme", "--agent", "qa", "hi"],
                &[],
                vec![resolved.clone(), ok("[]")],
                owner_and_port,
                1,
                2,
            ),
            case(
                "sessions that are not objects",
                &["write", "acme", "hi"],
                &[],
                vec![resolved.clone(), ok("[1]")],
                owner_and_port,
                1,
                2,
            ),
            case(
                "an agent name the shell's Python can't read",
                &["write", "acme", "--agent", "qa'", "hi"],
                &[],
                vec![resolved.clone(), sessions.clone()],
                owner_and_port,
                1,
                2,
            ),
            case(
                "an agent name with an escape",
                &["write", "acme", "--agent", "q\\x61", "hi"],
                &[],
                vec![resolved.clone(), sessions.clone(), ok("{}")],
                owner_and_port,
                0,
                3,
            ),
            case(
                "the last call fails",
                &["write", UUID, "hi"],
                &[],
                vec![Reply::Answer(404, r#"{"error":"no session"}"#.into())],
                owner_and_port,
                0,
                1,
            ),
            waited(case(
                "no answer to the write",
                &["write", UUID, "hi"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            )),
        ],
    );
    // macOS's bash 3.2 runs the lookup's reader on the empty input and
    // goes on to ask for the workspace's sessions (a second request), so
    // this row holds on Linux only, as read's does.
    if cfg!(target_os = "linux") {
        check(
            &rig,
            vec![case(
                "no answer to the lookup: nothing shown",
                &["write", "acme", "hi"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            )],
        );
    }
}

#[test]
fn read() {
    let rig = Rig::new(VERB, "read");
    let mut v = vec![
        // covers: terminal read
        case(
            "the lines",
            &["read", "t1"],
            &[],
            vec![ok(r#"{"lines":["one","two é",3,null]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--lines, a word skipped",
            &["read", "t1", "--x", "--lines", "5"],
            &[],
            vec![ok(r#"{"lines":"ab"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no lines",
            &["read", "t1"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no terminal"}"#.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an answer the Python can't read: asked again",
            &["read", "t1"],
            &[],
            vec![ok("raw text\n"), ok("raw text\n")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "a list: asked again",
            &["read", "t1"],
            &[],
            vec![ok("[1]"), ok("[2]")],
            owner_and_port,
            0,
            2,
        ),
    ];
    // bash 3.2 runs the reader on the empty input and asks again.
    if cfg!(target_os = "linux") {
        v.push(waited(case(
            "no answer: the line, then SIGPIPE",
            &["read", "t1"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )));
    }
    check(&rig, v);
}

/// The shell takes `--help` as a word and acts on it (a known bash bug,
/// to be fixed in bash); the front hands such a line to the shell, so
/// both sides send what the shell sends.
#[test]
fn help_on_a_line_is_the_shell_s() {
    let rig = Rig::new(VERB, "help");
    check(
        &rig,
        vec![
            shell(case(
                "write <id> --help sends a message",
                &["write", UUID, "--help"],
                &[],
                vec![ok("{}")],
                owner_and_port,
                0,
                1,
            )),
            shell(case(
                "spawn --command x -h spawns",
                &["spawn", "--command", "x", "-h"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                1,
            )),
            shell(case(
                "read <id> --help reads",
                &["read", "t1", "--help"],
                &[],
                vec![ok(r#"{"lines":[]}"#)],
                owner_and_port,
                0,
                1,
            )),
            shell(case(
                "--help",
                &["--help"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            )),
        ],
    );
}
