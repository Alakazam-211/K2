//! `k5-cli read <workspace>@<host>` against the shell CLI's `cmd_read`,
//! on the parity rig ([`common::parity`]): the host's token (from
//! `--token-stdin`, `--token` or `K2_REMOTE_TOKEN`; none is
//! `_apply_remote_host`'s two lines, exit 1), then the call to that host
//! over TCP with it, and the reader's lines. The other server here is the
//! side's own stub, named as `http://127.0.0.1:<port>`. Each native row
//! is the binary's own answer ([`answered_here`]); plain http to a name
//! that is looked up is the shell's.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const LINES: &str = r#"{"lines":["$ cargo test","test result: ok. 3 passed"]}"#;

/// `c` on the rig, then the binary alone once more: it answered the line
/// itself, so it never wrote the shell CLI out to hand it over.
fn answered_here(rig: &Rig, c: &Case) {
    rig.check(c);
    rig.binary(c, false);
    assert!(
        !k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
        "{}: the binary handed the line to the shell",
        c.name
    );
}

#[test]
fn read_on_another_server_as_the_shell_does() {
    let rig = Rig::new("read", "remote");
    let one =
        |name, args, body: Reply, code| case(name, args, &[], vec![body], owner_and_port, code, 1);
    for c in [
        // native: read <ws>@<host>
        Case {
            stdin: Some(b"remote-tok-1\n"),
            ..one(
                "the token from stdin",
                &[
                    "ws@http://127.0.0.1:{port}",
                    "--token-stdin",
                    "--lines",
                    "5",
                ],
                ok(LINES),
                0,
            )
        },
        Case {
            env: vec![("K2_REMOTE_TOKEN", "remote-tok-2".to_string())],
            ..one(
                "the token from the environment, a / after the URL",
                &["ws@http://127.0.0.1:{port}/", "--agent", "bob"],
                ok(LINES),
                0,
            )
        },
        one(
            "the token as --token",
            &[
                "ws@http://127.0.0.1:{port}",
                "--token",
                "remote-tok-3",
                "--session",
                "s 1",
            ],
            ok(LINES),
            0,
        ),
        one(
            "the other server's refusal",
            &["ws@http://127.0.0.1:{port}", "--token", "remote-tok-3"],
            Reply::Answer(
                403,
                r#"{"error":{"code":"not_connected","hint":"pair first"}}"#.into(),
            ),
            3,
        ),
        case(
            "no token for the host",
            &["ws@http://127.0.0.1:9"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        Case {
            stdin: Some(b"t\n"),
            ..case(
                "--token-stdin and --token",
                &[
                    "ws@http://127.0.0.1:{port}",
                    "--token-stdin",
                    "--token",
                    "x",
                ],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            )
        },
        case(
            "the connection gate first",
            &["ws@http://127.0.0.1:{port}", "--token", "x"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
    let shell = Case {
        compare: Compare::Shell,
        ..case(
            "plain http to a name: the shell's",
            &["ws@http://localhost:{port}", "--token", "remote-tok-4"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        )
    };
    rig.check(&shell);
}
