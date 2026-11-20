//! `k5-cli app suspend|self|usage|members|remote-pass|account-deletion`
//! against the shell CLI's (`cli/src/app-door.sh`,
//! `cli/src/app-recording.sh`), on the parity rig ([`common::parity`]):
//! the same requests, stdout, stderr and exit code.
//!
//! - `suspend`: already and not stopped yet, a reason, `--json`; the usage
//!   line, `--reason` last (exit 1), an unknown flag.
//! - `self`: live since a time, not live, no deployment; `--json`, usage.
//! - `usage`: the totals and the verbs, `--hours`, `--json`; usage, bad
//!   hours.
//! - `members`: a room, `--app`, no room in a session (the session's own
//!   room) and outside one (usage), `--json`.
//! - `remote-pass`: live, expired and revoked passes, none, `--revoke`
//!   with and without an id, `--json`; usage.
//! - `account-deletion` (and `account-deletions`): the list, none,
//!   `complete`, `cancel`, `--json`; the page with no App, a request on
//!   the list, no request to act on, too many words, an unknown flag.
//! - Python's raise part way; a refusal; no daemon.
//! - Help never acts: each write verb with `-h` or `--help` anywhere.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn suspend_self_and_usage_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "susp");
    let usage = r#"{"app":"acme","hours":24,"totals":{"calls":12,"denied":2,"bytesIn":100,"bytesOut":2048},"quotas":{"rps":5,"burst":10,"sockets":2,"subscriptions":4},"live":{"connections":1,"subscriptions":0},"verbs":[{"calls":10,"verb":"room.list","route":"/v1/rooms","denied":0},{"calls":2,"verb":null,"route":"/v1/x","denied":2}]}"#;
    let v = vec![
        case(
            "suspend, already, not stopped yet",
            &["suspend", "acme", "--reason", "abuse é"],
            &[],
            vec![ok(
                r#"{"app":"acme","already":true,"passesRevoked":3,"stopped":{"hint":"still draining"},"hint":"resume with the owner"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "suspend, plain",
            &["suspend", "acme"],
            &[],
            vec![ok(r#"{"app":"acme","passesRevoked":0,"stopped":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "suspend --json",
            &["suspend", "--json", "acme"],
            &[],
            vec![ok(r#"{"app":"acme"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "suspend usage",
            &["suspend"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "suspend --reason last: set -e, exit 1",
            &["suspend", "acme", "--reason"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "suspend: an unknown flag",
            &["suspend", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "suspend: no app in the answer",
            &["suspend", "acme"],
            &[],
            vec![ok(r#"{"passesRevoked":1}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: app self
        case(
            "self, live",
            &["self", "acme"],
            &[],
            vec![ok(
                r#"{"app":"acme","workspace":"ws-a","health":"ok","liveSeed":"abc","deployment":"d-1","deployedAt":1700000000}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "self, not live",
            &["self", "a b"],
            &[],
            vec![ok(
                r#"{"app":"a","workspace":"w","health":"down","deployment":"d-1","deployedAt":0}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: app self --json
        case(
            "self, no deployment, --json",
            &["self", "acme", "--json"],
            &[],
            vec![ok(r#"{"app":"a"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case("self usage", &["self"], &[], vec![], owner_and_port, 2, 0),
        traceback(case(
            "self: a time that is not a number",
            &["self", "acme"],
            &[],
            vec![ok(
                r#"{"app":"a","workspace":"w","health":"h","deployment":"d","deployedAt":"soon"}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        // covers: app usage
        case(
            "usage",
            &["usage", "acme"],
            &[],
            vec![ok(usage)],
            owner_and_port,
            0,
            1,
        ),
        // covers: app usage --json
        case(
            "usage --hours",
            &["usage", "--hours", "24", "acme", "--json"],
            &[],
            vec![ok(usage)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "usage: bad hours",
            &["usage", "acme", "--hours", "1d"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("usage usage", &["usage"], &[], vec![], owner_and_port, 2, 0),
        traceback(case(
            "usage: no totals",
            &["usage", "acme"],
            &[],
            vec![ok(r#"{"app":"a"}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a refusal with its fix line",
            &["self", "acme"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"app_exists","hint":"x"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

const MEMBERS: &str = r#"{"workspace":"room-a","count":3,"namesHidden":true,"members":[{"username":"ann","displayName":"Ann é","kind":"guest","app":"acme","role":"editor","caps":["room:read","room:members"]},{"username":"bo","displayName":"bo","kind":"agent","app":"host"},{"username":null,"kind":"guest","app":"acme"}],"admins":"the owner"}"#;

#[test]
fn members_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "memb");
    let v = vec![
        // covers: app members
        case(
            "a room",
            &["members", "room-a"],
            &[],
            vec![ok(MEMBERS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a room and --app",
            &["members", "room a", "--app", "all"],
            &[],
            vec![ok(MEMBERS)],
            owner_and_port,
            0,
            1,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "no room in a session: its own",
                &["members", "--app", "acme"],
                &[],
                vec![ok(MEMBERS)],
                port_only,
                0,
                1,
            )
        },
        case(
            "no room outside a session: usage",
            &["members", "--app", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: app members --json
        case(
            "--json",
            &["members", "room-a", "--json"],
            &[],
            vec![ok(MEMBERS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--app last: set -e, exit 1",
            &["members", "room-a", "--app"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        traceback(case(
            "a count that is not a number",
            &["members", "room-a"],
            &[],
            vec![ok(r#"{"workspace":"r","count":"3"}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn remote_pass_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "rpass");
    let passes = r#"{"app":"acme","passes":[{"id":"rp_0123456789","prefix":"k2rp_ab","expiresAt":4102444800,"caps":["room:read"]},{"id":"rp_x","prefix":"k2rp_cd","expiresAt":1000000000.5},{"id":"rp_y","prefix":"k2rp_ef","expiresAt":4102444800,"revokedAt":1}]}"#;
    let v = vec![
        case(
            "the passes",
            &["remote-pass", "acme"],
            &[],
            vec![ok(passes)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "none",
            &["remote-pass", "acme"],
            &[],
            vec![ok(r#"{"app":"acme","passes":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["remote-pass", "acme", "--json"],
            &[],
            vec![ok(passes)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--revoke one",
            &["remote-pass", "acme", "--revoke", "rp_x"],
            &[],
            vec![ok(r#"{"app":"acme","revoked":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--revoke all, --json",
            &["remote-pass", "--revoke", "--json", "acme"],
            &[],
            vec![ok(r#"{"app":"acme","revoked":2}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--revoke takes the App word as the id: usage",
            &["remote-pass", "--revoke", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["remote-pass", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "an expiry that is not a number",
            &["remote-pass", "acme"],
            &[],
            vec![ok(
                r#"{"app":"a","passes":[{"id":"x","prefix":"p","expiresAt":"never"}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "--revoke: no count",
            &["remote-pass", "acme", "--revoke"],
            &[],
            vec![ok(r#"{"app":"a"}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn account_deletion_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "adel");
    let dead = dead_port();
    let list = r#"{"app":"acme","accountDeletion":{"mode":"request","graceDays":30},"pending":[{"id":"adr_1","username":"ann é","requestedAt":1700000000,"dueAt":1702592000,"scope":"account"},{"id":"adr_2","username":"bo","requestedAt":0,"dueAt":null}]}"#;
    let v = vec![
        // covers: app account-deletion
        case(
            "the list",
            &["account-deletion", "acme"],
            &[],
            vec![ok(list)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "none, by list",
            &["account-deletions", "list", "a b"],
            &[],
            vec![ok(r#"{"app":"a b","accountDeletion":{}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: app account-deletion --json
        case(
            "the list --json",
            &["account-deletion", "--json", "acme"],
            &[],
            vec![ok(list)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "complete",
            &["account-deletion", "complete", "acme", "ann"],
            &[],
            vec![ok(
                r#"{"username":"ann","scope":"account","done":["login","grants"],"kept":[{"item":"audit","why":"the law"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "cancel",
            &["account-deletion", "cancel", "acme", "adr_1"],
            &[],
            vec![ok(r#"{"requestId":"adr_1","username":"ann","restored":2}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "complete --json",
            &["account-deletion", "complete", "acme", "ann", "--json"],
            &[],
            vec![ok(r#"{"username":"ann"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no App: the page",
            &["account-deletion"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a request on the list",
            &["account-deletion", "acme", "ann"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no request to act on",
            &["account-deletion", "cancel", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "too many words",
            &["account-deletion", "complete", "a", "b", "c"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["account-deletion", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "a time that is not a number",
            &["account-deletion", "acme"],
            &[],
            vec![ok(
                r#"{"app":"a","pending":[{"id":"x","username":"u","requestedAt":"then","dueAt":1}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "complete: a kept item with no why",
            &["account-deletion", "complete", "acme", "ann"],
            &[],
            vec![ok(r#"{"username":"ann","kept":[{"item":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "no daemon",
            &["account-deletion", "acme"],
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: a write verb with `-h` or `--help` anywhere on the
/// line prints help and sends nothing (the router's `app)` arm, or the
/// verb's own loop, answers it; the binary hands it to the shell).
#[test]
fn door_help_never_acts() {
    let rig = Rig::new(VERB, "doorhelp");
    let rows: [&'static [&'static str]; 6] = [
        &["suspend", "acme", "--reason", "x", "-h"],
        &["suspend", "--help", "acme"],
        &["remote-pass", "acme", "--revoke", "-h"],
        &["remote-pass", "--help", "acme", "--revoke", "rp_1"],
        &["account-deletion", "complete", "acme", "ann", "-h"],
        &["account-deletion", "cancel", "--help", "acme", "adr_1"],
    ];
    for args in rows {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}

const AUDIT: &str = r#"{"app":"acme","totals":{"calls":3,"denied":1,"bytesIn":120,"bytesOut":4096},"order":"newest first","calls":[{"atMs":1800000000123,"result":200,"actor":"ann é","verb":"db.query","bytesIn":40,"bytesOut":2048,"latencyMs":3},{"atMs":1800000001000,"result":403,"code":"scope","actor":"bo","verb":"files.put","bytesIn":80,"bytesOut":0,"latencyMs":1}],"chain":[{"day":"2026-11-19","ok":true,"rows":40},{"day":"2026-11-20","ok":false,"brokenAt":12}]}"#;

#[test]
fn resume_and_audit_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "resaud");
    let one = |name: &'static str, args: &'static [&'static str], body: &str, code: i32| {
        case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
    };
    let stop = |name: &'static str, args: &'static [&'static str], code: i32| {
        case(name, args, &[], vec![], owner_and_port, code, 0)
    };
    let v = vec![
        one(
            "resume --start",
            &["resume", "acme", "--start"],
            r#"{"app":"acme","deployment":"dep-3"}"#,
            0,
        ),
        one(
            "resume: already",
            &["resume", "acme"],
            r#"{"app":"acme","already":"not suspended é"}"#,
            0,
        ),
        one(
            "resume: a note",
            &["resume", "acme"],
            r#"{"app":"acme","note":"start it yourself"}"#,
            0,
        ),
        one(
            "resume --json",
            &["resume", "acme", "--json"],
            r#"{"app":"acme"}"#,
            0,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "resume: never in a session",
                &["resume", "acme"],
                &[],
                vec![],
                port_only,
                3,
                0,
            )
        },
        stop("resume: no App", &["resume", "--start"], 2),
        stop("resume: an unknown flag", &["resume", "acme", "--now"], 2),
        traceback(one(
            "resume: no app in the answer",
            &["resume", "acme"],
            r#"{}"#,
            1,
        )),
        one("audit", &["audit", "acme"], AUDIT, 0),
        one(
            "audit, every filter, one call",
            &[
                "audit",
                "acme",
                "--since",
                "1h",
                "--verb",
                "db.query",
                "--denied",
                "--limit",
                "5",
                "--oldest-first",
            ],
            r#"{"app":"acme","order":"oldest first","calls":[{"atMs":0,"result":500}]}"#,
            0,
        ),
        one("audit --json", &["audit", "--json", "acme"], AUDIT, 0),
        one(
            "audit --verify: a broken chain",
            &["audit", "acme", "--verify"],
            AUDIT,
            1,
        ),
        one(
            "audit --verify --json: a whole chain",
            &["audit", "acme", "--verify", "--json", "--newest-first"],
            r#"{"app":"acme","chain":[{"day":"d","ok":true,"rows":1}]}"#,
            0,
        ),
        traceback(one(
            "audit --verify: a chain day that is a number",
            &["audit", "acme", "--verify", "--json"],
            r#"{"app":"acme","chain":[5]}"#,
            1,
        )),
        traceback(one(
            "audit: a call with no time",
            &["audit", "acme"],
            r#"{"app":"a","calls":[{}]}"#,
            1,
        )),
        stop("audit: no App", &["audit"], 2),
        stop(
            "audit: --limit given last",
            &["audit", "acme", "--limit"],
            1,
        ),
        stop("audit: an unknown flag", &["audit", "acme", "--x"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn resume_and_audit_help_never_acts() {
    let rig = Rig::new(VERB, "resaudh");
    for args in [
        &["resume", "acme", "-h"][..],
        &["resume", "--help", "acme"],
        &["audit", "acme", "-h"],
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
