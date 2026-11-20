//! `k5-cli inbox` against the shell CLI's `cli/k2 inbox`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code for every read form `cmd_inbox` has (`list`, no verb, a first word
//! that is no verb, `read`, `search`, `folders`).
//!
//! - The words: `--folder`, a word as the folder, flags skipped, the usage
//!   lines of `read` and `search`, extra words.
//! - The answers: an empty inbox, many items, unicode and long text,
//!   summaries, odd values (numbers, null, true for an id or a folder),
//!   items after which Python raises, not JSON, a refusal (format_inbox
//!   prints nothing, exit 0), `read`'s content, not found (exit 4),
//!   refusals with a permission card (the closing line), an empty answer,
//!   `folders` with none, some and a bad one.
//! - The owner from disk over TCP, a passport over TCP, a live cell socket
//!   (the answer's trailing newlines dropped; "not served", an empty
//!   answer and no answer go on to TCP), a socket file nothing listens on.
//! - `inbox` is an `id` tool: `HOOK_SOCK` with no passport, and a socket
//!   that is gone with a passport the probe finds dead or good.
//! - A re-minted, a healed and a dead passport, the bulletin, the gates,
//!   PROJECT_PATH and a PWD through a link.
//! - No answer (an empty reply, a 5xx with no body, no answer in time):
//!   the line, then death by SIGPIPE (141), one request on TCP; a reset,
//!   not HTTP, TMPDIR unusable (76) and no daemon (7): the reader on an
//!   empty input, and the call's code.
//! - The writes (`compose`, `respond`, `move`, `archive`, `delete`): the
//!   form each sends, its usage line, the answer and a newline; over TCP,
//!   over a live cell socket (sent once there: a close is the "may have
//!   been applied" line, never a second send; "not served" goes on to
//!   TCP), a socket file nothing listens on, no daemon, no answer, a
//!   refusal with its card, a value flag as the last word (the shell's),
//!   and help never acts.
//!   The writes have no census entry: each row's tag is `// native:
//!   <form>`, for the native-bar gate (a `// covers:` tag would fail the
//!   coverage gate).
//! - What the binary hands to the shell: `--help`, a `CODEX_` variable
//!   with `HOOK_SOCK` set, a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "inbox";
const NONE: &str = "[]";
const ITEMS: &str = r#"[{"id":"a1","folder":"projects","priority":"high","title":"Ship the fix","summary":"Read before standup"},{"id":"b2","filename":"notes.md","priority":"low"},{"id":"c3","folder":"","priority":"critical","title":null,"filename":"x.md"},{"id":5,"folder":7,"title":"numbers"},{"id":true,"folder":1.5,"priority":null}]"#;
const UNICODE: &str = r#"[{"id":"ü-1","folder":"日本","priority":"normal","title":"Grüße 🚀 \"quoted\" 'single' \\ back","summary":"línea\twith tab"}]"#;
const CONTENT: &str = r##"{"id":"a1","content":"# Title\n\nhello é 🚀\n  indented\n"}"##;
const NOT_FOUND: &str = r#"{"error":"inbox item not found: zzz"}"#;
const ROLE: &str = r#"{"role":"canonical"}"#;

fn many_items(n: usize) -> String {
    let items: Vec<String> = (0..n)
        .map(|i| {
            let p = ["high", "normal", "low", "critical", "other"][i % 5];
            format!(r#"{{"id":"item-{i:04}","folder":"f{}","priority":"{p}","title":"Title number {i} é","summary":"{}"}}"#, i % 7, if i % 3 == 0 { "sum" } else { "" })
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn long_items() -> String {
    format!(
        r#"[{{"id":"{}","folder":"{}","title":"{}","summary":"{}"}}]"#,
        "i".repeat(60),
        "f".repeat(40),
        "t".repeat(6000),
        "s é".repeat(1000)
    )
}

/// A session with a live cell socket.
fn cell(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    e
}

/// A host session: the passport, no socket.
fn host(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    with_passport(extra)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn words(v: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(v.into_boxed_slice())
}

/// What the shell's readers print on an empty input (macOS's bash 3.2
/// runs them after a request with no answer: [`Compare::Bash32PipeReader`]).
const NO_ITEMS: &str = "No items.\n";
const NO_FOLDERS: &str =
    "No folders yet. Create one by moving an item:\n  k2 inbox move <id> projects\n";
const READ_NO_ANSWER: &str = "Error: no answer from the K2 daemon (neither this session's cell socket nor the loopback port answered). Is K2 running? Try again: k2 inbox read <id>\n";

fn in_pipe(c: Case, stdout: &'static str, stderr: &'static str) -> Case {
    Case {
        compare: Compare::Bash32PipeReader { stdout, stderr },
        ..c
    }
}

/// `list` or `search` with no answer.
fn listed(c: Case) -> Case {
    in_pipe(c, NO_ITEMS, "")
}

fn reply(body: String) -> Reply {
    Reply::Answer(200, body)
}

#[test]
fn inbox_lists_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let h = host;
    let v = vec![
        case(
            "no verb: an empty inbox",
            &[],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        // covers: inbox
        case(
            "list",
            &["list"],
            &[],
            vec![ok(ITEMS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "unicode, quotes and a tab",
            &["list"],
            &[],
            vec![ok(UNICODE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many items",
            &["list"],
            &[],
            vec![reply(many_items(400))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "long text",
            &[],
            &[],
            vec![reply(long_items())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--folder",
            &["list", "--folder", "a b/é&x"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a word is the folder, the first one",
            &["list", "projects", "other"],
            &[],
            vec![ok(ITEMS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--folder after a word wins",
            &["list", "x", "--folder", "y"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty --folder, then a word",
            &["list", "--folder", "", "z"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "flags are skipped",
            &["list", "--json", "-x", "-"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a first word that is no verb is list's folder",
            &["projects"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a flag first is list's",
            &["--json", "zz"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--folder first",
            &["--folder", "f"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty first word",
            &["", "a"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a verb in another case is a folder",
            &["Read", "x"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty answer is no items",
            &[],
            &[],
            vec![ok("")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty object is no items",
            &[],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not JSON prints nothing",
            &[],
            &[],
            vec![ok("hello\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an item after which Python raises",
            &[],
            &[],
            vec![ok(r#"[{"id":"a"},{"id":null},{"id":"c"}]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an unhashable priority",
            &[],
            &[],
            vec![ok(r#"[{"id":"a"},{"id":"b","priority":["x"]}]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a list of strings prints nothing",
            &[],
            &[],
            vec![ok(r#"["a","b"]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal prints nothing, exit 0, no closing line",
            &[],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a 500 with a body",
            &[],
            &[],
            vec![Reply::Answer(500, r#"{"error":"boom"}"#.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bulletin line when it worked",
            &[],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            &[],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(ITEMS),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "search",
            &["search", "q &x é/?"],
            &[],
            vec![ok(ITEMS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "search takes the first word",
            &["search", "one", "two"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "search with no query",
            &["search"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "search with an empty query",
            &["search", ""],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "search, a long query",
            words(vec!["search", leak("ü ".repeat(1500))]),
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        Case {
            cwd: Some(link_to_proj(&rig)),
            ..case(
                "PWD through a link",
                &[],
                &[],
                vec![ok(NONE)],
                owner_and_port,
                0,
                1,
            )
        },
        case(
            "PROJECT_PATH",
            &[],
            &[("K2_PROJECT_PATH", "/srv/acme ws")],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        // A passport over TCP.
        with(
            h(&[]),
            case(
                "a host session: the passport over TCP",
                &["list"],
                &[],
                vec![ok(ITEMS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &[],
                &[],
                vec![ok(NONE)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &[],
                &[],
                vec![heal(&rig), ok(ITEMS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a dead passport: the probe, then the restart line",
                &[],
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
        with(
            h(&[]),
            case(
                "a refused passport the probe finds good: no line",
                &[],
                &[],
                vec![Reply::Answer(403, REFUSED.into()), ok(ROLE)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_NO_OWNER_FALLBACK", "1")]),
            case(
                "the guard lets a passport through",
                &[],
                &[],
                vec![ok(NONE)],
                port_only,
                0,
                1,
            ),
        ),
        // The gates.
        case("no port", &[], &[], vec![], owner_only, 1, 0),
        case("no token", &[], &[], vec![], port_only, 1, 0),
        case(
            "the gate comes before the words",
            &["search"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &[],
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
fn inbox_reads_an_item_as_the_shell_does() {
    let rig = Rig::new(VERB, "read");
    let v = vec![
        case(
            "an item",
            &["read", "a1"],
            &[],
            vec![ok(CONTENT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an id to escape, extra words",
            &["read", "a 1/é&x", "extra"],
            &[],
            vec![ok(CONTENT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an id like a flag",
            &["read", "--x"],
            &[],
            vec![ok(CONTENT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "long content",
            &["read", "a1"],
            &[],
            vec![reply(format!(
                r#"{{"content":"{}"}}"#,
                "line é\\n".repeat(3000)
            ))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no content is an empty line",
            &["read", "a1"],
            &[],
            vec![ok(r#"{"id":"a1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "content that is not text",
            &["read", "a1"],
            &[],
            vec![ok(r#"{"content":{"a":[1,null,true]}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not found: exit 4",
            &["read", "zzz"],
            &[],
            vec![Reply::Answer(400, NOT_FOUND.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "not found by its code",
            &["read", "zzz"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"gone"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "an error object: its hint",
            &["read", "a1"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"Not yours."}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object with neither",
            &["read", "a1"],
            &[],
            vec![ok(r#"{"error":{"x":1}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: the closing line",
            &["read", "a1"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer",
            &["read", "a1"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an answer of spaces",
            &["read", "a1"],
            &[],
            vec![ok(" \n\t ")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON: its repr",
            &["read", "a1"],
            &[],
            vec![ok("not 'json' \"x\"\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long",
            &["read", "a1"],
            &[],
            vec![reply("é".repeat(300))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not an object",
            &["read", "a1"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        ),
        case("no id", &["read"], &[], vec![], owner_and_port, 2, 0),
        case(
            "an empty id",
            &["read", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "folders",
            &["folders"],
            &[],
            vec![ok(r#"["projects","archive é"]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "folders, extra words",
            &["folders", "x", "--y"],
            &[],
            vec![ok(r#"["a"]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no folders",
            &["folders"],
            &[],
            vec![ok(NONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "folders not JSON",
            &["folders"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a folder that is not a string: exit 1",
            &["folders"],
            &[],
            vec![ok(r#"["a",1,"c"]"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "folders as an object: its keys",
            &["folders"],
            &[],
            vec![ok(r#"{"x":1,"y":2}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bulletin after an item",
            &["read", "a1"],
            &[],
            vec![ok(
                r#"{"content":"c","bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
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
fn inbox_in_a_session_as_the_shell_does() {
    let rig = Rig::new(VERB, "cell");
    let c = cell;
    let h = host;
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            c(&[]),
            case(
                "the cell socket",
                &[],
                &[],
                vec![ok(ITEMS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "the cell socket, a folder",
                &["list", "--folder", "a b"],
                &[],
                vec![ok(NONE)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "read over the cell socket",
                &["read", "a1"],
                &[],
                vec![ok(CONTENT)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "its trailing newlines go",
                &["read", "a1"],
                &[],
                vec![ok("notjson\n\n")],
                port_only,
                1,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "search and folders over it",
                &["search", "q"],
                &[],
                vec![ok(NONE)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "folders over it",
                &["folders"],
                &[],
                vec![ok(r#"["a"]"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "not served: TCP",
                &[],
                &[],
                vec![not_served(), ok(ITEMS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "an empty answer: TCP",
                &["read", "a1"],
                &[],
                vec![ok(""), ok(CONTENT)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "only newlines: TCP",
                &[],
                &[],
                vec![ok("\n\n"), ok(ITEMS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "no answer there: TCP",
                &[],
                &[],
                vec![Reply::Close, ok(ITEMS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "a reset there: TCP",
                &["read", "a1"],
                &[],
                vec![Reply::Reset, ok(CONTENT)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "not HTTP there: TCP",
                &[],
                &[],
                vec![Reply::Raw(GARBAGE.to_vec()), ok(ITEMS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "a refusal there is the answer",
                &["read", "a1"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                1,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "a refused passport heals there",
                &[],
                &[],
                vec![heal(&rig), ok(ITEMS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "the bulletin there",
                &["read", "a1"],
                &[],
                vec![ok(
                    r#"{"content":"c","bulletin":{"day":"2026-11-17","unread":true,"key":"cd34"}}"#,
                )],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: TCP",
                &[],
                &[],
                vec![ok(ITEMS)],
                port_only,
                0,
                1,
            ),
        ),
        // An id tool in a session.
        case(
            "HOOK_SOCK and no passport",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "HOOK_SOCK and no passport, before the usage line",
            &["read"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "the socket gone, the probe finds the passport dead",
                &[],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "the socket gone, the probe finds it good: TCP",
                &[],
                &[],
                vec![ok(ROLE), ok(ITEMS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "the socket gone and an owner on disk: still the passport",
                &[],
                &[],
                vec![ok(ROLE), ok(ITEMS)],
                owner_and_port,
                0,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The 0.46.15 no-resend test's no-answer kinds. A GET goes on from the
/// cell socket to TCP; on TCP each is one request.
#[test]
fn inbox_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let c = || cell(&[]);
    let empty_5xx = || Reply::Answer(502, String::new());
    let garbage = || Reply::Raw(GARBAGE.to_vec());
    let v = vec![
        listed(case(
            "an empty reply: the line, then SIGPIPE",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        listed(case(
            "a 5xx with no body: the line, then SIGPIPE",
            &["search", "q"],
            &[],
            vec![empty_5xx()],
            owner_and_port,
            141,
            1,
        )),
        in_pipe(
            case(
                "read: an empty reply",
                &["read", "a1"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
            "",
            READ_NO_ANSWER,
        ),
        in_pipe(
            case(
                "folders: an empty reply",
                &["folders"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
            NO_FOLDERS,
            "",
        ),
        case(
            "a reset (56): no items, exit 56",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "read, a reset: the reader's line, exit 1",
            &["read", "a1"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "folders, a reset: none, exit 56",
            &["folders"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "not HTTP (curl 1)",
            &[],
            &[],
            vec![garbage()],
            owner_and_port,
            1,
            1,
        ),
        case(
            "read, not HTTP",
            &["read", "a1"],
            &[],
            vec![garbage()],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset: 76",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            76,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            &[],
            &[],
            vec![ok(ITEMS)],
            owner_and_port,
            0,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an empty reply: 76, no line",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            76,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, not HTTP: 76",
            &["folders"],
            &[],
            vec![garbage()],
            owner_and_port,
            76,
            1,
        )),
        no_tmp(listed(case(
            "TMPDIR unusable, a 5xx with no body still says so",
            &[],
            &[],
            vec![empty_5xx()],
            owner_and_port,
            141,
            1,
        ))),
        no_tmp(with(
            c(),
            case(
                "TMPDIR unusable, a reset on the cell socket: TCP",
                &[],
                &[],
                vec![Reply::Reset, ok(ITEMS)],
                port_only,
                0,
                2,
            ),
        )),
        listed(with(
            c(),
            case(
                "the cell socket and TCP: an empty reply on both",
                &[],
                &[],
                vec![Reply::Close, Reply::Close],
                port_only,
                141,
                2,
            ),
        )),
        with(
            c(),
            case(
                "the cell socket and TCP: a reset on both",
                &[],
                &[],
                vec![Reply::Reset, Reply::Reset],
                port_only,
                56,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Nothing accepts the connection: exit 7 after the tries for `list`
/// and `folders` (their readers end 0), the reader's 1 for `read`.
#[test]
fn inbox_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    for (name, args, code) in [
        ("list", &[][..], 7),
        ("read", &["read", "a1"][..], 1),
        ("folders", &["folders"][..], 7),
    ] {
        let args = words(args.to_vec());
        rig.check(&case(
            leak(format!("nothing listens: {name}")),
            args,
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            code,
            0,
        ));
    }
    rig.check(&with(
        cell(&[("K2_HOOK_SOCK", "{deadsock}"), ("K2_PORT", port.as_str())]),
        case(
            "a dead socket and no daemon",
            &[],
            &[],
            vec![],
            nothing,
            7,
            0,
        ),
    ));
}

/// No answer within 30 s: the line, then SIGPIPE. Slow.
#[test]
fn inbox_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    let c = listed(case(
        "no answer in time (28)",
        &[],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        141,
        1,
    ));
    rig.check(&c);
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn inbox_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let c = cell;
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
        shell(case("-h", &["-h"], &[], vec![], owner_and_port, 0, 0)),
        shell(case(
            "read --help reads the item --help",
            &["read", "--help"],
            &[],
            vec![ok(CONTENT)],
            owner_and_port,
            0,
            1,
        )),
        shell(with(
            c(&[("CODEX_THREAD_ID", "t1")]),
            case(
                "a CODEX_ variable with HOOK_SOCK",
                &[],
                &[],
                vec![ok(ITEMS)],
                port_only,
                0,
                1,
            ),
        )),
        shell(with(
            c(&[("K2_PORT", "41o0")]),
            case(
                "a port that is no port, with a cell socket: refused first",
                &[],
                &[],
                vec![],
                nothing,
                2,
                0,
            ),
        )),
        shell(case(
            "a port that is no port over TCP: the shell's port refusal",
            &[],
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

const DONE: &str = r#"{"ok":true,"id":"n1"}"#;

/// The writes: their forms, usage lines and answers.
#[test]
fn inbox_writes_as_the_shell_does() {
    let rig = Rig::new(VERB, "write");
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        // native: inbox compose
        case(
            "compose",
            &[
                "compose",
                "--title",
                "Brief",
                "--body",
                "é & x = y\nline two",
                "--summary",
                "s",
                "--priority",
                "H",
                "--source",
                "acme",
                "--from",
                "ops",
                "stray",
                "-x",
            ],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "compose, the last of each flag",
            &["compose", "--title", "a", "--title", "b c", "--body", ""],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "compose with no title",
            &["compose", "--body", "x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "compose with an empty title",
            &["compose", "--title", ""],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "compose, a long body",
            words(vec![
                "compose",
                "--title",
                "long",
                "--body",
                leak("b é".repeat(30000)),
            ]),
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        ),
        // native: inbox respond
        case(
            "respond",
            &["respond", "a1", "thanks & more"],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "respond with no text",
            &["respond", "a1"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "respond with an empty id",
            &["respond", "", "x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // native: inbox move
        case(
            "move",
            &["move", "a1", "projects/q4", "extra"],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "move with no folder",
            &["move", "a1"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // native: inbox archive
        case(
            "archive",
            &["archive", "a1"],
            &[],
            vec![ok("{\"ok\":true}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "archive with no id",
            &["archive"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // native: inbox delete
        case(
            "delete",
            &["delete", "a1"],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "delete with no id",
            &["delete"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "delete, not found",
            &["delete", "zzz"],
            &[],
            vec![Reply::Answer(404, NOT_FOUND.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "archive, refused with a card",
            &["archive", "a1"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "archive, an empty answer",
            &["archive", "a1"],
            &[],
            vec![ok("")],
            owner_and_port,
            0,
            1,
        ),
        with(
            host(&[]),
            case(
                "compose, a passport over TCP",
                &["compose", "--title", "x"],
                &[],
                vec![ok(DONE)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "archive over the cell socket",
                &["archive", "a1"],
                &[],
                vec![ok("{\"ok\":true}\n\n")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "move: the cell socket doesn't serve it, then TCP",
                &["move", "a1", "f"],
                &[],
                vec![not_served(), ok(DONE)],
                port_only,
                0,
                2,
            ),
        ),
        Case {
            compare: Compare::Waited,
            ..with(
                cell(&[]),
                case(
                    "respond: the cell socket closes, never sent again",
                    &["respond", "a1", "x"],
                    &[],
                    vec![Reply::Close],
                    port_only,
                    1,
                    1,
                ),
            )
        },
        with(
            with_passport(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "delete: a socket nothing listens on, then TCP",
                &["delete", "a1"],
                &[],
                vec![ok(DONE)],
                port_only,
                0,
                1,
            ),
        ),
        Case {
            compare: Compare::Waited,
            ..case(
                "compose: TCP closes with nothing",
                &["compose", "--title", "x"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            )
        },
        case(
            "archive: no daemon",
            &["archive", "a1"],
            &[("K2_PORT", leak(dead_port()))],
            vec![],
            owner_only,
            7,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// A value flag of `compose` as the last word: the shell's (`$2: unbound
/// variable`).
#[test]
fn inbox_compose_value_flag_last_is_the_shell_s() {
    let rig = Rig::new(VERB, "wlast");
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "compose --body last",
            &["compose", "--title", "x", "--body"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )
    });
}

/// The writes with `-h` or `--help` anywhere: the help, and nothing sent.
#[test]
fn inbox_help_never_acts() {
    let rig = Rig::new(VERB, "whelp");
    const LINES: &[&[&str]] = &[
        &["compose", "--title", "x", "-h"],
        &["respond", "a1", "x", "--help"],
        &["move", "--help", "a1", "f"],
        &["archive", "a1", "-h"],
        &["delete", "-h", "a1"],
    ];
    for line in LINES {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case(
                "help, nothing sent",
                line,
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        });
    }
}
