//! `k5-cli domain` against the shell CLI's `cli/k2 domain`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - `list` (text and `--json`): rows with their names, certificates,
//!   renewals and Apps; pending nameservers, read-only and missing zones;
//!   none; odd values; Python's raise part way.
//! - `add`, `remove` (and `rm`, `delete`, `del`), `refresh` (and `check`),
//!   `name add` and `name remove`: what each prints, the daemon's errors
//!   and exit codes, `"ok": false`, a permission card.
//! - The words: help for `domain` and `domain name` alone or with an empty
//!   verb, unknown verbs, flags, extra words, `--role` with no value, a
//!   blank apex for refresh (nothing sent but `{}`).
//! - The owner from disk, a passport over TCP, the cell socket for the
//!   routes it serves (list, add, the names) and TCP for the ones it
//!   doesn't (remove, refresh); the gates.
//! - No answer, and a POST on the cell socket that may have run.
//! - One large list (above 256 KiB), as for `cert`.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "domain";
const LIST: &str = r#"{"ok":true,"domains":[{"apex":"acme.example","dnsWrite":true,"zoneId":"z-1","names":[{"hostname":"www.acme.example","role":"publish","cert":{"state":"valid"},"renewal":{"k2Issued":true,"state":"scheduled"},"app":{"primary":true,"served":"elsewhere"}},"x",{"hostname":"crm.acme.example","role":"app:crm","app":{"error":"unknown_app","app":"crm"}}]},{"apex":"globex.example","status":"pending_ns","nameservers":["ns1.example",null,"ns2.example"],"created":true},{"apex":"initech.example","readOnly":true,"status":"suspended","zoneMissing":true,"names":[]},{"apex":"byo.example"},42]}"#;
const ADDED: &str = r#"{"ok":true,"domain":{"apex":"globex.example","status":"pending_ns","nameservers":[],"created":false}}"#;

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
fn domain_list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let v = vec![
        // covers: domain list
        case("list", &["list"], &[], vec![ok(LIST)], owner_and_port, 0, 1),
        // covers: domain list --json
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
            vec![ok(r#"{"ok":true,"domains":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd values",
            &["list"],
            &[],
            vec![ok(
                r#"{"domains":[{"apex":["a"],"dnsWrite":"","zoneId":0,"status":"pending_ns","nameservers":"ab","names":{"k":1}},{"readOnly":1,"status":null}]}"#,
            )],
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
                r#"{"error":{"code":"owner_only","hint":"owner"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "names that can't be walked, part way",
            &["list"],
            &[],
            vec![ok(r#"{"domains":[{"apex":"a"},{"apex":"b","names":3}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "nameservers that can't be walked",
            &["list"],
            &[],
            vec![ok(
                r#"{"domains":[{"apex":"a","status":"pending_ns","nameservers":true}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a name's cert that is not a dict",
            &["list"],
            &[],
            vec![ok(r#"{"domains":[{"apex":"a","names":[{"cert":[1]}]}]}"#)],
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
fn domain_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "writes");
    let v = vec![
        // covers: domain add
        case(
            "add",
            &["add", "globex.example"],
            &[],
            vec![ok(ADDED)],
            owner_and_port,
            0,
            1,
        ),
        // covers: domain add --json
        case(
            "add: writable, auto-added",
            &["add", "acme.example", "--json"],
            &[],
            vec![ok(
                r#"{"ok":true,"domain":{"apex":"acme.example","dnsWrite":true}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add: no domain in the answer",
            &["add", "a"],
            &[],
            vec![ok(r#"{"ok":true,"domain":"a"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add: a bad apex, exit 2",
            &["add", "a b"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"invalid_domain","hint":"not a domain: a b"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        // covers: domain remove
        case(
            "remove",
            &["remove", "acme.example"],
            &[],
            vec![ok(r#"{"ok":true,"removed":"acme.example"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: domain
        case(
            "rm",
            &["rm", "acme.example"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: domain --json
        case(
            "delete --json",
            &["delete", "a", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"x":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "del",
            &["del", "a"],
            &[],
            vec![ok(r#"{"removed":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: domain refresh
        case(
            "refresh: rows, not checked",
            &["refresh"],
            &[],
            vec![ok(
                r#"{"ok":true,"domains":[{"apex":"globex.example","status":"pending_ns","nameservers":["ns1.example"]},{"apex":"acme.example","dnsWrite":1}],"checked":false}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refresh: none, and its hint",
            &["check", "globex.example"],
            &[],
            vec![ok(
                r#"{"ok":true,"domains":[],"checked":false,"hint":"tunnel é"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refresh: a blank apex sends {}",
            &["refresh", "  "],
            &[],
            vec![ok(r#"{"domains":"ab"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: domain refresh --json
        case(
            "refresh --json",
            &["refresh", "acme.example", "--json"],
            &[],
            vec![ok(
                r#"{"ok":true,"domains":[{"apex":"acme.example"}],"checked":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refresh: an apex with spaces",
            &["refresh", " a.example "],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: domain name add
        case(
            "name add: the role and external",
            &[
                "name",
                "add",
                "--role",
                " app:crm ",
                "crm.acme.example",
                "--external",
            ],
            &[],
            vec![ok(r#"{"ok":true,"name":{"hostname":"crm.acme.example"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: domain name add --json
        case(
            "name add: a role that looks like a flag",
            &["name", "add", "--role", "--json", "h"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "name add: an invalid role, exit 2",
            &["name", "add", "h", "--role", "x"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"invalid_role","hint":"x"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        // covers: domain name remove
        case(
            "name remove",
            &["name", "remove", "h"],
            &[],
            vec![ok(r#"{"ok":true,"hostname":"h"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: domain name remove --json
        case(
            "name del --json",
            &["name", "del", "h", "--json"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal with a permission card",
            &["remove", "a"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "ok false",
            &["add", "a"],
            &[],
            vec![ok(r#"{"ok":false}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset: no response",
            &["remove", "a"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        waited(case(
            "an empty reply: the line",
            &["add", "a"],
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
fn domain_words_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        case(
            "domain alone: its help",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
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
            "name alone: its help",
            &["name"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "name, an empty verb: its help",
            &["name", ""],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "an unknown verb",
            &["hostmail"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown name verb",
            &["name", "list"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "list: a word",
            &["list", "a"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("add: no apex", &["add"], &[], vec![], owner_and_port, 2, 0),
        case(
            "add: two",
            &["add", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: a flag",
            &["add", "-f"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: domain remove --json
        case(
            "remove: no apex",
            &["remove", "", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "remove: two",
            &["rm", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "refresh: two",
            &["refresh", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "refresh: a flag",
            &["check", "--all"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "name add: no hostname",
            &["name", "add", "--external"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "name add: --role with no value",
            &["name", "add", "h", "--role"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "name add: a flag",
            &["name", "add", "h", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "name add: two",
            &["name", "add", "h", "i"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "name remove: none",
            &["name", "rm"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "name remove: two",
            &["name", "delete", "h", "i"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn domain_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a host session: TCP",
                &["list"],
                &[],
                vec![ok(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
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
            cell(),
            case(
                "the cell socket: add",
                &["add", "a"],
                &[],
                vec![ok(ADDED)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket: name add",
                &["name", "add", "h"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket: name remove",
                &["name", "rm", "h"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "remove is TCP's",
                &["remove", "a"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "refresh is TCP's",
                &["refresh"],
                &[],
                vec![ok(r#"{"ok":true,"domains":[]}"#)],
                port_only,
                0,
                1,
            ),
        ),
        waited(with(
            cell(),
            case(
                "add with no answer there: not sent again",
                &["add", "a"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["list"],
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

/// The large list: above 256 KiB, thousands of rows.
#[test]
fn domain_list_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"apex":"d{i:05}.example","dnsWrite":true,"zoneId":"zone-{i:05}","names":[{{"hostname":"www.d{i:05}.example","role":"publish","cert":{{"state":"valid"}}}}]}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"ok":true,"domains":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    let mut want = format!("{:<28}  {:<8}  ZONE\n", "APEX", "DNS");
    for i in 0..3000 {
        want.push_str(&format!(
            "{:<28}  {:<8}  zone-{i:05}\n  {:<26}  {:<8}  cert valid\n",
            format!("d{i:05}.example"),
            "write",
            format!("www.d{i:05}.example"),
            "publish"
        ));
    }
    let c = case(
        "a large list",
        &["list"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!((out.code, out.stderr.as_str()), (Some(0), ""), "the binary");
    assert!(
        out.stdout == want,
        "the binary's stdout differs ({} bytes, want {})",
        out.stdout.len(),
        want.len()
    );
    assert_eq!(log.len(), 1, "the binary's requests");
    rig.prepare(Side::Bash, &c);
    let bash = rig.run(true, &c);
    assert_eq!(
        rig.read(Side::Bash, false, c.name).len(),
        1,
        "bash's requests"
    );
    if bash.code == Some(0) {
        assert!(
            bash.stdout == want,
            "bash's stdout differs from the binary's"
        );
    } else {
        assert_eq!(bash.code, Some(126), "bash: {}", bash.stderr);
        assert!(
            bash.stderr.contains("Argument list too long"),
            "bash: {}",
            bash.stderr
        );
    }
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on a line that
/// would change something prints help and sends nothing. The binary hands
/// such a line to the shell, which checks `_wants_help` first.
#[test]
fn domain_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell("add --help", &["add", "acme.example", "--help"]),
        shell("remove -h", &["remove", "-h", "acme.example"]),
        shell("rm --help", &["rm", "acme.example", "--json", "--help"]),
        shell("refresh -h", &["refresh", "-h"]),
        shell("check --help", &["check", "acme.example", "--help"]),
        shell("name add -h", &["name", "add", "h", "--role", "mail", "-h"]),
        shell(
            "name add, -h as the role",
            &["name", "add", "h", "--role", "-h"],
        ),
        shell("name remove --help", &["name", "remove", "h", "--help"]),
        shell("name del -h", &["name", "del", "-h", "h"]),
    ] {
        rig.check(&c);
    }
}
