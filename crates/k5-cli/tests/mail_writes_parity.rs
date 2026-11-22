//! `k5-cli mail` writes against the shell CLI's `cli/k2 mail`, on the
//! parity rig ([`common::parity`]): the same requests (urllib's, from
//! `_mail_py`), stdout, stderr and exit code.
//!
//! - Every verb, plain and `--json`: send (every flag, `--body-file`,
//!   queued, submitted, decided under `--wait`, and still queued at the
//!   wait's end, exit 2), reply, forward-message, draft (a reply and a
//!   compose), move, flag, archive, delete (a message, and an address
//!   retired).
//! - Each `cmd_mail_*`'s usage lines (exit 2, nothing sent).
//! - The daemon's refusals: gated (exit 3), the permission card, the
//!   agent send path, an attachment secret (exit 2), not ok.
//! - Sent once: a POST closed with nothing, a reset and no answer in
//!   time are not sent again; a refused connect is tried four times
//!   (nothing went out) and the 503 "migrating" answer is asked again
//!   (no handler ran).
//! - Help never acts: each verb with `-h` or `--help` anywhere prints its
//!   help and sends nothing.
//! - A body above 128 KiB (from `--body-file`): the shell hands `M_BODY` to
//!   its Python in the environment, which Linux refuses for one string
//!   past 128 KiB (exit 126, nothing sent); the binary sends it
//!   ([`a_large_body`]): the named difference `env-answer-large`.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "mail";

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

/// `body.txt` (two lines and trailing newlines) and `empty.txt` in the
/// workspace, and the owner and port.
fn files(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("body.txt"), "line one é\nline two\n\n\n").unwrap();
    std::fs::write(r.proj.join("empty.txt"), "\n").unwrap();
    std::fs::create_dir_all(r.proj.join("adir")).unwrap();
}

#[test]
fn mail_writes_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        usage_case(
            "send, no recipient",
            &["send", "--subject", "s", "--body", "b"],
        ),
        usage_case("send, no subject", &["send", "a@x.example", "--body", "b"]),
        usage_case(
            "send, body and body file",
            &[
                "send",
                "a@x.example",
                "--subject",
                "s",
                "--body",
                "b",
                "--body-file",
                "f",
            ],
        ),
        usage_case(
            "send, no such body file",
            &[
                "send",
                "a@x.example",
                "--subject",
                "s",
                "--body-file",
                "nope.txt",
            ],
        ),
        usage_case(
            "send, a folder as the body file",
            &["send", "a@x.example", "--subject", "s", "--body-file", "/"],
        ),
        usage_case("send, no body", &["send", "a@x.example", "--subject", "s"]),
        usage_case(
            "send, an empty body",
            &["send", "a@x.example", "--subject", "s", "--body", ""],
        ),
        usage_case(
            "send, a blank display name",
            &[
                "send",
                "a@x.example",
                "--subject",
                "s",
                "--body",
                "b",
                "--from-name",
                " \t",
            ],
        ),
        usage_case(
            "send, a timeout with no wait",
            &[
                "send",
                "a@x.example",
                "--subject",
                "s",
                "--body",
                "b",
                "--timeout",
                "5",
            ],
        ),
        usage_case(
            "send, a timeout of 0",
            &[
                "send",
                "a@x.example",
                "--subject",
                "s",
                "--body",
                "b",
                "--wait",
                "--timeout",
                "0",
            ],
        ),
        usage_case(
            "send, at and in",
            &[
                "send",
                "a@x.example",
                "--subject",
                "s",
                "--body",
                "b",
                "--at",
                "x",
                "--in",
                "2h",
            ],
        ),
        usage_case(
            "send, wait and in",
            &[
                "send",
                "a@x.example",
                "--subject",
                "s",
                "--body",
                "b",
                "--wait",
                "--in",
                "2h",
            ],
        ),
        usage_case("send, an unknown flag", &["send", "a@x.example", "--nope"]),
        usage_case(
            "send, a value flag at the end",
            &["send", "a@x.example", "--cc"],
        ),
        usage_case("reply, no id", &["reply", "--body", "b"]),
        usage_case("reply, a reply-to", &["reply", "m_1", "--reply-to", "x@y"]),
        usage_case("reply, two ids", &["reply", "m_1", "m_2"]),
        usage_case("reply, no body", &["reply", "m_1"]),
        usage_case("reply, an unknown flag", &["reply", "m_1", "-b"]),
        usage_case(
            "forward-message, no id",
            &["forward-message", "--to", "a@x"],
        ),
        usage_case("forward-message, no to", &["forward-message", "m_1"]),
        usage_case(
            "forward-message, both reply-to forms",
            &[
                "forward-message",
                "m_1",
                "--to",
                "a@x",
                "--reply-to",
                "r@x",
                "--reply-to-original",
            ],
        ),
        usage_case(
            "forward-message, two ids",
            &["forward-message", "m_1", "m_2"],
        ),
        usage_case(
            "forward-message, a blank display name",
            &["forward-message", "m_1", "--to", "a@x", "--from-name", ""],
        ),
        usage_case(
            "draft, a recipient and reply-all on a reply",
            &["draft", "m_1", "--to", "a@x", "--reply-all", "--body", "b"],
        ),
        usage_case(
            "draft, a subject on a reply",
            &["draft", "m_1", "--subject", "s", "--body", "b"],
        ),
        usage_case(
            "draft, a subject and a recipient on a reply",
            &["draft", "m_1", "--to", "a@x", "--subject", "s"],
        ),
        usage_case(
            "draft, an idempotency key with no value",
            &["draft", "m_1", "--body", "b", "--idempotency-key"],
        ),
        usage_case(
            "draft, an unknown flag",
            &["draft", "m_1", "--body", "b", "--key", "k"],
        ),
        // native: mail draft delete
        usage_case("draft delete, no id", &["draft", "delete"]),
        usage_case("draft delete, two ids", &["draft", "delete", "d_1", "d_2"]),
        usage_case(
            "draft delete, an unknown flag",
            &["draft", "delete", "d_1", "--force"],
        ),
        usage_case(
            "draft delete, a lone dash",
            &["draft", "delete", "-", "d_1"],
        ),
        usage_case(
            "draft, reply-all with no id",
            &["draft", "--reply-all", "--body", "b"],
        ),
        usage_case(
            "draft, an id and a from",
            &["draft", "m_1", "--from", "a@x", "--body", "b"],
        ),
        usage_case(
            "draft, compose with no to",
            &["draft", "--subject", "s", "--body", "b"],
        ),
        usage_case(
            "draft, compose with no subject",
            &["draft", "--to", "a@x", "--body", "b"],
        ),
        usage_case("draft, two ids", &["draft", "m_1", "m_2"]),
        usage_case(
            "draft, two reply-to ids",
            &["draft", "m_1", "--reply-to", "m_2"],
        ),
        usage_case("draft, no body", &["draft", "m_1", "m_1"]),
        usage_case("move, no id", &["move"]),
        usage_case("move, no folder", &["move", "m_1"]),
        usage_case("move, three words", &["move", "m_1", "A", "B"]),
        usage_case("flag, no id", &["flag", "--read"]),
        usage_case("flag, nothing to set", &["flag", "m_1"]),
        usage_case("flag, two ids", &["flag", "m_1", "m_2", "--read"]),
        usage_case("archive, no id", &["archive", "--json"]),
        usage_case("archive, an unknown flag", &["archive", "m_1", "--now"]),
        usage_case("delete, nothing", &["delete"]),
        usage_case("delete, neither an id nor an address", &["delete", "abc"]),
        usage_case("delete, two", &["delete", "m_1", "m_2"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_writes_send_and_print_as_the_shell_does() {
    let rig = Rig::new(VERB, "send");
    let with = |c: Case| Case { setup: files, ..c };
    let v = vec![
        // covers: mail send
        ok_case(
            "send, queued",
            &[
                "send",
                "a@x.example",
                "--subject",
                "Hi",
                "--body",
                "Hello é",
            ],
            r#"{"ok":true,"queued":true,"id":"out_1","status":"pending"}"#,
        ),
        ok_case(
            "send, queued with the hint",
            &[
                "send",
                "--to",
                "a@x.example",
                "--to",
                "b@x.example, c@x.example",
                "--subject",
                "Hi",
                "--body",
                "b",
            ],
            r#"{"ok":true,"queued":true,"id":"out_1","hint":"queued; your human decides"}"#,
        ),
        ok_case(
            "send, every flag",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body",
                "B",
                "--cc",
                "c@x.example",
                "--bcc",
                "d@x.example",
                "--from",
                "bot@acme.example",
                "--from-name",
                "Bot",
                "--reply-to",
                "r@x.example",
                "--attach",
                "a b.txt",
                "--attach",
                "c,d.txt",
                "--in",
                "2h",
            ],
            r#"{"ok":true,"id":"out_2","status":"submitted"}"#,
        ),
        // covers: mail send --json
        ok_case(
            "send, submitted with the hint, --json",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body",
                "B",
                "--at",
                "2030-01-01T00:00:00Z",
                "--json",
            ],
            r#"{"ok":true,"id":"out_2","status":"scheduled","hint":"goes at 2030"}"#,
        ),
        ok_case(
            "send --wait, decided",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body",
                "B",
                "--wait",
                "--timeout",
                "0090",
            ],
            r#"{"ok":true,"id":"out_3","timedOut":false,"status":"rejected","note":"not this one"}"#,
        ),
        ok_case(
            "send --wait, decided with a note on the status",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body",
                "B",
                "--wait",
            ],
            r#"{"ok":true,"id":"out_3","timedOut":false,"statusNote":"approved and submitted"}"#,
        ),
        case(
            "send --wait, still queued",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body",
                "B",
                "--wait",
                "--timeout",
                "5",
            ],
            &[],
            vec![ok(r#"{"ok":true,"id":"out_4","timedOut":true}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "send --wait, still queued, the daemon's hint",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body",
                "B",
                "--wait",
                "--json",
            ],
            &[],
            vec![ok(
                r#"{"ok":true,"id":"out_4","timedOut":true,"hint":"still waiting"}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        with(ok_case(
            "send, a body file",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body-file",
                "body.txt",
            ],
            r#"{"ok":true,"id":"out_5","status":"submitted"}"#,
        )),
        with(usage_case(
            "send, a body file of one newline",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body-file",
                "empty.txt",
            ],
        )),
        with(usage_case(
            "send, a folder as the body file",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body-file",
                "adir",
            ],
        )),
        // covers: mail reply
        ok_case(
            "reply",
            &[
                "reply",
                "m_1",
                "--body",
                "thanks",
                "--reply-all",
                "--cc",
                "c@x.example",
                "--bcc",
                "d@x.example",
                "--from-name",
                "Ann",
                "--attach",
                "f.txt",
            ],
            r#"{"ok":true,"queued":true,"id":"out_6"}"#,
        ),
        with(ok_case(
            "reply --wait, a body file",
            &[
                "reply",
                "m_1",
                "--body-file",
                "body.txt",
                "--wait",
                "--timeout",
                "30",
            ],
            r#"{"ok":true,"id":"out_7","timedOut":false,"status":"submitted"}"#,
        )),
        ok_case(
            "forward-message, every flag",
            &[
                "forward-message",
                "m_1",
                "--to",
                "a@x.example",
                "--cc",
                "c@x.example",
                "--bcc",
                "d@x.example",
                "--note",
                "fyi",
                "--as-eml",
                "--from",
                "bot@acme.example",
                "--from-name",
                "Bot",
                "--reply-to-original",
                "--wait",
                "--timeout",
                "60",
            ],
            r#"{"ok":true,"id":"out_8","timedOut":false,"status":"approved"}"#,
        ),
        // covers: mail forward-message
        // covers: mail forward-message --json
        ok_case(
            "forward-message, a reply-to",
            &[
                "forward-message",
                "m_1",
                "--to",
                "a@x.example",
                "--reply-to",
                "r@x.example",
                "--json",
            ],
            r#"{"ok":true,"id":"out_8","status":"submitted"}"#,
        ),
        // covers: mail draft
        ok_case(
            "draft, a reply",
            &[
                "draft",
                "m_1",
                "--body",
                "draft text",
                "--reply-all",
                "--cc",
                "c@x.example",
                "--attach",
                "f.txt",
                "--idempotency-key",
                "key-0001",
            ],
            r#"{"ok":true,"folder":"Drafts","address":"me@globex.example"}"#,
        ),
        ok_case(
            "draft, a reply by --reply-to",
            &[
                "draft",
                "--reply-to",
                "m_1",
                "m_1",
                "--body",
                "x",
                "--idempotency-key",
                "key-0002",
            ],
            r#"{"ok":true,"hint":"saved"}"#,
        ),
        ok_case(
            "draft, a reply to whoever --to names",
            &[
                "draft",
                "m_1",
                "--to",
                "boss@x.example",
                "--to",
                "b2@x.example,b3@x.example",
                "--cc",
                "cy@x.example",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0003",
            ],
            r#"{"ok":true,"address":"me@x.example","folder":"Drafts","id":"m_1","draftId":"d_0001","hint":"draft saved"}"#,
        ),
        ok_case(
            "draft, the hint names the draft id",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0004",
            ],
            r#"{"ok":true,"draftId":"d_0002","hint":"draft d_0002 was already saved","replayed":true}"#,
        ),
        ok_case(
            "draft, no hint and a draft id",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0005",
            ],
            r#"{"ok":true,"draftId":"d_0003","address":"me@x.example"}"#,
        ),
        ok_case(
            "draft, a hint that is a list",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0006",
            ],
            r#"{"ok":true,"draftId":"d_0004","hint":["d_0004"]}"#,
        ),
        ok_case(
            "draft, not ok is printed as it came",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0007",
            ],
            r#"{"ok":false,"hint":"saved anyway"}"#,
        ),
        // native: mail draft delete
        ok_case(
            "draft delete",
            &["draft", "delete", "d_0001"],
            r#"{"ok":true,"draftId":"d_0001","draftStatus":"discarded","hint":"draft d_0001 deleted from me@x.example (moved to Trash)"}"#,
        ),
        ok_case(
            "draft delete, no hint",
            &["draft", "delete", "d_0001"],
            r#"{"ok":true,"draftId":"d_0001"}"#,
        ),
        // native: mail draft delete --json
        ok_case(
            "draft delete --json",
            &["draft", "delete", "--json", "d_0001"],
            r#"{"ok":true,"draftId":"d_0001","draftStatus":"discarded"}"#,
        ),
        // covers: mail draft --json
        with(ok_case(
            "draft, a compose",
            &[
                "draft",
                "--to",
                "a@x.example",
                "--subject",
                "S",
                "--body-file",
                "body.txt",
                "--from",
                "me@globex.example",
                "--bcc",
                "b@x.example",
                "--json",
                "--idempotency-key",
                "key-0008",
            ],
            r#"{"ok":true,"folder":"Drafts"}"#,
        )),
        // covers: mail move
        ok_case(
            "move",
            &["move", "m_1", "Projects/2026"],
            r#"{"ok":true,"id":"m_1"}"#,
        ),
        // covers: mail move --json
        ok_case(
            "move, the folder it went to",
            &["move", "m_1", "x", "--json"],
            r#"{"ok":true,"id":"m_1","folder":"X"}"#,
        ),
        // covers: mail flag
        ok_case(
            "flag",
            &["flag", "m_1", "--read", "--star", "--unread"],
            r#"{"ok":true,"id":"m_1"}"#,
        ),
        // covers: mail flag --json
        ok_case(
            "flag --json",
            &["flag", "m_1", "--star", "--json"],
            r#"{"ok":true,"id":"m_1","flagged":true}"#,
        ),
        // covers: mail reply --json
        ok_case(
            "reply --json",
            &["reply", "m_1", "--body", "ok", "--json"],
            r#"{"ok":true,"queued":true,"id":"out_6"}"#,
        ),
        ok_case(
            "flag, unflag",
            &["flag", "m_1", "--unflag"],
            r#"{"ok":true,"id":"m_1"}"#,
        ),
        // covers: mail archive
        ok_case("archive", &["archive", "m_1"], r#"{"ok":true,"id":"m_1"}"#),
        // covers: mail archive --json
        ok_case(
            "archive --json",
            &["archive", "m_1", "--json"],
            r#"{"ok":true,"id":"m_1","folder":"All Mail"}"#,
        ),
        // covers: mail delete
        ok_case(
            "delete a message",
            &["delete", "m_1"],
            r#"{"ok":true,"id":"m_1","folder":"Deleted Items"}"#,
        ),
        ok_case(
            "delete an address",
            &["delete", "old@acme.example"],
            r#"{"ok":true,"address":"old@acme.example"}"#,
        ),
        // covers: mail delete --json
        ok_case(
            "delete an address --json",
            &["delete", "--json", "old@acme.example"],
            r#"{"ok":true,"address":"old@acme.example"}"#,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_writes_read_the_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "answers");
    let send: &'static [&'static str] = &["send", "a@x.example", "--subject", "S", "--body", "B"];
    let v = vec![
        case(
            "gated",
            send,
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"gated","hint":"agent sending is off"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the permission card",
            &["move", "m_1", "X"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the agent send path",
            send,
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"agent_send_path_only","hint":"use mail send"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an attachment secret",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body",
                "B",
                "--attach",
                ".env",
            ],
            &[],
            vec![answer(
                400,
                r#"{"error":{"code":"attachment_secret","hint":"that file looks like a secret"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "not ok",
            &["archive", "m_1"],
            &[],
            vec![ok(r#"{"ok":false}"#)],
            owner_and_port,
            1,
            1,
        ),
        compare(
            Compare::Traceback,
            case(
                "an answer that is a list",
                &["flag", "m_1", "--read"],
                &[],
                vec![ok("[]")],
                owner_and_port,
                1,
                1,
            ),
        ),
        // Sent once.
        case(
            "closed, not sent again",
            send,
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset, not sent again",
            &["reply", "m_1", "--body", "b"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "no answer in time, not sent again",
            &["delete", "m_1"],
            &[("K5_MAIL_HTTP_TIMEOUT", "1")],
            vec![Reply::Hang(Duration::from_secs(3))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "migrating, asked again",
            &[
                "draft",
                "m_1",
                "--body",
                "b",
                "--idempotency-key",
                "key-0009",
            ],
            &[],
            vec![
                answer(503, r#"{"state":"migrating"}"#),
                ok(r#"{"ok":true,"hint":"saved"}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "nothing listening",
            send,
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
        // A draft save is asked again with the same key on busy, a
        // daemon timeout and a dropped answer (twice at most), and never
        // on anything else.
        case(
            "draft, busy then the saved draft",
            &[
                "draft",
                "--to",
                "a@x.example",
                "--subject",
                "s",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0010",
                "--json",
            ],
            &[],
            vec![
                answer(
                    409,
                    r#"{"ok":false,"error":{"code":"busy","hint":"still saving"}}"#,
                ),
                ok(
                    r#"{"ok":true,"draftId":"d_0005","replayed":true,"hint":"draft d_0005 was already saved"}"#,
                ),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "draft, the daemon's timeout then the draft",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0011",
            ],
            &[],
            vec![
                answer(
                    504,
                    r#"{"ok":false,"error":{"code":"timeout","hint":"slow"}}"#,
                ),
                ok(r#"{"ok":true,"hint":"saved"}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "draft, dropped twice then saved",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0012",
            ],
            &[],
            vec![
                Reply::Close,
                Reply::Close,
                ok(r#"{"ok":true,"hint":"saved"}"#),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "draft, dropped three times: the key to ask with",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0013",
            ],
            &[],
            vec![Reply::Close, Reply::Close, Reply::Close],
            owner_and_port,
            2,
            3,
        ),
        case(
            "draft, nothing listening: the key to ask with",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0014",
            ],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            2,
            0,
        ),
        case(
            "draft, inbox_busy is final",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0015",
            ],
            &[],
            vec![answer(
                429,
                r#"{"ok":false,"error":{"code":"inbox_busy","hint":"4 sessions open","retry_after_s":5},"retry_after_s":5}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "draft, inbox_throttled is final",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0016",
            ],
            &[],
            vec![answer(
                503,
                r#"{"ok":false,"error":{"code":"inbox_throttled","hint":"the provider is limiting this account","retry_after_s":0,"retry":null}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "draft, a refusal with the permission card",
            &[
                "draft",
                "m_1",
                "--body",
                "hi",
                "--idempotency-key",
                "key-0017",
            ],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        compare(
            Compare::Traceback,
            case(
                "draft, an error that is a string",
                &[
                    "draft",
                    "m_1",
                    "--body",
                    "hi",
                    "--idempotency-key",
                    "key-0018",
                ],
                &[],
                vec![ok(r#"{"error":"boom"}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        compare(
            Compare::Traceback,
            case(
                "draft, an answer that is a list",
                &[
                    "draft",
                    "m_1",
                    "--body",
                    "hi",
                    "--idempotency-key",
                    "key-0019",
                ],
                &[],
                vec![ok("[1]")],
                owner_and_port,
                1,
                1,
            ),
        ),
        compare(
            Compare::Traceback,
            case(
                "draft, an answer that is an empty list",
                &[
                    "draft",
                    "m_1",
                    "--body",
                    "hi",
                    "--idempotency-key",
                    "key-0020",
                ],
                &[],
                vec![ok("[]")],
                owner_and_port,
                1,
                1,
            ),
        ),
        compare(
            Compare::Traceback,
            case(
                "draft, a draft id that is a number",
                &[
                    "draft",
                    "m_1",
                    "--body",
                    "hi",
                    "--idempotency-key",
                    "key-0021",
                ],
                &[],
                vec![ok(r#"{"ok":true,"draftId":5,"hint":"saved"}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        case(
            "draft delete, someone else's",
            &["draft", "delete", "d_0009"],
            &[],
            vec![answer(
                403,
                r#"{"ok":false,"error":{"code":"not_your_draft","hint":"nothing was touched"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "draft delete, a failed fetch says whether to retry",
            &["draft", "delete", "d_0009"],
            &[],
            vec![answer(
                502,
                r#"{"ok":false,"error":{"code":"engine","hint":"Safe to retry","retry":"safe","retry_after_s":false}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "a passport over TCP",
                &["archive", "m_1"],
                &[],
                vec![ok(r#"{"ok":true,"id":"m_1"}"#)],
                nothing,
                0,
                1,
            )
        },
        case(
            "the id tool in a session with no passport",
            send,
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

/// Every write with `-h` or `--help` anywhere: the help, and nothing sent
/// (the front prints a help form of src/help_native.txt itself, as
/// `draft delete --help`, and hands any other help line to the shell).
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    const LINES: &[&[&str]] = &[
        &["send", "a@x.example", "--subject", "S", "--body", "B", "-h"],
        &[
            "send",
            "--help",
            "a@x.example",
            "--subject",
            "S",
            "--body",
            "B",
        ],
        &["send", "a@x.example", "--subject", "-h", "--body", "B"],
        &["reply", "m_1", "--body", "b", "--help"],
        &["forward-message", "m_1", "--to", "a@x.example", "-h"],
        &["draft", "m_1", "--body", "b", "--help"],
        &["draft", "delete", "d_1", "--help"],
        &["draft", "delete", "--help"],
        &["draft", "delete", "-h"],
        &["draft", "-h", "delete", "d_1"],
        &[
            "draft",
            "--to",
            "a@x.example",
            "--subject",
            "s",
            "--body",
            "b",
            "--idempotency-key",
            "k-12345678",
            "-h",
        ],
        &["move", "m_1", "X", "-h"],
        &["flag", "--help", "m_1", "--read"],
        &["archive", "m_1", "-h"],
        &["delete", "m_1", "--help"],
        &["delete", "-h", "old@acme.example"],
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

/// A body of 200 KiB, from `--body-file` (an argument that long can't
/// start either side: Linux refuses one argv string past 128 KiB).
fn big_body() -> String {
    "é and some words of the body ".repeat(7000)
}

/// `big.txt` in the workspace, and the owner and port.
fn big_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("big.txt"), big_body()).unwrap();
}

/// The shell reads the file into `M_BODY`, an environment string, which
/// Linux refuses past 128 KiB: `python3` never starts (exit 126,
/// `Argument list too long`) and nothing is sent. The binary sends it
/// once and prints the answer. Elsewhere (macOS: one limit for the whole
/// environment) both send it. The named difference `env-answer-large`.
#[test]
fn a_large_body() {
    let rig = Rig::new(VERB, "large");
    let big = big_body();
    assert!(big.len() > 200 * 1024, "{} bytes", big.len());
    let c = Case {
        setup: big_file,
        ..case(
            "a large body",
            &[
                "send",
                "a@x.example",
                "--subject",
                "S",
                "--body-file",
                "big.txt",
            ],
            &[],
            vec![ok(r#"{"ok":true,"queued":true,"id":"out_9"}"#)],
            owner_and_port,
            0,
            1,
        )
    };
    let (out, log) = rig.binary(&c, false);
    assert_eq!(log.len(), 1, "one request");
    assert!(log[0].contains(&big[..1000]), "the body went out");
    assert_eq!(
        (out.code, out.stderr.as_str()),
        (Some(0), ""),
        "{:?}",
        out.stdout
    );
    assert_eq!(
        out.stdout,
        "queued for approval (out_9) — track with 'k2 mail outbox out_9'\n"
    );
    rig.prepare(Side::Bash, &c);
    let bash = rig.run(true, &c);
    let sent = rig.read(Side::Bash, false, c.name);
    if cfg!(target_os = "linux") {
        assert_eq!(bash.code, Some(126), "{bash:?}");
        assert!(sent.is_empty(), "the shell sent nothing");
        assert!(
            bash.stderr.contains("Argument list too long"),
            "{}",
            bash.stderr
        );
    } else {
        assert_eq!(sent.len(), 1);
        assert_eq!(
            (bash.code, &bash.stdout, &bash.stderr),
            (out.code, &out.stdout, &out.stderr)
        );
    }
}

/// The key `"k2d_" + uuid.uuid4().hex` in a request body.
fn key_in(request: &str) -> String {
    let at = request
        .find("\"idempotencyKey\"")
        .unwrap_or_else(|| panic!("no idempotencyKey in {request:?}"));
    let rest = &request[at + "\"idempotencyKey\"".len()..];
    let open = rest.find('"').expect("the key's opening quote") + 1;
    let close = rest[open..].find('"').expect("the key's closing quote");
    rest[open..open + close].to_string()
}

/// A key in a version 4 UUID's shape, as `uuid.uuid4().hex` makes it.
fn assert_fresh_shape(key: &str, side: &str) {
    let hex = key
        .strip_prefix("k2d_")
        .unwrap_or_else(|| panic!("{side}: {key:?} does not start k2d_"));
    assert_eq!(hex.len(), 32, "{side}: {key:?}");
    assert!(
        hex.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{side}: {key:?} is not lower-case hex"
    );
    assert_eq!(&hex[12..13], "4", "{side}: {key:?}: the version digit");
    assert!(
        matches!(&hex[16..17], "8" | "9" | "a" | "b"),
        "{side}: {key:?}: the variant digit"
    );
}

/// With no `--idempotency-key`, every save carries a fresh key: random per
/// run, so the rows above set one, and this one checks its shape. Both
/// sides send the same request but for the key, and print the same.
#[test]
fn a_draft_with_no_key_carries_a_fresh_one() {
    let rig = Rig::new(VERB, "key");
    let c = case(
        "draft, a fresh key",
        &["draft", "m_1", "--to", "boss@x.example", "--body", "hi"],
        &[],
        vec![ok(r#"{"ok":true,"draftId":"d_0006","hint":"draft saved"}"#)],
        owner_and_port,
        0,
        1,
    );
    rig.prepare(Side::Bash, &c);
    let bash = rig.run(true, &c);
    let sent = rig.read(Side::Bash, false, c.name);
    assert_eq!(sent.len(), 1, "the shell sent one save");
    let bash_req = normalise(&sent[0], false, rig.noun(), c.name);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(log.len(), 1, "the binary sent one save");
    let (bk, nk) = (key_in(&bash_req), key_in(&log[0]));
    assert_fresh_shape(&bk, "the shell");
    assert_fresh_shape(&nk, "the binary");
    assert_ne!(bk, nk, "each run has its own key");
    let body = |r: &str, k: &str| {
        r.split_once("\n\n")
            .map(|(_, b)| b.replace(k, "<key>"))
            .expect("a body")
    };
    assert_eq!(body(&bash_req, &bk), body(&log[0], &nk), "the same save");
    assert_eq!(
        (bash.code, &bash.stdout, &bash.stderr),
        (out.code, &out.stdout, &out.stderr)
    );
    assert_eq!(
        out.stdout,
        format!(
            "draft saved\ndraft id: d_0006 ('{} mail draft delete d_0006' removes it)\n",
            k2_client::brand::CLI_NAME
        )
    );
    let (_, again) = rig.binary(&c, false);
    let k2 = key_in(&again[0]);
    assert_fresh_shape(&k2, "the binary, again");
    assert_ne!(k2, nk, "a second run has a new key");
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    let usage =
        |hint: String| format!("{{\"error\":{{\"code\":\"usage\",\"hint\":\"{hint}\"}}}}\n");
    let rows: Vec<(&'static [&'static str], String)> = vec![
        (
            &["draft", "delete"],
            usage(format!(
                "draft delete requires the draft id (d_…) — '{cli} mail drafts' lists yours"
            )),
        ),
        (
            &["draft", "delete", "d_1", "--x"],
            usage(format!(
                "unknown flag '--x' — usage: {cli} mail draft delete <draft-id> [--json]"
            )),
        ),
        (
            &["draft", "m_1", "--nope"],
            usage(format!("unknown flag '--nope' — usage: {cli} mail draft <message-id> (--body <text> | --body-file <f>) [--to <addr> | --reply-all] [--cc <addr>] [--bcc <addr>] [--attach <path>]  OR  {cli} mail draft --to <addr> --subject <s> (--body <text> | --body-file <f>) [--cc <addr>] [--bcc <addr>] [--attach <path>] [--from <linked>]  OR  {cli} mail draft delete <draft-id>")),
        ),
        (
            &["link", "relink"],
            usage(format!(
                "link relink requires the <address> ('{cli} mail link list' marks the ones that need it)"
            )),
        ),
        (
            &["link", "relink", "a@x", "--features", "mail"],
            usage(format!(
                "link relink takes only <address> [--json] (it keeps the link's features; add more with '{cli} mail link upgrade')"
            )),
        ),
    ];
    for (args, want) in rows {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!((out.code, out.stderr.as_str()), (Some(2), want.as_str()));
        assert!(log.is_empty(), "nothing sent");
    }
    let c = case(
        "brand, the draft id line",
        &[
            "draft",
            "m_1",
            "--body",
            "b",
            "--idempotency-key",
            "key-0022",
        ],
        &[],
        vec![ok(r#"{"ok":true,"draftId":"d_7","hint":"saved"}"#)],
        owner_and_port,
        0,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stdout,
        format!("saved\ndraft id: d_7 ('{cli} mail draft delete d_7' removes it)\n")
    );
    let c = case(
        "brand, the key to ask with",
        &[
            "draft",
            "m_1",
            "--body",
            "b",
            "--idempotency-key",
            "key-0023",
        ],
        &[("K2_PORT", &dead_port())],
        vec![],
        owner_only,
        2,
        0,
    );
    let (out, _) = rig.binary(&c, false);
    assert!(
        out.stderr.contains("--idempotency-key key-0023"),
        "{}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("k2 ") && !out.stderr.contains("k5 "),
        "{}",
        out.stderr
    );
    let c = case(
        "brand, the re-link fix line",
        &["link", "list"],
        &[],
        vec![ok(
            r#"{"ok":true,"inboxes":[{"address":"a@x.example","source":"linked","status":"connected","needsRelink":true}]}"#,
        )],
        owner_and_port,
        0,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert!(
        out.stdout.ends_with(&format!(
            "a@x.example: the provider refused the sign-in. A person runs '{cli} mail link relink a@x.example' (same inbox, grants and ids).\n"
        )),
        "{}",
        out.stdout
    );
}
