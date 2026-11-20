//! `k5-cli hostmail` with no verb, against the shell CLI's
//! `cli/k2 hostmail`, on the parity rig ([`common::parity`]): `hostmail`
//! alone is the family's help page, exit 0; `hostmail --json` has no verb
//! and is the usage error (`_mail_err usage …` on stderr), exit 2. Neither
//! asks the daemon; both stop at the connection gate first. The binary
//! answers both itself (the family's bare row), checked to be no
//! hand-over.

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "hostmail";

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
fn hostmail_with_no_verb_as_the_shell_does() {
    let rig = Rig::new(VERB, "bare");
    for c in [
        // covers: hostmail
        case(
            "hostmail alone: the help",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // covers: hostmail --json
        case(
            "hostmail --json: no verb",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "hostmail alone, no daemon: the connection gate",
            &[],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "hostmail --json, no daemon: the connection gate",
            &["--json"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}
