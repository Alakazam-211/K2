//! `k5-cli mail reader` and the retired `mail` owner verbs against the
//! shell CLI's `cli/k2 mail …`, on the parity rig ([`common::parity`]):
//! the same requests (urllib's, from the reader's own Python), stdout,
//! stderr and exit code.
//!
//! - The words: the help (alone, `help`, an empty word), each usage line
//!   (exit 2, nothing sent).
//! - Every verb, plain and `--json`: inboxes, folders, messages, show (a
//!   message, `--html`, a draft), thread, attachment (the file written,
//!   one that is there, a part that is no number), log, drafts,
//!   draft-show, draft-save, draft-send, draft-discard, flag, move,
//!   archive, delete, reply (`--body -` from stdin), draft (a reply and a
//!   new one), send.
//! - The daemon's errors: a refusal (exit 3, no card line), the role
//!   gate's string error, usage (exit 2), an HTTP error that is not JSON,
//!   a 2xx that is not JSON, `ok` missing; answers the Python raises on.
//! - No answer: nothing listening, closed, not HTTP, no answer in time,
//!   a timeout setting that is not a number.
//! - Who it is: the owner from disk, a passport over TCP, the `mail` id
//!   tool's refusal in a session with no passport.
//! - One answer above 256 KiB: thousands of inboxes.
//! - The retired owner verbs: the moved-verb line, exit 2, nothing sent.
//! - Help never acts.
//!
//! The forms here have no census entry (the census is the `--schema`
//! manifest's), so a `// covers:` tag would fail the coverage gate: each
//! row's tag is `// native: <form>`, for the native-bar gate to read.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "mail";

const INBOXES: &str = r#"{"ok":true,"viewer":{"opensAny":true},"inboxes":[
 {"address":"bot@acme.example","provider":"hosted","kind":"agent","workspace":"alpha"},
 {"address":"ann@acme.example","provider":"gmail","kind":"person","person":"Ann","own":true},
 {"address":"x@globex.example","kind":null}]}"#;

const FOLDERS: &str = r#"{"ok":true,"folders":[{"name":"INBOX","role":"inbox"},{"name":"Archive é"},{"name":"Spam","role":null}]}"#;

const MESSAGES: &str = r#"{"ok":true,"items":[
 {"id":"m1","unread":true,"date":"2026-11-20T10:11:12Z","from":{"name":" Ann ","address":"ann@acme.example"},"subject":"Hello é"},
 {"id":"m2","date":"2026-11-19","from":{"address":"a-very-long-address-that-goes-past-thirty@globex.example"}},
 {"id":3,"from":null,"subject":""}],"nextOffset":50}"#;

const MESSAGE: &str = r#"{"ok":true,"message":{"id":"m1","from":{"name":"Ann","address":"ann@acme.example"},
 "to":[{"address":"bot@acme.example"},{"name":"B","address":"b@x.example"}],"cc":[{"address":"c@x.example"}],
 "date":"2026-11-20T10:11:12Z","subject":"Hi","authResults":{"spf":"pass","dkim":null,"dmarc":"fail"},
 "attachments":[{"index":1,"filename":"a.pdf","mime":"application/pdf","size":1234},{"index":2}],
 "text":"body é\nline two","html":"<p>x</p>","remoteContentBlocked":true}}"#;

const DRAFT: &str = r#"{"ok":true,"draft":{"draft":true,"draftId":"d1","id":"m9","author":{"name":"Bot","kind":"agent"},
 "editedAt":1763600000,"editedBy":"Ann","hint":"edit before sending","from":{"address":"me@acme.example"},
 "to":[{"address":"x@globex.example"}],"subject":"Re: Hi","text":""}}"#;

const THREAD: &str = r#"{"ok":true,"messages":[
 {"id":"m1","from":{"address":"a@x.example"},"date":"d1","subject":"One","text":"first"},
 {"id":"m2","from":{"name":"","address":"b@x.example"},"subject":null,"text":null}]}"#;

const LOG: &str = r#"{"ok":true,"rows":[
 {"at":1763600000,"actor":"ann","actorRole":"owner","action":"open","address":"bot@acme.example","outcome":"ok","reason":"audit"},
 {"at":null,"actor":null,"action":"show"}]}"#;

const DRAFTS: &str = r#"{"ok":true,"drafts":[
 {"id":"d1","status":"open","author":{"name":"Bot","kind":"agent"},"editedAt":1763600000,"editedBy":"Ann"},
 {"id":"d2","status":"sent","author":{},"stateBy":"Ann"},
 {"id":"d3","status":"open","stateBy":"Bob"}]}"#;

const ATTACHMENT: &str = r#"{"ok":true,"filename":"a.txt","mime":"text/plain","dataBase64":"aGVsbG8gd29ybGQK","savedTo":"old"}"#;

const DONE: &str = r#"{"ok":true}"#;

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// No `att.bin` in the workspace before each side.
fn no_file(r: &Rig) {
    owner_and_port(r);
    let _ = std::fs::remove_file(r.proj.join("att.bin"));
}

/// `att.bin` is there already.
fn a_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("att.bin"), "old").unwrap();
}

#[test]
fn mail_reader_takes_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "rwords");
    let v = vec![
        case(
            "reader alone: the help",
            &["reader"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "reader help",
            &["reader", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "reader, an empty word",
            &["reader", "", "x"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        usage_case("an unknown command", &["reader", "nope"]),
        usage_case("an unknown flag", &["reader", "show", "m1", "--nope"]),
        usage_case(
            "a value flag last",
            &["reader", "messages", "a@x", "--limit"],
        ),
        usage_case("folders with no inbox", &["reader", "folders"]),
        usage_case("move with one word", &["reader", "move", "m1"]),
        usage_case("an extra word", &["reader", "show", "m1", "m2"]),
        usage_case("inboxes takes no word", &["reader", "inboxes", "x"]),
        usage_case(
            "attachment with no --out",
            &["reader", "attachment", "m1", "1"],
        ),
        usage_case("flag with nothing to change", &["reader", "flag", "m1"]),
        usage_case("reply with no body", &["reader", "reply", "m1"]),
        usage_case(
            "send with no inbox",
            &["reader", "send", "--to", "a@x", "--body", "b"],
        ),
        usage_case(
            "draft with neither id nor inbox",
            &["reader", "draft", "--body", "b"],
        ),
        usage_case("draft-send with no id", &["reader", "draft-send"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_reader_reads_as_the_shell_does() {
    let rig = Rig::new(VERB, "rreads");
    let v = vec![
        // native: mail reader inboxes
        ok_case("inboxes", &["reader", "inboxes"], INBOXES),
        // native: mail reader inboxes --json
        ok_case(
            "inboxes --json, a reason",
            &["reader", "inboxes", "--json", "--reason", "audit é"],
            INBOXES,
        ),
        ok_case(
            "inboxes, none (own only)",
            &["reader", "inboxes"],
            r#"{"ok":true,"inboxes":[]}"#,
        ),
        ok_case(
            "inboxes, none (opens any)",
            &["reader", "inboxes"],
            r#"{"ok":true,"inboxes":null,"viewer":{"opensAny":1}}"#,
        ),
        // native: mail reader folders
        ok_case(
            "folders",
            &["reader", "folders", "bot@acme.example"],
            FOLDERS,
        ),
        // native: mail reader folders --json
        ok_case(
            "folders --json",
            &["reader", "folders", "--json", "bot@acme.example"],
            FOLDERS,
        ),
        // native: mail reader messages
        ok_case(
            "messages, every filter",
            &[
                "reader",
                "messages",
                "bot@acme.example",
                "--folder",
                "INBOX",
                "--unread",
                "--query",
                "a b&c",
                "--from",
                "ann",
                "--since",
                "2026-11-01",
                "--before",
                "2026-12-01",
                "--limit",
                "5",
                "--offset",
                "10",
                "--reason",
                "r",
            ],
            MESSAGES,
        ),
        // native: mail reader messages --json
        ok_case(
            "messages --json",
            &["reader", "messages", "bot@acme.example", "--json"],
            MESSAGES,
        ),
        ok_case(
            "messages, none",
            &["reader", "messages", "bot@acme.example"],
            r#"{"ok":true,"items":[],"nextOffset":null}"#,
        ),
        // native: mail reader show
        ok_case("show", &["reader", "show", "m1"], MESSAGE),
        ok_case("show --html", &["reader", "show", "m1", "--html"], MESSAGE),
        // native: mail reader show --json
        ok_case("show --json", &["reader", "show", "--json", "m1"], MESSAGE),
        ok_case(
            "show, a bare message",
            &["reader", "show", "m1"],
            r#"{"ok":true,"message":{"authResults":{},"to":[],"from":{"name":"  "}}}"#,
        ),
        // native: mail reader thread
        ok_case(
            "thread",
            &["reader", "thread", "m1", "--folder", "INBOX"],
            THREAD,
        ),
        // native: mail reader thread --json
        ok_case(
            "thread --json",
            &["reader", "thread", "m1", "--json"],
            THREAD,
        ),
        // native: mail reader log
        ok_case("log", &["reader", "log"], LOG),
        ok_case(
            "log, every filter",
            &[
                "reader",
                "log",
                "--inbox",
                "bot@acme.example",
                "--actor",
                "ann",
                "--since",
                "1d",
                "--limit",
                "5",
            ],
            r#"{"ok":true,"rows":[]}"#,
        ),
        // native: mail reader log --json
        ok_case("log --json", &["reader", "log", "--json"], LOG),
        // native: mail reader drafts
        ok_case("drafts", &["reader", "drafts", "me@acme.example"], DRAFTS),
        ok_case(
            "drafts, none",
            &["reader", "drafts", "me@acme.example"],
            r#"{"ok":true}"#,
        ),
        // native: mail reader drafts --json
        ok_case(
            "drafts --json",
            &["reader", "drafts", "me@acme.example", "--json"],
            DRAFTS,
        ),
        // native: mail reader draft-show
        ok_case("draft-show", &["reader", "draft-show", "d1"], DRAFT),
        // native: mail reader draft-show --json
        ok_case(
            "draft-show --json",
            &["reader", "draft-show", "d1", "--json"],
            DRAFT,
        ),
        // native: mail reader attachment
        case(
            "attachment",
            &["reader", "attachment", "m1", "02", "--out", "att.bin"],
            &[],
            vec![ok(ATTACHMENT)],
            no_file,
            0,
            1,
        ),
        // native: mail reader attachment --json
        case(
            "attachment --json",
            &[
                "reader",
                "attachment",
                "m1",
                "1",
                "--out",
                "att.bin",
                "--json",
            ],
            &[],
            vec![ok(ATTACHMENT)],
            no_file,
            0,
            1,
        ),
        case(
            "attachment, the file is there",
            &["reader", "attachment", "m1", "1", "--out", "att.bin"],
            &[],
            vec![],
            a_file,
            2,
            0,
        ),
        case(
            "attachment, a part that is no number",
            &["reader", "attachment", "m1", "two", "--out", "att.bin"],
            &[],
            vec![],
            no_file,
            2,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_reader_writes_as_the_shell_does() {
    let rig = Rig::new(VERB, "rwrites");
    let v = vec![
        // native: mail reader flag
        ok_case("flag", &["reader", "flag", "m1", "--read", "--star"], DONE),
        // native: mail reader flag --json
        ok_case(
            "flag --json",
            &["reader", "flag", "m1", "--unread", "--unstar", "--json"],
            DONE,
        ),
        // native: mail reader move
        ok_case(
            "move",
            &["reader", "move", "m1", "Archive", "--reason", "tidy"],
            r#"{"ok":true,"folder":"Archive/2026"}"#,
        ),
        // native: mail reader move --json
        ok_case(
            "move --json",
            &["reader", "move", "m1", "Archive", "--json"],
            DONE,
        ),
        ok_case(
            "move, no folder in the answer",
            &["reader", "move", "m1", "X"],
            DONE,
        ),
        // native: mail reader archive
        ok_case("archive", &["reader", "archive", "m1"], DONE),
        // native: mail reader archive --json
        ok_case(
            "archive --json",
            &["reader", "archive", "m1", "--json"],
            DONE,
        ),
        // native: mail reader delete
        ok_case(
            "delete",
            &["reader", "delete", "m1", "--reason", "spam"],
            DONE,
        ),
        // native: mail reader delete --json
        ok_case("delete --json", &["reader", "delete", "m1", "--json"], DONE),
        // native: mail reader reply
        ok_case(
            "reply",
            &["reader", "reply", "m1", "--body", "Thanks é\nAnn"],
            r#"{"ok":true,"status":"sent","id":"out_1"}"#,
        ),
        // native: mail reader reply --json
        ok_case(
            "reply --json",
            &["reader", "reply", "m1", "--body", "x", "--json"],
            DONE,
        ),
        Case {
            stdin: Some(b"From stdin \xc3\xa9\n\n\n"),
            ..ok_case(
                "reply, the body on stdin",
                &["reader", "reply", "m1", "--body", "-"],
                DONE,
            )
        },
        Case {
            stdin: Some(b"read once\n"),
            ..ok_case(
                "reply, stdin read twice: the second is empty",
                &["reader", "reply", "m1", "--body", "-", "--body", "-"],
                DONE,
            )
        },
        // native: mail reader draft
        ok_case(
            "draft, a reply",
            &["reader", "draft", "m1", "--body", "x"],
            r#"{"ok":true,"draftId":"d7","folder":"[Gmail]/Drafts"}"#,
        ),
        ok_case(
            "draft, a new one",
            &[
                "reader",
                "draft",
                "--inbox",
                "me@acme.example",
                "--to",
                "a@x, b@x",
                "--to",
                "c@x",
                "--cc",
                "d@x",
                "--subject",
                "S",
                "--body",
                "x",
            ],
            r#"{"ok":true,"draftId":"d8"}"#,
        ),
        // native: mail reader draft --json
        ok_case(
            "draft --json, no draft id",
            &["reader", "draft", "m1", "--body", "x", "--json"],
            DONE,
        ),
        ok_case(
            "draft, no draft id",
            &["reader", "draft", "m1", "--body", "x"],
            DONE,
        ),
        // native: mail reader send
        ok_case(
            "send",
            &[
                "reader",
                "send",
                "--inbox",
                "me@acme.example",
                "--to",
                "a@x",
                "--subject",
                "S",
                "--body",
                "x",
            ],
            r#"{"ok":true,"status":"queued","id":"out_2"}"#,
        ),
        // native: mail reader send --json
        ok_case(
            "send --json",
            &[
                "reader",
                "send",
                "--inbox",
                "me@acme.example",
                "--body",
                "",
                "--json",
            ],
            DONE,
        ),
        ok_case(
            "send, no status",
            &[
                "reader",
                "send",
                "--inbox",
                "me@acme.example",
                "--body",
                "x",
            ],
            DONE,
        ),
        // native: mail reader draft-save
        ok_case(
            "draft-save",
            &[
                "reader",
                "draft-save",
                "d1",
                "--to",
                "a@x",
                "--cc",
                "b@x",
                "--subject",
                "S",
                "--body",
                "x",
                "--reason",
                "fix",
            ],
            DONE,
        ),
        // native: mail reader draft-save --json
        ok_case(
            "draft-save --json",
            &["reader", "draft-save", "d1", "--json"],
            DONE,
        ),
        // native: mail reader draft-send
        ok_case(
            "draft-send",
            &["reader", "draft-send", "d1"],
            r#"{"ok":true,"status":"sent","id":"out_3","draftStatus":"sent"}"#,
        ),
        ok_case(
            "draft-send, a bare answer",
            &["reader", "draft-send", "d1"],
            DONE,
        ),
        // native: mail reader draft-send --json
        ok_case(
            "draft-send --json",
            &["reader", "draft-send", "d1", "--json"],
            DONE,
        ),
        // native: mail reader draft-discard
        ok_case(
            "draft-discard",
            &[
                "reader",
                "draft-discard",
                "d1",
                "--body",
                "ignored",
                "--to",
                "x",
            ],
            DONE,
        ),
        // native: mail reader draft-discard --json
        ok_case(
            "draft-discard --json",
            &["reader", "draft-discard", "d1", "--json"],
            DONE,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_reader_errors_as_the_shell_does() {
    let rig = Rig::new(VERB, "rerrors");
    let traceback = |c: Case| compare(Compare::Traceback, c);
    let v = vec![
        case(
            "an agent: owner only",
            &["reader", "inboxes"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the role gate's string error",
            &["reader", "messages", "a@x"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"role_required","message":"people only"}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a string error with no message",
            &["reader", "log"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"not_found"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the daemon's usage error",
            &["reader", "show", "m1"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"ok":false,"error":{"code":"usage","hint":"bad id"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an HTTP error that is not JSON",
            &["reader", "inboxes"],
            &[],
            vec![Reply::Answer(500, "oops".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 2xx that is not JSON",
            &["reader", "inboxes"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "ok missing",
            &["reader", "archive", "m1"],
            &[],
            vec![ok(r#"{"status":"x é"}"#)],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "an answer that is a list",
            &["reader", "inboxes"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "show with no message",
            &["reader", "show", "m1"],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an inbox with no address",
            &["reader", "inboxes"],
            &[],
            vec![ok(r#"{"ok":true,"inboxes":[{"kind":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a date that is a number",
            &["reader", "messages", "a@x"],
            &[],
            vec![ok(r#"{"ok":true,"items":[{"id":"m1","date":5}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(Case {
            stdin: Some(b"caf\xe9 au lait\n"),
            ..case(
                "a body on stdin that is not UTF-8",
                &["reader", "reply", "m1", "--body", "-"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            )
        }),
        // No answer.
        case(
            "nothing listening",
            &["reader", "inboxes"],
            &[("K2_PORT", leak(dead_port()))],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "closed, never asked again",
            &["reader", "messages", "a@x"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["reader", "drafts", "a@x"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "no answer in time",
            &["reader", "log"],
            &[("K5_MAIL_HTTP_TIMEOUT", "1")],
            vec![Reply::Hang(Duration::from_secs(3))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a timeout setting that is not a number",
            &["reader", "inboxes"],
            &[("K2_MAIL_HTTP_TIMEOUT", "soon")],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // Who it is.
        Case {
            env: with_passport(&[]),
            ..case(
                "a passport over TCP",
                &["reader", "inboxes"],
                &[],
                vec![Reply::Answer(
                    403,
                    r#"{"ok":false,"error":{"code":"forbidden","hint":"people only"}}"#.into(),
                )],
                nothing,
                3,
                1,
            )
        },
        case(
            "the id tool in a session with no passport",
            &["reader", "inboxes"],
            &[("K2_HOOK_SOCK", "{deadsock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// The one large answer: 4,000 inboxes (above 256 KiB).
#[test]
fn mail_reader_large_list() {
    let rig = Rig::new(VERB, "rlarge");
    let rows: Vec<String> = (0..4000)
        .map(|i| {
            format!(
                r#"{{"address":"inbox-{i:05}@acme.example","provider":"hosted","kind":"agent","workspace":"workspace-number-{i}","own":{}}}"#,
                i % 7 == 0
            )
        })
        .collect();
    let body = format!(r#"{{"ok":true,"inboxes":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024);
    rig.check(&case(
        "4,000 inboxes",
        &["reader", "inboxes"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

/// The verbs that change something, with `-h` or `--help` anywhere: the
/// help, and nothing sent.
#[test]
fn mail_reader_help_never_acts() {
    let rig = Rig::new(VERB, "rhelp");
    const LINES: &[&[&str]] = &[
        &["reader", "flag", "m1", "--read", "-h"],
        &["reader", "move", "m1", "X", "--help"],
        &["reader", "archive", "-h", "m1"],
        &["reader", "delete", "m1", "--help"],
        &["reader", "reply", "m1", "--body", "x", "-h"],
        &["reader", "draft", "m1", "--body", "x", "--help"],
        &["reader", "send", "--inbox", "a", "--body", "x", "-h"],
        &["reader", "draft-save", "d1", "--help"],
        &["reader", "draft-send", "d1", "-h"],
        &["reader", "draft-discard", "d1", "--help"],
        &["reader", "attachment", "m1", "1", "--out", "f", "-h"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case(
                "help, nothing sent",
                line,
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ));
    }
}

/// The owner verbs that moved to `hostmail`: the moved-verb line, exit
/// 2, nothing sent, whatever follows (but `create`'s help).
#[test]
fn mail_retired_verbs_say_where_they_went() {
    let rig = Rig::new(VERB, "retired");
    let v = vec![
        // native: mail create
        usage_case("create", &["create", "bot"]),
        // native: mail status
        usage_case("status", &["status"]),
        // native: mail domain
        usage_case("domain", &["domain", "add", "acme.example"]),
        // native: mail doctor
        usage_case("doctor", &["doctor", "--json"]),
        // native: mail config
        usage_case("config", &["config"]),
        // native: mail approvals
        usage_case("approvals", &["approvals", "list"]),
        // native: mail catchall
        usage_case("catchall", &["catchall"]),
        // native: mail alias
        usage_case("alias", &["alias", "list"]),
        // native: mail ooo
        usage_case("ooo", &["ooo"]),
        // native: mail footer
        usage_case("footer", &["footer", "show"]),
        // native: mail spam
        usage_case("spam", &["spam"]),
        // native: mail queue
        usage_case("queue", &["queue"]),
        // native: mail acl
        usage_case("acl", &["acl"]),
        // native: mail autoconfig
        usage_case("autoconfig", &["autoconfig"]),
        // native: mail app-password
        usage_case("app-password", &["app-password", "list"]),
        // native: mail person
        usage_case("person", &["person"]),
        // native: mail dkim
        usage_case("dkim", &["dkim"]),
        // native: mail dmarc
        usage_case("dmarc", &["dmarc"]),
        // native: mail ptr
        usage_case("ptr", &["ptr"]),
        // native: mail bans
        usage_case("bans", &["bans"]),
        // native: mail allowlist
        usage_case("allowlist", &["allowlist", "add", "x"]),
        // native: mail audit-sieve
        usage_case("audit-sieve", &["audit-sieve"]),
        // Since 0.46.17 `create`'s help is `hostmail create`'s page, not
        // the moved line.
        // native: mail create --help
        case(
            "create --help: hostmail create's help",
            &["create", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        compare(
            Compare::Shell,
            case(
                "create, a name, then -h: the help",
                &["create", "bot", "-h"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        case(
            "a retired verb in a session with no passport",
            &["create", "bot"],
            &[("K2_HOOK_SOCK", "{deadsock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}
