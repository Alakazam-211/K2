//! `k5-cli settings` against the shell CLI's `cli/k2 settings`, on the
//! parity rig ([`common::parity`]): the show, `--mode`, and `set <key>
//! <true|false>`; the daemon's errors and refusals; the stops; the locked
//! tool's checks in a session; no answer.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "settings";
const SHOWN: &str = r#"{"name":"acme-ws","mode":"manager","heartbeatEnabled":true,"pinned":false}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn settings_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: settings
        case("the show", &[], &[], vec![ok(SHOWN)], owner_and_port, 0, 1),
        case("the show, stray words", &["x", "y"], &[], vec![ok("{}")], owner_and_port, 0, 1),
        case(
            "--mode with no value: the show",
            &["--mode"],
            &[],
            vec![ok(SHOWN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--mode k2",
            &["--mode", "k2"],
            &[],
            vec![ok(r#"{"mode":"k2"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--mode: refused",
            &["--mode", "off"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"owner only"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the show: an error string",
            &[],
            &[],
            vec![ok(r#"{"error":"db busy"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the show: a token refusal",
            &[],
            &[],
            vec![Reply::Answer(401, REFUSED.into())],
            owner_and_port,
            3,
            1,
        ),
        case("the show: not JSON", &[], &[], vec![ok("<html>")], owner_and_port, 1, 1),
        case("the show: a list", &[], &[], vec![ok("[]")], owner_and_port, 1, 1),
        case(
            "an unknown flag",
            &["--frob"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an invalid mode",
            &["--mode", "auto"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: settings set
        case(
            "set",
            &["set", "computePreview", "On"],
            &[],
            vec![ok(r#"{"computePreview":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "set off",
            &["set", "homeChat", "false"],
            &[],
            vec![ok(r#"{"homeChat":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "set: the daemon kept another value",
            &["set", "ciPreview", "true"],
            &[],
            vec![ok(r#"{"ciPreview":"yes"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "set: a card",
            &["set", "ciPreview", "true"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "set: refused, no card",
            &["set", "ciPreview", "true"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"forbidden"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "set: a bad request",
            &["set", "ciPreview", "true"],
            &[],
            vec![Reply::Answer(400, "nope".into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "set: no answer",
            &["set", "ciPreview", "true"],
            &[("K2_PORT", dead_port().as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case("set: one word", &["set", "ciPreview"], &[], vec![], owner_and_port, 2, 0),
        case(
            "set: a key it doesn't change",
            &["set", "apiEnabled", "true"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set: not a switch",
            &["set", "ciPreview", "maybe"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "HOOK_SOCK set and no passport",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "set in a session: the cell socket",
                &["set", "boardFilePreview", "on"],
                &[],
                vec![ok(r#"{"boardFilePreview":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "the show in a session: TCP",
                &[],
                &[],
                vec![Reply::Answer(403, r#"{"error":{"code":"owner_only","hint":"agents can't"}}"#.into())],
                port_only,
                3,
                1,
            ),
        ),
        case("no port", &[], &[], vec![], owner_only, 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name.
#[test]
fn settings_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case("brand", &["set", "x", "on"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stderr.starts_with(&format!("Error: {cli} settings set doesn't change 'x'.")),
        "{}",
        out.stderr
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
}

/// Help never acts.
#[test]
fn settings_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell("--mode -h", &["--mode", "k2", "-h"]),
        shell("set --help", &["set", "ciPreview", "on", "--help"]),
        shell("set -h first", &["set", "-h"]),
    ] {
        rig.check(&c);
    }
}
