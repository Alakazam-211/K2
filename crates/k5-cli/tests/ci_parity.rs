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
//!   read, each one plain line (`bad_response` or `io_error`, exit 1),
//!   compared exactly, never a traceback; many runs, and a `status
//!   --json` above 256 KiB.
//! - The signer verbs a rig can drive without a vault: `list`,
//!   `remove`, `finish`, and each verb's usage; a session is refused
//!   before anything is sent. `add` and `reveal` (the vault's confirm) are
//!   `tests/cli/ci_signer_code_stdin.sh`'s and `compute_trust_cli.sh`'s.
//! - Help never acts: each verb that changes something with `-h` or
//!   `--help` sends nothing, on a signer line anywhere before a `--`; a
//!   `--` on a signer line is a usage error and sends nothing.
//! - `evidence` and `provenance` names from the answer: one that is
//!   absolute, has a `..` part, holds a newline or a NUL, or reaches a
//!   link inside `--out` is refused (exit 3, `ci_name_unsafe`) and each
//!   side writes nothing; a second run into the same `--out` replaces the
//!   files.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "ci";

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
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
        case(
            "a 200 that is not JSON",
            &["status"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a run with no sha",
            &["run"],
            &[],
            vec![ok(
                r#"{"run":{"short":"r","state":"passed","trigger":"push"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a monitor with no to",
            &["monitor", "add", "m", "--kind", "slow"],
            &[],
            vec![ok(r#"{"monitor":{"name":"m"}}"#)],
            owner_and_port,
            1,
            1,
        ),
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
        ("signer remove <fp> -h", &["signer", "remove", "FP1", "-h"]),
        ("signer rm <fp> --help", &["signer", "rm", "FP1", "--help"]),
        (
            "signer finish --done -h",
            &["signer", "finish", "p1", "--done", "f.json", "-h"],
        ),
        (
            "signer reveal --reveal --help",
            &["signer", "reveal", "p1", "--reveal", "f.json", "--help"],
        ),
        (
            "signer add ... --help",
            &["signer", "add", "C.F", "--hello", "f.json", "--help"],
        ),
        ("signer list -h", &["signer", "list", "-h"]),
        ("signers --help", &["signers", "--help"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    // No signer verb takes a `--`: a usage error, nothing sent, and a
    // `-h` after it is not help.
    for (name, args) in [
        (
            "signer remove -- <fp>",
            &["signer", "remove", "--", "FP1"][..],
        ),
        (
            "signer remove <fp> -- -h",
            &["signer", "remove", "FP1", "--", "-h"],
        ),
        (
            "signer remove <fp> -- --help",
            &["signer", "remove", "FP1", "--", "--help"],
        ),
        (
            "signer finish --done -- -h",
            &["signer", "finish", "p1", "--done", "f.json", "--", "-h"],
        ),
        (
            "signer reveal --reveal -- --help",
            &[
                "signer", "reveal", "p1", "--reveal", "f.json", "--", "--help",
            ],
        ),
        ("signer list -- -h", &["signer", "list", "--", "-h"]),
        ("signers --", &["signers", "--"]),
        ("signer --", &["signer", "--"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
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
    // The refusal of a name from the answer names no command: the same
    // line in every brand.
    let c = case(
        "brand: a name refused",
        &["evidence", "r9", "--out", "nb", "--no-files"],
        &[],
        vec![ok(r#"{"short":"r9","file":"../x.json","bundle":{}}"#)],
        owner_and_port,
        3,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        (out.code, out.stderr.as_str()),
        (
            Some(3),
            "{\"error\": \"ci_name_unsafe\", \"hint\": \"file name ../x.json has a .. part: refused, nothing written to nb\"}\n"
        )
    );
    assert!(!rig.proj.join("nb").exists() && !rig.proj.join("x.json").exists());
}

/// Every file under `dir`, by its path there.
fn tree(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(list) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in list {
            let p = e.expect("a folder entry").path();
            if p.is_dir() {
                todo.push(p);
            } else {
                let rel = p.strip_prefix(dir).expect("under the folder");
                out.insert(
                    rel.to_string_lossy().into_owned(),
                    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display())),
                );
            }
        }
    }
    out
}

/// The case compared as the rig does, then the files each side wrote
/// under `dir` (the project's): the same paths and bytes, and `want` among
/// them.
fn check_files(rig: &Rig, c: &Case, dir: &str, want: &[&str]) {
    rig.check(c);
    let d = rig.proj.join(dir);
    let mut got = Vec::new();
    for (side, bash) in [(Side::Bash, true), (Side::Rust, false)] {
        let _ = std::fs::remove_dir_all(&d);
        rig.prepare(side, c);
        rig.run(bash, c);
        rig.read(side, !bash, c.name);
        got.push(tree(&d));
    }
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(got[0], got[1], "{}: the files each side wrote", c.name);
    for w in want {
        assert!(
            got[0].contains_key(*w),
            "{}: no {w} in {:?}",
            c.name,
            got[0].keys()
        );
    }
}

const TRUST: &str = r#""trust":{"nodeKeys":{"fp2":{"pem":"-----BEGIN PUBLIC KEY-----\nBBB\n-----END PUBLIC KEY-----\n"},"fp1":{"pem":"-----BEGIN PUBLIC KEY-----\nAAA\n-----END PUBLIC KEY-----\n"}},"statements":[{"body":"{\"node\":\"n1\",\"é\":1}","sig":"s1"}],"owners":[{"rosterKeyFp":"rk1","name":"ann é"},{"rosterKeyFp":"rk2"}],"rosters":[{"nodes":["n1"],"b":2,"a":1}]}"#;

fn provenance_json() -> String {
    format!(
        r#"{{"short":"r1234567","statements":{{"mac":{{"short":"j1234567","files":2,"line":"{{\"payloadType\":\"x\",\"é\":1}}\n"}},"lin":{{"short":"j7654321","files":0,"line":"{{}}\n"}}}},"missing":{{"lint":"no artifacts kept","docs":"never ran"}},{TRUST}}}"#
    )
}

fn evidence_json() -> String {
    format!(
        r#"{{"short":"r9","project":"acme","version":"1.2.3","file":"evidence-r9.json","bundle":{{"z":"é","receipts":{{"mac":{{"job":"j1"}},"lin":{{"job":"j2"}}}},"provenance":{{"mac":{{"payloadType":"application/vnd.in-toto+json","payload":"e30=","signatures":[{{"keyid":"k","sig":"s"}}]}}}}}},"jobs":{{"mac":{{"job":"j1","role":"build","build":"aarch64-apple","short":"j1","node":"n1","artifacts":["a.txt"]}},"lin":{{"job":"j2","role":"test","build":"x86_64-linux","short":"j2","node":"n2","artifacts":[]}}}},"dual":{{"aarch64-apple":{{"build":"mac2","verify":"match","compare":{{"matched":["a.txt"],"differ":["b.txt"],"only_one":[]}}}}}},"notes":["one note é"],{TRUST}}}"#
    )
}

const J2_LIST: &str = r#"{"short":"j2"}"#;
const J1_LIST: &str = r#"{"short":"j1","artifacts":[{"name":"a.txt"}]}"#;
const J1_A: &str = r#"{"name":"a.txt","offset":0,"eof":true,"data":"aGVsbG8=","sha256":"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824","bytes":5}"#;

#[test]
fn ci_provenance_writes_as_the_shell_does() {
    let rig = Rig::new(VERB, "prov");
    check_files(
        &rig,
        &case(
            "provenance",
            &["provenance", "r1234567", "--out", "prov"],
            &[],
            vec![ok(&provenance_json())],
            owner_and_port,
            0,
            1,
        ),
        "prov",
        &[
            "provenance/mac.intoto.jsonl",
            "trust/owners.json",
            "trust/nodes/fp1.pem",
            "trust/rosters/rk1.json",
        ],
    );
    check_files(
        &rig,
        &case(
            "provenance --json, a workspace",
            &[
                "provenance",
                "r1234567",
                "--out",
                "prov/deeper/",
                "--json",
                "--workspace",
                "/w/é",
            ],
            &[],
            vec![ok(&provenance_json())],
            owner_and_port,
            0,
            1,
        ),
        "prov",
        &["deeper/provenance/lin.intoto.jsonl"],
    );
    check_files(
        &rig,
        &case(
            "provenance: nothing to write but owners.json",
            &["provenance", "r1", "--out", "empty"],
            &[],
            vec![ok(r#"{"short":"r1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        "empty",
        &["trust/owners.json"],
    );
    let v = vec![
        case(
            "provenance with no --out",
            &["provenance", "r1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "provenance with no run",
            &["provenance", "--out", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "provenance: no such run",
            &["provenance", "r1", "--out", "nf"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"ci_run_not_found","hint":"no run r1"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "provenance: a statement with no line",
            &["provenance", "r1", "--out", "odd1"],
            &[],
            vec![ok(r#"{"short":"r1","statements":{"mac":{"short":"j"}}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "provenance: a line that is no string",
            &["provenance", "r1", "--out", "odd2"],
            &[],
            vec![ok(r#"{"short":"r1","statements":{"mac":{"line":5}}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "provenance: a roster key that is no string",
            &["provenance", "r1", "--out", "odd3"],
            &[],
            vec![ok(
                r#"{"short":"r1","trust":{"owners":[{"rosterKeyFp":7}],"rosters":[{}]}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "provenance: files that are no number",
            &["provenance", "r1", "--out", "odd4"],
            &[],
            vec![ok(
                r#"{"short":"r1","statements":{"mac":{"short":"j","files":"two","line":"{}\n"}}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Every file under the project but its `.k2`, and whether `dir` is
/// there: what a refused row must leave as it was.
fn project_files(rig: &Rig, dir: &str) -> (std::collections::BTreeMap<String, Vec<u8>>, bool) {
    let mut t = tree(&rig.proj);
    t.retain(|k, _| !k.starts_with(".k2/"));
    (t, rig.proj.join(dir).exists())
}

/// The case compared as the rig does (exit 3), then each side once more:
/// nothing written anywhere in the project, `dir` (the `--out`) made or
/// not as before.
fn check_refused(rig: &Rig, c: &Case, dir: &str) {
    assert_eq!(c.code, 3, "{}: a refused row exits 3", c.name);
    rig.check(c);
    for (side, bash) in [(Side::Bash, true), (Side::Rust, false)] {
        rig.prepare(side, c);
        let before = project_files(rig, dir);
        let out = rig.run(bash, c);
        rig.read(side, !bash, c.name);
        assert_eq!(out.code, Some(3), "{} ({side:?}): {out:?}", c.name);
        assert!(
            out.stderr
                .starts_with("{\"error\": \"ci_name_unsafe\", \"hint\": "),
            "{} ({side:?}): {out:?}",
            c.name
        );
        assert_eq!(
            project_files(rig, dir),
            before,
            "{} ({side:?}): nothing written",
            c.name
        );
    }
}

/// `nl1/trust` a link to `../elsewhere`.
fn linked_folder(r: &Rig) {
    owner_and_port(r);
    let d = r.proj.join("nl1");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(r.proj.join("elsewhere"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::create_dir_all(r.proj.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink("../elsewhere", d.join("trust")).unwrap();
}

/// `nl2/trust/owners.json` a link to `../../target.json`.
fn linked_file(r: &Rig) {
    owner_and_port(r);
    let d = r.proj.join("nl2");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("trust")).unwrap();
    std::fs::write(r.proj.join("target.json"), "keep\n").unwrap();
    std::os::unix::fs::symlink("../../target.json", d.join("trust/owners.json")).unwrap();
}

/// `nr/trust/owners.json` an old plain file, replaced by the run.
fn old_file(r: &Rig) {
    owner_and_port(r);
    let d = r.proj.join("nr");
    std::fs::create_dir_all(d.join("trust")).unwrap();
    std::fs::write(d.join("trust/owners.json"), "old\n").unwrap();
}

#[test]
fn ci_names_from_the_answer_are_checked_before_any_write() {
    let rig = Rig::new(VERB, "names");
    let proj = rig.proj.display().to_string();
    let line = r#"{"short":"j","files":1,"line":"{}\n"}"#;
    let refused = vec![
        (
            case(
                "evidence: an absolute file name",
                &["evidence", "r9", "--out", "na1", "--no-files"],
                &[],
                vec![ok(&format!(
                    r#"{{"short":"r9","file":"{proj}/abs.json","bundle":{{}}}}"#
                ))],
                owner_and_port,
                3,
                1,
            ),
            "na1",
        ),
        (
            case(
                "evidence: a file name with a .. part",
                &["evidence", "r9", "--out", "na2", "--no-files"],
                &[],
                vec![ok(r#"{"short":"r9","file":"../x.json","bundle":{}}"#)],
                owner_and_port,
                3,
                1,
            ),
            "na2",
        ),
        (
            case(
                "provenance: a statement label with a .. part",
                &["provenance", "r1", "--out", "na3"],
                &[],
                vec![ok(&format!(
                    r#"{{"short":"r1","statements":{{"../x":{line}}}}}"#
                ))],
                owner_and_port,
                3,
                1,
            ),
            "na3",
        ),
        (
            case(
                "evidence: a job label a/../../x, before any fetch",
                &["evidence", "r9", "--out", "na4"],
                &[],
                vec![ok(
                    r#"{"short":"r9","file":"e.json","bundle":{"v":3},"jobs":{"a/../../x":{"job":"j1","role":"build","build":"b","short":"j1","node":"n1","artifacts":[]}}}"#,
                )],
                owner_and_port,
                3,
                1,
            ),
            "na4",
        ),
        (
            case(
                "evidence: a receipt label ..",
                &["evidence", "r9", "--out", "na5", "--no-files"],
                &[],
                vec![ok(
                    r#"{"short":"r9","file":"e.json","bundle":{"receipts":{"..":{"job":"j1"}}}}"#,
                )],
                owner_and_port,
                3,
                1,
            ),
            "na5",
        ),
        (
            case(
                "provenance: a label with a newline",
                &["provenance", "r1", "--out", "na6"],
                &[],
                vec![ok(&format!(
                    r#"{{"short":"r1","statements":{{"a\nb":{line}}}}}"#
                ))],
                owner_and_port,
                3,
                1,
            ),
            "na6",
        ),
        (
            case(
                "provenance: a node key with a NUL",
                &["provenance", "r1", "--out", "na7"],
                &[],
                vec![ok(
                    r#"{"short":"r1","trust":{"nodeKeys":{"a\u0000b":{"pem":"P"}}}}"#,
                )],
                owner_and_port,
                3,
                1,
            ),
            "na7",
        ),
        (
            case(
                "provenance: one absolute roster key among good names",
                &["provenance", "r1", "--out", "na8"],
                &[],
                vec![ok(&format!(
                    r#"{{"short":"r1","statements":{{"u1":{line},"u2":{line}}},"trust":{{"nodeKeys":{{"fp1":{{"pem":"P"}}}},"owners":[{{"rosterKeyFp":"rk1"}},{{"rosterKeyFp":"{proj}/roster"}}],"rosters":[{{}},{{}}]}}}}"#
                ))],
                owner_and_port,
                3,
                1,
            ),
            "na8",
        ),
        (
            case(
                "provenance: a linked folder inside --out",
                &["provenance", "r1", "--out", "nl1"],
                &[],
                vec![ok(
                    r#"{"short":"r1","trust":{"nodeKeys":{"fp1":{"pem":"P"}}}}"#,
                )],
                linked_folder,
                3,
                1,
            ),
            "nl1",
        ),
        (
            case(
                "provenance: a linked file inside --out",
                &["provenance", "r1", "--out", "nl2"],
                &[],
                vec![ok(r#"{"short":"r1"}"#)],
                linked_file,
                3,
                1,
            ),
            "nl2",
        ),
    ];
    for (c, dir) in &refused {
        check_refused(&rig, c, dir);
    }
    assert_eq!(
        std::fs::read_to_string(rig.proj.join("target.json")).unwrap(),
        "keep\n",
        "the linked file's target"
    );
    assert_eq!(
        std::fs::read_dir(rig.proj.join("elsewhere"))
            .unwrap()
            .count(),
        0,
        "nothing in the linked folder's target"
    );
    check_files(
        &rig,
        &case(
            "provenance into an --out that has the files: replaced",
            &["provenance", "r1234567", "--out", "nr"],
            &[],
            vec![ok(&provenance_json())],
            old_file,
            0,
            1,
        ),
        "nr",
        &["provenance/mac.intoto.jsonl", "trust/owners.json"],
    );
}

#[test]
fn ci_evidence_writes_and_fetches_as_the_shell_does() {
    let rig = Rig::new(VERB, "evidence");
    check_files(
        &rig,
        &case(
            "evidence: the bundle, the statements, trust and each job's files",
            &["evidence", "r9", "--out", "ev"],
            &[],
            vec![ok(&evidence_json()), ok(J2_LIST), ok(J1_LIST), ok(J1_A)],
            owner_and_port,
            0,
            4,
        ),
        "ev",
        &[
            "evidence-r9.json",
            "provenance/mac.intoto.jsonl",
            "trust/owners.json",
            "mac/a.txt",
        ],
    );
    check_files(
        &rig,
        &case(
            "evidence --reported: the named file in the bundle",
            &["evidence", "r9", "--out", "evr", "--reported", "a.txt"],
            &[],
            vec![ok(&evidence_json()), ok(J2_LIST), ok(J1_LIST), ok(J1_A)],
            owner_and_port,
            0,
            4,
        ),
        "evr",
        &["evidence-r9.json", "mac/a.txt"],
    );
    check_files(
        &rig,
        &case(
            "evidence --json --no-files",
            &["evidence", "r9", "--out", "evj", "--json", "--no-files"],
            &[],
            vec![ok(&evidence_json())],
            owner_and_port,
            0,
            1,
        ),
        "evj",
        &["evidence-r9.json", "trust/owners.json"],
    );
    let v = vec![
        case(
            "evidence with no --out",
            &["evidence", "r9"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "evidence --reported with --no-files",
            &[
                "evidence",
                "r9",
                "--out",
                "x",
                "--no-files",
                "--reported",
                "a",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "evidence --reported: no job wrote it",
            &["evidence", "r9", "--out", "evn", "--reported", "b.txt"],
            &[],
            vec![ok(&evidence_json()), ok(J2_LIST), ok(J1_LIST), ok(J1_A)],
            owner_and_port,
            1,
            4,
        ),
        case(
            "evidence --reported: a path out of the job's files",
            &["evidence", "r9", "--out", "evp", "--reported", "../a.txt"],
            &[],
            vec![ok(&evidence_json()), ok(J2_LIST), ok(J1_LIST), ok(J1_A)],
            owner_and_port,
            1,
            4,
        ),
        case(
            "evidence: a job's files can't be fetched",
            &["evidence", "r9", "--out", "evf"],
            &[],
            vec![
                ok(&evidence_json()),
                ok(J2_LIST),
                Reply::Answer(
                    404,
                    r#"{"error":{"code":"job_not_found","hint":"no job j1"}}"#.into(),
                ),
            ],
            owner_and_port,
            1,
            3,
        ),
        case(
            "evidence: refused",
            &["evidence", "r9", "--out", "evx"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "evidence: a file name that is no string",
            &["evidence", "r9", "--out", "evo1"],
            &[],
            vec![ok(r#"{"short":"r9","file":5,"bundle":{}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "evidence: jobs that are no object",
            &["evidence", "r9", "--out", "evo2"],
            &[],
            vec![ok(
                r#"{"short":"r9","file":"e.json","bundle":{},"jobs":[]}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "evidence: a note that is no string",
            &["evidence", "r9", "--out", "evo3", "--no-files"],
            &[],
            vec![ok(
                r#"{"short":"r9","file":"e.json","bundle":{},"jobs":{},"notes":[5]}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

fn log_page(data: &str) -> String {
    format!(
        r#"{{"job":{{"short":"abcd1234"}},"cursor":10,"chunks":[{{"seq":0,"stream":"out","data":"{data}"}}],"attempt":1,"generation":1,"attempts":[]}}"#
    )
}

const LOG_JOB: &str = r#"{"short":"abcd1234","node":"linux-1","state":"done","argv":["cargo","test"],"exitCode":0,"attempt":1,"generation":1,"result":{"exit":0}}"#;
const LOG_DONE: &str = r#"{"job":{"short":"abcd1234"},"cursor":10,"chunks":[],"attempt":1,"generation":1,"attempts":[]}"#;
const LOG_TESTED: &str = r#"{"short":"abcd1234","state":"done","result":{"tests":{"failed":1,"errors":0,"passed":3,"skipped":0,"total":4,"failing":[{"name":"b","classname":"suite","message":"boom"}]}}}"#;

#[test]
fn ci_logs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "logs");
    let many: Vec<String> = (0..4000)
        .map(|i| {
            format!(
                r#"{{"name":"check-{i:05}-é","state":"queued","description":"waits for a node"}}"#
            )
        })
        .collect();
    let big = format!(
        r#"{{"run":{{"short":"r1","checks":[{}]}}}}"#,
        many.join(",")
    );
    let v = vec![
        case(
            "logs: each check's heading and log",
            &["logs", "r1234567"],
            &[],
            vec![
                ok(&run_json("failure")),
                ok(LOG_JOB),
                ok(&log_page("aGVsbG8K")),
                ok(LOG_DONE),
                ok(LOG_JOB),
                ok(&log_page("d29ybGQK")),
                ok(LOG_DONE),
            ],
            owner_and_port,
            0,
            7,
        ),
        case(
            "log --check docs: never submitted",
            &["log", "r1234567", "--check", "docs", "--workspace", "/w"],
            &[],
            vec![ok(&run_json("failure"))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "logs --failures --check lint",
            &["logs", "main", "--failures", "--check", "lint"],
            &[],
            vec![
                ok(&run_json("failure")),
                ok(LOG_TESTED),
                ok(LOG_TESTED),
                ok(&log_page("aGVsbG8K")),
                ok(LOG_DONE),
            ],
            owner_and_port,
            0,
            5,
        ),
        case(
            "logs: a job's log refused, the walk goes on",
            &["logs", "r1", "--check", "test"],
            &[],
            vec![
                ok(&run_json("failure")),
                Reply::Answer(
                    404,
                    r#"{"error":{"code":"not_found","hint":"no job"}}"#.into(),
                ),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "logs: no such check",
            &["logs", "r1", "--check", "nope"],
            &[],
            vec![ok(&run_json("failure"))],
            owner_and_port,
            4,
            1,
        ),
        case(
            "logs with no run",
            &["logs"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "logs: 4000 checks never submitted (above 256 KiB)",
            &["logs", "r1"],
            &[],
            vec![ok(&big)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "logs: no answer",
            &["logs", "r1"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "logs: a run that is no object",
            &["logs", "r1"],
            &[],
            vec![ok(r#"{"run":[1]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "logs: a job that can't be cut",
            &["logs", "r1"],
            &[],
            vec![ok(
                r#"{"run":{"checks":[{"name":"a","state":"x","job":7}]}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The help: `ci -h` and `ci --help` are the front's pages, `ci help` is
/// the family's; any other line with `-h` or `--help` the front hands to
/// the shell. Each with no daemon set up and behind the guard too.
#[test]
fn ci_help_forms() {
    let rig = Rig::new(VERB, "help");
    let mut v = Vec::new();
    for (name, args) in [
        ("-h", &["-h"][..]),
        ("--help", &["--help"]),
        ("help", &["help"]),
        ("evidence -h", &["evidence", "r9", "--out", "d", "-h"]),
        ("provenance --help", &["provenance", "--help"]),
        ("logs -h", &["logs", "r1", "-h"]),
        (
            "status --help after a flag",
            &["status", "--json", "--help"],
        ),
        ("an unknown verb with -h", &["bogus", "-h"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    for (name, args) in [
        ("-h with no daemon set up", &["-h"][..]),
        ("help with no daemon set up", &["help"]),
        ("logs -h with no daemon set up", &["logs", "r1", "-h"]),
    ] {
        v.push(case(name, args, &[], vec![], nothing, 0, 0));
    }
    v.push(case(
        "-h behind the guard",
        &["status", "-h"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        0,
        0,
    ));
    v.push(case(
        "help behind the guard: refused",
        &["help"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    v.push(case(
        "-h after -- is a word of the check: the gate",
        &["init", "--", "make", "-h"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
    v.push(case(
        "-h in a session with no passport",
        &["-h"],
        &[("K2_HOOK_SOCK", "{sock}")],
        vec![],
        owner_and_port,
        3,
        0,
    ));
    // `signer remove <fp> -h`: the help, nothing sent (the shell's fix,
    // 96ab12123f; it used to remove the signer). The front hands a `-h`
    // line that is no help page to the shell, so both sides are the
    // shell's.
    v.push(Case {
        compare: Compare::Shell,
        ..case(
            "signer remove <fp> -h: the help, nothing sent",
            &["signer", "remove", "fp1", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )
    });
    for c in v {
        rig.check(&c);
    }
}
