//! `k5-cli compute prune|pin|unpin|retention` and the store line of
//! `compute status`, against the shell CLI's `cli/k2`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `prune` (one POST over TCP: `olderThan`, `dryRun` a JSON bool),
//!   `pin` and `unpin` (`{job}`), `retention` (a GET) and `retention set`
//!   (`{maxAge, cap}`): the daemon's `text` lines, or its whole answer
//!   with `--json`; one large answer each for `prune` and `retention`.
//! - The refusals: the daemon's error (owner_only: exit 3; a 400: exit
//!   2), a permission card, no answer, nothing listening; Python's raise
//!   on an answer the reader can't read.
//! - The words: usage lines (exit 2) and a value flag last (`_need_value`:
//!   exit 2); nothing is sent after either. A `--` is refused before
//!   anything runs.
//! - Help never acts: `-h` or `--help` anywhere prints help and sends
//!   nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";

/// A case whose Python raises.
fn raises(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// A case run in a session (the passport, the hook socket set).
fn in_session(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: port_only,
        ..c
    }
}

const OWNER_ONLY: &str = r#"{"error":{"code":"owner_only","hint":"only the server's owner (or an admin, where noted) can do this"}}"#;
const PRUNED: &str = r#"{"dryRun":true,"removed":{"units":2,"bytes":3},"kept":{"pinned":1},"text":["would remove 2 job(s)' files: 3 B","kept: 1 pinned"]}"#;

/// An answer of `n` text lines (above 256 KiB for a few thousand).
fn many_lines(n: usize, line: &str) -> String {
    let lines: Vec<String> = (0..n)
        .map(|i| format!(r#""{line} {i:06} (only copy, kept until 2026-12-01)""#))
        .collect();
    format!(r#"{{"items":{n},"text":[{}]}}"#, lines.join(","))
}

#[test]
fn compute_prune() {
    let rig = Rig::new(VERB, "prune");
    let v = vec![
        // covers: compute prune
        case(
            "a dry run",
            &["prune", "--dry-run"],
            &[],
            vec![ok(PRUNED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "older than 3d",
            &["prune", "--older-than", "3d"],
            &[],
            vec![ok(r#"{"text":["removed 2 job(s)' files: 3 B"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "-n, the last --older-than counts, 0",
            &["prune", "--older-than", "1d12h", "-n", "--older-than", "0"],
            &[],
            vec![ok(PRUNED)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute prune --json
        case(
            "--json, the whole answer",
            &["prune", "--json"],
            &[],
            vec![ok(PRUNED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no text in the answer",
            &["prune"],
            &[],
            vec![ok(r#"{"removed":{"units":0}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the daemon's owner_only",
            &["prune", "--dry-run"],
            &[],
            vec![Reply::Answer(403, OWNER_ONLY.into())],
            owner_and_port,
            3,
            1,
        ),
        in_session(case(
            "a permission card",
            &["prune"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            port_only,
            3,
            1,
        )),
        case(
            "an empty reply",
            &["prune"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        raises(case(
            "an answer that is no JSON",
            &["prune"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        raises(case(
            "text that is no list",
            &["prune"],
            &[],
            vec![ok(r#"{"text":7}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "--older-than last",
            &["prune", "--older-than"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--older-than a word",
            &["prune", "--older-than", "soon"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--older-than empty",
            &["prune", "--older-than", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["prune", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a word",
            &["prune", "now"],
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
    // One answer above 256 KiB: thousands of lines.
    let big = many_lines(6000, "removed the files of job");
    assert!(big.len() > 256 * 1024);
    rig.check(&case(
        "6000 lines",
        &["prune", "--older-than", "0"],
        &[],
        vec![ok(&big)],
        owner_and_port,
        0,
        1,
    ));
}

#[test]
fn compute_pin_and_unpin() {
    let rig = Rig::new(VERB, "pin");
    let pinned = r#"{"job":"abcd1234","pinned":true,"here":3,"text":["pinned abcd1234: 3 of its files are here"]}"#;
    let v = vec![
        // covers: compute pin
        case(
            "pin",
            &["pin", "abcd1234"],
            &[],
            vec![ok(pinned)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute pin --json
        case(
            "pin --json first",
            &["pin", "--json", "abcd1234"],
            &[],
            vec![ok(pinned)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute unpin
        case(
            "unpin",
            &["unpin", "abcd1234"],
            &[],
            vec![ok(r#"{"job":"abcd1234","pinned":false,"text":["unpinned abcd1234"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute unpin --json
        case(
            "unpin --json",
            &["unpin", "abcd1234", "--json"],
            &[],
            vec![ok(r#"{"job":"abcd1234","pinned":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty word, then the job",
            &["pin", "", "j1"],
            &[],
            vec![ok(pinned)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a job that is no ASCII",
            &["pin", "jé-1"],
            &[],
            vec![ok(r#"{"text":["pinned jé-1"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the daemon's owner_only",
            &["pin", "lockedjob"],
            &[],
            vec![Reply::Answer(403, OWNER_ONLY.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "no such job",
            &["unpin", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no job nope"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "no job",
            &["pin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--json only",
            &["unpin", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "two jobs",
            &["pin", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["unpin", "--bogus", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a lone dash",
            &["pin", "-"],
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

#[test]
fn compute_retention() {
    let rig = Rig::new(VERB, "retention");
    let shown = r#"{"bytes":39728447488,"policy":{"maxAgeSecs":604800,"capBytes":null},"pins":["abcd1234"],"text":["compute store: 37.0 GB (artifacts 36.5 GB in 40 folders, blobs 512 MB in 2 files) · keep 7 d after a job ends · cap 20.0 GB (auto)","pinned: abcd1234"]}"#;
    let set = r#"{"policy":{"maxAgeSecs":1209600},"text":["compute store retention: keep 14d · cap 30G"]}"#;
    let v = vec![
        // covers: compute retention
        case(
            "the store and the rule",
            &["retention"],
            &[],
            vec![ok(shown)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute retention --json
        case(
            "--json",
            &["retention", "--json"],
            &[],
            vec![ok(shown)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute retention set
        case(
            "set both",
            &["retention", "set", "--max-age", "14d", "--cap", "30G"],
            &[],
            vec![ok(set)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "set the defaults, the last value counts",
            &[
                "retention",
                "set",
                "--cap",
                "1T",
                "--max-age",
                "default",
                "--cap",
                "auto",
            ],
            &[],
            vec![ok(set)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute retention set --json
        case(
            "set --json",
            &["retention", "set", "--json", "--cap", "auto"],
            &[],
            vec![ok(set)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a cap the daemon refuses",
            &["retention", "set", "--cap", "1K"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_request","hint":"--cap is at least 1G"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        in_session(case(
            "a permission card",
            &["retention"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            port_only,
            3,
            1,
        )),
        raises(case(
            "lines that are no strings",
            &["retention"],
            &[],
            vec![ok(r#"{"text":[1,2]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "set with nothing",
            &["retention", "set"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set with an empty value",
            &["retention", "set", "--max-age", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--max-age without set",
            &["retention", "--max-age", "3d"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--cap without set, last",
            &["retention", "--cap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set, --cap last",
            &["retention", "set", "--cap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set, an unknown flag",
            &["retention", "set", "--what", "1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set after --json",
            &["retention", "--json", "set"],
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
    // One answer above 256 KiB: thousands of only copies.
    let big = many_lines(6000, "job");
    assert!(big.len() > 256 * 1024);
    rig.check(&case(
        "6000 only copies",
        &["retention"],
        &[],
        vec![ok(&big)],
        owner_and_port,
        0,
        1,
    ));
}

/// `compute status` prints the store line under the controller's.
#[test]
fn compute_status_store_line() {
    let rig = Rig::new(VERB, "storeline");
    let v = vec![
        case(
            "the store line",
            &["status"],
            &[],
            vec![ok(
                r#"{"nodes":[],"controllerMachine":{"name":"ctl","diskFreeBytes":34359738368,"diskTotalBytes":549755813888,"store":{"line":"compute store: 2.0 GB (artifacts 1.5 GB in 3 folders, blobs 512 MB in 2 files) · keep 7 d after a job ends · cap 20.0 GB"}}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        raises(case(
            "a store line that is no string",
            &["status"],
            &[],
            vec![ok(
                r#"{"nodes":[],"controllerMachine":{"name":"ctl","store":{"line":5}}}"#,
            )],
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
fn compute_store_nothing_listens() {
    let rig = Rig::new(VERB, "storenone");
    let dead = dead_port();
    const LINES: &[&[&str]] = &[
        &["prune", "--dry-run"],
        &["pin", "j1"],
        &["unpin", "j1"],
        &["retention"],
        &["retention", "set", "--cap", "auto"],
    ];
    for args in LINES {
        rig.check(&case(
            "nothing listens",
            args,
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ));
    }
}

/// A `--` on these lines is refused before anything runs: none of them
/// takes one.
#[test]
fn compute_store_dashdash() {
    let rig = Rig::new(VERB, "storedd");
    const LINES: &[&[&str]] = &[
        &["prune", "--", "--dry-run"],
        &["prune", "--", "-h"],
        &["pin", "j1", "--"],
        &["unpin", "--", "j1"],
        &["retention", "--"],
        &["retention", "set", "--cap", "auto", "--", "-h"],
    ];
    for args in LINES {
        rig.check(&case("a --", args, &[], vec![], owner_and_port, 2, 0));
    }
}

/// Help never acts: -h or --help anywhere prints help and sends nothing
/// (the binary hands it to the shell, whose `_wants_help` answers first).
#[test]
fn compute_store_help_never_acts() {
    let rig = Rig::new(VERB, "storehelp");
    let lines: &[&[&str]] = &[
        &["prune", "--help"],
        &["prune", "-h"],
        &["prune", "--older-than", "0", "--help"],
        &["prune", "--older-than", "-h"],
        &["prune", "--dry-run", "-h"],
        &["pin", "j1", "-h"],
        &["pin", "--help"],
        &["unpin", "--help", "j1"],
        &["unpin", "-h"],
        &["retention", "-h"],
        &["retention", "--help"],
        &["retention", "set", "--help"],
        &["retention", "set", "--max-age", "14d", "-h"],
        &["retention", "set", "--cap", "--help"],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
