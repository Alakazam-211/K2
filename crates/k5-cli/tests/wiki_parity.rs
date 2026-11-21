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
    // A proxy set for the control plane is no longer the shell's: the
    // binary reads it (wiki_publish_through_the_proxy). Here the daemon
    // makes the Connect call, so the proxy is never needed.
    v.push(case(
        "publish: a tunnel token on disk and a proxy set, the daemon makes the call",
        &["publish", "--label", "acme"],
        &[("https_proxy", "http://127.0.0.1:9")],
        made_replies(),
        tunnel_token,
        0,
        6,
    ));
    v.push(shell(case(
        "a port that is no port: the shell's port refusal",
        &["status"],
        &[("K2_PORT", "41o0")],
        vec![],
        owner_only,
        2,
        0,
    )));
    for c in v {
        rig.check(&c);
    }
}

fn tunnel_token(r: &Rig) {
    owner_and_port(r);
    std::fs::write(
        r.k2home.join("tunnel.json"),
        r#"{"subdomain":"acme","token":"tun-tok"}"#,
    )
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

/// A control plane on loopback: answers each request 201 with `body`,
/// and keeps each request's bytes.
fn control_plane(body: &'static str) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let keep = std::sync::Arc::clone(&seen);
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let mut c = conn.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = c.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let t = String::from_utf8_lossy(&buf).into_owned();
                if let Some((head, rest)) = t.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|h| h.strip_prefix("Content-Length: "))
                        .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                    if rest.len() >= len {
                        break;
                    }
                }
            }
            keep.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf).into_owned());
            let reply = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            c.write_all(reply.as_bytes()).unwrap();
        }
    });
    (port, seen)
}

/// No daemon route (404 from /cli/connect/proxy): `_connect_curl` calls the
/// control plane itself with `tunnel.json`'s token. Both CLIs send it the
/// same request (the User-Agent aside), the token only as the
/// Authorization header, and print the same.
#[test]
fn wiki_publish_calls_the_control_plane_itself() {
    let rig = Rig::new(VERB, "cplane");
    let (port, seen) =
        control_plane(r#"{"label":"acme","target":"127.0.0.1:4711","status":"active"}"#);
    let base = leak(format!("http://127.0.0.1:{port}"));
    rig.check(&case(
        "publish: no proxy route, the control plane with the tunnel token",
        &["publish", "--label", "acme", "--json"],
        &[("K2_CONNECT_BASE", base)],
        vec![
            ok(SERVED),
            ok(STATUS),
            ok(CHECKED),
            Reply::Answer(404, "not found".into()),
            ok(CHECKED),
            ok("{}"),
        ],
        tunnel_token,
        0,
        6,
    ));
    let got = seen.lock().unwrap().clone();
    // The shell's, the binary's, and the binary's with no python3.
    assert!(got.len() >= 2, "one request from each run: {got:?}");
    let masked: Vec<String> = got
        .iter()
        .map(|r| {
            r.split("\r\n")
                .map(|l| {
                    if l.starts_with("User-Agent: ") {
                        "User-Agent: <ua>"
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
                .join("\r\n")
        })
        .collect();
    for m in &masked[1..] {
        assert_eq!(&masked[0], m, "the shell's, then the binary's");
    }
    for r in &got {
        assert!(r.starts_with("POST /subdomains HTTP/1.1\r\n"), "{r}");
        let with_token: Vec<&str> = r.split("\r\n").filter(|l| l.contains("tun-tok")).collect();
        assert_eq!(with_token, ["Authorization: Bearer tun-tok"], "{r}");
        assert!(!r.contains("X-K5-"), "{r}");
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

/// The control plane's answer to a create, through the tunnel.
const CREATED: &str = r#"{"label":"acme","target":"127.0.0.1:4711","status":"active"}"#;
const PROXY_USER: &str = "acme-proxy";
const PROXY_PASS: &str = "pr0xy-s3cret";
const TUN_TOK: &str = "tun-tok";

/// No daemon to make the Connect call (404 from /cli/connect/proxy):
/// serve on, its status, the claim check, the create through the
/// daemon (404), then the stamp and the nudge after a create the
/// control plane made.
fn cp_made() -> Vec<Reply> {
    vec![
        ok(SERVED),
        ok(STATUS),
        ok(CHECKED),
        Reply::Answer(404, "not found".into()),
        ok(CHECKED),
        ok("{}"),
    ]
}

/// The same when the control plane can't be reached: the create and the
/// point both go to the daemon (404) and on to the control plane.
fn cp_failed() -> Vec<Reply> {
    vec![
        ok(SERVED),
        ok(STATUS),
        ok(CHECKED),
        Reply::Answer(404, "not found".into()),
        Reply::Answer(404, "not found".into()),
    ]
}

fn cp_case(
    name: &'static str,
    env: Vec<(&'static str, String)>,
    replies: Vec<Reply>,
    requests: usize,
) -> Case {
    Case {
        env,
        ..case(
            name,
            &["publish", "--label", "acme"],
            &[],
            replies,
            tunnel_token,
            0,
            requests,
        )
    }
}

/// `wiki publish` calls the control plane itself through the person's
/// proxy (K2's ruling on the shared transport): a throwaway CA trusted
/// through SSL_CERT_FILE on both sides, a recording proxy, and the
/// control plane as a TLS stub under a name that never resolves.
#[test]
fn wiki_publish_through_the_proxy() {
    use common::tunnel::{self, ProxyMode, ProxyStub, TlsStub, HOST};
    let rig = Rig::new(VERB, "proxy");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let cp = TlsStub::start(&pki, 201, CREATED);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let target = format!("{HOST}:{}", cp.port);
    let base = format!("https://{target}");
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let with = |extra: &[(&'static str, String)]| {
        let mut env = vec![
            ("K2_CONNECT_BASE", base.clone()),
            ("SSL_CERT_FILE", ca.clone()),
        ];
        env.extend(extra.iter().cloned());
        env
    };
    let authed = proxy.url(Some((PROXY_USER, PROXY_PASS)));
    let basic = tunnel::basic_line(PROXY_USER, PROXY_PASS);

    // https_proxy with a user and password: Basic in the CONNECT, the
    // token only inside the tunnel.
    let c = cp_case(
        "https_proxy with user:pass",
        with(&[("https_proxy", authed.clone())]),
        cp_made(),
        6,
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &cp);
    // The binary's side first: the bite (a stand-in that puts the token
    // in the CONNECT head) must fail right here.
    for run in [&r, &b] {
        tunnel::assert_tunnel_only(c.name, run, &target, Some(&basic), TUN_TOK);
        assert_eq!(run.tls.len(), 1, "one create: {run:?}");
    }
    tunnel::assert_same(c.name, &b, &r);
    assert_eq!(r.out.code, Some(0), "{r:?}");
    assert!(r.out.stdout.contains("acme"), "{r:?}");
    tunnel::assert_unprinted(c.name, &[b, r], &[PROXY_PASS, TUN_TOK]);

    // HTTPS_PROXY with no user part: no Proxy-Authorization.
    let c = cp_case(
        "HTTPS_PROXY, no user",
        with(&[("HTTPS_PROXY", proxy.url(None))]),
        cp_made(),
        6,
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &cp);
    tunnel::assert_same(c.name, &b, &r);
    for run in [&b, &r] {
        tunnel::assert_tunnel_only(c.name, run, &target, None, TUN_TOK);
    }

    // Every proxy variable set: the daemon's calls (loopback) never reach
    // the proxy; only the control plane's CONNECT does.
    let every: Vec<(&'static str, String)> = [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ]
    .iter()
    .map(|n| (*n, authed.clone()))
    .collect();
    let c = cp_case("every proxy variable set", with(&every), cp_made(), 6);
    let [b, r] = tunnel::both(&rig, &c, &proxy, &cp);
    tunnel::assert_same(c.name, &b, &r);
    for run in [&b, &r] {
        assert_eq!(run.daemon.len(), 6, "the daemon got every call: {run:?}");
        tunnel::assert_tunnel_only(c.name, run, &target, Some(&basic), TUN_TOK);
        assert_eq!(
            run.proxy.len(),
            1,
            "only the control plane's CONNECT: {run:?}"
        );
    }

    // Direct: no_proxy naming the host, NO_PROXY=*, and HTTP_PROXY alone
    // (never read). The name never resolves, so the control plane is
    // quiet on both sides, and the proxy sees nothing.
    for (name, extra) in [
        (
            "no_proxy names the host",
            vec![
                ("https_proxy", authed.clone()),
                ("no_proxy", HOST.to_string()),
            ],
        ),
        (
            "no_proxy names its domain",
            vec![
                ("https_proxy", authed.clone()),
                ("no_proxy", "other.example,.example.invalid".to_string()),
            ],
        ),
        (
            "NO_PROXY=*",
            vec![
                ("HTTPS_PROXY", authed.clone()),
                ("NO_PROXY", "*".to_string()),
            ],
        ),
        ("HTTP_PROXY alone", vec![("HTTP_PROXY", authed.clone())]),
    ] {
        let c = cp_case(name, with(&extra), cp_failed(), 5);
        let [b, r] = tunnel::both(&rig, &c, &proxy, &cp);
        tunnel::assert_same(name, &b, &r);
        for run in [&b, &r] {
            assert_eq!(run.proxy, Vec::<String>::new(), "{name}: {run:?}");
            assert!(run.tls.is_empty(), "{name}: {run:?}");
            assert_eq!(run.daemon.len(), 5, "{name}: {run:?}");
        }
        tunnel::assert_unprinted(name, &[b, r], &[PROXY_PASS, TUN_TOK]);
    }

    // A 407, and a proxy that is down: quiet on both sides, the password
    // printed nowhere, nothing in the tunnel.
    let refusing = ProxyStub::start(ProxyMode::Refuse);
    let dead = format!(
        "http://{PROXY_USER}:{PROXY_PASS}@127.0.0.1:{}",
        dead_tcp_port()
    );
    for (name, url, p) in [
        (
            "the proxy answers 407",
            refusing.url(Some((PROXY_USER, PROXY_PASS))),
            &refusing,
        ),
        ("the proxy is down", dead, &proxy),
    ] {
        let c = cp_case(name, with(&[("https_proxy", url)]), cp_failed(), 5);
        let [b, r] = tunnel::both(&rig, &c, p, &cp);
        if p.port == refusing.port {
            assert_eq!(r.proxy.len(), 2, "{name}: one CONNECT per call: {r:?}");
            tunnel::assert_same_retried(name, &b, &r);
        } else {
            tunnel::assert_same(name, &b, &r);
        }
        for run in [&b, &r] {
            assert!(run.tls.is_empty(), "{name}: {run:?}");
            assert_eq!(run.daemon.len(), 5, "{name}: {run:?}");
            for h in &run.proxy {
                assert!(!h.contains(TUN_TOK), "{name}: {h:?}");
            }
        }
        tunnel::assert_unprinted(name, &[b, r], &[PROXY_PASS, TUN_TOK]);
    }
}

/// A port nothing listens on.
fn dead_tcp_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// K2's rulings on `K2_CONNECT_BASE`, checked as publish starts: no
/// scheme, or plain http to a host that is not this computer, is one
/// line and exit 2, with nothing sent. The bash side flips by itself
/// once cli/k2 carries the fix (pending-bash-fix until then: the
/// binary's side alone is checked, and bash must not print the line
/// half-fixed).
#[test]
fn wiki_publish_refuses_a_base_that_would_leak_the_token() {
    let rig = Rig::new(VERB, "base");
    for (name, base, line) in [
        (
            "a base with no scheme",
            "cp.example.invalid",
            "Error: K2_CONNECT_BASE must start with https:// (got: cp.example.invalid)",
        ),
        (
            "plain http to another machine",
            "http://cp.example.invalid:9",
            "Error: plain http:// only to this computer; use https:// (got: http://cp.example.invalid:9)",
        ),
        // Loopback is the URL as written: a name under localhost is not.
        (
            "plain http to x.localhost",
            "http://x.localhost:9",
            "Error: plain http:// only to this computer; use https:// (got: http://x.localhost:9)",
        ),
    ] {
        let c = Case {
            env: vec![("K2_CONNECT_BASE", base.to_string())],
            ..case(
                name,
                &["publish", "--label", "acme"],
                &[],
                vec![],
                tunnel_token,
                2,
                0,
            )
        };
        let fixed = common::tunnel::bash_has(line.split(" (got").next().unwrap());
        if fixed {
            rig.check(&c);
            continue;
        }
        println!("PENDING bash fix: {name}: the binary's side alone");
        rig.prepare(Side::Rust, &c);
        let out = rig.run(false, &c);
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (Some(2), "", format!("{line}\n").as_str()),
            "{name}"
        );
        assert_eq!(rig.read(Side::Rust, true, name).len(), 0, "{name}: sent");
        rig.prepare(Side::Bash, &c);
        let out = rig.run(true, &c);
        assert!(!out.stderr.contains(line), "{name}: bash half-fixed: {out:?}");
        rig.read(Side::Bash, false, name);
    }
}
