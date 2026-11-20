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
//! - A body above 128 KiB: the shell hands `M_BODY` to its Python in the
//!   environment, which Linux refuses for one string past 128 KiB (exit
//!   126, nothing sent); the binary sends it ([`a_large_body`]): the named
//!   difference `mail-body-large`.

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
        usage_case("send, no recipient", &["send", "--subject", "s", "--body", "b"]),
        usage_case("send, no subject", &["send", "a@x.example", "--body", "b"]),
        usage_case(
            "send, body and body file",
            &["send", "a@x.example", "--subject", "s", "--body", "b", "--body-file", "f"],
        ),
        usage_case(
            "send, no such body file",
            &["send", "a@x.example", "--subject", "s", "--body-file", "nope.txt"],
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
            &["send", "a@x.example", "--subject", "s", "--body", "b", "--from-name", " \t"],
        ),
        usage_case(
            "send, a timeout with no wait",
            &["send", "a@x.example", "--subject", "s", "--body", "b", "--timeout", "5"],
        ),
        usage_case(
            "send, a timeout of 0",
            &["send", "a@x.example", "--subject", "s", "--body", "b", "--wait", "--timeout", "0"],
        ),
        usage_case(
            "send, at and in",
            &["send", "a@x.example", "--subject", "s", "--body", "b", "--at", "x", "--in", "2h"],
        ),
        usage_case(
            "send, wait and in",
            &["send", "a@x.example", "--subject", "s", "--body", "b", "--wait", "--in", "2h"],
        ),
        usage_case("send, an unknown flag", &["send", "a@x.example", "--nope"]),
        usage_case("send, a value flag at the end", &["send", "a@x.example", "--cc"]),
        usage_case("reply, no id", &["reply", "--body", "b"]),
        usage_case("reply, a reply-to", &["reply", "m_1", "--reply-to", "x@y"]),
        usage_case("reply, two ids", &["reply", "m_1", "m_2"]),
        usage_case("reply, no body", &["reply", "m_1"]),
        usage_case("reply, an unknown flag", &["reply", "m_1", "-b"]),
        usage_case("forward-message, no id", &["forward-message", "--to", "a@x"]),
        usage_case("forward-message, no to", &["forward-message", "m_1"]),
        usage_case(
            "forward-message, both reply-to forms",
            &["forward-message", "m_1", "--to", "a@x", "--reply-to", "r@x", "--reply-to-original"],
        ),
        usage_case("forward-message, two ids", &["forward-message", "m_1", "m_2"]),
        usage_case(
            "forward-message, a blank display name",
            &["forward-message", "m_1", "--to", "a@x", "--from-name", ""],
        ),
        usage_case(
            "draft, an id and a recipient",
            &["draft", "m_1", "--to", "a@x", "--body", "b"],
        ),
        usage_case("draft, reply-all with no id", &["draft", "--reply-all", "--body", "b"]),
        usage_case("draft, an id and a from", &["draft", "m_1", "--from", "a@x", "--body", "b"]),
        usage_case("draft, compose with no to", &["draft", "--subject", "s", "--body", "b"]),
        usage_case("draft, compose with no subject", &["draft", "--to", "a@x", "--body", "b"]),
        usage_case("draft, two ids", &["draft", "m_1", "m_2"]),
        usage_case("draft, two reply-to ids", &["draft", "m_1", "--reply-to", "m_2"]),
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
            &["send", "a@x.example", "--subject", "Hi", "--body", "Hello é"],
            r#"{"ok":true,"queued":true,"id":"out_1","status":"pending"}"#,
        ),
        ok_case(
            "send, queued with the hint",
            &["send", "--to", "a@x.example", "--to", "b@x.example, c@x.example", "--subject", "Hi", "--body", "b"],
            r#"{"ok":true,"queued":true,"id":"out_1","hint":"queued; your human decides"}"#,
        ),
        ok_case(
            "send, every flag",
            &[
                "send", "a@x.example", "--subject", "S", "--body", "B", "--cc", "c@x.example",
                "--bcc", "d@x.example", "--from", "bot@acme.example", "--from-name", "Bot",
                "--reply-to", "r@x.example", "--attach", "a b.txt", "--attach", "c,d.txt",
                "--in", "2h",
            ],
            r#"{"ok":true,"id":"out_2","status":"submitted"}"#,
        ),
        // covers: mail send --json
        ok_case(
            "send, submitted with the hint, --json",
            &["send", "a@x.example", "--subject", "S", "--body", "B", "--at", "2030-01-01T00:00:00Z", "--json"],
            r#"{"ok":true,"id":"out_2","status":"scheduled","hint":"goes at 2030"}"#,
        ),
        ok_case(
            "send --wait, decided",
            &["send", "a@x.example", "--subject", "S", "--body", "B", "--wait", "--timeout", "0090"],
            r#"{"ok":true,"id":"out_3","timedOut":false,"status":"rejected","note":"not this one"}"#,
        ),
        ok_case(
            "send --wait, decided with a note on the status",
            &["send", "a@x.example", "--subject", "S", "--body", "B", "--wait"],
            r#"{"ok":true,"id":"out_3","timedOut":false,"statusNote":"approved and submitted"}"#,
        ),
        case(
            "send --wait, still queued",
            &["send", "a@x.example", "--subject", "S", "--body", "B", "--wait", "--timeout", "5"],
            &[],
            vec![ok(r#"{"ok":true,"id":"out_4","timedOut":true}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "send --wait, still queued, the daemon's hint",
            &["send", "a@x.example", "--subject", "S", "--body", "B", "--wait", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"id":"out_4","timedOut":true,"hint":"still waiting"}"#)],
            owner_and_port,
            2,
            1,
        ),
        with(ok_case(
            "send, a body file",
            &["send", "a@x.example", "--subject", "S", "--body-file", "body.txt"],
            r#"{"ok":true,"id":"out_5","status":"submitted"}"#,
        )),
        with(usage_case(
            "send, a body file of one newline",
            &["send", "a@x.example", "--subject", "S", "--body-file", "empty.txt"],
        )),
        with(usage_case(
            "send, a folder as the body file",
            &["send", "a@x.example", "--subject", "S", "--body-file", "adir"],
        )),
        // covers: mail reply
        ok_case(
            "reply",
            &["reply", "m_1", "--body", "thanks", "--reply-all", "--cc", "c@x.example", "--bcc", "d@x.example", "--from-name", "Ann", "--attach", "f.txt"],
            r#"{"ok":true,"queued":true,"id":"out_6"}"#,
        ),
        with(ok_case(
            "reply --wait, a body file",
            &["reply", "m_1", "--body-file", "body.txt", "--wait", "--timeout", "30"],
            r#"{"ok":true,"id":"out_7","timedOut":false,"status":"submitted"}"#,
        )),
        ok_case(
            "forward-message, every flag",
            &[
                "forward-message", "m_1", "--to", "a@x.example", "--cc", "c@x.example", "--bcc",
                "d@x.example", "--note", "fyi", "--as-eml", "--from", "bot@acme.example",
                "--from-name", "Bot", "--reply-to-original", "--wait", "--timeout", "60",
            ],
            r#"{"ok":true,"id":"out_8","timedOut":false,"status":"approved"}"#,
        ),
        // covers: mail forward-message
        // covers: mail forward-message --json
        ok_case(
            "forward-message, a reply-to",
            &["forward-message", "m_1", "--to", "a@x.example", "--reply-to", "r@x.example", "--json"],
            r#"{"ok":true,"id":"out_8","status":"submitted"}"#,
        ),
        // covers: mail draft
        ok_case(
            "draft, a reply",
            &["draft", "m_1", "--body", "draft text", "--reply-all", "--cc", "c@x.example", "--attach", "f.txt"],
            r#"{"ok":true,"folder":"Drafts","address":"me@globex.example"}"#,
        ),
        ok_case(
            "draft, a reply by --reply-to",
            &["draft", "--reply-to", "m_1", "m_1", "--body", "x"],
            r#"{"ok":true,"hint":"saved"}"#,
        ),
        // covers: mail draft --json
        with(ok_case(
            "draft, a compose",
            &["draft", "--to", "a@x.example", "--subject", "S", "--body-file", "body.txt", "--from", "me@globex.example", "--bcc", "b@x.example", "--json"],
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
            vec![answer(403, r#"{"error":{"code":"gated","hint":"agent sending is off"}}"#)],
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
            vec![answer(403, r#"{"error":{"code":"agent_send_path_only","hint":"use mail send"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an attachment secret",
            &["send", "a@x.example", "--subject", "S", "--body", "B", "--attach", ".env"],
            &[],
            vec![answer(400, r#"{"error":{"code":"attachment_secret","hint":"that file looks like a secret"}}"#)],
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
            &["draft", "m_1", "--body", "b"],
            &[],
            vec![answer(503, r#"{"state":"migrating"}"#), ok(r#"{"ok":true,"hint":"saved"}"#)],
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
/// (the front hands a help flag to the shell).
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    const LINES: &[&[&str]] = &[
        &["send", "a@x.example", "--subject", "S", "--body", "B", "-h"],
        &["send", "--help", "a@x.example", "--subject", "S", "--body", "B"],
        &["send", "a@x.example", "--subject", "-h", "--body", "B"],
        &["reply", "m_1", "--body", "b", "--help"],
        &["forward-message", "m_1", "--to", "a@x.example", "-h"],
        &["draft", "m_1", "--body", "b", "--help"],
        &["move", "m_1", "X", "-h"],
        &["flag", "--help", "m_1", "--read"],
        &["archive", "m_1", "-h"],
        &["delete", "m_1", "--help"],
        &["delete", "-h", "old@acme.example"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case("help, nothing sent", line, &[], vec![], owner_and_port, 0, 0),
        ));
    }
}

/// A body of 200 KiB. The shell hands it to its Python in `M_BODY=`, an
/// environment string, which Linux refuses past 128 KiB: `python3` never
/// starts (exit 126, `Argument list too long`) and nothing is sent. The
/// binary sends it once and prints the answer. Elsewhere (macOS: one
/// limit for the whole environment) both send it. The named difference
/// `mail-body-large`, for K2's sign-off.
#[test]
fn a_large_body() {
    let rig = Rig::new(VERB, "large");
    let big: &'static str = Box::leak("é and some words of the body ".repeat(7000).into_boxed_str());
    assert!(big.len() > 200 * 1024, "{} bytes", big.len());
    let args: &'static [&'static str] = Box::leak(Box::new([
        "send", "a@x.example", "--subject", "S", "--body", big,
    ]));
    let c = case(
        "a large body",
        args,
        &[],
        vec![ok(r#"{"ok":true,"queued":true,"id":"out_9"}"#)],
        owner_and_port,
        0,
        1,
    );
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
