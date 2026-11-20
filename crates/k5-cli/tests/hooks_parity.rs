//! `k5-cli hooks` against the shell CLI's `hooks` arm, `cmd_hooks_status`
//! and `cmd_hooks_install` (`cli/src/router.sh`, `cli/src/remind.sh`), on
//! the parity rig ([`common::parity`]): the same requests, stdout, stderr
//! and exit code.
//!
//! - No verb or an unknown one: the usage lines, exit 1, nothing sent;
//!   the gate first.
//! - `status`: the limit (default, `--limit`, `-n`), `--json` (echoed,
//!   echo's own options too), the reader's lines for a bare answer, a full
//!   one, each section's odd shapes, and where the reader raises: what it
//!   printed, then the raw answer, exit 0. One answer above 256 KiB.
//! - `install` and `uninstall`: the body, the reader's lines (a report,
//!   skipped, the `error` key whose line the shell throws away), `--json`.
//! - No answer: a reset.
//! - Help: the shell's (`hooks status -h` and `hooks install -h` reach the
//!   verb, which skips the word and sends: a bash bug, reported, whose rows
//!   flip when the fix lands).
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

#[test]
fn hooks_with_no_verb() {
    let rig = Rig::new("hooks", "usage");
    for c in [
        // native: hooks
        case("alone", &[], &[], vec![], owner_and_port, 1, 0),
        case("an unknown verb", &["nope", "x"], &[], vec![], owner_and_port, 1, 0),
        case("an empty verb", &[""], &[], vec![], owner_and_port, 1, 0),
        case("--json", &["--json"], &[], vec![], owner_and_port, 1, 0),
        case("the gate: no port", &[], &[], vec![], owner_only, 1, 0),
        case("the gate: no token", &["status"], &[], vec![], port_only, 1, 0),
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
        one("-n, the last wins", &["status", "-n", "5", "--limit", "7"], "{}"),
        one("--json read as --limit's value", &["status", "--limit", "--json"], "{}"),
        one("other words are skipped", &["status", "x", "y"], "{}"),
        one("--json", &["status", "--json"], FULL),
        one("--json: trailing newlines go", &["status", "--json"], "{\"a\":1}\n\n"),
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
        one(
            "clis not a dict",
            &["status"],
            r#"{"clis":[1]}"#,
        ),
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
        one(
            "totals not a dict",
            &["status"],
            r#"{"totals":[1]}"#,
        ),
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
            "an error status: the reader's lines",
            &["status"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"boom"}"#.into())],
            owner_and_port,
            0,
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
            one(
                "an error key: its line thrown away, then raw",
                args,
                r#"{"error":"denied"}"#,
            ),
            one(
                "a string with error in it: raw",
                args,
                r#""an error here""#,
            ),
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
            case(
                "an error status: the reader's lines",
                args,
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                owner_and_port,
                0,
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

/// Help is the shell's. `hooks install -h` and `hooks status -h` reach
/// the verb, which skips `-h` and sends (a bash bug in
/// `cli/src/remind.sh`'s `cmd_hooks_install` and `cmd_hooks_status`
/// loops, reported). These rows flip to "nothing sent" when the fix
/// lands.
#[test]
fn help_is_the_shell_s() {
    let rig = Rig::new("hooks", "help");
    for c in [
        case("-h alone", &["-h"], &[], vec![], owner_and_port, 1, 0),
        case("--help after a verb", &["nope", "--help"], &[], vec![], owner_and_port, 1, 0),
        case("install -h sends (bash bug)", &["install", "-h"], &[], vec![ok("{}")], owner_and_port, 0, 1),
        case("status -h sends (bash bug)", &["status", "-h"], &[], vec![ok("{}")], owner_and_port, 0, 1),
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..c
        });
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
