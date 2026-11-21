//! `k5-cli hooks` against the shell CLI's `hooks` arm, `cmd_hooks_status`
//! and `cmd_hooks_install` (`cli/src/router.sh`, `cli/src/remind.sh`), on
//! the parity rig ([`common::parity`]): the same requests, stdout, stderr
//! and exit code.
//!
//! - No verb or an unknown one: the usage lines, exit 1, nothing sent;
//!   the gate first.
//! - `status`: the limit (default, `--limit`, `-n`), a missing limit or
//!   one that is not digits only (`Error: <flag> needs a number`, exit 2,
//!   nothing sent), `--json` (echoed, echo's own options too), the
//!   reader's lines for a bare answer, a full one, each section's odd
//!   shapes, and where the reader raises: what it printed, then the raw
//!   answer, exit 0; an `error` key: its `Error:` line, exit 1, or 3 with
//!   a `permission` object (with `--json`: the answer as it came, and the
//!   same exit). One answer above 256 KiB.
//! - `install` and `uninstall`: the body, the reader's lines (a report,
//!   skipped, an `error` key: its `Error:` line, exit 1, or 3 with a
//!   `permission` object), `--json` (as it came; an `error` key exits 1,
//!   or 3).
//! - No answer: a reset.
//! - Help: `-h` or `--help` anywhere prints the hooks page and sends
//!   nothing (`hooks -h` and `hooks --help` the binary's own page, the
//!   rest the shell's).
//!
//! The times are far in the future, so `ago` is `0s ago` on both sides.
//! Each native row is the binary's own answer ([`answered_here`]).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

/// `c` on the rig, then the binary alone once more: it answered the line
/// itself, so it never wrote the shell CLI out to hand it over.
fn answered_here(rig: &Rig, c: &Case) {
    rig.check(c);
    rig.binary(c, false);
    assert!(
        !k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
        "{}: the binary handed the line to the shell",
        c.name
    );
}

fn one(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

const FULL: &str = r#"{"port":7171,"notify_script":{"path":"/h/.k2/hooks/notify.sh","exists":true,"version":"3"},"injections":{"notify_script":{"path":"/h/.k2/hooks/notify.sh","exists":true,"version":"3"},"claude":{"path":"/h/.claude/settings.json","exists":true,"injected":true,"events":["Stop","PreToolUse"]},"cursor":{"path":"/h/.cursor/hooks.json","exists":true,"injected":false},"gemini":{"path":"/h/.gemini/settings.json","configUnreadable":true}},"clis":{"claude":{"detectedVersion":"2.1.0","eventSet":"full","installedEvents":["Stop","PreToolUse","PostToolUse"],"lastEventMs":99999999999999},"cursor":{"lastEventMs":0},"gemini":{}},"ownerCheck":"conversation","lastInstall":{"report":{"action":"install"},"trigger":"boot","agentHooksSetting":true},"sessions":{"sess-1":{"owner":{"proc":{"pid":4242}},"evidence":"pty","counters":{"accepted":12,"foreign":1,"unknownPane":0,"parseError":2},"lastEnvelopeMs":99999999999999},"sess-2":{"owner":{"via":"conversation"},"counters":{"accepted":1.5,"foreign":true}},"sess-3":{}},"totals":{"accepted":13,"foreign":1},"recent_events":[{"timestamp":"2026-11-20T10:11:12.345Z","verdict":"accepted","raw_event":"PreToolUse","tool_name":"Bash","pane_id":"p1"},{"canonical":"stop","raw_event":"Stop"},{"timestamp":"short"},{}],"recent_events_cap":50}"#;

/// An answer above 256 KiB: thousands of recent events.
fn large() -> String {
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"timestamp":"2026-11-20T10:{:02}:{:02}.000Z","verdict":"accepted","raw_event":"PreToolUse","tool_name":"Tool{i}","pane_id":"pane-{i:05}-abcdefghijklmnop"}}"#,
                (i / 60) % 60,
                i % 60
            )
        })
        .collect();
    let body = format!(
        r#"{{"port":1,"injections":{{}},"recent_events":[{}],"recent_events_cap":3000}}"#,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024, "{}", body.len());
    body
}

/// A 200 whose body is not UTF-8.
fn not_utf8() -> Vec<u8> {
    let body: &[u8] = b"{\"error\":\"x\"}\xff";
    let mut r = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    r.extend_from_slice(body);
    r
}

#[test]
fn hooks_with_no_verb() {
    let rig = Rig::new("hooks", "usage");
    for c in [
        // native: hooks
        case("alone", &[], &[], vec![], owner_and_port, 1, 0),
        case(
            "an unknown verb",
            &["nope", "x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case("an empty verb", &[""], &[], vec![], owner_and_port, 1, 0),
        case("--json", &["--json"], &[], vec![], owner_and_port, 1, 0),
        case("the gate: no port", &[], &[], vec![], owner_only, 1, 0),
        case(
            "the gate: no token",
            &["status"],
            &[],
            vec![],
            port_only,
            1,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}

#[test]
fn hooks_status_as_the_shell_does() {
    let rig = Rig::new("hooks", "status");
    let big = large();
    let v = vec![
        // native: hooks status
        one("a bare answer", &["status"], "{}"),
        one("a full answer", &["status"], FULL),
        one("--limit", &["status", "--limit", "5"], "{}"),
        one(
            "-n, the last wins",
            &["status", "-n", "5", "--limit", "7"],
            "{}",
        ),
        one("-n 007", &["status", "-n", "007"], "{}"),
        one("other words are skipped", &["status", "x", "y"], "{}"),
        one("--json", &["status", "--json"], FULL),
        one(
            "--json: trailing newlines go",
            &["status", "--json"],
            "{\"a\":1}\n\n",
        ),
        one("--json: echo reads -n", &["status", "--json"], "-n"),
        one("--json: echo reads -e", &["status", "--json"], "-e"),
        one("not JSON: the raw answer", &["status"], "<html>oops</html>"),
        one("an empty answer", &["status"], ""),
        one("a list: the raw answer", &["status"], "[1,2]"),
        one("injections not a dict", &["status"], r#"{"injections":[]}"#),
        one(
            "notify_script not a dict: the port line, then raw",
            &["status"],
            r#"{"port":"p","injections":{"notify_script":"x"}}"#,
        ),
        one(
            "lastInstall skipped",
            &["status"],
            r#"{"lastInstall":{"skipped":"agentHooks off"},"ownerCheck":"pid"}"#,
        ),
        one(
            "lastInstall not a dict",
            &["status"],
            r#"{"lastInstall":"x"}"#,
        ),
        one(
            "a cli's entry not a dict",
            &["status"],
            r#"{"injections":{"cursor":5}}"#,
        ),
        one("clis not a dict", &["status"], r#"{"clis":[1]}"#),
        one(
            "events that are a number",
            &["status"],
            r#"{"clis":{"claude":{"installedEvents":5}}}"#,
        ),
        one(
            "lastEventMs not a number",
            &["status"],
            r#"{"clis":{"cursor":{"lastEventMs":"x"}}}"#,
        ),
        one(
            "sessions a list: the header, then raw",
            &["status"],
            r#"{"sessions":[1]}"#,
        ),
        one(
            "a session's counter that can't be formatted",
            &["status"],
            r#"{"sessions":{"s":{"counters":{"accepted":[1]}}}}"#,
        ),
        one(
            "a session's owner via null",
            &["status"],
            r#"{"sessions":{"s":{"owner":{"via":null,"proc":{"pid":0}},"evidence":7}}}"#,
        ),
        one("totals not a dict", &["status"], r#"{"totals":[1]}"#),
        one(
            "recent events a string: the headers, then raw",
            &["status"],
            r#"{"recent_events":"ab","recent_events_cap":"c"}"#,
        ),
        one(
            "recent events a number",
            &["status"],
            r#"{"recent_events":3}"#,
        ),
        one(
            "an event's timestamp a number",
            &["status"],
            r#"{"recent_events":[{"timestamp":5}]}"#,
        ),
        one(
            "an event's verdict a list",
            &["status"],
            r#"{"recent_events":[{"verdict":[1]}]}"#,
        ),
        one(
            "an event's fields of each type",
            &["status"],
            r#"{"recent_events":[{"timestamp":"aTbTcTdTeTfTgThTiTjTk","verdict":true,"raw_event":2.5,"tool_name":7,"pane_id":[1]}]}"#,
        ),
        case(
            "an error, 500: its Error line, exit 1",
            &["status"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"boom"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object: its hint",
            &["status", "--limit", "5"],
            &[],
            vec![ok(r#"{"error":{"code":"c","hint":"the hint"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object: its code",
            &["status"],
            &[],
            vec![ok(r#"{"error":{"code":"admin_only","hint":""}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error null",
            &["status"],
            &[],
            vec![ok(r#"{"error":null,"port":1}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error with a lone surrogate",
            &["status"],
            &[],
            vec![ok(r#"{"error":"bad \ud800 name"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal: the Error line, Ask with, exit 3",
            &["status"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "--json: an error answer as it came, exit 1",
            &["status", "--json"],
            &[],
            vec![ok(r#"{"error":"boom"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--json: a refusal as it came, Ask with, exit 3",
            &["status", "--limit", "5", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a reset",
            &["status"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "a passport over TCP",
                &["status"],
                &[],
                vec![ok("{}")],
                port_only,
                0,
                1,
            )
        },
        // The large answer.
        case(
            "an answer above 256 KiB",
            &["status"],
            &[],
            vec![ok(&big)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        answered_here(&rig, &c);
    }
}

/// A limit that is missing or not digits only: the usage error after the
/// gate, nothing sent.
#[test]
fn hooks_status_limit_is_digits() {
    let rig = Rig::new("hooks", "limit");
    let no = |name, args| case(name, args, &[], vec![], owner_and_port, 2, 0);
    for c in [
        // native: hooks status
        no("--limit as the last word", &["status", "--limit"]),
        no("-n as the last word", &["status", "-n"]),
        no("--limit after --json", &["status", "--json", "--limit"]),
        no("--limit 5x", &["status", "--limit", "5x"]),
        no("--limit -3", &["status", "--limit", "-3"]),
        no("--limit empty", &["status", "--limit", ""]),
        no("--limit with a space", &["status", "--limit", "5 6"]),
        no("--limit with a query", &["status", "--limit", "5&x=1"]),
        no("--limit --json", &["status", "--limit", "--json"]),
        no("-n 1.5", &["status", "-n", "1.5"]),
        no(
            "a bad second limit",
            &["status", "--limit", "5", "--limit", "x"],
        ),
        no("a bad -n then --limit", &["status", "-n", "x", "--limit"]),
        case(
            "the gate first",
            &["status", "--limit", "x"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}

#[test]
fn hooks_install_as_the_shell_does() {
    for (verb, tag) in [("install", "install"), ("uninstall", "uninstall")] {
        let rig = Rig::new("hooks", tag);
        let args: &'static [&'static str] = if verb == "install" {
            &["install"]
        } else {
            &["uninstall"]
        };
        let json: &'static [&'static str] = if verb == "install" {
            &["install", "--json"]
        } else {
            &["uninstall", "x", "--json"]
        };
        let v = vec![
            // native: hooks install
            // native: hooks uninstall
            one(
                "a report",
                args,
                r#"{"report":{"action":"install","script":{"state":"written","path":"/h/notify.sh"},"claudeVersion":"2.1","claudeEventSet":"full","clis":[{"cli":"claude","state":"injected","path":"/h/.claude/settings.json"},{"cli":"gemini","state":"config_unreadable","path":"/h/g"},{"cli":"cursor","state":"absent","path":"/h/c","error":"denied"}]}}"#,
            ),
            one(
                "a removal with the setting off",
                args,
                r#"{"agentHooksSetting":false,"report":{"action":"remove","clis":[{"cli":true,"state":1.5}]}}"#,
            ),
            one(
                "install with no version",
                args,
                r#"{"report":{"action":"install"}}"#,
            ),
            one("skipped", args, r#"{"skipped":"agentHooks off"}"#),
            one("an empty object", args, "{}"),
            case(
                "an error key: its Error line, exit 1",
                args,
                &[],
                vec![ok(r#"{"error":"denied"}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "an error object: its hint",
                args,
                &[],
                vec![ok(r#"{"error":{"code":"c","hint":"the hint"}}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "an error object: its code",
                args,
                &[],
                vec![ok(r#"{"error":{"code":"admin_only","hint":""}}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "an error null",
                args,
                &[],
                vec![ok(r#"{"error":null,"skipped":"x"}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "an error with a lone surrogate",
                args,
                &[],
                vec![ok(r#"{"error":"bad \ud800 name"}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "a 500 with an error",
                args,
                &[],
                vec![Reply::Answer(
                    500,
                    r#"{"error":"installer worker: task panicked"}"#.into(),
                )],
                owner_and_port,
                1,
                1,
            ),
            case(
                "not UTF-8: raw",
                args,
                &[],
                vec![Reply::Raw(not_utf8())],
                owner_and_port,
                0,
                1,
            ),
            one(
                "a skipped value that can't be printed: raw",
                args,
                r#"{"skipped":"\ud800"}"#,
            ),
            one("a string with error in it: raw", args, r#""an error here""#),
            one("a list: raw", args, r#"["x"]"#),
            one("a number: raw", args, "5"),
            one("not JSON: raw", args, "nope"),
            one(
                "clis not a list: each key has no get",
                args,
                r#"{"report":{"action":"x","clis":{"a":1}}}"#,
            ),
            one(
                "a cli's field that can't be formatted",
                args,
                r#"{"report":{"clis":[{"cli":null}]}}"#,
            ),
            one("--json", json, r#"{"report":{"action":"install"}}"#),
            one("--json: not JSON", json, "nope"),
            case(
                "--json: an error key, as it came, exit 1",
                json,
                &[],
                vec![ok(r#"{"error":"denied"}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "--json: an error object, exit 1",
                json,
                &[],
                vec![Reply::Answer(500, r#"{"error":{"code":"c"}}"#.into())],
                owner_and_port,
                1,
                1,
            ),
            case(
                "--json: a refusal, as it came, Ask with, exit 3",
                json,
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                owner_and_port,
                3,
                1,
            ),
            case(
                "a refusal: the Error line, Ask with, exit 3",
                args,
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                owner_and_port,
                3,
                1,
            ),
            case(
                "a reset",
                args,
                &[],
                vec![Reply::Reset],
                owner_and_port,
                56,
                1,
            ),
        ];
        for c in v {
            answered_here(&rig, &c);
        }
    }
}

/// Help never acts: `-h` or `--help` anywhere prints the hooks page on
/// stdout, exit 0, and sends nothing (`hooks -h` and `hooks --help` are
/// the binary's own page; the front hands the rest to the shell). emit
/// keeps its own page.
#[test]
fn help_never_acts() {
    let rig = Rig::new("hooks", "help");
    for c in [
        case("-h alone", &["-h"], &[], vec![], owner_and_port, 0, 0),
        case(
            "--help alone",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "--help after an unknown verb",
            &["nope", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "install -h",
            &["install", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "install --json --help",
            &["install", "--json", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "uninstall -h",
            &["uninstall", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "status -h",
            &["status", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "status --limit -h",
            &["status", "--limit", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "emit -h: its own page",
            &["emit", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "install --help with no daemon",
            &["install", "--help"],
            &[],
            vec![],
            nothing,
            0,
            0,
        ),
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..c
        });
    }
}

/// `hooks emit` before it reports: the words' stops (the emit page on
/// stderr where the shell prints it), exit 2; outside a session (no
/// passport, or no pane) its two lines, exit 3; with no daemon too (the
/// shell skips its connection gate), past the virtual desktop's guard.
/// With a passport and a pane it reports, which is the shell's.
#[test]
fn hooks_emit_stops_as_the_shell_does() {
    let rig = Rig::new("hooks", "emit");
    let v = vec![
        // native: hooks emit
        case("emit alone: the page", &["emit"], &[], vec![], owner_and_port, 2, 0),
        case("emit alone, no daemon", &["emit"], &[], vec![], nothing, 2, 0),
        case(
            "emit, an unknown flag",
            &["emit", "Stop", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "emit, two events",
            &["emit", "Stop", "SessionEnd"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "emit, an unknown event",
            &["emit", "Stopped"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "emit --json last",
            &["emit", "Stop", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "emit --session-id last",
            &["emit", "--session-id"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "emit, the owner: no session",
            &["emit", "Stop", "-q"],
            &[],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "emit, a passport and no pane",
                &["emit", "Stop", "--json", "{}"],
                &[],
                vec![],
                port_only,
                3,
                0,
            )
        },
        case(
            "emit, a pane and no passport",
            &["emit", "Notification"],
            &[("K2_PANE_ID", "p7")],
            vec![],
            nothing,
            3,
            0,
        ),
        case(
            "emit: the virtual desktop's guard",
            &["emit", "Stop"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        Case {
            env: with_passport(&[("K2_PANE_ID", "")]),
            ..case(
                "emit, an empty pane",
                &["emit", "Stop"],
                &[],
                vec![],
                port_only,
                3,
                0,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The usage names the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` it says `xy`).
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new("hooks", "brand");
    let c = case("brand", &[], &[], vec![], owner_and_port, 1, 0);
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stderr
            .starts_with(&format!("Usage: {cli} hooks status [--limit N] [--json]\n")),
        "{:?}",
        out.stderr
    );
    assert!(out.stderr.contains(&format!("({cli} study hooks)\n")));
    assert_eq!(out.code, Some(1));
    assert!(log.is_empty());
}

/// `hooks emit` outside a session names the command by the brand's name.
#[test]
fn the_emit_session_lines_say_the_brand_s_name() {
    let rig = Rig::new("hooks", "emitbrand");
    let cli = k2_client::brand::CLI_NAME;
    let c = case("brand", &["emit", "Stop"], &[], vec![], owner_and_port, 3, 0);
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stderr
            .starts_with(&format!("Error: {cli} hooks emit runs inside a ")),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.ends_with(&format!("See {cli} study hooks.\n")),
        "{}",
        out.stderr
    );
    assert_eq!(out.code, Some(3));
    assert!(log.is_empty(), "nothing sent: {log:?}");
}
