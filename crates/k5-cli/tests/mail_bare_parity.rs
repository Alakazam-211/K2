//! `k5-cli mail` with no verb, against the shell CLI's `cli/k2 mail`, on
//! the parity rig ([`common::parity`]): `mail` alone is the family's help
//! page (`cmd_help_mail`), exit 0; `mail --json` has no verb and is the
//! unknown-command line (`_mail_err usage …` on stderr), exit 2. Neither
//! asks the daemon; both stop at the connection gate, the virtual
//! desktop's guard and the `id` tool's checks first. The binary answers
//! both itself (the family's bare row), checked to be no hand-over.

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "mail";

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

fn session(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: port_only,
        ..c
    }
}

#[test]
fn mail_with_no_verb_as_the_shell_does() {
    let rig = Rig::new(VERB, "bare");
    for c in [
        // native: mail
        case(
            "mail alone: the help",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // native: mail --json
        case(
            "mail --json: no verb",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "mail alone, no daemon: the connection gate",
            &[],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "mail --json, no daemon: the connection gate",
            &["--json"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        session(case(
            "mail alone in a session",
            &[],
            &[],
            vec![],
            port_only,
            0,
            0,
        )),
        case(
            "mail alone: the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "mail alone: HOOK_SOCK set and no passport",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}

#[test]
fn the_unknown_command_line_says_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    let c = case("brand", &["--json"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stderr.contains(&format!(
            "unknown command '--json' \u{2014} see {cli} mail --help"
        )),
        "{}",
        out.stderr
    );
    assert_eq!(out.stdout, "");
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty(), "nothing sent: {log:?}");
}
