//! `k5-cli remote` against the shell CLI's `cli/k2 remote`, on the parity
//! rig ([`common::parity`]): the usage, `help` and an unknown verb, with
//! and without a local daemon (`remote` skips the connection gate), and
//! the locked tool's checks. `logout` is the shell's.

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "remote";

#[test]
fn remote_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: remote
        case(
            "alone: the usage, exit 2",
            &[],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case(
            "an unknown verb",
            &["login", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no daemon: still the usage",
            &[],
            &[],
            vec![],
            nothing,
            2,
            0,
        ),
        case(
            "HOOK_SOCK set and no passport",
            &["help"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name.
#[test]
fn remote_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case("brand", &["nope"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stderr.starts_with(&format!(
            "{cli} remote: unknown verb 'nope'\n{cli} remote \u{2014}"
        )),
        "{}",
        out.stderr
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
}
