//! `k5-cli hostmail bans`, `allowlist`, `group` and `calendar` against
//! the shell CLI's `cli/k2 hostmail …`, on the parity rig
//! ([`common::parity`]): the same requests (urllib's, from `_mail_py`),
//! stdout, stderr and exit code.
//!
//! - The words: each group's help with no words, each usage line, exit 2
//!   and nothing sent.
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
fn hostmail_access_verbs_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "awords");
    let v = vec![
        help_case("bans alone: the help", &["bans"]),
        help_case("bans migrate alone: the help", &["bans", "migrate"]),
        help_case(
            "bans migrate status -h",
            &["bans", "migrate", "status", "-h"],
        ),
        help_case("allowlist alone: the help", &["allowlist"]),
        help_case("group alone: the help", &["group"]),
        help_case("group list --help", &["group", "list", "--help"]),
        help_case("calendar alone: the help", &["calendar"]),
        help_case("calendar status -h", &["calendar", "status", "-h"]),
        help_case("calendar shares --help", &["calendar", "shares", "--help"]),
        usage_case("bans, an unknown sub-command", &["bans", "show"]),
        usage_case("bans list, a stray word", &["bans", "list", "x"]),
        usage_case("bans list, a flag", &["bans", "list", "--all"]),
        usage_case("bans clear, no ip", &["bans", "clear", "--all"]),
        usage_case(
            "bans clear, two",
            &["bans", "clear", "192.0.2.1", "192.0.2.2"],
        ),
        usage_case("bans migrate, no hours", &["bans", "migrate", "--json"]),
        usage_case(
            "bans migrate, hours not a number",
            &["bans", "migrate", "--hours", "2h"],
        ),
        usage_case(
            "bans migrate, zero hours",
            &["bans", "migrate", "--hours", "0"],
        ),
        usage_case(
            "bans migrate, --hours with no value",
            &["bans", "migrate", "--hours"],
        ),
        usage_case("bans migrate, a stray word", &["bans", "migrate", "now"]),
        usage_case(
            "bans migrate restore, a stray word",
            &["bans", "migrate", "restore", "x"],
        ),
        usage_case(
            "bans migrate status, a flag",
            &["bans", "migrate", "status", "--x"],
        ),
        usage_case("allowlist, an unknown sub-command", &["allowlist", "show"]),
        usage_case("allowlist list, a stray word", &["allowlist", "list", "x"]),
        usage_case("allowlist add, nothing", &["allowlist", "add"]),
        usage_case(
            "allowlist add, --expires with no value",
            &["allowlist", "add", "1.2.3.4", "--expires"],
        ),
        usage_case("allowlist add, two", &["allowlist", "add", "a", "b"]),
        usage_case(
            "allowlist remove, a flag",
            &["allowlist", "remove", "a", "--all"],
        ),
        usage_case("group, an unknown sub-command", &["group", "rename"]),
        usage_case("group create, no name", &["group", "create"]),
        usage_case("group create, two names", &["group", "create", "a", "b"]),
        usage_case("group add, --mail", &["group", "add", "g", "a@x", "--mail"]),
        usage_case("group list, --domain", &["group", "list", "--domain", "x"]),
        usage_case(
            "group create, --domain with no value",
            &["group", "create", "g", "--domain"],
        ),
        usage_case("group add, no member", &["group", "add", "g"]),
        usage_case("group list, a word", &["group", "list", "x"]),
        usage_case("group show, nothing", &["group", "show"]),
        usage_case(
            "group delete, an unknown flag",
            &["group", "delete", "g", "-f"],
        ),
        usage_case("calendar, an unknown sub-command", &["calendar", "list"]),
        usage_case(
            "calendar status, --files",
            &["calendar", "status", "--files", "on"],
        ),
        usage_case(
            "calendar enable, --files maybe",
            &["calendar", "enable", "--files", "maybe"],
        ),
        usage_case(
            "calendar enable, --files with no value",
            &["calendar", "enable", "--files"],
        ),
        usage_case(
            "calendar disable, a stray word",
            &["calendar", "disable", "x"],
        ),
        usage_case("calendar create, one word", &["calendar", "create", "a@x"]),
        usage_case(
            "calendar create, a flag it doesn't take",
            &["calendar", "create", "a@x", "Team", "--with", "b"],
        ),
        usage_case(
            "calendar rename, two words",
            &["calendar", "rename", "a@x", "c1"],
        ),
        usage_case(
            "calendar share, no --with",
            &["calendar", "share", "a@x", "--read"],
        ),
        usage_case(
            "calendar share, no level",
            &["calendar", "share", "a@x", "--with", "b@x"],
        ),
        usage_case(
            "calendar share, two levels",
            &["calendar", "share", "a@x", "--read", "--write"],
        ),
        usage_case(
            "calendar share, --with with no value",
            &["calendar", "share", "a@x", "--with"],
        ),
        usage_case(
            "calendar unshare, a level",
            &["calendar", "unshare", "a@x", "--read"],
        ),
        usage_case("calendar shares, none", &["calendar", "shares"]),
        usage_case("calendar sharing, a word", &["calendar", "sharing", "x"]),
        usage_case(
            "calendar sharing, max not a number",
            &["calendar", "sharing", "--max-shares", "lots"],
        ),
        usage_case(
            "calendar share, a short flag",
            &["calendar", "share", "a@x", "-r"],
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

const BANS: &str = r#"{"ok":true,"count":4,"counts":{"portScanning":2,"authFailure":1,"abuse":1},"bans":[{"address":"192.0.2.1","id":"b1","reason":"authFailure","expiresAt":null},{"address":"192.0.2.2","reason":"portScanning"},{"reason":"portScanning"},{"reason":"abuse"}]}"#;

const SHARES: &str = r#"{"ok":true,"owner":"a@acme.example","maxShares":20,"calendars":[{"id":"c1","name":"Team","isDefault":true,"shares":[{"with":"b@acme.example","withType":"person","level":"read"}]},{"id":"c2"}]}"#;

#[test]
fn hostmail_access_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "aanswer");
    let v = vec![
        // covers: hostmail bans list
        ok_case("bans list", &["bans", "list"], BANS),
        ok_case(
            "bans list, none",
            &["bans", "list"],
            r#"{"ok":true,"bans":[]}"#,
        ),
        ok_case(
            "bans list, scanners only, no count",
            &["bans", "list"],
            r#"{"ok":true,"bans":[{"reason":"portScanning"}]}"#,
        ),
        // covers: hostmail bans list --json
        ok_case("bans list --json", &["bans", "list", "--json"], BANS),
        // covers: hostmail bans clear
        ok_case(
            "bans clear",
            &["bans", "clear", "192.0.2.1", "--all"],
            r#"{"ok":true,"ip":"192.0.2.1","destroyed":["b1","b2"],"all":true,"warnings":["counter kept"]}"#,
        ),
        ok_case(
            "bans clear, none",
            &["bans", "clear", "192.0.2.1"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail bans clear --json
        ok_case(
            "bans clear --json",
            &["bans", "clear", "192.0.2.1", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail bans migrate
        ok_case(
            "bans migrate",
            &["bans", "migrate", "--hours", "6"],
            r#"{"ok":true,"active":true,"restoreAt":"12:00","authBanRateSnapshot":{"rate":"5/1d"}}"#,
        ),
        ok_case(
            "bans migrate, not active",
            &["bans", "migrate", "--hours", "6"],
            r#"{"ok":true,"active":false,"remainingSeconds":0}"#,
        ),
        // covers: hostmail bans migrate --json
        ok_case(
            "bans migrate --json",
            &["bans", "migrate", "--json", "--hours", "1"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail bans migrate restore
        ok_case(
            "bans migrate restore",
            &["bans", "migrate", "restore"],
            r#"{"ok":true,"restored":true,"authBanRate":"5/1d"}"#,
        ),
        // covers: hostmail bans migrate restore --json
        ok_case(
            "bans migrate restore --json",
            &["bans", "migrate", "restore", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail bans migrate status
        ok_case(
            "bans migrate status",
            &["bans", "migrate", "status"],
            r#"{"ok":true,"active":true,"remainingSeconds":60,"restoreAt":"12:00","overdue":false}"#,
        ),
        // covers: hostmail bans migrate status --json
        ok_case(
            "bans migrate status --json",
            &["bans", "migrate", "status", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail allowlist list
        ok_case(
            "allowlist list",
            &["allowlist", "list"],
            r#"{"ok":true,"allowlist":[{"address":"192.0.2.0/24","reason":"migration","id":"a1","expiresAt":"2027-01-01T00:00:00Z"},{}]}"#,
        ),
        ok_case(
            "allowlist list, empty",
            &["allowlist", "list"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail allowlist list --json
        ok_case(
            "allowlist list --json",
            &["allowlist", "list", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail allowlist add
        ok_case(
            "allowlist add",
            &[
                "allowlist",
                "add",
                "192.0.2.0/24",
                "--reason",
                "migration",
                "--expires",
                "2027-01-01T00:00:00Z",
            ],
            r#"{"ok":true,"id":"a1","reason":"migration"}"#,
        ),
        // covers: hostmail allowlist add --json
        ok_case(
            "allowlist add --json",
            &["allowlist", "add", "192.0.2.9", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail allowlist remove
        ok_case(
            "allowlist remove",
            &["allowlist", "remove", "192.0.2.9"],
            r#"{"ok":true,"destroyed":["a1"]}"#,
        ),
        // covers: hostmail allowlist remove --json
        ok_case(
            "allowlist remove --json",
            &["allowlist", "remove", "192.0.2.9", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail group create
        ok_case(
            "group create",
            &["group", "create", "team", "--domain", "acme.example"],
            r#"{"ok":true,"address":"team@acme.example","receivesMail":false}"#,
        ),
        // covers: hostmail group create --json
        ok_case(
            "group create --json",
            &["group", "create", "team", "--mail", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail group add
        ok_case(
            "group add",
            &[
                "group",
                "add",
                "team@acme.example",
                "a@acme.example",
                "b@acme.example,c@acme.example",
            ],
            r#"{"ok":true,"members":["a@acme.example","b@acme.example"]}"#,
        ),
        // covers: hostmail group add --json
        ok_case(
            "group add --json",
            &["group", "add", "g", "a@x", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail group remove
        ok_case(
            "group remove",
            &["group", "remove", "g", "a@x"],
            r#"{"ok":true,"group":"g"}"#,
        ),
        // covers: hostmail group remove --json
        ok_case(
            "group remove --json",
            &["group", "remove", "g", "a@x", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail group list
        ok_case(
            "group list",
            &["group", "list"],
            r#"{"ok":true,"groups":[{"address":"team@acme.example","members":["a@acme.example"],"receivesMail":true},{"name":"ops"}]}"#,
        ),
        ok_case("group list, none", &["group", "list"], r#"{"ok":true}"#),
        // covers: hostmail group list --json
        ok_case(
            "group list --json",
            &["group", "list", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail group show
        ok_case(
            "group show",
            &["group", "show", "team"],
            r#"{"ok":true,"group":{"name":"team","members":["a@x"]}}"#,
        ),
        // covers: hostmail group show --json
        ok_case(
            "group show --json",
            &["group", "show", "team", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail group delete
        ok_case(
            "group delete",
            &["group", "delete", "team"],
            r#"{"ok":true,"sharesRemoved":2}"#,
        ),
        // covers: hostmail group delete --json
        ok_case(
            "group delete --json",
            &["group", "delete", "team", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar status
        ok_case(
            "calendar status",
            &["calendar", "status"],
            r#"{"ok":true,"enabled":true,"files":false,"policy":"applied","reachable":true,"url":"https://mail.acme.example/dav","reason":"ok","accounts":{"total":3,"updated":1,"skipped":["x"]}}"#,
        ),
        ok_case(
            "calendar status, the default",
            &["calendar", "status"],
            r#"{"ok":true,"policy":"default"}"#,
        ),
        // covers: hostmail calendar status --json
        ok_case(
            "calendar status --json",
            &["calendar", "status", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar enable
        ok_case(
            "calendar enable",
            &["calendar", "enable", "--files", "on"],
            r#"{"ok":true,"enabled":true,"policy":"applied","backfilledAt":5}"#,
        ),
        // covers: hostmail calendar enable --json
        ok_case(
            "calendar enable --json",
            &["calendar", "enable", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar disable
        ok_case(
            "calendar disable",
            &["calendar", "disable"],
            r#"{"ok":true,"policy":"custom"}"#,
        ),
        // covers: hostmail calendar disable --json
        ok_case(
            "calendar disable --json",
            &["calendar", "disable", "--files", "off", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar create
        ok_case(
            "calendar create",
            &[
                "calendar",
                "create",
                "a@acme.example",
                "Team",
                "--color",
                "#ff0000",
            ],
            r#"{"ok":true,"created":true,"owner":"a@acme.example","calendar":{"name":"Team","id":"c1"}}"#,
        ),
        // covers: hostmail calendar create --json
        ok_case(
            "calendar create --json",
            &["calendar", "create", "a@x", "T", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar rename
        ok_case(
            "calendar rename",
            &["calendar", "rename", "a@acme.example", "c1", "Crew"],
            r#"{"ok":true,"calendar":{"previousName":"Team","name":"Crew","id":"c1"}}"#,
        ),
        // covers: hostmail calendar rename --json
        ok_case(
            "calendar rename --json",
            &["calendar", "rename", "a@x", "c1", "N", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar share
        ok_case(
            "calendar share",
            &[
                "calendar",
                "share",
                "a@acme.example",
                "--calendar",
                "Team",
                "--with",
                "b@acme.example",
                "--write",
            ],
            r#"{"ok":true,"owner":"a@acme.example","with":"b@acme.example","level":"write","previousLevel":"read","maxShares":20,"calendar":{"name":"Team"}}"#,
        ),
        // covers: hostmail calendar share --json
        ok_case(
            "calendar share --json",
            &[
                "calendar",
                "share",
                "a@x",
                "--with",
                "g",
                "--freebusy",
                "--json",
            ],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar unshare
        ok_case(
            "calendar unshare",
            &[
                "calendar",
                "unshare",
                "a@acme.example",
                "--with",
                "b@acme.example",
            ],
            r#"{"ok":true,"removed":true,"calendar":{"name":"Team"}}"#,
        ),
        ok_case(
            "calendar unshare, nothing to do",
            &["calendar", "unshare", "a@x", "--with", "b"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar unshare --json
        ok_case(
            "calendar unshare --json",
            &["calendar", "unshare", "a@x", "--with", "b", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar shares
        ok_case(
            "calendar shares",
            &["calendar", "shares", "a@acme.example"],
            SHARES,
        ),
        ok_case(
            "calendar shares, none",
            &["calendar", "shares", "a@acme.example"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail calendar shares --json
        ok_case(
            "calendar shares --json",
            &["calendar", "shares", "a@x", "--json"],
            SHARES,
        ),
        // covers: hostmail calendar sharing
        ok_case(
            "calendar sharing",
            &["calendar", "sharing"],
            r#"{"ok":true,"maxShares":20,"allowDirectoryQueries":true,"reloaded":"live now"}"#,
        ),
        ok_case(
            "calendar sharing, set",
            &["calendar", "sharing", "--max-shares", "50"],
            r#"{"ok":true,"reloaded":true}"#,
        ),
        // covers: hostmail calendar sharing --json
        ok_case(
            "calendar sharing --json",
            &["calendar", "sharing", "--json"],
            r#"{"ok":true}"#,
        ),
        case(
            "bans clear, refused",
            &["bans", "clear", "192.0.2.1"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        traceback(
            "bans list, a ban not a dict",
            &["bans", "list"],
            r#"{"ok":true,"count":1,"bans":["x"]}"#,
        ),
        traceback(
            "bans list, counts a string",
            &["bans", "list"],
            r#"{"ok":true,"counts":"ab","bans":[{}]}"#,
        ),
        traceback(
            "allowlist remove, ids not strings",
            &["allowlist", "remove", "a"],
            r#"{"ok":true,"destroyed":[1]}"#,
        ),
        traceback(
            "group list, a group not a dict",
            &["group", "list"],
            r#"{"ok":true,"groups":["x"]}"#,
        ),
        traceback(
            "calendar status, skipped a number",
            &["calendar", "status"],
            r#"{"ok":true,"accounts":{"skipped":3}}"#,
        ),
        traceback(
            "calendar shares, a share not a dict",
            &["calendar", "shares", "a@x"],
            r#"{"ok":true,"calendars":[{"shares":["x"]}]}"#,
        ),
        case(
            "group list, no answer",
            &["group", "list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "allowlist list, the daemon down",
            &["allowlist", "list"],
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
    let rig = Rig::new(VERB, "ahelp");
    const LINES: &[&[&str]] = &[
        &["bans", "clear", "192.0.2.1", "-h"],
        &["bans", "migrate", "--hours", "6", "--help"],
        &["bans", "migrate", "restore", "-h"],
        &["allowlist", "add", "192.0.2.1", "--help"],
        &["allowlist", "remove", "192.0.2.1", "-h"],
        &["group", "create", "team", "-h"],
        &["group", "add", "g", "a@x", "--help"],
        &["group", "remove", "g", "a@x", "-h"],
        &["group", "delete", "g", "--help"],
        &["calendar", "enable", "-h"],
        &["calendar", "disable", "--files", "off", "--help"],
        &["calendar", "create", "a@x", "T", "-h"],
        &["calendar", "rename", "a@x", "c", "N", "--help"],
        &["calendar", "share", "a@x", "--with", "b", "--read", "-h"],
        &["calendar", "unshare", "a@x", "--with", "b", "--help"],
        &["calendar", "sharing", "--max-shares", "5", "-h"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case("help never acts", line, &[], vec![], owner_and_port, 0, 0),
        ));
    }
}
