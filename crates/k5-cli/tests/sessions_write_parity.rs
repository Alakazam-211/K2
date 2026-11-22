//! `k5-cli sessions open|new|stop|main set|live` against the shell CLI's
//! `cli/k2 sessions …`, on the parity rig ([`common::parity`]): the same
//! requests, stdout, stderr and exit code.
//!
//! - `open`: each outcome's lines (resumed, reopened, already live,
//!   asleep with and without `wakeWith`), every flag in the body, a brief
//!   from a file, from stdin and as text, `--json`, a refusal, a missing
//!   route, the cell socket in a session.
//! - `main set`: the lines with a previous chat and each Thread choice,
//!   `--json`, `--thread` spelled both ways.
//! - `new` and `stop`: the sidecar's lines and bodies under `sessions`.
//! - `live`: a folder, a name resolved, the table, `--count`, `--json`, an
//!   older daemon's fallback, a refusal, an answer that is not JSON, and
//!   one large list.
//! - The words: every usage line, a value flag as the last word (exit 1,
//!   nothing said, as the shell's `shift 2` under `set -e`), and help on
//!   each verb, which never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "sessions";

fn row(
    name: &'static str,
    args: &'static [&'static str],
    replies: Vec<Reply>,
    code: i32,
    requests: usize,
) -> Case {
    case(name, args, &[], replies, owner_and_port, code, requests)
}

fn with_brief(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("brief.md"), "the brief é\nline two\n").unwrap();
    std::fs::write(r.proj.join("bad.bin"), b"\xff\xfe not text").unwrap();
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

const OPENED: &str = r#"{"address":"acme/x","opened":true,"outcome":"resumed_row","cli":"claude"}"#;

#[test]
fn sessions_open_as_the_shell_does() {
    let rig = Rig::new(VERB, "open");
    let v = vec![
        // covers: sessions open
        row(
            "open: resumed its tab",
            &["open", "ab12"],
            vec![ok(OPENED)],
            0,
            1,
        ),
        row(
            "open: reopened, every flag",
            &[
                "open",
                "--name",
                "Named é",
                "ab12",
                "--harness",
                "codex",
                "--model",
                "m-1",
                "--force",
                "--workspace",
                "/w/acme",
                "--brief-text",
                "go \"now\"",
            ],
            vec![ok(r#"{"address":"acme/x","opened":1,"cli":null}"#)],
            0,
            1,
        ),
        row(
            "open: already live",
            &["open", "ab12", "--name", ""],
            vec![ok(r#"{"address":"acme/x","alreadyLive":true}"#)],
            0,
            1,
        ),
        row(
            "open: asleep with wakeWith",
            &["open", "ab12"],
            vec![ok(
                r#"{"address":"acme","wakeWith":"k2 msg acme \"wake\""}"#,
            )],
            0,
            1,
        ),
        row(
            "open: asleep, no wakeWith",
            &["open", "ab12"],
            vec![ok(r#"{"address":7}"#)],
            0,
            1,
        ),
        case(
            "open: a brief from a file",
            &["open", "ab12", "--brief", "brief.md"],
            &[],
            vec![ok(OPENED)],
            with_brief,
            0,
            1,
        ),
        Case {
            stdin: Some("from stdin é\n".as_bytes()),
            ..row(
                "open: a brief from stdin",
                &["open", "ab12", "--brief", "-"],
                vec![ok(OPENED)],
                0,
                1,
            )
        },
        // covers: sessions open --json
        row(
            "open --json",
            &["open", "ab12", "--json"],
            vec![ok(r#"{"address":"acme/x", "opened": true}"#)],
            0,
            1,
        ),
        row(
            "open: refused, with the closing line",
            &["open", "ab12"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
            1,
        ),
        row(
            "open: an older daemon",
            &["open", "ab12"],
            vec![Reply::Answer(404, r#"{"error":"route not found"}"#.into())],
            1,
            1,
        ),
        row(
            "open: a 404 that is an answer",
            &["open", "ab12"],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"no_such_chat","hint":"none"}}"#.into(),
            )],
            4,
            1,
        ),
        traceback(row(
            "open: an answer that is no object",
            &["open", "ab12"],
            vec![ok("[1]")],
            1,
            1,
        )),
        traceback(row(
            "open: opened with no address",
            &["open", "ab12"],
            vec![ok(r#"{"opened":true}"#)],
            1,
            1,
        )),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case(
                "open: in a session, on the cell socket",
                &["open", "ab12"],
                &[],
                vec![ok(OPENED)],
                nothing,
                0,
                1,
            )
        },
        // The words.
        row("open: no id", &["open"], vec![], 2, 0),
        row("open: two ids", &["open", "a", "b"], vec![], 2, 0),
        row(
            "open: an unknown flag",
            &["open", "a", "--bogus"],
            vec![],
            2,
            0,
        ),
        row(
            "open: a value flag last",
            &["open", "a", "--name"],
            vec![],
            2,
            0,
        ),
        row(
            "open: --brief and --brief-text",
            &["open", "a", "--brief", "x", "--brief-text", "y"],
            vec![],
            2,
            0,
        ),
        row(
            "open: a brief file that is not there",
            &["open", "a", "--brief", "nope.md"],
            vec![],
            2,
            0,
        ),
        case(
            "open: a brief that is not UTF-8",
            &["open", "a", "--brief", "bad.bin"],
            &[],
            vec![],
            with_brief,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

const SET: &str = r#"{"workspace":"acme","main":{"conversationId":"cd34","cli":"codex"},"title":"A title\nthat is long enough to be cut at sixty code points, é é é é","previous":"ab12ef56gh","thread":{"choice":"keep_main","other":{"address":"acme/old"}},"restartWith":"k2 sessions open cd34"}"#;

#[test]
fn sessions_main_set_as_the_shell_does() {
    let rig = Rig::new(VERB, "mainset");
    let v = vec![
        // covers: sessions main set
        row(
            "main set: keep the main Thread",
            &["main", "set", "cd34"],
            vec![ok(SET)],
            0,
            1,
        ),
        row(
            "main set: adopt the sidecar's Thread, --workspace",
            &[
                "main",
                "--thread",
                "adopt-sidecar",
                "set",
                "cd34",
                "--workspace",
                "/w/acme",
            ],
            vec![ok(
                r#"{"workspace":"acme","main":{"conversationId":"cd34","cli":"codex"},"previous":"cd34","thread":{"choice":"adopt_sidecar","other":{}},"restartWith":null}"#,
            )],
            0,
            1,
        ),
        row(
            "main set: a plain answer",
            &["main", "set", "cd34", "--thread", "keep_main"],
            vec![ok(r#"{}"#)],
            0,
            1,
        ),
        // covers: sessions main set --json
        row(
            "main set --json",
            &["main", "set", "cd34", "--json"],
            vec![ok(SET)],
            0,
            1,
        ),
        row(
            "main set: refused",
            &["main", "set", "cd34"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
            1,
        ),
        traceback(row(
            "main set: a previous that is a number",
            &["main", "set", "cd34"],
            vec![ok(r#"{"previous":12}"#)],
            1,
            1,
        )),
        row("main set: no id", &["main", "set"], vec![], 2, 0),
        row(
            "main set: a bad --thread",
            &["main", "set", "x", "--thread", "other"],
            vec![],
            2,
            0,
        ),
        row(
            "main set: a word too many",
            &["main", "set", "x", "y"],
            vec![],
            2,
            0,
        ),
        row(
            "main set: --workspace last",
            &["main", "set", "x", "--workspace"],
            vec![],
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn sessions_new_and_stop_as_the_shell_does() {
    let rig = Rig::new(VERB, "newstop");
    let made = r#"{"address":"acme/helper","harness":"claude","brief":{"path":".k2/brief.md","bytes":12}}"#;
    let v = vec![
        // covers: sessions new
        row(
            "new: the default harness",
            &["new", "helper"],
            vec![ok(made)],
            0,
            1,
        ),
        row(
            "new: --cli cursor, --model, --brief-text, --workspace",
            &[
                "new",
                "helper",
                "--cli",
                "cursor",
                "--model",
                "m",
                "--brief-text",
                "b",
                "--workspace",
                "/w",
            ],
            vec![ok(made)],
            0,
            1,
        ),
        row(
            "new: --harness and --json",
            &["new", "helper", "--harness", "pi", "--json"],
            vec![ok(made)],
            0,
            1,
        ),
        row(
            "new: refused",
            &["new", "helper"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
            1,
        ),
        row("new: no name", &["new"], vec![], 2, 0),
        row("new: two names", &["new", "a", "b"], vec![], 2, 0),
        row("new: --worktree", &["new", "a", "--worktree"], vec![], 2, 0),
        row(
            "new: --cli and --harness",
            &["new", "a", "--cli", "pi", "--harness", "pi"],
            vec![],
            2,
            0,
        ),
        row(
            "new: an unknown --cli",
            &["new", "a", "--cli", "vim"],
            vec![],
            2,
            0,
        ),
        row(
            "new: a value flag last",
            &["new", "a", "--model"],
            vec![],
            2,
            0,
        ),
        // covers: sessions stop
        row(
            "stop",
            &["stop", "helper", "--force"],
            vec![ok(r#"{"stopped":true,"address":"acme/helper"}"#)],
            0,
            1,
        ),
        row(
            "stop: asleep already, --json, --workspace",
            &["stop", "helper", "--json", "--workspace", "/w"],
            vec![ok(r#"{"stopped":false,"address":"acme/helper"}"#)],
            0,
            1,
        ),
        row("stop: no name", &["stop"], vec![], 2, 0),
        row(
            "stop: an unknown flag",
            &["stop", "a", "--all"],
            vec![],
            2,
            0,
        ),
        row(
            "stop: --workspace last",
            &["stop", "a", "--workspace"],
            vec![],
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

fn live_rows(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"sessionId":"5e55{i:032}","kind":"agent-long-kind","handle":"acme/h{i}","agentName":"agent-{i}-with-a-name-longer-than-24","command":"claude-code-cli","cwd":"/w/acme/sub{i}"}}"#
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

#[test]
fn sessions_live_as_the_shell_does() {
    let rig = Rig::new(VERB, "live");
    let v = vec![
        // covers: sessions live
        row(
            "live: a name resolved, the table",
            &["live", "acme"],
            vec![
                ok(r#"{"path":"/w/acme"}"#),
                ok(r#"[{"sessionId":"s1","kind":"agent","handle":"acme","agentName":"ada","command":"claude","cwd":"/w/acme"},{"terminalId":"t2"},7]"#),
            ],
            0,
            2,
        ),
        row(
            "live: a folder, none live",
            &["live", ".", "--bogus"],
            vec![ok("[]")],
            0,
            1,
        ),
        row(
            "live: --count, the resolve answer no object",
            &["live", "acme", "--count"],
            vec![ok("[1]"), ok(r#"[{"sessionId":"s1"},{"sessionId":"s2"}]"#)],
            0,
            2,
        ),
        // covers: sessions live --json
        row(
            "live --json: the answer as it came",
            &["live", "acme", "--json"],
            vec![ok("{}"), ok("[ {\"sessionId\": \"s1\"} ]\n\n")],
            0,
            2,
        ),
        row(
            "live: an older daemon's fallback, --json",
            &["live", "/w/acme", "--json"],
            vec![
                ok(r#"{"path":"/w/acme"}"#),
                Reply::Answer(404, r#"{"error":"route not found"}"#.into()),
                ok(r#"[{"terminalId":"t1","agentName":"ada","cwd":"/w/acme/x/"},{"terminalId":"t2","cwd":"/w/acmeX"},{"terminalId":"t3"}]"#),
            ],
            0,
            3,
        ),
        row(
            "live: an older daemon's fallback, the table",
            &["live", "/w/acme"],
            vec![
                ok(r#"{"path":"/w/acme/"}"#),
                Reply::Answer(404, r#"{"error":"route not found"}"#.into()),
                ok(r#"[{"terminalId":"t1","agentName":"ada","command":"pi","cwd":"/w/acme"}]"#),
            ],
            0,
            3,
        ),
        row(
            "live: refused, the owner's view",
            &["live", "acme"],
            vec![
                ok("{}"),
                Reply::Answer(403, r#"{"error":{"code":"owner_only","hint":"Only the owner can see raw sessions"}}"#.into()),
            ],
            3,
            2,
        ),
        row(
            "live: an error that is a string",
            &["live", "acme"],
            vec![ok("{}"), ok(r#"{"error":"boom"}"#)],
            1,
            2,
        ),
        row(
            "live: an answer that is not JSON",
            &["live", "acme"],
            vec![ok("{}"), ok("<html>it's \"down\"</html>")],
            1,
            2,
        ),
        row(
            "live: an answer of neither",
            &["live", "acme"],
            vec![ok("{}"), ok("42")],
            1,
            2,
        ),
        row(
            "live: a large list",
            &["live", "acme"],
            vec![ok("{}"), ok(&live_rows(2500))],
            0,
            2,
        ),
        row("live: no workspace", &["live"], vec![], 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: each verb that changes something, with `-h` or
/// `--help` anywhere, prints the shell's page and sends nothing.
#[test]
fn sessions_write_help_never_acts() {
    let rig = Rig::new(VERB, "help");
    for (name, args) in [
        ("open -h", &["open", "ab12", "-h"][..]),
        ("open --help first", &["open", "--help", "ab12"]),
        ("new --help", &["new", "helper", "--help"]),
        ("stop -h", &["stop", "helper", "-h"]),
        ("main set --help", &["main", "set", "cd34", "--help"]),
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..row(name, args, vec![], 0, 0)
        });
    }
}

/// `sessions live`, `archives`, `compact` and `set-label` with `-h` or
/// `--help` (0.46.17): the sessions help, nothing asked or read, past
/// both gates. Before, `live` read the flag as the workspace and asked to
/// resolve it, `compact` looked for its archive and `archives` listed
/// them.
#[test]
fn sessions_live_help_lines_print_the_help() {
    let rig = Rig::new(VERB, "livehelp");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    for c in [
        // native: sessions live --help
        row("live --help: the help", &["live", "--help"], vec![], 0, 0),
        row("live -h: the help", &["live", "-h"], vec![], 0, 0),
        shell(row(
            "live, a path, then -h: the help",
            &["live", "/w/acme", "-h"],
            vec![],
            0,
            0,
        )),
        // native: sessions archives --help
        row("archives --help", &["archives", "--help"], vec![], 0, 0),
        // native: sessions compact --help
        row("compact --help", &["compact", "--help"], vec![], 0, 0),
        shell(row(
            "compact, a session, then -h: the help",
            &["compact", "ab12", "-h"],
            vec![],
            0,
            0,
        )),
        shell(row(
            "set-label --help: the help",
            &["set-label", "ab12", "x", "--help"],
            vec![],
            0,
            0,
        )),
        case(
            "live --help, the virtual desktop's switch: the help",
            &["live", "--help"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            0,
            0,
        ),
    ] {
        rig.check(&c);
    }
}
