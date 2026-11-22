//! `k5 terminal spawn --sandbox` on a computer that can't sandbox: k5
//! hands `terminal` to the shell, so K2's 409 refusal reaches the person
//! unchanged, with the shell's exit code (1, `cli_print_or_fail`), the
//! same stdout and stderr, and one request. The plain spawn still exits 0
//! (0.46.17). What the shell prints is pinned by
//! `tests/cli/terminal_sandbox_refusal.sh`.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const REFUSAL: &str = r#"{"error":"this computer can't run a sandboxed session. Run it without --sandbox, knowing it runs as you.","code":"sandbox_unavailable"}"#;

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

#[test]
fn a_refused_sandbox_passes_through_unchanged() {
    let rig = Rig::new("terminal", "sandbox");
    rig.check(&shell(case(
        "--sandbox where this computer can't sandbox",
        &["spawn", "--command", "ls", "--sandbox"],
        &[],
        vec![Reply::Answer(409, REFUSAL.to_string())],
        owner_and_port,
        1,
        1,
    )));
    rig.check(&shell(case(
        "a plain spawn",
        &["spawn", "--command", "ls"],
        &[],
        vec![ok(
            r#"{"success":true,"terminalId":"t1","agentName":"agent-a","sandbox":"none"}"#,
        )],
        owner_and_port,
        0,
        1,
    )));
}
