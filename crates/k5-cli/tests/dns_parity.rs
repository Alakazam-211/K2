//! `k5-cli dns` against the shell CLI's `cli/k2 dns`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `access`, `list`, `records`, `record add`, `record remove` (and its
//!   aliases) and `verify`, text and `--json`: what each prints, odd
//!   values, Python's raise part way.
//! - The daemon's errors: a gate (with and without Settings in its hint),
//!   pending nameservers, a read-only zone, a type not allowed, a code
//!   Python can't hash, `"ok": false`, a permission card.
//! - The words: help for `dns` and `dns record` alone or with an empty
//!   verb, unknown verbs, flags, extra words, `--ttl` and `--priority`
//!   (missing, not a number, leading zeros), empty values, a domain that
//!   needs escaping in the query.
//! - The cell socket (every route is its), no answer there for a GET
//!   (TCP) and a POST (not sent again), the gates.
//! - One large list (above 256 KiB), as for `cert`.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "dns";
const ZONES: &str = r#"{"ok":true,"zones":[{"domain":"acme.example","status":"active","id":"z1"},{"domain":"globex.example","status":"pending_ns","nameservers":["ns1.example","ns2.example"],"id":"z2"},{"domain":"initech.example","state":"error"},"raw"]}"#;
const RECORDS: &str = r#"{"ok":true,"domain":"acme.example","records":[{"id":"r1","type":"MX","name":"@","ttl":300,"content":"mx.acme.example","priority":10},{"id":"r2","type":"A","name":"www","ttl":null,"value":"203.0.113.7","managed_by":"publish"},{"type":"TXT","name":"_x","content":"v=é","prio":0,"managed_by":"user"},5]}"#;

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
fn dns_reads_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "reads");
    let v = vec![
        // covers: dns access
        case(
            "access: allowed",
            &["access"],
            &[],
            vec![ok(
                r#"{"ok":true,"allowed":true,"workspace":{"path":"/srv/acme"},"record_types":["A","AAAA","MX"],"zones":[{"domain":"acme.example","id":"z1"},"x"]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access: denied, its hint",
            &["access"],
            &[],
            vec![ok(r#"{"allowed":false,"hint":"ask é","zones":"ab"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access: from the capability",
            &["access"],
            &[],
            vec![ok(
                r#"{"capability":{"allowed":1},"workspace":{},"zones":{}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: dns access --json
        case(
            "access --json",
            &["access", "--json"],
            &[],
            vec![ok(r#"{"allowed":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "access: zones with no len",
            &["access"],
            &[],
            vec![ok(r#"{"allowed":true,"zones":2}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "access: a workspace that is not a dict",
            &["access"],
            &[],
            vec![ok(r#"{"allowed":true,"workspace":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: dns list
        case(
            "list",
            &["list"],
            &[],
            vec![ok(ZONES)],
            owner_and_port,
            0,
            1,
        ),
        // covers: dns list --json
        case(
            "list --json",
            &["list", "--json"],
            &[],
            vec![ok(ZONES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list: none",
            &["list"],
            &[],
            vec![ok(r#"{"zones":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "list: nameservers that can't be walked",
            &["list"],
            &[],
            vec![ok(
                r#"{"zones":[{"domain":"a","status":"pending","nameservers":1}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        // covers: dns records
        case(
            "records",
            &["records", "acme.example"],
            &[],
            vec![ok(RECORDS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: dns records --json
        case(
            "records: from the zone, a domain that needs escaping",
            &["records", "a b/é&c", "--json"],
            &[],
            vec![ok(r#"{"zone":{"domain":"a","records":[]}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "records: none, the zone's domain",
            &["records", "a"],
            &[],
            vec![ok(
                r#"{"records":null,"zone":{"domain":"acme.example","records":[]}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "records that can't be walked",
            &["records", "a"],
            &[],
            vec![ok(r#"{"records":true}"#)],
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
fn dns_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "writes");
    let v = vec![
        // covers: dns record add
        case(
            "record add",
            &[
                "record",
                "add",
                "acme.example",
                "MX",
                "@",
                "mx.acme.example",
                "--ttl",
                "0300",
                "--priority",
                "10",
            ],
            &[],
            vec![ok(
                r#"{"ok":true,"record":{"id":"r9","type":"MX","name":"@","content":"mx.acme.example"},"propagation":"~60s"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: dns record add --json
        case(
            "record add: empty values, --prio, --json",
            &[
                "record", "add", "", "A", "", "1.2.3.4", "--prio", "0", "--json",
            ],
            &[],
            vec![ok(r#"{"ok":true,"id":"r1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "record add: no record in the answer",
            &["record", "add", "a", "TXT", "_x", "v é \"q\""],
            &[],
            vec![ok(r#"{"id":"r2","value":"v"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "record add: a type not allowed, exit 2",
            &["record", "add", "a", "NS", "@", "x"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"type_not_allowed","hint":"NS is the zone's"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        // covers: dns record remove
        case(
            "record remove",
            &["record", "remove", "r1"],
            &[],
            vec![ok(r#"{"ok":true,"id":"r1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: dns
        case(
            "record rm, record_id",
            &["record", "rm", "r1"],
            &[],
            vec![ok(r#"{"record_id":"r1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: dns --json
        case(
            "record delete --json",
            &["record", "delete", "r1", "--json"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "record del: nothing named",
            &["record", "del", "r1"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: dns verify
        case(
            "verify",
            &["verify", "acme.example"],
            &[],
            vec![ok(
                r#"{"ok":true,"domain":"acme.example","status":"active","hint":"all good","checks":[{"name":"ns","ok":true},{"status":"fail","check":"soa"},{"x":"é"},"plain"]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "verify: no status",
            &["verify", "a"],
            &[],
            vec![ok(r#"{"zone":{"domain":""},"results":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: dns verify --json
        case(
            "verify --json",
            &["verify", "a", "--json"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ok false",
            &["verify", "a"],
            &[],
            vec![ok(r#"{"ok":false,"status":"x"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset: no response",
            &["record", "rm", "r1"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        waited(case(
            "an empty reply: the line",
            &["verify", "a"],
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
fn dns_errors_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "errors");
    let err = |name: &'static str, body: &str, status: u16, code: i32| {
        case(
            name,
            &["list"],
            &[],
            vec![Reply::Answer(status, body.into())],
            owner_and_port,
            code,
            1,
        )
    };
    let v = vec![
        err(
            "the switch is off: the teaching text",
            r#"{"error":{"code":"dns_manage_disabled","hint":"off é"}}"#,
            403,
            3,
        ),
        err(
            "forbidden, no hint",
            r#"{"error":{"code":"forbidden"}}"#,
            403,
            3,
        ),
        err(
            "gated, Settings named",
            r#"{"error":{"code":"gated","hint":"see Settings"}}"#,
            403,
            3,
        ),
        err(
            "pending nameservers",
            r#"{"error":{"code":"zone_pending_ns","hint":"zone_pending_ns"}}"#,
            409,
            3,
        ),
        err(
            "pending nameservers, a hint",
            r#"{"error":{"code":"zone_pending_ns","hint":"wait"}}"#,
            409,
            3,
        ),
        err(
            "pending nameservers, the hint says so",
            r#"{"error":{"code":"zone_pending_ns","hint":"Pending: x"}}"#,
            409,
            3,
        ),
        err(
            "read-only",
            r#"{"error":{"code":"zone_readonly","hint":"suspended"}}"#,
            409,
            3,
        ),
        err("an error text", r#"{"error":"nope"}"#, 500, 1),
        err("a refusal with a permission card", PERMISSION_ASK, 403, 3),
        traceback(err(
            "a code Python can't hash",
            r#"{"error":{"code":[1]}}"#,
            500,
            1,
        )),
        traceback(err(
            "a gate's hint that is null",
            r#"{"error":{"code":"forbidden","hint":null}}"#,
            403,
            1,
        )),
        traceback(err(
            "a gate's hint that is a list",
            r#"{"error":{"code":"forbidden","hint":["a"]}}"#,
            403,
            1,
        )),
        traceback(err(
            "a pending hint that is a number",
            r#"{"error":{"code":"zone_pending_ns","hint":5}}"#,
            409,
            1,
        )),
        err("not JSON", "x", 200, 1),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn dns_words_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        case(
            "dns alone: its help",
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
            "record alone: its help",
            &["record"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "record, an empty verb: its help",
            &["record", ""],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "an unknown verb",
            &["zones"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown record verb",
            &["record", "list"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "access: a word",
            &["access", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "list: a flag",
            &["list", "-a"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "records: none",
            &["records"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "records: two",
            &["records", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "record add: three",
            &["record", "add", "a", "b", "c"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "record add: five",
            &["record", "add", "a", "b", "c", "d", "e"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "record add: --ttl alone",
            &["record", "add", "a", "--ttl"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "record add: --prio alone",
            &["record", "add", "a", "--prio"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "record add: a ttl that is no number",
            &["record", "add", "a", "b", "c", "d", "--ttl", "1h"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "record add: a priority that is no number",
            &["record", "add", "a", "b", "c", "d", "--priority", " 1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "record add: a flag",
            &["record", "add", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: dns record remove --json
        case(
            "record remove: none",
            &["record", "remove", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "record remove: two",
            &["record", "rm", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "verify: none",
            &["verify"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "verify: a flag",
            &["verify", "a", "--all"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("no token", &["list"], &[], vec![], port_only, 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn dns_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a host session: TCP",
                &["list"],
                &[],
                vec![ok(ZONES)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket: records",
                &["records", "a b"],
                &[],
                vec![ok(RECORDS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket: verify",
                &["verify", "a"],
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
                "not served: TCP",
                &["record", "rm", "r1"],
                &[],
                vec![not_served(), ok(r#"{"id":"r1"}"#)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(),
            case(
                "a GET with no answer there: TCP",
                &["access"],
                &[],
                vec![Reply::Close, ok(r#"{"allowed":true}"#)],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(),
            case(
                "a POST with no answer there: not sent again",
                &["record", "add", "a", "A", "@", "1.2.3.4"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        with(
            cell(),
            case(
                "the gate there",
                &["list"],
                &[],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"dns_manage_disabled"}}"#.into(),
                )],
                port_only,
                3,
                1,
            ),
        ),
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
fn dns_records_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..4000)
        .map(|i| {
            format!(
                r#"{{"id":"rec-{i:05}","type":"A","name":"host{i:05}","ttl":300,"content":"203.0.113.{}"}}"#,
                i % 256
            )
        })
        .collect();
    let body = format!(
        r#"{{"ok":true,"domain":"acme.example","records":[{}]}}"#,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024, "{}", body.len());
    let mut want = format!(
        "zone: acme.example\n{:<6}  {:<24}  {:<8}  VALUE\n",
        "TYPE", "NAME", "TTL"
    );
    for i in 0..4000 {
        want.push_str(&format!(
            "{:<6}  {:<24}  {:<8}  203.0.113.{}  [rec-{i:05}]\n",
            "A",
            format!("host{i:05}"),
            "300",
            i % 256
        ));
    }
    let c = case(
        "a large list",
        &["records", "acme.example"],
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
fn dns_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell(
            "record add --help",
            &["record", "add", "a", "A", "@", "1.2.3.4", "--help"],
        ),
        shell(
            "record add, -h as the ttl",
            &["record", "add", "a", "A", "@", "1.2.3.4", "--ttl", "-h"],
        ),
        shell("record remove -h", &["record", "remove", "-h", "r1"]),
        shell("record rm --help", &["record", "rm", "r1", "--help"]),
        shell("verify -h", &["verify", "a", "-h"]),
    ] {
        rig.check(&c);
    }
}
