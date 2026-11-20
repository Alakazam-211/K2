//! `k5-cli cert` against the shell CLI's `cli/k2 cert`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `list` (text and `--json`), `issue` and `renew`: the answers (rows,
//!   none, odd values, renewals, App states), the daemon's errors (a gate,
//!   usage, an error object, text, a list), `"ok": false`, a permission
//!   card's closing line, the bulletin, migrating, not JSON, not a dict,
//!   Python's raise part way.
//! - The words: help for `cert` alone and an empty verb, an unknown verb,
//!   flags and extra words (the usage line, exit 2, nothing sent), a
//!   hostname that needs escaping.
//! - The owner from disk, a passport over TCP, the cell socket (GET and
//!   POST), not served there, a heal, the id tool's checks, the gates.
//! - No answer: an empty reply and a 5xx with no body (the line, exit 1),
//!   a reset and not HTTP (the reader's "no response" line), nothing
//!   listening; a POST on the cell socket that may have run is not sent
//!   again.
//! - One large list (above 256 KiB): the binary prints every row. Linux's
//!   bash can't hand an answer that large to its python3 through the
//!   environment (exit 126); where it can (macOS), the two are the same.
//! - What the binary hands to the shell: a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "cert";
const LIST: &str = r#"{"ok":true,"certs":[{"hostname":"www.acme.example","role":"publish","cert":{"state":"valid","notAfter":"2027-01-01"},"renewal":{"k2Issued":true,"state":"scheduled"},"app":{"primary":true}},{"hostname":"mail.acme.example","role":"mail","cert":null},{"hostname":"crm.acme.example","role":"app:crm","cert":{"state":"issuing"},"renewal":{"k2Issued":true,"state":"failing","failures":3,"nextAttempt":"2026-11-21T00:00:00Z"},"app":{"notServedYet":true,"state":"waiting_cert","served":"elsewhere","warning":"port 443 é"}},"odd",{"hostname":"x.globex.example","app":{"external":1,"primary":0}},{"app":{"error":"unknown_app","app":"wiki"}}]}"#;
const ISSUED: &str = r#"{"ok":true,"hostname":"www.acme.example","state":"issuing"}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

#[test]
fn cert_list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let v = vec![
        // covers: cert list
        case("list", &["list"], &[], vec![ok(LIST)], owner_and_port, 0, 1),
        // covers: cert list --json
        case(
            "list --json",
            &["list", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "none",
            &["list"],
            &[],
            vec![ok(r#"{"ok":true,"certs":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no list at all",
            &["list"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd values",
            &["list"],
            &[],
            vec![ok(
                r#"{"certs":[{"hostname":5,"role":["a"],"cert":{"state":1.5},"renewal":{"k2Issued":"y","failures":0}},{"hostname":"","cert":{}},null,true,{"app":{"served":"elsewhere"}}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a string of certs: one row a letter",
            &["list"],
            &[],
            vec![ok(r#"{"certs":"ab"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the gate: exit 3",
            &["list"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"dns_manage_disabled","hint":"Agents can't manage DNS é"}}"#
                    .into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a usage code: exit 2",
            &["list"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"invalid_domain","hint":"bad"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error object with no hint",
            &["list"],
            &[],
            vec![Reply::Answer(500, r#"{"error":{"code":"boom"}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error text",
            &["list"],
            &[],
            vec![ok(r#"{"error":"nope"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error list",
            &["list"],
            &[],
            vec![ok(r#"{"error":[1,null]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "ok false",
            &["list", "--json"],
            &[],
            vec![ok(r#"{"ok":false,"why":"x"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: the closing line",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the bulletin line when it worked",
            &["list"],
            &[],
            vec![ok(
                r#"{"ok":true,"certs":[],"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            &["list"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(LIST),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("<html>é")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty 200",
            &["list"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "not a dict",
            &["list"],
            &[],
            vec![ok("[1,2]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a code Python can't hash",
            &["list"],
            &[],
            vec![ok(r#"{"error":{"code":{"a":1},"hint":"h"}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a cert that is not a dict, part way",
            &["list"],
            &[],
            vec![ok(
                r#"{"certs":[{"hostname":"a"},{"hostname":"b","cert":"x"}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "certs that can't be walked",
            &["list"],
            &[],
            vec![ok(r#"{"certs":7}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "PROJECT_PATH",
            &["list"],
            &[("K2_PROJECT_PATH", "/srv/acme ws")],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn cert_words_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        case(
            "cert alone: its help",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // covers: cert
        case(
            "an empty verb: its help",
            &[""],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "an unknown verb",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "list: a flag",
            &["list", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "list: a word",
            &["list", "é \"q\""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "list: --",
            &["list", "--"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "issue: no hostname",
            &["issue"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: cert --json
        case(
            "issue: only --json",
            &["issue", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "issue: an empty one is not a hostname",
            &["issue", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "issue: a flag",
            &["issue", "a", "--force"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "issue: two hostnames",
            &["issue", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "renew: no hostname",
            &["renew"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // The gates and the id tool's checks come before the words.
        case("no port", &["nope"], &[], vec![], owner_only, 1, 0),
        case("no token", &[], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &["list"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["issue"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn cert_issue_and_renew_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "issue");
    let v = vec![
        case(
            "issue",
            &["issue", "www.acme.example"],
            &[],
            vec![ok(ISSUED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "issue --json, an empty word before the hostname",
            &["issue", "", "--json", "www.acme.example"],
            &[],
            vec![ok(ISSUED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a hostname that needs escaping",
            &["issue", "é \"q\"\\\t\n"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "only ok: printed as it is",
            &["renew", "a"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "renew: what it said",
            &["renew", "a"],
            &[],
            vec![ok(r#"{"ok":true,"renewal":{"state":"due"},"n":1e400}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the zone isn't attached: exit 3",
            &["issue", "a"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"dns_zone_not_attached","hint":"attach it"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not under the apex: exit 2",
            &["renew", "a"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"hostname_not_under_apex"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a refusal with a permission card",
            &["renew", "a", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 5xx with no body is no response",
            &["issue", "a"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset: no response",
            &["issue", "a"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP: no response",
            &["issue", "a"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        waited(case(
            "an empty reply: the line, exit 1",
            &["issue", "a"],
            &[],
            vec![Reply::Close],
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
fn cert_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            h(&[]),
            case(
                "a host session: the passport over TCP",
                &["list"],
                &[],
                vec![ok(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["issue", "a"],
                &[],
                vec![heal(&rig), ok(ISSUED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a dead passport: the probe, then the restart line",
                &["list"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                1,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["list"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: list",
                &["list"],
                &[],
                vec![ok(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: issue",
                &["issue", "a"],
                &[],
                vec![ok(ISSUED)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "its trailing newlines go",
                &["list"],
                &[],
                vec![ok("notjson\n\n")],
                port_only,
                1,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "not served: TCP",
                &["renew", "a"],
                &[],
                vec![not_served(), ok(ISSUED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "a GET with no answer there: TCP",
                &["list"],
                &[],
                vec![Reply::Close, ok(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(&[]),
            case(
                "a POST with no answer there: not sent again",
                &["issue", "a"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        with(
            cell(&[]),
            case(
                "a refusal there is the answer",
                &["issue", "a"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            cell(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: TCP",
                &["issue", "a"],
                &[],
                vec![ok(ISSUED)],
                port_only,
                0,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn cert_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    let lists: [&'static [&'static str]; 2] = [&["list"], &["issue", "a"]];
    for args in lists {
        rig.check(&Case {
            args,
            ..case(
                "nothing listens: no response",
                &[],
                &[("K2_PORT", port.as_str())],
                vec![],
                owner_only,
                1,
                0,
            )
        });
    }
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn cert_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "a port that is no port",
            &["list"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            1,
            0,
        )
    });
}

/// The large list: above 256 KiB, thousands of rows.
#[test]
fn cert_list_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..4000)
        .map(|i| {
            format!(
                r#"{{"hostname":"h{i:05}.acme.example","role":"publish","cert":{{"state":"valid"}},"renewal":{{"k2Issued":true,"state":"scheduled"}}}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"ok":true,"certs":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    let mut want = format!("{:<32}  {:<8}  CERT\n", "HOSTNAME", "ROLE");
    for i in 0..4000 {
        want.push_str(&format!(
            "{:<32}  {:<8}  valid  renew scheduled\n",
            format!("h{i:05}.acme.example"),
            "publish"
        ));
    }
    large_list(&rig, &["list"], body, &want);
}

/// The binary prints `want` for `body`, with one request; bash does the
/// same where its python3 takes an answer that large through the
/// environment, and fails to start it (exit 126) where it can't.
fn large_list(rig: &Rig, args: &'static [&'static str], body: String, want: &str) {
    let c = case(
        "a large list",
        args,
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        (out.code, out.stderr.as_str()),
        (Some(0), ""),
        "the binary: {}",
        out.stderr
    );
    assert!(
        out.stdout == want,
        "the binary's stdout differs ({} bytes, want {})",
        out.stdout.len(),
        want.len()
    );
    assert_eq!(log.len(), 1, "the binary's requests");
    let (out3, log3) = rig.binary(&c, true);
    assert_eq!(
        (out3.code, &out3.stdout, &out3.stderr),
        (out.code, &out.stdout, &out.stderr),
        "with no python3"
    );
    assert_eq!(log3, log, "the requests with no python3");
    rig.prepare(Side::Bash, &c);
    let bash = rig.run(true, &c);
    let sent = rig.read(Side::Bash, false, c.name);
    assert_eq!(sent.len(), 1, "bash's requests");
    if bash.code == Some(0) {
        assert!(
            bash.stdout == want,
            "bash's stdout differs from the binary's"
        );
        assert_eq!(bash.stderr, "", "bash's stderr");
    } else {
        assert_eq!(bash.code, Some(126), "bash: {}", bash.stderr);
        assert!(
            bash.stderr.contains("Argument list too long"),
            "bash: {}",
            bash.stderr
        );
        assert_eq!(bash.stdout, "", "bash's stdout");
    }
}

/// No answer within 30 s: the line, exit 1. Slow.
#[test]
fn cert_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&waited(case(
        "no answer in time (28)",
        &["list"],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        1,
        1,
    )));
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on a line that
/// would change something prints help and sends nothing. The binary hands
/// such a line to the shell, which checks `_wants_help` first.
#[test]
fn cert_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell("issue, --help last", &["issue", "a.example", "--help"]),
        shell("issue, -h first", &["issue", "-h", "a.example"]),
        shell(
            "renew, -h with --json",
            &["renew", "a.example", "--json", "-h"],
        ),
        shell("renew, --help alone", &["renew", "--help"]),
    ] {
        rig.check(&c);
    }
}
