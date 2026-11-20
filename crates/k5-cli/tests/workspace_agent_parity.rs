//! `k5-cli workspace launch [<path>]` and `workspace update --field <f>
//! --value <v>` against the shell CLI's `cli/k2 workspace launch` and
//! `update`, on the parity rig ([`common::parity`]): the same requests,
//! stdout, stderr and exit code.
//!
//! - `launch`: the workspace, a folder, `--json` anywhere, `--no-attach`,
//!   an unknown flag and a second target (exit 2), a name the daemon
//!   resolves (a path; none, `null`, not JSON, not an object: the error
//!   or the no-folder line, exit 1); the answer and a newline, an error on
//!   stderr (exit 1), a refusal with a permission card (exit 3),
//!   migrating, an older daemon's 405, a healed passport.
//! - `update`: the agent's name from the daemon or the folder's, the
//!   usage errors (a flag with no value, an unknown word, no field or
//!   value: exit 2), a projects DB column (exit 2), the answer, an error
//!   on stderr, a lookup's refusal that is no closing line.
//! - A request with no answer, or none taken, the lookup's or the POST's:
//!   the line, exit 1.
//! - The POST sent once: an empty 200 is no success; an empty reply, a
//!   5xx with no body and none in time are the line and exit 1; a reset,
//!   not HTTP and TMPDIR unusable are the line with curl's code, exit 1.
//! - The cell socket first (its card: exit 3; a POST it gave no answer:
//!   may have been applied, exit 1), a session's passport, a session with
//!   no passport (no credential).
//! - What the binary hands to the shell: help, a workspace path that is
//!   not absolute.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "workspace";
const ENSURED: &str = "{\"ok\":true,\"action\":\"spawn\",\"sessionId\":\"s1\"}\n";
const UPDATED: &str = r#"{"ok":true,"agent":"Acme"}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn words(v: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(v.into_boxed_slice())
}

fn with_sub(r: &Rig) {
    owner_and_port(r);
    std::fs::create_dir_all(r.proj.join("sub")).unwrap();
}

#[test]
fn workspace_launch_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "launch");
    let proj = rig.proj.to_str().unwrap().to_string();
    let v = vec![
        case(
            "the workspace",
            &["launch"],
            &[],
            vec![ok(ENSURED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a folder, --json before and after it",
            &["launch", "--json", "sub", "--json"],
            &[],
            vec![ok(ENSURED)],
            with_sub,
            0,
            1,
        ),
        case(
            "--no-attach is refused, exit 2",
            &["launch", "sub", "--no-attach"],
            &[],
            vec![],
            with_sub,
            2,
            0,
        ),
        case(
            "an unknown flag, exit 2",
            &["launch", "--attach", "sub"],
            &[],
            vec![],
            with_sub,
            2,
            0,
        ),
        case(
            "a second target, exit 2",
            &["launch", "sub", "other"],
            &[],
            vec![],
            with_sub,
            2,
            0,
        ),
        case(
            "an empty word is no target",
            &["launch", "", "sub"],
            &[],
            vec![ok(ENSURED)],
            with_sub,
            0,
            1,
        ),
        case(
            "an absolute folder",
            words(vec!["launch", leak(format!("{proj}/sub"))]),
            &[],
            vec![ok(ENSURED)],
            with_sub,
            0,
            1,
        ),
        case(
            "a name the daemon resolves",
            &["launch", "acme é&x"],
            &[],
            vec![ok("{\"path\":\"/srv/acme ws\\n\"}\n"), ok(ENSURED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "a name the daemon has no workspace for: its error, exit 1",
            &["launch", "acme"],
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
            "a lookup's permission card: exit 3, the closing line",
            &["launch", "acme"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a path of null: no folder, exit 1",
            &["launch", "acme"],
            &[],
            vec![ok(r#"{"path":null}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a path of spaces: no folder, exit 1",
            &["launch", "acme"],
            &[],
            vec![ok(r#"{"path":" \t"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "no folder: the answer cut at 200",
            &["launch", "acme"],
            &[],
            vec![ok(leak(format!(
                r#"{{"path":"","pad":"{}"}}"#,
                "x".repeat(300)
            )))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a lookup answer that is not JSON, exit 1",
            &["launch", "acme"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a lookup answer that is not an object, exit 1",
            &["launch", "acme"],
            &[],
            vec![ok("[\"/srv/x\"]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--json alone: the workspace",
            &["launch", "--json"],
            &[],
            vec![ok(ENSURED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an error: on stderr, exit 1",
            &["launch"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"busy","hint":"starting"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: exit 3, the closing line",
            &["launch"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the bulletin",
            &["launch"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            &["launch"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(ENSURED),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "an older daemon's 405: the same call as a GET",
            &["launch"],
            &[],
            vec![
                Reply::Answer(
                    405,
                    r#"{"error":"method not allowed for this route"}"#.into(),
                ),
                ok(ENSURED),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the gates: no token",
            &["launch"],
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
fn workspace_update_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "update");
    let v = vec![
        case(
            "the agent's name from the daemon",
            &["update", "--field", "role", "--value", "frontend lead é&x"],
            &[],
            vec![ok(r#"{"display_name":"Acme Lead"}"#), ok(UPDATED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "no name: the folder's",
            &["update", "--value", "v", "--field", "role"],
            &[],
            vec![ok(r#"{"display_name":""}"#), ok(UPDATED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the lookup's permission card is no closing line",
            &["update", "--field", "role", "--value", "v"],
            &[],
            vec![
                Reply::Answer(403, PERMISSION_ASK.into()),
                Reply::Answer(400, r#"{"error":"unknown field"}"#.into()),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "a lookup answer that is not JSON: the folder's",
            &["update", "--field", "role", "--value", "v"],
            &[],
            vec![Reply::Answer(500, "oops".into()), ok(UPDATED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "a name that is not text",
            &["update", "--field", "role", "--value", "v"],
            &[],
            vec![ok(r#"{"display_name":{"a":[1,true]}}"#), ok(UPDATED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "an error: on stderr, exit 1",
            &["update", "--field", "role", "--value", "v"],
            &[],
            vec![
                ok(r#"{"display_name":"Acme"}"#),
                Reply::Answer(400, r#"{"error":"unknown field"}"#.into()),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "an older daemon's 405: the same call as a GET",
            &["update", "--field", "role", "--value", "v"],
            &[],
            vec![
                ok(r#"{"display_name":"Acme"}"#),
                Reply::Answer(
                    405,
                    r#"{"error":"method not allowed for this route"}"#.into(),
                ),
                ok(UPDATED),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "no field: the usage line, exit 2",
            &["update", "--value", "v"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an empty value: the usage line, exit 2",
            &["update", "--field", "role", "--value", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--field with no value: a named usage error, exit 2",
            &["update", "--value", "v", "--field"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--value with no value: a named usage error, exit 2",
            &["update", "--field", "role", "--value"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown word: a named usage error, exit 2",
            &["update", "x", "--field", "role", "--value", "v"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a projects DB column (exit 2)",
            &["update", "--field", "agent_mode", "--value", "off"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "the gates come first",
            &["update"],
            &[],
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

#[test]
fn workspace_launch_and_update_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let v = vec![
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "launch: a live cell socket first, the lookup and the POST",
                &["launch", "acme"],
                &[],
                vec![ok(r#"{"path":"/srv/acme"}"#), ok(ENSURED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "update: the cell socket's card for the POST, exit 3",
                &["update", "--field", "role", "--value", "v"],
                &[],
                vec![
                    ok(r#"{"display_name":"A"}"#),
                    Reply::Answer(403, PERMISSION_ASK.into()),
                ],
                port_only,
                3,
                2,
            ),
        ),
        waited(with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "launch: no answer to the POST on the cell socket: may have been applied, exit 1",
                &["launch"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        with(
            h(&[]),
            case(
                "update: the lookup heals in its subshell, the POST has the old passport",
                &["update", "--field", "role", "--value", "v"],
                &[],
                vec![heal(&rig), ok(r#"{"display_name":"A"}"#), ok(UPDATED)],
                port_only,
                0,
                3,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "launch: a session, the owner on disk: the passport",
                &["launch"],
                &[],
                vec![ok(ENSURED)],
                owner_and_port,
                0,
                1,
            ),
        ),
        case(
            "launch: a session with no passport, the owner on disk: no credential",
            &["launch", "acme"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![ok(r#"{"path":"/srv/acme"}"#), ok(ENSURED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "update: a session with no passport: no credential",
            &["update", "--field", "role", "--value", "v"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![ok(r#"{"display_name":"A"}"#), ok(UPDATED)],
            owner_and_port,
            0,
            2,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lookup with no answer, or a failed one: the line, exit 1, no POST.
#[test]
fn workspace_lookups_with_no_answer() {
    let rig = Rig::new(VERB, "lookup");
    let v = vec![
        waited(case(
            "launch: the lookup's empty reply, the line, exit 1",
            &["launch", "acme"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "update: the lookup's 5xx with no body, the line, exit 1",
            &["update", "--field", "role", "--value", "v"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "launch: the lookup's reset (56), exit 1",
            &["launch", "acme"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "update: the lookup not HTTP (1), exit 1",
            &["update", "--field", "role", "--value", "v"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    let port = dead_port();
    rig.check(&case(
        "launch: nothing listens for the lookup (7), exit 1",
        &["launch", "acme"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// A POST that may have gone out is never sent again.
#[test]
fn workspace_launch_and_update_are_sent_once() {
    let rig = Rig::new(VERB, "once");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let up: &'static [&'static str] = &["update", "--field", "role", "--value", "v"];
    let name = || ok(r#"{"display_name":"Acme"}"#);
    let v = vec![
        case(
            "launch: an empty 200 is no success",
            &["launch"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        waited(case(
            "launch: an empty reply (52), the line, exit 1",
            &["launch"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "launch: a 5xx with no body, the line, exit 1",
            &["launch"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "launch: a reset (56), exit 1",
            &["launch"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "launch: not HTTP (1)",
            &["launch"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "launch: TMPDIR unusable, a reset (76), exit 1",
            &["launch"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "update: an empty reply (52), the line, exit 1",
            up,
            &[],
            vec![name(), Reply::Close],
            owner_and_port,
            1,
            2,
        )),
        waited(case(
            "update: a 5xx with no body, the line, exit 1",
            up,
            &[],
            vec![name(), Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            2,
        )),
        case(
            "update: an empty 200 is no success",
            up,
            &[],
            vec![name(), ok("")],
            owner_and_port,
            1,
            2,
        ),
        case(
            "update: a reset (56), exit 1",
            up,
            &[],
            vec![name(), Reply::Reset],
            owner_and_port,
            1,
            2,
        ),
        case(
            "update: not HTTP (1)",
            up,
            &[],
            vec![name(), Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            2,
        ),
        no_tmp(case(
            "update: TMPDIR unusable, a reset (76), exit 1",
            up,
            &[],
            vec![name(), Reply::Reset],
            owner_and_port,
            1,
            2,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
    let port = dead_port();
    rig.check(&case(
        "launch: nothing listens (7), exit 1",
        &["launch"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// No answer within 30 s (28) to the POST: the line, exit 1. Slow.
#[test]
fn workspace_launch_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&waited(case(
        "launch: no answer in time (28)",
        &["launch"],
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
fn workspace_launch_and_update_hand_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let v = vec![
        shell(case(
            "launch --help",
            &["launch", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "update -h",
            &["update", "--field", "a", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "update: a workspace path that is not absolute",
            &["update", "--field", "role", "--value", "v"],
            &[("K2_PROJECT_PATH", "rel ws")],
            vec![ok("{}"), ok(UPDATED)],
            owner_and_port,
            0,
            2,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
