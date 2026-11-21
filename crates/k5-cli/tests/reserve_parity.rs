//! `k5-cli reserve` and `k5-cli release` against the shell CLI's
//! `cmd_reserve` and `cmd_release` (`cli/src/usage.sh`), on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - The words: paths and `--agent`, the agent from `AGENT_NAME` (escaped
//!   as urlencode escapes it, odd bytes and all), each
//!   usage line (`--agent` with no name, a word that is no path, no agent,
//!   `reserve` with no path), `release` with no path.
//! - The credential: the owner from disk, a passport over TCP; the
//!   `connections` tool's refusal in a session with no passport.
//! - The answers: printed as they came with exit 0 whatever the status, a
//!   refusal's Ask-with line; a reset (56).
//! - Help never acts: `-h` and `--help` are the shell's (no request).
//!
//! Each native row is the binary's own answer, not a hand-over that prints
//! the same ([`answered_here`]).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const OK: &str = r#"{"ok":true,"reserved":["a"]}"#;
const AGENT: (&str, &str) = ("K2_AGENT_NAME", "tester");

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

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

/// The rows both verbs share.
fn shared() -> Vec<Case> {
    let ask = Reply::Answer(403, PERMISSION_ASK.into());
    vec![
        case(
            "paths and --agent",
            &["src/a.rs", "docs/b c.md", "--agent", "other"],
            &[],
            vec![ok(OK)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the agent from the setting",
            &["src/a.rs"],
            &[AGENT],
            vec![ok(OK)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the K5 name first",
            &["x"],
            &[("K5_AGENT_NAME", "k5name"), ("K2_AGENT_NAME", "k2name")],
            vec![ok(OK)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd bytes in a path",
            &["é & = /x \"q\" 'y' %41,#z", "--agent", "a.b-c_d~e"],
            &[],
            vec![ok(OK)],
            owner_and_port,
            0,
            1,
        ),
        // The agent goes through urlencode: '&', '=', '#', '%', '+', a
        // space and UTF-8 reach the daemon as one value.
        case(
            "odd bytes in the agent",
            &["p", "--agent", "a b&c=d#e%41+é"],
            &[],
            vec![ok(OK)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd bytes in the agent from the setting",
            &["p"],
            &[("K2_AGENT_NAME", "x&paths=y#z %2F")],
            vec![ok(OK)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty path is a path",
            &["", "--agent", "a"],
            &[],
            vec![ok(OK)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--agent with no name",
            &["p", "--agent"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--agent followed by a flag",
            &["p", "--agent", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a word that is no path",
            &["p", "--json", "--agent", "a"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("no agent", &["p"], &[], vec![], owner_and_port, 1, 0),
        case(
            "an error status: its body, exit 0",
            &["p"],
            &[AGENT],
            vec![Reply::Answer(500, r#"{"error":"boom"}"#.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal: its body and the Ask-with line",
            &["p"],
            &[AGENT],
            vec![ask],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a reset",
            &["p"],
            &[AGENT],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "the gate: no port",
            &["p"],
            &[AGENT],
            vec![],
            owner_only,
            1,
            0,
        ),
        Case {
            env: with_passport(&[("K2_AGENT_NAME", "tester")]),
            ..case(
                "a passport over TCP",
                &["p"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            )
        },
        case(
            "a session with no passport: the tool's refusal",
            &["p"],
            &[AGENT, ("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ]
}

#[test]
fn reserve_answers_as_the_shell_does() {
    let rig = Rig::new("reserve", "all");
    let mut v = shared();
    // native: reserve
    v.push(case(
        "reserve with no path",
        &["--agent", "a"],
        &[],
        vec![],
        owner_and_port,
        1,
        0,
    ));
    for c in v {
        answered_here(&rig, &c);
    }
}

#[test]
fn release_answers_as_the_shell_does() {
    let rig = Rig::new("release", "all");
    let mut v = shared();
    // native: release
    v.push(case(
        "release with no path",
        &["--agent", "a"],
        &[],
        vec![ok(OK)],
        owner_and_port,
        0,
        1,
    ));
    for c in v {
        answered_here(&rig, &c);
    }
}

/// Help never acts: the shell's lines, nothing sent.
#[test]
fn help_is_the_shell_s_and_sends_nothing() {
    for verb in ["reserve", "release"] {
        let rig = Rig::new(verb, "help");
        for c in [
            shell(case("-h", &["-h"], &[AGENT], vec![], owner_and_port, 2, 0)),
            shell(case(
                "--help after a path",
                &["p", "--help"],
                &[AGENT],
                vec![],
                owner_and_port,
                2,
                0,
            )),
        ] {
            rig.check(&c);
        }
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    for verb in ["reserve", "release"] {
        let rig = Rig::new(verb, "brand");
        for (args, want) in [
            (
                &["p", "--agent"][..],
                format!("{cli} {verb}: '--agent' needs a name.\n"),
            ),
            (
                &["--x"],
                format!(
                    "{cli} {verb}: '--x' is not a path. {cli} {verb} takes paths and --agent <name> only, and is being retired.\n"
                ),
            ),
        ] {
            let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
            let (out, log) = rig.binary(&c, false);
            assert_eq!(out.stderr, want, "{verb} {args:?}");
            assert_eq!(out.code, Some(2), "{verb} {args:?}");
            assert!(log.is_empty(), "{verb} {args:?}");
        }
    }
}
