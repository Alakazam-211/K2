//! `k5-cli hostmail` with no verb, against the shell CLI's
//! `cli/k2 hostmail`, on the parity rig ([`common::parity`]): `hostmail`
//! alone is the family's help page, exit 0; `hostmail --json` has no verb
//! and is the usage error (`_mail_err usage …` on stderr), exit 2. Neither
//! asks the daemon. Both are the family's own forms, which its route row
//! leaves to the shell (a row with verb rows is the shell's, so the
//! family keeps one help text).

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "hostmail";

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
    ] {
        rig.check(&c);
    }
}
