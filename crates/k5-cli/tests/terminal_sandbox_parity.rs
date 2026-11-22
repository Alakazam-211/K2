//! `k5 terminal spawn --sandbox` on a computer that can't sandbox: K2's
//! 409 refusal reaches the person as the shell's does, with the shell's
//! exit code (1, `cli_print_or_fail`), the same stdout and stderr, and one
//! request. The plain spawn and `--no-sandbox` still exit 0 (0.46.17).
//! `terminal` is native on the k5 line, so every row compares the
//! binary's own answer with the shell's, byte for byte. What the shell
//! prints is pinned by `tests/cli/terminal_sandbox_refusal.sh`.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const REFUSAL: &str = r#"{"error":"this computer can't run a sandboxed session. Run it without --sandbox, knowing it runs as you.","code":"sandbox_unavailable"}"#;

#[test]
fn a_refused_sandbox_passes_through_unchanged() {
    let rig = Rig::new("terminal", "sandbox");
    let v = vec![
        case(
            "--sandbox where this computer can't sandbox",
            &["spawn", "--command", "ls", "--sandbox"],
            &[],
            vec![Reply::Answer(409, REFUSAL.to_string())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--sandbox --wait with a title, refused",
            &[
                "spawn",
                "--title",
                "t",
                "--command",
                "ls",
                "--wait",
                "--sandbox",
            ],
            &[],
            vec![Reply::Answer(409, REFUSAL.to_string())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a plain spawn",
            &["spawn", "--command", "ls"],
            &[],
            vec![ok(
                r#"{"success":true,"terminalId":"t1","agentName":"agent-a","sandbox":"none"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--no-sandbox",
            &["spawn", "--command", "ls", "--no-sandbox"],
            &[],
            vec![ok(
                r#"{"success":true,"terminalId":"t1","agentName":"agent-a","sandbox":"none"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--sandbox where it can: microvm",
            &["spawn", "--command", "ls", "--sandbox"],
            &[],
            vec![ok(
                r#"{"success":true,"terminalId":"t1","agentName":"agent-a","sandbox":"microvm"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}
