//! `k5-cli compute node packages ...` against the shell CLI's `cli/k2
//! compute node packages ...`, on the parity rig ([`common::parity`]):
//! the same requests, stdout, stderr and exit code.
//!
//! - The page (`packages`, `packages help`); a node's packages (and
//!   `--json`); `request` (a held request: exit 2); `requests` (in and
//!   outside a session); `approve` and `deny` (the owner outside a
//!   session; refused in one); `policy` (show and set, a held change).
//! - Usage lines (exit 2) and a value flag last (exit 1).
//! - One large list (thousands of requests, above 256 KiB).
//! - Help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "compute";

fn raises(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn in_session(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: port_only,
        ..c
    }
}

fn one(name: &'static str, args: &'static [&'static str], body: &'static str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

const SHOW: &str = r#"{"node":"mini","hostLine":"Ubuntu 24.04","packages":{"manager":"apt","policy":"list","approved":["jq","webkit2gtk"],"installed":[],"indexAgeSecs":200000,"openRequests":2},"owner":"globex","takesRequests":false,"requests":[{"batchId":"pkb_1","state":"pending","names":["jq"]}]}"#;

const REQUESTS: &str = r#"{"canDecide":true,"requests":[{"batchId":"pkb_0001","id":"pkr_1","state":"pending","node":"mini","host":"box","createdAt":1000000000,"names":["jq","git"],"origin":"own","askedBy":"agent-a","jobId":"0123456789abcdef","why":"needs jq","dry":{"installs":["jq","libjq1"],"size_mb":1.5,"refusal":"too big"},"note":"ok","decidedBy":"owner"},{"batchId":"pkb_0002","id":"pkr_2","state":"approved","node":"big","createdAt":"1000000000","origin":"fwd","server":"globex","dry":{"installs":["x"]}},{"batchId":"pkb_0003","id":"pkr_3","state":"pending","node":"n","createdAt":null,"origin":"sent","server":"acme"}]}"#;

#[test]
fn compute_node_packages() {
    let rig = Rig::new(VERB, "nodepkg");
    let v = vec![
        // covers: compute node packages
        words("the page", &["node", "packages"], 0),
        words("help", &["node", "pkg", "help"], 0),
        one("a node", &["node", "packages", "mini"], SHOW, 0),
        one(
            "a node with no packages",
            &["node", "pkg", "mini"],
            r#"{"node":"mini","note":"offline","takesRequests":true}"#,
            0,
        ),
        one(
            "a young index",
            &["node", "packages", "mini"],
            r#"{"node":"mini","packages":{"indexAgeSecs":true},"takesRequests":true}"#,
            0,
        ),
        // covers: compute node packages --json
        one("--json", &["node", "packages", "mini", "--json"], SHOW, 0),
        raises(one(
            "approved a number",
            &["node", "packages", "mini"],
            r#"{"node":"m","packages":{"approved":5}}"#,
            1,
        )),
        words("a node and a word", &["node", "packages", "mini", "x"], 2),
        words("a flag first", &["node", "packages", "--frob"], 2),
        // covers: compute node packages request
        one(
            "request, sent on",
            &[
                "node",
                "packages",
                "request",
                "mini, big,",
                "jq,git",
                " curl ",
                "--why",
                "builds",
                "--job",
                "abcd",
            ],
            r#"{"request":{"id":"pkb_9","summary":"jq git on mini","rows":[{"node":"mini","state":"pending","names":["jq","git"],"dry":{"installs":["jq"],"size_mb":2}},{"node":"big","state":"sent","dry":{"refusal":"not listed"}}]},"cant":["huge on mini"],"hint":"the owner decides"}"#,
            0,
        ),
        case(
            "request, held",
            &["node", "packages", "request", "mini", "jq", "--why", "w"],
            &[],
            vec![ok(r#"{"pkgHeld":true,"request":{"id":"pkb_9"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        // covers: compute node packages request --json
        case(
            "request --json, held",
            &[
                "node", "packages", "request", "mini", "jq", "--why", "w", "--json",
            ],
            &[],
            vec![ok(r#"{"pkgHeld": true,"request":null}"#)],
            owner_and_port,
            2,
            1,
        ),
        one(
            "request --json, not held",
            &[
                "node", "packages", "request", "mini", "jq", "--why", "w", "--json",
            ],
            r#"{"request":{"id":"r"}}"#,
            0,
        ),
        in_session(case(
            "request in a session",
            &["node", "packages", "request", "mini", "jq", "--why", "w"],
            &[],
            vec![ok(r#"{"request":{}}"#)],
            port_only,
            0,
            1,
        )),
        words(
            "request, no names",
            &["node", "packages", "request", "mini", "--why", "w"],
            2,
        ),
        words(
            "request, no why",
            &["node", "packages", "request", "mini", "jq"],
            2,
        ),
        words(
            "request, an unknown flag",
            &["node", "packages", "request", "mini", "jq", "--frob"],
            2,
        ),
        words(
            "request, --why last",
            &["node", "packages", "request", "mini", "jq", "--why"],
            1,
        ),
        // covers: compute node packages requests
        one(
            "requests",
            &[
                "node", "packages", "requests", "--node", "mini", "--state", "all", "--limit", "5",
            ],
            REQUESTS,
            0,
        ),
        one(
            "ls, none",
            &["node", "packages", "ls"],
            r#"{"requests":[]}"#,
            0,
        ),
        // covers: compute node packages requests --json
        one(
            "requests --json",
            &["node", "packages", "list", "--json"],
            REQUESTS,
            0,
        ),
        in_session(case(
            "requests in a session",
            &["node", "packages", "requests"],
            &[],
            vec![ok(REQUESTS)],
            port_only,
            0,
            1,
        )),
        raises(one(
            "requests, a bad createdAt",
            &["node", "packages", "requests"],
            r#"{"requests":[{"createdAt":"soon"}]}"#,
            1,
        )),
        raises(one(
            "requests, an origin that is a list",
            &["node", "packages", "requests"],
            r#"{"requests":[{"origin":[1]}]}"#,
            1,
        )),
        words(
            "requests, an unknown word",
            &["node", "packages", "requests", "x"],
            2,
        ),
        words(
            "requests, --node last",
            &["node", "packages", "requests", "--node"],
            1,
        ),
        // covers: compute node packages approve
        case(
            "approve",
            &[
                "node", "packages", "approve", "pkb_0001", "--add", "--note", "fine",
            ],
            &[],
            vec![
                ok(REQUESTS),
                ok(
                    r#"{"rows":[{"state":"approved","request":"pkr_1","node":"mini","note":"installing"},{"state":"approved"}]}"#,
                ),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "approve, no batch matches",
            &["node", "packages", "approve", "zzz", "--json"],
            &[],
            vec![ok(REQUESTS), ok(r#"{"rows":[]}"#)],
            owner_and_port,
            0,
            2,
        ),
        raises(case(
            "approve, a list with no batchId",
            &["node", "packages", "approve", "pkb"],
            &[],
            vec![ok(r#"{"requests":[{"id":"x"}]}"#), ok("{}")],
            owner_and_port,
            0,
            2,
        )),
        in_session(case(
            "approve in a session",
            &["node", "packages", "approve", "pkb_1"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )),
        words("approve, no request", &["node", "packages", "approve"], 2),
        words(
            "approve, two requests",
            &["node", "packages", "approve", "a", "b"],
            2,
        ),
        words(
            "approve, an unknown flag",
            &["node", "packages", "approve", "a", "--frob"],
            2,
        ),
        // covers: compute node packages deny
        case(
            "deny",
            &["node", "packages", "deny", "pkb_0001", "--why", "no"],
            &[],
            vec![
                ok(REQUESTS),
                ok(r#"{"rows":[{"state":"denied","request":"pkr_1","node":"mini"}]}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        words("deny --add", &["node", "packages", "deny", "a", "--add"], 2),
        words(
            "deny, --note last",
            &["node", "packages", "deny", "a", "--note"],
            1,
        ),
        // covers: compute node packages policy
        one(
            "policy, show",
            &["node", "packages", "policy", "mini", "globex"],
            SHOW,
            0,
        ),
        one(
            "policy, show own",
            &["node", "packages", "policy", "mini", "own"],
            r#"{"node":"mini","note":"offline"}"#,
            0,
        ),
        one(
            "policy, set",
            &[
                "node",
                "packages",
                "policy",
                "mini",
                "level=profiles",
                "profiles= a ,,b",
                "max_install_mb=0512",
                "level=official",
                "--reason",
                "tidy",
            ],
            r#"{"node":"mini","note":"set"}"#,
            0,
        ),
        case(
            "policy, set, held",
            &["node", "packages", "policy", "mini", "globex", "level=list"],
            &[],
            vec![ok(
                r#"{"held":true,"request":{"id":"r1","summary":"pkg.policy"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        words(
            "policy, a bad level",
            &["node", "packages", "policy", "mini", "level=all"],
            2,
        ),
        words(
            "policy, a bad size",
            &["node", "packages", "policy", "mini", "max_install_mb=1e3"],
            2,
        ),
        words(
            "policy, an unknown key",
            &["node", "packages", "policy", "mini", "color=red"],
            2,
        ),
        words("policy, no node", &["node", "packages", "policy"], 2),
        words(
            "policy, three words",
            &["node", "packages", "policy", "a", "b", "c"],
            2,
        ),
        words(
            "policy, --reason last",
            &["node", "packages", "policy", "mini", "--reason"],
            1,
        ),
        // covers: compute node packages policy --json
        one(
            "policy --json",
            &["node", "packages", "policy", "mini", "--json"],
            SHOW,
            0,
        ),
        one(
            "policy set --json",
            &["node", "packages", "policy", "mini", "level=list", "--json"],
            r#"{"node":"mini"}"#,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The large-list case: thousands of package requests, above 256 KiB.
#[test]
fn compute_node_packages_large_list() {
    let rig = Rig::new(VERB, "nodepkgbig");
    let rows: Vec<String> = (0..3000u64)
        .map(|i| {
            format!(
                r#"{{"batchId":"pkb_{i:08x}","id":"pkr_{i}","state":"pending","node":"node-{i}","createdAt":1000000000,"names":["libfoo{i}","libbar{i}"],"origin":"own","askedBy":"agent-{i}"}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"canDecide":true,"requests":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    let body: &'static str = Box::leak(body.into_boxed_str());
    rig.check(&one(
        "3000 requests",
        &["node", "packages", "requests"],
        body,
        0,
    ));
}

/// The lines name the command by the brand's name.
#[test]
fn the_node_packages_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "nodepkgbrand");
    for (args, want) in [
        (
            &["node", "packages", "mini", "x"][..],
            format!("usage: {cli} compute node packages <node> [--json]\n"),
        ),
        (
            &["node", "packages", "approve"],
            format!(
                "usage: {cli} compute node packages approve <request> [--add] [--note \"…\"]\n"
            ),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// Help never acts.
#[test]
fn compute_node_packages_help_never_acts() {
    let rig = Rig::new(VERB, "nodepkghelp");
    let lines: &[&[&str]] = &[
        &[
            "node", "packages", "request", "mini", "jq", "--why", "w", "-h",
        ],
        &["node", "packages", "approve", "pkb_1", "--help"],
        &["node", "packages", "deny", "-h", "pkb_1"],
        &["node", "pkg", "policy", "mini", "level=list", "--help"],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
