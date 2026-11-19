//! `k5-cli feedback list` and `k5-cli feedback show` against the shell
//! CLI's `cli/k2 feedback list|show` (and `tickets`, the same command), on
//! the parity rig ([`common::parity`]): the same requests, stdout, stderr
//! and exit code for every local path of the two.
//!
//! - The words: `--all`, `--status <s>` (each status, an empty one, a bad
//!   one, none after it), `--json`, an unknown flag, an extra word, words
//!   that are not ASCII.
//! - The answers: rows of every shape (long titles, unicode, numbers,
//!   `null`s, `true`), none, no list, many rows; `--json`; the daemon's
//!   error (an object with each code, text, a number, `null`), a refusal
//!   with a permission card (the closing line), not JSON (short, long,
//!   empty), a NUL, not an object, Python's raises, the bulletin,
//!   migrating, trailing newlines.
//! - The owner from disk, a passport over TCP (the workspace from
//!   `/cli/whoami`, on the cell socket or TCP), re-minted, healed, dead;
//!   `HOOK_SOCK` with the owner on disk (PRD 13.9 #4); PROJECT_PATH; the
//!   gates; the passport rule's would-refuse mark.
//! - No answer: an empty reply, a 5xx with no body and none in time are
//!   the line and exit 1; a reset (56), not HTTP (000), TMPDIR unusable
//!   (75), an empty 200 and no daemon (7) are the `daemon_unreachable`
//!   line and exit 1.
//! - What the binary hands to the shell: `feedback` alone, help, another
//!   verb, a port that is no port.
//!
//! `show`, fetched through `cli_request` as `list` is: the ticket with
//! every field (a brief, its size each way, 40 and 45 lines of its text,
//! the thread with each message's files), `--json`, `--html`, `--html
//! --json`, no brief; odd values, a long body, bytes that are not UTF-8,
//! an id that needs escaping; the daemon's error (each code, text, a
//! number, a permission card, migrating then the answer); not JSON, a
//! NUL, not an object; Python's raises; the words; a passport over TCP
//! only (re-minted; refused: one heal; dead: the restart line), PRD 13.9 #4;
//! no answer (an empty reply and an empty 5xx: the transport's line; a
//! reset, not HTTP, TMPDIR unusable, none in time, no daemon:
//! `daemon_unreachable`, as `list`'s); and what it hands to the shell (help). A port with a leading zero is the
//! binary's own, as `list`'s is.
//!
//! Neither verb sends a request on the left of a pipe (`list` holds its
//! answer in `$(…)`, and so does `show`), so no case dies of
//! SIGPIPE; every case runs with stdout and stderr on pipes.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "feedback";
const LIST: &str = r#"{"items":[
  {"id":"fb_0123456789","status":"waiting","priority":2,"kind":"question","title":"Ship the release? é 日本"},
  {"id":"fb_x","status":"needs_discussion","priority":10,"kind":"decision","title":"LONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONG"},
  {"id":12345678901,"status":null,"priority":1.5,"kind":true,"title":["a",1]},
  {"status":"answered"},
  {}]}"#;
const PROJ_WS: &str = r#"{"workspace":"acme","path":"/srv/acme ws é"}"#;

fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"fb_{i:06}","status":"waiting","priority":{},"kind":"question","title":"Row {i} {}"}}"#,
                i % 4,
                "é".repeat(i % 30)
            )
        })
        .collect();
    format!(r#"{{"items":[{}]}}"#, rows.join(","))
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// A NUL in the answer: `scripts/cli-parity.sh`'s named difference
/// `nul-in-answer` ([`Compare::NulInAnswer`]). Bash's `$(cli_request …)`
/// drops it with a warning naming the script's path and line (bash 4.4
/// and later), the binary silently; all else is exact.
fn nul_in_answer(c: Case) -> Case {
    Case {
        compare: Compare::NulInAnswer,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn no_tmp(c: Case) -> Case {
    Case {
        no_tmpdir: true,
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

#[test]
fn list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let long_text = format!("not json {}", "x".repeat(400));
    let many = many(300);
    let v = vec![
        case("list", &["list"], &[], vec![ok(LIST)], owner_and_port, 0, 1),
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
            "--all",
            &["list", "--all"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--status planned",
            &["list", "--status", "planned", "--all"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "each status",
            &[
                "list",
                "--status",
                "needs_discussion",
                "--status",
                "dismissed",
            ],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty status is none",
            &["list", "--status", "", "--all", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no rows",
            &["list"],
            &[],
            vec![ok(r#"{"items":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no list",
            &["list"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many rows",
            &["list"],
            &[],
            vec![ok(&many)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many rows, --json",
            &["list", "--json"],
            &[],
            vec![ok(&many)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json keeps odd values",
            &["list", "--json"],
            &[],
            vec![ok(
                r#"{"items":[{"id":1e16,"t":NaN,"u":"😀 \u0000"}],"items":[],"n":-0,"big":123456789012345678901234567890}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trailing newlines go",
            &["list"],
            &[],
            vec![ok("{\"items\":[{\"id\":\"a\"}]}\n\n\n")],
            owner_and_port,
            0,
            1,
        ),
        // The daemon's error.
        case(
            "not_found: exit 4",
            &["list"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"nothing é"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "ambiguous_id: exit 4",
            &["list"],
            &[],
            vec![ok(r#"{"error":{"code":"ambiguous_id","hint":"a, b"}}"#)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "usage: exit 2",
            &["list"],
            &[],
            vec![ok(r#"{"error":{"code":"usage","hint":"no"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "another code: exit 1",
            &["list", "--json"],
            &[],
            vec![ok(r#"{"error":{"code":["usage"],"x":1}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error as text",
            &["list"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"it broke — badly"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error of null is none",
            &["list"],
            &[],
            vec![ok(r#"{"error":null,"items":[{"id":"x"}]}"#)],
            owner_and_port,
            0,
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
            "the bulletin",
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
        // Not JSON.
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("nope é")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long",
            &["list"],
            &[],
            vec![ok(&long_text)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, quotes",
            &["list"],
            &[],
            vec![ok("it's \"x\"\t\u{1}")],
            owner_and_port,
            1,
            1,
        ),
        nul_in_answer(case(
            "a NUL in a string",
            &["list"],
            &[],
            vec![ok("{\"items\":[{\"id\":\"x\",\"title\":\"a\u{0}b\"}]}")],
            owner_and_port,
            0,
            1,
        )),
        case(
            "an empty 200",
            &["list"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "only newlines",
            &["list"],
            &[],
            vec![ok("\n\n")],
            owner_and_port,
            1,
            1,
        ),
        // Python's raises.
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
            "an error that is a number",
            &["list"],
            &[],
            vec![ok(r#"{"error":3}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "items of null",
            &["list"],
            &[],
            vec![ok(r#"{"items":null}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a row that is no object, part way",
            &["list"],
            &[],
            vec![ok(r#"{"items":[{"id":"a"},"x"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "items as an empty object: no rows",
            &["list"],
            &[],
            vec![ok(r#"{"items":{}}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "items as an object: its keys, then a raise",
            &["list"],
            &[],
            vec![ok(r#"{"items":{"k":1}}"#)],
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
        case("a dash", &["list", "-"], &[], vec![], owner_and_port, 2, 0),
        case(
            "an extra word",
            &["list", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an empty word",
            &["list", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--status alone",
            &["list", "--status"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad status",
            &["list", "--status", "nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad status after a bad flag",
            &["list", "--status", "nope", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a word that is not ASCII",
            &["list", "日本 é"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "PROJECT_PATH",
            &["list"],
            &[("K2_PROJECT_PATH", "/srv/acme ws é")],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "in a session by its markers: the would-refuse mark",
            &["list"],
            &[("K2_CELL", "canonical")],
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
            &["list", "--bogus"],
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
fn list_with_a_passport() {
    let rig = Rig::new(VERB, "lpass");
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(rig.stub.port, &[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let v = vec![
        with(
            h(&[]),
            case(
                "whoami's path is the workspace",
                &["list"],
                &[],
                vec![ok(PROJ_WS), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "whoami not JSON",
                &["list", "--all"],
                &[],
                vec![ok("nope"), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_PROJECT_PATH", "/set")]),
            case(
                "PROJECT_PATH: no whoami",
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
                vec![ok(PROJ_WS), ok(LIST)],
                healed_on_disk,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_PROJECT_PATH", "/set")]),
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
            h(&[("K2_PROJECT_PATH", "/set")]),
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
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                cell(&[("K2_PROJECT_PATH", "/set")]),
                case(
                    "HOOK_SOCK and the owner on disk (PRD 13.9 #4)",
                    &["list"],
                    &[],
                    vec![ok(LIST)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
        with(
            cell(&[]),
            case(
                "the cell socket for whoami, TCP for the list",
                &["list"],
                &[],
                vec![ok(PROJ_WS), ok(LIST)],
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
                vec![ok(PROJ_WS), ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: an open tool, the owner",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![ok(LIST)],
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
fn list_with_no_answer() {
    let rig = Rig::new(VERB, "lnone");
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
            "a reset (56): daemon_unreachable",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP (000): daemon_unreachable",
            &["list"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset (76): daemon_unreachable",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
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
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn list_with_no_daemon() {
    let rig = Rig::new(VERB, "ldead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens (7): daemon_unreachable",
        &["list"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// No answer within 30 s: the line, exit 1. Slow.
#[test]
fn list_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "lhang");
    rig.check(&waited(case(
        "no answer in time (28)",
        &["list"],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        1,
        1,
    )));
}

/// `tickets` is the same command.
#[test]
fn tickets_list_is_feedback_list() {
    let rig = Rig::new("tickets", "list");
    let v = vec![
        case(
            "tickets list",
            &["list"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tickets list --status waiting --json",
            &["list", "--status", "waiting", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tickets list x",
            &["list", "x"],
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

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn feedback_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let v = vec![
        shell(case(
            "feedback alone",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
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
            "list --status --help",
            &["list", "--status", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "template",
            &["template"],
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
            1,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

// ---------------------------------------------------------------------
// `show`

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_secs()
}

/// A whole ticket, its times a little before now (in the middle of each
/// of `ago`'s steps, so both sides print the same).
fn full() -> String {
    let n = now();
    format!(
        r#"{{"id":"fb_12345678zz","status":"answered","title":"Pick one é 日本","kind":"question","priority":1,
  "agentName":"acme","sessionId":"sess-abcdefghij","createdAt":{c},"assignees":["ann","bob"],
  "options":["A — yes","B"],"answer":"B","body":"Line one\r\nLine two three","hasBrief":true,"briefBytes":1280,
  "brief":{{"html":"<h2>Problem</h2>\n<p>é</p>\n","text":"Brief text\nline 2","bytes":1280}},"projectPath":"/srv/acme",
  "comments":[
    {{"author":"ann","at":{a},"body":"ok\nsecond","attachments":[{{"path":"a.png","size":2048}},
      {{"path":"/abs/b","size":"12","state":"changed","line":3,"lineEnd":9}},{{"state":"hidden"}},"skip",
      {{"path":"c","state":"gone","line":4}},{{"path":"big","size":2359296}}]}},
    {{"author":"a-very-long-author","at":{b},"body":""}},
    {{"at":{d}}}]}}"#,
        c = n - 330,
        a = n - 5400,
        b = n - 200_000,
        d = n + 100,
    )
}

const PLAIN: &str = r#"{"id":"plain123456","status":"waiting","title":"No brief","kind":"note","priority":3,"createdAt":0}"#;

fn brief_text(lines: usize) -> String {
    let rows: Vec<String> = (0..lines).map(|i| format!("line {i} é")).collect();
    format!(
        r#"{{"id":"b1","hasBrief":true,"briefBytes":3584,"brief":{{"text":"{}","html":"<p>x</p>"}}}}"#,
        rows.join("\\n")
    )
}

/// The HTTP answer as bytes, for a body that is not UTF-8.
fn raw_answer(body: &[u8]) -> Reply {
    let mut r = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    r.extend_from_slice(body);
    Reply::Raw(r)
}

#[test]
fn show_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "show");
    let full = full();
    let long_text = format!("not json {}", "y".repeat(400));
    let forty = brief_text(40);
    let many = brief_text(45);
    let long_body = format!(r#"{{"id":"x","body":"{}"}}"#, "word ".repeat(2000));
    let v = vec![
        case(
            "show",
            &["show", "fb_1"],
            &[],
            vec![ok(&full)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json: no brief",
            &["show", "fb_1", "--json"],
            &[],
            vec![ok(&full)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--html: the HTML alone",
            &["show", "--html", "fb_1"],
            &[],
            vec![ok(&full)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--html --json: the brief too",
            &["show", "fb_1", "--html", "--json"],
            &[],
            vec![ok(&full)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no brief",
            &["show", "p"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--html, no brief: no_brief",
            &["show", "p", "--html"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--html, a brief with no HTML",
            &["show", "p", "--html", "--json"],
            &[],
            vec![ok(r#"{"id":"x","brief":{"html":5}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "40 lines of brief",
            &["show", "b1"],
            &[],
            vec![ok(&forty)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "45 lines of brief: the rest counted",
            &["show", "b1"],
            &[],
            vec![ok(&many)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the brief's size, each way",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"hasBrief":true,"briefBytes":null,"brief":{"bytes":99}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a size of true, MB, a float, text",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"hasBrief":1,"briefBytes":true,"comments":[{"attachments":[{"path":"a","size":2359296},{"path":"b","size":1536.9},{"path":"c","size":" 1_024 "},{"path":"d","size":-5}]}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a brief size that is not a number",
            &["show", "x"],
            &[],
            vec![ok(r#"{"hasBrief":true,"briefBytes":"12"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd values",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"id":123456789012,"status":null,"title":{"a":1},"kind":true,"priority":1.5,"agentName":"","sessionId":987654321098,
              "assignees":"ab","options":{"k":1,"m":2},"answer":0,"body":5,"createdAt":false,"comments":{}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a list answer and an empty body",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"answer":[1,"two"],"body":"\n","assignees":[],"options":[]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "files: the project folder with a slash, none",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"projectPath":"/srv/","comments":[{"author":"x","body":"a","attachments":[{"path":"f"},{"path":"/g","line":"7"}]},{"attachments":[{"path":"h"}]}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a long body",
            &["show", "x"],
            &[],
            vec![ok(&long_body)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an id with spaces and more",
            &["show", "a b/é+&=?#"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty id, then one",
            &["show", "", "ab"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bytes that are not UTF-8",
            &["show", "x"],
            &[],
            vec![raw_answer(
                b"{\"title\":\"a\xffb\xe2\x82\",\"body\":\"\xc3\"}",
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bulletin is not noted",
            &["show", "x"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        // The daemon's error.
        case(
            "not_found: exit 4",
            &["show", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no ticket é"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "not_found with --html: exit 4",
            &["show", "nope", "--html"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no ticket"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "ambiguous_id: exit 4",
            &["show", "fb"],
            &[],
            vec![ok(
                r#"{"error":{"code":"ambiguous_id","hint":"fb_1, fb_2"}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "usage: exit 2",
            &["show", "x"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"usage","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error as text",
            &["show", "x"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"it broke"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: no closing line",
            &["show", "x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "migrating, then the answer",
            &["show", "x"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(PLAIN),
            ],
            owner_and_port,
            0,
            2,
        ),
        // Not JSON.
        case(
            "not JSON",
            &["show", "x"],
            &[],
            vec![ok("nope é")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long",
            &["show", "x"],
            &[],
            vec![ok(&long_text)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty 200",
            &["show", "x"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        nul_in_answer(case(
            "a NUL in a string",
            &["show", "x"],
            &[],
            vec![ok("{\"title\":\"a\u{0}b\"}")],
            owner_and_port,
            0,
            1,
        )),
        case(
            "not an object",
            &["show", "x"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a string",
            &["show", "x"],
            &[],
            vec![ok(r#""str""#)],
            owner_and_port,
            1,
            1,
        ),
        // Python's raises.
        traceback(case(
            "an error that is a number",
            &["show", "x"],
            &[],
            vec![ok(r#"{"error":3}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a time that is text",
            &["show", "x"],
            &[],
            vec![ok(r#"{"id":"x","createdAt":"yesterday"}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a time of null",
            &["show", "x"],
            &[],
            vec![ok(r#"{"createdAt":null}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an assignee that is a number",
            &["show", "x"],
            &[],
            vec![ok(r#"{"assignees":["a",1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "options of a number",
            &["show", "x"],
            &[],
            vec![ok(r#"{"options":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a brief size of NaN",
            &["show", "x"],
            &[],
            vec![ok(r#"{"hasBrief":true,"briefBytes":NaN}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "comments of null",
            &["show", "x"],
            &[],
            vec![ok(r#"{"comments":null}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a comment that is text, part way",
            &["show", "x"],
            &[],
            vec![ok(r#"{"comments":[{"author":"a","body":"b"},"x"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a comment's time that is text",
            &["show", "x"],
            &[],
            vec![ok(r#"{"comments":[{"at":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "attachments of a number",
            &["show", "x"],
            &[],
            vec![ok(r#"{"comments":[{"attachments":7}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a project folder that is a number",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"projectPath":5,"comments":[{"attachments":[{"path":"a"}]}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a state that is a list",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"comments":[{"attachments":[{"path":"a","state":["x"]}]}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a size that is not a number",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"comments":[{"attachments":[{"path":"a","size":"big"}]}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        // The words.
        case("no id", &["show"], &[], vec![], owner_and_port, 2, 0),
        case(
            "an empty id",
            &["show", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an extra word",
            &["show", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["show", "a", "--all"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a word that is not ASCII",
            &["show", "a", "日本"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "in a session by its markers: the would-refuse mark",
            &["show", "x"],
            &[("K2_CELL", "canonical")],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        // The gates.
        case("no port", &["show", "x"], &[], vec![], owner_only, 1, 0),
        case("no token", &["show", "x"], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &["show", "x"],
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
fn show_with_a_passport() {
    let rig = Rig::new(VERB, "spass");
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(rig.stub.port, &[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let v = vec![
        with(
            h(&[]),
            case(
                "the passport over TCP, no whoami",
                &["show", "x"],
                &[],
                vec![ok(PLAIN)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &["show", "x"],
                &[],
                vec![ok(PLAIN)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["show", "x"],
                &[],
                vec![heal(&rig), ok(PLAIN)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a dead passport: the probe, then the restart line",
                &["show", "x"],
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
            cell(&[]),
            case(
                "a live cell socket: still TCP",
                &["show", "x", "--json"],
                &[],
                vec![ok(PLAIN)],
                port_only,
                0,
                1,
            ),
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                cell(&[]),
                case(
                    "HOOK_SOCK and the owner on disk (PRD 13.9 #4)",
                    &["show", "x"],
                    &[],
                    vec![ok(PLAIN)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
        case(
            "HOOK_SOCK set and no passport: the owner",
            &["show", "x"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![ok(PLAIN)],
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
fn show_with_no_answer() {
    let rig = Rig::new(VERB, "snone");
    let v = vec![
        waited(case(
            "an empty reply: the line, exit 1",
            &["show", "x"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "a 5xx with no body: the line, exit 1",
            &["show", "x"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a reset (56): daemon_unreachable",
            &["show", "x"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP (000): daemon_unreachable",
            &["show", "x"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset (76 for curl): daemon_unreachable",
            &["show", "x"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            &["show", "x"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn show_with_no_daemon() {
    let rig = Rig::new(VERB, "sdead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens (7): daemon_unreachable at once",
        &["show", "x"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// No answer within 30 s (28): urllib's timeout, daemon_unreachable.
/// Slow.
#[test]
fn show_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "shang");
    rig.check(&waited(case(
        "no answer in time (28)",
        &["show", "x"],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        1,
        1,
    )));
}

#[test]
fn tickets_show_is_feedback_show() {
    let rig = Rig::new("tickets", "show");
    let full = full();
    let v = vec![
        case(
            "tickets show",
            &["show", "fb_1"],
            &[],
            vec![ok(&full)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tickets show --html",
            &["show", "fb_1", "--html"],
            &[],
            vec![ok(&full)],
            owner_and_port,
            0,
            1,
        ),
        case("tickets show", &["show"], &[], vec![], owner_and_port, 2, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// What `show` hands to the shell: both sides are the shell's.
#[test]
fn show_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "sshell");
    let zero_port = format!("0{}", rig.stub.port);
    let v = vec![
        shell(case(
            "show -h",
            &["show", "x", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "show --help",
            &["show", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        // The binary's own now, as `list`'s: curl reads the port as a number.
        case(
            "a port with a leading zero",
            &["show", "x"],
            &[("K2_PORT", zero_port.as_str())],
            vec![ok(PLAIN)],
            owner_only,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}
