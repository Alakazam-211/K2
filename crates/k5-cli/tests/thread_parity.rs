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
//! - Every usage stop, the shell's line for a value flag last, and help that
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
        // thread-reply-to-v1: `replyTo=N` (the `#` off, zeros folded)
        // after the key, and the daemon's refusal of an N not on the Thread.
        plain(
            "a reply to a card",
            &[
                "bob",
                "--reply-to",
                "#013664",
                "see this",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                ok(r#"{"ok":true,"id":"m2","seq":5,"from":"me","replyTo":{"seq":13664,"cardId":"c1"}}"#),
            ],
            0,
            2,
        ),
        plain(
            "a reply to part of a message",
            &[
                "bob",
                "yes, that",
                "--reply-to",
                "4",
                "--reply-quote",
                "the contrast pass",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                ok(r#"{"ok":true,"id":"m3","seq":6,"from":"me","replyTo":{"seq":4,"part":true,"quote":"the contrast pass"}}"#),
            ],
            0,
            2,
        ),
        plain(
            "a reply to part, words not in the message",
            &[
                "bob",
                "hm",
                "--reply-to",
                "4",
                "--reply-quote",
                "not there",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                Reply::Answer(
                    400,
                    r#"{"ok":false,"error":{"code":"reply_quote_not_in_target","hint":"replyQuote must be words from message #4 (as it reads in the Thread); these aren't in it"}}"#.into(),
                ),
            ],
            2,
            2,
        ),
        plain(
            "a reply to a message not on the Thread",
            &[
                "bob",
                "hi",
                "--reply-to",
                "77",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                Reply::Answer(
                    404,
                    r#"{"ok":false,"error":{"code":"reply_not_found","hint":"no message or card #77 in this Thread (a read lists them as [N])"}}"#.into(),
                ),
            ],
            2,
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

/// A card's `--options` goes as typed (the daemon splits it on `;`
/// only). A value with a comma and no `;` prints one note, exactly, and
/// the daemon's `hint` with the same words is never printed again; no
/// note for `a; b`, a value with no comma, a JSON list or `--option`.
#[test]
fn thread_ask_options_split_on_semicolons() {
    const NOTE_HINT: &str = r#"{"ok":true,"id":"c1","seq":9,"hint":"--options splits on ';'; this became one option. Use ';' between options, or repeat --option"}"#;
    const CARD: &str = r#"{"ok":true,"id":"c1","seq":9}"#;
    const NO: &str = "No, the forward clean-up is enough";
    const YES: &str = "Yes, rewrite the history too";
    let rig = Rig::new(VERB, "optsemi");
    let ask = |name: &'static str, opts: &[&str], reply: &'static str| {
        let words: Vec<String> = ["ask", "bob", "Rewrite the history?"]
            .iter()
            .chain(opts)
            .chain(&["--from", "me", "--idempotency-key", KEY])
            .map(|w| w.to_string())
            .collect();
        plain(name, leak(words), vec![ok(ON), ok(reply)], 0, 2)
    };
    let commas = format!("{NO}, {YES}");
    let semi = format!("{NO}; {YES}");
    for c in [
        ask(
            "commas, no ';': the note once",
            &["--options", &commas],
            NOTE_HINT,
        ),
        ask("'a; b': no note", &["--options", &semi], CARD),
        ask("no comma: no note", &["--options", "Done"], CARD),
        ask(
            "a JSON list: no note",
            &["--options", r#" ["Yes, now","No"]"#],
            CARD,
        ),
        ask(
            "--json: the note on stderr",
            &["--options", "Yes, now", "--json"],
            NOTE_HINT,
        ),
        ask(
            "repeated --option: no note",
            &["--option", NO, "--option", YES],
            CARD,
        ),
    ] {
        rig.check(&c);
    }
}

/// `ask --meta key=value` and `--meta-json` (0.46.17): the merged map,
/// keys sorted, at the end of the body; `--json` passes the answer's map
/// through; an empty map sends the body it always did; at the limits it is
/// sent. Every refusal is in [`thread_stops_before_it_sends`].
#[test]
fn thread_ask_meta_as_the_shell_does() {
    const CARD: &str = r#"{"ok":true,"id":"c1","seq":9,"meta":{"order":"A-17","qty":3}}"#;
    let rig = Rig::new(VERB, "meta");
    let ask = |name: &'static str, extra: Vec<String>| {
        let words: Vec<String> = ["ask", "bob", "Ship?", "--option", "Go"]
            .iter()
            .map(|w| w.to_string())
            .chain(extra)
            .chain(
                ["--from", "me", "--idempotency-key", KEY]
                    .iter()
                    .map(|w| w.to_string()),
            )
            .collect();
        plain(name, leak(words), vec![ok(ON), ok(CARD)], 0, 2)
    };
    let v = |w: &[&str]| -> Vec<String> { w.iter().map(|x| x.to_string()).collect() };
    let keys32 = format!(
        "{{{}}}",
        (0..32)
            .map(|i| format!("\"k{i}\": {i}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for c in [
        ask(
            "--meta strings",
            v(&["--meta", "order=A-17", "--meta", "note=a=b c"]),
        ),
        ask(
            "--meta-json numbers and booleans",
            v(&[
                "--meta-json",
                r#"{"rush": true, "qty": 3, "w": 1.5, "big": 1e16, "who": "ad\u00e9"}"#,
            ]),
        ),
        ask(
            "both, merged and sorted",
            v(&["--meta-json", r#"{"qty": 3}"#, "--meta", "order=A-17"]),
        ),
        ask(
            "--json passes meta through",
            v(&["--meta", "order=A-17", "--json"]),
        ),
        ask("an empty map", v(&["--meta-json", "{}"])),
        ask("32 keys", vec!["--meta-json".to_string(), keys32]),
        ask(
            "a 64-byte key",
            vec!["--meta".to_string(), format!("{}=x", "k".repeat(64))],
        ),
        ask(
            "4096 bytes",
            vec!["--meta".to_string(), format!("k={}", "v".repeat(4088))],
        ),
        ask(
            "a key repeated inside --meta-json",
            v(&["--meta-json", r#"{"a": 1, "b": 2, "a": 3}"#]),
        ),
    ] {
        rig.check(&c);
    }
    // The refusals that need long words: exit 2, nothing sent. A lone
    // surrogate is the shell's own (Python fails on it, exit 1).
    let keys33 = format!(
        "{{{}}}",
        (0..33)
            .map(|i| format!("\"k{i}\": {i}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for (name, extra, code) in [
        ("33 keys", vec!["--meta-json".to_string(), keys33], 2),
        (
            "a 65-byte key",
            vec!["--meta".to_string(), format!("{}=x", "k".repeat(65))],
            2,
        ),
        (
            "4097 bytes",
            vec!["--meta".to_string(), format!("k={}", "v".repeat(4089))],
            2,
        ),
        (
            "a lone surrogate",
            v(&["--meta-json", r#"{"k": "\ud83d"}"#]),
            1,
        ),
    ] {
        let words: Vec<String> = ["ask", "bob", "Ship?", "--option", "Go"]
            .iter()
            .map(|w| w.to_string())
            .chain(extra)
            .collect();
        rig.check(&plain(name, leak(words), vec![], code, 0));
    }
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
        ("--reply-to last", &["bob", "t", "--reply-to"], 1),
        ("--reply-to 0", &["bob", "t", "--reply-to", "#0"], 1),
        ("--reply-to x", &["bob", "t", "--reply-to", "x"], 1),
        ("--reply-to, no words", &["bob", "--reply-to", "4"], 1),
        ("--reply-quote alone", &["bob", "t", "--reply-quote", "x"], 1),
        ("--reply-quote last", &["bob", "t", "--reply-to", "4", "--reply-quote"], 1),
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
        (
            "ask, both, commas: no note",
            &["ask", "bob", "p", "--option", "a", "--options", "b,c"],
            1,
        ),
        ("ask, --options last", &["ask", "bob", "p", "--options"], 1),
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
        (
            "ask --meta last",
            &["ask", "bob", "p", "--option", "a", "--meta"],
            2,
        ),
        (
            "ask --meta-json last",
            &["ask", "bob", "p", "--option", "a", "--meta-json"],
            2,
        ),
        (
            "ask --meta-json twice",
            &[
                "ask",
                "bob",
                "p",
                "--option",
                "a",
                "--meta-json",
                "{}",
                "--meta-json",
                "{}",
            ],
            2,
        ),
        (
            "ask --meta, no '='",
            &["ask", "bob", "p", "--option", "a", "--meta", "order"],
            2,
        ),
        (
            "ask, a key in both",
            &[
                "ask",
                "bob",
                "p",
                "--option",
                "a",
                "--meta-json",
                r#"{"order": "B"}"#,
                "--meta",
                "order=A",
            ],
            2,
        ),
        (
            "ask, a --meta key twice",
            &[
                "ask", "bob", "p", "--option", "a", "--meta", "a=1", "--meta", "a=2",
            ],
            2,
        ),
        (
            "ask --meta-json, not JSON",
            &["ask", "bob", "p", "--option", "a", "--meta-json", "order=A"],
            2,
        ),
        (
            "ask --meta-json, a list",
            &["ask", "bob", "p", "--option", "a", "--meta-json", "[1, 2]"],
            2,
        ),
        (
            "ask --meta-json, NaN",
            &[
                "ask",
                "bob",
                "p",
                "--option",
                "a",
                "--meta-json",
                r#"{"n": NaN}"#,
            ],
            2,
        ),
        (
            "ask --meta-json, out of range",
            &[
                "ask",
                "bob",
                "p",
                "--option",
                "a",
                "--meta-json",
                r#"{"n": -1e400}"#,
            ],
            2,
        ),
        (
            "ask --meta-json, a null value",
            &[
                "ask",
                "bob",
                "p",
                "--option",
                "a",
                "--meta-json",
                r#"{"k": null}"#,
            ],
            2,
        ),
        (
            "ask --meta-json, an object value",
            &[
                "ask",
                "bob",
                "p",
                "--option",
                "a",
                "--meta-json",
                r#"{"k": {"a": [1]}}"#,
            ],
            2,
        ),
        (
            "ask --meta, an upper-case key",
            &["ask", "bob", "p", "--option", "a", "--meta", "Order=A"],
            2,
        ),
        (
            "ask --meta, a key outside ASCII",
            &["ask", "bob", "p", "--option", "a", "--meta", "ordré=A"],
            2,
        ),
        (
            "ask --meta, an empty key",
            &["ask", "bob", "p", "--option", "a", "--meta", "=A"],
            2,
        ),
        (
            "ask --meta, a usage stop first",
            &["ask", "bob", "p", "--meta", "Order=A"],
            1,
        ),
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

/// A value flag as the last word: bash's `_need_value` and the binary
/// print the same line, "Error: <flag> takes a value", and exit 2.
/// Neither sends anything.
/// After a bare `--` the words are the address and the text (or the
/// prompt), as typed (`cmd_thread`'s and `cmd_thread_ask`'s `--)` arms);
/// the verb scan and the help scan stop there.
#[test]
fn thread_after_dashes_as_the_shell_does() {
    let rig = Rig::new(VERB, "dashes");
    const CARD: &str = r#"{"ok":true,"id":"c1","seq":9}"#;
    for c in [
        // covers: thread
        plain(
            "a write: the text --help after --",
            &[
                "--from",
                "me",
                "--idempotency-key",
                KEY,
                "--",
                "bob",
                "--help",
            ],
            vec![ok(ON), ok(POSTED)],
            0,
            2,
        ),
        plain(
            "a write: a text that starts with a dash",
            &[
                "bob",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
                "--",
                "-x --json",
            ],
            vec![ok(ON), ok(POSTED)],
            0,
            2,
        ),
        plain(
            "a verb after -- is the address",
            &["--from", "me", "--idempotency-key", KEY, "--", "ask", "-h"],
            vec![ok(ON), ok(POSTED)],
            0,
            2,
        ),
        plain("-- alone: no address", &["--"], vec![], 1, 0),
        plain(
            "ask: the prompt --help after --",
            &[
                "ask",
                "--option",
                "yes",
                "--option",
                "no",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
                "--",
                "bob",
                "--help",
            ],
            vec![ok(ON), ok(CARD)],
            0,
            2,
        ),
        plain(
            "ask: a word after the prompt is one too many",
            &["ask", "--option", "yes", "bob", "--", "why", "-x"],
            vec![],
            1,
            0,
        ),
        plain(
            "ask: -- with no prompt",
            &["ask", "--option", "yes", "--", "bob"],
            vec![],
            1,
            0,
        ),
        plain(
            "ask: help before --",
            &["ask", "bob", "-h", "--", "why"],
            vec![],
            0,
            0,
        ),
    ] {
        rig.check(&as_the_front_routes(VERB, c));
    }
}

#[test]
fn thread_value_flag_last_is_the_d6_line() {
    let rig = Rig::new(VERB, "d6");
    for (args, flag) in [
        (&["bob", "--from"][..], "--from"),
        (&["bob", "--limit"], "--limit"),
        (&["bob", "--before-seq"], "--before-seq"),
        (&["unread", "--kind"], "--kind"),
    ] {
        let words = leak(args.iter().map(|s| s.to_string()).collect());
        let name: &'static str = Box::leak(format!("{flag} last").into_boxed_str());
        let c = plain(name, words, vec![], 2, 0);
        rig.prepare(Side::Bash, &c);
        let b = rig.run(true, &c);
        assert_eq!(b.code, Some(2), "{name}: bash's exit: {b:?}");
        assert_eq!(
            b.stderr,
            format!("Error: {flag} takes a value\n"),
            "{name}: bash's stderr: {b:?}"
        );
        assert!(
            rig.read(Side::Bash, false, name).is_empty(),
            "{name}: bash sent"
        );
        let (r, sent) = rig.binary(&c, false);
        assert!(sent.is_empty(), "{name}: the binary sent {sent:?}");
        assert_eq!((r.code, r.signal), (Some(2), None), "{name}: {r:?}");
        assert_eq!(r.stderr, format!("Error: {flag} takes a value\n"), "{name}");
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
        &[
            "ask", "bob", "p", "--option", "a", "--meta", "k=v", "--help",
        ],
        &[
            "ask",
            "-h",
            "bob",
            "p",
            "--option",
            "a",
            "--meta-json",
            "{}",
        ],
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

/// `ask`'s usage names the command by the brand's name and shows `;`
/// (in a build with `K2_BRAND_DIR=brand/xy` it says `xy`).
#[test]
fn ask_usage_says_the_brand_s_name_and_semicolons() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = plain("brand", &["ask", "bob"], vec![], 1, 0);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!(
            "Error: prompt required\nUsage: {cli} thread ask <addr> \"<prompt>\" --option \"…\" --option \"…\"   (or --options \"a; b; c\")\n"
        )
    );
    assert_eq!(out.code, Some(1));
    assert!(log.is_empty(), "{log:?}");
}

/// A `--file` path that starts with `~name`: Python's `expanduser` reads
/// that user's folder from the password database (root's is there on
/// every machine), and a name with no entry stays as it is.
#[test]
fn thread_files_with_a_user_s_folder() {
    let rig = Rig::new(VERB, "tilde");
    for (name, args) in [
        // A probe of the native bar: `thread --file ~user/x`.
        (
            "~nobody/k5-bar",
            &[
                "w1",
                "hello",
                "--file",
                "~nobody/k5-bar",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ][..],
        ),
        (
            "~root/ and ~root/:3",
            &[
                "bob",
                "--file",
                "~root/",
                "--file",
                "~root/:3",
                "--file",
                "~k5-no-such-user-7171/a.rs:2-4",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
        ),
    ] {
        rig.check(&plain(name, args, vec![ok(ON), ok(POSTED)], 0, 2));
    }
}
