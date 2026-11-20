//! `k5-cli thread` against the shell CLI's `cli/k2 thread`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - A write: the send-once probe, the form sent once by its key, the
//!   answer's lines, the rename and replay notes, the markdown hint (the
//!   first and the 10th), `--json`, a refusal, no answer.
//! - A read: the lines of each kind of message, `--json`, `--seq`, the
//!   empty Thread, an error, a 300 KiB Thread of thousands of messages,
//!   no answer.
//! - The sender lookup when `--from` names none.
//! - `ask`, `secret`, `withdraw`, `dismiss`, `delete`, `read-all` and
//!   `unread`: their words, bodies, lines and exits.
//! - Every usage stop, the D6 line for a value flag last, and help that
//!   never acts.
//!
//! Every write names its key (`--idempotency-key`) so the requests match
//! byte for byte.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "thread";
const KEY: &str = "parity-key-0001";
const ON: &str = r#"{"version":"0","features":["send-once"]}"#;
const NUDGE: &str = ".k2/.thread-md-nudge";

/// The owner on disk and no markdown-hint count yet.
fn fresh(r: &Rig) {
    r.owner_on_disk();
    let _ = std::fs::remove_file(r.proj.join(NUDGE));
}

/// The count at 9: this write is the 10th.
fn ninth(r: &Rig) {
    r.owner_on_disk();
    std::fs::write(r.proj.join(NUDGE), " 9\n").unwrap();
}

fn plain(
    name: &'static str,
    args: &'static [&'static str],
    replies: Vec<Reply>,
    code: i32,
    requests: usize,
) -> Case {
    Case {
        env: vec![("K2_PORT", PORT.to_string())],
        ..case(name, args, &[], replies, fresh, code, requests)
    }
}

fn leak(words: Vec<String>) -> &'static [&'static str] {
    let v: Vec<&'static str> = words
        .into_iter()
        .map(|w| &*Box::leak(w.into_boxed_str()))
        .collect();
    Box::leak(v.into_boxed_slice())
}

const POSTED: &str = r#"{"ok":true,"id":"m1","seq":4,"from":"me","attachments":[{"path":"a.rs","line":3,"lineEnd":9,"size":2048}]}"#;

#[test]
fn thread_writes_as_the_shell_does() {
    let rig = Rig::new(VERB, "write");
    let mut tenth = plain(
        "the 10th write's hint",
        &["bob", "hi", "--from", "me", "--idempotency-key", KEY],
        vec![ok(ON), ok(POSTED)],
        0,
        2,
    );
    tenth.setup = ninth;
    for c in [
        // covers: thread
        plain(
            "a write",
            &["bob", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), ok(POSTED)],
            0,
            2,
        ),
        tenth,
        // covers: thread --json
        plain(
            "a write --json",
            &[
                "bob",
                "hi",
                "--from",
                "me",
                "--json",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON), ok(POSTED)],
            0,
            2,
        ),
        plain(
            "a write with files",
            &[
                "bob",
                "--file",
                "src/x.rs:3-9",
                "--file",
                "gone.rs",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON), ok(POSTED)],
            0,
            2,
        ),
        plain(
            "the rename and replay notes",
            &["bob", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![
                ok(ON),
                ok(
                    r#"{"ok":true,"id":"m1","seq":1,"from":"me","movedFrom":"bob","addr":"robert","replayed":true}"#,
                ),
            ],
            0,
            2,
        ),
        plain(
            "refused",
            &["bob", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![
                ok(ON),
                Reply::Answer(
                    403,
                    r#"{"ok":false,"error":{"code":"forbidden","hint":"not yours"}}"#.into(),
                ),
            ],
            2,
            2,
        ),
        plain(
            "a permission card",
            &["bob", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Answer(403, PERMISSION_ASK.into())],
            2,
            2,
        ),
        plain(
            "not JSON",
            &["bob", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Answer(502, "<html>".into())],
            0,
            2,
        ),
        plain(
            "a bad key",
            &["bob", "hi", "--from", "me", "--idempotency-key", "x"],
            vec![],
            2,
            0,
        ),
        plain(
            "the sender's handle",
            &["bob", "hi", "--idempotency-key", KEY],
            vec![ok(r#"{"handle":" ada "}"#), ok(ON), ok(POSTED)],
            0,
            3,
        ),
        plain(
            "no handle: k2",
            &["bob", "hi", "--idempotency-key", KEY],
            vec![Reply::Answer(500, "{}".into()), ok(ON), ok(POSTED)],
            0,
            3,
        ),
        plain(
            "11 files",
            leak(
                std::iter::once("bob".to_string())
                    .chain((0..11).flat_map(|i| ["--file".to_string(), format!("f{i}")]))
                    .chain(["--from".into(), "me".into()])
                    .collect(),
            ),
            vec![],
            1,
            0,
        ),
    ] {
        rig.check(&c);
    }
}

const READ: &str = r#"{"ok":true,"items":[{"seq":1,"doc":{"from":"ada","body":"a\r\nb"}},{"seq":2,"doc":{"deleted":true,"from":"bob","author":{"kind":"guest","name":"g"},"context":{"placement":"p","text":"hé"},"attachments":[{"state":"hidden"},{"path":"x","size":3000000,"state":"changed","line":2}]}}],"has_more":false}"#;

#[test]
fn thread_reads_as_the_shell_does() {
    let rig = Rig::new(VERB, "read");
    let big = format!(
        r#"{{"ok":true,"items":[{}]}}"#,
        (1..=4000)
            .map(|i| format!(r#"{{"seq":{i},"doc":{{"from":"ada","body":"message number {i} of the long Thread"}}}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(big.len() > 256 * 1024);
    for c in [
        plain("a read", &["bob", "--from", "me"], vec![ok(READ)], 0, 1),
        plain(
            "a read --json",
            &["bob", "--from", "me", "--json", "--limit", "5"],
            vec![ok(READ)],
            0,
            1,
        ),
        plain(
            "--all and --before-seq",
            &["bob", "--from", "me", "--all", "--before-seq", "9"],
            vec![ok(READ)],
            0,
            1,
        ),
        plain(
            "--seq",
            &["bob", "--from", "me", "--seq", "2"],
            vec![ok(READ)],
            0,
            1,
        ),
        plain(
            "--seq --json",
            &["bob", "--from", "me", "--seq", "1", "--json"],
            vec![ok(READ)],
            0,
            1,
        ),
        plain(
            "--seq not there",
            &["bob", "--from", "me", "--seq", "7"],
            vec![ok(READ)],
            1,
            1,
        ),
        plain(
            "empty",
            &["bob", "--from", "me"],
            vec![ok(r#"{"ok":true,"items":[]}"#)],
            0,
            1,
        ),
        plain(
            "an error",
            &["bob", "--from", "me"],
            vec![Reply::Answer(
                404,
                r#"{"ok":false,"error":{"code":"not_found","hint":"no such Thread"}}"#.into(),
            )],
            2,
            1,
        ),
        plain(
            "an error --seq --json",
            &["bob", "--from", "me", "--seq", "3", "--json"],
            vec![Reply::Answer(404, r#"{"ok":false}"#.into())],
            2,
            1,
        ),
        plain(
            "a list Python can't read",
            &["bob", "--from", "me"],
            vec![ok("[1,2]")],
            1,
            1,
        ),
        plain(
            "a 300 KiB Thread",
            &["bob", "--from", "me"],
            vec![ok(&big)],
            0,
            1,
        ),
        plain(
            "a 300 KiB Thread --json",
            &["bob", "--from", "me", "--json"],
            vec![ok(&big)],
            0,
            1,
        ),
    ] {
        rig.check(&c);
    }
    let mut c = plain(
        "no answer",
        &["bob", "--from", "me"],
        vec![Reply::Close],
        1,
        1,
    );
    c.compare = Compare::Waited;
    rig.check(&c);
}

#[test]
fn thread_cards_as_the_shell_does() {
    let rig = Rig::new(VERB, "cards");
    for c in [
        plain(
            "ask",
            &[
                "ask",
                "bob",
                "Pick one",
                "--option",
                "A (x, y)",
                "--option",
                "B",
                "--from",
                "me",
                "--expires",
                "2h",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                ok(r#"{"ok":true,"id":"c1","seq":9,"hint":"kept (x, y) whole"}"#),
            ],
            0,
            2,
        ),
        plain(
            "ask --options --json",
            &[
                "ask",
                "bob",
                "Pick",
                "--options",
                "a,b",
                "--allow-custom",
                "--no-expiry",
                "--from",
                "me",
                "--json",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON), ok(r#"{"ok":true,"id":"c1","hint":"h"}"#)],
            0,
            2,
        ),
        plain(
            "ask refused",
            &[
                "ask",
                "bob",
                "P",
                "--option",
                "a",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                Reply::Answer(403, r#"{"ok":false,"error":"gated"}"#.into()),
            ],
            2,
            2,
        ),
        plain(
            "secret",
            &[
                "secret",
                "bob",
                "--name",
                "API",
                "--for",
                "a.com",
                "--ttl",
                "7d",
                "--once",
                "--inject-header",
                "X-Key",
                "--from",
                "me",
            ],
            vec![ok(r#"{"ok":true,"id":"c2","sends":"API to a.com"}"#)],
            0,
            1,
        ),
        plain(
            "secret --json drops a value",
            &["secret", "bob", "--name", "API", "--from", "me", "--json"],
            vec![ok(r#"{"ok":true,"id":"c2","value":"hunter2"}"#)],
            0,
            1,
        ),
        plain(
            "withdraw",
            &["withdraw", "bob", "c1", "--why", "done"],
            vec![ok(r#"{"ok":true,"id":"c1","closedAs":"withdrawn"}"#)],
            0,
            1,
        ),
        plain(
            "dismiss, not yours",
            &["dismiss", "bob", "c1"],
            vec![Reply::Answer(
                403,
                r#"{"ok":false,"error":{"code":"card_not_yours","hint":"h"}}"#.into(),
            )],
            3,
            1,
        ),
        plain(
            "void --json",
            &["void", "bob", "c1", "--json"],
            vec![ok(r#"{"ok":true,"id":"c1","status":"voided"}"#)],
            0,
            1,
        ),
        plain(
            "delete by number",
            &["delete", "bob", "#3"],
            vec![ok(r#"{"ok":true,"seq":3,"id":"m3"}"#)],
            0,
            1,
        ),
        plain(
            "delete --json, not found",
            &["delete", "bob", "m_9", "--json"],
            vec![Reply::Answer(
                404,
                r#"{"ok":false,"error":{"code":"not_found"}}"#.into(),
            )],
            4,
            1,
        ),
        plain(
            "delete, an agent",
            &["delete", "bob", "2"],
            vec![Reply::Answer(
                403,
                r#"{"ok":false,"error":{"code":"person_only","hint":"a person deletes"}}"#.into(),
            )],
            3,
            1,
        ),
        plain(
            "read-all",
            &["read-all", "bob"],
            vec![ok(
                r#"{"ok":true,"marked":["a","b"],"unread":0,"waiting":1}"#,
            )],
            0,
            1,
        ),
        plain(
            "unread",
            &["unread", "--kind", "thread"],
            vec![ok(
                r#"{"ok":true,"person":"ada","total":120,"waitingTotal":1,"byConversation":[{"kind":"thread","id":"c","unread":120,"waiting":1}]}"#,
            )],
            0,
            1,
        ),
        plain(
            "unread --json",
            &["unread", "--json"],
            vec![ok(r#"{"ok":true}"#)],
            0,
            1,
        ),
        plain(
            "unread, an agent",
            &["unread"],
            vec![Reply::Answer(
                403,
                r#"{"ok":false,"error":{"code":"person_only"}}"#.into(),
            )],
            2,
            1,
        ),
    ] {
        rig.check(&c);
    }
}

#[test]
fn thread_stops_before_it_sends() {
    let rig = Rig::new(VERB, "words");
    for (name, args, code) in [
        ("no address", &[][..], 1),
        ("post", &["post", "bob", "x"], 1),
        ("an unknown flag", &["bob", "--nope"], 1),
        ("--seq 0", &["bob", "--seq", "0"], 1),
        ("--seq with text", &["bob", "t", "--seq", "2"], 1),
        ("--seq last", &["bob", "--seq"], 1),
        ("--file empty", &["bob", "--file", ""], 1),
        (
            "--idempotency-key last",
            &["bob", "t", "--idempotency-key"],
            2,
        ),
        ("ask, no prompt", &["ask", "bob"], 1),
        ("ask, no options", &["ask", "bob", "p"], 1),
        (
            "ask, both",
            &["ask", "bob", "p", "--option", "a", "--options", "b"],
            1,
        ),
        ("ask --wait", &["ask", "bob", "p", "--wait"], 1),
        ("ask --style", &["ask", "bob", "p", "--style", "x"], 1),
        (
            "ask, both expiries",
            &[
                "ask",
                "bob",
                "p",
                "--option",
                "a",
                "--expires",
                "1h",
                "--no-expiry",
            ],
            1,
        ),
        ("ask --from last", &["ask", "bob", "p", "--from"], 1),
        ("secret, no name", &["secret", "bob"], 1),
        (
            "secret --dest",
            &["secret", "bob", "--name", "A", "--dest", "env"],
            1,
        ),
        (
            "secret, both injects",
            &[
                "secret",
                "bob",
                "--name",
                "A",
                "--inject-dst",
                "q",
                "--inject-header",
                "H",
            ],
            1,
        ),
        ("secret --kind last", &["secret", "bob", "--kind"], 1),
        ("withdraw, no card", &["withdraw", "bob"], 2),
        ("dismiss --why last", &["dismiss", "bob", "c", "--why"], 2),
        ("void, extra", &["void", "bob", "c", "d"], 2),
        ("delete, no which", &["delete", "bob"], 2),
        ("delete --x", &["delete", "bob", "--x"], 2),
        ("read-all, none", &["read-all"], 1),
        ("read-all, two", &["read-all", "a", "b"], 1),
        ("unread, a word", &["unread", "x"], 1),
    ] {
        rig.check(&plain(name, args, vec![], code, 0));
    }
}

/// A value flag as the last word: bash stops on `$2: unbound variable`
/// (exit 1); the binary prints one line and exits 2 (K2's D6, the named
/// difference `edge-usage-exit-2`). Neither sends anything.
#[test]
fn thread_value_flag_last_is_the_d6_line() {
    let rig = Rig::new(VERB, "d6");
    let cli = k2_client::brand::CLI_NAME;
    for (args, flag) in [
        (&["bob", "--from"][..], "--from"),
        (&["bob", "--limit"], "--limit"),
        (&["bob", "--before-seq"], "--before-seq"),
        (&["unread", "--kind"], "--kind"),
    ] {
        let words = leak(args.iter().map(|s| s.to_string()).collect());
        let name: &'static str = Box::leak(format!("{flag} last").into_boxed_str());
        let c = plain(name, words, vec![], 1, 0);
        rig.prepare(Side::Bash, &c);
        let b = rig.run(true, &c);
        if edge_usage_exit_2_proven() {
            assert_eq!(b.code, Some(1), "{name}: bash's exit: {b:?}");
            assert!(
                b.stderr.ends_with(": $2: unbound variable\n"),
                "{name}: bash's stderr: {b:?}"
            );
            assert!(
                rig.read(Side::Bash, false, name).is_empty(),
                "{name}: bash sent"
            );
        }
        let (r, sent) = rig.binary(&c, false);
        assert!(sent.is_empty(), "{name}: the binary sent {sent:?}");
        assert_eq!((r.code, r.signal), (Some(2), None), "{name}: {r:?}");
        assert_eq!(
            r.stderr,
            format!("Error: {flag} needs a value (try '{cli} thread --help')\n"),
            "{name}"
        );
    }
}

/// Help never acts: `-h` or `--help` anywhere on a line that changes
/// something prints help and sends nothing.
#[test]
fn thread_help_never_acts() {
    let rig = Rig::new(VERB, "help");
    for args in [
        &["bob", "hi", "--help"][..],
        &["bob", "-h", "hi"],
        &["ask", "bob", "p", "--option", "a", "-h"],
        &["secret", "bob", "--name", "A", "--help"],
        &["secret", "-h"],
        &["withdraw", "bob", "c1", "--help"],
        &["dismiss", "-h", "bob", "c1"],
        &["delete", "bob", "3", "--help"],
        &["read-all", "bob", "-h"],
    ] {
        rig.check(&plain("help", args, vec![], 0, 0));
    }
}
