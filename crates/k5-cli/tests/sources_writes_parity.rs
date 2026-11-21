//! `k5-cli sources` against the shell CLI's `cli/k2 sources` for the
//! verbs that write or try a source: `propose`, `add`, `edit`, `try`,
//! `approve`, `secret`, `pin`, `unpin` and `trigger`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - The flags parser (`_sources_spec_json`): every flag, its errors, and
//!   `--from` with a file, a missing file, a file that is not one object.
//! - Each verb with `--json` and without; `approve` and `secret` inside a
//!   K2 terminal (no confirm) and outside it (the source id, then the
//!   vault confirm, which fails here: no vault socket).
//! - `trigger`: every sub-verb, a short id matched from the list, and
//!   thousands of triggers (above 256 KiB).
//!
//! Rows named `bash D6:` are a value flag as the last word: `shift 2`
//! fails under `set -e` and bash ends with 1, saying nothing (copied until
//! bash is fixed). The row named `bash bug: --from -` is the parser's
//! program arriving on stdin (`python3 - <<'PY'`), so `--from -` reads an
//! input that has already ended (`sources.sh:200`, `:221`).

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "sources";

/// A full id (36 characters): no list lookup.
const ID: &str = "11111111-2222-3333-4444-555555555555";

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn in_k2(c: Case) -> Case {
    with(with_passport(&[]), c)
}

fn spec_files(r: &Rig) {
    owner_and_port(r);
    let put = |name: &str, text: &str| {
        std::fs::write(r.proj.join(name), text).unwrap_or_else(|e| panic!("write {name}: {e}"))
    };
    put(
        "s.json",
        r#"{"label":"L é","fetch":{"url":"https://e.example"},"name":"old","everySecs":30}"#,
    );
    put("arr.json", "[1]");
    put("bad.json", "{nope");
    put("fetch5.json", r#"{"name":"a","label":"b","fetch":5}"#);
}

fn trigger_row(i: usize) -> String {
    format!(
        r#"{{"id":"{i:08}-aaaa-bbbb-cccc-dddddddddddd","source":"src-{i}","on":{{"kind":"stale_for","secs":{}}},"agent":{{"handle":"bot{i}","missing":{}}},"status":"active","cooldownSecs":300,"dailyCap":5,"firesToday":{},"lastFireAt":{},"note":"{}"}}"#,
        i % 3 * 60,
        i.is_multiple_of(5),
        i % 4,
        if i.is_multiple_of(2) {
            "1763600000".to_string()
        } else {
            "null".to_string()
        },
        "n".repeat(i % 30)
    )
}

fn triggers(n: usize) -> String {
    let rows: Vec<String> = (0..n).map(trigger_row).collect();
    format!(r#"{{"triggers":[{}]}}"#, rows.join(","))
}

fn fires() -> String {
    format!(
        r#"{{"trigger":{},"fires":[
        {{"at":1763600000,"outcome":"fired","woke":true,"incidentId":"inc-0123456789abcdef","itemKey":"eu-1"}},
        {{"at":1763000000.7,"outcome":"suppressed","suppressReason":"cooldown","detail":"d é","incidentId":"inc-2"}}]}}"#,
        trigger_row(2)
    )
}

#[test]
fn sources_propose_add_and_edit() {
    let rig = Rig::new(VERB, "create");
    let v = vec![
        // covers: sources propose
        case(
            "propose with flags",
            &[
                "propose",
                "--name",
                "web",
                "--label",
                "The web",
                "--url",
                "https://e.example/x",
                "--every",
                "5m",
                "--field",
                "up=/ok",
                "--rule",
                "up:crit:eq:false",
                "--agg",
                "n=count",
                "--secret",
                "API",
            ],
            &[],
            vec![ok(
                r#"{"name":"web","status":"proposed","approval":"one_click","warnings":[{"hint":"the origin is slow"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources propose --json
        case(
            "propose --from a file, --json",
            &["propose", "--from", "s.json", "--name", "web", "--json"],
            &[],
            vec![ok(
                r#"{"name":"web","status":"proposed","approval":"owner_only"}"#,
            )],
            spec_files,
            0,
            1,
        ),
        // covers: sources add
        case(
            "add with every list and item flag",
            &[
                "add",
                "--name",
                "db",
                "--label",
                "D \u{e9}",
                "--target",
                "eu=https://a",
                "--target",
                " us =https://b",
                "--item-field",
                "ms=/ms",
                "--item-key",
                "/id",
                "--list",
                "l=/rows",
                "--list-key",
                "l=/id",
                "--list-field",
                "l.v=/v",
                "--count",
                "n=/n",
                "--header",
                "X-A: b ",
                "--max-bytes",
                "512K",
                "--timeout",
                "10s",
                "--expect-status",
                "200, 401,",
                "--trend",
                "ms",
                "--fail-after",
                "3",
                "--link",
                "https://l",
                "--viewers",
                "admin",
                "--agent-access",
                "shape",
                "--keep",
                "states",
                "--origin",
                "https://o",
                "--on-target-error",
                "skip",
                "--yes",
            ],
            &[],
            vec![ok(
                r#"{"name":"db","status":"proposed","approval":"needs_confirm","warnings":[]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources add --json
        case(
            "add a preset, --json",
            &[
                "add",
                "--preset",
                "uptime",
                "--preset-arg",
                "url=https://x",
                "--name",
                "p",
                "--label",
                "P",
                "--json",
            ],
            &[],
            vec![ok(r#"{"name":"p","status":"active"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add: url-secret, origin after it, rules and aggregates",
            &[
                "add",
                "--name",
                "s",
                "--label",
                "S",
                "--origin",
                "https://first",
                "--url-secret",
                "URL",
                "--origin",
                "https://o",
                "--every",
                "1h",
                "--agg",
                "c=count-where:state:in:a|b|3",
                "--agg",
                "m=max:ms",
                "--agg",
                "k=count:l",
                "--rule",
                "ms:warn:gt:1.5,crit:gte:@limit,critAfter:3",
                "--rule",
                "v:crit:versionBehind:@latest~minor=2",
                "--rule",
                "w:crit:versionAtLeast:1.2",
                "--rule",
                "a:crit:ageGt:30m",
                "--rule",
                "e:ok:exists: No",
                "--rule",
                "x:warn:ne:{\"a\":1}",
                "--rule",
                "y:crit:lt:-7",
            ],
            &[],
            vec![ok(r#"{"name":"s","status":"active"}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "add: the answer is not what the reader expects",
            &["add", "--name", "a", "--label", "b", "--url", "https://e"],
            &[],
            vec![ok(r#"{"status":"active"}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: sources edit
        case(
            "edit",
            &["edit", "web", "--label", "New", "--yes", "--every", "2m"],
            &[],
            vec![ok(
                r#"{"name":"web","status":"proposed","unbound":true,"warnings":[{"hint":"h"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources edit --json
        case(
            "edit --json: the set has no name",
            &[
                "edit", "--json", "web", "--name", "renamed", "--every", "30",
            ],
            &[],
            vec![ok(r#"{"name":"web","status":"active"}"#)],
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
fn sources_spec_errors() {
    let rig = Rig::new(VERB, "specerr");
    let mut v = Vec::new();
    for (name, args) in [
        ("propose: no name", &["propose"][..]),
        (
            "propose: nothing to fetch",
            &["propose", "--name", "a", "--label", "b"],
        ),
        ("a bad duration", &["propose", "--every", "5x"]),
        ("a bad size", &["add", "--max-bytes", "1G"]),
        ("an unknown flag", &["propose", "--bogus"]),
        ("a word, not a flag", &["add", "web"]),
        ("a flag with no value", &["propose", "--name"]),
        ("a header with no colon", &["propose", "--header", "x"]),
        ("a field with no =", &["propose", "--field", "x"]),
        ("a rule with no parts", &["propose", "--rule", "x"]),
        (
            "a rule's unknown operator",
            &["propose", "--rule", "f:crit:zz:1"],
        ),
        ("a rule's bad level", &["propose", "--rule", "f:hot:eq:1"]),
        (
            "a rule's warnAfter",
            &["propose", "--rule", "f:warnAfter:x"],
        ),
        ("lt with no number", &["propose", "--rule", "f:crit:lt:abc"]),
        (
            "a version distance",
            &["propose", "--rule", "f:crit:versionBehind:1~minor=x"],
        ),
        ("a bad aggregate", &["propose", "--agg", "a=sum"]),
        (
            "a list field with no dot",
            &["propose", "--list-field", "lv=/v"],
        ),
        (
            "a list with no pointer",
            &["propose", "--list-key", "l=/id"],
        ),
        ("bad statuses", &["propose", "--expect-status", "2xx"]),
        (
            "--targets is for try",
            &[
                "add",
                "--name",
                "a",
                "--label",
                "b",
                "--url",
                "u",
                "--targets",
                "2",
            ],
        ),
        ("--targets takes a number", &["add", "--targets", "x"]),
        (
            "--from a missing file",
            &["propose", "--from", "missing.json"],
        ),
        ("--from not one object", &["propose", "--from", "arr.json"]),
        ("--from not JSON", &["propose", "--from", "bad.json"]),
        // A bash bug, copied: the program came on stdin.
        ("bash bug: --from -", &["propose", "--from", "-"]),
        ("edit with no name", &["edit", "--every", "5m"]),
        ("edit: --targets", &["edit", "web", "--targets", "2"]),
        ("edit: a value flag last", &["edit", "web", "--every"]),
    ] {
        v.push(case(name, args, &[], vec![], spec_files, 2, 0));
    }
    v.push(traceback(case(
        "--fail-after not a number: the traceback, exit 2",
        &[
            "propose",
            "--name",
            "a",
            "--label",
            "b",
            "--url",
            "u",
            "--fail-after",
            "x",
        ],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    )));
    v.push(traceback(case(
        "--item-field with a fetch that is a number",
        &["propose", "--from", "fetch5.json", "--item-field", "a=/a"],
        &[],
        vec![],
        spec_files,
        2,
        0,
    )));
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn sources_try() {
    let rig = Rig::new(VERB, "try");
    let shape = r#"{"ok":true,"status":200,"elapsedMs":12,"truncated":true,
        "targets":[{"key":"eu","status":200,"elapsedMs":3},{"key":"us","status":0,"reason":"timeout","elapsedMs":5000}],
        "warnings":[{"code":"w","hint":"h"}],"rootKey":{"hint":"rk"},
        "tree":[{"pointer":"","type":"object","len":2},{"pointer":"/a","type":"string","sample":"é a long sample that is cut at eighty code points, so that the slice shows here ok"},
                {"pointer":"/a/b","type":"number","sample":1},{"pointer":"/c","type":"array","len":0}],
        "treeTruncated":true,"view":{"display":"warn","counts":{"ok":1,"warn":2}}}"#;
    let v = vec![
        // covers: sources try
        case(
            "try a draft, depth 1",
            &[
                "try",
                "--url",
                "https://e",
                "--targets",
                "2",
                "--tree-depth",
                "1",
            ],
            &[],
            vec![ok(shape)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources try --json
        case(
            "try a saved source, --json",
            &["try", "--name", "web", "--json"],
            &[],
            vec![ok(shape)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "try, failed",
            &["try", "--name", "web"],
            &[],
            vec![ok(r#"{"ok":false,"reason":"timeout","status":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "try --name with other flags names the draft",
            &["try", "--name", "web", "--url", "https://e"],
            &[],
            vec![ok(r#"{"ok":true,"status":200,"elapsedMs":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "try with nothing",
            &["try"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "try --tree-depth with no value",
            &["try", "--url", "u", "--tree-depth"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "bash D6: try --name last",
            &["try", "--url", "u", "--name"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "try: a parser error is exit 2",
            &["try", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "try: a parser traceback is exit 1",
            &["try", "--url", "u", "--fail-after", "x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        traceback(case(
            "try --tree-depth not a number: after the request",
            &["try", "--url", "u", "--tree-depth", "abc"],
            &[],
            vec![ok(shape)],
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
fn sources_approve_and_secret() {
    let rig = Rig::new(VERB, "approve");
    let pending = r#"{"sources":[{"name":"web","id":"s1","specHash":"h1","secret":"API"},
        {"name":"db","id":"s2","specHash":"h2"},{"name":"web","id":"s3","specHash":"h3"}]}"#;
    let v = vec![
        // covers: sources approve
        in_k2(case(
            "approve one, in K2: no confirm",
            &[
                "approve",
                "web",
                "--viewers",
                "admin",
                "--agent-access",
                "shape",
                "--keep",
                "states",
            ],
            &[],
            vec![ok(pending), ok(r#"{"name":"web","secretBound":true}"#)],
            port_only,
            0,
            2,
        )),
        // covers: sources approve --json
        in_k2(case(
            "approve two, --json",
            &[
                "approve",
                "web",
                "db",
                "--secret",
                "S",
                "--use-anyway",
                "--json",
            ],
            &[],
            vec![ok(pending), ok(r#"{"approved":["web","db"]}"#)],
            port_only,
            0,
            2,
        )),
        in_k2(case(
            "approve two",
            &["approve", "db,web"],
            &[],
            vec![ok(pending), ok(r#"{"approved":["db","web"]}"#)],
            port_only,
            0,
            2,
        )),
        case(
            "approve a name not waiting",
            &["approve", "nope", "web"],
            &[],
            vec![ok(pending)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "approve two with --viewers",
            &["approve", "web", "db", "--viewers", "admin"],
            &[],
            vec![ok(pending)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "approve outside K2: the confirm fails with no vault",
            &["approve", "web", "--secret", "S"],
            &[],
            vec![ok(pending)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "approve db outside K2: no secret, no confirm",
            &["approve", "db"],
            &[],
            vec![ok(pending), ok(r#"{"name":"db"}"#)],
            owner_and_port,
            0,
            2,
        ),
        traceback(case(
            "approve: a row with no name",
            &["approve", "web"],
            &[],
            vec![ok(r#"{"sources":[{"id":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "approve nothing",
            &["approve"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "approve --secret with no value",
            &["approve", "web", "--secret"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "bash D6: approve --keep last",
            &["approve", "web", "--keep"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "approve -x",
            &["approve", "web", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: sources secret
        in_k2(case(
            "secret one, in K2",
            &["secret", "web", "API", "--use-anyway"],
            &[],
            vec![ok(r#"{"name":"web","secret":"API","secretBound":true}"#)],
            port_only,
            0,
            1,
        )),
        // covers: sources secret --json
        in_k2(case(
            "secret two, in K2, --json",
            &["secret", "web", "db", "API", "--json"],
            &[],
            vec![ok(
                r#"{"names":["web","db"],"secret":"API","secretBound":true}"#,
            )],
            port_only,
            0,
            1,
        )),
        in_k2(case(
            "secret two, in K2",
            &["secret", "web", "db", "API"],
            &[],
            vec![ok(
                r#"{"names":["web","db"],"secret":"API","secretBound":true}"#,
            )],
            port_only,
            0,
            1,
        )),
        case(
            "secret --none",
            &["secret", "web", "--none"],
            &[],
            vec![ok(r#"{"name":"web","secretBound":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "secret --none, two",
            &["secret", "--none", "web", "db", "--use-anyway"],
            &[],
            vec![ok(r#"{"names":["web","db"],"secretBound":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "secret outside K2: no source id",
            &["secret", "web", "API"],
            &[],
            vec![ok(r#"{"source":{}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "secret outside K2: the confirm fails with no vault",
            &["secret", "web", "API"],
            &[],
            vec![ok(r#"{"source":{"id":"s1"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "secret two outside K2: the second has no id",
            &["secret", "web", "db", "API"],
            &[],
            vec![
                ok(r#"{"source":{"id":"s1"}}"#),
                ok(r#"{"source":{"id":null}}"#),
            ],
            owner_and_port,
            3,
            2,
        ),
        case(
            "secret with one word",
            &["secret", "web"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "secret -x",
            &["secret", "web", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn sources_pin_and_trigger() {
    let rig = Rig::new(VERB, "trigger");
    let one = format!(r#"{{"triggers":[{}]}}"#, trigger_row(2));
    let one: &'static str = Box::leak(one.into_boxed_str());
    let many: &'static str = Box::leak(triggers(2500).into_boxed_str());
    assert!(many.len() > 256 * 1024, "{} bytes", many.len());
    let fires: &'static str = Box::leak(fires().into_boxed_str());
    let v = vec![
        // covers: sources pin
        case(
            "pin",
            &["pin", "web"],
            &[],
            vec![ok(r#"{"name":"web","pinned":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources pin --json
        case(
            "pin --json",
            &["pin", "web", "--json"],
            &[],
            vec![ok(r#"{"name":"web","pinned":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "unpin, --json not third: text",
            &["unpin", "web", "x", "--json"],
            &[],
            vec![ok(r#"{"name":"web","pinned":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "pin with no name",
            &["pin", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger alone: its page",
            &["trigger"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "trigger help",
            &["triggers", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // covers: sources trigger
        case(
            "trigger list",
            &["trigger", "list", "--source", "src-2"],
            &[],
            vec![ok(one)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger ls, thousands (above 256 KiB)",
            &["triggers", "ls"],
            &[],
            vec![ok(many)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger list, none",
            &["trigger", "list"],
            &[],
            vec![ok(r#"{"triggers":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources trigger --json
        case(
            "trigger show by a short id, --json",
            &["trigger", "show", "00000002", "--json"],
            &[],
            vec![ok(one), ok(fires)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "trigger show by a short id",
            &["trigger", "show", "00000002-aaaa"],
            &[],
            vec![ok(one), ok(fires)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "trigger show: no trigger starts so",
            &["trigger", "show", "zz"],
            &[],
            vec![ok(one)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "trigger log",
            &["trigger", "log", ID, "--limit", "5"],
            &[],
            vec![ok(fires)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger log, none",
            &["trigger", "log", ID],
            &[],
            vec![ok(r#"{"fires":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger propose",
            &[
                "trigger",
                "propose",
                "--source",
                "web",
                "--on",
                "stale_for:30m",
                "--agent",
                "bot",
                "--note",
                "n é",
                "--cooldown",
                "1h",
                "--daily-cap",
                "3",
            ],
            &[],
            vec![ok(r#"{"id":"t-1234567890","status":"proposed"}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "trigger add, a cap that is not a number: the body is empty",
            &[
                "trigger",
                "add",
                "--source",
                "web",
                "--on",
                "went_crit",
                "--daily-cap",
                "x",
            ],
            &[],
            vec![ok(r#"{"id":"t1","status":"active"}"#)],
            owner_and_port,
            0,
            1,
        )),
        case(
            "trigger edit",
            &[
                "trigger",
                "edit",
                ID,
                "--no-note",
                "--on",
                "recovered",
                "--cooldown",
                "90",
            ],
            &[],
            vec![ok(one)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "trigger edit, a cap that is not a number",
            &["trigger", "edit", ID, "--daily-cap", "x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        case(
            "trigger pause",
            &["trigger", "pause", ID],
            &[],
            vec![ok(&format!(r#"{{"id":"{ID}","status":"paused"}}"#))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger rm",
            &["trigger", "rm", ID],
            &[],
            vec![ok(&format!(r#"{{"id":"{ID}","removed":true}}"#))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger test",
            &["trigger", "test", ID, "--json"],
            &[],
            vec![ok(r#"{"outcome":"fired","woke":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger test, text",
            &["trigger", "test", ID],
            &[],
            vec![ok(r#"{"outcome":"fired","woke":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger propose with no source",
            &["trigger", "propose", "--on", "went_crit"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger show with no id",
            &["trigger", "show"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger list --bogus",
            &["trigger", "list", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger --on with a duration it doesn't take",
            &["trigger", "list", "--on", "went_crit:5m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger --on stale_for, two units",
            &["trigger", "list", "--on", "stale_for:1h30m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger --cooldown, a unit with no digits",
            &[
                "trigger",
                "add",
                "--source",
                "web",
                "--on",
                "went_crit",
                "--cooldown",
                "m",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger --on stale_for and --cooldown with leading zeros",
            &[
                "trigger",
                "propose",
                "--source",
                "web",
                "--on",
                "stale_for:0030m",
                "--agent",
                "bot",
                "--cooldown",
                "09s",
            ],
            &[],
            vec![ok(r#"{"id":"t-1234567890","status":"proposed"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "trigger --on stale_for alone",
            &["trigger", "list", "--on", "stale_for"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger --on last",
            &["trigger", "list", "--on"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "bash D6: trigger --source last",
            &["trigger", "list", "--source"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "trigger nosuch",
            &["trigger", "nosuch"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want) in [
        (
            &["edit"][..],
            format!("Error: usage: {cli} sources edit <name> [flags]\n"),
        ),
        (
            &["propose", "--x"],
            format!("Error: unknown flag --x ({cli} sources --help)\n"),
        ),
        (
            &["trigger", "show"],
            format!("usage: {cli} sources trigger show <id>\n"),
        ),
        (&["pin"], format!("usage: {cli} sources pin <name>\n")),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
