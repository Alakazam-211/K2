//! `k5-cli review` with no verb, or one it has no arm for, against the
//! shell CLI's `review` arm (`cli/src/router.sh`), on the parity rig
//! ([`common::parity`]): `Unknown review subcommand: <word>` on stderr,
//! then `cmd_help`'s daily tier on stdout, exit 1. With a passport the
//! tier asks the daemon its two questions as the shell does
//! ([`common::cards::daily_rows`]); without one nothing is sent. Each row
//! is the binary's own answer ([`answered_here`]).

#![cfg(unix)]

mod common;

use common::parity::*;

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
fn review_with_no_verb_as_the_shell_does() {
    for (args, tag) in [
        (&[][..], "bare"),
        (&["merge"][..], "merge"),
        (&[""][..], "empty"),
        (&["--json"][..], "json"),
    ] {
        let rig = Rig::new("review", tag);
        let args: &'static [&'static str] = args;
        let row = |name, setup, code| case(name, args, &[], vec![], setup, code, 0);
        for c in [
            // covers: review
            row("an owner's terminal: the line and the tier", owner_and_port, 1),
            Case {
                env: vec![("K2_HOOK_SOCK", "{sock}".to_string())],
                ..row("HOOK_SOCK set and no passport", owner_and_port, 1)
            },
            row("no token: the connection gate", port_only, 1),
            row("no port: the connection gate", owner_only, 1),
        ] {
            answered_here(&rig, &c);
        }
    }
}

#[test]
fn review_with_no_verb_and_a_passport() {
    let rig = Rig::new("review", "passport");
    for c in common::cards::daily_rows(&rig, &[], 1, 1) {
        // The rows the tier hands to the shell (named so) compare as the
        // shell's; every other row is the binary's own answer.
        if c.name.ends_with("the shell's") {
            rig.check(&c);
        } else {
            answered_here(&rig, &c);
        }
    }
}
