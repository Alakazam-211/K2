//! `k5-cli send-feedback` against the shell CLI's, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - The pages that need no daemon (no verb: the help on stderr, exit 2;
//!   `help`) and the virtual desktop's guard.
//! - A report: its text from a word or stdin, `--category`, `--scope`,
//!   `--command`, `--json`, the reads of its state until it is no longer
//!   waiting, `--diagnostics-preview`, `--diagnostics --yes`, and
//!   `--diagnostics` with no terminal to ask on; every usage line.
//! - `list` (rows, none, `--state`, `--json`, one list above 256 KiB),
//!   `show`, `retry`, `cancel`.
//! - `settings`: the page for a 0.46.17 server and an older one, `--json`,
//!   each write and the read before it, an older server refusing
//!   `--agents cli`, and the usage lines.
//! - A refusal, an answer the reader can't read, and no answer.
//! - `-h` and `--help` are the shell's, and send nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const SENT: &str =
    r#"{"item":{"id":"pf_1","state":"sent","category":"bug","hasDiagnostics":false}}"#;
const QUEUED_ERR: &str = r#"{"item":{"id":"pf_1","state":"queued","category":"idea","error":"offline","errorMessage":"No K2 Connect pairing yet."}}"#;
const SHOW: &str = r#"{"item":{"id":"pf_1","state":"sent","category":"bug","sender":"ada","senderKind":"person","client":"cli","scope":"cli","createdAt":1763600000000,"sentAt":1763600005000,"remoteId":"r-9","text":"It broke é\ntwice","diagnostics":[{"title":"Versions","text":"k2 0.46.16"}]}}"#;
const NEW_SETTINGS: &str = r#"{"enabled":true,"agents":true,"agentPlatform":true,"agentsAsked":true,"agentsAskedBy":"owner","agentsAskedAt":1763600000000,"agentAttach":["logs"],"attachParts":[{"id":"logs","label":"Recent logs"},{"id":"config","label":"Settings"}],"canSend":false,"message":"Pair with K2 Connect first.","service":{"ok":false,"message":"unreachable"},"pending":3,"endpoint":"https://feedback.example"}"#;
const OLD_SETTINGS: &str = r#"{"enabled":true,"agents":false,"agentDiagnostics":true,"canSend":true,"pending":0,"endpoint":"https://feedback.example"}"#;
const DIAG: &str = r#"{"draftId":"dr_1","sections":[{"id":"versions","title":"Versions","text":"k2 0.46.16\nmacOS"},{"id":"logs","title":"Logs","text":""}]}"#;

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn run(name: &'static str, args: &'static [&'static str], replies: Vec<Reply>, code: i32) -> Case {
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, code, n)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

#[test]
fn the_pages_and_a_report() {
    let rig = Rig::new("send-feedback", "send");
    let v = vec![
        case(
            "no verb: the help on stderr",
            &[],
            &[],
            vec![],
            nothing,
            2,
            0,
        ),
        case("an empty word", &[""], &[], vec![], nothing, 2, 0),
        case("help", &["help"], &[], vec![], nothing, 0, 0),
        case(
            "the virtual desktop's guard",
            &["help"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            port_only,
            3,
            0,
        ),
        case(
            "a report needs the daemon",
            &["hi"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        // covers: send-feedback
        run(
            "a report: sent",
            &["It broke"],
            vec![ok(r#"{"id":"pf_1"}"#), ok(SENT)],
            0,
        ),
        // covers: send-feedback --json
        run(
            "a report with every word, --json",
            &[
                "--category",
                "bug",
                "--scope=cli",
                "--command",
                "k2 x é",
                "--json",
                "--",
                "-dash first",
            ],
            vec![ok(r#"{"id":"pf_1"}"#), ok(SENT)],
            0,
        ),
        run(
            "queued with a reason: one read",
            &["--category=idea", "an idea"],
            vec![ok(r#"{"id":"pf_1"}"#), ok(QUEUED_ERR)],
            0,
        ),
        run(
            "sending, then refused",
            &["x"],
            vec![
                ok(r#"{"id":"pf 1"}"#),
                ok(r#"{"item":{"id":"pf 1","state":"sending","category":"other"}}"#),
                ok(
                    r#"{"item":{"id":"pf 1","state":"refused","category":"other","error":"too_long","hasDiagnostics":true}}"#,
                ),
            ],
            0,
        ),
        Case {
            stdin: Some(b"from\nstdin \xc3\xa9\n\n"),
            env: vec![("LANG", "en_US.UTF-8".to_string())],
            ..run(
                "text on stdin, a locale",
                &["-"],
                vec![ok(r#"{"id":"pf_1"}"#), ok(SENT)],
                0,
            )
        },
        run(
            "--diagnostics-preview",
            &["--diagnostics-preview"],
            vec![ok(DIAG)],
            0,
        ),
        run(
            "--diagnostics-preview --json",
            &["--diagnostics-preview", "--json"],
            vec![ok(DIAG)],
            0,
        ),
        run(
            "--diagnostics --yes: the draft goes with it",
            &["--diagnostics", "-y", "slow"],
            vec![ok(DIAG), ok(r#"{"id":"pf_1"}"#), ok(SENT)],
            0,
        ),
        run(
            "--diagnostics with no terminal to ask on",
            &["--diagnostics", "slow"],
            vec![ok(DIAG)],
            2,
        ),
        run(
            "answered without an id",
            &["x"],
            vec![ok(r#"{"ok":true}"#)],
            3,
        ),
        traceback(run(
            "the state is not JSON",
            &["x"],
            vec![ok(r#"{"id":"a"}"#), ok("<html>")],
            1,
        )),
        run(
            "refused: turned off",
            &["x"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"feedback_off","hint":"Turned off on this server."}}"#.into(),
            )],
            3,
        ),
        run("no answer", &["x"], vec![Reply::Close], 3),
        stop("two texts", &["a", "b"], 2),
        stop("an unknown flag", &["--bogus", "a"], 2),
        stop("--category last", &["a", "--category"], 2),
        stop(
            "a category it doesn't know",
            &["--category", "rant", "a"],
            2,
        ),
        stop("--scope cli with no command", &["--scope", "cli", "a"], 2),
        stop("--scope of another word", &["--scope", "web", "a"], 2),
        stop("--command on two lines", &["--command", "a\nb", "a"], 2),
        stop("no text", &["--json"], 2),
        stop(
            "both diagnostics",
            &["--diagnostics", "--diagnostics-preview", "a"],
            2,
        ),
        shell(stop("-h: the help", &["a", "-h"], 0)),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn list_show_retry_and_cancel() {
    let rig = Rig::new("send-feedback", "list");
    let many = {
        let rows: Vec<String> = (0..2500)
            .map(|i| {
                format!(
                    r#"{{"id":"pf_{i}","state":"queued","category":"bug","createdAt":{},"text":"report {i}   with   spaces and a long tail that goes past sixty characters é","mine":{},"sender":"agent-{i}","scope":"cli","errorMessage":"waiting {i}"}}"#,
                    1_763_600_000_000_u64 + i * 61_000,
                    i % 2 == 0
                )
            })
            .collect();
        format!(r#"{{"items":[{}]}}"#, rows.join(","))
    };
    assert!(many.len() > 256 * 1024);
    let v = vec![
        run(
            "list",
            &["list"],
            vec![ok(
                r#"{"items":[{"id":"pf_1","state":"sent","category":"bug","createdAt":1763600000000,"text":"short","mine":true},{"id":"pf_2","state":"failed","category":"praise","createdAt":1763600000500.5,"text":"x","sender":"bo","errorMessage":"gone"}]}"#,
            )],
            0,
        ),
        run("list: none", &["list"], vec![ok(r#"{"items":[]}"#)], 0),
        run(
            "list: none on the server",
            &["list", "--state", "queued"],
            vec![ok(r#"{"all":true}"#)],
            0,
        ),
        run("list: many", &["list"], vec![ok(&many)], 0),
        run(
            "list --json",
            &["list", "--json"],
            vec![ok(r#"{"items":[]}"#)],
            0,
        ),
        traceback(run(
            "list: a sender that is a number",
            &["list"],
            vec![ok(
                r#"{"items":[{"id":"a","state":"s","category":"c","createdAt":0,"sender":7}]}"#,
            )],
            1,
        )),
        stop("list: --state last", &["list", "--state"], 2),
        stop("list: a stray word", &["list", "all"], 2),
        run("show", &["show", "pf_1"], vec![ok(SHOW)], 0),
        run(
            "show: queued",
            &["show", "pf 1"],
            vec![ok(
                r#"{"item":{"id":"pf_1","state":"queued","category":"idea","createdAt":0,"nextAttemptAt":1763600060000,"attempts":2,"errorMessage":"offline","contactEmail":"a@example.com"}}"#,
            )],
            0,
        ),
        run(
            "show --json",
            &["show", "--json", "pf_1"],
            vec![ok(SHOW)],
            0,
        ),
        traceback(run("show: no item", &["show", "pf_1"], vec![ok("{}")], 1)),
        run(
            "show: not found",
            &["show", "pf_9"],
            vec![Reply::Answer(
                404,
                r#"{"error":"not_found","message":"no such feedback"}"#.into(),
            )],
            3,
        ),
        stop("show: no id", &["show"], 2),
        stop("show: two ids", &["show", "a", "b"], 2),
        stop("show: a flag", &["show", "--all"], 2),
        run("retry", &["retry", "pf_1"], vec![ok("{}")], 0),
        run("cancel", &["cancel", "pf_1"], vec![ok("{}")], 0),
        stop("retry: no id", &["retry"], 2),
        stop("cancel: two ids", &["cancel", "a", "b"], 2),
        shell(stop("cancel -h: the help", &["cancel", "-h"], 0)),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn settings_answer_as_the_shell_does() {
    let rig = Rig::new("send-feedback", "settings");
    let v = vec![
        run(
            "settings: a 0.46.17 server",
            &["settings"],
            vec![ok(NEW_SETTINGS)],
            0,
        ),
        run(
            "settings: an older server",
            &["settings"],
            vec![ok(OLD_SETTINGS)],
            0,
        ),
        run(
            "settings --json",
            &["settings", "--json"],
            vec![ok(OLD_SETTINGS)],
            0,
        ),
        run(
            "--enabled off: one write",
            &["settings", "--enabled", "off", "--agent-diagnostics", "yes"],
            vec![ok(OLD_SETTINGS)],
            0,
        ),
        run(
            "--agents cli: a read, then the write",
            &["settings", "--agents", "cli", "--attach", "logs, config"],
            vec![ok(NEW_SETTINGS), ok(NEW_SETTINGS)],
            0,
        ),
        run(
            "--agents on, an older server: agents only",
            &["settings", "--agents", "on", "--json"],
            vec![ok(OLD_SETTINGS), ok(OLD_SETTINGS)],
            0,
        ),
        run(
            "--agents all, an older server: nothing changed",
            &["settings", "--agents", "all"],
            vec![ok(OLD_SETTINGS)],
            1,
        ),
        run(
            "--agents off",
            &["settings", "--agents", "0"],
            vec![ok(NEW_SETTINGS)],
            0,
        ),
        run(
            "a write refused",
            &["settings", "--enabled", "on"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
        ),
        stop(
            "--enabled with a word it doesn't know",
            &["settings", "--enabled", "maybe"],
            2,
        ),
        stop("--agents last", &["settings", "--agents"], 2),
        stop("--attach blank", &["settings", "--attach", " "], 2),
        stop(
            "--attach and --agent-diagnostics",
            &["settings", "--attach", "a", "--agent-diagnostics", "on"],
            2,
        ),
        stop("a stray word", &["settings", "on"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command and the product by the brand's names (in a
/// build with `K2_BRAND_DIR=brand/xy` they say `xy` and `XY`).
#[test]
fn the_lines_say_the_brand_s_names() {
    let rig = Rig::new("send-feedback", "brand");
    let (cli, product) = (k2_client::brand::CLI_NAME, k2_client::brand::PRODUCT_NAME);
    for (args, want) in [
        (
            &["a", "b"][..],
            format!("usage: one text (quote it): {cli} send-feedback \"<text>\"\n"),
        ),
        (
            &["--scope", "web", "a"],
            format!("usage: --scope is cli (a problem with a {product} command) or platform (anything else about {product})\n"),
        ),
        (&["retry"], format!("usage: {cli} send-feedback retry <id>\n")),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// In a session (`HOOK_SOCK` set) every request carries the session's
/// passport, never the owner token from disk, also over TCP when there is
/// no cell socket; a session with no passport is one line, exit 3, and
/// nothing is sent, the words' usage lines first (`_sf_session_token`).
#[test]
fn a_session_sends_its_passport_or_nothing() {
    let rig = Rig::new("send-feedback", "session");
    let no_passport = |name: &'static str, args: &'static [&'static str], code: i32| {
        case(
            name,
            args,
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![],
            owner_and_port,
            code,
            0,
        )
    };
    let passport = |name: &'static str, args: &'static [&'static str], replies: Vec<Reply>| {
        let n = replies.len();
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            ..case(name, args, &[], replies, port_only, 0, n)
        }
    };
    let v = vec![
        no_passport("no passport: list", &["list"], 3),
        no_passport("no passport: show", &["show", "pf_1"], 3),
        no_passport("no passport: retry", &["retry", "pf_1"], 3),
        no_passport("no passport: cancel", &["cancel", "pf_1"], 3),
        no_passport("no passport: settings", &["settings"], 3),
        no_passport(
            "no passport: settings --enabled off",
            &["settings", "--enabled", "off"],
            3,
        ),
        no_passport(
            "no passport: the opt-in",
            &["settings", "--agents", "cli"],
            3,
        ),
        no_passport("no passport: a report", &["it broke"], 3),
        no_passport(
            "no passport: a report with diagnostics",
            &["it broke", "--diagnostics", "--yes"],
            3,
        ),
        no_passport("no passport: a usage line first", &["list", "--state"], 2),
        no_passport("no passport: show with no id", &["show"], 2),
        passport("a passport over TCP: list", &["list"], vec![ok(r#"{"items":[]}"#)]),
        passport(
            "a passport over TCP: the opt-in",
            &["settings", "--agents", "cli"],
            vec![ok(NEW_SETTINGS), ok(NEW_SETTINGS)],
        ),
        passport(
            "a passport over TCP: a report with diagnostics",
            &["x broke", "--category", "bug", "--diagnostics", "--yes"],
            vec![ok(DIAG), ok(r#"{"id":"pf_1"}"#), ok(SENT)],
        ),
        passport("a passport over TCP: retry", &["retry", "pf_1"], vec![ok("{}")]),
    ];
    for c in v {
        rig.check(&c);
    }
}
