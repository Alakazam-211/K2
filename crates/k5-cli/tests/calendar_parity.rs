//! `k5-cli calendar` (and `cal`) against the shell CLI's `cli/k2 calendar`,
//! on the parity rig ([`common::parity`]): the same requests (urllib's,
//! from `_cal_py`), stdout, stderr and exit code.
//!
//! - Every verb, plain and `--json`: list, events (short and
//!   `--full`, all its query words), show (`event` too), calendars (read,
//!   and `--add`/`--remove`), freebusy, wait (a change, and its timed-out
//!   answer, exit 2), create, update, delete.
//! - `cmd_calendar`'s usage errors and help (no verb, `help`).
//! - `link`: `--request`, Google in a browser, Microsoft's device code,
//!   paste-back from a file and the prompt that reads nothing (the
//!   redirect port is random, so those rows mask it), `--calendars` and
//!   `--omit-description`, a denied link, the owner's refusal, answers
//!   that are not what it expects, no answer.
//! - The daemon's errors: an object (exit 3 for a code that needs a
//!   person, with `cap`/`reason`/`fix`; the owner-only hint; 2 for usage),
//!   a string, `ok` missing, an HTTP error that is not JSON, a 2xx that is
//!   not JSON, empty or `null`, an answer that is not an object (Python
//!   raises); the 503 "migrating" answer asked again.
//! - No answer: nothing listening (four tries), closed with nothing and a
//!   reset (a GET once more, a POST never), not HTTP.
//! - The owner from disk, a passport over TCP, the `mail` id tool's
//!   refusal in a session with no passport, the gates.
//! - One answer above 256 KiB: thousands of events.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "calendar";

const LIST: &str = r#"{"ok":true,"address":"a@x.example","calendars":[
 {"id":"c1","name":"Work","isDefault":true,"rights":{"read":true,"write":true}},
 {"id":"c2","shared":true,"owner":"b@x.example","rights":{}},
 {"id":"c3","name":"é 日本","rights":{"read":1,"delete":true}},
 {"id":"c4","name":"","rights":null}]}"#;

const EVENTS: &str = r#"{"ok":true,"address":"a@x.example","start":"2026-10-08","end":"2026-10-15","truncated":true,"events":[
 {"id":"e1","start":"2026-10-08T09:00:00-07:00","end":"2026-10-08T10:00:00-07:00","title":"Standup","timeZone":"America/Denver",
  "join":{"provider":"meet","meeting_id":"abc-def"},"self_response":"needsAction","recurring":true,"participants":3,"status":"tentative"},
 {"id":"e2","allDay":true,"start":"2026-10-09T00:00:00Z","shared":true,"owner":"b@x.example","self_response":"declined","status":"confirmed"},
 {"id":"e3","join":{"provider":"other"},"title":"","self_response":"tentative","participants":0},
 {"id":"e4","start":"s","end":"e","join":{"provider":"teams","meeting_id":7},"status":"cancelled","title":"Ünï ☃"}]}"#;

const EVENT: &str = r#"{"ok":true,"event":{"id":"e1","title":"Plan","start":"s","end":"e","location":"Room 1",
 "calendarIds":["c1","c2"],"status":"confirmed","freeBusy":"free","recurring":true,"organizer":"o@x.example",
 "organizer_is_self":true,"self_response":"accepted","join":{"provider":"zoom","url":"https://z.example/j"},
 "updated":"2026-10-01","attendees":[{"email":"a@x.example","response":"accepted"},{"name":"N"},{}],
 "description":"line one\nline two"}}"#;

const EVENT_REDACTED: &str = r#"{"ok":true,"event":{"id":"e2","status":"tentative",
 "participantList":[{"email":"p@x.example","status":"needs-action"},{"name":"Q"}],"redacted":["description"]}}"#;

fn ok_owner(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn many_events(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"ev-{i}","start":"2026-10-{:02}T09:00:00Z","end":"2026-10-{:02}T10:00:00Z","title":"Meeting {i} {}","participants":{}}}"#,
                1 + i % 28,
                1 + i % 28,
                "é".repeat(i % 30),
                i % 5
            )
        })
        .collect();
    format!(
        r#"{{"ok":true,"address":"a@x.example","start":"s","end":"e","events":[{}]}}"#,
        rows.join(",")
    )
}

#[test]
fn calendar_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let traceback = |c: Case| Case {
        compare: Compare::Traceback,
        ..c
    };
    let big = many_events(3000);
    assert!(
        big.len() > 256 * 1024,
        "the large answer is {} bytes",
        big.len()
    );
    let cases = vec![
        // Help and usage: nothing sent.
        case("no verb", &[], &[], vec![], owner_and_port, 0, 0),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case("unknown verb", &["nope"], &[], vec![], owner_and_port, 2, 0),
        case(
            "a flag the verb doesn't take",
            &["show", "e1", "--from", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a value flag at the end",
            &["events", "--limit"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown single-dash flag",
            &["list", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an extra word",
            &["list", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "show with no id",
            &["show"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "calendars with no address",
            &["calendars"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad limit",
            &["events", "--limit", "5x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad timeout",
            &["wait", "--timeout", "901"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "create with no title",
            &["create", "--start", "a", "--end", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "create with no end",
            &["create", "--title", "t", "--start", "a"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "update with nothing",
            &["update", "e1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "link with no provider",
            &["link", "a@x.example"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // The verbs.
        // covers: calendar list
        ok_owner("list", &["list"], LIST),
        // covers: calendar list --json
        ok_owner("list --json", &["list", "a@x.example", "--json"], LIST),
        ok_owner(
            "list, none",
            &["list"],
            r#"{"ok":true,"address":"a@x.example","calendars":[]}"#,
        ),
        // covers: calendar events
        ok_owner(
            "events",
            &[
                "events",
                "a@x.example",
                "--from",
                "2026-10-08",
                "--to",
                "2026-10-15",
                "--calendar",
                "c 1",
                "--limit",
                "50",
                "--include-cancelled",
            ],
            EVENTS,
        ),
        ok_owner("events --full", &["events", "--full"], EVENTS),
        // covers: calendar events --json
        ok_owner("events --json", &["events", "--json"], EVENTS),
        ok_owner(
            "events, none",
            &["events"],
            r#"{"ok":true,"address":"a@x.example","start":"s","end":"e","events":[]}"#,
        ),
        ok_owner(
            "events, odd fields",
            &["events"],
            r#"{"ok":true,"events":[{"id":1,"start":[1,2],"allDay":true,"participants":[1],"status":2}]}"#,
        ),
        // covers: calendar show
        ok_owner("show", &["show", "e1"], EVENT),
        ok_owner("event is show", &["event", "e 1/é"], EVENT_REDACTED),
        // covers: calendar show --json
        ok_owner("show --json", &["show", "e1", "--json"], EVENT),
        ok_owner("show, no event", &["show", "e1"], r#"{"ok":true}"#),
        // covers: calendar calendars
        ok_owner(
            "calendars",
            &["calendars", "a@x.example"],
            r#"{"ok":true,"address":"a@x.example","access":"read","calendars":[{"id":"primary","name":"Main","primary":true},{"id":"c2","optedIn":false},{"id":"c3","optedIn":0,"isDefault":true}],"hint":"add one with --add"}"#,
        ),
        ok_owner(
            "calendars --add --remove",
            &[
                "calendars",
                "a@x.example",
                "--add",
                "x",
                "--add",
                " y \n\nw",
                "--remove",
                "z",
            ],
            r#"{"ok":true,"address":"a@x.example","access":"read","calendars":[],"added":["x","y","w"],"removed":["z"],"hint":"not shown"}"#,
        ),
        // covers: calendar calendars --json
        ok_owner(
            "calendars --json",
            &["calendars", "a@x.example", "--json"],
            r#"{"ok":true,"calendars":[]}"#,
        ),
        // covers: calendar freebusy
        ok_owner(
            "freebusy",
            &["freebusy", "a@x.example", "--from", "a", "--to", "b"],
            r#"{"ok":true,"address":"a@x.example","start":"a","end":"b","busy":[{"start":"1","end":"2"},{"start":"3","end":"4","status":"tentative"},{}]}"#,
        ),
        ok_owner("freebusy, free", &["freebusy"], r#"{"ok":true,"busy":[]}"#),
        // covers: calendar freebusy --json
        ok_owner(
            "freebusy --json",
            &["freebusy", "--json"],
            r#"{"ok":true,"busy":[]}"#,
        ),
        // covers: calendar wait
        ok_owner(
            "wait",
            &["wait", "--since-state", "s1", "--timeout", "5"],
            r#"{"ok":true,"state":"s2","created":["e1","e2"],"updated":[],"destroyed":["e3"]}"#,
        ),
        // covers: calendar wait --json
        ok_owner(
            "wait --json",
            &["wait", "--json"],
            r#"{"ok":true,"state":"s2"}"#,
        ),
        case(
            "wait, timed out",
            &["wait", "--timeout", "1"],
            &[],
            vec![ok(r#"{"ok":true,"timedOut":true,"state":"s1"}"#)],
            owner_and_port,
            2,
            1,
        ),
        // covers: calendar create
        ok_owner(
            "create",
            &[
                "create",
                "a@x.example",
                "--title",
                "Plan é",
                "--start",
                "2026-10-08T09:00:00-07:00",
                "--end",
                "2026-10-08T10:00:00-07:00",
                "--tz",
                "America/Denver",
                "--location",
                "",
                "--notes",
                "n\"q",
                "--calendar",
                "c1",
            ],
            r#"{"ok":true,"id":"e9","event":{"id":"e9","start":"a","end":"b","title":"Plan é"}}"#,
        ),
        // covers: calendar create --json
        ok_owner(
            "create --json",
            &[
                "create", "--title", "t", "--start", "a", "--end", "b", "--json",
            ],
            r#"{"ok":true,"id":"e9"}"#,
        ),
        // covers: calendar update
        ok_owner(
            "update",
            &["update", "e9", "--title", "", "--notes", "x"],
            r#"{"ok":true,"id":"e9"}"#,
        ),
        // covers: calendar update --json
        ok_owner(
            "update --json",
            &["update", "e9", "--tz", "UTC", "--json"],
            r#"{"ok":true,"id":"e9"}"#,
        ),
        // covers: calendar delete
        ok_owner("delete", &["delete", "e9"], r#"{"ok":true,"id":"e9"}"#),
        // covers: calendar delete --json
        ok_owner(
            "delete --json",
            &["delete", "e9", "--json"],
            r#"{"ok":true,"id":"e9"}"#,
        ),
        // One answer above 256 KiB.
        ok_owner("events, thousands", &["events"], &big),
        ok_owner("events --json, thousands", &["events", "--json"], &big),
        // The daemon's errors.
        case(
            "capability_missing",
            &["events"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"capability_missing","hint":"needs calendar","cap":"calendar","reason":"r","fix":"","extra":1}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "owner_only",
            &["list"],
            &[],
            vec![ok(
                r#"{"ok":false,"error":{"code":"owner_only","hint":"Only the owner."}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "forbidden, no hint",
            &["list"],
            &[],
            vec![answer(403, r#"{"error":{"code":"forbidden"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "usage from the daemon",
            &["create", "--title", "t", "--start", "a", "--end", "b"],
            &[],
            vec![answer(
                400,
                r#"{"error":{"code":"usage","hint":"bad start"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "hosted_only",
            &["list"],
            &[],
            vec![answer(
                400,
                r#"{"error":{"code":"hosted_only","hint":"h"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error string",
            &["list"],
            &[],
            vec![ok(r#"{"error":"boom"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not ok",
            &["list"],
            &[],
            vec![ok(r#"{"ok":false,"why":"é"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an HTTP error that is not JSON",
            &["list"],
            &[],
            vec![answer(500, "oops")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 2xx that is not JSON",
            &["list"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty 2xx",
            &["list"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "null",
            &["list"],
            &[],
            vec![ok("null")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a list",
            &["list"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "events as text",
            &["events"],
            &[],
            vec![ok(r#"{"ok":true,"events":"ab"}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a number to walk",
            &["freebusy"],
            &[],
            vec![ok(r#"{"ok":true,"busy":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "migrating, then the answer",
            &["list"],
            &[],
            vec![answer(503, r#"{"ok":false,"state":"migrating"}"#), ok(LIST)],
            owner_and_port,
            0,
            2,
        ),
        // No answer.
        case(
            "nothing listening",
            &["list"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "closed, a GET tries once more",
            &["list"],
            &[],
            vec![Reply::Close, Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        case(
            "closed, then the answer",
            &["list"],
            &[],
            vec![Reply::Close, ok(LIST)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "closed, a POST never again",
            &["delete", "e9"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["show", "e1"],
            &[],
            vec![Reply::Reset, Reply::Reset],
            owner_and_port,
            1,
            2,
        ),
        case(
            "not HTTP",
            &["list"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        // Who it is.
        Case {
            env: with_passport(&[]),
            ..case(
                "a passport over TCP",
                &["list"],
                &[],
                vec![ok(LIST)],
                nothing,
                0,
                1,
            )
        },
        case(
            "the id tool in a session with no passport",
            &["list"],
            &[("K2_HOOK_SOCK", "{deadsock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "no port: the gate",
            &["list"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        // Help never acts: -h or --help anywhere on a verb that changes
        // something prints the help and sends nothing (the shell's, by
        // the front's rule for a help flag).
        Case {
            compare: Compare::Shell,
            ..case(
                "create -h",
                &["create", "--title", "t", "--start", "a", "--end", "b", "-h"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..case(
                "create --help first",
                &["create", "--help", "--title", "t"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..case(
                "update --help",
                &["update", "e9", "--title", "x", "--help"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..case(
                "delete -h",
                &["delete", "e9", "-h"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..case(
                "calendars --add -h",
                &["calendars", "a@x.example", "--add", "x", "-h"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..case(
                "link --help",
                &[
                    "link",
                    "a@x.example",
                    "--provider",
                    "google",
                    "--request",
                    "--help",
                ],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..case(
                "--help before the verb",
                &["--help", "delete", "e9"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

#[test]
fn cal_is_calendar() {
    let rig = Rig::new("cal", "alias");
    for c in [
        case(
            "cal list",
            &["list"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case("cal help", &["help"], &[], vec![], owner_and_port, 0, 0),
    ] {
        rig.check(&c);
    }
}

/// Each `127.0.0.1:<5 digits>/cb` (the paste-back's random port, 20000 to
/// 59999) as `127.0.0.1:PORT/cb`, after checking the digits are in range;
/// and the `Host` line's port as `{port}`.
fn mask_link(request: &str) -> String {
    let mut out = String::new();
    let mut rest = request;
    while let Some(at) = rest.find("127.0.0.1:") {
        let (head, tail) = rest.split_at(at + "127.0.0.1:".len());
        out.push_str(head);
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if tail[digits.len()..].starts_with("/cb") {
            let port: u32 = digits.parse().unwrap();
            assert!((20000..60000).contains(&port), "the redirect port {port}");
            out.push_str("PORT");
            rest = &tail[digits.len()..];
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out.lines()
        .map(|l| {
            if l.starts_with("Host: ") {
                "Host: {port}".to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Both sides of `c`, compared with the redirect port masked: the same
/// requests, stdout, stderr and exit.
fn check_masked(rig: &Rig, c: &Case) {
    let mut runs = Vec::new();
    for bash in [true, false] {
        let side = if bash { Side::Bash } else { Side::Rust };
        rig.prepare(side, c);
        let out = rig.run(bash, c);
        assert_eq!(rig.stub().left(), 0, "{}: answers left: {out:?}", c.name);
        let log: Vec<String> = rig
            .read(side, !bash, c.name)
            .iter()
            .map(|r| mask_link(&normalise(r, !bash, rig.noun(), c.name)))
            .collect();
        runs.push((out, log));
    }
    let (b, r) = (&runs[0], &runs[1]);
    println!("-- {}: bash {:?}", c.name, b.0);
    assert_eq!(b.1.len(), c.requests, "{}: bash's requests", c.name);
    assert_eq!(b.0.code, Some(c.code), "{}: bash's exit", c.name);
    assert_eq!(r.1, b.1, "{}: the requests", c.name);
    assert_eq!(r.0.code, b.0.code, "{}: the exit", c.name);
    assert_eq!(r.0.stdout, b.0.stdout, "{}: stdout", c.name);
    assert_eq!(r.0.stderr, b.0.stderr, "{}: stderr", c.name);
}

const LINK_START: &str =
    r#"{"ok":true,"linkId":"L1","authorizationUrl":"https://accounts.example/o?x=1&y=é"}"#;

fn pasted(rig: &Rig) {
    owner_and_port(rig);
    std::fs::write(
        rig.proj.join("pasted.txt"),
        "  http://127.0.0.1:40000/cb?state=s1&code=4%2F0Ab+c&scope=x\r\n",
    )
    .unwrap();
}

fn pasted_error(rig: &Rig) {
    owner_and_port(rig);
    std::fs::write(rig.proj.join("pasted.txt"), "x?error=access_denied").unwrap();
}

#[test]
fn link_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "link");
    let cases = vec![
        // covers: calendar link
        case(
            "link in a browser, pending then connected",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"linkId":"L1"}"#),
                ok(r#"{"ok":true,"state":"pending"}"#),
                ok(r#"{"ok":true,"state":"connected","hint":"linked é"}"#),
            ],
            owner_and_port,
            0,
            3,
        ),
        // covers: calendar link --json
        case(
            "link microsoft --calendars --omit-description --json",
            &[
                "link",
                "a@x.example",
                "--provider",
                "microsoft",
                "--workspace",
                "w",
                "--calendars",
                "Work, primary ,,b",
                "--omit-description",
                "--json",
            ],
            &[],
            vec![
                ok(
                    r#"{"ok":true,"linkId":"L2","verificationUrl":"https://v.example","userCode":"ABC"}"#,
                ),
                ok(r#"{"state":"connected","hint":"h","calendars":"old"}"#),
                ok(r#"{"ok":true,"added":["Work","b"]}"#),
                ok(r#"{"ok":true,"omit":true}"#),
            ],
            owner_and_port,
            0,
            4,
        ),
        case(
            "link microsoft --write --calendars, none added",
            &[
                "link",
                "a@x.example",
                "--provider",
                "microsoft",
                "--workspace",
                "w",
                "--write",
                "--calendars",
                "Work",
                "--omit-description",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"linkId":7}"#),
                ok(r#"{"state":"connected"}"#),
                ok(r#"{"ok":true}"#),
                ok(r#"{"ok":true}"#),
            ],
            owner_and_port,
            0,
            4,
        ),
        case(
            "link --request",
            &["link", "a@x.example", "--provider", "google", "--request"],
            &[],
            vec![ok(r#"{"ok":true,"hint":"card posted to your Thread"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "link --request --json",
            &[
                "link",
                "a@x.example",
                "--provider",
                "microsoft",
                "--request",
                "--json",
            ],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "link denied, with admin consent",
            &[
                "link",
                "a@x.example",
                "--provider",
                "microsoft",
                "--workspace",
                "w",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"linkId":"L"}"#),
                ok(r#"{"state":"denied","adminConsentUrl":"https://c.example"}"#),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "link error with a code",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"linkId":"L"}"#),
                ok(
                    r#"{"state":"error","code":"consent_required","hint":"see https://c.example","adminConsentUrl":"https://c.example"}"#,
                ),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "link, the owner's refusal at the start",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
            ],
            &[],
            vec![answer(403, r#"{"error":{"code":"forbidden","hint":"no"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "link, the owner's refusal on the status",
            &[
                "link",
                "a@x.example",
                "--provider",
                "microsoft",
                "--workspace",
                "w",
            ],
            &[],
            vec![
                ok(r#"{"ok":true,"linkId":"L"}"#),
                answer(403, r#"{"error":{"code":"owner_only"}}"#),
            ],
            owner_and_port,
            3,
            2,
        ),
        case(
            "link, a start that isn't ok",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
            ],
            &[],
            vec![ok(r#"{"linkId":"x","note":"é"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "link, an HTTP error that is not JSON",
            &[
                "link",
                "a@x.example",
                "--provider",
                "microsoft",
                "--workspace",
                "w",
            ],
            &[],
            vec![answer(500, "oops")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "link, a 200 that is not JSON",
            &[
                "link",
                "a@x.example",
                "--provider",
                "microsoft",
                "--workspace",
                "w",
            ],
            &[],
            vec![ok("oops")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "link, closed with nothing",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
            ],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "link, a start that is a list",
                &[
                    "link",
                    "a@x.example",
                    "--provider",
                    "google",
                    "--workspace",
                    "w",
                ],
                &[],
                vec![ok("[1]")],
                owner_and_port,
                1,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "link, a status that is a list",
                &[
                    "link",
                    "a@x.example",
                    "--provider",
                    "microsoft",
                    "--workspace",
                    "w",
                ],
                &[],
                vec![ok(r#"{"ok":true,"linkId":"L"}"#), ok("[2]")],
                owner_and_port,
                1,
                2,
            )
        },
    ];
    for c in &cases {
        rig.check(c);
    }
    // Paste-back: the redirect port is random on both sides.
    let paste = vec![
        case(
            "link --paste-file",
            &[
                "link",
                "a@x.example",
                "--provider",
                "gmail",
                "--workspace",
                "w",
                "--paste-file",
                "pasted.txt",
            ],
            &[],
            vec![
                ok(LINK_START),
                ok(r#"{"ok":true}"#),
                ok(r#"{"state":"connected","hint":"ok"}"#),
            ],
            pasted,
            0,
            3,
        ),
        case(
            "link --paste-file, an error pasted, expired",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
                "--paste-file",
                "pasted.txt",
                "--json",
            ],
            &[],
            vec![ok(LINK_START), answer(410, r#"{"error":"gone"}"#)],
            pasted_error,
            1,
            2,
        ),
        case(
            "link --paste-file that isn't there",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
                "--paste-file",
                "nothing.txt",
            ],
            &[],
            vec![ok(LINK_START)],
            owner_and_port,
            2,
            1,
        ),
        // Bash's prompt reads the script's own heredoc: nothing.
        case(
            "link --paste reads nothing",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
                "--paste",
            ],
            &[],
            vec![ok(LINK_START)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "link, no browser here: paste-back",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
            ],
            &[],
            vec![
                answer(400, r#"{"error":{"code":"remote_unsupported","hint":"x"}}"#),
                ok(LINK_START),
            ],
            owner_and_port,
            2,
            2,
        ),
    ];
    for c in &paste {
        check_masked(&rig, c);
    }
}
