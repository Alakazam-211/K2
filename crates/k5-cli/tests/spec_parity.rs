//! The spec runner against the shell CLI, on the parity rig
//! ([`common::parity`]): `k5-spec-fixture compute stats …`, the runner over
//! the test entry in `tests/data/spec_fixture.entries` (contract-gen's
//! `src/bin/k5-spec-fixture/table.rs`), and `cli/k2 compute stats …` must
//! send the same requests and print the same stdout and stderr with the
//! same exit code, then the same with no `python3` on `PATH`.
//!
//! - The words: the job, `--json` before or after it, an empty word before
//!   it; the usage line (no job, a short or bad one, two jobs, a flag).
//! - The answers: `text` lines, none, a string, an object; `--json`'s body
//!   as it came (trailing newlines, NULs, not JSON); 400, 404 and 500
//!   (an error object, a string with a message, not JSON, empty); a
//!   refusal with its "Ask with:" line (the ci.preview card's wording too);
//!   the bulletin; Python's raise (not JSON, not an object, an item that is
//!   no string).
//! - The owner from disk, a passport over TCP, a live cell socket, a dead
//!   socket file, a socket that is gone (the stale-passport probe),
//!   `HOOK_SOCK` with no passport; no port; nothing listening; a reset.
//! - What the runner hands to the shell: `--help`, and `_CP_404_EXIT` set.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";
const FIXTURE: &str = env!("CARGO_BIN_EXE_k5-spec-fixture");
const JOB: &str = "j0b-ABCD_12";
const STATS: &str =
    r#"{"job":"j0b-ABCD_12","text":["CPU  2.1 of 8","memory 1.2G é"],"samples":[1,2]}"#;
const USAGE: &str =
    "usage: k2 compute stats <job> [--json]  (a job id or its first 8 characters)\n";

fn rig(tag: &str) -> Rig {
    let mut r = Rig::new(VERB, tag);
    r.bin = FIXTURE;
    r
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

/// A 200 the shell's Python raises on: exit 1, the traceback compared by
/// its exception.
fn raises(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    Case {
        compare: Compare::Traceback,
        ..case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1)
    }
}

fn answer(status: u16, body: &str) -> Vec<Reply> {
    vec![Reply::Answer(status, body.to_string())]
}

#[test]
fn compute_stats_answers_as_the_shell_does() {
    let rig = rig("all");
    let cases = vec![
        ok_case("the text lines", &["stats", JOB], STATS),
        ok_case(
            "--json: the body as it came",
            &["stats", JOB, "--json"],
            "{\"a\": 1}\n\n",
        ),
        ok_case("--json first", &["stats", "--json", JOB], STATS),
        ok_case("an empty word before the job", &["stats", "", JOB], STATS),
        ok_case(
            "--json: not JSON is printed too",
            &["stats", JOB, "--json"],
            "<html>oops</html>",
        ),
        ok_case(
            "no text: the server's line",
            &["stats", JOB],
            r#"{"ok":true}"#,
        ),
        ok_case("an empty text list", &["stats", JOB], r#"{"text":[]}"#),
        ok_case("text as a string", &["stats", JOB], r#"{"text":"ab"}"#),
        ok_case(
            "text as an object",
            &["stats", JOB],
            r#"{"text":{"k":1,"j":2}}"#,
        ),
        ok_case(
            "the bulletin line when it worked",
            &["stats", JOB],
            r#"{"text":["x"],"bulletin":{"day":"2026-11-19","unread":true,"key":"ab12"}}"#,
        ),
        raises(
            "not JSON, the text form",
            &["stats", JOB],
            "<html>oops</html>",
        ),
        raises("an empty 200, the text form", &["stats", JOB], ""),
        raises("not an object", &["stats", JOB], "[1]"),
        raises(
            "an item that is no string",
            &["stats", JOB],
            r#"{"text":["a",2]}"#,
        ),
        case(
            "400: the error object, exit 2",
            &["stats", JOB],
            &[],
            answer(400, r#"{"error":{"code":"bad_job","hint":"a job id é"}}"#),
            owner_and_port,
            2,
            1,
        ),
        case(
            "404: an error string and its message, exit 3",
            &["stats", JOB],
            &[],
            answer(404, r#"{"error":"not_found","message":"no such job"}"#),
            owner_and_port,
            3,
            1,
        ),
        case(
            "500, not JSON",
            &["stats", JOB, "--json"],
            &[],
            answer(500, "<html>\nInternal error\n</html>\n"),
            owner_and_port,
            3,
            1,
        ),
        case(
            "500, empty",
            &["stats", JOB],
            &[],
            answer(500, ""),
            owner_and_port,
            3,
            1,
        ),
        case(
            "a refusal and its Ask with: line",
            &["stats", JOB],
            &[],
            answer(403, PERMISSION_ASK),
            owner_and_port,
            3,
            1,
        ),
        case(
            "the ci.preview card",
            &["stats", JOB, "--json"],
            &[],
            answer(
                403,
                r#"{"error":{"code":"ci_preview_off","hint":"CI preview is off on this server."},"permission":{"command":"k2 permission request ci.preview","requestable":true}}"#,
            ),
            owner_and_port,
            3,
            1,
        ),
        case(
            "no job: the usage line",
            &["stats"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a short job",
            &["stats", "abc"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a job with a dot",
            &["stats", "ab.cd"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "two jobs",
            &["stats", JOB, "abcd"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a flag",
            &["stats", JOB, "-v"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--json alone",
            &["stats", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no port: the connection gate",
            &["stats", JOB],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        Case {
            compare: Compare::NulInAnswer,
            ..ok_case(
                "--json drops a NUL",
                &["stats", JOB, "--json"],
                "{\"a\":\"x\0y\"}",
            )
        },
        case(
            "a reset: the -w lines are the body",
            &["stats", JOB],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in cases {
        rig.check(&c);
    }
}

#[test]
fn compute_stats_with_a_passport() {
    let rig = rig("pass");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let cases = vec![
        Case {
            env: with_passport(&[]),
            ..case(
                "a passport over TCP",
                &["stats", JOB],
                &[],
                vec![ok(STATS)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: cell(),
            ..case(
                "a live cell socket",
                &["stats", JOB, "--json"],
                &[],
                vec![ok(STATS)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: cell(),
            ..case(
                "not served on the cell socket: TCP",
                &["stats", JOB],
                &[],
                vec![
                    Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into()),
                    ok(STATS),
                ],
                port_only,
                0,
                2,
            )
        },
        Case {
            env: cell(),
            ..case(
                "a refusal on the cell socket is the answer",
                &["stats", JOB],
                &[],
                answer(403, PERMISSION_ASK),
                port_only,
                3,
                1,
            )
        },
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{deadsock}")]),
            ..case(
                "a dead socket file: TCP",
                &["stats", JOB],
                &[],
                vec![ok(STATS)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            ..case(
                "a socket that is gone: the probe finds the passport good",
                &["stats", JOB],
                &[],
                vec![ok(r#"{"role":"r"}"#), ok(STATS)],
                port_only,
                0,
                2,
            )
        },
        Case {
            env: vec![("K2_HOOK_SOCK", "{sock}".to_string())],
            ..case(
                "HOOK_SOCK with no passport: the id tool's lines",
                &["stats", JOB],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            )
        },
        Case {
            env: with_passport(&[]),
            ..case(
                "a refused passport: the dead-passport probe",
                &["stats", JOB],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                3,
                2,
            )
        },
    ];
    for c in cases {
        rig.check(&c);
    }
}

/// Nothing accepted the connection: `_compute_send`'s `-w` lines are the
/// body `_compute_fail` reads, exit 3.
#[test]
fn compute_stats_with_no_daemon() {
    let rig = rig("dead");
    let c = Case {
        env: vec![("K2_PORT", dead_port())],
        ..case(
            "nothing listens",
            &["stats", JOB],
            &[],
            vec![],
            owner_only,
            3,
            0,
        )
    };
    rig.check(&c);
}

/// What the runner hands to the shell, before anything is sent.
#[test]
fn compute_stats_forms_the_shell_answers() {
    let rig = rig("shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let cases = vec![
        shell(case(
            "--help",
            &["stats", JOB, "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(Case {
            env: vec![("_CP_404_EXIT", "4".to_string())],
            ..case(
                "_CP_404_EXIT set: _compute_fail would read it",
                &["stats", JOB],
                &[],
                answer(404, r#"{"error":{"code":"x"}}"#),
                owner_and_port,
                4,
                1,
            )
        }),
    ];
    for c in cases {
        rig.check(&c);
    }
}

/// The usage line is bash's, byte for byte.
#[test]
fn the_usage_line_is_bash_s() {
    let rig = rig("usage");
    let c = case("no job", &["stats"], &[], vec![], owner_and_port, 2, 0);
    rig.prepare(Side::Bash, &c);
    let out = rig.run(true, &c);
    assert_eq!(
        (out.code, out.stdout.as_str(), out.stderr.as_str()),
        (Some(2), "", USAGE)
    );
    rig.read(Side::Bash, false, c.name);
}
