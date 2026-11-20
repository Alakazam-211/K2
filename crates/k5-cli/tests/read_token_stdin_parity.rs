//! `k5-cli read <workspace> --token-stdin` for a workspace on this server,
//! against the shell CLI's `cmd_read`, on the parity rig
//! ([`common::parity`]): the token is read from stdin (one line) and
//! checked, then left unused (`_apply_remote_host` with no host does
//! nothing); `--token` beside it and an empty one are exit 2 before
//! anything is sent; no workspace is the usage first. Each row is the
//! binary's own answer ([`answered_here`]).

#![cfg(unix)]

mod common;

use common::parity::*;

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
fn read_token_stdin_here_as_the_shell_does() {
    let rig = Rig::new("read", "token-stdin");
    for c in [
        // native: read --token-stdin
        Case {
            stdin: Some(b"k2rt_tok\nnext line\n"),
            ..case(
                "a token: read and unused",
                &["acme", "--token-stdin", "--lines", "5"],
                &[],
                vec![ok(LINES)],
                owner_and_port,
                0,
                1,
            )
        },
        Case {
            stdin: Some(b"no-newline"),
            ..case(
                "a token with no newline",
                &["acme", "--token-stdin"],
                &[],
                vec![ok(LINES)],
                owner_and_port,
                0,
                1,
            )
        },
        case(
            "nothing on stdin",
            &["acme", "--token-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            stdin: Some(b"\nk2rt_late\n"),
            ..case(
                "an empty first line",
                &["acme", "--token-stdin"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            )
        },
        Case {
            stdin: Some(b"k2rt_tok\n"),
            ..case(
                "--token beside it",
                &["acme", "--token", "t", "--token-stdin"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            )
        },
        Case {
            stdin: Some(b"k2rt_tok\n"),
            ..case(
                "no workspace: the usage first",
                &["", "--token-stdin"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            )
        },
        case(
            "the gate first",
            &["acme", "--token-stdin"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new("read", "brand");
    let c = case(
        "brand",
        &["acme", "--token-stdin"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("{cli} read: --token-stdin: no token on stdin\n")
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
}
