//! `k5-cli help remote` against the shell CLI's `cmd_help remote`
//! (`cli/src/wiki.sh`): `_remote_usage` on stdout, exit 0, nothing sent;
//! the connection gate wants a port and a token, as for any help. The
//! binary answers it itself ([`answered_here`]); `--help remote` and
//! `-h remote` stay the shell's.

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
fn help_remote_as_the_shell_does() {
    let rig = Rig::new("help", "remote");
    for c in [
        // native: help remote
        case("the usage", &["remote"], &[], vec![], owner_and_port, 0, 0),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case("in a session", &["remote"], &[], vec![], port_only, 0, 0)
        },
        Case {
            env: vec![("K2_HOOK_SOCK", "{sock}".to_string())],
            ..case(
                "HOOK_SOCK and no passport",
                &["remote"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
        case(
            "no port: the gate",
            &["remote"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the virtual desktop's switch, no token: the connection gate",
            &["remote"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            port_only,
            1,
            0,
        ),
        case(
            "no token: the gate",
            &["remote"],
            &[],
            vec![],
            port_only,
            1,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}

/// The page names the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` it says `xy`).
#[test]
fn the_page_says_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new("help", "brand");
    let c = case("brand", &["remote"], &[], vec![], owner_and_port, 0, 0);
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stdout
            .starts_with(&format!("{cli} remote \u{2014} this computer's sign-in")),
        "{:?}",
        out.stdout
    );
    assert!(out.stdout.contains(&format!("  {cli} remote logout --host HOST")));
    assert_eq!(out.code, Some(0));
    assert!(log.is_empty());
}
