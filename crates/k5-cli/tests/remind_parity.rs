//! `k5-cli remind` and `k5-cli reminders` against the shell CLI's
//! `cli/k2 remind` and `cli/k2 reminders`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code
//! for every form.
//!
//! - The forms: set (every flag, `--file` alone, a first word that is a
//!   verb when a time is given), `list` (`--all`, `--workspace` as a
//!   folder and as a name), `show`, `snooze` (a positional time and the
//!   flags), `cancel`; `--json` for each; `reminders` (and with a time, a
//!   reminder whose text is `list`); many reminders (an answer above
//!   256 KiB).
//! - The stops: each flag with no value, an unknown flag or word, no text,
//!   no time, no name, a third word to `snooze`.
//! - The answers: an error object (owner only, usage, a plain string, a
//!   token refusal), `"ok": false`, empty, not JSON, a card on a refusal,
//!   the bulletin's mark, Python's raise part way.
//! - A passport over TCP and on the cell socket (not served goes on to
//!   TCP), the id tool's checks, the gates; no answer.
//! - `remind` alone: the help page, after the gates and the id tool's
//!   checks.
//! - What the binary hands to the shell: the other help forms, a port curl
//!   won't take.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const ADDED: &str = r#"{"name":"r1","fires":"in 45m (13:45)","ok":true}"#;
const ROWS: &str = r#"[{"name":"r1","state":"pending","fireAt":"2026-11-18T13:45:00Z","text":"Call\nback é"},{"name":"a-very-long-reminder-name-x","state":"fired","text":"0123456789012345678901234567890123456789012345678901234567890123456789XYZ"}]"#;
const SHOWN: &str = r#"{"name":"r1","state":"fired","fireAt":"13:45","tz":"Europe/Paris","text":"hi","createdBy":"ann","firedAt":"13:46","waitReason":"busy","waitDetail":"in a turn","fires":[{"firedAt":"13:46","decision":"sent"},{"fired_at":"x","reason":"later"}]}"#;

fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"name":"r{i}","state":"pending","fireAt":"2026-11-18T13:{:02}:00Z","text":"reminder {i} {}"}}"#,
                i % 60,
                "é".repeat(i % 50)
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn a_folder(r: &Rig) {
    owner_and_port(r);
    std::fs::create_dir_all(r.proj.join("sub")).unwrap();
}

#[test]
fn remind_answers_as_the_shell_does() {
    let rig = Rig::new("remind", "all");
    let traceback = |c: Case| Case {
        compare: Compare::Traceback,
        ..c
    };
    let v = vec![
        // covers: remind
        case(
            "set",
            &["Call", "back", "--in", "45m"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "set, every flag",
            &[
                "Call é",
                "--at",
                "09:00",
                "--on",
                "2026-11-20",
                "--tz",
                "Europe/Paris",
                "--name",
                "n1",
                "--file",
                "notes.md",
                "--session",
                "own",
                "--workspace",
                "sub",
            ],
            &[],
            vec![ok(ADDED)],
            a_folder,
            0,
            1,
        ),
        case(
            "set --file alone",
            &["--file", "f.md", "--in", "1m", "--project", "acme"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind --json
        case(
            "set --json",
            &["x", "--in", "1m", "--json"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list as text, with a time",
            &["list", "--in", "1m"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind list
        case("list", &["list"], &[], vec![ok(ROWS)], owner_and_port, 0, 1),
        case(
            "list, none",
            &["list", "--all"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind list --json
        case(
            "list --workspace --json",
            &["list", "--workspace", "sub", "--json"],
            &[],
            vec![ok(ROWS)],
            a_folder,
            0,
            1,
        ),
        case(
            "list, many",
            &["list"],
            &[],
            vec![ok(&many(4000))],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind show
        case(
            "show",
            &["show", "r1", "extra"],
            &[],
            vec![ok(SHOWN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, little",
            &["show", "r1"],
            &[],
            vec![ok(r#"{"name":"r1","state":"pending"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind show --json
        case(
            "show --json",
            &["show", "r1", "--json"],
            &[],
            vec![ok(SHOWN)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind snooze
        case(
            "snooze, a positional time",
            &["snooze", "r1", "10m"],
            &[],
            vec![ok(r#"{"name":"r1","fires":"at 13:55"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "snooze --at --tz",
            &["snooze", "r1", "--at", "14:00", "--tz", "UTC"],
            &[],
            vec![ok(r#"{"name":"r1","fires":"at 14:00"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind snooze --json
        case(
            "snooze --json",
            &["snooze", "r1", "--in", "5m", "--json"],
            &[],
            vec![ok(r#"{"name":"r1","fires":"at 13:55"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind cancel
        case(
            "cancel",
            &["cancel", "r 1"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remind cancel --json
        case(
            "cancel --json",
            &["cancel", "r1", "--json"],
            &[],
            vec![ok("{\"ok\":true}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "owner only",
            &["list"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Owner only."}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a card",
            &["cancel", "r1"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "usage",
            &["x", "--in", "soon"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"usage","message":"bad time"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a plain string",
            &["show", "r9"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no reminder r9"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a token refusal",
            &["list"],
            &[],
            vec![Reply::Answer(
                401,
                r#"{"error":"Invalid or missing auth token"}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "ok false",
            &["snooze", "r1", "5m"],
            &[],
            vec![ok(r#"{"ok":false,"message":"already fired"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case("empty", &["list"], &[], vec![ok("")], owner_and_port, 1, 1),
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin's mark",
            &["show", "r1"],
            &[],
            vec![ok(
                r#"{"name":"r1","state":"pending","bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "Python raises part way",
            &["list"],
            &[],
            vec![ok(r#"[{"name":"a"},{"text":5}]"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a gated hint that is a number",
            &["list"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"gated","hint":5}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// After a bare `--` the words are the reminder's text, as typed
/// (`_remind_add`'s `--)` arm), and the time scan stops there: a text
/// that starts with a dash or is `--help`, or a verb's name, is text.
#[test]
fn remind_after_dashes_as_the_shell_does() {
    let rig = Rig::new("remind", "dashes");
    let v = vec![
        // covers: remind
        case(
            "the text --help after --",
            &["--in", "10m", "--", "--help"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "dashed words after --, joined",
            &["--in", "10m", "a", "--", "", "-x", "--json", "-h"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a time after -- is text: no time",
            &["--", "x", "--in", "10m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "list after --, with a time: the text",
            &["--in", "1m", "--", "list"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list with -- and a time after it: list refuses --",
            &["list", "--", "--in", "1m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an empty first word, then -- and -h: the help page",
            &["", "--", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "help before --",
            &["--in", "10m", "-h", "--", "x"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
    ];
    for c in v {
        rig.check(&as_the_front_routes("remind", c));
    }
    // covers: heartbeat remind
    let hb = Rig::new("heartbeat", "rdashes");
    for c in [
        case(
            "heartbeat remind: the text -h after --",
            &["remind", "--in", "10m", "--", "-h"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "heartbeat remind: help before --",
            &["remind", "--in", "10m", "-h", "--", "hi"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
    ] {
        hb.check(&as_the_front_routes("heartbeat", c));
    }
}

#[test]
fn remind_stops_as_the_shell_does() {
    let rig = Rig::new("remind", "stops");
    let stop = |name: &'static str, args: &'static [&'static str]| {
        case(name, args, &[], vec![], owner_and_port, 2, 0)
    };
    let v = vec![
        stop("--in with no value", &["x", "--in"]),
        stop("--session with no value", &["x", "--in", "1m", "--session"]),
        stop("an unknown flag", &["x", "--in", "1m", "--bogus"]),
        stop("no text", &["--in", "1m"]),
        stop("no time", &["text", "--at", ""]),
        stop("list, --workspace with no value", &["list", "--workspace"]),
        stop("list, a word", &["list", "x"]),
        stop("show, no name", &["show"]),
        stop("show, an unknown flag", &["show", "--x"]),
        stop("cancel, no name", &["cancel", "--json"]),
        stop("snooze, no time", &["snooze", "r1"]),
        stop("snooze, a third word", &["snooze", "a", "b", "c"]),
        stop("snooze, --tz with no value", &["snooze", "--tz"]),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn reminders_is_remind_list() {
    let rig = Rig::new("reminders", "all");
    let v = vec![
        case("reminders", &[], &[], vec![ok(ROWS)], owner_and_port, 0, 1),
        case(
            "reminders --all --json",
            &["--all", "--json"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "reminders, a word",
            &["x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "reminders with a time sets one",
            &["--in", "1m"],
            &[],
            vec![ok(ADDED)],
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
fn remind_with_a_passport() {
    let rig = Rig::new("remind", "pass");
    let h = with_passport;
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            h(&[]),
            case(
                "the passport over TCP",
                &["list"],
                &[],
                vec![ok(ROWS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["list"],
                &[],
                vec![heal(&rig), ok(ROWS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["list"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            cell(),
            case(
                "the cell socket, a GET",
                &["list"],
                &[],
                vec![ok(ROWS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket, a POST",
                &["x", "--in", "1m"],
                &[],
                vec![ok(ADDED)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "not served: TCP",
                &["cancel", "r1"],
                &[],
                vec![not_served(), ok(r#"{"ok":true}"#)],
                port_only,
                0,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn remind_with_no_answer() {
    let rig = Rig::new("remind", "none");
    let port = dead_port();
    // The reader after these requests (`_remind_show_result`) says this
    // on an empty input; macOS's bash 3.2 runs it there (`in_pipe`).
    let in_pipe = |c: Case| Case {
        compare: Compare::Bash32PipeReader {
            stdout: "",
            stderr: "Error: empty response from daemon (is it running?)\n",
        },
        ..c
    };
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        in_pipe(case(
            "an empty reply",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        in_pipe(case(
            "an empty reply to a POST",
            &["cancel", "r1"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        case(
            "a reset",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
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
        case(
            "nothing listens",
            &["list"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        in_pipe(with(
            cell(),
            case(
                "a POST the cell socket may have run",
                &["cancel", "r1"],
                &[],
                vec![Reply::Close],
                port_only,
                141,
                1,
            ),
        )),
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
        case("no token", &["list"], &[], vec![], port_only, 1, 0),
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
fn remind_alone_is_the_help_page() {
    let rig = Rig::new("remind", "alone");
    let session = |c: Case| Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: port_only,
        ..c
    };
    let v = vec![
        case("alone", &[], &[], vec![], owner_and_port, 0, 0),
        case("an empty word", &[""], &[], vec![], owner_and_port, 0, 0),
        case(
            "an empty word and text, no time",
            &["", "call back"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "alone, no daemon: the gate",
            &[],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        session(case(
            "alone in a session",
            &[],
            &[],
            vec![],
            port_only,
            0,
            0,
        )),
        case(
            "alone: the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "alone: HOOK_SOCK set and no passport",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
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
fn remind_hands_these_to_the_shell() {
    let rig = Rig::new("remind", "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case("help", &["help"], &[], vec![], owner_and_port, 0, 0)),
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
            "a port that is no port: the shell's port refusal",
            &["list"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            2,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
