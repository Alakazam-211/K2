//! `k5-cli workspace trust` alone, with an empty word and `workspace
//! trust help`, against the shell CLI's `cmd_workspace_trust` (its `""`
//! and `help` arm): `cmd_help_workspace_trust`'s page on stdout, exit 0,
//! nothing sent, after the connection gate. `-h` and `--help` stay the
//! front's. Each row is the binary's own answer ([`answered_here`]).

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
fn workspace_trust_page_as_the_shell_does() {
    let rig = Rig::new("workspace", "trust-page");
    for c in [
        // native: workspace trust
        case("alone", &["trust"], &[], vec![], owner_and_port, 0, 0),
        case("an empty word", &["trust", ""], &[], vec![], owner_and_port, 0, 0),
        // native: workspace trust help
        case("help", &["trust", "help", "x"], &[], vec![], owner_and_port, 0, 0),
        case("the gate: no port", &["trust"], &[], vec![], owner_only, 1, 0),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case("in a session", &["trust", "help"], &[], vec![], port_only, 0, 0)
        },
    ] {
        answered_here(&rig, &c);
    }
}
