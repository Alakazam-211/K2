//! `k5-cli notifications` and `notify` against the shell CLI's, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - `notifications`: the help, `email status` (and `--json`), each
//!   `sender`, `system-sender`, `relay`, `address`, `suppressions` and
//!   `agent` verb, `log` (with one list above 256 KiB) and `settings`,
//!   with their usage lines and the readers' tracebacks.
//! - `notify`: the help, `email off`, `mute`, `unmute` and a notice from a
//!   word or a file, and their usage lines.
//! - A refusal, a `vault_confirm_required` that names no action (no
//!   confirm is asked), no answer, and the relay password from stdin.
//! - `-h` and `--help` are the shell's, and send nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const STATUS: &str = r#"{"boxSwitch":"on","hostedMail":false,"airgap":true,"systemSender":{"address":"noreply@x.example","state":"ready"},"senders":[{"id":"s1","address":"noreply@x.example","path":"relay","state":"ready","owner":null},{"id":"s2","address":"é@x.example","path":"hosted","state":"paused","owner":"ada"}],"relays":[{"id":"r1","host":"smtp.example","port":587,"tls":"starttls","username":"u","fromDomain":"x.example","lastTestOk":true},{"id":"r2","lastTestOk":null}],"settings":{"logRetentionDays":30,"ownerEmail":null}}"#;
const SENDER: &str = r#"{"sender":{"id":"s1","address":"noreply@x.example","path":"relay","state":"ready","owner":"ada","fromName":"X","envelope":"b.x.example","notReady":{"code":"dkim","hint":"publish the key"},"next":"k2 notifications email sender check s1"}}"#;
const TEST: &str = r#"{"id":"t1","result":"sent","to":"o@x.example","checks":{"dmarc":{"ok":true,"at":"_dmarc.x.example","policy":"none"},"dkimPublished":true,"spfPublished":false,"note":"check the inbox"}}"#;

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
fn notifications_answer_as_the_shell_does() {
    let rig = Rig::new("notifications", "all");
    let log = {
        let rows: Vec<String> = (0..2500)
            .map(|i| {
                format!(
                    r#"{{"createdAt":{},"id":"m{i}","status":"sent","to":["p{i}@x.example","q@x.example"],"subject":"notice {i} é with a subject long enough to be cut at sixty characters","error":"{}"}}"#,
                    1_763_600_000 + i * 97,
                    if i % 3 == 0 { "bounced" } else { "" }
                )
            })
            .collect();
        format!(r#"{{"items":[{}]}}"#, rows.join(","))
    };
    assert!(log.len() > 256 * 1024);
    let v = vec![
        // covers: notifications
        run("the help", &[], vec![], 0),
        run("help", &["help"], vec![], 0),
        stop("another word", &["sms"], 2),
        case("the help needs the daemon", &[], &[], vec![], nothing, 1, 0),
        // covers: notifications email
        run("email: the status", &["email"], vec![ok(STATUS)], 0),
        // covers: notifications email --json
        run("email --json", &["email", "--json"], vec![ok(STATUS)], 0),
        run(
            "email status --json",
            &["email", "status", "--json"],
            vec![ok(STATUS)],
            0,
        ),
        traceback(run(
            "email: a system sender with no address",
            &["email", "status"],
            vec![ok(r#"{"systemSender":{"state":"x"}}"#)],
            1,
        )),
        stop("email: a verb it doesn't know", &["email", "send"], 2),
        // covers: notifications email sender add
        run(
            "sender add",
            &[
                "email",
                "sender",
                "add",
                "noreply@x.example",
                "--name",
                "X é",
                "--lock-name",
                "--reply-to",
                "a@x",
                "--reply-to",
                "",
                "--via",
                "b",
            ],
            vec![ok(SENDER)],
            0,
        ),
        stop(
            "sender add: no address",
            &["email", "sender", "add", "--lock-name"],
            2,
        ),
        stop(
            "sender add: --name last",
            &["email", "sender", "add", "a@x", "--name"],
            1,
        ),
        // covers: notifications email sender list
        run(
            "sender list",
            &["email", "sender"],
            vec![ok(
                r#"{"senders":[{"id":"s1","address":"a@x","path":"relay","state":"ready","owner":"ada","fromName":"A","paused":{"reason":"bounces"}},{"id":"s2"}]}"#,
            )],
            0,
        ),
        // covers: notifications email sender list --json
        run(
            "sender list --all --json",
            &["email", "sender", "list", "--all", "--json"],
            vec![ok(r#"{"senders":[]}"#)],
            0,
        ),
        run(
            "sender list: none",
            &["email", "sender", "ls"],
            vec![ok(r#"{"senders":[]}"#)],
            0,
        ),
        // covers: notifications email sender check
        run(
            "sender check",
            &["email", "sender", "check", "s1"],
            vec![ok(SENDER)],
            0,
        ),
        // covers: notifications email sender set
        run(
            "sender set",
            &[
                "email",
                "sender",
                "set",
                "s1",
                "--unlock-name",
                "--reply-to",
                "r@x",
            ],
            vec![ok(SENDER)],
            0,
        ),
        // covers: notifications email sender rm
        run(
            "sender remove",
            &["email", "sender", "remove", "s1"],
            vec![ok(r#"{"removed":"s1","address":"a@x","reservedDays":30}"#)],
            0,
        ),
        // covers: notifications email sender resume
        run(
            "sender resume",
            &["email", "sender", "resume", "s1"],
            vec![ok(r#"{"sender":"s1"}"#)],
            0,
        ),
        stop(
            "sender resume: two",
            &["email", "sender", "resume", "a", "b"],
            2,
        ),
        stop(
            "sender: a verb it doesn't know",
            &["email", "sender", "pause"],
            2,
        ),
        run(
            "a confirm the answer names no action for",
            &["email", "sender", "resume", "s1"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"vault_confirm_required","hint":"confirm it"}}"#.into(),
            )],
            3,
        ),
        // covers: notifications email system-sender
        run(
            "system-sender",
            &["email", "system-sender"],
            vec![ok(r#"{"systemSender":null}"#)],
            0,
        ),
        run(
            "system-sender set",
            &["email", "system-sender", "set", "s1"],
            vec![ok(SENDER)],
            0,
        ),
        run(
            "system-sender clear",
            &["email", "system-sender", "clear"],
            vec![ok("{}")],
            0,
        ),
        run(
            "system-sender test",
            &["email", "system-sender", "test"],
            vec![ok(TEST)],
            0,
        ),
        stop(
            "system-sender set: no sender",
            &["email", "system-sender", "set"],
            2,
        ),
        // covers: notifications email relay add
        Case {
            stdin: Some(b"pw \xc3\xa9 1\nsecond line\n"),
            ..run(
                "relay add: the password from stdin",
                &[
                    "email",
                    "relay",
                    "add",
                    "--host",
                    "smtp.example",
                    "--port",
                    "0587",
                    "--tls",
                    "starttls",
                    "--username",
                    "u",
                    "--from-domain",
                    "x.example",
                ],
                vec![ok(
                    r#"{"relay":{"id":"r1","host":"smtp.example","port":587,"tls":"starttls","fromDomain":"x.example"},"next":"test it"}"#,
                )],
                0,
            )
        },
        Case {
            stdin: Some(b"pw"),
            ..traceback(run(
                "relay add: next is null",
                &[
                    "email",
                    "relay",
                    "add",
                    "--host",
                    "h",
                    "--username",
                    "u",
                    "--from-domain",
                    "d",
                ],
                vec![ok(r#"{"relay":{},"next":null}"#)],
                1,
            ))
        },
        stop(
            "relay add: no password",
            &[
                "email",
                "relay",
                "add",
                "--host",
                "h",
                "--username",
                "u",
                "--from-domain",
                "d",
            ],
            2,
        ),
        stop(
            "relay add: a port that is not a number",
            &[
                "email",
                "relay",
                "add",
                "--host",
                "h",
                "--port",
                "5x",
                "--username",
                "u",
                "--from-domain",
                "d",
            ],
            2,
        ),
        stop(
            "relay add: a stray word",
            &["email", "relay", "add", "h"],
            2,
        ),
        // covers: notifications email relay list
        run(
            "relay list",
            &["email", "relay"],
            vec![ok(
                r#"{"relays":[{"id":"r1","host":"h","port":25,"tls":"implicit","username":"u","fromDomain":"d","lastTestOk":false}]}"#,
            )],
            0,
        ),
        run(
            "relay list: none",
            &["email", "relays", "ls"],
            vec![ok("{}")],
            0,
        ),
        // covers: notifications email relay rm
        run(
            "relay rm",
            &["email", "relay", "rm", "r1"],
            vec![ok("{}")],
            0,
        ),
        // covers: notifications email relay test
        run(
            "relay test",
            &["email", "relay", "test", "r1", "--from", "s1"],
            vec![ok(TEST)],
            0,
        ),
        stop(
            "relay test: no --from",
            &["email", "relay", "test", "r1"],
            2,
        ),
        // covers: notifications email address
        run(
            "address",
            &["email", "address"],
            vec![ok(
                r#"{"address":{"address":"me@x.example"},"pending":[{"address":"new@x.example","expiresAt":1763600000}]}"#,
            )],
            0,
        ),
        // covers: notifications email address --json
        run(
            "address --json",
            &["email", "address", "--json"],
            vec![ok(r#"{"address":null}"#)],
            0,
        ),
        run(
            "address --person",
            &["email", "address", "--person", "bo é"],
            vec![ok(r#"{"address":null}"#)],
            0,
        ),
        stop(
            "address --person with no login",
            &["email", "address", "show", "--person", ""],
            2,
        ),
        // covers: notifications email address set
        run(
            "address set",
            &["email", "address", "set", "me@x.example"],
            vec![Reply::Answer(
                202,
                r#"{"address":{"address":"me@x.example"}}"#.into(),
            )],
            0,
        ),
        // covers: notifications email address set --json
        run(
            "address set --json",
            &["email", "address", "set", "me@x.example", "--json"],
            vec![ok("{}")],
            0,
        ),
        stop(
            "address set: for another person",
            &["email", "address", "set", "a@x", "--person", "bo"],
            2,
        ),
        // covers: notifications email address remove
        run(
            "address remove",
            &["email", "address", "rm", "--person", "bo"],
            vec![ok(r#"{"removed":1}"#)],
            0,
        ),
        // covers: notifications email address remove --json
        run(
            "address remove --json",
            &["email", "address", "remove", "a@x", "--json"],
            vec![ok(r#"{"removed":1}"#)],
            0,
        ),
        run(
            "address remove: a 202 is a refusal there",
            &["email", "address", "remove"],
            vec![Reply::Answer(202, r#"{"removed":0}"#.into())],
            3,
        ),
        // covers: notifications email suppressions
        run(
            "suppressions",
            &["email", "suppressions"],
            vec![ok(
                r#"{"suppressions":[{"createdAt":1763600000,"domain":"x.example","reason":"bounce","scope":"box","addressSha256":"0123456789abcdef"},{"scope":"sender","domain":"y","reason":"complaint","addressSha256":"ab"}]}"#,
            )],
            0,
        ),
        // covers: notifications email suppressions --json
        run(
            "suppressions for one address",
            &["email", "suppressions", "a@x", "--limit", "5", "--json"],
            vec![ok(r#"{"suppressions":[]}"#)],
            0,
        ),
        run(
            "suppressions: none for one",
            &["email", "suppression", "list", "a@x"],
            vec![ok(r#"{"suppressions":[]}"#)],
            0,
        ),
        traceback(run(
            "suppressions: no hash",
            &["email", "suppressions"],
            vec![ok(r#"{"suppressions":[{"scope":"box"}]}"#)],
            1,
        )),
        stop(
            "suppressions: a word that is not a verb or an address",
            &["email", "suppressions", "lst"],
            2,
        ),
        // covers: notifications email suppressions add
        run(
            "suppressions add",
            &["email", "suppressions", "add", "a@x", "--sender", "s1"],
            vec![ok(r#"{"added":true,"scope":"sender"}"#)],
            0,
        ),
        // covers: notifications email suppressions add --json
        run(
            "suppressions add --json",
            &["email", "suppressions", "add", "a@x", "--json"],
            vec![ok(r#"{"added":false}"#)],
            0,
        ),
        // covers: notifications email suppressions rm
        run(
            "suppressions rm --all",
            &["email", "suppressions", "remove", "a@x", "--all"],
            vec![ok(r#"{"removed":2}"#)],
            0,
        ),
        // covers: notifications email suppressions rm --json
        run(
            "suppressions rm --json",
            &["email", "suppressions", "rm", "a@x", "--json"],
            vec![ok(r#"{"removed":2}"#)],
            0,
        ),
        stop(
            "suppressions add: no address",
            &["email", "suppressions", "add"],
            2,
        ),
        // covers: notifications email agent
        run(
            "agent",
            &["email", "agent"],
            vec![ok(
                r#"{"agents":[{"workspace":"ws é","on":true,"sender":"a@x"},{"workspace":"w2","on":false,"sender":null,"offReason":"sender removed"}]}"#,
            )],
            0,
        ),
        // covers: notifications email agent --json
        run(
            "agent --json",
            &["email", "agents", "--json"],
            vec![ok(r#"{"agents":[]}"#)],
            0,
        ),
        run(
            "agent: none",
            &["email", "agent", "list"],
            vec![ok(r#"{"agents":[]}"#)],
            0,
        ),
        // covers: notifications email agent on
        run(
            "agent on",
            &["email", "agent", "on", "ws", "--sender", "a@x"],
            vec![ok(r#"{"workspace":"ws","sender":"a@x"}"#)],
            0,
        ),
        // covers: notifications email agent off
        run(
            "agent off",
            &["email", "agent", "off", "ws"],
            vec![ok(r#"{"workspace":"ws"}"#)],
            0,
        ),
        stop("agent on: no sender", &["email", "agent", "on", "ws"], 2),
        stop(
            "agent: two workspaces",
            &["email", "agent", "off", "a", "b"],
            2,
        ),
        // covers: notifications email log
        run(
            "log",
            &["email", "log", "--app", "a b", "--limit", "3"],
            vec![ok(&log)],
            0,
        ),
        run("log: none", &["email", "log"], vec![ok("{}")], 0),
        // covers: notifications email log --json
        run("log --json", &["email", "log", "--json"], vec![ok("{}")], 0),
        stop("log: a stray word", &["email", "log", "all"], 2),
        // covers: notifications email settings
        run(
            "settings",
            &[
                "email",
                "settings",
                "--retention-days",
                " 1_4 ",
                "--owner-email",
                "",
            ],
            vec![ok(r#"{"settings":{"logRetentionDays":14}}"#)],
            0,
        ),
        traceback(stop(
            "settings: days that are not a number",
            &["email", "settings", "--retention-days", "2w"],
            2,
        )),
        run(
            "settings: refused",
            &["email", "settings"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
        ),
        run("no answer", &["email"], vec![Reply::Close], 3),
        // `-h` there is an unknown flag to bash (no request).
        shell(stop("-h: an unknown flag", &["email", "relay", "add", "-h"], 2)),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    for (verb, args, want) in [
        (
            "notifications",
            &["sms"][..],
            format!("usage: {cli} notifications email \u{2026} ({cli} notifications --help)\n"),
        ),
        (
            "notifications",
            &["email", "relay", "rm"],
            format!("usage: {cli} notifications email relay rm <relay>\n"),
        ),
    ] {
        let rig = Rig::new(verb, "brand");
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
