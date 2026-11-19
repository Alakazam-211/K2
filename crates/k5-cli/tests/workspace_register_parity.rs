//! `k5-cli workspace create <path>` and `workspace open <path>` against
//! the shell CLI's `cli/k2 workspace create` and `open`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - The words: the flags (sent in the shell's order), an unknown flag, a
//!   second path, no path (usage, exit 1, nothing sent).
//! - The path: relative, missing parents, `..` and `.`, a `/` at the end,
//!   `/` itself, a folder through a link (`cd` is logical), and for
//!   `open` a folder as `cd` names it and anything else as typed.
//! - The answers (`cli_print_or_fail`): printed with a newline, the
//!   daemon's error (text, `hint`, `code`, neither, folder trust with its
//!   message), a refusal with a permission card (exit 3 and the closing
//!   line), not JSON, an answer that is not an object (exit 1, nothing
//!   printed), an error of only newlines, the bulletin, migrating, an
//!   older daemon's 405 (the same call as a GET), a healed passport.
//! - Sent once: an empty 200 is the answer; an empty reply, a 5xx with no
//!   body and none in time are the line and exit 1; a reset (56), not
//!   HTTP (1) and TMPDIR unusable (76) are curl's code; only a connect
//!   nothing accepted (7) is tried again. Each with one request.
//! - What the binary hands to the shell: help, a doubled `/`, a relative
//!   `open` while `CDPATH` is set, a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "workspace";
const MADE: &str = r#"{"ok":true,"workspace":{"id":"w1","name":"new-ws","path":"/srv/new-ws"}}"#;

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

/// Folders the paths name: `proj/sub/inner`, a file, and `sublink` beside
/// `proj` pointing at `proj/sub`.
fn folders(r: &Rig) {
    owner_and_port(r);
    std::fs::create_dir_all(r.proj.join("sub/inner")).unwrap();
    std::fs::write(r.proj.join("file"), "").unwrap();
    let link = r.dir.join("sublink");
    if !link.exists() {
        std::os::unix::fs::symlink(r.proj.join("sub"), &link).unwrap();
    }
}

fn register_answers(verb: &'static str, tag: &str) {
    let rig = Rig::new(VERB, tag);
    let a = |rest: &[&'static str]| {
        let mut v = vec![verb];
        v.extend_from_slice(rest);
        words(v)
    };
    let v = vec![
        case(
            "the answer",
            a(&["new-ws"]),
            &[],
            vec![ok(MADE)],
            folders,
            0,
            1,
        ),
        case(
            "every flag, in the shell's order",
            a(&["--fanout", "new ws&x=y é", "--no-agents-md", "--no-wiki"]),
            &[],
            vec![ok(MADE)],
            folders,
            0,
            1,
        ),
        case(
            "the daemon's error in text",
            a(&["new-ws"]),
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":"A workspace already exists at that path."}"#.into(),
            )],
            folders,
            1,
            1,
        ),
        case(
            "an error's hint",
            a(&["new-ws"]),
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"not_writable","hint":"The folder is not writable é"}}"#.into(),
            )],
            folders,
            1,
            1,
        ),
        case(
            "an error's code",
            a(&["new-ws"]),
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"poc_of_projects","hint":""}}"#.into(),
            )],
            folders,
            1,
            1,
        ),
        case(
            "an error with neither",
            a(&["new-ws"]),
            &[],
            vec![Reply::Answer(400, r#"{"error":{"x":[1,null]}}"#.into())],
            folders,
            1,
            1,
        ),
        case(
            "folder trust, with its message",
            a(&["new-ws"]),
            &[],
            vec![ok(
                r#"{"error":"folder_trust_pending","message":"Asked the owner on Thread."}"#,
            )],
            folders,
            1,
            1,
        ),
        case(
            "an error that is not text",
            a(&["new-ws"]),
            &[],
            vec![ok(r#"{"error":[1,{"a":true}],"message":"m"}"#)],
            folders,
            1,
            1,
        ),
        case(
            "an error of only newlines is printed as an answer",
            a(&["new-ws"]),
            &[],
            vec![ok("{\"error\":\"\\n\\n\"}\n\n")],
            folders,
            0,
            1,
        ),
        case(
            "a refusal with a permission card: exit 3 and the closing line",
            a(&["new-ws"]),
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            folders,
            3,
            1,
        ),
        case(
            "not JSON is printed as an answer",
            a(&["new-ws"]),
            &[],
            vec![ok("<html>it's up</html>\n")],
            folders,
            0,
            1,
        ),
        case(
            "an answer that is not an object: exit 1, nothing printed",
            a(&["new-ws"]),
            &[],
            vec![ok("[1, 2]")],
            folders,
            1,
            1,
        ),
        case(
            "the bulletin",
            a(&["new-ws"]),
            &[],
            vec![ok(BULLETIN)],
            folders,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            a(&["new-ws"]),
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(MADE),
            ],
            folders,
            0,
            2,
        ),
        case(
            "an older daemon's 405: the same call as a GET",
            a(&["new-ws"]),
            &[],
            vec![
                Reply::Answer(
                    405,
                    r#"{"error":"method not allowed for this route"}"#.into(),
                ),
                ok(MADE),
            ],
            folders,
            0,
            2,
        ),
        case("no path", a(&[]), &[], vec![], folders, 1, 0),
        case(
            "an empty path",
            a(&["", "--fanout"]),
            &[],
            vec![],
            folders,
            1,
            0,
        ),
        case(
            "an unknown flag",
            a(&["new-ws", "--json"]),
            &[],
            vec![],
            folders,
            1,
            0,
        ),
        case("a second path", a(&["a", "b"]), &[], vec![], folders, 1, 0),
        case(
            "the gates: no port",
            a(&["x"]),
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
fn workspace_create_answers_as_the_shell_does() {
    register_answers("create", "create");
}

#[test]
fn workspace_open_answers_as_the_shell_does() {
    register_answers("open", "open");
}

/// The path each verb sends.
#[test]
fn workspace_create_and_open_send_the_shell_s_path() {
    let rig = Rig::new(VERB, "paths");
    let proj = rig.proj.to_str().unwrap().to_string();
    let dir = rig.dir.to_str().unwrap().to_string();
    let mut v = Vec::new();
    for verb in ["create", "open"] {
        for (name, path) in [
            ("a folder", "sub"),
            ("a folder, a / at its end", "sub/inner/"),
            ("a folder through . and ..", "./sub/inner/../."),
            ("missing parents", "sub/a/b/c"),
            ("a missing folder past ..", "../sibling/x"),
            ("a file", "file"),
            ("a path under a file", "file/x"),
            ("/", "/"),
            ("a missing top folder", "/nonexistent-k5-ws-root/ai"),
            ("an absolute folder", leak(format!("{proj}/sub"))),
            (
                "a folder through a link",
                leak(format!("{dir}/sublink/inner")),
            ),
            (
                "a missing folder through a link",
                leak(format!("{dir}/sublink/new")),
            ),
            ("a link and ..", leak(format!("{dir}/sublink/.."))),
        ] {
            v.push(case(
                leak(format!("{verb}: {name}")),
                words(vec![verb, path]),
                &[],
                vec![ok(MADE)],
                folders,
                0,
                1,
            ));
        }
    }
    v.push(Case {
        cwd: Some(rig.proj.join("sub")),
        ..case(
            "create: a relative path from a folder below the workspace",
            &["create", "../new"],
            &[],
            vec![ok(MADE)],
            folders,
            0,
            1,
        )
    });
    v.push(Case {
        cwd: Some(link_to_proj(&rig)),
        ..case(
            "open: a PWD through a link",
            &["open", "sub"],
            &[("PWD", leak(format!("{dir}/link")))],
            vec![ok(MADE)],
            folders,
            0,
            1,
        )
    });
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn workspace_create_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
    let v = vec![
        with(
            h(&[]),
            case(
                "a passport over TCP",
                &["create", "new-ws"],
                &[],
                vec![ok(MADE)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a live cell socket: still TCP",
                &["open", "new-ws"],
                &[],
                vec![ok(MADE)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["create", "new-ws"],
                &[],
                vec![heal(&rig), ok(MADE)],
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
                    &["create", "new-ws"],
                    &[],
                    vec![ok(MADE)],
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

/// A POST that may have gone out is never sent again: one request each.
fn sent_once(verb: &'static str, tag: &str) {
    let rig = Rig::new(VERB, tag);
    let args = words(vec![verb, "new-ws"]);
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let v = vec![
        case(
            "an empty 200 is the answer",
            args,
            &[],
            vec![ok("")],
            owner_and_port,
            0,
            1,
        ),
        waited(case(
            "an empty reply (52): the line, exit 1",
            args,
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "a 5xx with no body: the line, exit 1",
            args,
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a reset (56)",
            args,
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "not HTTP (000, curl 1)",
            args,
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset (76)",
            args,
            &[],
            vec![Reply::Reset],
            owner_and_port,
            76,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            args,
            &[],
            vec![ok(MADE)],
            owner_and_port,
            0,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
    let port = dead_port();
    rig.check(&case(
        "nothing listens (7): tried again, exit 7",
        args,
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        7,
        0,
    ));
}

#[test]
fn workspace_create_is_sent_once() {
    sent_once("create", "createonce");
}

#[test]
fn workspace_open_is_sent_once() {
    sent_once("open", "openonce");
}

/// No answer within 30 s (28): the line, exit 1, one request. Slow.
#[test]
fn workspace_create_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&waited(case(
        "no answer in time (28)",
        &["create", "new-ws"],
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
fn workspace_create_and_open_hand_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let v = vec![
        shell(case(
            "create --help",
            &["create", "--help"],
            &[],
            vec![],
            folders,
            0,
            0,
        )),
        shell(case(
            "open x -h",
            &["open", "x", "-h"],
            &[],
            vec![],
            folders,
            0,
            0,
        )),
        shell(case(
            "create: a doubled /",
            &["create", "//nonexistent-k5-ws/x"],
            &[],
            vec![ok(MADE)],
            folders,
            0,
            1,
        )),
        shell(case(
            "open: a relative folder while CDPATH is set",
            &["open", "sub"],
            &[("CDPATH", "/nonexistent-k5-cdpath")],
            vec![ok(MADE)],
            folders,
            0,
            1,
        )),
        shell(case(
            "a port that is no port: curl's 3",
            &["create", "x"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            3,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
