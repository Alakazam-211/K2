//! `k5-cli hostmail forward` against the shell CLI's `cli/k2 hostmail
//! forward`, on the parity rig ([`common::parity`]): the same requests
//! (urllib's, from `_mail_py`), stdout, stderr and exit code.
//!
//! - The words, as `cmd_hostmail_forward` and its helpers read them: the
//!   help with no words, each usage line, exit 2 and nothing sent.
//! - show, set, add, rm, unset, pause and resume on the admin routes, and
//!   list (its filters, the delivery-report warning, each row's words,
//!   none, `--json`, and one answer above 256 KiB).
//! - The refusals `fwd_die` words itself, the permission card, and no
//!   answer.
//! - Help never acts: each verb with `-h` or `--help` sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "hostmail";

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

const SHOW: &str = r#"{"ok":true,"address":"sales@acme.example","forwards":[
 {"to":"a@globex.example","keep":true,"kind":"external","state":"paused","pauseReason":"box_brake",
  "pauseWords":"too many were refused","last24h":{"forwarded":9,"refused":4},"setBy":"admin"},
 {"to":"b@acme.example","kind":"local","state":"active"}],
 "volume":{"copiesCounted":12,"copiesPerDay":200,"copiesPerHour":40},
 "events":[{"at":1700000000,"event":"paused","actor":"k2","detail":{"reason":"box_brake"}}]}"#;

const LIST: &str = r#"{"ok":true,"outcomes":{"ok":false,"error":"no bounce reader"},"forwards":[
 {"address":"sales@acme.example","to":"a@globex.example","keep":true,"kind":"external","state":"paused",
  "pauseReason":"not_told","pauseWords":"it was never told","cantArrive":"no front door","noticeWords":"told it today"},
 {"address":"info@acme.example","to":"b@acme.example","kind":"local","state":"active"},
 {"to":"c@acme.example"}]}"#;

/// A list answer above 256 KiB: thousands of forwards.
fn big_list() -> String {
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"address":"box{i}@acme.example","to":"dest{i}@globex.example","keep":{},"kind":"external","state":"paused","pauseReason":"manual","pauseWords":"paused by hand, number {i}"}}"#,
                i % 2 == 0
            )
        })
        .collect();
    format!(r#"{{"ok":true,"forwards":[{}]}}"#, rows.join(","))
}

#[test]
fn hostmail_forward_takes_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "fwords");
    let v = vec![
        case(
            "forward alone: the help",
            &["forward"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        usage_case("an unknown sub-command", &["forward", "move"]),
        usage_case("set, no address", &["forward", "set", "--to", "a@x"]),
        usage_case("set, no --to", &["forward", "set", "sales@acme.example"]),
        usage_case(
            "set, two addresses",
            &["forward", "set", "a@acme.example", "b@acme.example"],
        ),
        usage_case(
            "add, --address and a word",
            &["forward", "add", "--address", "a@acme.example", "b@acme.example"],
        ),
        usage_case(
            "rm, --no-keep",
            &["forward", "rm", "a@acme.example", "--to", "x@y", "--no-keep"],
        ),
        usage_case(
            "set, --keep twice then --no-keep",
            &["forward", "set", "a@acme.example", "--keep", "--keep", "--no-keep", "--to", "x"],
        ),
        usage_case("show, no address", &["forward", "show"]),
        usage_case("show, --to", &["forward", "show", "a@acme.example", "--to", "x"]),
        usage_case("unset, an unknown flag", &["forward", "unset", "a@acme.example", "--all"]),
        usage_case("pause, two addresses", &["forward", "pause", "a@x", "b@x"]),
        usage_case("resume, --to with no value", &["forward", "resume", "a@x", "--to"]),
        usage_case("show, --address with no value", &["forward", "show", "--address"]),
        usage_case("list, a word", &["forward", "list", "all"]),
        usage_case("list, an unknown flag", &["forward", "list", "--stopped"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn hostmail_forward_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "fanswer");
    let big = big_list();
    assert!(big.len() > 256 * 1024, "the large list is {} bytes", big.len());
    let v = vec![
        // covers: hostmail forward
        ok_case("show", &["forward", "show", "sales@acme.example"], SHOW),
        // covers: hostmail forward --json
        ok_case(
            "show --json",
            &["forward", "show", "--json", "sales@acme.example"],
            SHOW,
        ),
        ok_case(
            "set, two destinations",
            &[
                "forward",
                "set",
                "sales@acme.example",
                "--to",
                "a@globex.example",
                "--to",
                "b@acme.example",
                "--no-keep",
            ],
            r#"{"ok":true,"address":"sales@acme.example","destinations":[{"to":"a@globex.example","state":"pending_verify","message":"K2 emailed it a link"},{"to":"b@acme.example"}]}"#,
        ),
        ok_case(
            "add, --address, restart",
            &[
                "forward",
                "add",
                "--address",
                "sales@acme.example",
                "--to",
                "a@globex.example",
                "--restart-stopped",
                "--keep",
            ],
            r#"{"ok":true,"to":"a@globex.example","keep":true,"note":"restarted"}"#,
        ),
        ok_case(
            "rm",
            &["forward", "rm", "sales@acme.example", "--to", "a@globex.example"],
            r#"{"ok":true,"removed":["a@globex.example"],"remaining":[]}"#,
        ),
        ok_case(
            "unset",
            &["forward", "unset", "sales@acme.example"],
            r#"{"ok":true,"removed":true}"#,
        ),
        ok_case(
            "pause, one destination",
            &["forward", "pause", "sales@acme.example", "--to", "a@globex.example"],
            r#"{"ok":true,"forwards":[{"to":"a@globex.example"}]}"#,
        ),
        ok_case(
            "resume",
            &["forward", "resume", "sales@acme.example"],
            r#"{"ok":true,"state":"active","skipped":[{"to":"b@globex.example","why":"it said stop"}]}"#,
        ),
        ok_case("list", &["forward", "list"], LIST),
        ok_case(
            "list, paused and external",
            &["forward", "list", "--active", "--paused", "--external"],
            LIST,
        ),
        ok_case(
            "list, pending, none",
            &["forward", "list", "--pending"],
            r#"{"ok":true,"forwards":[],"outcomes":{"ok":true}}"#,
        ),
        ok_case("list --json", &["forward", "list", "--json"], LIST),
        ok_case("list, thousands of forwards", &["forward", "list"], &big),
        case(
            "owner only, on resume",
            &["forward", "resume", "sales@acme.example"],
            &[],
            vec![answer(
                403,
                r#"{"ok":false,"error":{"code":"owner_only","hint":"K2 paused this forward; the owner, an Admin or the mailbox's person resumes it"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an agent without the toggle",
            &["forward", "list"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a destination stopped",
            &["forward", "add", "sales@acme.example", "--to", "a@globex.example"],
            &[],
            vec![answer(
                409,
                r#"{"ok":false,"error":{"code":"destination_stopped","hint":"that address pressed Stop"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        compare(
            Compare::Traceback,
            case(
                "outcomes that are a list",
                &["forward", "list"],
                &[],
                vec![ok(r#"{"ok":true,"outcomes":[1]}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        case(
            "closed, a GET tries once more",
            &["forward", "list"],
            &[],
            vec![Reply::Close, Reply::Close],
            owner_and_port,
            1,
            2,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// The verbs that change something, with `-h` or `--help` anywhere: the
/// help, and nothing sent.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "fhelp");
    const LINES: &[&[&str]] = &[
        &["forward", "set", "a@x", "--to", "b@x", "-h"],
        &["forward", "add", "--help", "a@x", "--to", "b@x"],
        &["forward", "rm", "a@x", "--to", "b@x", "--help"],
        &["forward", "unset", "a@x", "-h"],
        &["forward", "pause", "-h", "a@x"],
        &["forward", "resume", "a@x", "--help"],
        &["forward", "list", "--help"],
        &["forward", "--help"],
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
