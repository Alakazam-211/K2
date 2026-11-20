//! `k5-cli ci` against the shell CLI's `cli/k2 ci`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - The bare line (`ci`: the help; `ci --json`: the unknown verb), the
//!   gates, and `ci`'s `compute` tool in a session with no passport.
//! - The words: each value flag with no value, a bad number or duration,
//!   the flags that go with one verb only, an unknown flag, each verb's
//!   own checks.
//! - Each native verb with `--json` and without, with every line its
//!   reader prints; `run --wait` and `release --wait` (one more status
//!   call, `CI_WAIT_SECS=0`); refusals (`code: hint`, a string error, not
//!   JSON, 400, 404, 413) and no answer; odd answers its Python can't
//!   read; many runs, and a `status --json` above 256 KiB.
//! - The signer verbs a rig can drive without a vault: `list`,
//!   `remove`, `finish`, and each verb's usage; a session is refused
//!   before anything is sent. `add` and `reveal` (the vault's confirm) are
//!   `tests/cli/ci_signer_code_stdin.sh`'s and `compute_trust_cli.sh`'s.
//! - Help never acts: each verb that changes something with `-h` or
//!   `--help` sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "ci";

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// A run with every line its block prints.
fn run_json(state: &str) -> String {
    format!(
        r#"{{"run":{{"id":"r1234567890","short":"r1234567","state":"{state}","sha":"0123456789abcdef0123","ref":"main","trigger":"push","tier":"ci","createdAt":1000000000,"by":"ann é","detail":"two checks",
        "statuses":{{"slug":"acme/app","context":"k2","tokenSecret":"GH_TOKEN"}},
        "checks":[{{"name":"test","state":"failure","job":"abcdef123456","status":"failure","description":"1 failed","statusError":"401"}},{{"name":"lint","state":"success","job":"aa"}},{{"name":"docs","state":"queued"}}]}}}}"#
    )
}

fn release_json() -> String {
    r#"{"run":{"id":"r9","short":"r9","state":"passed","sha":"feedface","kind":"release","release":{"version":"1.2.3","project":"acme"},"trigger":"release","checks":[{"name":"mac","role":"build","build":"aarch64-apple","state":"success","job":"j123456789","description":"ok"},{"name":"lin","state":"running"}]}}"#.to_string()
}

fn status_json(n: usize) -> String {
    let runs: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"short":"r{i:07}","state":"passed","sha":"{:040x}","ref":"refs/heads/feature-{i}-é","trigger":"push","createdAt":{},"checks":[{{"name":"test","state":"success"}},{{"name":"lint","state":"success"}}]}}"#,
                i,
                1_000_000_000 + i
            )
        })
        .collect();
    format!(
        r#"{{"workspace":"/w/acme","watch":{{"on":true,"remote":"origin","branches":["main","dev"],"pollSecs":60,"tier":"standard","maxInflight":2,"lastPollAt":0,"lastError":"auth failed"}},"freeze":{{"until":1795132800,"why":"release"}},"runs":[{}]}}"#,
        runs.join(",")
    )
}

#[test]
fn ci_bare_help_and_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = vec![
        // covers: ci
        case("ci alone: the help", &[], &[], vec![], owner_and_port, 0, 0),
        // covers: ci --json
        case(
            "ci --json: an unknown verb",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "ci alone with no daemon set up: no gate for the help",
            &[],
            &[],
            vec![],
            nothing,
            0,
            0,
        ),
        case(
            "ci --json with no daemon set up: the gate",
            &["--json"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "ci alone behind the guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            port_only,
            3,
            0,
        ),
        case(
            "ci alone in a session with no passport",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for (name, args) in [
        ("--workspace with no value", &["status", "--workspace"][..]),
        ("--days with no value", &["stats", "--days"]),
        ("--kind with no value", &["monitor", "add", "m", "--kind"]),
        ("--mem not a number", &["init", "--mem", "x"]),
        (
            "--count not a number",
            &["monitor", "add", "m", "--count", "x"],
        ),
        ("--timeout a bad duration", &["init", "--timeout", "1x"]),
        ("--poll a bad duration", &["watch", "on", "--poll", "h"]),
        ("two test filters", &["tests", "--flaky", "--slow"]),
        ("--flaky on status", &["status", "--flaky"]),
        ("--days on run", &["run", "--days", "3d"]),
        ("--branch on run", &["run", "--branch", "main"]),
        ("--kind on status", &["status", "--kind", "slow"]),
        ("--tier other", &["status", "--tier", "gold"]),
        ("--add on status", &["status", "--add", "main"]),
        ("cache --set with no branch", &["cache", "trust", "--set"]),
        ("an unknown flag", &["status", "--bogus"]),
        ("release with no version", &["release"]),
        (
            "release <rev> and --sha",
            &["release", "1.0", "abc", "--sha", "def"],
        ),
        ("stats with a word", &["stats", "x"]),
        ("monitors with a word", &["monitors", "x"]),
        ("monitor with no action", &["monitor"]),
        ("monitor add with no name", &["monitor", "add"]),
        ("monitor add with no kind", &["monitor", "add", "m"]),
        ("monitor mute with no --for", &["monitor", "mute", "m"]),
        ("tests with a word", &["tests", "x"]),
        ("setup with no --plan", &["setup", "--out", "f"]),
        ("watch maybe", &["watch", "maybe"]),
        ("init -- with no command", &["init", "--"]),
        ("freeze with no --until", &["freeze"]),
        ("freeze --until soon", &["freeze", "--until", "soon"]),
        (
            "freeze --until with no zone",
            &["freeze", "--until", "2026-11-20T00:00:00"],
        ),
        ("cache without trust", &["cache"]),
        ("signer, a verb it doesn't have", &["signer", "show"]),
        ("signer remove with no fingerprint", &["signer", "remove"]),
        (
            "signer add with no --hello",
            &["signer", "add", "--code-stdin"],
        ),
        (
            "signer add, both forms",
            &["signer", "add", "C.F", "--code-stdin"],
        ),
        ("signer add, an unknown flag", &["signer", "add", "--x"]),
        ("signer reveal with no file", &["signer", "reveal", "p1"]),
        (
            "signer finish with no file",
            &["signer", "finish", "p1", "--done", "/nonexistent"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn ci_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "verbs");
    let monitor = r#"{"monitor":{"name":"red","kind":"fail-streak","watches":"test on main","to":{"kind":"thread","target":"k2"},"cooldownSecs":21600,"mutedUntil":0}}"#;
    let v = vec![
        case(
            "status: the watch, a freeze, runs",
            &["status", "--limit", "5"],
            &[],
            vec![ok(&status_json(3))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status, many runs",
            &["status"],
            &[],
            vec![ok(&status_json(500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status --json above 256 KiB",
            &["status", "--json"],
            &[],
            vec![ok(&status_json(1500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status: no runs, watch off",
            &["status", "--workspace", "/w/é x"],
            &[],
            vec![ok(
                r#"{"workspace":"/w","watch":{"on":false,"remote":"o","branches":["main"]},"runs":[]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status <run>",
            &["status", "r1234567"],
            &[],
            vec![ok(&run_json("failure"))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status of a release run",
            &["status", "r9"],
            &[],
            vec![ok(&release_json())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "run",
            &[
                "run",
                "HEAD",
                "--check",
                "test, lint",
                "--ref",
                "main",
                "--tier",
                "ci",
            ],
            &[],
            vec![ok(&run_json("running"))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "run --json",
            &["run", "--json"],
            &[],
            vec![ok(&run_json("passed"))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "run --wait: running, then failed",
            &["run", "--wait"],
            &[("K2_CI_WAIT_SECS", "0")],
            vec![ok(&run_json("running")), ok(&run_json("failure"))],
            owner_and_port,
            1,
            2,
        ),
        case(
            "run --wait: skipped is fine",
            &["run", "--wait", "--json"],
            &[("K2_CI_WAIT_SECS", "0")],
            vec![ok(&run_json("skipped"))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "release --plan",
            &[
                "release",
                "1.2.3",
                "--sha",
                "abc",
                "--build",
                "mac,lin",
                "--plan",
                "--only-keys",
                "k1",
            ],
            &[],
            vec![ok(
                r#"{"version":"1.2.3","project":"acme","sha":"0123456789abcdef","owner":"ann","seat":"s1","lane":"l1","placed":[{"label":"a","role":"build","build":"mac","node":"n1","why":"fits"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "release",
            &["release", "1.2.3", "HEAD"],
            &[],
            vec![ok(&release_json())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stats",
            &[
                "stats", "--check", "test", "--branch", "main", "--days", "14d",
            ],
            &[],
            vec![ok(
                r#"{"workspace":"/w","days":14,"check":"test","branch":"main","checks":[{"check":"test-with-a-long-name","runs":12,"failed":2,"p50Secs":45,"p95Secs":3700,"p50WaitSecs":5,"p95WaitSecs":125,"failureRate":0.125,"flakyRate":0.5,"cpuHours":1.234,"cacheHitRate":null,"errors":1},{"check":"lint","runs":1,"failed":0}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stats, none",
            &["stats"],
            &[],
            vec![ok(r#"{"workspace":"/w","days":14,"checks":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "monitors",
            &["monitors"],
            &[],
            vec![ok(
                r#"{"workspace":"/w","monitors":[{"name":"red","kind":"fail-streak","watches":"test","to":{"kind":"workspace","target":"/w"},"cooldownSecs":5400,"fired":3,"mutedUntil":0}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "monitor add",
            &[
                "monitor",
                "add",
                "red",
                "--kind",
                "fail-streak",
                "--count",
                "03",
                "--branch",
                "main",
                "--to",
                "thread:k2",
            ],
            &[],
            vec![ok(monitor)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "monitor remove",
            &["monitor", "remove", "red"],
            &[],
            vec![ok(r#"{"removed":"red"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tests",
            &["tests", "--flaky", "--days", "30", "--limit", "5"],
            &[],
            vec![ok(
                r#"{"workspace":"/w","days":30,"check":"test","tests":[{"why":"flaky","check":"test","test":"a::b \"q\" é","windowCount":2,"windowJobs":10,"lastFlakeAt":1000000000,"lastFlakeTree":"0123456789abcdef"},{"why":"failing","check":"t","test":"c","windowCount":1,"windowJobs":3,"lastFailAt":0},{"why":"slow","check":"t","test":"d","ewmaMs":1234.5,"rose":true},{"why":"other","check":"t","test":"e"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "watch alone",
            &["watch"],
            &[],
            vec![ok(r#"{"watch":{"on":true,"branches":["main"]}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "watch alone --json, off",
            &["watch", "--json"],
            &[],
            vec![ok(r#"{"watch":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "watch on",
            &[
                "watch",
                "on",
                "--branches",
                "main,dev",
                "--poll",
                "1m30s",
                "--max-inflight",
                "2",
                "--skip-current",
                "--remote",
                "origin",
            ],
            &[],
            vec![ok(
                r#"{"watch":{"on":true,"remote":"origin","branches":["main"],"pollSecs":90,"maxInflight":2},"tips":[{"branch":"main","sha":"0123456789"}],"skippedCurrent":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "watch off",
            &["watch", "off"],
            &[],
            vec![ok(r#"{"changed":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "init",
            &[
                "init",
                "--check",
                "test",
                "--needs",
                "rust@1, node",
                "--mem",
                "8g",
                "--cpus",
                "4",
                "--timeout",
                "1h",
                "--force",
                "--",
                "cargo",
                "test",
            ],
            &[],
            vec![ok(
                r#"{"path":"/w/.k2/ci.toml","config":"[checks.test]\nargv = [\"cargo\"]\n","ignored":true,"placeholder":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "freeze --until an epoch",
            &["freeze", "--until", "1795132800", "--why", "ship it"],
            &[],
            vec![ok(r#"{"freeze":{"until":1795132800,"why":"ship it"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "freeze --until a time with a zone",
            &["freeze", "--until", "2026-11-20T01:00:00+01:00"],
            &[],
            vec![ok(r#"{"freeze":{"until":1795132800}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "freeze off",
            &["freeze", "off"],
            &[],
            vec![ok(r#"{"freeze":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "cache trust",
            &["cache", "trust"],
            &[],
            vec![ok(
                r#"{"workspace":"/w","branches":["main"],"default":true,"note":"n"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "cache trust --set",
            &["cache", "trust", "--set", "main, dev", "--add", "x"],
            &[],
            vec![ok(
                r#"{"workspace":"/w","branches":[],"by":"ann","remote":"origin"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "setup --plan: still missing",
            &["setup", "--plan", "--out", "/nonexistent/setup.json"],
            &[],
            vec![ok(
                r#"{"project":"acme","sha":"0123456789abcdef","controllerFp":"0123456789abcdef0123","owner":"ann","lane":"l","missing":["a roster"]}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "signers",
            &["signers"],
            &[],
            vec![ok(
                r#"{"controllerFp":"ab12","signers":[{"signerFp":"cd34","label":"mac é","pairedBy":"ann"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "signer, none",
            &["signer"],
            &[],
            vec![ok(r#"{"controllerFp":"ab12","signers":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "signer remove",
            &["signer", "rm", "cd34"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "signer list refused",
            &["signer", "ls"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
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
fn ci_refusals_and_odd_answers() {
    let rig = Rig::new(VERB, "refused");
    let v = vec![
        case(
            "403 with Ask with:",
            &["status"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "400, a string error",
            &["run"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"bad_rev","message":"no such rev"}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "404",
            &["status", "nosuch"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"ci_run_not_found"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "413, not JSON",
            &["init"],
            &[],
            vec![Reply::Answer(413, "too big".into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "500, a list",
            &["stats"],
            &[],
            vec![Reply::Answer(500, "[1,2]".into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "no answer",
            &["status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a 200 that is not JSON",
            &["status"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a run with no sha",
            &["run"],
            &[],
            vec![ok(
                r#"{"run":{"short":"r","state":"passed","trigger":"push"}}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a monitor with no to",
            &["monitor", "add", "m", "--kind", "slow"],
            &[],
            vec![ok(r#"{"monitor":{"name":"m"}}"#)],
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
fn ci_in_a_session() {
    let rig = Rig::new(VERB, "pass");
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a passport over TCP: no workspace named",
                &["status"],
                &[],
                vec![ok(r#"{"workspace":"/w","runs":[]}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "signer remove is a person's",
                &["signer", "remove", "cd34"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "signer finish is a person's, after its usage",
                &["signer", "finish", "p1"],
                &[],
                vec![],
                port_only,
                2,
                0,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts.
#[test]
fn ci_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    let mut v = Vec::new();
    for (name, args) in [
        ("run -h", &["run", "-h"][..]),
        ("release --help", &["release", "1.0", "--help"]),
        (
            "monitor add -h",
            &["monitor", "add", "m", "--kind", "slow", "-h"],
        ),
        ("watch on --help", &["watch", "on", "--help"]),
        ("init -h", &["init", "-h", "--", "make"]),
        ("freeze -h", &["freeze", "--until", "3d", "-h"]),
        (
            "cache trust --set --help",
            &["cache", "trust", "--set", "main", "--help"],
        ),
        ("setup -h", &["setup", "--plan", "--out", "f", "-h"]),
        ("signer add -h", &["signer", "-h"]),
        ("signer remove --help", &["signer", "--help", "remove", "x"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name.
#[test]
fn ci_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case("brand", &["--json"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("usage: unknown {cli} ci verb '--json' ({cli} ci --help)\n")
    );
    assert!(log.is_empty());
    let c = case(
        "brand: signer's usage",
        &["signer", "show"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("usage: {cli} ci signer [list|add|reveal|finish|remove] ({cli} ci --help)\n")
    );
}
