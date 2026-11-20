//! `k5-cli hostmail status`, `domain`, `create`, `delete`, `doctor` and
//! `config` against the shell CLI's `cli/k2 hostmail …`, on the parity rig
//! ([`common::parity`]): the same requests (urllib's, from `_mail_py`),
//! stdout, stderr and exit code.
//!
//! - The words: each usage line, exit 2 and nothing sent.
//! - status (running, not installed, not supported, disagreeing with
//!   systemd, an older daemon's envelope), each domain verb, create,
//!   delete, doctor and config (get, set, the forward caps).
//! - `config --relay-pass-stdin`: the password read as `$(cat)` reads
//!   it, sent in the request's body, and on no child's argv or
//!   environment.
//! - A refusal with the permission card, answers the Python raises on,
//!   and no answer.
//! - Help never acts: each verb that changes something with `-h` or
//!   `--help` sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::os::unix::fs::PermissionsExt;

const VERB: &str = "hostmail";

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn exit1(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

fn help_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 0, 0)
}

fn traceback(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    compare(
        Compare::Traceback,
        case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1),
    )
}

fn piped(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

#[test]
fn hostmail_admin_verbs_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "mwords2");
    let v = vec![
        help_case("domain alone: the help", &["domain"]),
        help_case("status -h", &["status", "-h"]),
        help_case("config --help", &["config", "--help"]),
        usage_case("status, a stray word", &["status", "now"]),
        usage_case("domain, an unknown sub-command", &["domain", "rm", "a"]),
        usage_case("domain add, no domain", &["domain", "add"]),
        usage_case("domain show, two", &["domain", "show", "a", "b"]),
        usage_case("domain list, a domain", &["domain", "list", "acme.example"]),
        usage_case("domain check, --yes", &["domain", "check", "a", "--yes"]),
        usage_case("domain remove, an unknown flag", &["domain", "remove", "a", "--force"]),
        usage_case("create, nothing", &["create", "--person"]),
        usage_case("create, two", &["create", "a", "b"]),
        usage_case("create, --id with no value", &["create", "a", "--id"]),
        usage_case("create, an unknown flag", &["create", "a", "--name", "x"]),
        usage_case("delete, nothing", &["delete"]),
        usage_case("delete, two", &["delete", "a@x", "b@x"]),
        usage_case("doctor, two", &["doctor", "a", "b"]),
        usage_case("doctor, a flag", &["doctor", "--fix"]),
        usage_case("config, an unknown argument", &["config", "show"]),
        usage_case("config, a send mode it doesn't know", &["config", "--send-mode", "smtp"]),
        usage_case("config, an agent-send it doesn't know", &["config", "--agent-send", "maybe"]),
        usage_case("config, a relay port out of range", &["config", "--relay-port", "70000"]),
        usage_case("config, a relay port zero", &["config", "--relay-port", "0"]),
        usage_case("config, a relay port not a number", &["config", "--relay-port", "smtp"]),
        usage_case("config, an address cap not a number", &["config", "--address-cap", "-1"]),
        usage_case("config, bytes and gb", &["config", "--quota-bytes", "1", "--quota-gb", "1"]),
        usage_case("config, gb not a number", &["config", "--quota-gb", "1.5"]),
        usage_case("config, always-bcc with no workspace", &["config", "--always-bcc", "a@x"]),
        usage_case("config, copies a day not a number", &["config", "--forward-copies-per-day", "x"]),
        usage_case(
            "config, copies a day and an hour",
            &["config", "--forward-copies-per-day", "5", "--forward-copies-per-hour", "1"],
        ),
        usage_case(
            "config, new per day with a setting",
            &["config", "--forward-new-per-day", "5", "--domain", "a"],
        ),
        usage_case("config, a value missing", &["config", "--relay-host"]),
        piped(b"", usage_case("config, no relay password piped", &["config", "--relay-pass-stdin"])),
    ];
    for c in &v {
        rig.check(c);
    }
}

const RUNNING: &str = r#"{"ok":true,"consistent":true,"supported":true,"state":"running","version":"0.12.1","pinnedVersion":"0.12.1","upgradeAvailable":true,"upgrade":{"state":"rollback_failed","step":"start","error":"boom"},"hostname":"mail.acme.example","portPlan":"direct","enableProgress":{"current":"done"},"lastError":"none","helper":"installed","helperFix":"sudo x","cert":{"names":["mail.acme.example","autoconfig.acme.example"],"state":"issued","expiresAt":"2027-01-01","acme":{"mode":"Automatic","orderNames":["mail.acme.example"],"lastTask":{"state":"Failed","at":"10:00","reason":"dns"}},"ports":[{"port":443,"state":"ok"},{"port":993,"state":"stale"},"x"],"note":"one note","owner":"k2"},"outbound":{"ipStrategy":"v4Only","daneSummary":"optional","pendingReload":{"error":"slow"}},"certRenewal":{"mailHost":{"k2Issued":true,"state":"ok","failures":2,"lastError":"rate"}},"extraNames":[{"name":"autoconfig.acme.example","pointsHere":true,"cert":{"state":"issued"}},{"name":"mta-sts.acme.example","pointsHere":true},{"pointsHere":false}],"calendar":{"enabled":true,"files":false,"reachable":true,"url":"https://mail.acme.example/dav","reason":"ok"},"health":{"smtp":"up"}}"#;

#[test]
fn hostmail_admin_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "manswer2");
    let v = vec![
        ok_case("status, running", &["status"], RUNNING),
        ok_case("status --json --health", &["status", "--json", "--health"], RUNNING),
        exit1("status, not installed", &["status"], r#"{"ok":true,"supported":true,"state":"not-installed","helper":"missing","helperFix":"sudo install"}"#),
        exit1("status, not supported", &["status"], r#"{"ok":true,"supported":false}"#),
        exit1(
            "status, systemd disagrees",
            &["status"],
            r#"{"ok":true,"consistent":false,"supported":true,"state":"running","lastError":"inactive","cert":{"host":"mail.acme.example","selfSigned":true,"owner":"unknown"}}"#,
        ),
        exit1(
            "status, an older daemon's envelope",
            &["status", "--json"],
            r#"{"ok":false,"state":"stopped"}"#,
        ),
        exit1("status, not a status", &["status"], r#"{"ok":false}"#),
        case(
            "status, an error",
            &["status"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        ok_case(
            "domain add",
            &["domain", "add", "acme.example"],
            r#"{"ok":true,"domain":"acme.example","status":"pending","dnsCommands":["k2 dns add acme.example MX mail"]}"#,
        ),
        ok_case("domain add --json", &["domain", "add", "acme.example", "--json"], r#"{"ok":true}"#),
        ok_case(
            "domain list",
            &["domain", "list"],
            r#"{"ok":true,"domains":[{"domain":"acme.example","status":"active","sendMode":"direct","records":{"valid":5,"missing":1},"dmarcNag":true},{"domain":"globex-example.com"}]}"#,
        ),
        ok_case("domain list, none", &["domain", "list"], r#"{"ok":true,"domains":[]}"#),
        ok_case("domain show", &["domain", "show", "acme.example"], r#"{"ok":true,"domain":"acme.example","status":"active"}"#),
        ok_case("domain check --json", &["domain", "check", "acme.example", "--json"], r#"{"ok":true}"#),
        ok_case(
            "domain remove",
            &["domain", "remove", "acme.example", "--yes", "--purge-data"],
            r#"{"ok":true,"domain":"acme.example","retiredAddresses":3,"stalwartDeleted":true,"purge":true}"#,
        ),
        case(
            "domain remove, confirm first",
            &["domain", "remove", "acme.example"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"confirm_required","hint":"3 addresses; repeat with confirm: true"}}"#.to_string())],
            owner_and_port,
            1,
            1,
        ),
        // covers: hostmail create
        ok_case(
            "create",
            &["create", "bot", "--domain", "acme.example", "--id", "c1", "--person"],
            r#"{"ok":true,"address":"bot@acme.example","cap":{"used":2,"cap":5},"note":"fresh","password":"pw","imap":{"host":"mail.acme.example","port":993},"submission":{"alpn":true}}"#,
        ),
        ok_case(
            "create, existing",
            &["create", "bot@acme.example"],
            r#"{"ok":true,"existing":true,"address":"bot@acme.example"}"#,
        ),
        ok_case(
            "create, password withheld",
            &["create", "bot"],
            r#"{"ok":true,"passwordWithheld":true,"username":"bot"}"#,
        ),
        // covers: hostmail create --json
        ok_case("create --json", &["create", "bot", "--json"], r#"{"ok":true}"#),
        // covers: hostmail delete
        ok_case("delete", &["delete", "bot@acme.example"], r#"{"ok":true,"address":"bot@acme.example"}"#),
        // covers: hostmail delete --json
        ok_case("delete --json", &["delete", "bot@acme.example", "--json"], r#"{"ok":true}"#),
        ok_case("doctor", &["doctor"], r#"{"ok":true,"checks":[]}"#),
        ok_case("doctor, a domain", &["doctor", "acme.example", "--json"], r#"{"ok":true}"#),
        ok_case("config", &["config"], r#"{"ok":true,"sendMode":"direct"}"#),
        ok_case(
            "config set",
            &["config", "--send-mode", "relay", "--relay-host", "smtp.globex.example", "--relay-port", "587", "--quota-gb", "2", "--quota-messages", "100", "--address-cap", "0"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "config set, a workspace's always-bcc",
            &["config", "--workspace", "alpha", "--always-bcc", "a@x, b@x", "--quota-bytes", "10", "--json"],
            r#"{"ok":true,"hint":"set"}"#,
        ),
        ok_case(
            "config, new forwards a day",
            &["config", "--forward-new-per-day", "20"],
            r#"{"ok":true,"perDay":20,"default":20,"max":1000}"#,
        ),
        ok_case(
            "config, copies a day",
            &["config", "--forward-copies-per-day", "500"],
            r#"{"ok":true,"perDay":500,"default":500,"max":24000,"note":"live"}"#,
        ),
        ok_case(
            "config, copies an hour --json",
            &["config", "--forward-copies-per-hour", "50", "--json"],
            r#"{"ok":true}"#,
        ),
        traceback("domain list, a row not a dict", &["domain", "list"], r#"{"ok":true,"domains":["x"]}"#),
        traceback("domain add, a command not text", &["domain", "add", "a"], r#"{"ok":true,"dnsCommands":[1]}"#),
        compare(
            Compare::Traceback,
            exit1("status, names a number", &["status"], r#"{"ok":true,"supported":true,"state":"running","cert":{"names":5}}"#),
        ),
        compare(
            Compare::Traceback,
            exit1("status, an unhashable port state", &["status"], r#"{"ok":true,"supported":true,"state":"running","cert":{"ports":[{"state":[1]}]}}"#),
        ),
        traceback("create, an endpoint not a dict", &["create", "bot"], r#"{"ok":true,"jmap":[1]}"#),
        case("delete, no answer", &["delete", "a@x"], &[], vec![Reply::Close], owner_and_port, 1, 1),
        case(
            "doctor, the daemon down",
            &["doctor"],
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
    let rig = Rig::new(VERB, "mhelp2");
    const LINES: &[&[&str]] = &[
        &["domain", "add", "acme.example", "-h"],
        &["domain", "remove", "acme.example", "--yes", "--help"],
        &["domain", "check", "acme.example", "-h"],
        &["create", "bot", "--help"],
        &["delete", "bot@acme.example", "-h"],
        &["doctor", "--help"],
        &["config", "--send-mode", "relay", "-h"],
        &["config", "--relay-pass-stdin", "--help"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case("help never acts", line, &[], vec![], owner_and_port, 0, 0),
        ));
    }
}

/// The password the piped cases send: it must reach the daemon in the
/// request's body, and no child's argv or environment.
const SECRET: &str = "CHILDTEST_PASSWORD_7Kq";

const RELAY: &[&str] = &[
    "config",
    "--send-mode",
    "relay",
    "--relay-user",
    "postmaster",
    "--relay-pass-stdin",
];

fn piped_cases() -> Vec<Case> {
    vec![
        piped(
            b"CHILDTEST_PASSWORD_7Kq\n",
            ok_case("config, the relay password piped", RELAY, r#"{"ok":true}"#),
        ),
        piped(
            b"CHILDTEST_PASSWORD_7Kq",
            ok_case("config, no newline at the end", RELAY, r#"{"ok":true}"#),
        ),
        piped(
            b"CHILDTEST_PASSWORD_7Kq\nmore\n\n",
            ok_case("config, two lines", RELAY, r#"{"ok":true}"#),
        ),
    ]
}

#[test]
fn a_piped_relay_password_is_read_as_the_shell_reads_it() {
    let rig = Rig::new(VERB, "mpiped2");
    for c in &piped_cases() {
        rig.check(c);
    }
}

/// A folder of `python3` and `curl` that log their argv and environment
/// to `log`, then run the real program.
fn logging_programs(rig: &Rig, log: &std::path::Path) -> std::path::PathBuf {
    let dir = rig.dir.join("logging-bin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = std::env::var_os("PATH").expect("a PATH to run the tests with");
    for prog in ["python3", "curl"] {
        let real = std::env::split_paths(&path)
            .map(|d| d.join(prog))
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("no {prog} on PATH"));
        let script = format!(
            "#!/bin/sh\n{{ printf '== {prog}'; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; printf '\\n'; env; }} >> '{log}'\nexec '{real}' \"$@\"\n",
            log = log.display(),
            real = real.display(),
        );
        let file = dir.join(prog);
        std::fs::write(&file, script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// Each side's run of a piped case with `python3` and `curl` logging
/// what they were started with: the password reaches the daemon in the
/// request and no child's argv or environment. The shell starts its
/// Python (so the logging is seen to work); the binary starts nothing.
#[test]
fn a_piped_relay_password_is_on_no_child_s_argv_or_environment() {
    let mut rig = Rig::new(VERB, "mchildren2");
    let log = rig.dir.join("children.log");
    rig.path_first = Some(logging_programs(&rig, &log));
    rig.tool_log = Some(log);
    for c in &piped_cases() {
        rig.prepare(Side::Bash, c);
        let out = rig.run(true, c);
        assert_eq!(out.code, Some(0), "{}: the shell: {out:?}", c.name);
        let sent = rig.read(Side::Bash, false, c.name);
        let children = rig.take_tool_log();
        assert!(
            children.contains("== python3 "),
            "{}: the shell's Python was not logged:\n{children}",
            c.name
        );
        assert!(
            !children.contains(SECRET),
            "{}: the password is on a child of the shell:\n{children}",
            c.name
        );
        assert_eq!(sent.len(), 1, "{}: the shell's requests", c.name);
        assert!(
            String::from_utf8_lossy(&sent[0].bytes).contains(SECRET),
            "{}: the shell did not send the password",
            c.name
        );

        let (out, sent) = rig.binary(c, false);
        let children = rig.take_tool_log();
        assert_eq!(out.code, Some(0), "{}: the binary: {out:?}", c.name);
        assert_eq!(children, "", "{}: the binary started a program", c.name);
        assert_eq!(sent.len(), 1, "{}: the binary's requests", c.name);
        assert!(
            sent[0].contains(SECRET),
            "{}: the binary did not send the password",
            c.name
        );
    }
}
