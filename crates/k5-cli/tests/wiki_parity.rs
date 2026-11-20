//! `k5-cli wiki` against the shell CLI's `cli/k2 wiki`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - Every verb's request and output: `status`, `index` (and more than
//!   256 KiB of nodes), `note`, `seed`, `serve on|off`, `chat off`,
//!   `unpublish`; `--json`; the workspace as a folder and as a name the
//!   daemon resolves (or doesn't).
//! - The CLI's own refusals (`_wiki_err usage`, `chat on`'s exit 3), the
//!   help pages a verb prints, an unknown verb.
//! - The daemon's error, an answer that is not JSON, one a reader can't
//!   read (a raise).
//! - No answer: a dead port (curl 7), an empty reply, a reset; the index
//!   `status` reads with its errors hidden.
//! - Help anywhere on every verb that changes something (no request), the
//!   gate, a passport, and the owner token on disk in a session
//!   (`owner-token-with-passport`).
//! - `wiki publish` and the edges the binary hands to the shell.
//! - `status` with an index above 1 MiB: bash can't hand it to its Python
//!   in an environment variable; the named difference
//!   `env-answer-large`.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "wiki";
const STATUS: &str = r#"{"enabled":true,"port":4711,"url":"http://127.0.0.1:4711","publishedLabel":"acme-brain","generatedAt":"2026-11-20T10:00:00Z","extra":"é"}"#;
const INDEX: &str = r#"{"workspacePath":"/srv/acme","wikiRel":".k2/wiki","generatedAt":"2026-11-20T10:00:00Z","nodes":[{"id":"Home.md","title":"Home é"},{"id":"_Index.md","title":"Index"},{"id":"Gone.md","exists":false}],"links":[{"from":"Home.md","to":"_Index.md"}]}"#;
const NOTE: &str =
    r##"{"id":"Home.md","title":"Home","tags":["start","é"],"body":"# Home\n\nSee [[_Index]]."}"##;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

/// A request on the left of a pipe into the shell's reader, with no
/// answer: the binary's line, then SIGPIPE. macOS's `/bin/bash` 3.2 runs
/// the reader on the empty input, which prints `stdout` and `stderr`, and
/// exits 1 (`scripts/cli-parity.sh`'s named difference
/// `bash32-pipe-reader`, through [`Compare::Bash32PipeReader`]).
fn in_pipe(c: Case, stdout: &'static str, stderr: &'static str) -> Case {
    Case {
        compare: Compare::Bash32PipeReader { stdout, stderr },
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
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

/// An index of `n` notes.
fn many(n: usize) -> String {
    let nodes: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"note-{i:05}.md","title":"Note {} é {i}","exists":{}}}"#,
                n - i,
                i % 7 != 0
            )
        })
        .collect();
    format!(
        r#"{{"workspacePath":"/srv/acme","generatedAt":"g","nodes":[{}],"links":[]}}"#,
        nodes.join(",")
    )
}

#[test]
fn wiki_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let big = leak(many(6000));
    assert!(big.len() > 256 * 1024, "the large case is above 256 KiB");
    let v = vec![
        // covers: wiki
        case(
            "wiki alone: the help",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        // covers: wiki --json
        case(
            "--json is no verb",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "an unknown verb",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: wiki status
        case(
            "status",
            &["status"],
            &[],
            vec![ok(STATUS), ok(INDEX)],
            owner_and_port,
            0,
            2,
        ),
        // covers: wiki status --json
        case(
            "status --json",
            &["status", "--json"],
            &[],
            vec![ok(STATUS), ok(INDEX)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "status, served off, no index",
            &["status", "--port", "9"],
            &[],
            vec![
                ok(r#"{"enabled":false,"publicChatEnabled":true}"#),
                ok("nope"),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "status, the daemon's error",
            &["status"],
            &[],
            vec![
                Reply::Answer(500, r#"{"error":{"hint":"no wiki"}}"#.into()),
                ok(INDEX),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "status --json, the daemon's error",
            &["status", "--json"],
            &[],
            vec![ok(r#"{"error":"down é"}"#), ok(INDEX)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "status, an empty answer",
            &["status"],
            &[],
            vec![ok(""), ok(INDEX)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "status, the index gives no answer: exit 1, no line",
            &["status"],
            &[],
            vec![ok(STATUS), Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        case(
            "status, the index is reset: an empty index",
            &["status"],
            &[],
            vec![ok(STATUS), Reply::Reset],
            owner_and_port,
            0,
            2,
        ),
        traceback(case(
            "status, a node that is no dict",
            &["status"],
            &[],
            vec![ok("{}"), ok(r#"{"nodes":[{"id":"a"},3]}"#)],
            owner_and_port,
            1,
            2,
        )),
        traceback(case(
            "status, a list",
            &["status"],
            &[],
            vec![ok("[1]"), ok("{}")],
            owner_and_port,
            1,
            2,
        )),
        // covers: wiki index
        case(
            "index",
            &["index"],
            &[],
            vec![ok(INDEX)],
            owner_and_port,
            0,
            1,
        ),
        // covers: wiki index --json
        case(
            "index --json",
            &["index", "--json"],
            &[],
            vec![ok(INDEX)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "index, more than 256 KiB of notes",
            &["index"],
            &[],
            vec![ok(big)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "index --json, more than 256 KiB of notes",
            &["index", "--json"],
            &[],
            vec![ok(big)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "index, an empty vault",
            &["index"],
            &[],
            vec![ok(r#"{"nodes":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "index, not JSON",
            &["index"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "index --json, not JSON",
            &["index", "--json"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "index --json, the daemon's error",
            &["index", "--json"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"nope"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "index, an empty answer",
            &["index", "--json"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "index, a title that is no string",
            &["index"],
            &[],
            vec![ok(r#"{"nodes":[{"id":"a"},{"title":5}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: wiki note
        case(
            "note",
            &["note", "Home é.md"],
            &[],
            vec![ok(NOTE)],
            owner_and_port,
            0,
            1,
        ),
        // covers: wiki note --json
        case(
            "note --json",
            &["note", "--json", "Home.md"],
            &[],
            vec![ok(NOTE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "note, the daemon's error",
            &["note", "Nope.md"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case("note, no id", &["note"], &[], vec![], owner_and_port, 2, 0),
        case(
            "note --json, no id",
            &["note", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "note, an unknown flag",
            &["note", "a", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "note, a third word",
            &["note", "a", "b", "c"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "note, a body that is no string",
            &["note", "a"],
            &[],
            vec![ok(r#"{"title":"t","body":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: wiki seed
        case(
            "seed",
            &["seed"],
            &[],
            vec![ok(
                r#"{"created":["Home.md","_Index.md"],"wikiRel":".k2/wiki"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "seed, already seeded",
            &["seed", "--json"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "seed, a flag with no value",
            &["seed", "--label"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "seed --json, an unknown flag: JSON",
            &["seed", "--json", "--nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: wiki serve
        case(
            "serve on --port",
            &["serve", "on", "--port", "4711"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "serve off",
            &["serve", "stop"],
            &[],
            vec![ok(r#"{"enabled":false,"published_label":"acme"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "serve on, the daemon's error",
            &["serve", "true"],
            &[],
            vec![ok(r#"{"error":{"hint":"port taken"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "serve alone: its help, exit 2",
            &["serve"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "serve help",
            &["serve", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "serve, a bad mode",
            &["serve", "up"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: wiki chat
        case(
            "chat on: refused, no daemon",
            &["chat", "on"],
            &[],
            vec![],
            nothing,
            3,
            0,
        ),
        case(
            "chat on --json",
            &["chat", "enable", "--json"],
            &[],
            vec![],
            nothing,
            3,
            0,
        ),
        case(
            "chat on, a bad flag",
            &["chat", "on", "--x"],
            &[],
            vec![],
            nothing,
            2,
            0,
        ),
        case(
            "chat off",
            &["chat", "off"],
            &[],
            vec![ok(r#"{"publicChatEnabled":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "chat off, an older daemon",
            &["chat", "0"],
            &[],
            vec![ok(r#"{"publicChatEnabled":true}"#)],
            owner_and_port,
            1,
            1,
        ),
        case("chat alone", &["chat"], &[], vec![], owner_and_port, 2, 0),
        // covers: wiki unpublish
        case(
            "unpublish",
            &["unpublish"],
            &[],
            vec![ok(STATUS), ok(r#"{"enabled":false}"#)],
            owner_and_port,
            0,
            2,
        ),
        // covers: wiki unpublish --json
        case(
            "unpublish --json, no label",
            &["unpublish", "--json"],
            &[],
            vec![ok("{}"), ok(r#"{"error":"busy"}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "unpublish --label, an empty serve answer",
            &["unpublish", "--label", "mine", "--json"],
            &[],
            vec![Reply::Reset, ok("")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "unpublish, the status gives no answer",
            &["unpublish"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        waited(case(
            "unpublish, serve off gives no answer",
            &["unpublish"],
            &[],
            vec![ok(STATUS), Reply::Close],
            owner_and_port,
            1,
            2,
        )),
        waited(case(
            "status, no answer",
            &["status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        case(
            "index, a reset",
            &["index"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "the gate: no port",
            &["status"],
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
fn wiki_finds_the_workspace() {
    let rig = Rig::new(VERB, "ws");
    std::fs::create_dir_all(rig.dir.join("other ws")).unwrap();
    let other = leak(rig.dir.join("other ws").to_string_lossy().into_owned());
    let v = vec![
        Case {
            cwd: Some(rig.dir.clone()),
            ..case(
                "a folder, relative",
                &["index", "--workspace", "other ws/."],
                &[],
                vec![ok(INDEX)],
                owner_and_port,
                0,
                1,
            )
        },
        case(
            "a folder, the trailing word",
            leak_words(&["seed", other]),
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a name the daemon resolves",
            &["status", "--project=acme é"],
            &[],
            vec![
                ok(r#"{"projectPath":"/srv/acme é"}"#),
                ok(STATUS),
                ok(INDEX),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "a name the daemon doesn't know",
            &["note", "Home.md", "globex"],
            &[],
            vec![
                Reply::Answer(404, r#"{"error":"no such workspace"}"#.into()),
                ok(NOTE),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the resolve is reset: the name as it is",
            &["index", "globex"],
            &[],
            vec![Reply::Reset, ok(INDEX)],
            owner_and_port,
            0,
            2,
        ),
        in_pipe(
            case(
                "the resolve gives no answer: SIGPIPE",
                &["index", "globex"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
            "",
            "",
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn wiki_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a passport, no session",
                &["status"],
                &[],
                vec![ok(STATUS), ok(INDEX)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a session, no owner on disk: the passport over TCP",
                &["index"],
                &[],
                vec![ok(INDEX)],
                port_only,
                0,
                1,
            ),
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "a session, the owner on disk (PRD 13.9 #4)",
                    &["serve", "on"],
                    &[],
                    vec![ok(STATUS)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
        with(
            with_passport(&[("K2_PROJECT_PATH", "/set")]),
            case(
                "a refused passport heals once",
                &["index"],
                &[],
                vec![heal(&rig), ok(INDEX)],
                port_only,
                0,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn wiki_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    for (name, args) in [
        ("status: curl's 7", &["status"][..]),
        ("seed: curl's 7", &["seed"][..]),
    ] {
        rig.check(&case(
            name,
            leak_words(args),
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            7,
            0,
        ));
    }
}

fn leak_words(w: &[&str]) -> &'static [&'static str] {
    let v: Vec<&'static str> = w.iter().map(|s| leak((*s).to_string())).collect();
    Box::leak(v.into_boxed_slice())
}

/// Help anywhere on a verb that changes something prints help and sends
/// nothing; `publish` and a port curl won't take are the shell's. (A
/// `--workspace` folder `cd` reads as an option is the shell's too, where
/// bash prints its own error naming the script, D6: not compared here.)
#[test]
fn wiki_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let mut v = Vec::new();
    for args in [
        &["seed", "--help"][..],
        &["serve", "on", "-h"],
        &["serve", "-h", "off"],
        &["chat", "off", "--help"],
        &["unpublish", "--json", "-h"],
        &["publish", "-h"],
        &["publish", "--label", "x", "--help"],
    ] {
        v.push(shell(case(
            "help never acts",
            leak_words(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )));
    }
    // The control plane over HTTPS waits for the TLS client: with a token
    // in tunnel.json (and air-gap off) publish is the shell's.
    v.push(shell(case(
        "publish: a tunnel token on disk",
        &["publish", "--label", "acme"],
        &[],
        made_replies(),
        tunnel_token,
        0,
        6,
    )));
    v.push(shell(case(
        "a port that is no port",
        &["status"],
        &[("K2_PORT", "41o0")],
        vec![],
        owner_only,
        3,
        0,
    )));
    for c in v {
        rig.check(&c);
    }
}

fn tunnel_token(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.k2home.join("tunnel.json"), r#"{"subdomain":"acme","token":"tun-tok"}"#)
        .unwrap();
}

const SERVED: &str = r#"{"enabled":true,"port":4711,"url":"http://127.0.0.1:4711"}"#;
const CHECKED: &str = r#"{"success":true}"#;

/// The control plane's answer as the daemon relays it.
fn relayed(status: u16, body: &str) -> Reply {
    let b = body.replace('\\', "\\\\").replace('"', "\\\"");
    ok(leak(format!(r#"{{"status":{status},"body":"{b}"}}"#)))
}

/// Serve on, its status, the claim check, the create, the stamp, the
/// nudge.
fn made_replies() -> Vec<Reply> {
    vec![
        ok(SERVED),
        ok(STATUS),
        ok(CHECKED),
        relayed(
            201,
            r#"{"label":"acme","host":"acme.box.k2.dev","target":"127.0.0.1:4711","status":"active","effective":"acme.box.k2.dev"}"#,
        ),
        ok(CHECKED),
        ok("{}"),
    ]
}

/// `wiki publish`: serve on, the port, the claim check, then the Connect
/// steps through the daemon (/cli/connect/proxy), create else point, the
/// stamp and the nudge; the manual lines when none of it made the name.
#[test]
fn wiki_publish_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "publish");
    let taken = relayed(409, r#"{"error":"label_taken"}"#);
    let pointed = relayed(200, r#"{"label":"acme","target":"127.0.0.1:4711"}"#);
    let v = vec![
        // covers: wiki publish
        case(
            "publish: created through the daemon",
            &["publish", "--label", "acme"],
            &[],
            made_replies(),
            owner_and_port,
            0,
            6,
        ),
        // covers: wiki publish --json
        case(
            "publish --json, the folder's name as the label",
            &["publish", "--json"],
            &[],
            made_replies(),
            owner_and_port,
            0,
            6,
        ),
        case(
            "publish --port: the create is taken, the point works",
            &["publish", "--port", "4711", "--label", "acme"],
            &[],
            vec![
                ok(SERVED),
                ok(STATUS),
                ok(CHECKED),
                taken.clone(),
                pointed,
                ok(CHECKED),
                ok("{}"),
            ],
            owner_and_port,
            0,
            7,
        ),
        case(
            "publish: another workspace owns the label",
            &["publish", "--label", "acme", "--json"],
            &[],
            vec![
                ok(SERVED),
                ok(STATUS),
                ok(r#"{"error":"owned by another workspace","hint":"transfer it first"}"#),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "publish: a daemon with no proxy route (404), no token on disk",
            &["publish", "--label", "acme"],
            &[],
            vec![
                ok(SERVED),
                ok(STATUS),
                ok(CHECKED),
                Reply::Answer(404, "not found".into()),
                Reply::Answer(405, "".into()),
            ],
            owner_and_port,
            0,
            5,
        ),
        case(
            "publish: the daemon refuses the Connect call",
            &["publish", "--label", "acme"],
            &[],
            vec![
                ok(SERVED),
                ok(STATUS),
                ok(CHECKED),
                Reply::Answer(403, r#"{"error":"forbidden"}"#.into()),
                Reply::Answer(500, r#"{"code":"x","error":"boom"}"#.into()),
            ],
            owner_and_port,
            0,
            5,
        ),
        case(
            "publish: both relayed answers are errors, one unreadable",
            &["publish", "--label", "acme"],
            &[],
            vec![
                ok(SERVED),
                ok(STATUS),
                ok(CHECKED),
                taken,
                ok(r#"{"status":"x"}"#),
            ],
            owner_and_port,
            0,
            5,
        ),
        case(
            "publish: the stamp fails",
            &["publish", "--label", "acme"],
            &[],
            vec![
                ok(SERVED),
                ok(STATUS),
                ok(CHECKED),
                relayed(201, "{}"),
                ok(r#"{"error":"stamp refused"}"#),
                Reply::Answer(500, "".into()),
            ],
            owner_and_port,
            0,
            6,
        ),
        case(
            "publish: air-gap on, no Connect call",
            &["publish", "--label", "acme"],
            &[("K2_AIRGAP", "1")],
            vec![ok(SERVED), ok(STATUS), ok(CHECKED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "publish: the serve fails",
            &["publish", "--label", "acme"],
            &[],
            vec![ok(r#"{"error":"down"}"#), ok(r#"{"error":"down"}"#)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "publish --json: the serve fails",
            &["publish", "--json"],
            &[],
            vec![ok(r#"{"error":"down"}"#), ok(r#"{"enabled":false}"#)],
            owner_and_port,
            1,
            2,
        ),
        traceback(case(
            "publish: the port reader raises",
            &["publish", "--label", "acme"],
            &[],
            vec![ok("[1]"), ok("{}")],
            owner_and_port,
            1,
            2,
        )),
        case(
            "publish: a reserved label",
            &["publish", "--label", " Chat"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "publish: a bad flag",
            &["publish", "--nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "publish in a session: the claim and Connect calls on the cell socket",
                &["publish", "--label", "acme"],
                &[],
                made_replies(),
                port_only,
                0,
                6,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `env-answer-large`: the index above what an environment
/// variable may hold. Bash stops at its Python ("Argument list too long";
/// exit 126, or 1 on macOS with its /bin/bash 3.2); the binary prints the
/// status. Both send the same two requests.
#[test]
fn wiki_status_with_a_large_answer() {
    let rig = Rig::new(VERB, "large");
    let big = leak(many(20_000));
    assert!(big.len() > 1024 * 1024, "above macOS's whole ARG_MAX too");
    let c = Case {
        no_python3: NoPython3::Own,
        ..case(
            "status, an index above 1 MiB",
            &["status"],
            &[],
            vec![ok(STATUS), ok(big)],
            owner_and_port,
            126,
            2,
        )
    };
    let mut logs = Vec::new();
    let mut outs = Vec::new();
    for (bash, side) in [(true, Side::Bash), (false, Side::Rust)] {
        rig.prepare(side, &c);
        let out = rig.run(bash, &c);
        let sent: Vec<String> = rig
            .read(side, !bash, c.name)
            .iter()
            .map(|r| {
                let head = String::from_utf8_lossy(&r.bytes).into_owned();
                head.lines().next().unwrap_or("").to_string()
            })
            .collect();
        logs.push(sent);
        outs.push(out);
    }
    let (b, r) = (&outs[0], &outs[1]);
    assert_eq!(logs[0].len(), 2, "bash's requests: {:?}", logs[0]);
    assert_eq!(
        logs[0]
            .iter()
            .map(|l| fold_query_escapes(l))
            .collect::<Vec<_>>(),
        logs[1]
            .iter()
            .map(|l| fold_query_escapes(l))
            .collect::<Vec<_>>(),
        "the requests"
    );
    let want = if cfg!(target_os = "macos") { 1 } else { 126 };
    assert_eq!(b.code, Some(want), "bash: {b:?}");
    assert_eq!(b.stdout, "");
    assert!(
        b.stderr.contains("Argument list too long"),
        "bash: {}",
        b.stderr
    );
    assert_eq!(r.code, Some(0), "the binary: {r:?}");
    assert_eq!(r.stderr, "");
    let notes = (0..20_000).filter(|i| i % 7 != 0).count();
    assert!(
        r.stdout
            .starts_with(&format!("Wiki  /srv/acme/.k2/wiki\n  Notes:      {notes}\n  Serving:    yes  http://127.0.0.1:4711\n")),
        "the binary: {}",
        r.stdout
    );
}
