//! `k5-cli mail watch` against the shell CLI's `cli/k2 mail watch`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - Every sub-command: the status (and `--json`), on and off, inboxes,
//!   vip list, add and rm, the window, the digest (its instructions send
//!   nothing), outside-window, snippets, vip-line, pause (and `--all`),
//!   the log (and `--json`, `--limit`).
//! - The words: `--workspace` and `--json` anywhere, a workspace path
//!   with a space (the shell splits it: `_mw_body`'s traceback and an
//!   empty body, still sent), each usage line (exit 2, nothing sent).
//! - The answers: an owner-only refusal with a permission card (exit 3,
//!   the closing line), a 403, the daemon's usage error (exit 2), `"ok":
//!   false`, a 5xx with an empty body, not JSON, an error that is a
//!   string or null, the bulletin, a 503 "migrating" (no second try),
//!   the printer's raise, a NUL.
//! - The owner from disk, a passport on the cell socket, a heal, the id
//!   tool's refusal in a session with no passport.
//! - No answer: an empty reply, a reset and not HTTP over TCP (curl's
//!   `\n000` lines read as the answer); a refused connect, tried four
//!   times (five `\n000`s); a POST on the cell socket with no answer (the
//!   transport's line, exit 1, sent once).
//! - What the binary hands to the shell: help. A value flag as the last
//!   word is the shell's too, but its `$2: unbound variable` line names
//!   the script's path, so no row can compare it (the plan's D6; the
//!   family's unit tests hold the hand-back).
//! - Help never acts: every write with `-h` or `--help` anywhere prints
//!   its help and sends nothing ([`help_never_acts`]).
//! - One large answer (the log, above 256 KiB): the shell hands it to
//!   Python in an environment variable, which Linux refuses above
//!   128 KiB; the binary prints it ([`a_large_log`]).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "mail";

const STATUS: &str = r#"{"ok":true,"enabled":true,"inboxes":[{"address":"a@x.io","wake":"vip","needsRelink":true},{"address":"b@x.io","wake":"all","lastError":"auth é"},{"address":"c@x.io"}],"vips":["boss@x.io","@x.io"],"digest":{"start":"09:00","end":"17:00","days":["mon","tue"],"tz":"Europe/Paris","everySeconds":5400,"nextFireAt":"2026-11-20T09:00:00Z"},"vipOutsideWindow":false,"vipSessionLine":true,"snippets":false,"pausedUntil":"2026-11-21T10:00:00Z","last24h":{"vipDelivered":2,"digested":5}}"#;
const LOG: &str = r#"{"ok":true,"log":[{"receivedAt":"2026-11-19T10:00:00Z","class":"vip","state":"delivered","from":"boss@x.io","inbox":"a@x.io"},{"receivedAt":"2026-11-19T11:00:00Z","class":"digest","state":"held","from":"n@y.io","inbox":"b@x.io","reason":"outside window"}]}"#;

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn env(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn statused(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

#[test]
fn mail_watch_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "watch");
    let v = vec![
        case(
            "status",
            &["watch"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status by name, --json before it",
            &["watch", "--json", "status"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "another workspace",
            &["watch", "status", "--workspace", "/o/é b&c"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "on",
            &["watch", "on"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "off, --json",
            &["watch", "off", "--json"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        compare(
            Compare::Traceback,
            case(
                "a workspace path with a space: split, traceback, empty body",
                &["watch", "on", "--project", "/a b"],
                &[],
                vec![ok(STATUS)],
                owner_and_port,
                0,
                1,
            ),
        ),
        case(
            "a workspace path split into words with =",
            &["watch", "off", "--project", "/a b=2 c={\"x\":1}"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "inboxes, no change",
            &["watch", "inboxes"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "inboxes add and remove",
            &[
                "watch", "inboxes", "--add", "é@x.io", "--wake", "all", "--remove", "c@x.io",
            ],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "inboxes add, vip by default",
            &["watch", "inboxes", "--add", "d@x.io"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "inboxes, a bad wake",
            &["watch", "inboxes", "--add", "d@x.io", "--wake", "some"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "inboxes, a stray word",
            &["watch", "inboxes", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "vip list",
            &["watch", "vip"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "vip list, none",
            &["watch", "vip", "list"],
            &[],
            vec![ok(r#"{"vips":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "vip add",
            &["watch", "vip", "add", "a@x.io", "@y.io"],
            &[],
            vec![ok(r#"{"vips":["a@x.io","@y.io"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "vip rm",
            &["watch", "vip", "rm", "a@x.io"],
            &[],
            vec![ok(r#"{"vips":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "vip add, nothing",
            &["watch", "vip", "add"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "vip, a bad verb",
            &["watch", "vip", "show"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "window",
            &[
                "watch",
                "window",
                "09:00-17:00",
                "--weekdays",
                "--tz",
                "Europe/Paris",
            ],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "window, days",
            &["watch", "window", "8-6", "--days", "sat,,sun"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "window off",
            &["watch", "window", "off"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "window, none",
            &["watch", "window"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "window, no dash",
            &["watch", "window", "9"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "window, an unknown flag",
            &["watch", "window", "9-5", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "digest instructions: nothing sent",
            &["watch", "digest", "instructions"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "digest every",
            &["watch", "digest", "--every", "1H30m"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "digest every, not a duration",
            &["watch", "digest", "--every", "90"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "digest every, nothing",
            &["watch", "digest", "--every"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "digest, nothing",
            &["watch", "digest"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "outside-window",
            &["watch", "outside-window", "deliver"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "outside-window, bad",
            &["watch", "outside-window", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "snippets",
            &["watch", "snippets", "on"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "vip-line",
            &["watch", "vip-line", "off"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "vip-line, bad",
            &["watch", "vip-line"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "pause",
            &["watch", "pause", "--for", "2h"],
            &[],
            vec![ok(r#"{"pausedUntil":"2026-11-19T12:00:00Z"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "pause, a number",
            &["watch", "pause", "--for", "30"],
            &[],
            vec![ok(r#"{"pausedUntil":"x"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "resume all",
            &["watch", "pause", "--resume", "--all", "--workspace", "/a b"],
            &[],
            vec![ok(r#"{"pausedUntil":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "pause, bad",
            &["watch", "pause", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "log",
            &["watch", "log"],
            &[],
            vec![ok(LOG)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log, limit, json",
            &["watch", "log", "--limit", "5", "--json"],
            &[],
            vec![ok(LOG)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log, nothing yet",
            &["watch", "log"],
            &[],
            vec![ok(r#"{"log":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log, bad",
            &["watch", "log", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown sub-command",
            &["watch", "help"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--workspace with no value",
            &["watch", "status", "--workspace"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_watch_reads_the_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "answers");
    let v = vec![
        case(
            "owner only, with a permission card",
            &["watch", "on"],
            &[],
            vec![statused(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 403 with another code",
            &["watch"],
            &[],
            vec![statused(403, r#"{"error":{"code":"x","message":"no"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the daemon's usage error",
            &["watch", "pause", "--for", "x"],
            &[],
            vec![statused(
                400,
                r#"{"error":{"code":"usage","hint":"bad --for"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "ok false on a 200",
            &["watch"],
            &[],
            vec![ok(r#"{"ok":false,"message":"off"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 5xx with an empty body",
            &["watch"],
            &[],
            vec![statused(500, "")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "migrating: asked once",
            &["watch"],
            &[],
            vec![statused(503, r#"{"state":"migrating"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["watch"],
            &[],
            vec![ok("<html>nope</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error that is a string",
            &["watch"],
            &[],
            vec![statused(500, r#"{"error":"down"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error that is null",
            &["watch"],
            &[],
            vec![statused(500, r#"{"error":null}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a list",
            &["watch"],
            &[],
            vec![statused(500, "[1, 2]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin",
            &["watch", "vip", "list"],
            &[],
            vec![ok(
                r#"{"vips":["a"],"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        compare(
            Compare::Traceback,
            case(
                "the printer raises",
                &["watch"],
                &[],
                vec![ok(r#"{"enabled":true,"inboxes":[{"wake":"vip"}]}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        compare(
            Compare::Traceback,
            case(
                "a hint that is not a string",
                &["watch"],
                &[],
                vec![statused(500, r#"{"error":{"hint":5}}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        compare(
            Compare::NulInAnswer,
            case(
                "a NUL",
                &["watch", "vip", "list"],
                &[],
                vec![ok("{\"vips\":[\"a\u{0}b\"]}")],
                owner_and_port,
                0,
                1,
            ),
        ),
        case(
            "an empty reply",
            &["watch"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["watch", "on"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["watch", "log"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_watch_sends_as_the_shell_does() {
    let rig = Rig::new(VERB, "send");
    let sock = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        env(
            sock(),
            case(
                "a passport on the cell socket: GET",
                &["watch"],
                &[],
                vec![ok(STATUS)],
                nothing,
                0,
                1,
            ),
        ),
        env(
            sock(),
            case(
                "a passport on the cell socket: POST",
                &["watch", "on"],
                &[],
                vec![ok(STATUS)],
                nothing,
                0,
                1,
            ),
        ),
        env(
            sock(),
            case(
                "a passport healed",
                &["watch", "vip"],
                &[],
                vec![heal(&rig), ok(r#"{"vips":["a"]}"#)],
                port_only,
                0,
                2,
            ),
        ),
        env(
            sock(),
            compare(
                Compare::Waited,
                case(
                    "a POST on the cell socket, no answer: sent once",
                    &["watch", "off"],
                    &[],
                    vec![Reply::Close],
                    nothing,
                    1,
                    1,
                ),
            ),
        ),
        case(
            "a session with no passport: the id tool's refusal",
            &["watch"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "help is a page",
            &["watch", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// Help never acts (K2's rule): every verb that changes something, with
/// `-h` or `--help` anywhere on the line, prints its help and sends
/// nothing.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    const LINES: &[&[&str]] = &[
        &["watch", "on", "-h"],
        &["watch", "--help", "off"],
        &["watch", "inboxes", "--add", "a@x.io", "--help"],
        &["watch", "inboxes", "--remove", "-h"],
        &["watch", "vip", "add", "-h", "a@x.io"],
        &["watch", "vip", "rm", "a@x.io", "--help"],
        &["watch", "window", "09:00-17:00", "--tz", "--help"],
        &["watch", "window", "off", "-h"],
        &["watch", "digest", "--every", "-h"],
        &["watch", "outside-window", "hold", "--help"],
        &["watch", "snippets", "-h", "on"],
        &["watch", "vip-line", "off", "--help"],
        &["watch", "pause", "--for", "2h", "-h"],
        &["watch", "pause", "--resume", "--all", "--help"],
        &["watch", "--workspace", "-h", "on"],
    ];
    for line in LINES {
        rig.check(&case(
            "help, nothing sent",
            line,
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
}

#[test]
fn a_refused_connect_is_tried_four_times() {
    let rig = Rig::new(VERB, "refused");
    let c = case("nothing listens", &["watch"], &[], vec![], owner_only, 1, 0);
    let c = env(vec![("K2_PORT", dead_port())], c);
    rig.check(&c);
}

/// The one large answer: a log of 3,000 rows (above 256 KiB). The shell's
/// `_mw_python` hands the answer to Python in `BODY=`, an environment
/// variable, and Linux refuses one string above 128 KiB, so its
/// `python3` never starts (exit 126, `Argument list too long`). The
/// binary prints every row. Elsewhere (macOS: one limit for the whole
/// environment) both print the same. A named difference, for K2's
/// sign-off.
#[test]
fn a_large_log() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(r#"{{"receivedAt":"2026-11-19T10:{:02}:00Z","class":"vip","state":"delivered","from":"sender{n}@example.com","inbox":"agent@example.com","reason":"row {n} of a long log"}}"#, n % 60)
        })
        .collect();
    let body = format!(r#"{{"ok":true,"log":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    let c = case(
        "a large log",
        &["watch", "log"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!((out.code, out.stderr.as_str()), (Some(0), ""), "{out:?}");
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 3000);
    assert_eq!(
        lines[2999],
        "2026-11-19T10:59:00Z  vip      delivered  sender2999@example.com -> agent@example.com  (row 2999 of a long log)"
    );
    rig.prepare(Side::Bash, &c);
    let bash = rig.run(true, &c);
    let sent = rig.read(Side::Bash, false, c.name);
    assert_eq!(sent.len(), 1);
    if cfg!(target_os = "linux") {
        assert_eq!(bash.code, Some(126), "{bash:?}");
        assert_eq!(bash.stdout, "");
        assert!(
            bash.stderr.contains("Argument list too long"),
            "{}",
            bash.stderr
        );
    } else {
        assert_eq!(
            (bash.code, &bash.stdout, &bash.stderr),
            (out.code, &out.stdout, &out.stderr)
        );
    }
}
