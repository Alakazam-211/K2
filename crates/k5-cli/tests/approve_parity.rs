//! `k5-cli approve` against the shell CLI's, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - The pages that need no daemon: the help, `verify`, `show` and
//!   `key-id` (the `k2-approve` program), and the virtual desktop's guard,
//!   which still stops them.
//! - `request`: the payload in base64, `--to`, `--json`, `--wait` until an
//!   answer with the record written to `--out`, a declined answer, and the
//!   stops before any request.
//! - `status` (one, a list, none, one list above 256 KiB, `--out`),
//!   `cancel`, `agents` (list and set), and an unknown verb.
//! - A refusal with its "Ask with:" line, an answer the reader can't read,
//!   no answer, and the cell socket of a session.
//! - Help never acts: `-h` and `--help` are the shell's; on `cancel` and
//!   `agents` it sends the request (a bash bug K2 fixes): those rows say
//!   so, and turn when the fix lands.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::os::unix::fs::PermissionsExt;

const ASKED: &str = r#"{"id":"ap_1","state":"waiting","kind":"release","person":"ada","answerBy":1763600000}"#;
const APPROVED: &str = r#"{"id":"ap_1","state":"approved","kind":"release","person":"ada","answerBy":1763600000,"device":"Ada's phone","recordText":"K2-APPROVAL-RECORD\nsig\n"}"#;
const DECLINED: &str = r#"{"id":"ap_1","state":"declined","kind":"release","person":"ada","answerBy":0,"device":"phone"}"#;

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn payload(rig: &Rig) {
    owner_and_port(rig);
    std::fs::write(
        rig.proj.join("payload.bin"),
        b"K2-SIGNED-APPROVAL-1\0{\"action\":\"release \xc3\xa9\"}",
    )
    .unwrap();
}

fn approve_bin(rig: &Rig) {
    let p = rig.secrets.join("k2-approve");
    std::fs::write(&p, "#!/bin/sh\necho \"k2-approve: $*\"\nexit 4\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn one(name: &'static str, args: &'static [&'static str], body: &str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

#[test]
fn the_pages_that_need_no_daemon() {
    let rig = Rig::new("approve", "local");
    let v = vec![
        // covers: approve
        case("no verb: the help", &[], &[], vec![], nothing, 0, 0),
        case("help", &["help"], &[], vec![], nothing, 0, 0),
        case("an empty verb", &[""], &[], vec![], nothing, 0, 0),
        case(
            "verify: k2-approve runs in its place",
            &["verify", "rec.txt", "--payload", "p b"],
            &[("K2_APPROVE_BIN", "{secrets}/k2-approve")],
            vec![],
            approve_bin,
            4,
            0,
        ),
        case(
            "key-id",
            &["key-id"],
            &[("K5_APPROVE_BIN", "{secrets}/k2-approve")],
            vec![],
            approve_bin,
            4,
            0,
        ),
        case(
            "show: not a program",
            &["show", "x"],
            &[("K2_APPROVE_BIN", "{secrets}/nothing")],
            vec![],
            nothing,
            2,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            port_only,
            3,
            0,
        ),
        case(
            "status needs the daemon",
            &["status"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        stop("an unknown verb: its line and the help", &["sign"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn request_answers_as_the_shell_does() {
    let rig = Rig::new("approve", "request");
    let req = |name, args, replies: Vec<Reply>, code, n| case(name, args, &[], replies, payload, code, n);
    let v = vec![
        req(
            "request",
            &["request", "--payload", "payload.bin"],
            vec![ok(ASKED)],
            0,
            1,
        ),
        req(
            "request --to --json",
            &["request", "--to", "bé", "--payload", "payload.bin", "--json"],
            vec![ok(ASKED)],
            0,
            1,
        ),
        req(
            "request --wait: waiting, then approved, the record written",
            &["request", "--payload", "payload.bin", "--wait", "--out", "rec.txt"],
            vec![ok(ASKED), ok(r#"{"state":"waiting"}"#), ok(APPROVED)],
            0,
            3,
        ),
        req(
            "request --wait --json: declined",
            &["request", "--payload", "payload.bin", "--wait", "--json"],
            vec![ok(ASKED), ok(DECLINED)],
            1,
            2,
        ),
        req(
            "request --wait --out: no record",
            &["request", "--payload", "payload.bin", "--wait", "--out", "rec.txt"],
            vec![ok(ASKED), ok(r#"{"id":"ap_1","state":"approved"}"#)],
            1,
            2,
        ),
        traceback(req(
            "request --wait --out: a folder that isn't there",
            &["request", "--payload", "payload.bin", "--wait", "--out", "no/rec.txt"],
            vec![ok(ASKED), ok(APPROVED)],
            1,
            2,
        )),
        req("no payload file", &["request", "--payload", "none.bin"], vec![], 2, 0),
        req(
            "--out with no --wait",
            &["request", "--payload", "payload.bin", "--out", "x"],
            vec![],
            2,
            0,
        ),
        req("--payload last: shift fails", &["request", "--payload"], vec![], 1, 0),
        req("no --payload", &["request", "--wait"], vec![], 2, 0),
        req("an unexpected word", &["request", "payload.bin"], vec![], 2, 0),
        req(
            "a refusal: the Ask with: line",
            &["request", "--payload", "payload.bin"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
            1,
        ),
        req(
            "a bad request",
            &["request", "--payload", "payload.bin"],
            vec![Reply::Answer(400, r#"{"error":"bad_payload","message":"answer_by is past"}"#.into())],
            2,
            1,
        ),
        traceback(req(
            "an answer with no id",
            &["request", "--payload", "payload.bin"],
            vec![ok(r#"{"state":"waiting"}"#)],
            1,
            1,
        )),
        req(
            "no answer",
            &["request", "--payload", "payload.bin"],
            vec![Reply::Close],
            3,
            1,
        ),
        // Help never acts: the shell's, and it sends nothing here.
        shell(req(
            "request -h: the help",
            &["request", "--payload", "payload.bin", "-h"],
            vec![],
            0,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn status_cancel_and_agents_answer_as_the_shell_does() {
    let rig = Rig::new("approve", "status");
    let many = {
        let rows: Vec<String> = (0..3000)
            .map(|i| {
                format!(
                    r#"{{"id":"ap_{i}","state":"approved","kind":"release é {i}","person":"ada","answerBy":{},"device":"phone {i}"}}"#,
                    1_763_600_000 + i
                )
            })
            .collect();
        format!(r#"{{"requests":[{}]}}"#, rows.join(","))
    };
    assert!(many.len() > 256 * 1024);
    let agents = {
        let rows: Vec<String> = (0..4000)
            .map(|i| format!(r#"{{"workspace":"ws-{i}-é","grantedBy":"owner"}}"#))
            .collect();
        format!(r#"{{"agents":[{}]}}"#, rows.join(","))
    };
    assert!(agents.len() > 128 * 1024);
    let v = vec![
        one("status: one", &["status", "ap_1"], APPROVED, 0),
        one(
            "status: the list",
            &["status"],
            r#"{"requests":[{"id":"ap_1","state":"waiting","kind":"k","person":"p","answerBy":1.5},{"id":"ap_2","state":"declined","person":"q","device":"d"}]}"#,
            0,
        ),
        one("status: none", &["status"], r#"{"requests":[]}"#, 0),
        one("status: many", &["status"], &many, 0),
        // covers: approve --json
        one("status --json", &["status", "ap 1", "--json"], APPROVED, 0),
        one("status --out", &["status", "ap_1", "--out", "rec.txt"], APPROVED, 0),
        one("status --out: not approved", &["status", "ap_1", "--out", "rec.txt"], DECLINED, 1),
        traceback(one("status: not JSON", &["status"], "<html>", 1)),
        traceback(one(
            "status: a row with no state",
            &["status"],
            r#"{"requests":[{"id":"a","state":"x"},{"id":"b"}]}"#,
            1,
        )),
        stop("status: --out with no id", &["status", "--out", "rec.txt"], 2),
        stop("status: an unknown flag", &["status", "--all"], 2),
        stop("status --out last: shift fails", &["status", "ap_1", "--out"], 1),
        case("status: no answer", &["status"], &[], vec![Reply::Close], owner_and_port, 3, 1),
        case(
            "status: 404",
            &["status", "ap_9"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"not_found","hint":"no such request"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case("status: on the cell socket", &["status"], &[], vec![ok(ASKED)], port_only, 0, 1)
        },
        one("cancel", &["cancel", "ap_1"], "{}", 0),
        stop("cancel: no id", &["cancel"], 2),
        one("agents: the list", &["agents"], r#"{"agents":[{"workspace":"w","grantedBy":"owner"}]}"#, 0),
        one("agents: many", &["agents"], &agents, 0),
        one("agents: none", &["agents"], r#"{"agents":[]}"#, 0),
        one("agents on", &["agents", "w é", "on"], "{}", 0),
        one("agents off", &["agents", "w", "off"], "{}", 0),
        stop("agents: neither on nor off", &["agents", "w"], 2),
        // Help never acts: bash sends these (a bug K2 fixes; flip the rows then).
        shell(one("cancel -h: bash sends it", &["cancel", "ap_1", "-h"], "{}", 0)),
        shell(one("agents --help: bash sends it", &["agents", "w", "on", "--help"], "{}", 0)),
    ];
    for c in v {
        rig.check(&c);
    }
}
