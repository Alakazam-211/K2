//! `k5-cli hostmail list`, `spam`, `queue`, `acl`, `autoconfig` and
//! `audit-sieve` against the shell CLI's `cli/k2 hostmail …`, on the
//! parity rig ([`common::parity`]): the same requests (urllib's, from
//! `_mail_py`), stdout, stderr and exit code.
//!
//! - The words: each group's help with no words (`list` alone exits 2),
//!   each usage line, exit 2 and nothing sent.
//! - Each verb's answer, plain and `--json`.
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
fn hostmail_list_verbs_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "lwords");
    let v = vec![
        case(
            "list alone: the help, exit 2",
            &["list"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        help_case("list members alone: the help", &["list", "members"]),
        help_case("list create -h", &["list", "create", "-h"]),
        help_case("spam alone: the help", &["spam"]),
        help_case("spam quarantine alone: the help", &["spam", "quarantine"]),
        help_case("queue alone: the help", &["queue"]),
        help_case("acl alone: the help", &["acl"]),
        help_case("autoconfig alone: the help", &["autoconfig"]),
        help_case("audit-sieve --help", &["audit-sieve", "--help"]),
        usage_case("list, an unknown sub-command", &["list", "add"]),
        usage_case(
            "list create, no address",
            &["list", "create", "--member", "a@x"],
        ),
        usage_case(
            "list create, --member with no value",
            &["list", "create", "l@x", "--member"],
        ),
        usage_case(
            "list create, an unknown flag",
            &["list", "create", "l@x", "--owner", "a"],
        ),
        usage_case(
            "list members add, no member",
            &["list", "members", "add", "l@x"],
        ),
        usage_case(
            "list members remove, no address",
            &["list", "members", "remove", "--member", "a"],
        ),
        usage_case("list show, two", &["list", "show", "a", "b"]),
        usage_case("list delete, nothing", &["list", "delete"]),
        usage_case("spam, an unknown sub-command", &["spam", "train"]),
        usage_case("spam ham, no id", &["spam", "ham", "--address", "a@x"]),
        usage_case("spam spam, no address", &["spam", "spam", "m1"]),
        usage_case("spam allow, no sender", &["spam", "allow"]),
        usage_case("spam block, a flag", &["spam", "block", "x@y", "--all"]),
        usage_case(
            "spam quarantine, an unknown sub-command",
            &["spam", "quarantine", "show"],
        ),
        usage_case(
            "spam quarantine list, no address",
            &["spam", "quarantine", "list"],
        ),
        usage_case(
            "spam quarantine release, no address",
            &["spam", "quarantine", "release", "q1"],
        ),
        usage_case("queue, an unknown sub-command", &["queue", "flush"]),
        usage_case("queue list, a word", &["queue", "list", "x"]),
        usage_case("queue retry, no id", &["queue", "retry"]),
        usage_case("queue drop, two ids", &["queue", "drop", "a", "b"]),
        usage_case("acl, an unknown sub-command", &["acl", "list"]),
        usage_case("acl grant, no user", &["acl", "grant", "m@x", "--read"]),
        usage_case("acl grant, no mailbox", &["acl", "grant", "--user", "u@x"]),
        usage_case(
            "acl grant, --rights with no value",
            &["acl", "grant", "m@x", "--rights"],
        ),
        usage_case("acl revoke, no user", &["acl", "revoke", "m@x"]),
        usage_case("acl show, a flag", &["acl", "show", "m@x", "--all"]),
        usage_case(
            "autoconfig, an unknown sub-command",
            &["autoconfig", "plant"],
        ),
        usage_case("autoconfig show, no domain", &["autoconfig", "show"]),
        usage_case("autoconfig apply, two", &["autoconfig", "apply", "a", "b"]),
        usage_case("audit-sieve, a word", &["audit-sieve", "all"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

const AUDIT: &str = r#"{"ok":true,"installed":true,"summary":{"accountsScanned":5,"accountsSkipped":1,"k2Mailboxes":3,"lockedMailboxes":2,"unlockedMailboxes":1,"userScripts":2,"apiKeys":1,"errors":["one"]},"enotify":{"disabled":false},"accounts":[{"address":"a@acme.example","k2Minted":true,"sieveLocked":false,"apiKeyLocked":false,"apiKeys":1,"apiKeysError":"slow","k2RedirectsNoRow":[{"script":"k2-forward","active":true,"domain":"globex.example","external":true}],"scripts":[{"name":"vacation","active":false,"owner":"user","targets":[{"action":"redirect","domain":"globex.example","external":true},{"action":"notify","domain":"acme.example"}]},{"name":"empty"}]},{"name":"svc","error":"denied"}]}"#;

#[test]
fn hostmail_list_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "lanswer");
    let v = vec![
        ok_case(
            "list create",
            &[
                "list",
                "create",
                "team@acme.example",
                "--member",
                "a@acme.example",
                "--member",
                "b@acme.example",
            ],
            r#"{"ok":true,"members":["a@acme.example","b@acme.example"]}"#,
        ),
        ok_case(
            "list create --json",
            &["list", "create", "t@x", "--json"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "list show",
            &["list", "show", "team@acme.example"],
            r#"{"ok":true,"address":"team@acme.example","members":["a@acme.example"]}"#,
        ),
        ok_case(
            "list members, a show",
            &["list", "members", "team@acme.example"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "list members add",
            &["list", "members", "add", "t@x", "--member", "c@x,d@x"],
            r#"{"ok":true,"members":["c@x"]}"#,
        ),
        ok_case(
            "list members remove --json",
            &[
                "list", "members", "remove", "t@x", "--member", "c@x", "--json",
            ],
            r#"{"ok":true}"#,
        ),
        ok_case("list delete", &["list", "delete", "t@x"], r#"{"ok":true}"#),
        ok_case(
            "spam ham",
            &["spam", "ham", "m1", "--address", "a@x"],
            r#"{"ok":true,"class":"ham"}"#,
        ),
        ok_case(
            "spam spam --json",
            &["spam", "spam", "m1", "--address", "a@x", "--json"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "spam allow",
            &["spam", "allow", "x@globex.example"],
            r#"{"ok":true,"key":"globex.example"}"#,
        ),
        ok_case(
            "spam block",
            &["spam", "block", "x@globex.example"],
            r#"{"ok":true,"action":"blocked"}"#,
        ),
        ok_case(
            "spam quarantine list",
            &["spam", "quarantine", "list", "--address", "a@x"],
            r#"{"ok":true,"messages":[{"id":"q1","subject":"Win"},{}]}"#,
        ),
        ok_case(
            "spam quarantine list, none",
            &["spam", "quarantine", "list", "a@x"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "spam quarantine release",
            &["spam", "quarantine", "release", "q1", "--address", "a@x"],
            r#"{"ok":true,"folder":"INBOX"}"#,
        ),
        ok_case(
            "spam quarantine discard --json",
            &[
                "spam",
                "quarantine",
                "discard",
                "q1",
                "--address",
                "a@x",
                "--json",
            ],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "queue list",
            &["queue", "list"],
            r#"{"ok":true,"messages":[{"id":"o1","nextRetry":"10:00","returnPath":"a@x"},{}]}"#,
        ),
        ok_case(
            "queue list, empty",
            &["queue", "list"],
            r#"{"ok":true,"messages":[]}"#,
        ),
        ok_case(
            "queue list --json",
            &["queue", "list", "--json"],
            r#"{"ok":true}"#,
        ),
        ok_case("queue retry", &["queue", "retry", "o1"], r#"{"ok":true}"#),
        ok_case(
            "queue drop",
            &["queue", "drop", "o1"],
            r#"{"ok":true,"id":"o1"}"#,
        ),
        ok_case(
            "acl grant",
            &[
                "acl",
                "grant",
                "shared@acme.example",
                "--user",
                "a@acme.example",
                "--read",
                "--post",
                "--rights",
                "lrs",
            ],
            r#"{"ok":true,"rights":"lrsp"}"#,
        ),
        ok_case(
            "acl revoke",
            &["acl", "revoke", "m@x", "--user", "u@x"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "acl show",
            &["acl", "show", "m@x"],
            r#"{"ok":true,"shareWith":[{"user":"u@x","rights":"lr"}]}"#,
        ),
        ok_case(
            "acl show, none shared",
            &["acl", "show", "m@x"],
            r#"{"ok":true,"shareWith":[]}"#,
        ),
        ok_case(
            "acl show --json",
            &["acl", "show", "m@x", "--json"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "autoconfig show",
            &["autoconfig", "show", "acme.example"],
            r#"{"ok":true,"thunderbirdUrl":"https://autoconfig.acme.example","caldavUrl":"https://mail.acme.example/dav","records":[{"type":"SRV","name":"_imaps._tcp","display":"0 1 993 mail"},{"type":"CNAME","name":"autoconfig","value":"mail"}]}"#,
        ),
        ok_case(
            "autoconfig apply, planted",
            &["autoconfig", "apply", "acme.example"],
            r#"{"ok":true,"planted":true}"#,
        ),
        ok_case(
            "autoconfig apply, by hand",
            &["autoconfig", "apply", "acme.example"],
            r#"{"ok":true,"records":[{"type":"TXT","name":"x","value":"y"}]}"#,
        ),
        ok_case(
            "autoconfig show --json",
            &["autoconfig", "show", "a", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail audit-sieve
        ok_case("audit-sieve", &["audit-sieve"], AUDIT),
        ok_case(
            "audit-sieve, nothing found",
            &["audit-sieve"],
            r#"{"ok":true,"installed":true,"enotify":{"disabled":true}}"#,
        ),
        ok_case(
            "audit-sieve, not installed",
            &["audit-sieve"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "audit-sieve, enotify unknown",
            &["audit-sieve"],
            r#"{"ok":true,"installed":true,"enotify":{"error":"no api"},"accounts":[]}"#,
        ),
        // covers: hostmail audit-sieve --json
        ok_case("audit-sieve --json", &["audit-sieve", "--json"], AUDIT),
        case(
            "queue drop, refused",
            &["queue", "drop", "o1"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        traceback(
            "list show, members not strings",
            &["list", "show", "t@x"],
            r#"{"ok":true,"members":[1]}"#,
        ),
        traceback(
            "queue list, a row not a dict",
            &["queue", "list"],
            r#"{"ok":true,"messages":["x"]}"#,
        ),
        traceback(
            "autoconfig show, a record not a dict",
            &["autoconfig", "show", "a"],
            r#"{"ok":true,"records":[1]}"#,
        ),
        traceback(
            "audit-sieve, a target not a dict",
            &["audit-sieve"],
            r#"{"ok":true,"installed":true,"accounts":[{"scripts":[{"targets":["x"]}]}]}"#,
        ),
        traceback(
            "audit-sieve, the summary not a dict",
            &["audit-sieve"],
            r#"{"ok":true,"installed":true,"summary":"x"}"#,
        ),
        case(
            "queue retry, no answer",
            &["queue", "retry", "o1"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "audit-sieve, the daemon down",
            &["audit-sieve"],
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
    let rig = Rig::new(VERB, "lhelp");
    const LINES: &[&[&str]] = &[
        &["list", "create", "t@x", "--member", "a@x", "-h"],
        &["list", "members", "add", "t@x", "--member", "a@x", "--help"],
        &["list", "delete", "t@x", "-h"],
        &["spam", "ham", "m1", "--address", "a@x", "--help"],
        &["spam", "block", "x@y", "-h"],
        &[
            "spam",
            "quarantine",
            "release",
            "q1",
            "--address",
            "a@x",
            "-h",
        ],
        &["queue", "retry", "o1", "--help"],
        &["queue", "drop", "o1", "-h"],
        &["acl", "grant", "m@x", "--user", "u@x", "--read", "--help"],
        &["acl", "revoke", "m@x", "--user", "u@x", "-h"],
        &["autoconfig", "apply", "acme.example", "--help"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case("help never acts", line, &[], vec![], owner_and_port, 0, 0),
        ));
    }
}
