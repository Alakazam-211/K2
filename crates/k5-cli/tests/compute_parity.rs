//! `k5-cli compute <read verb>` against the shell CLI's `cli/k2 compute
//! <read verb>`, on the parity rig ([`common::parity`]): the same requests,
//! stdout, stderr and exit code.
//!
//! - Each verb's words: its forms, `--json`, its usage lines (exit 2), a
//!   value flag last (`shift 2` runs out: exit 1, nothing printed). `list`
//!   is `jobs` (`ls` stays `nodes`).
//! - The answers: full ones for each printer (its every branch is checked
//!   against the shell's Python in compute_render_oracle.rs), none, the
//!   daemon's errors (400, 403 with a permission card and its "Ask with:"
//!   line, 404 (4 for get), 409, 500 not JSON), Python's raise, the
//!   bulletin, a list over 256 KiB (jobs and events --audit).
//! - The owner from disk, a passport over TCP, the cell socket (and "not
//!   served" there), a dead socket file (TCP with the passport), the id
//!   tool's checks, a refused passport (the dead-passport line).
//! - No answer: an empty reply, a reset, not HTTP, no daemon: the
//!   no-answer object, exit 3.
//! - What the binary hands to the shell.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";

const NODES: &str = r#"{"criticalPath":[{"submitter":"agent-a","jobs":2,"workspace":"/w/a"}],"nodes":[{"name":"linux-1","platform":"ubuntu24.04-x86_64","online":true,"liveRoute":"lan","offer":{"control":"active","caps":{"cpu_millis":8000,"mem_bytes":17179869184},"free":{"cpu_millis":6500,"mem_bytes":12884901888,"disk_free_bytes":107374182400},"parallelMax":4},"idleCpuMillis":7250,"room":{"busy":"this_server","running":3,"otherServers":1},"mine":{"running":1,"queued":0},"running":2,"queued":1,"queueTiers":{"ci":1,"normal":2},"grant":{"maxJobSecs":7200,"maxParallel":2,"maxQueued":5,"allowCi":true},"reservedBy":[{"short":"aaaa1111","state":"running","cpuMillis":2000,"memBytes":4294967296}],"share":{"role":"tenant","ownerLabel":"Acme","servers":3,"line":"run 1/2"},"rollout":{"code":"node_needs_manual_install"},"nodeVersion":"0.46.17","allowed":false},{"name":"mac-1","state":"pending","sas":"123456","os":"macos","arch":"arm64"}],"allowNodes":["linux-1"]}"#;
const JOB: &str = r#"{"short":"abcd1234","node":"linux-1","state":"done","argv":["cargo","test"],"tier":"priority","ctl":{"placement":{"mode":"auto","code":"moved_node_draining","node":"linux-1","policy":"v2","why":"free cpu"}},"exitCode":0,"signal":null,"attempt":1,"generation":1,"result":{"exit":0,"tests":{"passed":3,"failed":0,"errors":0,"skipped":1},"artifacts":[{"name":"out.tar","bytes":100,"sha256":"0123456789abcdef0123"}]}}"#;
const JOBS: &str = r#"{"jobs":[{"short":"abcd1234","tier":"ci","node":"linux-1","state":"queued","pinned":true,"submitter":"agent-a","via":"owner_token","tag":"t","argv":["cargo","test"]},{"short":"efgh5678","state":"done","exitCode":0,"reason":"","submittedBy":"owner"}]}"#;
/// Running jobs that waited before they started: 10 minutes (shown
/// apart), 59 s (not), over an hour, a float, none.
const STATUS_WAITED: &str = r#"{"nodes":[{"name":"mini-1","os":"macos","arch":"arm64","status":{"state":"active","parallelMax":1,"running":[{"short":"run00001","state":"running","elapsedSecs":120,"queuedSecs":600,"workspace":"ws","argv":"cargo test","cpuMillis":4000},{"short":"run00002","elapsedSecs":5,"queuedSecs":59,"argv":"a"},{"short":"run00003","elapsedSecs":7300,"queuedSecs":3725,"tierWhy":"gate","tier":"priority"},{"short":"run00004","elapsedSecs":1,"queuedSecs":90.5,"cpuMillis":null},{"short":"run00005","queuedSecs":null}],"queued":[{"short":"que00001","state":"queued","ageSecs":300,"queuedSecs":300,"position":1,"workspace":"ws","argv":"true","pinned":false}]}}]}"#;
const STATUS: &str = r#"{"nodes":[{"name":"linux-1","os":"linux","arch":"x86_64","status":{"state":"online","parallelMax":4,"running":[{"short":"abcd1234","elapsedSecs":125,"workspace":"/w","argv":"cargo test","tier":"ci"}],"queued":[{"short":"q1","position":2,"ageSecs":30,"pinned":true}],"free":{"cpuMillis":6500,"memBytes":12884901888},"caps":{"cpuMillis":8000,"memBytes":17179869184},"load":[1.25,0.5],"disk":{"freeBytes":107374182400}}}],"notYet":"GPU stats"}"#;
const QUEUE: &str = r#"{"node":"linux-1","entries":[{"pos":1,"mine":true,"short":"abcd","state":"queued","pinned":false,"argv":"cargo test","etaSecs":61,"cpuMillis":2500,"why":"waits"},{"pos":2,"server":"Globex","state":"running","etaSecs":30}],"totals":[{"label":"this","running":1,"queued":2,"cpuSecsToday":5400}]}"#;
const GRANTS: &str = r#"{"grants":[{"node":"n1","workspace":"/w","maxJobSecs":3600,"maxParallel":2,"maxQueued":3,"maxDiskGb":10,"allowCi":true,"grantedBy":"owner"}]}"#;
const RECEIPT: &str =
    r#"{"receipt":{"id":"r1","sig":"ab"},"verified":true,"proves":"the node ran it"}"#;
const EVENTS: &str = r#"{"events":[{"at":1763600000,"kind":"node.pause","actor":"owner","nodeId":"0123456789","detail":{"why":"maintenance"}}]}"#;
const STATS: &str = r#"{"text":["job abcd1234: 3 samples","cpu 2.0 of 4"]}"#;
const USAGE: &str = r#"{"usage":[{"day":"2026-11-19","node":"n1","route":"relay","bytesIn":1234567,"bytesOut":5}],"metered":"relay traffic is metered"}"#;

fn big_jobs(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"short":"j{i:07}","node":"node-{}","state":"done","exitCode":{},"reason":"r{i}","submitter":"agent-{}","tag":"tag-{i}","argv":["cargo","nextest","run","--workspace","-E","test(name_{i})"]}}"#,
                i % 7,
                i % 3,
                i % 11
            )
        })
        .collect();
    format!(r#"{{"jobs":[{}]}}"#, rows.join(","))
}

fn big_events(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"at":{},"kind":"job.cancel","actor":"agent-{i}","jobId":"j{i:07}xx","detail":{{"why":"row {i} é","n":{i}}}}}"#,
                1_763_600_000 + i
            )
        })
        .collect();
    format!(r#"{{"events":[{}]}}"#, rows.join(","))
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

#[test]
fn compute_reads_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: compute nodes
        ok_case("nodes", &["nodes"], NODES),
        // covers: compute nodes --json
        ok_case("ls --json", &["ls", "x", "--json"], NODES),
        ok_case("nodes: none", &["nodes"], r#"{"nodes":[]}"#),
        // covers: compute info
        ok_case("info", &["info", "linux-1"], &NODES[NODES.find("{\"name\"").unwrap()..NODES.find(",{\"name\":\"mac-1\"").unwrap()]),
        // covers: compute info --json
        ok_case("info --json first", &["info", "--json"], r#"{"name":"--json"}"#),
        usage_case("info alone", &["info"]),
        // covers: compute queue
        ok_case("queue", &["queue", "linux-1"], QUEUE),
        // covers: compute queue --json
        ok_case("queue --json", &["queue", "n 1", "--json"], QUEUE),
        usage_case("queue -x", &["queue", "-x"]),
        // covers: compute jobs
        ok_case("jobs", &["jobs"], JOBS),
        // covers: compute jobs --json
        ok_case(
            "jobs with every filter",
            &["jobs", "--node", "a b/c", "--mine", "--state", "queued", "--tier", "standard", "--tag", "t&1", "--limit", "5", "--workspace", "/w x", "--json"],
            JOBS,
        ),
        ok_case("jobs --tier ci", &["jobs", "--tier", "ci"], r#"{"jobs":[]}"#),
        usage_case("jobs --tier bad", &["jobs", "--tier", "x"]),
        usage_case("jobs, a word it doesn't take", &["jobs", "x"]),
        case("jobs --node last: shift runs out", &["jobs", "--node"], &[], vec![], owner_and_port, 1, 0),
        ok_case("list is jobs", &["list"], JOBS),
        ok_case("list --mine --state done --json", &["list", "--mine", "--state", "done", "--json"], JOBS),
        usage_case("list, a word it doesn't take", &["list", "x"]),
        case("list --tag last: shift runs out", &["list", "--tag"], &[], vec![], owner_and_port, 1, 0),
        // covers: compute get
        ok_case("get", &["get", "abcd1234"], JOB),
        ok_case("get --full", &["get", "abcd1234", "--full"], JOB),
        // covers: compute get --json
        ok_case("get --json: the result", &["get", "abcd1234", "--json"], JOB),
        // covers: compute status
        ok_case("status <job>", &["status", "abcd1234"], JOB),
        usage_case("get alone", &["get"]),
        case(
            "get: not found is 4",
            &["get", "zzzz"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"job_not_found","hint":"no such job"}}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "status <job>: not found is 4",
            &["status", "zzzz"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"job_not_found","hint":"no such job"}}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        ok_case("status", &["status"], STATUS),
        ok_case("status: the wait before a running job, apart", &["status"], STATUS_WAITED),
        Case {
            compare: Compare::Traceback,
            ..case(
                "status: a queuedSecs that is no number",
                &["status"],
                &[],
                vec![ok(r#"{"nodes":[{"name":"n","status":{"running":[{"short":"r","elapsedSecs":1,"queuedSecs":"10m"}]}}]}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        // covers: compute status --json
        ok_case("status --json", &["status", "--json"], STATUS),
        usage_case("status --bad", &["status", "--bad"]),
        // covers: compute grants
        ok_case("grants", &["grants"], GRANTS),
        // covers: compute grants --json
        ok_case("grants filtered --json", &["grants", "--node", "n1", "--workspace", "/w", "--json"], GRANTS),
        case("grants --workspace last", &["grants", "--workspace"], &[], vec![], owner_and_port, 1, 0),
        // covers: compute receipt
        ok_case("receipt", &["receipt", "abcd1234"], RECEIPT),
        // covers: compute receipt --json
        ok_case("receipt --json", &["receipt", "--json", "abcd1234"], RECEIPT),
        usage_case("receipt alone", &["receipt"]),
        usage_case("receipt, two jobs", &["receipt", "a", "b"]),
        usage_case("receipt, an unknown flag", &["receipt", "--x"]),
        // covers: compute events
        ok_case("events --audit", &["events", "--audit"], EVENTS),
        ok_case("events --audit --limit --json", &["events", "--audit", "--limit", "5", "--json"], EVENTS),
        usage_case("events, a word it doesn't take", &["events", "--audit", "x"]),
        ok_case("events", &["events"], r#"{"events":[{"at":1763600000,"short":"abcd1234","event":"finished","node":"n1","state":"failed","exit":2,"reason":"tests"},{"at":1763600001,"short":"efgh","event":"moved","detail":"moved from a to b: b can start it now"},{"event":"queued"}],"cursor":"c42"}"#),
        // covers: compute events --json
        ok_case("events --mine --tag --limit --json", &["events", "--mine", "--tag", "t 1", "--limit", "5", "--json"], r#"{"events":[{"at":1,"short":"a","event":"started"}],"cursor":7}"#),
        ok_case("events --since: none, no cursor", &["events", "--since", "c41"], r#"{"events":[]}"#),
        case(
            "events --follow --max-wait 0: one wait, then the line",
            &["events", "--follow", "--max-wait", "0"],
            &[],
            vec![ok(r#"{"events":[{"at":1,"short":"a","event":"queued","node":"n1"}],"cursor":"c9"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute stats
        ok_case("stats", &["stats", "abcd1234"], STATS),
        // covers: compute stats --json
        ok_case("stats --json", &["stats", "--json", "abcd1234"], STATS),
        ok_case("stats: no text", &["stats", "abcd1234"], r#"{"text":[]}"#),
        usage_case("stats: too short", &["stats", "abc"]),
        // covers: compute usage
        ok_case("usage", &["usage"], USAGE),
        ok_case("usage --days", &["usage", "--days", "3"], USAGE),
        ok_case("the bulletin", &["jobs"], BULLETIN),
        // covers: compute rightsize
        ok_case("rightsize --check", &["rightsize", "--check", "unit tests", "--days", "14d"], STATS),
        // covers: compute rightsize --json
        ok_case("rightsize -- a command", &["rightsize", "--json", "--", "cargo", "test", "é x"], STATS),
        ok_case("rightsize --tag --workspace", &["rightsize", "--tag", "t", "--workspace", "/w x"], STATS),
        usage_case("rightsize: none of the three", &["rightsize"]),
        usage_case("rightsize: two of them", &["rightsize", "--check", "a", "--tag", "b"]),
        usage_case("rightsize --days x", &["rightsize", "--check", "a", "--days", "x"]),
        case("rightsize --days last", &["rightsize", "--check", "a", "--days"], &[], vec![], owner_and_port, 1, 0),
        // covers: compute preview
        ok_case("preview", &["preview"], r#"{"computePreview":true}"#),
        ok_case("preview status", &["preview", "status"], r#"{}"#),
        usage_case("preview x", &["preview", "x"]),
        // covers: compute placement
        ok_case("placement", &["placement"], r#"{"computePlacement":"v1"}"#),
        ok_case("placement status: an odd value", &["placement", "status"], r#"{"computePlacement":"v9"}"#),
        usage_case("placement x", &["placement", "x"]),
        case("400: exit 2", &["jobs"], &[], vec![Reply::Answer(400, r#"{"error":{"code":"bad_tier","hint":"no such tier"}}"#.into())], owner_and_port, 2, 1),
        case("a permission card: exit 3 and Ask with", &["nodes"], &[], vec![Reply::Answer(403, PERMISSION_ASK.into())], owner_and_port, 3, 1),
        case(
            "ci.preview off: its card",
            &["stats", "abcd1234"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"ci_preview_off","hint":"CI preview is off"},"permission":{"command":"k2 permission request --scope ci.preview --why \"...\"","requestable":true}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case("404 elsewhere: exit 3", &["info", "zz"], &[], vec![Reply::Answer(404, r#"{"error":{"code":"no_node"}}"#.into())], owner_and_port, 3, 1),
        case("the route floor's error", &["queue", "n"], &[], vec![Reply::Answer(409, r#"{"error":"busy","message":"try later"}"#.into())], owner_and_port, 3, 1),
        case("500 not JSON", &["jobs"], &[], vec![Reply::Answer(500, "boom\n".into())], owner_and_port, 3, 1),
        Case {
            compare: Compare::Traceback,
            ..case("Python's raise", &["get", "abcd1234"], &[], vec![ok("[]")], owner_and_port, 1, 1)
        },
        Case {
            compare: Compare::Traceback,
            ..case("Python's raise part way", &["jobs"], &[], vec![ok(r#"{"jobs":[{"short":"a","state":"done"},{}]}"#)], owner_and_port, 1, 1)
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One answer over 256 KiB per list verb: thousands of rows.
#[test]
fn compute_large_lists() {
    let rig = Rig::new(VERB, "big");
    let jobs = big_jobs(3000);
    let events = big_events(3000);
    assert!(jobs.len() > 256 * 1024 && events.len() > 256 * 1024);
    for c in [
        ok_case("jobs, 3000 rows", &["jobs"], &jobs),
        ok_case("jobs --json, 3000 rows", &["jobs", "--json"], &jobs),
        ok_case("list, 3000 rows", &["list"], &jobs),
        ok_case("events --audit, 3000 rows", &["events", "--audit"], &events),
    ] {
        rig.check(&c);
    }
}

#[test]
fn compute_with_a_passport() {
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
                "nodes: the passport over TCP, no workspace",
                &["nodes"],
                &[],
                vec![ok(NODES)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "status: the passport over TCP",
                &["status"],
                &[],
                vec![ok(STATUS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "jobs on the cell socket",
                &["jobs", "--mine"],
                &[],
                vec![ok(JOBS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "get on the cell socket",
                &["get", "abcd1234"],
                &[],
                vec![ok(JOB)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "not served on the cell socket: TCP",
                &["grants"],
                &[],
                vec![not_served(), ok(GRANTS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "events --audit: TCP only",
                &["events", "--audit"],
                &[],
                vec![ok(EVENTS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "usage: TCP only",
                &["usage"],
                &[],
                vec![ok(USAGE)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "a refusal on the cell socket is the answer",
                &["nodes"],
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
                "a dead socket file: TCP with the passport",
                &["jobs"],
                &[],
                vec![ok(JOBS)],
                owner_and_port,
                0,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone: the probe, then the passport",
                &["nodes"],
                &[],
                vec![ok(r#"{"role":"canonical"}"#), ok(NODES)],
                owner_and_port,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["nodes"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["jobs"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            h(&[]),
            case(
                "a refused passport: the probe and the dead-passport line",
                &["jobs"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                3,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        case(
            "an empty reply",
            &["jobs"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 5xx with no body",
            &["nodes"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a reset",
            &["get", "abcd1234"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not HTTP",
            &["status"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            3,
            1,
        ),
        with(
            cell(),
            case(
                "the cell socket and TCP: an empty reply on both",
                &["jobs"],
                &[],
                vec![Reply::Close, Reply::Close],
                port_only,
                3,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens",
        &["jobs"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        3,
        0,
    ));
}

/// `_CP_404_EXIT` exported: nothing reads it any more (`_compute_fail`
/// takes its exit for a 404 from the caller), so the binary answers and a
/// 404 is the verb's own exit: 3, or 4 for `get`.
#[test]
fn compute_an_exported_404_exit_changes_nothing() {
    let rig = Rig::new(VERB, "cp404");
    let v = vec![
        case(
            "_CP_404_EXIT set: info's 404 is 3",
            &["info", "zz"],
            &[("_CP_404_EXIT", "9")],
            vec![Reply::Answer(404, r#"{"error":{"code":"no_node"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "_CP_404_EXIT set: get's 404 is 4",
            &["get", "j1"],
            &[("_CP_404_EXIT", "0")],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn compute_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        // `compute` alone is the binary's (bare_parity.rs); `help` is no
        // verb row.
        shell(case(
            "compute help",
            &["help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "jobs --help",
            &["jobs", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        // `preview on` itself is the admin verbs' (compute_admin_parity).
        // Help never acts: the write forms of these arms print help and
        // send nothing.
        shell(case(
            "preview on -h: help, nothing sent",
            &["preview", "on", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "placement --help v1: help, nothing sent",
            &["placement", "--help", "v1"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "a port curl won't take",
            &["jobs"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            3,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

fn b64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn page(cursor: u64, chunks: &[&[u8]], attempts: &str) -> String {
    let c: Vec<String> = chunks
        .iter()
        .enumerate()
        .map(|(i, d)| format!(r#"{{"seq":{i},"stream":"out","data":"{}"}}"#, b64(d)))
        .collect();
    format!(
        r#"{{"job":{{"short":"abcd1234"}},"cursor":{cursor},"chunks":[{}],"attempt":2,"generation":3,"attempts":{attempts}}}"#,
        c.join(",")
    )
}

const LOG1: &[u8] = b"\x1b[1;32mCompiling\x1b[0m k5-cli\r\ntest a ... ok\ntest b ... FAILED\n";
const LOG2: &[u8] = b"error[E0425]: no x\n\xff bad byte\nthread 'b' panicked at src/x.rs\ndone\n";
const ATTEMPTS: &str = r#"[{"attempt":1,"generation":1,"node":"mac-1","state":"failed","reason":"node_lost","startedAt":1763600000},{"attempt":2,"generation":3,"node":"linux-1","current":true}]"#;
const TESTED: &str = r#"{"short":"abcd1234","state":"done","result":{"tests":{"failed":1,"errors":0,"passed":3,"skipped":0,"total":4,"failing":[{"name":"b","classname":"suite","message":"boom\nmore","mark":"new","shard":2}],"truncated":true},"testHistory":{"flaky":1}}}"#;

#[test]
fn compute_logs() {
    let rig = Rig::new(VERB, "logs");
    let p1 = page(10, &[LOG1, LOG2], "[]");
    let p2 = page(10, &[], "[]");
    let a1 = page(10, &[LOG1], ATTEMPTS);
    let a2 = page(10, &[], ATTEMPTS);
    let shards = r#"{"short":"abcd1234","state":"done","result":{"shards":[{"job":"s1-full-id","index":1,"short":"s1","node":"n1","state":"done","exit":0},{"job":"s2-full-id","index":2,"short":"s2","state":"failed","exit":1}]}}"#;
    let big: Vec<u8> = (0..20_000)
        .flat_map(|i| format!("line {i}: test name_{i} ... ok é\n").into_bytes())
        .collect();
    assert!(big.len() > 256 * 1024);
    let pb = page(1, &[&big], "[]");
    let v = vec![
        // covers: compute logs
        case(
            "logs",
            &["logs", "abcd1234"],
            &[],
            vec![ok(JOB), ok(&p1), ok(&p2)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "logs --raw --tail 2",
            &["logs", "abcd1234", "--raw", "--tail", "2"],
            &[],
            vec![ok(JOB), ok(&p1), ok(&p2)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "logs --tail 0: all",
            &["logs", "abcd1234", "--tail", "0"],
            &[],
            vec![ok(JOB), ok(&p1), ok(&p2)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "logs --failures",
            &["logs", "abcd1234", "--failures"],
            &[],
            vec![ok(TESTED), ok(TESTED), ok(&p1), ok(&p2)],
            owner_and_port,
            0,
            4,
        ),
        case(
            "logs --failures --no-tests",
            &["logs", "abcd1234", "--failures", "--no-tests"],
            &[],
            vec![ok(JOB), ok(&p1), ok(&p2)],
            owner_and_port,
            0,
            3,
        ),
        // covers: compute logs --json
        case(
            "logs --json",
            &["logs", "abcd1234", "--json"],
            &[],
            vec![ok(JOB), ok(&a1), ok(&a2)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "logs: other attempts",
            &["logs", "abcd1234"],
            &[],
            vec![ok(JOB), ok(&a1), ok(&a2)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "logs --attempt 1",
            &["logs", "abcd1234", "--attempt", "1"],
            &[],
            vec![ok(JOB), ok(&a1), ok(&a2)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "logs: shards",
            &["logs", "abcd1234", "--tail", "1"],
            &[],
            vec![
                ok(shards),
                ok(JOB),
                ok(&p1),
                ok(&p2),
                ok(JOB),
                ok(&p1),
                ok(&p2),
            ],
            owner_and_port,
            0,
            7,
        ),
        case(
            "logs over 256 KiB",
            &["logs", "abcd1234"],
            &[],
            vec![ok(JOB), ok(&pb), ok(&p2)],
            owner_and_port,
            0,
            3,
        ),
        usage_case("logs alone", &["logs"]),
        usage_case("logs --attempt 0", &["logs", "j", "--attempt", "0"]),
        usage_case("logs -x", &["logs", "j", "-x"]),
        case(
            "logs --tail last",
            &["logs", "j", "--tail"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "logs: the job not found",
            &["logs", "zz"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"job_not_found","hint":"no such job"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // TMPDIR names a folder that isn't there. Linux's `mktemp -t`
        // fails there, after the job's answer; macOS's falls back to the
        // user's own temp folder (then /tmp), so the log is read as ever.
        // Both sides run the mktemp on PATH.
        if cfg!(target_os = "macos") {
            Case {
                no_tmpdir: true,
                ..case(
                    "logs: TMPDIR unusable, macOS's mktemp falls back",
                    &["logs", "abcd1234"],
                    &[],
                    vec![ok(JOB), ok(&p1), ok(&p2)],
                    owner_and_port,
                    0,
                    3,
                )
            }
        } else {
            Case {
                no_tmpdir: true,
                ..case(
                    "logs: TMPDIR unusable",
                    &["logs", "abcd1234"],
                    &[],
                    vec![ok(JOB)],
                    owner_and_port,
                    1,
                    1,
                )
            }
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "logs: a chunk that is not base64",
                &["logs", "abcd1234"],
                &[],
                vec![ok(JOB), ok(r#"{"cursor":1,"chunks":[{"data":"abc"}]}"#)],
                owner_and_port,
                1,
                2,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
    let found = r#"{"workspace":"/w","search":"boom é","fold":true,"matches":[{"short":"abcd1234","at":1763600000,"check":"unit","line":"thread panicked: boom \"x\"\t\u001b"},{"short":"efgh","line":5}],"scanned":{"jobs":1},"cut":"limit"}"#;
    let none =
        r#"{"workspace":"/w","search":"zz","matches":[],"scanned":{"jobs":3},"cut":"other"}"#;
    for c in [
        ok_case(
            "logs --search",
            &[
                "logs", "--search", "boom é", "-i", "--check", "unit", "--limit", "5",
            ],
            found,
        ),
        ok_case(
            "logs --search=, none found",
            &[
                "logs",
                "--search=zz",
                "--state",
                "failed",
                "--since",
                "7d",
                "--branch",
                "main",
                "--workspace",
                "/w x",
            ],
            none,
        ),
        ok_case(
            "logs --search --json",
            &["logs", "--json", "--search", "boom"],
            found,
        ),
        usage_case("logs --search, one character", &["logs", "--search", "x"]),
        usage_case(
            "logs --search, a job word",
            &["logs", "abcd", "--search", "boom"],
        ),
        usage_case(
            "logs --search --limit x",
            &["logs", "--search", "boom", "--limit", "x"],
        ),
        case(
            "logs --search last",
            &["logs", "--search"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
    ] {
        rig.check(&c);
    }
}
