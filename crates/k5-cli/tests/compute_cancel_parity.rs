//! `k5-cli compute cancel` against the shell CLI's `cli/k2 compute
//! cancel`, on the parity rig ([`common::parity`]): the same requests,
//! stdout, stderr and exit code.
//!
//! - One job: queued and cancelled, a cancel asked on the node, a started
//!   job stopped with `--force` (its run time), any other state; `--json`.
//! - `--if-queued`: cancelled, and the 409s that name a started or an
//!   ended job (exit 0, with and without `--json`), and one that names
//!   neither (the refusal).
//! - `--all-mine` (a passport: no workspace) and `--workspace-all` (the
//!   owner: this checkout, or `--workspace`), each filter, the
//!   `--state` warning, the skipped jobs; one answer above 256 KiB.
//! - The words: no job, two, both modes, an unknown flag, a bad state,
//!   a flag with no value last (exit 1, nothing said), nothing sent.
//! - The daemon's refusals: 400 and 413 (exit 2), 404 and 409 (exit 3),
//!   one with a permission card (its closing line), an error that is
//!   text, not JSON; the bulletin; Python's raise on an answer it
//!   doesn't expect.
//! - The channels: the cell socket, "not served" there (then TCP), the
//!   id tool's checks, the gates.
//! - Sent once: no answer in time and an empty reply over TCP are the
//!   unreachable line (exit 3); a POST the cell socket read and never
//!   answered is "may have been applied" (exit 1) and never sent again;
//!   nothing listening.
//! - What the binary hands to the shell: help, the shell's own settings.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "compute";
const QUEUED: &str = r#"{"state":"cancelled","was":"queued","node":"mini"}"#;
const STOPPING: &str = r#"{"state":"cancel_requested","was":"running","node":"mini","ranSecs":125,"cancel":"sigterm"}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// A session with a live cell socket.
fn cell(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    e
}

#[test]
fn compute_cancel_one_job() {
    let rig = Rig::new(VERB, "cancel");
    let v = vec![
        // covers: compute cancel
        case("queued: cancelled", &["cancel", "j1234567890ab"], &[], vec![ok(QUEUED)], owner_and_port, 0, 1),
        case("--force, a started job", &["cancel", "--force", "j1"], &[], vec![ok(STOPPING)], owner_and_port, 0, 1),
        case("-f after the id", &["cancel", "j1", "-f"], &[], vec![ok(r#"{"state":"stopping","cancel":"sigkill","ranSecs":-5}"#)], owner_and_port, 0, 1),
        case(
            "a cancel asked on the node",
            &["cancel", "j1"],
            &[],
            vec![ok(r#"{"state":"cancel_requested","onNode":true,"node":"n","cancel":"asked","note":"it stops at its next check"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case("any other state", &["cancel", "j1"], &[], vec![ok(r#"{"state":"done"}"#)], owner_and_port, 0, 1),
        case("no state", &["cancel", "j1"], &[], vec![ok("{}")], owner_and_port, 0, 1),
        // covers: compute cancel --json
        case("--json", &["cancel", "j1", "--json"], &[], vec![ok(QUEUED)], owner_and_port, 0, 1),
        case("a word that needs escaping", &["cancel", "j\"é\\x"], &[], vec![ok(QUEUED)], owner_and_port, 0, 1),
        case("the bulletin", &["cancel", "j1"], &[], vec![ok(r#"{"state":"done","bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#)], owner_and_port, 0, 1),
        with(cell(&[]), case("over the cell socket", &["cancel", "j1"], &[], vec![ok(QUEUED)], port_only, 0, 1)),
        with(
            cell(&[]),
            case(
                "not served on the cell socket: TCP",
                &["cancel", "j1"],
                &[],
                vec![Reply::Answer(404, r#"{"error":"not served on cell socket"}"#.into()), ok(QUEUED)],
                port_only,
                0,
                2,
            ),
        ),
        // --if-queued.
        case("--if-queued, cancelled", &["cancel", "j1", "--if-queued"], &[], vec![ok(QUEUED)], owner_and_port, 0, 1),
        case(
            "--if-queued, already started",
            &["cancel", "--if-queued", "j1234567890"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"job_started","hint":"it runs"}}"#.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--if-queued, already ended",
            &["cancel", "--if-queued", "j1"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"already_ended","hint":"ended 2m ago"}}"#.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--if-queued --json, already ended",
            &["cancel", "--if-queued", "j1", "--json"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"already_ended","hint":"x"}}"#.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--if-queued, another 409",
            &["cancel", "--if-queued", "j1"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"busy","hint":"later"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "--if-queued, a 409 whose error is a string",
            &["cancel", "--if-queued", "j1"],
            &[],
            vec![Reply::Answer(409, r#"{"error":"job_started","code":"job_started"}"#.into())],
            owner_and_port,
            1,
            1,
        )),
        // The daemon's refusals.
        case(
            "409: started, no --force",
            &["cancel", "j1"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"job_started","hint":"k2 compute cancel j1 --force stops it"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case("400", &["cancel", "j1"], &[], vec![Reply::Answer(400, r#"{"error":{"code":"bad_request","hint":"no"}}"#.into())], owner_and_port, 2, 1),
        case("413", &["cancel", "j1"], &[], vec![Reply::Answer(413, r#"{"error":"too_big","message":"too big"}"#.into())], owner_and_port, 2, 1),
        case("404", &["cancel", "j1"], &[], vec![Reply::Answer(404, r#"{"error":{"code":"not_found"}}"#.into())], owner_and_port, 3, 1),
        case("500, text", &["cancel", "j1"], &[], vec![Reply::Answer(500, "it broke\n".into())], owner_and_port, 3, 1),
        case("500, no error", &["cancel", "j1"], &[], vec![Reply::Answer(500, r#"{"x":1}"#.into())], owner_and_port, 3, 1),
        case("503, empty", &["cancel", "j1"], &[], vec![Reply::Answer(503, String::new())], owner_and_port, 3, 1),
        with(
            cell(&[]),
            case("a permission card", &["cancel", "j1"], &[], vec![Reply::Answer(403, PERMISSION_ASK.into())], port_only, 3, 1),
        ),
        traceback(case("not JSON", &["cancel", "j1"], &[], vec![ok("not json")], owner_and_port, 1, 1)),
        traceback(case("a list", &["cancel", "j1"], &[], vec![ok("[1]")], owner_and_port, 1, 1)),
        traceback(case(
            "a run time that is no number",
            &["cancel", "j1"],
            &[],
            vec![ok(r#"{"cancel":"x","ranSecs":"1.5"}"#)],
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
fn compute_cancel_many() {
    let rig = Rig::new(VERB, "cancelmany");
    let many = r#"{"cancelled":[{"job":"a1"},{"job":"b2"}],"matched":3,"skipped":[{"job":"c3","code":"job_started","hint":"use --force"}]}"#;
    let v = vec![
        with(
            cell(&[]),
            case(
                "--all-mine with a passport",
                &["cancel", "--all-mine"],
                &[],
                vec![ok(many)],
                port_only,
                0,
                1,
            ),
        ),
        case(
            "--all-mine as the owner",
            &["cancel", "--all-mine", "--json"],
            &[],
            vec![ok(many)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--workspace-all, every filter",
            &[
                "cancel",
                "--workspace-all",
                "--tag",
                "smoke:any",
                "--state",
                "any",
                "--node",
                "mini",
                "--workspace",
                "acme ws",
                "--force",
            ],
            &[],
            vec![ok(r#"{"cancelled":[],"matched":0}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--state running warns",
            &["cancel", "--workspace-all", "--state", "running"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty --state is queued",
            &["cancel", "--all-mine", "--state", ""],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refused",
            &["cancel", "--all-mine"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "a job that is no string",
            &["cancel", "--all-mine"],
            &[],
            vec![ok(r#"{"cancelled":[{"job":1}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a skipped row that is no object",
            &["cancel", "--all-mine"],
            &[],
            vec![ok(r#"{"skipped":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
    // One answer above 256 KiB: thousands of jobs.
    let rows: Vec<String> = (0..9000)
        .map(|i| format!(r#"{{"job":"job-{i:08}-abcdef0123456789"}}"#))
        .collect();
    let big = format!(
        r#"{{"cancelled":[{}],"matched":9000,"skipped":[]}}"#,
        rows.join(",")
    );
    assert!(big.len() > 256 * 1024);
    let big: &'static str = Box::leak(big.into_boxed_str());
    rig.check(&case(
        "9000 jobs",
        &["cancel", "--all-mine"],
        &[],
        vec![ok(big)],
        owner_and_port,
        0,
        1,
    ));
}

#[test]
fn compute_cancel_words() {
    let rig = Rig::new(VERB, "cancelwords");
    let v = vec![
        case("no job", &["cancel"], &[], vec![], owner_and_port, 2, 0),
        case(
            "an empty job",
            &["cancel", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "two jobs",
            &["cancel", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["cancel", "a", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--if-queued and --force",
            &["cancel", "a", "--if-queued", "--force"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a job and --all-mine",
            &["cancel", "--all-mine", "a"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad state",
            &["cancel", "--all-mine", "--state", "done"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--tag last",
            &["cancel", "a", "--tag"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("the gate", &["cancel", "a"], &[], vec![], nothing, 1, 0),
        case(
            "a session with no passport",
            &["cancel", "a"],
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

#[test]
fn compute_cancel_with_no_answer() {
    let rig = Rig::new(VERB, "cancelnone");
    let dead = dead_port();
    let v = vec![
        case(
            "nothing listens",
            &["cancel", "j1"],
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ),
        case(
            "no answer in time over TCP",
            &["cancel", "j1"],
            &[("K2_CP_MAX_TIME", "1")],
            vec![Reply::Hang(Duration::from_secs(3))],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an empty reply over TCP",
            &["cancel", "j1"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        Case {
            compare: Compare::Waited,
            ..with(
                cell(&[("K2_CP_MAX_TIME", "1")]),
                case(
                    "the cell socket read it and never answered: not sent again",
                    &["cancel", "j1"],
                    &[],
                    vec![Reply::Hang(Duration::from_secs(3))],
                    port_only,
                    1,
                    1,
                ),
            )
        },
        // The line counts whole seconds of bash's `$SECONDS`, which may
        // tick on a loaded host: one apart is the same line.
        Case {
            compare: Compare::Waited,
            ..with(
                cell(&[]),
                case(
                    "the cell socket closed with nothing: not sent again",
                    &["cancel", "--all-mine"],
                    &[],
                    vec![Reply::Close],
                    port_only,
                    1,
                    1,
                ),
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `_CP_404_EXIT` exported: nothing reads it any more (`_compute_fail`
/// takes its exit for a 404 from the caller), so the binary answers and a
/// 404 is cancel's own 3.
#[test]
fn compute_cancel_an_exported_404_exit_changes_nothing() {
    let rig = Rig::new(VERB, "cancel404");
    rig.check(&case(
        "_CP_404_EXIT set: nothing reads it",
        &["cancel", "j1"],
        &[("_CP_404_EXIT", "4")],
        vec![Reply::Answer(404, "{}".into())],
        owner_and_port,
        3,
        1,
    ));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn compute_cancel_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "cancelshell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "cancel --help",
            &["cancel", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        // Help never acts: -h or --help anywhere sends nothing.
        shell(case(
            "-h after the job",
            &["cancel", "j1", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "--help with --force",
            &["cancel", "--force", "j1", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "-h with --all-mine",
            &["cancel", "--all-mine", "--tag", "t", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "a K2_CJ_ variable",
            &["cancel", "j1"],
            &[("K2_CJ_extra", "x")],
            vec![ok(QUEUED)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "K2_CP_MAX_TIME of 0",
            &["cancel", "j1"],
            &[("K2_CP_MAX_TIME", "0")],
            vec![ok(QUEUED)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "a port that is no port",
            &["cancel", "j1"],
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
