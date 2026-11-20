//! `k5-cli mail forward` against the shell CLI's `cli/k2 mail forward`, on
//! the parity rig ([`common::parity`]): the same requests (urllib's, from
//! `_mail_py`), stdout, stderr and exit code.
//!
//! - The words, as `cmd_mail_forward` and `_forward_set_parse` read them:
//!   the help with no words, each usage line, exit 2 and nothing sent.
//! - show (a paused forward with each reason's words, the notice, the
//!   counts, the volume and the events; one `forward`; none; `--json`),
//!   set and add (several `--to`, `--keep`, `--no-keep`,
//!   `--restart-stopped`, each destination's message and note), rm,
//!   unset, pause and resume (the rows, the skipped ones).
//! - The refusals `fwd_die` words itself (person only, the codes whose
//!   hint is the daemon's, owner only on resume), the permission card,
//!   answers the Python raises on, and no answer.
//! - Help never acts: each verb with `-h` or `--help` sends nothing.

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

fn refused(name: &'static str, args: &'static [&'static str], status: u16, body: &str, code: i32) -> Case {
    case(name, args, &[], vec![answer(status, body)], owner_and_port, code, 1)
}

fn traceback(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    compare(
        Compare::Traceback,
        case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1),
    )
}

/// A forward in each state the printer words differently.
const SHOW: &str = r#"{"ok":true,"address":"me@acme.example","forwards":[
 {"to":"a@globex.example","keep":true,"kind":"external","state":"paused","pauseReason":"not_told",
  "pauseWords":"the address was never told","heldBy":"owner","cantArrive":"it can't arrive yet",
  "notice":"sent","noticeAt":1700000000,"last24h":{"forwarded":3,"delivered":2,"refused":1},"setBy":"owner"},
 {"to":"b@globex.example","kind":"external","state":"paused","pauseReason":"upgrade_required"},
 {"to":"c@acme.example","keep":false,"kind":"local","state":"paused","pauseReason":"agent_send_off","pauseWords":"agent sending is off",
  "noticeWords":"told it on Monday"},
 {"to":"d@globex.example","kind":"external","state":"paused","pauseReason":"revoked","pauseWords":"it said stop","last24h":{}},
 {"to":"e@acme.example","kind":"local","state":"paused","pauseReason":"air_gap","pauseWords":"the box is air-gapped"},
 {"to":"f@acme.example","kind":"local","state":"paused","pauseReason":"box_brake","pauseWords":"the box braked","last24h":{"refused":2}},
 {"to":"g@acme.example","kind":"local","state":"paused","pauseReason":"manual","pauseWords":"someone paused it"},
 {"to":"h@acme.example","kind":"local","state":"paused","pauseReason":"other","pauseWords":"a reason"},
 {"kind":"local","state":"active","notice":"sent","noticeAt":"1700000000"}],
 "volume":{"copiesCounted":7,"copiesPerDay":200},
 "events":[{"at":1700000000,"event":"paused","actor":"k2","detail":{"reason":"not_told"}},
  {"at":"x","event":"resumed"},{"detail":"text"},{"at":1700000100,"event":"a-long-event-name","actor":"owner"},
  {"at":1},{"at":2},{"at":3}]}"#;

#[test]
fn mail_forward_takes_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "words");
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
        usage_case("an unknown sub-command", &["forward", "list"]),
        usage_case("set, no --to", &["forward", "set"]),
        usage_case("set, --to with no value", &["forward", "set", "--to"]),
        usage_case("set, an empty --to", &["forward", "set", "--to", ""]),
        usage_case(
            "set, --keep and --no-keep",
            &["forward", "set", "--to", "a@x", "--keep", "--no-keep"],
        ),
        usage_case(
            "add, --no-keep and --keep",
            &["forward", "add", "--no-keep", "--keep", "--to", "a@x"],
        ),
        usage_case("rm, --keep", &["forward", "rm", "--to", "a@x", "--keep"]),
        usage_case(
            "rm, --restart-stopped",
            &["forward", "rm", "--restart-stopped", "--to", "a@x"],
        ),
        usage_case("set, --address with no value", &["forward", "set", "--address"]),
        usage_case("set, a word", &["forward", "set", "me@acme.example"]),
        usage_case("add, an unknown flag", &["forward", "add", "--all"]),
        usage_case("rm, a dash alone", &["forward", "rm", "-"]),
        usage_case("show, --to", &["forward", "show", "--to", "a@x"]),
        usage_case("unset, --to", &["forward", "unset", "--to"]),
        usage_case("pause, --to with no value", &["forward", "pause", "--to"]),
        usage_case("show, a word", &["forward", "show", "me@acme.example"]),
        usage_case("resume, an unknown flag", &["forward", "resume", "--all"]),
        usage_case("unset, --address with no value", &["forward", "unset", "--address"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_forward_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "answer");
    let v = vec![
        // covers: mail forward
        ok_case("show", &["forward", "show"], SHOW),
        // covers: mail forward --json
        ok_case("show --json", &["forward", "show", "--json"], SHOW),
        ok_case(
            "show, one forward, an address asked",
            &["forward", "show", "--address", "me@acme.example"],
            r#"{"ok":true,"forward":{"to":"a@globex.example","state":"active","kind":"external"}}"#,
        ),
        ok_case(
            "show, none",
            &["forward", "show", "--address", "me@acme.example"],
            r#"{"ok":true,"forwards":[]}"#,
        ),
        ok_case(
            "show, no forward at all",
            &["forward", "show"],
            r#"{"ok":true,"forward":null}"#,
        ),
        ok_case(
            "set, two destinations",
            &[
                "forward",
                "set",
                "--to",
                "a@globex.example",
                "--to",
                "b@globex.example",
                "--keep",
                "--restart-stopped",
                "--address",
                "me@acme.example",
            ],
            r#"{"ok":true,"address":"me@acme.example","note":"both told","destinations":[
              {"to":"a@globex.example","keep":true,"state":"pending_verify","message":"K2 emailed it a link","note":"both told"},
              {"to":"b@globex.example","keep":false,"note":"this one is new"}]}"#,
        ),
        ok_case(
            "add, one destination, no copy",
            &["forward", "add", "--to", "a@acme.example", "--no-keep", "--json"],
            r#"{"ok":true,"to":"a@acme.example","keep":false}"#,
        ),
        ok_case(
            "add, the answer is the destination",
            &["forward", "add", "--to", "a@acme.example", "--no-keep"],
            r#"{"ok":true,"to":"a@acme.example","keep":false,"state":"active","message":"done"}"#,
        ),
        ok_case(
            "rm",
            &["forward", "rm", "--to", "a@globex.example", "--to", "b@globex.example"],
            r#"{"ok":true,"address":"me@acme.example","removed":["a@globex.example"],"remaining":["c@acme.example","d@acme.example"],"note":"one left"}"#,
        ),
        ok_case(
            "rm, nothing left, the words given",
            &["forward", "rm", "--to", "a@globex.example"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "unset",
            &["forward", "unset"],
            r#"{"ok":true,"address":"me@acme.example"}"#,
        ),
        ok_case(
            "unset, nothing to remove",
            &["forward", "unset", "--address", "me@acme.example"],
            r#"{"ok":true,"removed":false,"note":"nothing was set"}"#,
        ),
        ok_case(
            "pause",
            &["forward", "pause", "--to", "a@globex.example"],
            r#"{"ok":true,"address":"me@acme.example","forwards":[{"to":"a@globex.example"},{}],
              "skipped":[{"to":"b@globex.example","why":"already paused"},{}]}"#,
        ),
        ok_case(
            "resume",
            &["forward", "resume", "--address", "me@acme.example"],
            r#"{"ok":true,"state":"active"}"#,
        ),
        ok_case("resume --json", &["forward", "resume", "--json"], r#"{"ok":true}"#),
        refused(
            "an agent: person only",
            &["forward", "show"],
            403,
            r#"{"error":{"code":"person_only","hint":"people only"}}"#,
            3,
        ),
        refused(
            "a destination stopped: the daemon's hint",
            &["forward", "set", "--to", "a@globex.example"],
            409,
            r#"{"error":{"code":"destination_stopped","hint":"it said stop; ask it first"}}"#,
            1,
        ),
        refused(
            "a forward loop",
            &["forward", "add", "--to", "a@acme.example"],
            400,
            r#"{"error":{"code":"forward_loop","hint":"that loops","cap":"x"}}"#,
            1,
        ),
        refused(
            "owner only, on resume: the hint as it is",
            &["forward", "resume"],
            403,
            r#"{"error":{"code":"owner_only","hint":"the owner resumes a brake"}}"#,
            3,
        ),
        refused(
            "owner only, on pause: ask your human",
            &["forward", "pause"],
            403,
            PERMISSION_ASK,
            3,
        ),
        refused(
            "not found",
            &["forward", "unset"],
            404,
            r#"{"error":{"code":"not_found","hint":"no such mailbox"}}"#,
            1,
        ),
        case(
            "not ok",
            &["forward", "unset"],
            &[],
            vec![ok(r#"{"ok":false,"why":"no"}"#)],
            owner_and_port,
            1,
            1,
        ),
        traceback("an answer that is a list", &["forward", "show"], "[1]"),
        traceback(
            "an error that is a string",
            &["forward", "show"],
            r#"{"error":"broken"}"#,
        ),
        traceback(
            "a forward that is a number",
            &["forward", "show"],
            r#"{"ok":true,"forwards":[1]}"#,
        ),
        traceback(
            "counts that are a list, external",
            &["forward", "show"],
            r#"{"ok":true,"forwards":[{"kind":"external","last24h":[1]}]}"#,
        ),
        traceback(
            "counts that are a list, local",
            &["forward", "show"],
            r#"{"ok":true,"forwards":[{"kind":"local","last24h":[1]}]}"#,
        ),
        traceback(
            "events that are a dict",
            &["forward", "show"],
            r#"{"ok":true,"forwards":[{}],"events":{"a":1}}"#,
        ),
        traceback(
            "events that are a string",
            &["forward", "show"],
            r#"{"ok":true,"forwards":[{}],"events":"abc"}"#,
        ),
        traceback(
            "a volume that is a list",
            &["forward", "show"],
            r#"{"ok":true,"forwards":[{}],"volume":[1]}"#,
        ),
        traceback(
            "removed that are numbers",
            &["forward", "rm", "--to", "a@x"],
            r#"{"ok":true,"removed":[1]}"#,
        ),
        traceback(
            "pause rows that are numbers",
            &["forward", "pause"],
            r#"{"ok":true,"forwards":[1]}"#,
        ),
        traceback(
            "pause rows with a number for to",
            &["forward", "pause"],
            r#"{"ok":true,"forwards":[{"to":5}]}"#,
        ),
        traceback(
            "destinations that are a number",
            &["forward", "set", "--to", "a@x"],
            r#"{"ok":true,"destinations":7}"#,
        ),
        case(
            "closed, not sent again",
            &["forward", "set", "--to", "a@x"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "no answer in time",
            &["forward", "show"],
            &[("K5_MAIL_HTTP_TIMEOUT", "1")],
            vec![Reply::Hang(Duration::from_secs(3))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listening",
            &["forward", "unset"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
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
    let rig = Rig::new(VERB, "help");
    const LINES: &[&[&str]] = &[
        &["forward", "set", "--to", "a@x", "-h"],
        &["forward", "add", "--help", "--to", "a@x"],
        &["forward", "rm", "--to", "a@x", "--help"],
        &["forward", "unset", "-h"],
        &["forward", "pause", "--to", "a@x", "--help"],
        &["forward", "resume", "-h"],
        &["forward", "-h"],
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
