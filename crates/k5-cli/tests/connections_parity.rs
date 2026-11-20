//! `k5-cli connections list` against the shell CLI's `cli/k2 connections
//! list`, on the parity rig ([`common::parity`]): the same requests,
//! stdout, stderr and exit code for every local path of its `list`.
//!
//! - The words: `list`, `--json`, `--users` (also first, for `list`), an
//!   unknown flag, an extra word.
//! - The answers: local and remote rows (bound, renamed, re-pair, gone,
//!   unreachable), who is working here (`thisWorkspace`: rows, you, long
//!   titles, its error), people with `--users` (names, roles, disabled),
//!   none, many rows, unicode; `--json` with each of them; the daemon's
//!   error (an object on stderr, text on stdout), not JSON (short, long,
//!   empty), not an object, `--users` with no list, a refusal with a
//!   permission card (the closing line), the bulletin, migrating, Python's
//!   raise part way.
//! - The owner from disk, a passport over TCP, a live cell socket (its
//!   trailing newlines dropped; not served, empty, only newlines, no
//!   answer and a reset go on to TCP; a refusal there is the answer), a
//!   dead socket file; the id tool's checks; a re-minted, a healed and a
//!   dead passport; PROJECT_PATH; the gates.
//! - No answer (an empty reply, a 5xx with no body, none in time): the
//!   line and exit 1; a reset (56), not HTTP (1), TMPDIR unusable (76) and
//!   no daemon (7): curl's code, nothing printed.
//! - The rest of `cmd_connections` (the whole row): `connections` alone
//!   and an empty action (`list`), an action it has no arm for (`help`, a
//!   word, a flag after one: the usage lines, exit 1), with the gates and
//!   the id tool's checks in front of them; one answer above 256 KiB.
//! - What the binary hands to the shell: help with `-h` or `--help` in a
//!   place that is not a help page, a port that is no port (`add` and
//!   `remove`: connections_write_parity.rs).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "connections";
const LIST: &str = r#"{"workspace":{"name":"acme","path":"/srv/acme","id":"p1"},
  "thisWorkspace":{"handle":"acme","sessions":[
    {"address":"acme","role":"main","cli":"claude","state":"working","title":"Fix it\nnow é","you":true},
    {"handle":"acme/h","title":"LONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONG 日本"}]},
  "connections":[
    {"projectName":"beta","projectId":"p2"},
    {"projectName":"gone","reachable":false},
    {"remote":true,"agent":"seoca","host":"quahog.example","remoteWorkspaceId":"r1","handle":"quahog-site","displayName":"Quahog Site","repairRecommended":true},
    {"remote":true,"address":"x::y.example","remoteWorkspaceId":"r2","handle":"h","displayName":" H ","remoteWorkspaceGone":true},
    {"remote":true}],
  "users":[
    {"username":"ann","role":"owner","firstName":"Ann","displayName":"Ann Lée"},
    {"username":"bob","role":"member","disabled":true,"displayName":"Bob"},
    "skip",
    {}]}"#;

fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"projectName":"ws-{i}-{}","projectId":"p{i}"}}"#,
                "é".repeat(i % 40)
            )
        })
        .collect();
    format!(
        r#"{{"workspace":{{"name":"acme"}},"connections":[{}]}}"#,
        rows.join(",")
    )
}

fn reply(body: String) -> Reply {
    Reply::Answer(200, body)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn connections_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let traceback = |c: Case| Case {
        compare: Compare::Traceback,
        ..c
    };
    let v = vec![
        case("list", &["list"], &[], vec![ok(LIST)], owner_and_port, 0, 1),
        case(
            "--users",
            &["list", "--users"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--users first",
            &["--users"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: connections list --json
        case(
            "--json",
            &["list", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json first",
            &["--json", "--users"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: connections --json
        case(
            "--json, local rows' odd values",
            &["--json"],
            &[],
            vec![ok(
                r#"{"workspace":{"projectId":"p9"},"connections":[{"projectName":"","name":"n","id":"i","reachable":0},{"remote":1,"agent":[1],"host":2.5}],"thisWorkspace":null}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: connections list
        case(
            "none",
            &["list"],
            &[],
            vec![ok(r#"{"workspace":{"path":"/p"},"connections":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty object",
            &["list"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no people",
            &["list", "--users"],
            &[],
            vec![ok(r#"{"users":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--users and no list of them",
            &["list", "--users"],
            &[],
            vec![ok(r#"{"connections":[{"projectName":"b"}]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--json --users and no list of them",
            &["list", "--json", "--users"],
            &[],
            vec![ok(r#"{"users":{}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "this workspace's error",
            &["list"],
            &[],
            vec![ok(
                r#"{"thisWorkspace":{"error":"nope é"},"connections":[{"projectName":null}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many rows",
            &["list"],
            &[],
            vec![reply(many(400))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a large list: an answer above 256 KiB",
            &["list", "--json"],
            &[],
            vec![reply(many(4000))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a large list as text",
            &[],
            &[],
            vec![reply(many(4000))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the daemon's error object",
            &["list"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"nope é"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the daemon's error text",
            &["list", "--json"],
            &[],
            vec![ok(r#"{"error":"not allowed"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: the closing line",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long",
            &["list"],
            &[],
            vec![reply("é".repeat(300))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer",
            &["list"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin line when it worked",
            &["list"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            &["list"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(LIST),
            ],
            owner_and_port,
            0,
            2,
        ),
        traceback(case(
            "not an object",
            &["list"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a row that is no object: Python raises part way",
            &["list"],
            &[],
            vec![ok(r#"{"connections":[{"projectName":"a"},5]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a display name that is a number",
            &["list"],
            &[],
            vec![ok(
                r#"{"connections":[{"remote":true,"remoteWorkspaceId":1,"handle":"h","displayName":7}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        // The words.
        case(
            "an unknown flag",
            &["list", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an extra word",
            &["list", "--json", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--json then list is an extra word",
            &["--json", "list"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "PROJECT_PATH",
            &["list"],
            &[("K2_PROJECT_PATH", "/srv/acme ws")],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // The gates.
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
        case("no token", &["list"], &[], vec![], port_only, 1, 0),
        case(
            "the gate comes before the words",
            &["list", "x"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &["list"],
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

#[test]
fn connections_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            h(&[]),
            case(
                "a host session: the passport over TCP",
                &["list"],
                &[],
                vec![ok(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &["list"],
                &[],
                vec![ok(LIST)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["list", "--json"],
                &[],
                vec![heal(&rig), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a dead passport: the probe, then the restart line",
                &["list"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                1,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone, the owner on disk: the passport (an id tool)",
                &["list"],
                &[],
                vec![ok(r#"{"role":"canonical"}"#), ok(LIST)],
                owner_and_port,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["list"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            cell(&[]),
            case(
                "the cell socket",
                &["list", "--users"],
                &[],
                vec![ok(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "its trailing newlines go",
                &["list"],
                &[],
                vec![ok("notjson\n\n")],
                port_only,
                1,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "not served: TCP",
                &["list"],
                &[],
                vec![not_served(), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "an empty answer: TCP",
                &["list"],
                &[],
                vec![ok(""), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "only newlines: TCP",
                &["list"],
                &[],
                vec![ok("\n"), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "no answer there: TCP",
                &["list"],
                &[],
                vec![Reply::Close, ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "a reset there: TCP",
                &["list"],
                &[],
                vec![Reply::Reset, ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "a refusal there is the answer",
                &["list"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                1,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "a refused passport heals there",
                &["list"],
                &[],
                vec![heal(&rig), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: TCP",
                &["list"],
                &[],
                vec![ok(LIST)],
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
fn connections_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let waited = |c: Case| Case {
        compare: Compare::Waited,
        ..c
    };
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        waited(case(
            "an empty reply: the line, exit 1",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "a 5xx with no body: the line, exit 1",
            &["list"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a reset: curl's 56",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "not HTTP: curl's 1",
            &["list"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset: 76",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            76,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            &["list"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        )),
        waited(with(
            cell(),
            case(
                "the cell socket and TCP: an empty reply on both",
                &["list"],
                &[],
                vec![Reply::Close, Reply::Close],
                port_only,
                1,
                2,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn connections_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens: curl's 7",
        &["list"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        7,
        0,
    ));
}

/// No answer within 30 s: the line, exit 1. Slow.
#[test]
fn connections_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&Case {
        compare: Compare::Waited,
        ..case(
            "no answer in time (28)",
            &["list"],
            &[],
            vec![Reply::Hang(Duration::from_secs(35))],
            owner_and_port,
            1,
            1,
        )
    });
}

/// The rest of the row: `connections` alone, an empty action, and the
/// actions `cmd_connections` has no arm for.
#[test]
fn connections_the_rest_of_the_row() {
    let rig = Rig::new(VERB, "row");
    let v = vec![
        // covers: connections
        case(
            "connections alone",
            &[],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty action is list",
            &["", "--users"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty action, then a word",
            &["", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "help is no action: the usage lines",
            &["help"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "an unknown action and words after it",
            &["show", "acme", "--json"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "a flag first that is not list's",
            &["--all"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "an unknown action with no port",
            &["show"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "an unknown action with no token",
            &["show"],
            &[],
            vec![],
            port_only,
            1,
            0,
        ),
        case(
            "an unknown action, HOOK_SOCK set and no passport",
            &["show"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            with_passport(&[]),
            case(
                "connections alone with a passport",
                &[],
                &[],
                vec![ok(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        Case {
            compare: Compare::Waited,
            ..case(
                "connections alone, an empty reply: the line",
                &[],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn connections_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "--help",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "list -h",
            &["list", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "list --json --help",
            &["list", "--json", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "a port that is no port",
            &["list"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            2,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
