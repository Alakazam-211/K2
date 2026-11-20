//! `k5-cli hostmail quota`, `catchall` and `alias` against the shell CLI's
//! `cli/k2 hostmail …`, on the parity rig ([`common::parity`]): the same
//! requests (urllib's, from `_mail_py`), stdout, stderr and exit code.
//!
//! - The words: each sub-command's help with no words, each usage line,
//!   exit 2 and nothing sent.
//! - quota get and set (bytes, gb, messages; a limit not set), catchall
//!   set, unset and show, alias add, remove and list (none, some).
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

const QUOTA: &str = r#"{"ok":true,"address":"sales@acme.example","quotas":{"maxDiskQuota":1073741824,"maxEmails":0}}"#;

#[test]
fn hostmail_mailbox_verbs_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "mwords");
    let v = vec![
        help_case("quota alone: the help", &["quota"]),
        help_case("catchall alone: the help", &["catchall"]),
        help_case("alias alone: the help", &["alias"]),
        usage_case("quota, an unknown sub-command", &["quota", "show"]),
        usage_case("quota get, no address", &["quota", "get"]),
        usage_case("quota get, two addresses", &["quota", "get", "a@x", "b@x"]),
        usage_case("quota get, an unknown flag", &["quota", "get", "a@x", "--gb"]),
        usage_case("quota get, --address with no value", &["quota", "get", "--address"]),
        usage_case("quota set, no address", &["quota", "set", "--gb", "1"]),
        usage_case("quota set, nothing to set", &["quota", "set", "a@x"]),
        usage_case(
            "quota set, bytes and gb",
            &["quota", "set", "a@x", "--bytes", "1", "--gb", "1"],
        ),
        usage_case("quota set, gb not a number", &["quota", "set", "a@x", "--gb", "1.5"]),
        usage_case(
            "quota set, messages negative",
            &["quota", "set", "a@x", "--messages", "-1"],
        ),
        usage_case("quota set, --bytes with no value", &["quota", "set", "a@x", "--bytes"]),
        usage_case("quota set, an unknown flag", &["quota", "set", "a@x", "--tb", "1"]),
        usage_case("catchall, an unknown sub-command", &["catchall", "get"]),
        usage_case("catchall set, no domain", &["catchall", "set", "--address", "a@x"]),
        usage_case("catchall set, no address", &["catchall", "set", "acme.example"]),
        usage_case(
            "catchall set, --address with no value",
            &["catchall", "set", "acme.example", "--address"],
        ),
        usage_case("catchall unset, no domain", &["catchall", "unset"]),
        usage_case("catchall unset, --address", &["catchall", "unset", "x", "--address", "a"]),
        usage_case("catchall show, two domains", &["catchall", "show", "a", "b"]),
        usage_case("catchall show, --domain with no value", &["catchall", "show", "--domain"]),
        usage_case("alias, an unknown sub-command", &["alias", "rm"]),
        usage_case("alias add, no alias", &["alias", "add", "a@x"]),
        usage_case("alias add, three words", &["alias", "add", "a@x", "b@x", "c@x"]),
        usage_case("alias remove, nothing", &["alias", "remove"]),
        usage_case("alias remove, an unknown flag", &["alias", "remove", "a@x", "-x"]),
        usage_case("alias list, no mailbox", &["alias", "list", "--json"]),
        usage_case("alias list, two", &["alias", "list", "a@x", "b@x"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn hostmail_mailbox_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "manswer");
    let v = vec![
        // covers: hostmail quota get
        ok_case("quota get", &["quota", "get", "sales@acme.example"], QUOTA),
        // covers: hostmail quota get --json
        ok_case(
            "quota get --json",
            &["quota", "get", "--address", "sales@acme.example", "--json"],
            QUOTA,
        ),
        ok_case(
            "quota get, no limits known",
            &["quota", "get", "sales@acme.example"],
            r#"{"ok":true,"quotas":{"maxDiskQuota":null}}"#,
        ),
        // covers: hostmail quota set
        ok_case(
            "quota set, gb and messages",
            &["quota", "set", "sales@acme.example", "--gb", "007", "--messages", "5000"],
            QUOTA,
        ),
        // covers: hostmail quota set --json
        ok_case(
            "quota set, bytes --json",
            &["quota", "set", "--bytes", "0", "sales@acme.example", "--json"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "quota set, messages only",
            &["quota", "set", "sales@acme.example", "--messages", "10"],
            r#"{"ok":true,"quotas":{"maxEmails":10.5}}"#,
        ),
        // covers: hostmail catchall
        ok_case(
            "catchall set",
            &["catchall", "set", "acme.example", "--address", "sales@acme.example"],
            r#"{"ok":true,"domain":"acme.example","catchAllAddress":"sales@acme.example"}"#,
        ),
        // covers: hostmail catchall --json
        ok_case(
            "catchall unset --json",
            &["catchall", "unset", "--domain", "acme.example", "--json"],
            r#"{"ok":true,"domain":"acme.example","catchAllAddress":null}"#,
        ),
        ok_case(
            "catchall unset",
            &["catchall", "unset", "acme.example"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "catchall show",
            &["catchall", "show", "acme.example"],
            r#"{"ok":true,"catchAllAddress":""}"#,
        ),
        // covers: hostmail alias
        ok_case(
            "alias add",
            &["alias", "add", "sales@acme.example", "deals@acme.example"],
            r#"{"ok":true,"alias":"deals@acme.example","mailbox":"sales@acme.example"}"#,
        ),
        // covers: hostmail alias --json
        ok_case(
            "alias remove --json",
            &["alias", "remove", "--json", "sales@acme.example", "deals@acme.example"],
            r#"{"ok":true,"removed":"deals@acme.example"}"#,
        ),
        ok_case(
            "alias remove",
            &["alias", "remove", "sales@acme.example", "deals@acme.example"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "alias list",
            &["alias", "list", "sales@acme.example"],
            r#"{"ok":true,"mailbox":"sales@acme.example","aliases":["deals@acme.example","info@acme.example"]}"#,
        ),
        ok_case(
            "alias list, none",
            &["alias", "list", "sales@acme.example"],
            r#"{"ok":true,"aliases":[]}"#,
        ),
        case(
            "an agent without the toggle",
            &["alias", "add", "a@x", "b@x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        traceback("quotas that are a list", &["quota", "get", "a@x"], r#"{"ok":true,"quotas":[1]}"#),
        traceback("an answer that is a list", &["catchall", "show", "x"], "[1]"),
        traceback(
            "aliases that are numbers",
            &["alias", "list", "a@x"],
            r#"{"ok":true,"aliases":[1]}"#,
        ),
        case(
            "closed, not sent again",
            &["quota", "set", "a@x", "--gb", "1"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listening",
            &["catchall", "show", "x"],
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
    let rig = Rig::new(VERB, "mhelp");
    const LINES: &[&[&str]] = &[
        &["quota", "set", "a@x", "--gb", "1", "-h"],
        &["catchall", "set", "--help", "x", "--address", "a@x"],
        &["catchall", "unset", "x", "-h"],
        &["alias", "add", "a@x", "b@x", "--help"],
        &["alias", "remove", "-h", "a@x", "b@x"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case(
                "help, nothing sent",
                line,
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ));
    }
}
