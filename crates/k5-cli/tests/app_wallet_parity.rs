//! `k5-cli app wallet …` against the shell CLI's `cli/k2 app wallet …`,
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - The verbs: `settings` (a read; a write with `--on`, `--off`,
//!   `--fallback`), `people` (one App, every App, none, a large list),
//!   `remove`, `personal-agent` (set, `--clear`), `personal-agents`;
//!   `--json` prints the answer as it came.
//! - The words: each usage line (exit 2, nothing sent), a bad
//!   `--fallback`, an unknown flag, a fourth word, the usage page.
//! - The refusals (`_compute_fail`): a registry row (its `fix:` line, none
//!   with `--json`), no row, the route floor's shape, a 400 (exit 2), a
//!   404, a permission card's closing line, not JSON.
//! - Python's raise part way (a traceback, exit 1), not JSON on a 200.
//! - The credentials and channels: the owner, a passport over TCP, the
//!   cell socket (a GET and a POST), a socket nothing listens on, a
//!   session with no passport; the bulletin.
//! - No answer: no daemon, an empty reply, a reset, not HTTP; a POST on
//!   the cell socket that got no answer (sent once, the line, exit 1).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";
const SETTINGS: &str =
    r#"{"appId":"acme","settings":{"serviceId":"svc-1","enabled":true,"fallback":false}}"#;
const PEOPLE: &str = r#"{"items":[{"id":"w1","username":"ann","provider":"openai","type":"key","label":"Ann é","state":"ok","usedBy":["ws-a","ws-b"],"dormantSince":"2026-01-01"},{"id":"w2","username":"bo","provider":"p","type":"t","label":"","state":"s","usedBy":[]}]}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn cell(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    e
}

#[test]
fn wallet_settings_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "wset");
    let v = vec![
        // covers: app wallet
        case(
            "a read",
            &["wallet", "settings", "acme"],
            &[],
            vec![ok(SETTINGS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a read, no App id, an App that needs escaping",
            &["wallet", "settings", "a b/é&c"],
            &[],
            vec![ok(
                r#"{"settings":{"serviceId":["x"],"enabled":0,"fallback":"y"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--on and --fallback off: a write",
            &["wallet", "settings", "acme", "--on", "--fallback", "off"],
            &[],
            vec![ok(SETTINGS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a flag before the verb: usage",
            &[
                "wallet",
                "--json",
                "settings",
                "--off",
                "acme",
                "--fallback=on",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--off and --fallback=on",
            &["wallet", "settings", "--off", "acme", "--fallback=on"],
            &[],
            vec![ok(SETTINGS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: app wallet --json
        case(
            "--json",
            &["wallet", "settings", "acme", "--json"],
            &[],
            vec![ok("{\"a\": 1}\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--fallback alone: a write",
            &["wallet", "settings", "é", "--fallback=off"],
            &[],
            vec![ok(SETTINGS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no App: usage",
            &["wallet", "settings", "--on"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad --fallback",
            &["wallet", "settings", "acme", "--fallback", "maybe"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--fallback last",
            &["wallet", "settings", "acme", "--fallback"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["wallet", "settings", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a fourth word",
            &["wallet", "settings", "a", "b", "c", "d"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "Python's raise part way",
            &["wallet", "settings", "acme"],
            &[],
            vec![ok(r#"{"settings":{"serviceId":"s"}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "not JSON on a 200",
            &["wallet", "settings", "acme"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an empty 200",
            &["wallet", "settings", "acme"],
            &[],
            vec![ok("")],
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
fn wallet_refusals_are_the_shell_s() {
    let rig = Rig::new(VERB, "wref");
    let v = vec![
        case(
            "a registry row: next, explain and the fix line",
            &["wallet", "settings", "acme"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"app_exists","hint":"taken é"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a registry row with --json: no fix line",
            &["wallet", "people", "--json"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"app_exists","hint":"taken"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "no row",
            &["wallet", "people"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"nope_nope","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the route floor's shape",
            &["wallet", "personal-agents"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"forbidden","message":"agents can't"}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 400: exit 2",
            &["wallet", "remove", "w1"],
            &[],
            vec![Reply::Answer(400, r#"{"error":{"code":"usage"}}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a 404",
            &["wallet", "remove", "w9"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no such item"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 413 with no body",
            &["wallet", "remove", "w9"],
            &[],
            vec![Reply::Answer(413, String::new())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "not JSON",
            &["wallet", "people", "acme"],
            &[],
            vec![Reply::Answer(502, "Bad gateway\n".into())],
            owner_and_port,
            3,
            1,
        ),
        with(
            with_passport(&[]),
            case(
                "a permission card's closing line",
                &["wallet", "remove", "w1"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                3,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn wallet_people_and_the_rest() {
    let rig = Rig::new(VERB, "wpeople");
    let v = vec![
        case(
            "people of one App",
            &["wallet", "people", "acme"],
            &[],
            vec![ok(PEOPLE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "people of every App",
            &["wallet", "people"],
            &[],
            vec![ok(PEOPLE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no people",
            &["wallet", "people"],
            &[],
            vec![ok(r#"{"items":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "a usedBy that is not words",
            &["wallet", "people"],
            &[],
            vec![ok(r#"{"items":[{"usedBy":[1]}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "remove",
            &["wallet", "remove", "w1"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "rm --json",
            &["wallet", "rm", "w1", "--json"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "remove: usage",
            &["wallet", "remove"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "personal-agent",
            &["wallet", "personal-agent", "acme", "ann", "ws-a"],
            &[],
            vec![ok(r#"{"workspace":"ws-a","username":"ann","app":"acme"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "personal-agent --clear",
            &["wallet", "personal-agent", "--clear", "ws-a"],
            &[],
            vec![ok(r#"{"workspace":"ws-a","cleared":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "personal-agent: usage",
            &["wallet", "personal-agent", "acme", "ann"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "personal-agent --clear: usage",
            &["wallet", "personal-agent", "--clear"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "personal-agents",
            &["wallet", "personal-agents"],
            &[],
            vec![ok(
                r#"{"agents":[{"handle":"h1","username":"ann","app":"acme"},{"workspace":"/w","app":"b"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no personal agents",
            &["wallet", "personal-agents"],
            &[],
            vec![ok(r#"{"agents":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the usage page",
            &["wallet"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "help: the usage page",
            &["wallet", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "an unknown verb",
            &["wallet", "nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a flag for a verb",
            &["wallet", "--json"],
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

/// One answer above 256 KiB: thousands of rows.
#[test]
fn wallet_people_a_large_list() {
    let rig = Rig::new(VERB, "wlarge");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(
                r#"{{"id":"w{n}","username":"user-{n}","provider":"openai","type":"key","label":"label {n} é","state":"ok","usedBy":["ws-{n}","ws-x"]}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"items":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&["wallet", "people"][..], &["wallet", "people", "--json"]] {
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
fn wallet_credentials_and_channels() {
    let rig = Rig::new(VERB, "wcred");
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a passport over TCP",
                &["wallet", "people"],
                &[],
                vec![ok(PEOPLE)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a GET",
                &["wallet", "settings", "acme"],
                &[],
                vec![ok(SETTINGS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a POST",
                &["wallet", "remove", "w1"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket nothing listens on: TCP",
                &["wallet", "people"],
                &[],
                vec![ok(PEOPLE)],
                port_only,
                0,
                1,
            ),
        ),
        waited(with(
            cell(&[]),
            case(
                "a POST on the cell socket with no answer: sent once",
                &["wallet", "remove", "w1"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        with(
            cell(&[]),
            case(
                "a GET on the cell socket with no answer: TCP",
                &["wallet", "people"],
                &[],
                vec![Reply::Close, ok(PEOPLE)],
                port_only,
                0,
                2,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: app is an id tool",
            &["wallet", "people"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            with_passport(&[]),
            case(
                "the bulletin",
                &["wallet", "personal-agents"],
                &[],
                vec![ok(&format!(
                    r#"{{"agents":[],{}}}"#,
                    &BULLETIN[1..BULLETIN.len() - 1]
                ))],
                port_only,
                0,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn wallet_with_no_answer() {
    let rig = Rig::new(VERB, "wnone");
    let dead = dead_port();
    let v = vec![
        case(
            "no daemon",
            &["wallet", "people"],
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ),
        case(
            "an empty reply",
            &["wallet", "people"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a reset",
            &["wallet", "remove", "w1"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not HTTP",
            &["wallet", "people"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: a write verb with `-h` or `--help` anywhere on the
/// line prints help and sends nothing (the router's `app)` arm answers it
/// before `cmd_app`; the binary hands it to the shell).
#[test]
fn wallet_help_never_acts() {
    let rig = Rig::new(VERB, "whelp");
    let rows: [&'static [&'static str]; 6] = [
        &["wallet", "settings", "acme", "--on", "-h"],
        &["wallet", "settings", "acme", "--fallback", "off", "--help"],
        &["wallet", "remove", "w1", "--help"],
        &["wallet", "-h", "remove", "w1"],
        &["wallet", "personal-agent", "acme", "ann", "ws-a", "-h"],
        &["wallet", "personal-agent", "--clear", "--help", "ws-a"],
    ];
    for args in rows {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
