//! `k5-cli root-helper` against the shell CLI's `cli/k2 root-helper`, on
//! the parity rig ([`common::parity`]): the CLI forms only (`status`,
//! `update`, `rollback`, `install-command`, each with `--json`), the
//! daemon's answers and errors, the stops, no answer. The daemon decides
//! every request; nothing here touches a helper.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "root-helper";

const STATUS: &str = r#"{"state":"installed","path":"/usr/local/libexec/k2-root-helper","version":"1.4.0","protocol":3,"daemonVersion":"0.46.17","legacy":{"mail":"none","pg":false},"rootSetup":"done","autoHeal":{"superseded":{"at":1763625600,"result":"failed","reason":"no sudo","why":"newer attempt"}},"cli":{"path":"/usr/local/bin/k2","version":"0.46.16","state":"stale","rootOwned":true,"daemonCliVersion":"0.46.17","lastResult":{"result":"refreshed","from":"0.46.15","to":"0.46.16"}},"plainSudoRestartUsed":true,"lastEvents":{"events":[{"at":"2026-11-20T10:00:00Z","event":{"verb":"heal","result":"ok","caller":{"pid":42}}}]}}"#;
const HEALED: &str = r#"{"heal":{"result":"installed","from":"1.3.0"},"target":"1.4.0","warnings":["old drop-in"],"dropin":{"result":"installed","unit":"k2-daemon.service","restartNeeded":true,"restart":"systemctl restart k2-daemon","why":"a new drop-in"},"status":{"state":"installed","path":"/p","version":"1.4.0","protocol":3,"daemonVersion":"0.46.17","legacy":{},"rootSetup":"copy-command","installCommand":"sudo sh /tmp/install.sh","autoHeal":{"lastResult":{"result":"ok"}},"cli":{"path":"/usr/local/bin/k2","state":"symlink"}}}"#;

#[test]
fn root_helper_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: root-helper
        case(
            "status (the default)",
            &[],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: root-helper status
        case(
            "status --events",
            &["status", "--events"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status --json",
            &["status", "--json"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status: not installed yet",
            &["status"],
            &[],
            vec![ok(
                r#"{"state":"absent","rootSetup":"copy-command","installCommand":"sudo k2-install","cli":{}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status: an error object",
            &["status"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"people only"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status: an error string",
            &["status"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"route not found"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status: not JSON",
            &["status"],
            &[],
            vec![Reply::Answer(502, " bad gateway \n".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status: a list",
            &["status"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status --json: an error",
            &["status", "--json"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"no"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: root-helper update
        case(
            "update",
            &["update", "--version", "1.4.0"],
            &[],
            vec![ok(HEALED)],
            owner_and_port,
            0,
            1,
        ),
        // covers: root-helper update --json
        case(
            "update --json --from a folder",
            &["update", "--from", "/tmp", "--match-installed", "--json"],
            &[],
            vec![ok(HEALED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "update: both",
            &["update", "--version", "1", "--match-installed"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "update: no such folder",
            &["update", "--from", "nope-dir"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "update: --version with no value",
            &["update", "--version"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "update: refused",
            &["update"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: root-helper rollback
        case(
            "rollback",
            &["rollback"],
            &[],
            vec![ok(
                r#"{"heal":{"result":"rolled_back"},"status":{"state":"installed","legacy":{"pg":true}}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: root-helper install-command
        case(
            "install-command",
            &["install-command"],
            &[],
            vec![ok(r#"{"installCommand":"sudo sh -c 'curl x | sh'\n"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "install-command --json",
            &["install-command", "--json"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "install-command: none",
            &["install-command"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            1,
            1,
        ),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
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
            "an unknown flag",
            &["status", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no answer",
            &["status"],
            &[("K2_PORT", dead_port().as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name.
#[test]
fn root_helper_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case(
        "brand",
        &["status", "--x"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("usage: unknown argument '--x' ({cli} root-helper --help)\n")
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
}

/// Help never acts: `update` and `rollback` with `-h` or `--help` print
/// the help and send nothing.
#[test]
fn root_helper_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell("update -h", &["update", "--version", "1.4.0", "-h"]),
        shell("rollback --help", &["rollback", "--help"]),
        // `--from` takes `-h` as its folder: no such folder, exit 2, nothing sent.
        Case {
            code: 2,
            ..shell("update --from -h", &["update", "--from", "-h"])
        },
    ] {
        rig.check(&c);
    }
}
