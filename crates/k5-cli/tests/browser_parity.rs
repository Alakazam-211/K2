//! `k5-cli browser` against the shell CLI's `cli/k2 browser`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - `note`: delivered, posted but not delivered, `--json`, the page's
//!   payload from an HTML file (bad bytes, `\r\n`, more than 64 KiB), the
//!   daemon's error (an object, a message, not ok), not JSON, an answer
//!   Python can't `.get` (a raise); the words' stops (help anywhere, a
//!   value flag last, an unknown flag, two note texts, no `--url`, a bad
//!   intent, an HTML file that can't be read).
//! - `redeliver`: sent, no id, an unknown flag.
//! - `browser` alone, `help`, an unknown verb.
//! - The cell socket with a passport, no passport in a session (the id
//!   tool), no answer (a dead port: curl 7; an empty reply: the line), a
//!   POST with no answer on the socket (sent once), the gate, the
//!   bulletin, a refusal's closing line.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "browser";
const SENT: &str = r#"{"ok":true,"delivered":true,"addr":"proj","threadItemId":"ti-1"}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn html_file(r: &Rig) {
    owner_and_port(r);
    let mut bytes = b"<p>a\r\nb\rc \xff</p>".to_vec();
    bytes.extend(std::iter::repeat_n(b'x', 70 * 1024));
    std::fs::write(r.proj.join("page.html"), bytes).unwrap();
}

#[test]
fn browser_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: browser note
        case(
            "note, delivered",
            &["note", "--url", "http://x/a b", "make it é", "--title", "T"],
            &[],
            vec![ok(SENT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "note, not delivered",
            &["note", "c", "--url", "u", "-w", "other"],
            &[],
            vec![ok(r#"{"ok":true,"addr":"other","threadItemId":"ti-2"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: browser note --json
        case(
            "note --json",
            &["note", "c", "--url", "u", "--json", "--intent", "question"],
            &[],
            vec![ok(r#"{"ok":true,"delivered":false,"x":"é"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "note with a selector and an HTML file",
            &[
                "note",
                "c",
                "--url",
                "u",
                "--selector",
                "#a",
                "--html-file",
                "page.html",
                "--text",
                "t",
            ],
            &[],
            vec![ok(SENT)],
            html_file,
            0,
            1,
        ),
        case(
            "the daemon's error object",
            &["note", "c", "--url", "u"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"not_yours","hint":"Only your own workspace."}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a message wins",
            &["note", "c", "--url", "u"],
            &[],
            vec![ok(r#"{"ok":false,"message":"login needed","error":"x"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not ok, nothing said",
            &["note", "c", "--url", "u"],
            &[],
            vec![ok(r#"{"ok":false}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["note", "c", "--url", "u"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer",
            &["note", "c", "--url", "u"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "redeliver",
            &["redeliver", "ti-1", "--workspace", "w"],
            &[],
            vec![ok(SENT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bulletin",
            &["redeliver", "ti-1"],
            &[],
            vec![ok(
                r#"{"ok":true,"delivered":true,"addr":"a","threadItemId":"t","bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal's closing line",
            &["redeliver", "ti-1"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "browser alone: help",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case(
            "an unknown verb",
            &["zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "unknown verb with --help",
            &["zap", "--help"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "note --help anywhere",
            &["note", "--url", "u", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "a value flag last",
            &["note", "c", "--url"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "redeliver -w last",
            &["redeliver", "t", "-w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["note", "--zz"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "two note texts",
            &["note", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no --url",
            &["note", "a"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad intent",
            &["note", "a", "--url", "u", "--intent", "maybe"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an HTML file that can't be read",
            &["note", "a", "--url", "u", "--html-file", "nope.html"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "redeliver with no id",
            &["redeliver", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "redeliver, an unknown flag",
            &["redeliver", "--zz"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "the gate",
            &["note", "a", "--url", "u"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a passport on the cell socket",
                &["note", "a", "--url", "u"],
                &[],
                vec![ok(SENT)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{sock}".to_string())],
            case(
                "a session with no passport",
                &["note", "a", "--url", "u"],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ),
        with(
            vec![("K2_PORT", dead_port())],
            case(
                "a dead port",
                &["note", "a", "--url", "u"],
                &[],
                vec![],
                owner_only,
                7,
                0,
            ),
        ),
        compare_waited(case(
            "an empty reply",
            &["note", "a", "--url", "u"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        compare_waited(with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "no answer on the socket: never sent again",
                &["note", "a", "--url", "u"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
    rig.check(&Case {
        compare: Compare::Traceback,
        ..case(
            "an answer that is a list",
            &["note", "a", "--url", "u"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )
    });
}

fn compare_waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}
