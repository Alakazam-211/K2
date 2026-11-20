//! `k5-cli compute fetch` against the shell CLI's `cli/k2 compute fetch`,
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr, exit code and files.
//!
//! - Every kept file, or the named ones, into `.` or `--out`; a file in
//!   two pieces; one the server still pulls from the node (its line, then
//!   again); `--json`; `--attempt` and `--over-relay` in the requests;
//!   none kept.
//! - The refusals: a name the job didn't keep (with one answer above
//!   256 KiB: thousands of kept names in the hint), a name that would
//!   leave the folder, a file whose sha256 isn't the node's, the daemon's
//!   error; Python's raise (exit 3).
//! - The words: no job, a bad attempt, an unknown flag, `--out` last or
//!   empty.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";
const HELP: &[&[&str]] = &[
    &["fetch", "j1", "-h"],
    &["fetch", "--help", "j1", "b.txt"],
    &["fetch", "j1", "--out", "d", "--json", "-h"],
];
const HELLO_SHA: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const LIST: &str = r#"{"short":"j1","artifacts":[{"name":"d/a.txt"},{"name":"b.txt"}]}"#;

fn hello(name: &str) -> Reply {
    ok(&format!(
        r#"{{"name":"{name}","offset":0,"eof":true,"data":"aGVsbG8=","sha256":"{HELLO_SHA}","bytes":5}}"#
    ))
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn compute_fetch_files() {
    let rig = Rig::new(VERB, "fetch");
    let v = vec![
        // covers: compute fetch
        case(
            "every kept file",
            &["fetch", "j1"],
            &[],
            vec![ok(LIST), hello("b.txt"), hello("d/a.txt")],
            owner_and_port,
            0,
            3,
        ),
        // covers: compute fetch --json
        case(
            "one named, --out, --json",
            &["fetch", "j1", "d/a.txt", "--out", "art/x", "--json"],
            &[],
            vec![ok(LIST), hello("d/a.txt")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "--attempt and --over-relay",
            &[
                "fetch",
                "--attempt",
                "2",
                "--over-relay",
                "j1",
                "b.txt",
                "-o",
                "art2/",
            ],
            &[],
            vec![ok(LIST), hello("b.txt")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "two pieces",
            &["fetch", "j1", "b.txt", "--out", "two"],
            &[],
            vec![
                ok(LIST),
                ok(&format!(
                    r#"{{"name":"b.txt","offset":0,"eof":false,"data":"aGVs","sha256":"{HELLO_SHA}","bytes":5}}"#
                )),
                ok(&format!(
                    r#"{{"name":"b.txt","offset":3,"eof":true,"data":"bG8=","sha256":"{HELLO_SHA}","bytes":5}}"#
                )),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "still on its way from the node",
            &["fetch", "j1", "b.txt", "--out", "pend"],
            &[],
            vec![
                ok(LIST),
                ok(r#"{"pending":true,"name":"b.txt","got":3145728,"bytes":10485760}"#),
                hello("b.txt"),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "none kept",
            &["fetch", "j1", "--out", "none"],
            &[],
            vec![ok(r#"{"short":"j1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a name the job didn't keep",
            &["fetch", "j1", "nope"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a name that would leave the folder",
            &["fetch", "j1", "--out", "unsafe"],
            &[],
            vec![ok(r#"{"artifacts":[{"name":"../x"}]}"#), hello("../x")],
            owner_and_port,
            3,
            2,
        ),
        case(
            "not the node's sha256",
            &["fetch", "j1", "b.txt", "--out", "changed"],
            &[],
            vec![
                ok(LIST),
                ok(
                    r#"{"name":"b.txt","offset":0,"eof":true,"data":"dHdv","sha256":"ffff","bytes":3}"#,
                ),
            ],
            owner_and_port,
            3,
            2,
        ),
        case(
            "404",
            &["fetch", "j1"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no job"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "a list that is no object",
            &["fetch", "j1"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            3,
            1,
        )),
        traceback(case(
            "data that is no base64",
            &["fetch", "j1", "b.txt", "--out", "bad64"],
            &[],
            vec![
                ok(LIST),
                ok(
                    r#"{"name":"b.txt","offset":0,"eof":true,"data":"aGVsbG8","sha256":"x","bytes":5}"#,
                ),
            ],
            owner_and_port,
            3,
            2,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
    // One answer above 256 KiB: thousands of kept names in the hint.
    let names: Vec<String> = (0..9000)
        .map(|i| format!(r#"{{"name":"out/file-{i:06}.log"}}"#))
        .collect();
    let big = format!(r#"{{"short":"j1","artifacts":[{}]}}"#, names.join(","));
    assert!(big.len() > 256 * 1024);
    rig.check(&case(
        "9000 kept, one missing",
        &["fetch", "j1", "nope"],
        &[],
        vec![ok(&big)],
        owner_and_port,
        3,
        1,
    ));
}

#[test]
fn compute_fetch_words() {
    let rig = Rig::new(VERB, "fetchwords");
    let v = vec![
        case("no job", &["fetch"], &[], vec![], owner_and_port, 2, 0),
        case(
            "attempt 0",
            &["fetch", "j", "--attempt", "0"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "attempt x",
            &["fetch", "j", "--attempt", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--attempt last",
            &["fetch", "j", "--attempt"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "an unknown flag",
            &["fetch", "j", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--out empty",
            &["fetch", "j", "--out", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--out last",
            &["fetch", "j", "--out"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: -h or --help anywhere prints help and sends nothing
/// (the binary hands it to the shell, whose `_wants_help` answers first).
#[test]
fn compute_fetch_help_never_acts() {
    let rig = Rig::new(VERB, "fetchhelp");
    for args in HELP {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
