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

/// A job's signal, times and attempt reach the shell's arithmetic only as
/// digits (the shell's a1d64ef741): a signal that isn't digits is none
/// (128), times that aren't digits give no "after", an attempt that isn't
/// digits is "?". Valid ones print as before.
#[test]
fn compute_follow_reads_only_digits() {
    let rig = Rig::new(VERB, "followdigits");
    let job = |signal: &str, started: &str, ended: &str| -> &'static str {
        Box::leak(
            format!(
                r#"{{"state":"done","exitCode":null,"signal":{signal},"short":"abcd1234","node":"n1","startedAt":{started},"endedAt":{ended},"generation":1}}"#
            )
            .into_boxed_str(),
        )
    };
    let follow = |name: &'static str, reply: Vec<Reply>, code: i32| {
        let n = reply.len();
        case(
            name,
            &["wait", "abcd1234", "--follow"],
            &[],
            reply,
            owner_and_port,
            code,
            n,
        )
    };
    let v = vec![
        // native: compute wait --follow
        follow(
            "signal 9, a time",
            vec![logs("", true, job("9", "1000", "1125"), "{}")],
            137,
        ),
        follow(
            "a signal that names a variable",
            vec![logs(
                "",
                true,
                job(r#""BASH_VERSINFO""#, "1000", "1125"),
                "{}",
            )],
            128,
        ),
        follow(
            "a signal with a subscript",
            vec![logs(
                "",
                true,
                job(r#""BASH_VERSINFO[$(true)]""#, "1000", "1125"),
                "{}",
            )],
            128,
        ),
        follow(
            "a start that names a variable",
            vec![logs("", true, job("9", r#""HOME""#, "1125"), "{}")],
            137,
        ),
        follow(
            "an end with a subscript, a negative start",
            vec![logs("", true, job("9", "-5", r#""x[1]""#), "{}")],
            137,
        ),
        follow(
            "an end before the start",
            vec![logs("", true, job("9", "2125", "1125"), "{}")],
            137,
        ),
        follow(
            "an attempt that names a variable",
            vec![
                logs(
                    "",
                    true,
                    r#"{"state":"queued","short":"abcd1234","node":"n2","attempt":"HOME","generation":2,"ctl":{"failedAttempts":[{"reason":"node_lost"}]}}"#,
                    "{}",
                ),
                logs(
                    "",
                    true,
                    &done("0").replace("\"generation\":1", "\"generation\":2"),
                    "{}",
                ),
            ],
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The generation followed ended without ending the job (the shell's
/// 46854e87d1): a move (the attempt record's `moved` words, or from an
/// older daemon an attempt number that stayed) prints `moved: …; now on
/// <node> (not a failure, no retry used)`, never "infrastructure
/// failure"; a retry names its record's end. Whatever the record holds.
#[test]
fn compute_follow_a_move() {
    let rig = Rig::new(VERB, "followmove");
    let first = |rec: &str, attempt: &str| -> Reply {
        ok(&format!(
            r#"{{"chunks":[],"end":true,"cursor":1,"generation":1,"attempts":[{rec},{{"attempt":1,"generation":2,"current":true}}],"job":{{"state":"queued","short":"abcd1234","node":"node-b","attempt":{attempt},"generation":2}}}}"#
        ))
    };
    let after = || {
        ok(
            r#"{"chunks":[],"end":true,"cursor":1,"generation":2,"job":{"state":"done","short":"abcd1234","node":"node-b","exitCode":0,"generation":2}}"#,
        )
    };
    let follow = |name: &'static str, rec: &str, attempt: &str| {
        case(
            name,
            &["wait", "abcd1234", "--follow"],
            &[],
            vec![first(rec, attempt), after()],
            owner_and_port,
            0,
            2,
        )
    };
    let v = vec![
        // native: compute wait --follow
        follow(
            "a move, the daemon's words",
            r#"{"attempt":1,"generation":1,"current":false,"state":"assigned","reason":"withdrawn","moved":"node-a gave it back from its queue before it started for node-b"}"#,
            "1",
        ),
        follow(
            "an unanswered withdraw, words with a quote",
            r#"{"attempt":1,"generation":1,"current":false,"reason":"withdraw_unanswered","moved":"node-a didn't answer a request to give it back within 3 min; K2 took it back"}"#,
            "1",
        ),
        follow(
            "a move, words with a newline, a NUL, a backslash and a dollar",
            r#"{"attempt":1,"generation":1,"moved":"one\ntwo\u0000three \\n $HOME `x`"}"#,
            "1",
        ),
        follow(
            "a move, words that aren't a string",
            r#"{"attempt":1,"generation":1.0,"moved":{"to":"node-b","n":[1,2.5,null,true]}}"#,
            "1",
        ),
        follow(
            "an older daemon: the attempt stayed",
            r#"{"attempt":1,"generation":1,"current":false,"reason":"withdraw_requested","next":"taken back from node-a's queue before it started, to move it to node-b (no retry used)"}"#,
            "1",
        ),
        follow(
            "an older daemon, no next words",
            r#"{"attempt":1,"generation":1,"next":""}"#,
            "1",
        ),
        follow(
            "an older daemon, neither has an attempt",
            r#"{"generation":1}"#,
            "null",
        ),
        follow(
            "a retry with its record",
            r#"{"attempt":1,"generation":1,"current":false,"state":"interrupted","reason":"node_update","nextCode":"retry_backoff","moved":null}"#,
            "2",
        ),
        follow(
            "a retry, moved empty, the record's state",
            r#"{"attempt":1,"generation":1,"state":"lost","moved":""}"#,
            "2",
        ),
        follow(
            "no moved key, the attempt moved on: a retry",
            r#"{"attempt":1,"generation":1,"reason":"oom"}"#,
            "2",
        ),
        follow(
            "the record is still current: the failed attempts' end",
            r#"{"attempt":1,"generation":1,"current":true,"moved":"not this one"}"#,
            "2",
        ),
        follow(
            "no record for this generation",
            r#"{"attempt":1,"generation":7,"moved":"not this one"}"#,
            "1",
        ),
        follow(
            "records that aren't objects",
            r#""text",[1],null,{"attempt":1,"generation":1,"moved":"the third"}"#,
            "1",
        ),
        case(
            "attach: a move",
            &["attach", "abcd"],
            &[],
            vec![
                ok(JOB),
                first(
                    r#"{"attempt":1,"generation":1,"moved":"node-a gave it back"}"#,
                    "1",
                ),
                after(),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "attempts an object: its keys, no record",
            &["wait", "abcd1234", "--follow"],
            &[],
            vec![
                ok(
                    r#"{"chunks":[],"end":true,"cursor":1,"generation":1,"attempts":{"generation":1},"job":{"state":"queued","short":"abcd1234","node":"node-b","attempt":2,"generation":2,"ctl":{"failedAttempts":[{"reason":"node_lost"}]}}}"#,
                ),
                after(),
            ],
            owner_and_port,
            0,
            2,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "attempts a number: Python's raise",
                &["wait", "abcd1234", "--follow"],
                &[],
                vec![ok(
                    r#"{"chunks":[],"end":true,"cursor":1,"generation":1,"attempts":5,"job":{"state":"queued","short":"abcd1234","node":"node-b","attempt":1,"generation":2}}"#,
                )],
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

/// The move line names the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` it says `xy`), never a typed `k2` or `k5`.
#[test]
fn the_move_line_says_the_brand_s_name() {
    let rig = Rig::new(VERB, "movebrand");
    let cli = k2_client::brand::CLI_NAME;
    let c = case(
        "brand",
        &["wait", "abcd1234", "--follow"],
        &[],
        vec![
            ok(
                r#"{"chunks":[],"end":true,"cursor":1,"generation":1,"attempts":[{"attempt":1,"generation":1,"moved":"node-a gave it back"}],"job":{"state":"queued","short":"abcd1234","node":"node-b","attempt":1,"generation":2}}"#,
            ),
            ok(
                r#"{"chunks":[],"end":true,"cursor":1,"generation":2,"job":{"state":"done","short":"abcd1234","node":"node-b","exitCode":0,"generation":2}}"#,
            ),
        ],
        owner_and_port,
        0,
        2,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!(
            "[{cli} compute] moved: node-a gave it back; now on node-b (not a failure, no retry used)\n[{cli} compute] exit 0 \u{b7} {cli} compute logs abcd1234 --failures\n"
        )
    );
    assert_eq!(out.code, Some(0));
    assert_eq!(log.len(), 2, "two polls, nothing else sent: {log:?}");
}
