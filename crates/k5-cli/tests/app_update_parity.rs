//! `k5-cli app-update` against the shell CLI's `app-update` arm
//! (`cli/src/router.sh`), on the parity rig ([`common::parity`]): after
//! the gates, `fail_deprecated`'s lines, exit 1, whatever words follow;
//! nothing is sent. Help never acts: `-h` is the shell's (nothing sent).
//! Each native row is the binary's own answer ([`answered_here`]).

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
fn app_update_answers_as_the_shell_does() {
    let rig = Rig::new("app-update", "all");
    for c in [
        // native: app-update
        case("alone", &[], &[], vec![], owner_and_port, 1, 0),
        case(
            "with words",
            &["--check", "x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case("the gate: no port", &[], &[], vec![], owner_only, 1, 0),
        case("the gate: no token", &[], &[], vec![], port_only, 1, 0),
        Case {
            env: with_passport(&[]),
            ..case("a passport", &[], &[], vec![], port_only, 1, 0)
        },
    ] {
        answered_here(&rig, &c);
    }
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "-h is the shell's",
            &["-h"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )
    });
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new("app-update", "brand");
    let c = case("brand", &[], &[], vec![], owner_and_port, 1, 0);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!(
            "Error: 'app-update' is no longer supported.\n\nUse instead:\n  {cli} update --app\n\nRun '{cli} help-deprecated' for the full map of retired verbs.\n"
        )
    );
    assert_eq!(out.code, Some(1));
    assert!(log.is_empty());
}
