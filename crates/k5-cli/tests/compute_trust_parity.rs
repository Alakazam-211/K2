//! `k5-cli compute retry|roster|seats|trust|owners ...` against the shell
//! CLI's `cli/k2 compute ...`, on the parity rig ([`common::parity`]) and,
//! for a person's confirm, the vault rig ([`common::vault`]): the same
//! requests, stdout, stderr and exit code.
//!
//! - `retry`: its reader, the daemon's error, no answer, a raise.
//! - `roster`, `seats`, `trust` and `owners`: each list's reader (and
//!   `--json`), each POST's pretty answer, the usage lines (exit 2) and a
//!   value flag last (exit 1, nothing printed).
//! - The person's steps: `roster key`, `seats end|suspend|resume`, `trust
//!   approve` and `owners consent on` refuse a session (exit 3, nothing
//!   sent); outside one, the confirm on the vault socket and the token in
//!   the next request's body, never a child's argv or environment.
//! - One large list (thousands of trust requests, above 256 KiB).
//! - Help never acts: `-h` or `--help` anywhere prints help and sends
//!   nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};

const VERB: &str = "compute";

/// A case whose Python raises.
fn raises(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// A case run in a session (the passport, the hook socket set).
fn in_session(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: port_only,
        ..c
    }
}

/// One request answered `body`: exit `code`.
fn one(name: &'static str, args: &'static [&'static str], body: &'static str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

/// One request answered `reply`: exit `code`.
fn reply(name: &'static str, args: &'static [&'static str], reply: Reply, code: i32) -> Case {
    case(name, args, &[], vec![reply], owner_and_port, code, 1)
}

/// Words that stop before anything is sent: exit `code`.
fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn daemon_error() -> Reply {
    Reply::Answer(
        409,
        r#"{"error":{"code":"seat_busy","hint":"the seat is in use"}}"#.into(),
    )
}

#[test]
fn compute_retry() {
    let rig = Rig::new(VERB, "retry");
    let v = vec![
        // covers: compute retry
        one(
            "retrying",
            &["retry", "abcd1234"],
            r#"{"job":{"attempt":2,"short":"abcd1234"}}"#,
            0,
        ),
        one("a state", &["retry", "abcd"], r#"{"state":"queued"}"#, 0),
        one("no state", &["retry", "abcd"], "{}", 0),
        one(
            "a dict state",
            &["retry", "abcd"],
            r#"{"state":{"a":[1,true,null]}}"#,
            0,
        ),
        raises(one(
            "a job with no attempt",
            &["retry", "abcd"],
            r#"{"job":{}}"#,
            1,
        )),
        raises(one("a list", &["retry", "abcd"], "[1]", 1)),
        raises(one("no JSON", &["retry", "abcd"], "<html>", 1)),
        reply("the daemon's error", &["retry", "abcd"], daemon_error(), 3),
        reply("an empty reply", &["retry", "abcd"], Reply::Close, 3),
        reply(
            "a permission card",
            &["retry", "abcd"],
            Reply::Answer(403, PERMISSION_ASK.into()),
            3,
        ),
        reply(
            "a bad request",
            &["retry", "abcd"],
            Reply::Answer(400, r#"{"error":{"code":"bad","hint":"no"}}"#.into()),
            2,
        ),
        words("no job", &["retry"], 2),
        words("an empty job", &["retry", ""], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

const ROSTER: &str = r#"{"latest":{"seq":4,"keyFingerprint":"abcdef0123456789abcdef","issuedAt":"2026-11-01T00:00:00Z","entries":[{"name":"mini","status":"active","taintBasis":null,"host":"h1","nodeFp":"0123456789abcdef"},{"name":"a-very-long-node-name","status":"retired","taintBasis":"shared","host":"h2","nodeFp":"fedcba"}]},"stale":true,"nodes":[{"name":"mini","statement":{"taint":"clean"},"roster":"yes"},{"name":"old","statement":{}}],"pending":[{"id":"ctr_1","summary":"set mini"}]}"#;

const PROPOSED: &str = r#"{"request":{"id":"ctr_9","summary":"set mini","note":"café ✓"},"held":true,"n":[1,2.5,null]}"#;

#[test]
fn compute_roster() {
    let rig = Rig::new(VERB, "roster");
    let v = vec![
        // covers: compute roster
        one("a roster", &["roster"], ROSTER, 0),
        one(
            "show, not stale",
            &["roster", "show"],
            r#"{"latest":{"seq":1,"keyFingerprint":"k","issuedAt":"t","entries":[]}}"#,
            0,
        ),
        one(
            "no roster yet",
            &["roster"],
            r#"{"latest":null,"nodes":[]}"#,
            0,
        ),
        // covers: compute roster --json
        one("--json", &["roster", "--json"], ROSTER, 0),
        one("show --json", &["roster", "show", "--json"], PROPOSED, 0),
        raises(one("no key", &["roster"], r#"{"latest":{"seq":1}}"#, 1)),
        raises(one("a list", &["roster"], "[]", 1)),
        raises(one(
            "a statement that is a string",
            &["roster"],
            r#"{"nodes":[{"name":"n","statement":"x"}]}"#,
            1,
        )),
        raises(one("--json, no JSON", &["roster", "--json"], "", 1)),
        in_session(case(
            "in a session, over the socket",
            &["roster"],
            &[],
            vec![ok(ROSTER)],
            port_only,
            0,
            1,
        )),
        reply("the daemon's error", &["roster"], daemon_error(), 3),
        words("an unknown word", &["roster", "frob"], 2),
        // covers: compute roster set
        one(
            "set",
            &[
                "roster",
                "set",
                "mini",
                "--host",
                "h1",
                "--provider",
                "p",
                "--site",
                "s",
                "--admin-reach",
                "a,b",
                "--attest",
                "ok",
                "--why",
                "w",
                "--json",
            ],
            PROPOSED,
            0,
        ),
        one(
            "attest, --reason",
            &["roster", "attest", "mini", "--reason", "r"],
            PROPOSED,
            0,
        ),
        raises(one("set, no JSON", &["roster", "set", "mini"], "<html>", 1)),
        reply(
            "set, an empty reply",
            &["roster", "set", "mini"],
            Reply::Close,
            3,
        ),
        words("set, no node", &["roster", "set"], 2),
        words(
            "set, a flag for the node",
            &["roster", "set", "--host", "h"],
            2,
        ),
        words(
            "set, an unknown flag",
            &["roster", "set", "mini", "--frob"],
            2,
        ),
        words("set, --host last", &["roster", "set", "mini", "--host"], 2),
        // covers: compute roster retire
        one(
            "retire",
            &["roster", "retire", "mini", "--reason", "key_lost"],
            PROPOSED,
            0,
        ),
        words("retire, no node", &["roster", "retire"], 2),
        words(
            "retire, an unknown word",
            &["roster", "retire", "mini", "x"],
            2,
        ),
        words(
            "retire, --reason last",
            &["roster", "retire", "mini", "--reason"],
            2,
        ),
        // covers: compute roster revoke
        one(
            "revoke",
            &["roster", "revoke", "mini", "--json"],
            PROPOSED,
            0,
        ),
        words("revoke, no node", &["roster", "revoke", ""], 2),
        // covers: compute roster publish
        one("publish", &["roster", "publish"], r#"{"seq":5}"#, 0),
        one(
            "publish --resign",
            &["roster", "publish", "x", "--resign"],
            r#"{"seq":5,"resigned":true}"#,
            0,
        ),
        reply(
            "publish, the daemon's error",
            &["roster", "publish"],
            daemon_error(),
            3,
        ),
        // covers: compute roster key
        one(
            "key",
            &["roster", "key"],
            r#"{"fingerprint":"fp1","key":"pub"}"#,
            0,
        ),
        in_session(case(
            "key in a session",
            &["roster", "key"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

const SEATS: &str = r#"{"offered":[{"id":"seat_1","clientLabel":"acme","lane":"ci","state":"active","nodes":["a","b"]}],"held":[{"id":"seat_2","ownerId":"globex","lane":"ci","state":"suspended","nodes":[],"note":"maintenance"},{"id":"seat_3","ownerId":"globex","lane":"ci-standard-long","state":"active","nodes":["x"]}]}"#;

#[test]
fn compute_seats() {
    let rig = Rig::new(VERB, "seats");
    let v = vec![
        // covers: compute seats
        one("seats", &["seats"], SEATS, 0),
        one(
            "list, none",
            &["seats", "list"],
            r#"{"offered":[],"held":[]}"#,
            0,
        ),
        // covers: compute seats --json
        one("--json", &["seats", "--json"], SEATS, 0),
        one("list --json", &["seats", "list", "--json"], SEATS, 0),
        raises(one(
            "a note that is a number",
            &["seats"],
            r#"{"offered":[],"held":[{"id":"s","ownerId":"o","lane":"l","state":"x","nodes":[],"note":5}]}"#,
            1,
        )),
        raises(one(
            "a node that is a number",
            &["seats"],
            r#"{"offered":[{"id":"s","clientLabel":"c","lane":"l","state":"x","nodes":[1]}],"held":[]}"#,
            1,
        )),
        raises(one("no held", &["seats"], r#"{"offered":[]}"#, 1)),
        words("an unknown word", &["seats", "frob"], 2),
        // covers: compute seats request
        one(
            "request",
            &[
                "seats",
                "request",
                "box.example",
                "--lane",
                "ci",
                "--seat",
                "seat_1",
                "--secrets",
                "--why",
                "builds",
                "--json",
            ],
            PROPOSED,
            0,
        ),
        one(
            "request, secrets off",
            &[
                "seats",
                "request",
                "box.example",
                "--secrets",
                "--no-secrets",
            ],
            PROPOSED,
            0,
        ),
        words("request, no owner", &["seats", "request"], 2),
        words(
            "request, an unknown flag",
            &["seats", "request", "box", "--frob"],
            2,
        ),
        words(
            "request, --lane last",
            &["seats", "request", "box", "--lane"],
            2,
        ),
        // covers: compute seats offer
        one(
            "offer",
            &["seats", "offer", "ctr_1", "--nodes", "a,b", "--json"],
            PROPOSED,
            0,
        ),
        words("offer, no nodes", &["seats", "offer", "ctr_1"], 2),
        words("offer, no request", &["seats", "offer", "--nodes", "a"], 2),
        words(
            "offer, --nodes last",
            &["seats", "offer", "ctr_1", "--nodes"],
            2,
        ),
        reply(
            "offer, a permission card",
            &["seats", "offer", "r", "--nodes", "a"],
            Reply::Answer(403, PERMISSION_ASK.into()),
            3,
        ),
        // covers: compute seats end
        words("end, no seat", &["seats", "end"], 2),
        words("end, an unknown word", &["seats", "end", "seat_1", "x"], 2),
        words("end, --why last", &["seats", "end", "seat_1", "--why"], 2),
        in_session(case(
            "end in a session",
            &["seats", "end", "seat_1"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )),
        // covers: compute seats suspend
        in_session(case(
            "suspend in a session",
            &["seats", "suspend", "seat_1", "--why", "w"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )),
        // covers: compute seats resume
        in_session(case(
            "resume in a session",
            &["seats", "resume", "seat_1"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )),
        words("resume, no seat", &["seats", "resume"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

const REQUESTS: &str = r#"{"requests":[{"id":"ctr_abc123","state":"pending","kind":"seat","summary":"seat seat_1 for acme on mini","digest":"d1","reason":"needs ci"},{"id":"ctr_xyz","state":"approved","kind":"roster","summary":"set mini","digest":"d2"}]}"#;

#[test]
fn compute_trust() {
    let rig = Rig::new(VERB, "trust");
    let v = vec![
        // covers: compute trust
        one("requests", &["trust"], REQUESTS, 0),
        one("ls --all", &["trust", "ls", "--all"], REQUESTS, 0),
        one("--all", &["trust", "--all"], r#"{"requests":[]}"#, 0),
        // covers: compute trust --json
        one("--json", &["trust", "--json"], REQUESTS, 0),
        one(
            "list --json",
            &["trust", "list", "--json", "--all"],
            REQUESTS,
            0,
        ),
        raises(one(
            "no kind",
            &["trust"],
            r#"{"requests":[{"id":"x","state":"s","summary":"y"}]}"#,
            1,
        )),
        in_session(case(
            "in a session",
            &["trust", "requests"],
            &[],
            vec![ok(REQUESTS)],
            port_only,
            0,
            1,
        )),
        words("an unknown word", &["trust", "frob"], 2),
        // covers: compute trust deny
        one(
            "deny",
            &["trust", "deny", "ctr_1", "--why", "no"],
            PROPOSED,
            0,
        ),
        one(
            "deny, --why first",
            &["trust", "deny", "--why", "x"],
            PROPOSED,
            0,
        ),
        one(
            "deny, --why with no text",
            &["trust", "deny", "ctr_1", "--why"],
            PROPOSED,
            0,
        ),
        words("deny, no id", &["trust", "deny"], 2),
        // covers: compute trust withdraw
        one("withdraw", &["trust", "withdraw", "ctr_1"], PROPOSED, 0),
        reply(
            "withdraw, the daemon's error",
            &["trust", "withdraw", "ctr_1"],
            daemon_error(),
            3,
        ),
        words("withdraw, no id", &["trust", "withdraw"], 2),
        // covers: compute trust approve
        words("approve, no id", &["trust", "approve"], 2),
        in_session(case(
            "approve in a session",
            &["trust", "approve", "ctr_abc"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )),
        one(
            "approve, no request matches",
            &["trust", "approve", "ctr_nope"],
            REQUESTS,
            3,
        ),
        one(
            "approve, two match",
            &["trust", "approve", "ctr_"],
            REQUESTS,
            3,
        ),
        one(
            "approve, not pending",
            &["trust", "approve", "ctr_xyz"],
            REQUESTS,
            3,
        ),
        raises(one(
            "approve, an id that is a number",
            &["trust", "approve", "1"],
            r#"{"requests":[{"id":1}]}"#,
            3,
        )),
        reply(
            "approve, the list refused",
            &["trust", "approve", "ctr_abc"],
            daemon_error(),
            3,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

const OWNERS: &str = r#"{"self":{"ownerId":"me","key":{"fingerprint":"fp1"}},"trusted":[{"ownerId":"globex","keyFp":"fp2","latest":{"seq":3},"secretsConsent":true},{"ownerId":"acme","keyFp":"fp3","secretsConsent":false}]}"#;

#[test]
fn compute_owners() {
    let rig = Rig::new(VERB, "owners");
    let v = vec![
        // covers: compute owners
        one("owners", &["owners"], OWNERS, 0),
        one("none", &["owners", "x"], r#"{"self":null,"trusted":[]}"#, 0),
        // covers: compute owners --json
        one("--json", &["owners", "--json"], OWNERS, 0),
        raises(one("no trusted", &["owners"], r#"{"self":null}"#, 1)),
        // covers: compute owners consent
        one(
            "consent off",
            &["owners", "consent", "globex", "off", "--json"],
            r#"{"owner":"globex","on":false}"#,
            0,
        ),
        one(
            "consent off, no owner",
            &["owners", "consent", "", "off"],
            r#"{"on":false}"#,
            0,
        ),
        words("consent, no on|off", &["owners", "consent", "globex"], 2),
        words(
            "consent, a bad word",
            &["owners", "consent", "globex", "yes"],
            2,
        ),
        in_session(case(
            "consent on in a session",
            &["owners", "consent", "globex", "on"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )),
        one(
            "consent on, not pinned",
            &["owners", "consent", "nobody", "on"],
            OWNERS,
            3,
        ),
        raises(one(
            "consent on, a list",
            &["owners", "consent", "globex", "on"],
            "[]",
            1,
        )),
        raises(case(
            "consent on, a preview with no text",
            &["owners", "consent", "fp2", "on"],
            &[],
            vec![ok(OWNERS), ok(r#"{"owner":"globex"}"#)],
            owner_and_port,
            1,
            2,
        )),
        case(
            "consent on, the preview refused",
            &["owners", "consent", "globex", "on"],
            &[],
            vec![ok(OWNERS), daemon_error()],
            owner_and_port,
            3,
            2,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// A case on the vault rig whose command line starts with `compute`.
fn vcase(name: &'static str, args: &[&'static str]) -> VCase {
    let mut c = VCase::new(name, args);
    c.noun = VERB;
    c
}

/// A person's step: the daemon's answers, the confirm on the vault
/// socket (the vault password, typed on stdin), then the step with the
/// token.
fn person(name: &'static str, args: &[&'static str], daemon: Vec<Reply>, code: i32) -> VCase {
    let mut c = vcase(name, args);
    c.daemon_requests = daemon.len();
    c.daemon = daemon;
    c.vault = Some(vec![
        ok(r#"{"methods":["vault-password"]}"#),
        ok(r#"{"token":"ctok-5150"}"#),
    ]);
    c.vault_requests = 2;
    c.stdin = b"pass-9191\n".to_vec();
    c.code = code;
    c.secrets = vec!["ctok-5150", "pass-9191"];
    c
}

#[test]
fn compute_trust_a_person_s_steps() {
    let rig = VRig::new();
    let mut v = vec![
        person(
            "seats end",
            &["seats", "end", "seat_1", "--why", "done"],
            vec![ok(PROPOSED)],
            0,
        ),
        person(
            "seats suspend",
            &["seats", "suspend", "seat_1"],
            vec![ok(r#"{"state":"suspended"}"#)],
            0,
        ),
        person(
            "seats resume",
            &["seats", "resume", "seat_1", "--json"],
            vec![ok(r#"{"state":"active"}"#)],
            0,
        ),
        person(
            "seats end, refused",
            &["seats", "end", "seat_1"],
            vec![daemon_error()],
            3,
        ),
        person(
            "trust approve",
            &["trust", "approve", "ctr_abc"],
            vec![
                ok(REQUESTS),
                ok(r#"{"result":{"consentNext":"run the owners consent for globex"}}"#),
            ],
            0,
        ),
        person(
            "trust approve, no next",
            &["trust", "approve", "ctr_abc123"],
            vec![ok(REQUESTS), ok(r#"{"result":null}"#)],
            0,
        ),
        person(
            "owners consent on",
            &["owners", "consent", "globex", "on"],
            vec![
                ok(OWNERS),
                ok(
                    r#"{"text":"Send vault secrets to globex's nodes?","owner":"globex","keyFp":"fp2"}"#,
                ),
                ok(r#"{"owner":"globex","on":true}"#),
            ],
            0,
        ),
    ];
    let mut raised = person(
        "trust approve, an answer that is a list",
        &["trust", "approve", "ctr_abc"],
        vec![ok(REQUESTS), ok("[1]")],
        1,
    );
    raised.compare = VCompare::Traceback;
    v.push(raised);
    // The vault refuses the confirm: nothing more is sent.
    let mut refused = vcase(
        "seats end, the confirm refused",
        &["seats", "end", "seat_1"],
    );
    refused.vault = Some(vec![
        ok(r#"{"methods":["vault-password"]}"#),
        Reply::Answer(
            403,
            r#"{"error":{"code":"bad_password","hint":"wrong"}}"#.into(),
        ),
    ]);
    refused.vault_requests = 2;
    refused.stdin = b"pass-9191\n".to_vec();
    refused.code = 3;
    refused.secrets = vec!["pass-9191"];
    v.push(refused);
    // Roster key outside a session: one POST over TCP.
    let mut key = vcase("roster key", &["roster", "key"]);
    key.daemon = vec![ok(r#"{"fingerprint":"fp1"}"#)];
    key.daemon_requests = 1;
    key.vault = None;
    v.push(key);
    for c in &v {
        rig.check(c);
    }
}

/// The large-list case: thousands of trust requests, an answer above
/// 256 KiB.
#[test]
fn compute_trust_large_list() {
    let rig = Rig::new(VERB, "trustbig");
    let rows: Vec<String> = (0..3000u64)
        .map(|i| {
            format!(
                r#"{{"id":"ctr_{i:08x}","state":"pending","kind":"seat","summary":"seat for client {i} on node-{i}","digest":"d{i}"}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"requests":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    let body: &'static str = Box::leak(body.into_boxed_str());
    rig.check(&one("3000 requests", &["trust", "--all"], body, 0));
    rig.check(&one("3000 requests, --json", &["trust", "--json"], body, 0));
}

/// Help never acts.
#[test]
fn compute_trust_help_never_acts() {
    let rig = Rig::new(VERB, "trusthelp");
    let lines: &[&[&str]] = &[
        &["retry", "abcd", "-h"],
        &["roster", "key", "--help"],
        &["roster", "set", "mini", "-h"],
        &["roster", "retire", "-h", "mini"],
        &["roster", "revoke", "mini", "--help"],
        &["roster", "publish", "--resign", "-h"],
        &["seats", "request", "box", "--help"],
        &["seats", "offer", "r", "--nodes", "a", "-h"],
        &["seats", "end", "seat_1", "-h"],
        &["seats", "suspend", "--help", "seat_1"],
        &["seats", "resume", "seat_1", "-h"],
        &["trust", "approve", "ctr_1", "-h"],
        &["trust", "deny", "ctr_1", "--help"],
        &["trust", "withdraw", "-h", "ctr_1"],
        &["owners", "consent", "globex", "on", "-h"],
        &["owners", "consent", "globex", "off", "--help"],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn the_trust_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "trustbrand");
    for (args, want) in [
        (&["retry"][..], format!("usage: {cli} compute retry <job>\n")),
        (
            &["roster", "frob"],
            format!("usage: {cli} compute roster [show|key|set|retire|revoke|publish] ({cli} compute --help)\n"),
        ),
        (
            &["seats", "end"],
            format!("usage: {cli} compute seats end <seat> [--why TEXT]\n"),
        ),
        (
            &["trust", "approve"],
            format!("usage: {cli} compute trust approve <ctr_…>\n"),
        ),
        (
            &["owners", "consent", "x", "y"],
            format!("usage: {cli} compute owners consent <owner> on|off\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
