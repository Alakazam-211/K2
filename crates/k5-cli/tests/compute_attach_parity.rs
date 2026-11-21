//! `k5-cli compute attach` against the shell CLI's `cli/k2 compute attach`
//! (`_compute_follow`), on the parity rig ([`common::parity`]): the same
//! requests (the job, then each poll of its logs), stdout, stderr and exit
//! code.
//!
//! - A job that ends `done`: its exit code, its output with escapes taken
//!   out (or kept with `--raw`), from `--since`.
//! - Queued first (a line for each change of place), a retried attempt
//!   followed into its new generation, killed by a signal, failed.
//! - No answer once (the lost and reattached lines), refusals, a job not
//!   found, the words' errors, Python's raise on a chunk.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";
const JOB: &str = r#"{"id":"abcd1234-0000-4000-8000-000000000001","short":"abcd1234"}"#;
const RED: &str = "G1szMW1yZWQbWzBtIGxpbmUNCg==";
const WARN: &str = "d2FybgcgdHdvCg==";
const BUILD: &str = "YnVpbGRpbmcK";

fn logs(chunks: &str, end: bool, job: &str, queue: &str) -> Reply {
    ok(&format!(
        r#"{{"cursor":"c:{n}","end":{end},"generation":1,"chunks":[{chunks}],"job":{job},"queue":{queue}}}"#,
        n = chunks.len()
    ))
}

fn done(code: &str) -> String {
    format!(
        r#"{{"state":"done","exitCode":{code},"signal":null,"short":"abcd1234","node":"linux-1","startedAt":1000,"endedAt":1125,"generation":1,"attempt":1}}"#
    )
}

#[test]
fn compute_attach() {
    let rig = Rig::new(VERB, "attach");
    let out_err = format!(
        r#"{{"seq":1,"stream":"out","data":"{RED}"}},{{"seq":2,"stream":"err","data":"{WARN}"}}"#
    );
    let queued =
        r#"{"state":"queued","short":"abcd1234","node":"mini","generation":1,"attempt":1}"#;
    let v = vec![
        // covers: compute attach
        case(
            "done, exit 7",
            &["attach", "abcd"],
            &[],
            vec![ok(JOB), logs(&out_err, true, &done("7"), "null")],
            owner_and_port,
            7,
            2,
        ),
        case(
            "--raw, from --since",
            &["attach", "--raw", "abcd", "--since", "12"],
            &[],
            vec![ok(JOB), logs(&out_err, true, &done("0"), "{}")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "queued, then running, then done",
            &["attach", "abcd"],
            &[],
            vec![
                ok(JOB),
                logs(
                    "",
                    false,
                    queued,
                    r#"{"position":2,"waitReason":"busy","line":"waits on mini: 2 of 3\n(busy)"}"#,
                ),
                logs(
                    "",
                    false,
                    queued,
                    r#"{"position":2,"waitReason":"busy","line":"same"}"#,
                ),
                logs("", false, queued, r#"{"position":1,"reason":"busy"}"#),
                logs(
                    &format!(r#"{{"seq":1,"stream":"out","data":"{BUILD}"}}"#),
                    true,
                    &done("0"),
                    "{}",
                ),
            ],
            owner_and_port,
            0,
            5,
        ),
        case(
            "a retried attempt",
            &["attach", "abcd"],
            &[],
            vec![
                ok(JOB),
                logs(
                    "",
                    true,
                    r#"{"state":"queued","short":"abcd1234","node":"big","generation":2,"attempt":2,"ctl":{"failedAttempts":[{"state":"lost"},{"reason":"node_lost"}]}}"#,
                    "{}",
                ),
                logs(
                    "",
                    true,
                    &done("0").replace("\"generation\":1", "\"generation\":2"),
                    "{}",
                ),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "killed by a signal",
            &["attach", "abcd"],
            &[],
            vec![
                ok(JOB),
                logs(
                    "",
                    true,
                    r#"{"state":"done","exitCode":null,"signal":9,"short":"abcd1234"}"#,
                    "{}",
                ),
            ],
            owner_and_port,
            137,
            2,
        ),
        case(
            "failed",
            &["attach", "abcd"],
            &[],
            vec![
                ok(JOB),
                logs(
                    "",
                    true,
                    r#"{"state":"failed","reason":"oom","detail":"out of memory","short":"abcd1234","startedAt":10,"endedAt":75}"#,
                    "{}",
                ),
            ],
            owner_and_port,
            3,
            2,
        ),
        case(
            "lost once",
            &["attach", "abcd"],
            &[],
            vec![
                ok(JOB),
                Reply::Close,
                logs(&out_err, true, &done("0"), "{}"),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "the job not found",
            &["attach", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"job_not_found","hint":"no such job"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the logs refused",
            &["attach", "abcd"],
            &[],
            vec![ok(JOB), Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            2,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case(
                "in a session, on the cell socket",
                &["attach", "abcd"],
                &[],
                vec![ok(JOB), logs(&out_err, true, &done("3"), "{}")],
                port_only,
                3,
                2,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "a chunk with no data",
                &["attach", "abcd"],
                &[],
                vec![
                    ok(JOB),
                    logs(
                        r#"{"seq":1,"stream":"out","data":5}"#,
                        true,
                        &done("0"),
                        "{}",
                    ),
                ],
                owner_and_port,
                1,
                2,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "an answer with no id",
                &["attach", "abcd"],
                &[],
                vec![ok(r#"{"short":"abcd1234"}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        case(
            "no job",
            &["attach", "--raw"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--since last",
            &["attach", "abcd", "--since"],
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

/// `compute wait <job> --follow`: `_compute_follow <job> 0 0 0`, from the
/// start (cursor 0), as attach follows: the output, the end line and its
/// exit; a limit that has passed is still_running with 124 for
/// `--timeout`, 3 for `--max-wait`; more than one job, `--json` or
/// `--quiet` is the usage line.
#[test]
fn compute_wait_follow() {
    let rig = Rig::new(VERB, "waitfollow");
    let out_err = format!(
        r#"{{"seq":1,"stream":"out","data":"{RED}"}},{{"seq":2,"stream":"err","data":"{WARN}"}}"#
    );
    let v = vec![
        // native: compute wait --follow
        case(
            "wait --follow: done, exit 7",
            &["wait", "abcd1234", "--follow"],
            &[],
            vec![logs(&out_err, true, &done("7"), "null")],
            owner_and_port,
            7,
            1,
        ),
        case(
            "wait -f, a generous limit: done, exit 0",
            &["wait", "-f", "abcd1234", "--timeout", "1h"],
            &[],
            vec![logs(&out_err, true, &done("0"), "{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "wait --follow --timeout 0: still running, 124",
            &["wait", "abcd1234", "--follow", "--timeout", "0"],
            &[],
            vec![],
            owner_and_port,
            124,
            0,
        ),
        case(
            "wait --follow --max-wait 0: still running, 3",
            &["wait", "abcd1234", "--follow", "--max-wait", "0"],
            &[],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "wait --follow, two jobs: the usage",
            &["wait", "a", "b", "--follow"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "wait --follow --json: the usage",
            &["wait", "a", "--follow", "--json"],
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
