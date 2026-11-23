//! `k5-cli compute run|test|prewarm` (and `compute` alone) against the
//! shell CLI's `cli/k2`, on the parity rig ([`common::parity`]): the same
//! requests, stdout, stderr and exit code.
//!
//! - `run`: `--detach` (the id, or with `--json` the job's result), the
//!   job's lines (notes, notices, the source, the placement, a tier),
//!   attached (the job's output and its exit), `--wait` (the id, then the
//!   output), `--wait --quiet` and `--wait --json` (the wait), a split run
//!   attached and with `--wait`, `--plan` (its lines and the answer), a
//!   long submit line (each flag in the body), refusals, a 400, no answer
//!   (3 sends), a session on the cell socket (and its POST unanswered),
//!   Python's raise, and every usage line the words can stop at.
//! - `test`: the nextest script in the body (the default config, a
//!   profile, a runner, shards), its usage lines, a runner shlex refuses.
//! - `prewarm`: the lines per node, `--json`, `--wait` (and with
//!   `--json`), a package request held for the owner (exit 2), one answer
//!   over 256 KiB, refusals, the usage lines.
//! - Help never acts: `-h` or `--help` anywhere prints help, sends
//!   nothing.
//! - `compute` alone and `compute help`: the help page.
//!
//! A submit's body has a new `clientJobId` (Python's `uuid4().hex`) on each
//! side: those rows compare it masked ([`submitted`]), each side's checked
//! to be 32 hex digits and the same on every send of one run.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";

const SUBMIT: &str = r#"{"job":{"id":"abcd1234-0000-4000-8000-000000000001","short":"abcd1234","node":"mini","state":"queued","srcCommit":"0123456789abcdef","tier":"priority","tierAsked":"normal"},"queue":{"position":1,"reason":"busy"},"warnings":["hint: name no node","slow disk"],"notices":[{"message":"courtesy note"},{"code":"tier_capped"}],"replaced":["0123456789abcdef",42],"source":{"checkout":"worktree","repo":"r","path":"sub","commit":"fedcba9876543210","dirty":{"changed":2,"untracked":1}},"placement":{"mode":"auto","node":"mini","why":"most free","fits":"fits 4 CPUs"}}"#;
const PLAIN: &str = r#"{"job":{"id":"abcd1234-0000-4000-8000-000000000001","short":"abcd1234","node":"mini","state":"running"}}"#;
const SET: &str = r#"{"job":{"id":"abcd1234-0000-4000-8000-000000000001","short":"abcd1234","state":"queued"},"shards":[{"index":1,"node":"a"},{"index":2}],"source":{"repo":"r","commit":"0123"}}"#;
const RESULT: &str = r#"{"id":"abcd1234-0000-4000-8000-000000000001","short":"abcd1234","state":"done","result":{"exit":0,"tests":{"passed":3,"failed":0,"errors":0}}}"#;
const WAIT_DONE: &str = r#"{"id":"abcd1234-0000-4000-8000-000000000001","short":"abcd1234","state":"done","node":"mini","exitCode":7,"startedAt":1000,"endedAt":1125}"#;
const RED: &str = "G1szMW1yZWQbWzBtIGxpbmUNCg==";
const WARN: &str = "d2FybgcgdHdvCg==";

const PLAN: &str = r#"{"node":"mini","startsNow":false,"queue":{"ahead":2,"blockedBy":{"code":"busy"}},"online":false,"warmSlot":"slot-a 12G","estimate":{"medianSecs":125,"p90Secs":300.5,"samples":7,"basis":"same command"},"warnings":["w1"],"notices":[{"code":"n1"},{"message":"m2"}],"placement":{"mode":"auto","node":"mini","why":"most free","fits":"4 CPUs"},"source":{"checkout":null,"repo":"r","commit":"0123456789abcdef","dirty":{"changed":1,"untracked":0}},"toolsLine":"rust 1.99, cmake 3","limits":{"stallSecs":600,"queueTimeoutSecs":7200,"timeoutSecs":90},"gpu":{"gpus":[{"model":"Radeon"},{"pci":"0000:03"}],"free":1,"mem":{"budgetBytes":4294967296,"watched":false,"countedBytes":1}},"network":{"line":"none","dummies":["d0"]},"effective":{"source":".k2/compute.toml","preset":"ci","os":{"value":"linux","from":"preset"},"mem":{"value":8,"from":"auto (3 runs)"},"needs":{"value":["a","b"],"from":"defaults"},"cpus":{"value":4,"from":"flag"},"tier":{"value":"normal"},"env":{"cpuHints":{"MAKEFLAGS":"-j4","N":4}}},"ranking":[{"node":"mini","state":"idle","etaSecs":0,"etaBasis":"now"},{"node":"big","state":"busy","etaSecs":3600,"etaBasis":"queue"},{"node":"old","state":"offline"}]}"#;
const PLAN_USERNS: &str = r#"{"node":"n","startsNow":true,"online":true,"placement":{"node":"n","why":"only one"},"source":{"checkout":"workspace","commit":null},"network":{"line":"none"},"userns":true,"effective":{"os":{"value":"linux","from":"defaults"}},"ranking":[{"node":"n","state":"idle"}]}"#;
const PLAN_NOW: &str = r#"{"node":"n","startsNow":true,"online":true,"placement":{"node":"n","why":"only one"},"source":{"checkout":"workspace","commit":null},"gpu":{"gpus":[],"mem":{"budgetBytes":null}},"effective":{"os":{"value":"macos","from":"defaults"}},"ranking":[{"node":"n","state":"idle"}]}"#;

fn logs(chunks: &str, end: bool, job: &str) -> Reply {
    ok(&format!(
        r#"{{"cursor":"c:{n}","end":{end},"generation":1,"chunks":[{chunks}],"job":{job},"queue":{{}}}}"#,
        n = chunks.len()
    ))
}

fn done(code: &str) -> String {
    format!(
        r#"{{"state":"done","exitCode":{code},"signal":null,"short":"abcd1234","node":"mini","startedAt":1000,"endedAt":1125,"generation":1,"attempt":1}}"#
    )
}

fn out_err() -> String {
    format!(
        r#"{{"seq":1,"stream":"out","data":"{RED}"}},{{"seq":2,"stream":"err","data":"{WARN}"}}"#
    )
}

fn sess(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: port_only,
        ..c
    }
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

/// `"clientJobId": "<32 hex>"` as `<id>`; each one found, in order.
fn mask_client(text: &str, name: &str) -> (String, Vec<String>) {
    const KEY: &str = "\"clientJobId\": \"";
    let mut out = String::new();
    let mut ids = vec![];
    let mut rest = text;
    while let Some(at) = rest.find(KEY) {
        let start = at + KEY.len();
        out.push_str(&rest[..start]);
        let id: String = rest[start..].chars().take(32).collect();
        assert!(
            id.len() == 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "{name}: a clientJobId is 32 hex digits: {text}"
        );
        out.push_str("<id>");
        ids.push(id);
        rest = &rest[start + 32..];
    }
    out.push_str(rest);
    (out, ids)
}

/// One side's requests: its port as `{port}`, the clientJobId masked; the
/// ids it sent are all one.
fn masked_log(log: Vec<String>, port: u16, name: &str) -> Vec<String> {
    let mut seen: Vec<String> = vec![];
    let out = log
        .into_iter()
        .map(|l| {
            let (t, ids) = mask_client(&l.replace(&port.to_string(), PORT), name);
            seen.extend(ids);
            t
        })
        .collect();
    seen.dedup();
    assert!(seen.len() <= 1, "{name}: one clientJobId per run: {seen:?}");
    out
}

/// Both sides of a case with a submit in it, as the rig's check, its
/// `clientJobId` masked: bash, then the binary with and without
/// `python3`.
fn submitted(rig: &Rig, c: &Case) {
    let name = c.name;
    rig.prepare(Side::Bash, c);
    let b = rig.run(true, c);
    assert_eq!(
        rig.stub_of(Side::Bash).left(),
        0,
        "{name}: bash left answers: {b:?}"
    );
    let bl: Vec<String> = rig
        .read(Side::Bash, false, name)
        .iter()
        .map(|r| normalise(r, false, VERB, name))
        .collect();
    let bl = masked_log(bl, rig.stub_of(Side::Bash).port, name);
    println!("== {name}\n{}\n-- bash: {b:?}", bl.join("\n----\n"));
    assert_eq!(b.code, Some(c.code), "{name}: bash's exit: {b:?}");
    assert_eq!(bl.len(), c.requests, "{name}: bash's requests: {bl:#?}");
    for without in [false, true] {
        let side = if without { Side::NoPython3 } else { Side::Rust };
        let (r, rl) = rig.binary(c, without);
        println!("-- {side:?}: {r:?}");
        assert_eq!(rig.stub_of(side).left(), 0, "{name}: left answers: {r:?}");
        let port = rig.stub_of(side).port;
        let rl = masked_log(
            rl.iter()
                .map(|l| l.replace(PORT, &port.to_string()))
                .collect(),
            port,
            name,
        );
        assert_eq!(
            rl,
            bl,
            "{name} ({side:?}): the requests\n== rust ==\n{}\n== bash ==\n{}",
            rl.join("\n----\n"),
            bl.join("\n----\n")
        );
        assert_eq!(
            (r.code, r.signal),
            (b.code, b.signal),
            "{name} ({side:?}): the exit\n{r:?}\n{b:?}"
        );
        assert_eq!(r.stdout, b.stdout, "{name} ({side:?}): stdout");
        match c.compare {
            Compare::Traceback => {
                const HEAD: &str = "Traceback (most recent call last):\n";
                let split = |text: &str| -> (String, String) {
                    let (before, tb) = text
                        .split_once(HEAD)
                        .unwrap_or_else(|| panic!("{name}: no traceback: {text:?}"));
                    let last = tb.lines().last().unwrap_or_default();
                    (
                        before.to_string(),
                        last.split(':').next().unwrap_or_default().to_string(),
                    )
                };
                assert_eq!(
                    split(&r.stderr),
                    split(&b.stderr),
                    "{name} ({side:?}): stderr"
                );
            }
            Compare::Waited => {
                let (bs, bn) = mask_waited(&b.stderr);
                let (rs, rn) = mask_waited(&r.stderr);
                assert_eq!(rs, bs, "{name} ({side:?}): stderr (seconds masked)");
                assert!(
                    rn.len() == bn.len() && rn.iter().zip(&bn).all(|(x, y)| x.abs_diff(*y) <= 1),
                    "{name} ({side:?}): the waited lines"
                );
            }
            _ => assert_eq!(r.stderr, b.stderr, "{name} ({side:?}): stderr"),
        }
    }
}

#[test]
fn compute_run_detached_and_planned() {
    let rig = Rig::new(VERB, "rund");
    let v = vec![
        // covers: compute run
        case(
            "--detach: the id and the job's lines",
            &["run", "--detach", "--", "echo", "hi"],
            &[],
            vec![ok(SUBMIT)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute run --json
        case(
            "--detach --json: the job's result",
            &["run", "mini", "--detach", "--json", "--", "make"],
            &[],
            vec![ok(PLAIN), ok(RESULT)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "--tier batch is detached",
            &["run", "--tier", "batch", "--", "sleep", "1"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a long submit line",
            &[
                "run",
                "mini",
                "--os",
                "linux",
                "--distro",
                "ubuntu@24.04",
                "--arch",
                "x86_64",
                "--avoid",
                "n9",
                "--label",
                "k=v",
                "--label",
                "k=w",
                "--env",
                "A=1=2",
                "--needs",
                "rust@stable, cmake@3",
                "--needs",
                "bun",
                "--artifact",
                "out/*",
                "--secret",
                "TOKEN=GH",
                "--secret",
                "PLAIN",
                "--queue-timeout",
                "1h30m",
                "--retries",
                "02",
                "--stall-timeout",
                "off",
                "--dirty",
                "--sha",
                "abc",
                "--cpus",
                "2.50+8",
                "--mem",
                "auto:8G",
                "--timeout",
                "90m",
                "--exclusive",
                "--why",
                "bench",
                "--retry-interrupted",
                "--workspace",
                "acme",
                "--tier",
                "standard",
                "--supersede",
                "nightly",
                "--submitter",
                "ci-bot",
                "--repo",
                "lib",
                "--cache",
                "target",
                "--with",
                "a@b@c",
                "--with",
                "plain",
                "--preset",
                "ci",
                "--now",
                "--cache-required",
                "--strict",
                "--tag",
                "fuzz:p3",
                "--same-node-as",
                "j1",
                "--receipt-v2",
                "--gpu=amd",
                "--gpu-mem",
                "4G",
                "--netns-dummy",
                "d0=10.254.254.1/32",
                "--detach",
                "--",
                "sh",
                "-c",
                "echo \"hé\"",
            ],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--gate, --no-retry, --network none, --mem 512M, --cpus 4",
            &[
                "run",
                "--any",
                "--gate",
                "--no-retry",
                "--network",
                "none",
                "--mem",
                "512M",
                "--cpus",
                "4",
                "--no-source",
                "--gpu",
                "--detach",
                "--",
                "true",
            ],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--plan: the lines and the answer",
            &["run", "--plan", "--", "make"],
            &[],
            vec![ok(PLAN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--dry-run: starts now, pinned, no GPU",
            &["run", "n", "--dry-run", "--json", "--", "make"],
            &[],
            vec![ok(PLAN_NOW)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refused",
            &["run", "--detach", "--", "x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "--strict: a sh -c script, strict in the body",
            &["run", "--no-source", "--strict", "--detach", "--", "sh", "-c", "cargo test | tee log"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--userns (0.46.18): userns in the body",
            &["run", "--no-source", "--userns", "--detach", "--", "unshare", "-U", "true"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--userns --plan: the userns line",
            &["run", "--userns", "--network", "none", "--plan", "--", "podman", "run", "alpine"],
            &[],
            vec![ok(PLAN_USERNS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--userns on a tenant node: userns_owner_only, exit 3",
            &["run", "mini", "--userns", "--detach", "--", "x"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"userns_owner_only","hint":"--userns is only for the node owner's own server: this server is a tenant of mini, so its jobs can't make user namespaces there","node":"mini"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "--userns without the grant: userns_not_granted, exit 3",
            &["run", "--userns", "--detach", "--", "x"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"userns_not_granted","hint":"--userns: workspace acme isn't granted user namespaces on 'mini'","node":"mini"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "--strict on a command that is no sh -c: the server's bad_request",
            &["run", "--strict", "--detach", "--", "make", "-j"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_request","hint":"--strict runs a sh -c or bash -c script: -- sh -c '...'"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a bad request",
            &["run", "--detach", "--", "x"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_netns","hint":"private ranges only"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error the route floor words",
            &["run", "--detach", "--", "x"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":"not_found","message":"no such node"}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "no answer: sent three times",
            &["run", "--detach", "--", "x"],
            &[],
            vec![Reply::Close, Reply::Close, Reply::Close],
            owner_and_port,
            3,
            3,
        ),
        case(
            "no answer once, then the job",
            &["run", "--detach", "--", "x"],
            &[],
            vec![Reply::Close, ok(PLAIN)],
            owner_and_port,
            0,
            2,
        ),
        sess(case(
            "in a session, on the cell socket",
            &["run", "--detach", "--", "x"],
            &[],
            vec![ok(SUBMIT)],
            port_only,
            0,
            1,
        )),
        waited(sess(case(
            "in a session, the cell socket took it and never answered",
            &["run", "--detach", "--", "x"],
            &[],
            vec![Reply::Close],
            port_only,
            1,
            1,
        ))),
        case(
            "the job's reader raises",
            &["run", "--detach", "--", "x"],
            &[],
            vec![ok(r#"{"job":["not","a","job"]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an id that is no string",
            &["run", "--detach", "--", "x"],
            &[],
            vec![ok(
                r#"{"job":{"id":7,"short":"7","node":"n","state":"queued"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "the plan's reader raises",
            &["run", "--plan", "--", "x"],
            &[],
            vec![ok(
                r#"{"node":"n","startsNow":true,"online":true,"estimate":{"medianSecs":"x"}}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        submitted(&rig, &c);
    }
}

#[test]
fn compute_run_followed() {
    let rig = Rig::new(VERB, "runf");
    let set_done = r#"{"state":"done","short":"abcd1234","result":{"exit":1,"durationSecs":125,"shards":[{"index":1,"state":"done","exit":0,"node":"a"},{"index":2,"state":"done","exit":1,"attempt":2,"node":"b","tests":{"passed":3,"failed":1,"errors":0}}],"tests":{"passed":5,"failed":1,"errors":0}}}"#;
    let set_running = r#"{"state":"running","short":"abcd1234","result":{"shards":[{"index":1,"state":"running","node":"a"},{"index":2,"state":"queued","reason":"cpu"}]}}"#;
    let set_failed = r#"{"state":"failed","reason":"lost","detail":"node gone","short":"abcd1234","result":{"shards":[{"index":1,"state":"failed","reason":"lost","node":"a"}]}}"#;
    let v = vec![
        case(
            "attached: the output and its exit",
            &["run", "--", "make"],
            &[],
            vec![ok(SUBMIT), logs(&out_err(), true, &done("7"))],
            owner_and_port,
            7,
            2,
        ),
        case(
            "attached --raw --json: the output on stderr, the result on stdout",
            &["run", "--raw", "--json", "--", "make"],
            &[],
            vec![ok(PLAIN), logs(&out_err(), true, &done("0")), ok(RESULT)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "--wait: the id, then the output",
            &["run", "mini", "--wait", "--", "make"],
            &[],
            vec![
                ok(PLAIN),
                logs(
                    "",
                    false,
                    r#"{"state":"running","short":"abcd1234","node":"mini"}"#,
                ),
                logs(&out_err(), true, &done("3")),
            ],
            owner_and_port,
            3,
            3,
        ),
        case(
            "--wait --max-wait: the output before the time is up",
            &["run", "--wait", "--max-wait", "10m", "--", "make"],
            &[],
            vec![ok(PLAIN), logs(&out_err(), true, &done("0"))],
            owner_and_port,
            0,
            2,
        ),
        case(
            "--wait --max-wait 0: the wait limit passed, exit 3",
            &["run", "--wait", "--max-wait", "0", "--", "make"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "--wait --json --max-wait 0: the wait's still_running, exit 3",
            &["run", "--wait", "--json", "--max-wait", "0", "--", "make"],
            &[],
            vec![
                ok(PLAIN),
                ok(r#"{"short":"abcd1234","state":"running","node":"mini"}"#),
            ],
            owner_and_port,
            3,
            2,
        ),
        case(
            "--wait --quiet: the wait's lines",
            &["run", "--wait", "-q", "--", "make"],
            &[],
            vec![ok(PLAIN), ok(WAIT_DONE)],
            owner_and_port,
            7,
            2,
        ),
        case(
            "--wait --json: the id on stderr, the wait's answer",
            &["run", "--wait", "--json", "--", "make"],
            &[],
            vec![ok(PLAIN), ok(WAIT_DONE)],
            owner_and_port,
            7,
            2,
        ),
        case(
            "a split run attached: each shard, then the end",
            &["run", "--shards", "2", "--", "make"],
            &[],
            vec![ok(SET), ok(set_running), ok(set_running), ok(set_done)],
            owner_and_port,
            1,
            4,
        ),
        case(
            "a split run that failed, --json",
            &["run", "--shards", "2", "--json", "--", "make"],
            &[],
            vec![ok(SET), ok(set_failed), ok(RESULT)],
            owner_and_port,
            3,
            3,
        ),
        case(
            "a split run, --wait",
            &["run", "--shards", "2", "--wait", "--", "make"],
            &[],
            vec![ok(SET), ok(WAIT_DONE)],
            owner_and_port,
            7,
            2,
        ),
        case(
            "the follow refused",
            &["run", "--", "make"],
            &[],
            vec![ok(PLAIN), Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            2,
        ),
        traceback(case(
            "a split run's reader raises",
            &["run", "--shards", "2", "--", "make"],
            &[],
            vec![
                ok(SET),
                ok(r#"{"state":"running","result":{"shards":[{"index":1}]}}"#),
            ],
            owner_and_port,
            1,
            2,
        )),
    ];
    for c in v {
        submitted(&rig, &c);
    }
}

#[test]
fn compute_run_words() {
    let rig = Rig::new(VERB, "runw");
    const STOPS: &[(&str, &[&str], i32)] = &[
        ("no command", &["run", "mini"], 2),
        (
            "no dashes after --any",
            &["run", "--any", "mini", "--", "x"],
            2,
        ),
        ("two nodes", &["run", "a", "b", "--", "x"], 2),
        ("an unknown flag", &["run", "--bogus", "--", "x"], 2),
        ("--avoid empty", &["run", "--avoid", "", "--", "x"], 2),
        ("--only last", &["run", "--only"], 2),
        ("--preset bad", &["run", "--preset", "Bad", "--", "x"], 2),
        ("--tag bad", &["run", "--tag", "a b", "--", "x"], 2),
        (
            "--same-node-as empty",
            &["run", "--same-node-as", "", "--", "x"],
            2,
        ),
        ("--repo empty", &["run", "--repo", "", "--", "x"], 2),
        ("--with empty", &["run", "--with", "", "--", "x"], 2),
        ("--with no dir", &["run", "--with", "@abc", "--", "x"], 2),
        ("--cache bad", &["run", "--cache", "-x", "--", "x"], 2),
        ("--tier bad", &["run", "--tier", "fast", "--", "x"], 2),
        ("--tier last", &["run", "--tier"], 2),
        (
            "--supersede with a space",
            &["run", "--supersede", " k", "--", "x"],
            2,
        ),
        (
            "--submitter bad",
            &["run", "--submitter", "A", "--", "x"],
            2,
        ),
        ("--distro bad", &["run", "--distro", "Ubuntu", "--", "x"], 2),
        ("--distro last", &["run", "--distro"], 2),
        ("--cpus N+M bad", &["run", "--cpus", "4+x", "--", "x"], 2),
        ("--mem bad", &["run", "--mem", "4T", "--", "x"], 2),
        ("--mem auto: bad", &["run", "--mem", "auto:x", "--", "x"], 2),
        ("--timeout bad", &["run", "--timeout", "1d", "--", "x"], 2),
        ("--timeout last", &["run", "--timeout"], 2),
        (
            "--queue-timeout bad",
            &["run", "--queue-timeout", "soon", "--", "x"],
            2,
        ),
        ("--gpu=nvidia", &["run", "--gpu=nvidia", "--", "x"], 2),
        ("--gpu bogus", &["run", "--gpu", "bogus", "--", "x"], 2),
        ("--gpu-mem bad", &["run", "--gpu-mem", "x", "--", "x"], 2),
        (
            "--gpu-mem with no --gpu",
            &["run", "--gpu-mem", "4G", "--", "x"],
            2,
        ),
        ("--network on", &["run", "--network", "on", "--", "x"], 2),
        ("--network last", &["run", "--network"], 2),
        (
            "--netns-dummy bad",
            &["run", "--netns-dummy", "d0", "--", "x"],
            2,
        ),
        (
            "five --netns-dummy",
            &[
                "run",
                "--netns-dummy",
                "a=1/2",
                "--netns-dummy",
                "b=1/2",
                "--netns-dummy",
                "c=1/2",
                "--netns-dummy",
                "d=1/2",
                "--netns-dummy",
                "e=1/2",
                "--",
                "x",
            ],
            2,
        ),
        ("--retries bad", &["run", "--retries", "x", "--", "x"], 2),
        ("--shards bad", &["run", "--shards", "", "--", "x"], 2),
        ("--wait --detach", &["run", "--wait", "-d", "--", "x"], 2),
        (
            "--priority with no why",
            &["run", "--priority", "--", "x"],
            2,
        ),
        (
            "--exclusive with no why",
            &["run", "--exclusive", "--", "x"],
            2,
        ),
        (
            "--now --shards",
            &["run", "--now", "--shards", "2", "--", "x"],
            2,
        ),
        (
            "batch --wait",
            &["run", "--tier", "batch", "--wait", "--", "x"],
            2,
        ),
        ("--quiet with no --wait", &["run", "--quiet", "--", "x"], 2),
        (
            "--max-wait with no --wait",
            &["run", "--max-wait", "9m", "--", "x"],
            2,
        ),
        ("--label with no =", &["run", "--label", "k", "--", "x"], 2),
        ("--env with no =", &["run", "--env", "A", "--", "x"], 2),
        ("--secret =X", &["run", "--secret", "=X", "--", "x"], 2),
    ];
    for (name, args, code) in STOPS {
        rig.check(&case(name, args, &[], vec![], owner_and_port, *code, 0));
    }
}

#[test]
fn compute_test() {
    let rig = Rig::new(VERB, "test");
    let v = vec![
        // covers: compute test
        case(
            "the default nextest script, detached",
            &["test", "--detach", "--", "-p", "k2 core", "it's"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a profile, a runner, shards, a need, run flags",
            &[
                "test",
                "--shards",
                "3",
                "--partition",
                "count",
                "--profile",
                "ci",
                "--runner",
                "cargo \"nextest\" run",
                "--needs",
                "cargo-nextest@0.9",
                "--os",
                "linux",
                "--gpu",
                "--gpu-mem",
                "2G",
                "--userns",
                "--detach",
                "--json",
            ],
            &[],
            vec![ok(SET), ok(RESULT)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "another runner, a node, --plan",
            &[
                "test",
                "mini",
                "--runner",
                "cargo test",
                "--plan",
                "--",
                "--lib",
            ],
            &[],
            vec![ok(PLAN_NOW)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        submitted(&rig, &c);
    }
    const STOPS: &[(&str, &[&str], i32)] = &[
        ("an unknown flag", &["test", "--wait"], 2),
        ("two nodes", &["test", "a", "b"], 2),
        ("--shards bad", &["test", "--shards", "x"], 2),
        ("--partition bad", &["test", "--partition", "mod"], 2),
        (
            "--shards with a node",
            &["test", "mini", "--shards", "2"],
            2,
        ),
        ("--profile last", &["test", "--profile"], 2),
        ("a run flag last", &["test", "--os"], 2),
        ("run's own usage", &["test", "--priority"], 2),
        ("run's checks", &["test", "--exclusive"], 2),
    ];
    for (name, args, code) in STOPS {
        rig.check(&case(name, args, &[], vec![], owner_and_port, *code, 0));
    }
    // shlex refuses the runner: the shell's traceback, exit 2.
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "a runner with no closing quote",
            &["test", "--runner", "cargo 'nextest"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )
    });
}

#[test]
fn compute_prewarm() {
    let rig = Rig::new(VERB, "prewarm");
    let answer = r#"{"nodes":[{"node":"mini","state":"ready"},{"node":"big","state":"installing","installs":["rust@stable","cmake@3"],"short":"j1"},{"node":"n3","state":"would_install","installs":["bun"]},{"node":"n4","state":"asked","ask":["libssl"],"note":"owner asked"},{"node":"n5","state":"would_ask","ask":[]},{"node":"n6","state":"cant_install"},{"node":"n7","state":"refused","error":{"code":"no_grant","hint":"ask for one"}},{"node":"n8"}],"fits":"fits 2 nodes","request":{"id":"pkr_1","summary":"libssl on n4"},"jobs":["j1","j2"]}"#;
    let not_filed =
        r#"{"nodes":[],"request":{"error":{"code":"not_allowed","hint":"policy"}},"jobs":[]}"#;
    let held = r#"{"nodes":[{"node":"n4","state":"asked","ask":["libssl"]}],"request":{"id":"pkr_2","summary":"libssl"},"pkgHeld": true,"jobs":[]}"#;
    let j1 = r#"{"id":"j1","short":"j1","state":"done","node":"big","exitCode":0}"#;
    let j2 = r#"{"id":"j2","short":"j2","state":"done","node":"big","exitCode":1}"#;
    let mut big = String::from(r#"{"nodes":["#);
    for i in 0..5000 {
        if i > 0 {
            big.push(',');
        }
        big.push_str(&format!(
            r#"{{"node":"node-{i:05}","state":"ready","note":"warm cache in slot {i}"}}"#
        ));
    }
    big.push_str(r#"],"jobs":[]}"#);
    assert!(big.len() > 256 * 1024);
    let big: &'static str = Box::leak(big.into_boxed_str());
    let v = vec![
        // covers: compute prewarm
        case(
            "a line per node",
            &["prewarm", "--needs", "rust@stable, cmake@3", "--node", "mini", "--node", "big", "--label", "gpu=amd", "--os", "linux", "--distro", "debian", "--arch", "arm64", "--tag", "t", "--workspace", "acme"],
            &[],
            vec![ok(answer)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute prewarm --json
        case(
            "--json: the answer",
            &["prewarm", "--preset", "ci", "--json"],
            &[],
            vec![ok(answer)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--plan, a request not filed",
            &["prewarm", "--dry-run"],
            &[],
            vec![ok(not_filed)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--wait: the jobs it started",
            &["prewarm", "--wait"],
            &[],
            vec![ok(answer), ok(j1), ok(j2)],
            owner_and_port,
            1,
            3,
        ),
        case(
            "--wait --json",
            &["prewarm", "--wait", "--json"],
            &[],
            vec![ok(answer), ok(j1), ok(j2)],
            owner_and_port,
            1,
            3,
        ),
        case(
            "--wait --json, no jobs",
            &["prewarm", "--wait", "--json"],
            &[],
            vec![ok(not_filed)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "held for the owner",
            &["prewarm", "--needs", "libssl"],
            &[],
            vec![ok(held)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "held, with a card to ask on",
            &["prewarm", "--needs", "libssl"],
            &[],
            vec![ok(&*Box::leak(
                held.replace(
                    "\"jobs\":[]",
                    "\"jobs\":[],\"permission\":{\"command\":\"k2 permission request compute.packages request=pkr_2 --why \\\"...\\\"\",\"requestable\":true}",
                )
                .into_boxed_str(),
            ))],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an answer over 256 KiB",
            &["prewarm"],
            &[],
            vec![ok(big)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refused",
            &["prewarm", "--wait"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case(
                "in a session, on the cell socket",
                &["prewarm", "--needs", "bun"],
                &[],
                vec![ok(answer)],
                port_only,
                0,
                1,
            )
        },
        traceback(case(
            "the reader raises",
            &["prewarm"],
            &[],
            vec![ok(r#"{"nodes":[{"node":"n","state":"installing","installs":[1]}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case("an unknown word", &["prewarm", "--bogus"], &[], vec![], owner_and_port, 2, 0),
        case("--plan --wait", &["prewarm", "--plan", "--wait"], &[], vec![], owner_and_port, 2, 0),
        case("--label with no =", &["prewarm", "--label", "x"], &[], vec![], owner_and_port, 2, 0),
        case("--distro bad", &["prewarm", "--distro", "A"], &[], vec![], owner_and_port, 2, 0),
        case("--node last", &["prewarm", "--node"], &[], vec![], owner_and_port, 2, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: -h or --help before any bare `--` on run, test or
/// prewarm prints help and sends nothing (the front hands it to the
/// shell's help). After `--` it is the command's (75dfa4fc07).
#[test]
fn compute_submit_help_never_acts() {
    let rig = Rig::new(VERB, "submithelp");
    const HELP: &[&[&str]] = &[
        &["run", "mini", "--detach", "-h", "--", "x"],
        &["run", "--help"],
        &["test", "--shards", "2", "-h"],
        &["prewarm", "--wait", "--help"],
        &["prewarm", "-h"],
    ];
    for args in HELP {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
    // After a bare `--` a help word is the command's: submitted. The
    // front's scan and the family's stop at `--`, as `_wants_help` does.
    for (name, args, reply) in [
        (
            "run -- make --help: submitted",
            &["run", "--detach", "--", "make", "--help"][..],
            SUBMIT,
        ),
        (
            "test -- --help: submitted",
            &["test", "--detach", "--", "--help"][..],
            PLAIN,
        ),
    ] {
        let c = case(name, args, &[], vec![ok(reply)], owner_and_port, 0, 1);
        submitted(&rig, &c);
    }
}

/// What the binary hands to the shell: `--failures` (the failing tests'
/// list after the end); bash then stops at the label with no `=`.
#[test]
fn compute_run_failures_is_the_shells() {
    let rig = Rig::new(VERB, "runfail");
    // The shell's follow exits with _CP_DEADLINE_RC when it is set, so
    // the binary hands the whole line over (a usage stop shows it).
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "_CP_DEADLINE_RC set: the line is the shell's",
            &["run", "--wait", "--max-wait", "0", "--bogus", "--", "make"],
            &[("_CP_DEADLINE_RC", "124")],
            vec![],
            owner_and_port,
            2,
            0,
        )
    });
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "--wait --failures",
            &["run", "--wait", "--failures", "--label", "k", "--", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )
    });
}

/// `compute` alone and `compute help`: the help page.
#[test]
fn compute_alone() {
    let rig = Rig::new(VERB, "alone");
    // covers: compute
    rig.check(&case(
        "compute alone",
        &[],
        &[],
        vec![],
        owner_and_port,
        0,
        0,
    ));
    rig.check(&case(
        "compute help",
        &["help"],
        &[],
        vec![],
        owner_and_port,
        0,
        0,
    ));
    rig.check(&case(
        "compute \"\"",
        &[""],
        &[],
        vec![],
        owner_and_port,
        0,
        0,
    ));
}

/// The lines name the command and the product by the brand's names (in a
/// build with `K2_BRAND_DIR=brand/xy` they say `xy` and `Globex Desk`), never a
/// typed `k2`, `k5` or `K2`.
#[test]
fn the_submit_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    for (args, want) in [
        (
            &["run", "mini"][..],
            format!("usage: {cli} compute run [<node>] [flags] -- <command\u{2026}>  (no node: {product} picks one)\n"),
        ),
        (
            &["run", "--bogus", "--", "x"],
            format!("usage: unknown flag '--bogus' ({cli} compute --help)\n"),
        ),
        (
            &["run", "--priority", "--", "x"],
            format!("usage: --priority needs a one-line reason: --priority --why \"release gate for 0.46.6\" (it shows in the queue and {cli} compute status; Priority is for the critical path only)\n"),
        ),
        (
            &["run", "--avoid", "", "--", "x"],
            format!("usage: --avoid <node> (a node {product} must not pick; repeatable)\n"),
        ),
        (
            &["test", "mini", "--shards", "2"],
            format!("usage: --shards picks a node for each shard itself (no node: {product} picks); drop the node name\n"),
        ),
        (
            &["test", "--wait"],
            format!("usage: unknown flag '--wait' ({cli} compute test --help)\n"),
        ),
        (
            &["prewarm", "--bogus"],
            format!("usage: {cli} compute prewarm [--preset NAME | --needs rust@stable,cmake@3] [--os linux|macos] [--distro ubuntu@24.04] [--arch A] [--label k=v] [--node N]... [--tag T] [--plan] [--wait] [--workspace WS] [--json] (unknown '--bogus')\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// `compute run`, attached and with `--wait`, follows a moved job into its
/// new generation: the `moved:` line, never "infrastructure failure" (the
/// shell's 46854e87d1).
#[test]
fn compute_run_follows_a_move() {
    let rig = Rig::new(VERB, "runmove");
    let moved = || {
        ok(
            r#"{"chunks":[],"end":true,"cursor":1,"generation":1,"attempts":[{"attempt":1,"generation":1,"current":false,"reason":"withdrawn","moved":"node-a gave it back from its queue before it started for node-b"},{"attempt":1,"generation":2,"current":true}],"job":{"state":"queued","short":"abcd1234","node":"node-b","attempt":1,"generation":2}}"#,
        )
    };
    let after = || {
        ok(&format!(
            r#"{{"cursor":"c:1","end":true,"generation":2,"chunks":[{}],"job":{},"queue":{{}}}}"#,
            out_err(),
            done("0").replace("\"generation\":1", "\"generation\":2")
        ))
    };
    let v = vec![
        case(
            "attached: a move, then the output",
            &["run", "--", "make"],
            &[],
            vec![ok(SUBMIT), moved(), after()],
            owner_and_port,
            0,
            3,
        ),
        case(
            "--wait: a move, then the output",
            &["run", "mini", "--wait", "--", "make"],
            &[],
            vec![ok(PLAIN), moved(), after()],
            owner_and_port,
            0,
            3,
        ),
    ];
    for c in v {
        submitted(&rig, &c);
    }
}
