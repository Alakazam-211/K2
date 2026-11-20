//! `k5-cli hostmail dkim`, `dmarc`, `ptr` and `profile` against the shell
//! CLI's `cli/k2 hostmail …`, on the parity rig ([`common::parity`]): the
//! same requests (urllib's, from `_mail_py`), stdout, stderr and exit
//! code.
//!
//! - The words: each group's help with no words, each usage line
//!   (profile's file checks too), exit 2 and nothing sent.
//! - Each verb's answer, plain and `--json`; profile to stdout and to a
//!   file.
//! - A refusal with the permission card, answers the Python raises on,
//!   and no answer.
//! - Help never acts: each verb that changes something with `-h` or
//!   `--help` sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "hostmail";

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
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

#[test]
fn hostmail_dns_verbs_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "dwords");
    let v = vec![
        help_case("dkim alone: the help", &["dkim"]),
        help_case("dmarc alone: the help", &["dmarc"]),
        help_case("ptr alone: the help", &["ptr"]),
        help_case("profile --help", &["profile", "--help"]),
        usage_case("dkim, an unknown sub-command", &["dkim", "list"]),
        usage_case("dkim show, no domain", &["dkim", "show"]),
        usage_case("dkim show, two", &["dkim", "show", "a", "b"]),
        usage_case(
            "dkim rotate, an unknown flag",
            &["dkim", "rotate", "a", "--now"],
        ),
        usage_case(
            "dkim rotate, --domain with no value",
            &["dkim", "rotate", "--domain"],
        ),
        usage_case(
            "dkim retire, no selector",
            &["dkim", "retire", "acme.example"],
        ),
        usage_case("dkim retire, three", &["dkim", "retire", "a", "s", "t"]),
        usage_case("dmarc, an unknown sub-command", &["dmarc", "set"]),
        usage_case("dmarc show, no domain", &["dmarc", "show", "--json"]),
        usage_case(
            "dmarc report-to, no address",
            &["dmarc", "report-to", "acme.example"],
        ),
        usage_case(
            "dmarc report-to, no domain",
            &["dmarc", "report-to", "--address", "a@x"],
        ),
        usage_case(
            "dmarc report-to, two domains",
            &["dmarc", "report-to", "a", "b"],
        ),
        usage_case("ptr, an unknown sub-command", &["ptr", "get"]),
        usage_case("ptr show, a stray word", &["ptr", "show", "x"]),
        usage_case("ptr show, a flag", &["ptr", "show", "--all"]),
        usage_case("ptr set, no hostname", &["ptr", "set"]),
        usage_case("ptr set, two", &["ptr", "set", "a", "b"]),
        usage_case("profile, nothing", &["profile"]),
        usage_case("profile, no --apple", &["profile", "a@x"]),
        usage_case(
            "profile, two addresses",
            &["profile", "a@x", "b@x", "--apple"],
        ),
        usage_case("profile, an unknown flag", &["profile", "a@x", "--android"]),
        usage_case(
            "profile, --out with no value",
            &["profile", "a@x", "--apple", "--out"],
        ),
        usage_case(
            "profile, --out empty",
            &["profile", "a@x", "--apple", "--out", ""],
        ),
        usage_case(
            "profile, --force with no --out",
            &["profile", "a@x", "--apple", "--force"],
        ),
        usage_case(
            "profile, --out a folder",
            &["profile", "a@x", "--apple", "--out", "/"],
        ),
        usage_case(
            "profile, --out a file that is there",
            &["profile", "a@x", "--apple", "--out", "/etc/passwd"],
        ),
        usage_case(
            "profile, --out in no folder",
            &[
                "profile",
                "a@x",
                "--apple",
                "--out",
                "/no-such-folder-k5/x.mobileconfig",
            ],
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

const DKIM: &str = r#"{"ok":true,"domain":"acme.example","signatures":[{"selector":"k2-2026","stage":"active","type":"ed25519","match":"ok"},{"selector":"old","expected":"v=DKIM1; p=abc","name":"old._domainkey.acme.example"}]}"#;

const PROFILE: &str = r#"{"ok":true,"profile":"<?xml version=\"1.0\"?>\n<plist>…</plist>\n","appPasswordId":"ap_9","description":"iPhone","address":"sales@acme.example","mailHost":"mail.acme.example","includes":["mail","calendar"],"notes":["one note"]}"#;

#[test]
fn hostmail_dns_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "danswer");
    let v = vec![
        // covers: hostmail dkim show
        ok_case("dkim show", &["dkim", "show", "acme.example"], DKIM),
        // covers: hostmail dkim show --json
        ok_case(
            "dkim show --json",
            &["dkim", "show", "--domain", "acme.example", "--json"],
            DKIM,
        ),
        // covers: hostmail dkim rotate
        ok_case(
            "dkim rotate",
            &["dkim", "rotate", "acme.example"],
            r#"{"ok":true,"planted":false,"records":[{"name":"k2-2027._domainkey","expectedDisplay":"v=DKIM1"},{"value":"v=DKIM1; p=x"}]}"#,
        ),
        // covers: hostmail dkim rotate --json
        ok_case(
            "dkim rotate --json",
            &["dkim", "rotate", "acme.example", "--json"],
            r#"{"ok":true,"planted":true}"#,
        ),
        // covers: hostmail dkim retire
        ok_case(
            "dkim retire",
            &["dkim", "retire", "acme.example", "old"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail dkim retire --json
        ok_case(
            "dkim retire --json",
            &[
                "dkim",
                "retire",
                "--selector",
                "old",
                "--domain",
                "acme.example",
                "--json",
            ],
            r#"{"ok":true,"selector":"old"}"#,
        ),
        // covers: hostmail dmarc show
        ok_case(
            "dmarc show",
            &["dmarc", "show", "acme.example"],
            r#"{"ok":true,"dmarc":{"expected":"v=DMARC1; p=none","match":"ok"},"reportAddressUri":"mailto:dmarc@acme.example","reportAddressMinted":true,"suggestedRua":"rua=mailto:dmarc@acme.example","note":"once a day"}"#,
        ),
        ok_case(
            "dmarc show, little known",
            &["dmarc", "show", "acme.example"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail dmarc show --json
        ok_case(
            "dmarc show --json",
            &["dmarc", "show", "acme.example", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail dmarc report-to
        ok_case(
            "dmarc report-to",
            &[
                "dmarc",
                "report-to",
                "acme.example",
                "--address",
                "dmarc@acme.example",
            ],
            r#"{"ok":true,"suggestedRua":"rua=mailto:dmarc@acme.example"}"#,
        ),
        // covers: hostmail dmarc report-to --json
        ok_case(
            "dmarc report-to --json",
            &[
                "dmarc",
                "report-to",
                "acme.example",
                "--address",
                "d@acme.example",
                "--json",
            ],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail ptr show
        ok_case(
            "ptr show",
            &["ptr", "show"],
            r#"{"ok":true,"originIpv4":"192.0.2.10","ptr":"mail.acme.example","hostname":"mail.acme.example","aRecords":["192.0.2.10","192.0.2.11"],"ptrMatchesHelo":true,"aMatchesOrigin":false,"relayCheck":{"unresolved":["relay1.example"]}}"#,
        ),
        ok_case("ptr show, little known", &["ptr", "show"], r#"{"ok":true}"#),
        // covers: hostmail ptr show --json
        ok_case(
            "ptr show --json",
            &["ptr", "show", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail ptr set
        ok_case(
            "ptr set, authoritative",
            &["ptr", "set", "mail.acme.example"],
            r#"{"ok":true,"helo":"mail.acme.example","originIpv4":"192.0.2.10","aligned":true,"ptrSource":"authoritative","ptr":"mail.acme.example","resolverNote":"cached","settings":{"warning":"reload later"},"defaultCertificate":{"state":"moved","certificateId":"c1"}}"#,
        ),
        ok_case(
            "ptr set, live",
            &["ptr", "set", "--hostname", "mail.acme.example"],
            r#"{"ok":true,"ptr":"x","defaultCertificate":{"warning":"old cert"}}"#,
        ),
        // covers: hostmail ptr set --json
        ok_case(
            "ptr set --json",
            &["ptr", "set", "h", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail profile
        ok_case(
            "profile to stdout",
            &["profile", "sales@acme.example", "--apple"],
            PROFILE,
        ),
        // covers: hostmail profile --json
        ok_case(
            "profile --json",
            &["profile", "--json", "sales@acme.example", "--apple"],
            PROFILE,
        ),
        Case {
            cwd: Some(rig.dir.clone()),
            ..ok_case(
                "profile to a file",
                &[
                    "profile",
                    "sales@acme.example",
                    "--apple",
                    "--out",
                    "sales.mobileconfig",
                    "--force",
                ],
                PROFILE,
            )
        },
        Case {
            cwd: Some(rig.dir.clone()),
            ..ok_case(
                "profile to a file --json",
                &[
                    "profile",
                    "sales@acme.example",
                    "--apple",
                    "--json",
                    "--out",
                    "sales.mobileconfig",
                    "--force",
                ],
                PROFILE,
            )
        },
        case(
            "profile, no profile back",
            &["profile", "sales@acme.example", "--apple"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "dkim rotate, refused",
            &["dkim", "rotate", "acme.example"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        traceback(
            "dkim show, a signature not a dict",
            &["dkim", "show", "a"],
            r#"{"ok":true,"signatures":["x"]}"#,
        ),
        traceback(
            "ptr show, A records not strings",
            &["ptr", "show"],
            r#"{"ok":true,"aRecords":[1]}"#,
        ),
        traceback(
            "ptr set, settings not a dict",
            &["ptr", "set", "h"],
            r#"{"ok":true,"settings":"x"}"#,
        ),
        traceback(
            "profile, the profile not text",
            &["profile", "a@x", "--apple"],
            r#"{"ok":true,"profile":5}"#,
        ),
        case(
            "dmarc report-to, no answer",
            &["dmarc", "report-to", "a", "--address", "b@a"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "ptr show, the daemon down",
            &["ptr", "show"],
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
    let rig = Rig::new(VERB, "dhelp");
    const LINES: &[&[&str]] = &[
        &["dkim", "rotate", "acme.example", "-h"],
        &["dkim", "retire", "acme.example", "old", "--help"],
        &["dmarc", "report-to", "a", "--address", "b", "-h"],
        &["ptr", "set", "h", "--help"],
        &["profile", "a@x", "--apple", "-h"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case("help never acts", line, &[], vec![], owner_and_port, 0, 0),
        ));
    }
}
