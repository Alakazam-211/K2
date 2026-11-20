//! `k5-cli workspace list [--running] [--json]` and `workspace profile`
//! against the shell CLI's `cli/k2 workspace list` and `profile`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - The table: workspaces (spaces, unicode, odd types), `--running`,
//!   none, words it skips, the daemon's error (its hint or code, on
//!   stderr), an object, not JSON (short, empty), a refusal with a
//!   permission card (exit 3), migrating, Python's raise.
//! - `--json`: the rows, `--running`, an error (text, object), `ok` false,
//!   not an object, not JSON, the bulletin, a permission card (exit 3),
//!   Python's raise.
//! - `profile`: the answer and a newline, a target (as `project=` and
//!   `project_path=`), an error on stderr (exit 1), a refusal (exit 3),
//!   an answer that is no success (empty, not JSON, not an object).
//! - The owner from disk, a passport over TCP, a live cell socket (tried
//!   first, its refusal teaching: exit 3), a healed passport, a session's
//!   passport with the owner on disk, a session with no passport (no
//!   credential at all), the gates.
//! - A request with no answer, or none taken: the line, exit 1 (an empty
//!   reply, a 5xx with no body, none in time, a reset, not HTTP, TMPDIR
//!   unusable, nothing listening).
//! - What the binary hands to the shell: help, a port that is no port, a
//!   session with no passport whose cell socket is live.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "workspace";
const ROWS: &str = r#"[
    {"name":"acme","agentMode":"agent","path":"/srv/acme ws","agentEnabled":true},
    {"name":"Globex é 日本","agentMode":null,"path":"/srv/globex","agentEnabled":false},
    {"name":"","path":null},
    {"name":true,"agentMode":1.5,"agentEnabled":1},
    {"name":"a-workspace-name-longer-than-24","agentMode":"managed-agent","path":"/x"}]"#;
const AGENTS: &str = r#"{"ok":true,"agents":[
    {"name":"acme","id":"w1","handle":"acme","path":"/srv/acme ws","mode":"agent","enabled":true,"live":false},
    {"name":"Globex é","id":"w2","enabled":false,"n":1.0e16}]}"#;

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

#[test]
fn workspace_list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let v = vec![
        // covers: workspace
        case(
            "the table",
            &["list"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--running",
            &["list", "--running"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--running, none enabled",
            &["list", "--running"],
            &[],
            vec![ok(r#"[{"name":"a"},{"agentEnabled":0}]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "none",
            &["list"],
            &[],
            vec![ok("[]\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "words it skips",
            &["list", "x", "--nope", "--running"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the daemon's error",
            &["list"],
            &[],
            vec![Reply::Answer(
                500,
                r#"{"error":{"code":"db","hint":"locked é"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an object with no error",
            &["list"],
            &[],
            vec![ok("{\"rows\":[1]}\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("<html>it's \"down\"</html>")],
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
            "a refusal with a permission card: exit 3, the closing line",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an error's code, a hint that is empty",
            &["list"],
            &[],
            vec![ok(r#"{"error":{"code":"db","hint":""}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object with neither: the answer",
            &["list"],
            &[],
            vec![ok(r#"{"error":{"x":1}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "migrating, then the answer",
            &["list"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(ROWS),
            ],
            owner_and_port,
            0,
            2,
        ),
        traceback(case(
            "a string of rows: Python raises",
            &["list", "--running"],
            &[],
            vec![ok("\"ab\"")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a number: Python raises",
            &["list"],
            &[],
            vec![ok("5")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a name that is a list: Python raises part way",
            &["list"],
            &[],
            vec![ok(r#"[{"name":"a"},{"name":[1]}]"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "the gates: no port",
            &["list"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the gates: no token",
            &["list"],
            &[],
            vec![],
            port_only,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn workspace_list_json_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "json");
    let v = vec![
        case(
            "the rows",
            &["list", "--json"],
            &[],
            vec![ok(AGENTS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--running",
            &["list", "--running", "--json"],
            &[],
            vec![ok(AGENTS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no agents",
            &["list", "--json", "--running"],
            &[],
            vec![ok(r#"{"ok":true,"agents":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an error in text",
            &["list", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"error":"down é"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object",
            &["list", "--json"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "ok false",
            &["list", "--json"],
            &[],
            vec![ok(r#"{"ok":false}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not an object",
            &["list", "--json"],
            &[],
            vec![ok("[1, 2]\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["list", "--json"],
            &[],
            vec![ok("nope é")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin",
            &["list", "--json"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal with a permission card: exit 3, the closing line",
            &["list", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "--running over a string: Python raises",
            &["list", "--json", "--running"],
            &[],
            vec![ok(r#"{"ok":1,"agents":"ab"}"#)],
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
fn workspace_profile_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "profile");
    let v = vec![
        case(
            "the answer as it came",
            &["profile"],
            &[],
            vec![ok("{\"display_name\":\"Acme é\"}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a target, and words after it",
            &["profile", "/srv/other ws&x=y", "z"],
            &[],
            vec![ok(r#"{"display_name":"other"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty target is the workspace",
            &["profile", ""],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an error: on stderr, exit 1",
            &["profile", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":"no such workspace"}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: exit 3, the closing line",
            &["profile"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an empty answer is no success",
            &["profile"],
            &[],
            vec![ok("\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an answer that is not JSON is no success",
            &["profile"],
            &[],
            vec![ok("<html>up é</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "JSON that is not an object is no success",
            &["profile"],
            &[],
            vec![ok("[\"Acme\"]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin",
            &["profile"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn workspace_reads_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let v = vec![
        with(
            h(&[]),
            case(
                "a passport over TCP",
                &["list"],
                &[],
                vec![ok(ROWS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a live cell socket: the socket first",
                &["list", "--json"],
                &[],
                vec![ok(AGENTS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a live cell socket: its refusal teaches, exit 3",
                &["list"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a live cell socket: profile names the target as project_path",
                &["profile", "/srv/other ws"],
                &[],
                vec![ok(r#"{"display_name":"other"}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a live cell socket: an empty reply goes on to TCP",
                &["list"],
                &[],
                vec![Reply::Close, ok(ROWS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &["profile"],
                &[],
                vec![ok("{}")],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["list"],
                &[],
                vec![heal(&rig), ok(ROWS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a dead passport: the probe, then the restart line",
                &["profile"],
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
                "a session, the owner on disk: the passport",
                &["list"],
                &[],
                vec![ok(ROWS)],
                owner_and_port,
                0,
                1,
            ),
        ),
        case(
            "a session with no passport, the owner on disk: no credential",
            &["list", "--json"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![ok(AGENTS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a session with no passport: profile sends no credential",
            &["profile"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn workspace_reads_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let v = vec![
        waited(case(
            "list: an empty reply, the line, exit 1",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "list --json: a 5xx with no body, the line, exit 1",
            &["list", "--json"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "list: a reset (56), the line, exit 1",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "list --json: not HTTP (1), the line, exit 1",
            &["list", "--json"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "list: TMPDIR unusable, a reset (76), the line, exit 1",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        case(
            "list: an empty 200 is the reader's",
            &["list"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        waited(case(
            "profile: an empty reply, the line, exit 1",
            &["profile"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "profile: a 5xx with no body, the line, exit 1",
            &["profile"],
            &[],
            vec![Reply::Answer(503, String::new())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "profile: a reset (56), the line, exit 1",
            &["profile"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "profile: not HTTP (1), the line, exit 1",
            &["profile"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "profile: TMPDIR unusable, a reset (76), the line, exit 1",
            &["profile"],
            &[],
            vec![Reply::Reset],
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
fn workspace_reads_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    for (name, args) in [
        ("list: nothing listens (7), the line, exit 1", &["list"][..]),
        ("list --json: nothing listens (7)", &["list", "--json"]),
        ("profile: nothing listens (7)", &["profile"]),
    ] {
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        rig.check(&case(
            name,
            args,
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ));
    }
}

/// No answer within 30 s: the line, exit 1. Slow.
#[test]
fn workspace_reads_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&waited(case(
        "list: no answer in time (28)",
        &["list"],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        1,
        1,
    )));
    rig.check(&waited(case(
        "profile: no answer in time (28)",
        &["profile"],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        1,
        1,
    )));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn workspace_reads_hand_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "list --help",
            &["list", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "profile x -h",
            &["profile", "x", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "workspace alone",
            &[],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "a verb it does not answer",
            &["triage", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
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
        shell(case(
            "a session with no passport whose cell socket is live",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
