//! `k5-cli workspace list [--running] [--json]` and `workspace profile`
//! against the shell CLI's `cli/k2 workspace list` and `profile`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - The table: workspaces (spaces, unicode, odd types), `--running`,
//!   none, words it skips, the daemon's error, an object, not JSON (short,
//!   empty), a refusal with a permission card, migrating, Python's raise.
//! - `--json`: the rows, `--running`, an error (text, object), `ok` false,
//!   not an object, not JSON, the bulletin, Python's raise.
//! - `profile`: the answer as it came (any status), a target, a refusal.
//! - The owner from disk, a passport over TCP, a live cell socket (still
//!   TCP: the shell calls `cli_request` directly), a healed passport, PRD
//!   13.9 #4's owner token with a passport, the gates.
//! - No answer: on the left of the pipe the line, then death by SIGPIPE;
//!   `profile` the line and exit 1. A reset, not HTTP, TMPDIR unusable
//!   and no daemon: the reader on nothing (`list`), curl's code
//!   (`profile`).
//! - What the binary hands to the shell: help, a port that is no port.

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
const UNPARSEABLE: &str = "Error: daemon returned an unparseable response: ''\n";
const BAD_JSON: &str = "{\"error\": {\"code\": \"bad_response\", \"hint\": \"daemon returned an unparseable response: ''\"}}\n";

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

/// No answer on the left of the pipe: the line, then SIGPIPE (141);
/// macOS's bash 3.2 runs the reader on an empty input instead.
fn in_pipe(c: Case, json: bool) -> Case {
    Case {
        compare: if json {
            Compare::Bash32PipeReader {
                stdout: "",
                stderr: BAD_JSON,
            }
        } else {
            Compare::Bash32PipeReader {
                stdout: UNPARSEABLE,
                stderr: "",
            }
        },
        ..c
    }
}

#[test]
fn workspace_list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let v = vec![
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
            "a refusal with a permission card: the closing line",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
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
            "a refusal with a permission card: the closing line",
            &["list", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
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
            "an error, printed as it came",
            &["profile", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":"no such workspace"}"#.into(),
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal with a permission card: no closing line on exit 0",
            &["profile"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            0,
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
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
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
                "a live cell socket: still TCP",
                &["list", "--json"],
                &[],
                vec![ok(AGENTS)],
                port_only,
                0,
                1,
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
                0,
                2,
            ),
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                h(&[("K2_HOOK_SOCK", "{nosock}")]),
                case(
                    "HOOK_SOCK set, the owner on disk (PRD 13.9 #4)",
                    &["list"],
                    &[],
                    vec![ok(ROWS)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
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
        in_pipe(
            case(
                "list: an empty reply, the line, then SIGPIPE",
                &["list"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
            false,
        ),
        in_pipe(
            case(
                "list --json: a 5xx with no body, the line, then SIGPIPE",
                &["list", "--json"],
                &[],
                vec![Reply::Answer(502, String::new())],
                owner_and_port,
                141,
                1,
            ),
            true,
        ),
        case(
            "list: a reset, the reader on nothing",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "list --json: not HTTP, the reader on nothing",
            &["list", "--json"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "list: TMPDIR unusable, a reset",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
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
            "profile: a reset (56)",
            &["profile"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "profile: not HTTP (1)",
            &["profile"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "profile: TMPDIR unusable, a reset (76)",
            &["profile"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            76,
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
    for (name, args, code) in [
        ("list: nothing listens", &["list"][..], 1),
        ("list --json: nothing listens", &["list", "--json"], 1),
        ("profile: nothing listens (7)", &["profile"], 7),
    ] {
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        rig.check(&case(
            name,
            args,
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            code,
            0,
        ));
    }
}

/// No answer within 30 s: the line, then SIGPIPE (`list`) or exit 1
/// (`profile`). Slow.
#[test]
fn workspace_reads_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&in_pipe(
        case(
            "list: no answer in time (28)",
            &["list"],
            &[],
            vec![Reply::Hang(Duration::from_secs(35))],
            owner_and_port,
            141,
            1,
        ),
        false,
    ));
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
            1,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
