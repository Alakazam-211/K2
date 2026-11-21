//! `k5-cli compute wait` against the shell CLI's `cli/k2 compute wait`,
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - One job and several: done at once, the progress lines as the state
//!   or the queue changes (one per change, none with `--quiet`), each way
//!   a job ends (its exit code, a signal, failed with its reason, a code
//!   above 255), a job's hint under its line (a stalled job's), `--json`,
//!   `--timeout 0` (still running, exit 124, nothing sent but the polls),
//!   `--max-wait 0` (the older spelling: exit 3).
//! - `--any`: the first to end, the round with none ended, `--timeout`
//!   (124), `--max-wait` (3), `--json`.
//! - No answer once, then the answer (the lost line, 10 s); the daemon's
//!   refusals (404 is exit 4); Python's raise; the cell socket.
//! - The words: none, an unknown flag, `--timeout` or `--max-wait` last
//!   or empty, both limits, one twice.
//! - What the binary hands to the shell: `--follow`, `--failures`, no
//!   temp folder (Linux; macOS's mktemp falls back, so it is native
//!   there), help. A duration that is none goes to the shell too;
//!   bash's own arithmetic error names the script's path there, so it has
//!   no row until D6's named difference.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";
const DONE: &str = r#"{"id":"aaaa1111bbbb","short":"aaaa1111","state":"done","node":"mini","exitCode":0,"startedAt":1000,"endedAt":1125}"#;
const RUNNING: &str = r#"{"id":"aaaa1111bbbb","short":"aaaa1111","state":"running","node":"mini"}"#;
const QUEUED: &str = r#"{"id":"aaaa1111bbbb","short":"aaaa1111","state":"queued","node":"mini","queue":{"position":2,"of":3,"reason":"cpu"}}"#;
const QUEUED_LINE: &str = r#"{"short":"aaaa1111","state":"queued","queue":{"line":"queued 1st on mini","position":1,"waitReason":"slot"},"wait":{"elsewhere":"linux-2 can start it now"}}"#;
// 0.46.17: held on this server (queue.waitReason "held"), then a pull
// back from a node's own queue in flight, then on a node.
const HELD: &str = r#"{"short":"aaaa1111","state":"queued","node":"mini","queue":{"line":"held: waiting for a node with a free slot (fits: mini, linux-2, node-c, node-d, +1 more)","position":1,"reason":"grant_parallel_full","waitReason":"held","held":true,"fits":["mini","linux-2","node-c","node-d","node-e"],"nodeWaitWords":"all slots busy"},"wait":{"reason":"held","words":"held: waiting for a node with a free slot (fits: mini, linux-2, node-c, node-d, +1 more)"}}"#;
const PULLING: &str = r#"{"short":"aaaa1111","state":"queued","node":"mini","queue":{"onNode":true,"message":"2nd in line on mini","position":2,"reason":"pull_requested"},"move":{"note":"pulling back: mini was asked to give it back (k2 compute move); then it is held on this server for the first node that fits with a free slot"}}"#;
const B_RUNNING: &str = r#"{"id":"bbbb2222cccc","short":"bbbb2222","state":"running","node":"n2"}"#;
const STALLED: &str = r#"{"id":"ssss3333dddd","short":"stalled1","state":"timeout","reason":"stalled","node":"linux-1","startedAt":1000,"endedAt":2900,"hint":"stopped by the stall guard: no output and no CPU for its whole --stall-timeout; set it with --stall-timeout (e.g. --stall-timeout 2h, or off for a quiet job), or stall_timeout in .k2/compute.toml"}"#;
const B_DONE: &str = r#"{"id":"bbbb2222cccc","short":"bbbb2222","state":"done","node":"n2","exitCode":2,"assignedAt":10.5,"endedAt":75}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn compute_wait_for_each_job() {
    let rig = Rig::new(VERB, "wait");
    let v = vec![
        // covers: compute wait
        case(
            "done at once",
            &["wait", "aaaa1111"],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the progress lines, one per change",
            &["wait", "aaaa1111"],
            &[],
            vec![
                ok(QUEUED),
                ok(QUEUED),
                ok(QUEUED_LINE),
                ok(RUNNING),
                ok(RUNNING),
                ok(DONE),
            ],
            owner_and_port,
            0,
            6,
        ),
        case(
            "held, a pull back, held again, then done",
            &["wait", "aaaa1111"],
            &[],
            vec![ok(QUEUED), ok(PULLING), ok(HELD), ok(HELD), ok(DONE)],
            owner_and_port,
            0,
            5,
        ),
        case(
            "held, --json",
            &["wait", "aaaa1111", "--json"],
            &[],
            vec![ok(HELD), ok(DONE)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "--quiet",
            &["wait", "-q", "aaaa1111"],
            &[],
            vec![ok(QUEUED), ok(RUNNING), ok(DONE)],
            owner_and_port,
            0,
            3,
        ),
        // covers: compute wait --json
        case(
            "two jobs, --json: the first failed code",
            &["wait", "aaaa1111", "bbbb2222", "--json"],
            &[],
            vec![ok(DONE), ok(B_RUNNING), ok(B_DONE)],
            owner_and_port,
            2,
            3,
        ),
        case(
            "killed by a signal",
            &["wait", "j"],
            &[],
            vec![ok(
                r#"{"short":"j","state":"done","signal":9,"reportedLate":{"secs":130}}"#,
            )],
            owner_and_port,
            137,
            1,
        ),
        case(
            "an exit code above 255",
            &["wait", "j"],
            &[],
            vec![ok(r#"{"short":"j","state":"done","exitCode":300}"#)],
            owner_and_port,
            44,
            1,
        ),
        case(
            "failed, with its reason",
            &["wait", "j"],
            &[],
            vec![ok(
                r#"{"id":"jj","short":"j","state":"failed","reason":"out of memory","node":"n"}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "--max-wait 0: still running",
            &["wait", "aaaa1111", "--max-wait", "0"],
            &[],
            vec![ok(RUNNING)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "--timeout 0: still running, 124, the job left alone",
            &["wait", "aaaa1111", "--timeout", "0"],
            &[],
            vec![ok(QUEUED)],
            owner_and_port,
            124,
            1,
        ),
        case(
            "--timeout 0, two jobs, one done, --json: 124",
            &["wait", "aaaa1111", "bbbb2222", "--timeout", "0", "--json"],
            &[],
            vec![ok(DONE), ok(B_RUNNING)],
            owner_and_port,
            124,
            2,
        ),
        case(
            "--timeout, a failed job: 3, not 124",
            &["wait", "j", "--timeout", "30s"],
            &[],
            vec![ok(
                r#"{"id":"jj","short":"j","state":"failed","reason":"spawn_failed","node":"n"}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a stalled job: its hint under its line",
            &["wait", "stalled1"],
            &[],
            vec![ok(STALLED)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a done job's hint that is no string",
            &["wait", "j"],
            &[],
            vec![ok(
                r#"{"short":"j","state":"done","exitCode":0,"node":"n","hint":["a",1,{"b":null}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty hint says nothing",
            &["wait", "j"],
            &[],
            vec![ok(r#"{"short":"j","state":"done","exitCode":0,"hint":""}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "404 is exit 4",
            &["wait", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no such job"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "403",
            &["wait", "j"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "no answer once, then the answer",
            &["wait", "aaaa1111"],
            &[],
            vec![Reply::Close, ok(DONE)],
            owner_and_port,
            0,
            2,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "an answer that is no object",
                &["wait", "j"],
                &[],
                vec![ok("[1]")],
                owner_and_port,
                1,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "an exit code that is no number",
                &["wait", "j"],
                &[],
                vec![ok(r#"{"state":"done","exitCode":"x"}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "over the cell socket",
                &["wait", "aaaa1111"],
                &[],
                vec![ok(RUNNING), ok(DONE)],
                port_only,
                0,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_wait_any() {
    let rig = Rig::new(VERB, "waitany");
    let v = vec![
        case(
            "the second ends first",
            &["wait", "aaaa1111", "bbbb2222", "--any"],
            &[],
            vec![ok(RUNNING), ok(B_DONE)],
            owner_and_port,
            2,
            2,
        ),
        case(
            "a round with none ended",
            &["wait", "--any", "aaaa1111", "bbbb2222"],
            &[],
            vec![ok(RUNNING), ok(B_RUNNING), ok(DONE)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "--quiet --json",
            &["wait", "--any", "-q", "--json", "aaaa1111", "bbbb2222"],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--max-wait 0",
            &["wait", "--any", "--max-wait", "0", "aaaa1111", "bbbb2222"],
            &[],
            vec![ok(RUNNING), ok(B_RUNNING)],
            owner_and_port,
            3,
            2,
        ),
        case(
            "--timeout 0: 124",
            &["wait", "--any", "--timeout", "0", "aaaa1111", "bbbb2222"],
            &[],
            vec![ok(RUNNING), ok(B_RUNNING)],
            owner_and_port,
            124,
            2,
        ),
        case(
            "--timeout, the first ends in time",
            &["wait", "--any", "--timeout", "9m", "aaaa1111", "bbbb2222"],
            &[],
            vec![ok(RUNNING), ok(B_DONE)],
            owner_and_port,
            2,
            2,
        ),
        case(
            "404 is exit 4",
            &["wait", "--any", "x"],
            &[],
            vec![Reply::Answer(404, "{}".into())],
            owner_and_port,
            4,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "an answer that is no object",
                &["wait", "--any", "x"],
                &[],
                vec![ok("7")],
                owner_and_port,
                1,
                1,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_wait_words() {
    let rig = Rig::new(VERB, "waitwords");
    let v = vec![
        case("no job", &["wait"], &[], vec![], owner_and_port, 2, 0),
        case(
            "only flags",
            &["wait", "--json", "-q"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["wait", "a", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--max-wait last",
            &["wait", "a", "--max-wait"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--timeout last",
            &["wait", "a", "--timeout"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--max-wait empty",
            &["wait", "a", "--max-wait", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--timeout with --max-wait",
            &["wait", "a", "--timeout", "1s", "--max-wait", "1s"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--timeout twice",
            &["wait", "a", "--timeout", "1s", "--timeout", "2s"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--timeout twice, with --follow: the words stop it first",
            &[
                "wait",
                "a",
                "--follow",
                "--timeout",
                "1s",
                "--timeout",
                "2s",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("the gate", &["wait", "a"], &[], vec![], nothing, 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_wait_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "waitshell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "--follow with two jobs",
            &["wait", "a", "b", "--follow"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "--failures",
            &["wait", "aaaa1111", "--failures"],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "wait --help",
            &["wait", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        // Help never acts: -h or --help anywhere sends nothing.
        shell(case(
            "-h after the job",
            &["wait", "aaaa1111", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "--help with --any",
            &["wait", "a", "b", "--any", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// TMPDIR names a folder that isn't there. Linux's `mktemp -d -t` fails
/// there, so the binary hands the line to the shell, which says so and
/// ends with 1 before it sends anything. macOS's falls back to the user's
/// own temp folder and the wait goes on, so the binary answers it too.
/// The row has its job's answer either way: with none, the stub's
/// default answer has no state and the wait would never end.
#[test]
fn compute_wait_with_no_temp_folder() {
    let rig = Rig::new(VERB, "waitnotmp");
    let c = if cfg!(target_os = "macos") {
        case(
            "no temp folder: macOS's mktemp falls back",
            &["wait", "aaaa1111"],
            &[],
            vec![ok(DONE)],
            owner_and_port,
            0,
            1,
        )
    } else {
        Case {
            compare: Compare::Shell,
            ..case(
                "no temp folder",
                &["wait", "aaaa1111"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            )
        }
    };
    rig.check(&Case {
        no_tmpdir: true,
        ..c
    });
}

/// The wait's own lines name the command by the brand's name (in a build
/// with `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or
/// `k5`: the two new usage lines and `--timeout`'s still_running.
#[test]
fn the_wait_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "waitbrand");
    let cli = k2_client::brand::CLI_NAME;
    let usage = format!(
        "usage: {cli} compute wait <job>... [--any] [--follow] [--failures] [--timeout 9m] [--quiet] [--json]"
    );
    for (args, want) in [
        (
            &["wait", "a", "--timeout", "1s", "--max-wait", "1s"][..],
            format!("{usage}   (--timeout or --max-wait, once)\n"),
        ),
        (
            &["wait", "a", "--timeout"],
            format!("{usage}   (--timeout takes a duration: 9m, 90s, 1h)\n"),
        ),
        (&["wait", "a", "--bogus"], format!("{usage}\n")),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
    let c = case(
        "brand",
        &["wait", "aaaa1111", "--timeout", "0"],
        &[],
        vec![ok(RUNNING)],
        owner_and_port,
        124,
        1,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!(
            "[{cli} compute] aaaa1111 running on mini\n{{\"error\": \"still_running\", \"jobs\": [\"aaaa1111\"], \"hint\": \"{cli} compute wait aaaa1111\"}}\n"
        )
    );
    assert_eq!(out.code, Some(124));
    assert_eq!(log.len(), 1, "one poll, nothing else sent: {log:?}");
}
