//! `k5-cli app people|discovery|stay-signed-in|profile settings|set`
//! against the shell CLI's (`cli/src/app-people.sh`), on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `people`: the grid (settings, people with and without agents, their
//!   folders, devices), nobody, a user, `--json`, a list above 256 KiB,
//!   over TCP even with a cell socket; the usage line, an unknown flag.
//! - `discovery`: on, off and unchanged, the usage lines, a refusal with
//!   the registry's fix line.
//! - `stay-signed-in`: days (leading zeros), no App word, over TCP even
//!   with a cell socket; the usage line.
//! - `profile settings`: the read, a write (`--names`, `--image`,
//!   `--picture`), `--json`; `profile` with no `settings`, no App, a bad
//!   value, a value missing at the end (exit 1), an unknown flag.
//! - `set`: the read, a write with each flag form (`=`, a `d` on days),
//!   `--json`; the page on stderr with no App, a second App, a bad mode,
//!   bad days, an unknown flag.
//! - Python's raise part way (a traceback, exit 1).
//! - Help never acts: each write verb with `-h` or `--help` anywhere.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";
const PEOPLE: &str = r#"{"app":"acme","settings":{"maxDevices":5,"staySignedInDays":30,"listed":true},"people":[{"username":"ann","fullName":"Ann é","enabled":true,"role":"editor","cells":[{"handle":"ws-a","caps":["read","write"],"folders":{"read":["docs","x"],"write":["out"]}},{"handle":"ws-b","caps":[]}]},{"username":"bo","enabled":true,"cells":[]},{"username":"cy","enabled":false}],"devices":[{"id":"d1","username":"ann","name":"phone","via":"pairing"},{"id":"d2","username":"bo","name":null}]}"#;
const SET: &str = r#"{"app":"acme","accountDeletion":{"mode":"request","graceDays":30,"set":true},"clientLogs":{"keepDays":7}}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn cell() -> Vec<(&'static str, String)> {
    with_passport(&[("K2_HOOK_SOCK", "{sock}")])
}

#[test]
fn people_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "ppl");
    let v = vec![
        case(
            "the grid",
            &["people", "acme"],
            &[],
            vec![ok(PEOPLE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "nobody, stay off",
            &["people", "acme"],
            &[],
            vec![ok(r#"{"app":"acme","settings":{"maxDevices":20}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "one user, the last word wins",
            &["people", "a b", "x", "ann é"],
            &[],
            vec![ok(PEOPLE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["people", "--json", "acme"],
            &[],
            vec![ok(PEOPLE)],
            owner_and_port,
            0,
            1,
        ),
        with(
            cell(),
            case(
                "a cell socket: still TCP",
                &["people", "acme"],
                &[],
                vec![ok(PEOPLE)],
                port_only,
                0,
                1,
            ),
        ),
        case("usage", &["people"], &[], vec![], owner_and_port, 2, 0),
        case(
            "an unknown flag",
            &["people", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "a folder that is not words",
            &["people", "acme"],
            &[],
            vec![ok(
                r#"{"app":"a","people":[{"username":"u","enabled":true,"cells":[{"handle":"h","folders":{"read":[7]}}]}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a refusal with its fix line",
            &["people", "acme"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"app_exists","hint":"x"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One answer above 256 KiB: thousands of people.
#[test]
fn people_a_large_list() {
    let rig = Rig::new(VERB, "pplbig");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(
                r#"{{"username":"user-{n}","fullName":"Person {n} é","enabled":true,"role":"r","cells":[{{"handle":"ws-{n}","caps":["read"]}}]}}"#
            )
        })
        .collect();
    let body = format!(
        r#"{{"app":"acme","settings":{{}},"people":[{}]}}"#,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&["people", "acme"][..], &["people", "acme", "--json"]] {
        rig.check(&case(
            "a large list",
            args,
            &[],
            vec![ok(&body)],
            owner_and_port,
            0,
            1,
        ));
    }
}

#[test]
fn discovery_and_stay_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "disc");
    let v = vec![
        case(
            "discovery on",
            &["discovery", "acme", "on"],
            &[],
            vec![ok(r#"{"settings":{"listed":true},"changed":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "discovery off, unchanged",
            &["discovery", "a b", "off", "extra"],
            &[],
            vec![ok(r#"{"settings":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "discovery: a bad mode",
            &["discovery", "acme", "yes"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "discovery: no App",
            &["discovery", "", "on"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "discovery: a 400",
            &["discovery", "acme", "on"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "stay-signed-in",
            &["stay-signed-in", "acme", "007"],
            &[],
            vec![ok(r#"{"settings":{"staySignedInDays":7}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stay-signed-in 0, no App word",
            &["stay-signed-in", "", "0"],
            &[],
            vec![ok(r#"{"settings":{"staySignedInDays":0}}"#)],
            owner_and_port,
            0,
            1,
        ),
        with(
            cell(),
            case(
                "stay-signed-in with a cell socket: still TCP",
                &["stay-signed-in", "acme", "9"],
                &[],
                vec![ok(r#"{"settings":{"staySignedInDays":9}}"#)],
                port_only,
                0,
                1,
            ),
        ),
        case(
            "stay-signed-in: days that are not a number",
            &["stay-signed-in", "acme", "1x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "stay-signed-in: not a dict",
            &["stay-signed-in", "acme", "1"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn profile_settings_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "prof");
    let read = r#"{"settings":{"app":"acme","editNames":true,"editImage":false}}"#;
    let v = vec![
        // covers: app profile settings
        case(
            "the read",
            &["profile", "settings", "acme"],
            &[],
            vec![ok(read)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a write",
            &[
                "profile", "settings", "acme", "--names", "off", "--image", "on",
            ],
            &[],
            vec![ok(read)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--picture",
            &["profile", "settings", "--picture", "off", "a b"],
            &[],
            vec![ok(read)],
            owner_and_port,
            0,
            1,
        ),
        // covers: app profile settings --json
        case(
            "--json",
            &["profile", "settings", "acme", "--json"],
            &[],
            vec![ok(read)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "profile with no settings",
            &["profile", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no App",
            &["profile", "settings"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad value",
            &["profile", "settings", "acme", "--names", "maybe"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--image last: set -e, exit 1",
            &["profile", "settings", "acme", "--image"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "an unknown flag",
            &["profile", "settings", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn set_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "aset");
    let v = vec![
        // covers: app set
        case(
            "the read",
            &["set", "acme"],
            &[],
            vec![ok(SET)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a write, every form",
            &[
                "set",
                "--account-deletion=immediate",
                "acme",
                "--deletion-grace",
                "030d",
                "--client-logs-days=7",
            ],
            &[],
            vec![ok(SET)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a write, one flag",
            &["set", "acme", "--client-logs-days", "14d"],
            &[],
            vec![ok(r#"{"app":"acme"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: app set --json
        case(
            "--json",
            &["set", "acme", "--json", "--account-deletion", "request"],
            &[],
            vec![ok(SET)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no App: the page",
            &["set"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a second App",
            &["set", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad mode",
            &["set", "acme", "--account-deletion", "never"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "bad days",
            &["set", "acme", "--deletion-grace", "1w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["set", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--deletion-grace last: set -e, exit 1",
            &["set", "acme", "--deletion-grace"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        traceback(case(
            "Python's raise",
            &["set", "acme"],
            &[],
            vec![ok(r#"{"app":"a","accountDeletion":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: a write verb with `-h` or `--help` anywhere on the
/// line prints help and sends nothing (the router's `app)` arm, or the
/// verb's own loop, answers it; the binary hands it to the shell).
#[test]
fn access_help_never_acts() {
    let rig = Rig::new(VERB, "acchelp");
    let rows: [&'static [&'static str]; 8] = [
        &["discovery", "acme", "on", "-h"],
        &["discovery", "-h", "acme", "on"],
        &["stay-signed-in", "acme", "5", "--help"],
        &["profile", "settings", "acme", "--names", "off", "-h"],
        &["profile", "--help", "settings", "acme", "--image", "on"],
        &["set", "acme", "--account-deletion", "immediate", "-h"],
        &["set", "--help", "acme", "--deletion-grace", "5"],
        &["people", "acme", "--help"],
    ];
    for args in rows {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
