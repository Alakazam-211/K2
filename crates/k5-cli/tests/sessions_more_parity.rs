//! `k5-cli sessions spawn` and `sessions set-label` against the shell
//! CLI's `cmd_sessions_spawn` and `cmd_sessions_set_label`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - `spawn`: its line, exit 2, with or without a daemon (the router skips
//!   its connection gate); the virtual desktop's guard and the tool's
//!   refusal in a session with no passport still stop it.
//! - `set-label`: the request (the label's words joined, `--no-lock`), the
//!   reader's line, its error lines (the daemon's error, not JSON, a list),
//!   the usage lines, the gate, the tool's refusal, no answer (the line,
//!   then SIGPIPE).
//!
//! Each native row is the binary's own answer ([`answered_here`]).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

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
fn sessions_spawn_as_the_shell_does() {
    let rig = Rig::new("sessions", "spawn");
    for c in [
        // native: sessions spawn
        case("alone", &["spawn"], &[], vec![], owner_and_port, 2, 0),
        case("with words", &["spawn", "x", "--cli", "claude"], &[], vec![], owner_and_port, 2, 0),
        case("no port: no gate", &["spawn"], &[], vec![], owner_only, 2, 0),
        case("nothing at all: no gate", &["spawn"], &[], vec![], nothing, 2, 0),
        case(
            "the virtual desktop's guard",
            &["spawn"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            nothing,
            3,
            0,
        ),
        case(
            "a session with no passport: the tool's refusal",
            &["spawn"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case("in a session", &["spawn"], &[], vec![], port_only, 2, 0)
        },
    ] {
        answered_here(&rig, &c);
    }
}

#[test]
fn sessions_set_label_as_the_shell_does() {
    let rig = Rig::new("sessions", "label");
    let one = |name, args, body: &str| case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1);
    let v = vec![
        // native: sessions set-label
        one(
            "a label",
            &["set-label", "sess-1", "my", "label & more"],
            r#"{"label":"my label & more","locked":true}"#,
        ),
        one(
            "--no-lock anywhere",
            &["set-label", "sess 1", "--no-lock", "é"],
            r#"{"label":"é","locked":false}"#,
        ),
        one("an empty answer object", &["set-label", "s", "x"], "{}"),
        case(
            "the daemon's error",
            &["set-label", "s", "x"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no such session"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["set-label", "s", "x"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a list",
            &["set-label", "s", "x"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object",
            &["set-label", "s", "x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case("no id", &["set-label"], &[], vec![], owner_and_port, 1, 0),
        case("an empty id", &["set-label", "", "x"], &[], vec![], owner_and_port, 1, 0),
        bash32_empty_array(
            case("no label", &["set-label", "s"], &[], vec![], owner_and_port, 1, 0),
            &["label_parts"],
            Some((1, 0)),
        ),
        bash32_empty_array(
            case(
                "only --no-lock",
                &["set-label", "s", "--no-lock"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
            &["label_parts"],
            Some((1, 0)),
        ),
        case("the gate: no port", &["set-label", "s", "x"], &[], vec![], owner_only, 1, 0),
        case(
            "a session with no passport: the tool's refusal",
            &["set-label", "s", "x"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "a passport over TCP",
                &["set-label", "s", "x"],
                &[],
                vec![ok(r#"{"label":"x","locked":true}"#)],
                port_only,
                0,
                1,
            )
        },
        Case {
            compare: Compare::Bash32PipeReader {
                stdout: "",
                stderr: "Error: Expecting value: line 1 column 1 (char 0)\n",
            },
            ..case(
                "no answer: the line, then SIGPIPE",
                &["set-label", "s", "x"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            )
        },
    ];
    for c in v {
        answered_here(&rig, &c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new("sessions", "brand");
    for (args, want, code) in [
        (
            &["spawn"][..],
            format!("{cli} sessions spawn moved to: {cli} sessions new <name> [--cli <cli>] (see {cli} sessions --help)\n"),
            2,
        ),
        (
            &["set-label"],
            format!("Usage: {cli} sessions set-label <session-id> <label> [--no-lock]\n"),
            1,
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, code, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(code), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
