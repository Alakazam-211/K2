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
//! - 0.46.17, held requests as permission cards: `approvals [<id>]
//!   [--state S]` and their usage lines; each request's card line (a
//!   card, an empty one, none while pending, no logins, logins that
//!   aren't all dicts); an agent's last line under its own requests (none
//!   with canApprove); Python's raise in the card line.
//! - `--assign` (`_app_take_assign`, the shared pre-scan): one login, a
//!   comma list, `--assign=`, before or after the id; on a POST only
//!   (`?assign=`), never on a GET; its usage line (no value, an empty
//!   one, `--assign=`) before anything is sent and before the verb's own
//!   loop; its value counts for `--json`; the verbs handed to `cmd_skin`
//!   and `cmd_skin_token` get the words without it; before the verb it
//!   is the shell's.
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
        read(
            "approvals: none",
            &["approvals", "--all", "--app", "acme é"],
            r#"{"requests":[]}"#,
        ),
        read("approvals --json", &["approvals", "--json"], LIST),
        stop("approvals: an unknown argument", &["approvals", "-x"], 2),
        stop("approvals: --app given last", &["approvals", "--app"], 2),
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
        read(
            "approvals revoke: no note",
            &["approvals", "revoke", "w-1"],
            r#"{}"#,
        ),
        stop(
            "approvals revoke: nothing named",
            &["approvals", "revoke"],
            2,
        ),
        stop(
            "approvals revoke: both",
            &["approvals", "revoke", "w-1", "--app", "a"],
            2,
        ),
        stop(
            "approvals revoke: two ids",
            &["approvals", "revoke", "a", "b"],
            2,
        ),
        stop(
            "approvals revoke: --app given last",
            &["approvals", "revoke", "--app"],
            2,
        ),
        stop(
            "approvals revoke: an unknown flag",
            &["approvals", "revoke", "--x"],
            2,
        ),
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
        read(
            "deny --reason, the last id",
            &["deny", "a", "req-1", "--reason", "no é"],
            denied,
        ),
        read("deny --json", &["deny", "req-1", "--json"], denied),
        stop("deny: no id", &["deny"], 2),
        stop("deny: an empty id", &["deny", ""], 2),
        stop("deny: an unknown flag", &["deny", "req-1", "--x"], 2),
        stop(
            "deny: --reason given last",
            &["deny", "req-1", "--reason"],
            2,
        ),
        traceback(case(
            "deny: no request",
            &["deny", "req-1"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            1,
            1,
        )),
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
        rig.check(&case(
            "a large list",
            args,
            &[],
            vec![ok(&body)],
            owner_and_port,
            0,
            1,
        ));
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

const CARDS: &str = r#"{"canApprove":false,"requests":[{"id":"w_abc","status":"pending","kind":"manage","appLabel":"app1","workspaceHandle":"a","requestedBy":"agent:a","summary":"connect é","reasons":[],"card":{"id":"w_abc","state":"pending","card":"c1","thread":"a","routedTo":[{"login":"ada","role":"admin"},"x",{"login":"bo é"}],"narrowed":true}},{"id":"w_e","status":"pending","kind":"k","appLabel":"b","requestedBy":"r","summary":"s","card":{}},{"id":"w_old","status":"denied","kind":"manage","appLabel":"app1","workspaceHandle":"a","requestedBy":"agent:a","summary":"older","reasons":[],"decidedBy":"owner","card":null},{"id":"w_n","status":"pending","kind":"k","appLabel":"c","requestedBy":"r","summary":"t","card":{"routedTo":[]}},{"id":"w_s","status":"pending","kind":"k","appLabel":"d","requestedBy":"r","summary":"u","card":"not a dict"}]}"#;

fn agent(c: Case) -> Case {
    Case {
        env: with_passport(&[]),
        setup: port_only,
        ..c
    }
}

/// 0.46.17: one request by its id, a state, and each request's card.
#[test]
fn approvals_name_each_card_as_the_shell_does() {
    let rig = Rig::new(VERB, "apprcards");
    let can = CARDS.replacen(r#""canApprove":false"#, r#""canApprove":true"#, 1);
    let v = vec![
        // covers: app approvals
        read("approvals <id>: the card lines", &["approvals", "w_abc"], CARDS),
        read(
            "approvals --state denied --app",
            &["approvals", "--state", "denied", "--app", "app1 é"],
            CARDS,
        ),
        read(
            "approvals: an empty --state, an empty id, then the id",
            &["approvals", "--state", "", "", "w_abc"],
            CARDS,
        ),
        read(
            "approvals --state denied --all: all",
            &["approvals", "--state", "denied", "--all"],
            CARDS,
        ),
        // covers: app approvals --json
        read("approvals <id> --json", &["approvals", "w_abc", "--json"], CARDS),
        stop("approvals: --state given last", &["approvals", "--state"], 2),
        stop(
            "approvals: a state that isn't one",
            &["approvals", "--state", "nonsense", "--all"],
            2,
        ),
        stop("approvals: two ids", &["approvals", "w_a", "w_b"], 2),
        agent(case(
            "approvals: an agent's own requests",
            &["approvals"],
            &[],
            vec![ok(CARDS)],
            port_only,
            0,
            1,
        )),
        agent(case(
            "approvals: an agent that may approve",
            &["approvals"],
            &[],
            vec![ok(&can)],
            port_only,
            0,
            1,
        )),
        agent(case(
            "approvals: an agent with none",
            &["approvals", "--state", "all"],
            &[],
            vec![ok(r#"{"requests":[],"canApprove":false}"#)],
            port_only,
            0,
            1,
        )),
        traceback(case(
            "approvals: a login that isn't a string",
            &["approvals"],
            &[],
            vec![ok(r#"{"requests":[{"id":"w","status":"pending","kind":"k","appLabel":"a","requestedBy":"r","summary":"s","card":{"id":"w","routedTo":[{"login":7}]}}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "approvals: routedTo that isn't a list",
            &["approvals"],
            &[],
            vec![ok(r#"{"requests":[{"id":"w","status":"pending","kind":"k","appLabel":"a","requestedBy":"r","summary":"s","card":{"id":"w","routedTo":5}}]}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// 0.46.17: `--assign` on any `app` command, taken before the verb reads
/// its words (`_app_take_assign`), and on its POSTs as `?assign=`.
#[test]
fn assign_rides_the_posts_as_the_shell_s_does() {
    let rig = Rig::new(VERB, "apprassign");
    let denied = r#"{"request":{"id":"req-1","status":"denied","summary":"reach x é"}}"#;
    let v = vec![
        read(
            "deny --assign one login",
            &["deny", "req-1", "--assign", "ada"],
            denied,
        ),
        read(
            "deny: --assign a comma list, before the id",
            &["deny", "--assign", "ada,owner é", "req-1", "--reason", "r"],
            denied,
        ),
        read("deny --assign=bo", &["deny", "req-1", "--assign=bo"], denied),
        read(
            "deny: the last --assign stands",
            &["deny", "--assign", "ada", "req-1", "--assign=bo"],
            denied,
        ),
        read(
            "approvals revoke --assign: the POST carries it",
            &["approvals", "revoke", "w-1", "--assign", "ada"],
            r#"{"revoked":[]}"#,
        ),
        read(
            "approvals --assign: a GET carries none",
            &["approvals", "--assign", "ada", "w_abc"],
            CARDS,
        ),
        stop("deny: --assign given last", &["deny", "req-1", "--assign"], 2),
        stop(
            "deny: --assign with an empty value",
            &["deny", "req-1", "--assign", ""],
            2,
        ),
        stop("deny: --assign= with none", &["deny", "req-1", "--assign="], 2),
        stop(
            "approvals: --assign's usage before the verb's own",
            &["approvals", "-x", "--assign"],
            2,
        ),
        case(
            "deny --assign --json: the value, and a refusal as JSON",
            &["deny", "req-1", "--assign", "--json"],
            &[],
            vec![Reply::Answer(403, REFUSED.into())],
            owner_and_port,
            3,
            1,
        ),
        // The verbs `cmd_app` hands to `cmd_skin` and `cmd_skin_token`.
        read(
            "grant --assign: cmd_skin's list",
            &["grant", "--assign", "ada"],
            r#"{"grants":[]}"#,
        ),
        read(
            "hydra off --assign: cmd_skin's POST, no /cli/app path",
            &["hydra", "off", "--assign", "ada"],
            r#"{"enabled":false}"#,
        ),
        read(
            "token list --assign: cmd_skin_token's",
            &["token", "--assign", "ada", "list"],
            r#"{"tokens":[]}"#,
        ),
        // `user chat`'s POST is /cli/app/guest/chat, through cmd_skin:
        // with --assign it is the shell's.
        shell(read(
            "user chat <scope> --assign: the shell's",
            &["user", "chat", "bob", "member", "--assign", "ada"],
            r#"{"username":"bob","scope":"member","enabled":true}"#,
        )),
        // The route reads the word after `app` as the verb: before it,
        // the line is the shell's.
        shell(read(
            "--assign before the verb: the shell's",
            &["--assign", "ada,owner", "deny", "req-1"],
            denied,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn assign_help_never_acts() {
    let rig = Rig::new(VERB, "apprassignhelp");
    rig.check(&shell(stop(
        "help never acts: deny --assign ada -h",
        &["deny", "req-1", "--assign", "ada", "-h"],
        0,
    )));
    // The shell's pre-scan takes `-h` as --assign's value and sends (bash
    // bug, app-wallet.sh _app_take_assign; the row flips when it's fixed).
    rig.check(&shell(read(
        "help as --assign's value acts: deny --assign -h (bash bug)",
        &["deny", "req-1", "--assign", "-h"],
        r#"{"request":{"id":"req-1","status":"denied","summary":"s"}}"#,
    )));
}

/// The binary's lines name the brand's command.
#[test]
fn the_held_card_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "apprbrand");
    let cli = k2_client::brand::CLI_NAME;
    let c = stop("brand: two ids", &["approvals", "a", "b"], 2);
    let (ran, _) = rig.binary(&c, false);
    assert_eq!(ran.stderr, format!("usage: {cli} app approvals [<request-id>] [--app A] [--state pending|approved|denied|withdrawn|expired|failed|all] [--all] [--json]\n"));
    assert_eq!(ran.code, Some(2));
    let c = agent(case(
        "brand: an agent's own requests",
        &["approvals"],
        &[],
        vec![ok(CARDS)],
        port_only,
        0,
        1,
    ));
    let (ran, _) = rig.binary(&c, false);
    assert!(
        ran.stdout.ends_with(&format!("you can withdraw one: {cli} app deny <request-id>.\n")),
        "{}",
        ran.stdout
    );
    assert_eq!(ran.code, Some(0));
    if cli != "k2" {
        for text in [&ran.stdout, &ran.stderr] {
            assert!(!text.contains("k2 ") && !text.contains("k5 "), "{text}");
        }
    }
}
