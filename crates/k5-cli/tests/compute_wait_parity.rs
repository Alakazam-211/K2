//! `k5-cli compute wait` against the shell CLI's `cli/k2 compute wait`,
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - One job and several: done at once, the progress lines as the state
//!   or the queue changes (one per change, none with `--quiet`), each way
//!   a job ends (its exit code, a signal, failed with its reason, a code
//!   above 255), `--json`, `--max-wait 0` (still running, exit 3).
//! - `--any`: the first to end, the round with none ended, `--max-wait`,
//!   `--json`.
//! - No answer once, then the answer (the lost line, 10 s); the daemon's
//!   refusals (404 is exit 4); Python's raise; the cell socket.
//! - The words: none, an unknown flag, `--max-wait` last.
//! - What the binary hands to the shell: `--follow`, `--failures`, a
//!   duration that is none, no temp folder, help.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";
const DONE: &str = r#"{"id":"aaaa1111bbbb","short":"aaaa1111","state":"done","node":"mini","exitCode":0,"startedAt":1000,"endedAt":1125}"#;
const RUNNING: &str = r#"{"id":"aaaa1111bbbb","short":"aaaa1111","state":"running","node":"mini"}"#;
const QUEUED: &str = r#"{"id":"aaaa1111bbbb","short":"aaaa1111","state":"queued","node":"mini","queue":{"position":2,"of":3,"reason":"cpu"}}"#;
const QUEUED_LINE: &str = r#"{"short":"aaaa1111","state":"queued","queue":{"line":"queued 1st on mini","position":1,"waitReason":"slot"},"wait":{"elsewhere":"linux-2 can start it now"}}"#;
const B_RUNNING: &str = r#"{"id":"bbbb2222cccc","short":"bbbb2222","state":"running","node":"n2"}"#;
const B_DONE: &str = r#"{"id":"bbbb2222cccc","short":"bbbb2222","state":"done","node":"n2","exitCode":2,"assignedAt":10.5,"endedAt":75}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn compute_wait_for_each_job() {
    let rig = Rig::new(VERB, "wait");
    let v = vec![
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
            "--quiet",
            &["wait", "-q", "aaaa1111"],
            &[],
            vec![ok(QUEUED), ok(RUNNING), ok(DONE)],
            owner_and_port,
            0,
            3,
        ),
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
            1,
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
            "a duration that is none",
            &["wait", "a", "--max-wait", "5x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
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
        shell(Case {
            no_tmpdir: true,
            ..case(
                "no temp folder",
                &["wait", "a"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            )
        }),
    ];
    for c in v {
        rig.check(&c);
    }
}
