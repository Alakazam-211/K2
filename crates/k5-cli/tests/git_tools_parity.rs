//! `k5-cli git-tools` against the shell CLI's `cli/k2 git-tools`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code for every verb of `cmd_git_tools`.
//!
//! - `status` (bare, `--json` first), `install`, `recheck`, each as text
//!   and `--json`; `help`; a word it doesn't take; an unknown verb.
//! - What came back: a 200, the daemon's own text, an error, `owner_only`
//!   with its hint (exit 3), a body that is not JSON, a refusal's card.
//! - No answer: an empty reply and nothing listening; the gates.
//! - Help with `-h` or `--help` on a verb that changes something: the
//!   shell's, and it sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "git-tools";

fn reply(body: &str) -> Reply {
    ok(body)
}

#[test]
fn git_tools_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: git-tools
        case(
            "status as text",
            &[],
            &[],
            vec![reply("git: /usr/bin/git (2.43)\n")],
            owner_and_port,
            0,
            1,
        ),
        // covers: git-tools --json
        case(
            "--json first is status --json",
            &["--json"],
            &[],
            vec![reply(r#"{"git":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: git-tools install
        case(
            "install",
            &["install"],
            &[],
            vec![reply("k2 git-tools: installing\n")],
            owner_and_port,
            0,
            1,
        ),
        // covers: git-tools install --json
        case(
            "install --json: an error is exit 1",
            &["install", "--json"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"boom"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: git-tools recheck
        case(
            "recheck: owner_only and its hint",
            &["recheck"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error" : "owner_only", "hint": "Ask the owner to run it."}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // covers: git-tools recheck --json
        case(
            "recheck --json",
            &["recheck", "--json"],
            &[],
            vec![reply(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the daemon's own text, on an error",
            &["install"],
            &[],
            vec![Reply::Answer(409, "k2 git-tools: already running\n".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error that is not JSON",
            &["status"],
            &[],
            vec![Reply::Answer(502, "bad gateway".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal's card",
            &["install"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case(
            "a word it doesn't take",
            &["install", "now"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown verb",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no port or token: the gate",
            &[],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "an empty reply",
            &["status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    rig.check(&Case {
        env: vec![("K2_PORT", dead_port())],
        ..case(
            "nothing listening",
            &["recheck"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        )
    });
}

/// `-h` or `--help`: the shell's (the binary hands the line over); its
/// `cmd_git_tools` prints the help page and sends nothing.
#[test]
fn git_tools_help_never_acts() {
    let rig = Rig::new(VERB, "help");
    for args in [&["install", "-h"][..], &["recheck", "--help"], &["--help"]] {
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        rig.check(&Case {
            compare: Compare::Shell,
            ..case(
                "help acts on nothing",
                args,
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        });
    }
}
