//! The held App requests of `k5-cli app` against the shell CLI's
//! (`cli/src/app-auth.sh`), on the parity rig ([`common::parity`]): the
//! same requests, stdout, stderr and exit code.
//!
//! - `approvals [--app A] [--all]`: the list (status, kind, why, held,
//!   decided), none; `--json`; an unknown argument; a value flag given
//!   last; Python's raise; a refusal; a large list.
//! - `approvals revoke <id> | --app <name>`: sent once; `--json`; usage.
//! - `deny <id> [--reason T]`: sent once; `--json`; usage.
//! - `approve <id>`: refused in a session; no single request (none,
//!   several, or the finder raised); not pending; pending, and no vault
//!   socket to confirm on (exit 3); usage.
//! - Help never acts, but where the shell takes `-h` as a value or an id.

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

fn read(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

const REFUSED: &str = r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#;

const LIST: &str = r#"{"requests":[{"id":"req-1","status":"pending","kind":"egress","appLabel":"acme","workspaceHandle":"ops","requestedBy":"ann","summary":"reach api.example:443 é","reason":"sync","reasons":["a new host","broad"]},{"id":"req-2","status":"approved","kind":"boxService","appLabel":"b","requestedBy":"bo","summary":"s","decidedBy":"owner","confirmVia":"touch id"},{"id":"req-3","status":"denied","kind":"k","appLabel":"c","requestedBy":"cy","summary":["x"],"decidedBy":"admin"}]}"#;

#[test]
fn approvals_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "apprs");
    let v = vec![
        read("approvals", &["approvals"], LIST),
        read("approvals: none", &["approvals", "--all", "--app", "acme é"], r#"{"requests":[]}"#),
        read("approvals --json", &["approvals", "--json"], LIST),
        stop("approvals: an unknown argument", &["approvals", "req-1"], 2),
        stop("approvals: --app given last", &["approvals", "--app"], 1),
        traceback(case(
            "approvals: a request with no id",
            &["approvals"],
            &[],
            vec![ok(r#"{"requests":[{"status":"pending"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "approvals: a refusal",
            &["approvals"],
            &[],
            vec![Reply::Answer(403, REFUSED.into())],
            owner_and_port,
            3,
            1,
        ),
        read(
            "approvals revoke",
            &["approvals", "revoke", "w-1"],
            r#"{"revoked":[{"id":"w-1","summary":"publish run é"}],"note":"it asks again next time"}"#,
        ),
        read(
            "approvals revoke --app --json",
            &["approvals", "revoke", "--app", "acme", "--json"],
            r#"{"revoked":[]}"#,
        ),
        read("approvals revoke: no note", &["approvals", "revoke", "w-1"], r#"{}"#),
        stop("approvals revoke: nothing named", &["approvals", "revoke"], 2),
        stop("approvals revoke: both", &["approvals", "revoke", "w-1", "--app", "a"], 2),
        stop("approvals revoke: two ids", &["approvals", "revoke", "a", "b"], 2),
        stop("approvals revoke: --app given last", &["approvals", "revoke", "--app"], 2),
        stop("approvals revoke: an unknown flag", &["approvals", "revoke", "--x"], 2),
        case(
            "approvals revoke: a refusal",
            &["approvals", "revoke", "w-1"],
            &[],
            vec![Reply::Answer(403, REFUSED.into())],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn deny_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "deny");
    let denied = r#"{"request":{"id":"req-1","status":"denied","summary":"reach x é"}}"#;
    let v = vec![
        read("deny", &["deny", "req-1"], denied),
        read("deny --reason, the last id", &["deny", "a", "req-1", "--reason", "no é"], denied),
        read("deny --json", &["deny", "req-1", "--json"], denied),
        stop("deny: no id", &["deny"], 2),
        stop("deny: an empty id", &["deny", ""], 2),
        stop("deny: an unknown flag", &["deny", "req-1", "--x"], 2),
        stop("deny: --reason given last", &["deny", "req-1", "--reason"], 1),
        traceback(read("deny: no request", &["deny", "req-1"], r#"{}"#)),
        case(
            "deny: a refusal",
            &["deny", "req-1"],
            &[],
            vec![Reply::Answer(403, REFUSED.into())],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn approve_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "approve");
    let v = vec![
        Case {
            env: with_passport(&[]),
            ..case(
                "approve: never in a session",
                &["approve", "req-1"],
                &[],
                vec![],
                port_only,
                3,
                0,
            )
        },
        case(
            "approve: no such request",
            &["approve", "req-9"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "approve: a prefix of several",
            &["approve", "req-"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "approve: not pending",
            &["approve", "req-2"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "approve: pending, no vault socket to confirm on",
            &["approve", "req-1", "extra"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "approve: the finder raises",
            &["approve", "req-1"],
            &[],
            vec![ok(r#"{"requests":[{"id":7}]}"#)],
            owner_and_port,
            3,
            1,
        )),
        case(
            "approve: the list refused",
            &["approve", "req-1"],
            &[],
            vec![Reply::Answer(403, REFUSED.into())],
            owner_and_port,
            3,
            1,
        ),
        stop("approve: no id", &["approve"], 2),
        stop("approve: an empty id", &["approve", ""], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One list above 256 KiB.
#[test]
fn approvals_a_large_list() {
    let rig = Rig::new(VERB, "apprlarge");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(
                r#"{{"id":"req-{n}","status":"pending","kind":"egress","appLabel":"app-{n}","workspaceHandle":"ws","requestedBy":"agent é","summary":"reach host-{n}.example:443 for its sync job","reasons":["a new host"]}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"requests":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&["approvals"][..], &["approvals", "--json"]] {
        rig.check(&case("a large list", args, &[], vec![ok(&body)], owner_and_port, 0, 1));
    }
}

#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "apprhelp");
    for (args, code) in [
        (&["approvals", "-h"][..], 0),
        (&["approvals", "revoke", "w-1", "--help"], 0),
        (&["deny", "req-1", "-h"], 0),
        (&["approve", "-h"], 2),
        (&["approve", "--help"], 2),
    ] {
        rig.check(&shell(stop("help never acts", args, code)));
    }
    // The shell takes `-h` as a value or an id and sends (bash bugs,
    // app-auth.sh:1646 approvals revoke --app, app-auth.sh:1670 deny
    // --reason, and app-auth.sh:1632-1635 approve, which looks at its first
    // word only; the rows flip when the fixes land).
    rig.check(&shell(read(
        "help as a value acts: approvals revoke --app -h (bash bug)",
        &["approvals", "revoke", "--app", "-h"],
        r#"{"revoked":[]}"#,
    )));
    rig.check(&shell(read(
        "help as a value acts: deny --reason -h (bash bug)",
        &["deny", "req-1", "--reason", "-h"],
        r#"{"request":{"id":"req-1","status":"denied","summary":"s"}}"#,
    )));
    rig.check(&shell(case(
        "help after the id acts: approve req-9 -h (bash bug)",
        &["approve", "req-9", "-h"],
        &[],
        vec![ok(LIST)],
        owner_and_port,
        3,
        1,
    )));
}
