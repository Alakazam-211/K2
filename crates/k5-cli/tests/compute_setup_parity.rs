//! `k5-cli compute allow-nodes|git-access|lanes|local` against the shell
//! CLI's, on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - `allow-nodes`: the list, `--json`, a change (nodes, `--add`,
//!   `--remove`, `--clear`), a session's own workspace on the cell socket,
//!   the words' errors, a refusal, no answer, Python's raise.
//! - `git-access`: the list, `--json`, `setup` (a key file, a token file,
//!   a token on stdin, `--clear`, and each way it stops), `grant`,
//!   `revoke`, the words' errors, a refusal.
//! - `lanes`: the list (none, some, over 256 KiB), `--json`, `set` with
//!   `key=value` words, the words' errors.
//! - `local`: `status` (not a node, a node), `--json`, `pause`, `drain`,
//!   `resume`, `policy` (show and set), `enable` (the code and the ticket
//!   on stdin or argv, and each way it stops), `remote-admin`, `disable`
//!   (nothing running, jobs running with and without `--yes`).
//! - Help never acts: `-h` or `--help` on every verb that changes
//!   something prints help and sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";

fn sess(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: port_only,
        ..c
    }
}

fn stdin(c: Case, input: &'static [u8]) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

const ALLOW_LIST: &str = r#"{"workspace":"acme","list":true,"nodes":[{"name":"mini","state":"active","granted":true},{"name":"big","state":"paused"},{"id":"n9"}]}"#;
const ALLOW_FIXED: &str = r#"{"workspace":"acme","change":"fixed","list":true,"fixed":true,"nodes":[{"name":"mini","state":"active","granted":true}]}"#;
const ALLOW_SET: &str = r#"{"workspace":"acme","change":"same","list":false,"failedQueued":["0123456789abcdef","fedcba9876543210"],"withdrawn":3}"#;

#[test]
fn compute_allow_nodes() {
    let rig = Rig::new(VERB, "allow");
    let v = vec![
        // covers: compute allow-nodes
        case(
            "the list",
            &["allow-nodes"],
            &[],
            vec![ok(ALLOW_LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute allow-nodes --json
        case(
            "the list as JSON",
            &["allow-nodes", "--json", "--workspace", "acme"],
            &[],
            vec![ok(ALLOW_LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a change",
            &[
                "allow-nodes",
                "mini",
                "big",
                "--add",
                "x",
                "--remove",
                "y,z",
                "--clear",
            ],
            &[],
            vec![ok(ALLOW_SET)],
            owner_and_port,
            0,
            1,
        ),
        // 0.46.18: --fixed on|off, and the list's fixed line both ways.
        case(
            "--fixed on",
            &["allow-nodes", "--fixed", "on", "--workspace", "acme"],
            &[],
            vec![ok(ALLOW_FIXED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--fixed off",
            &["allow-nodes", "--fixed", "off"],
            &[],
            vec![ok(ALLOW_LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--fixed with another word",
            &["allow-nodes", "--fixed", "yes"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        sess(case(
            "a session's own, on the cell socket",
            &["allow-nodes", "--add", "mini"],
            &[],
            vec![ok(ALLOW_LIST)],
            port_only,
            0,
            1,
        )),
        sess(case(
            "a refusal",
            &["allow-nodes", "--clear"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            port_only,
            3,
            1,
        )),
        case(
            "no answer",
            &["allow-nodes"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "a name that is no string",
            &["allow-nodes"],
            &[],
            vec![ok(r#"{"list":true,"nodes":[{"name":5,"granted":true}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "--add last",
            &["allow-nodes", "--add"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an empty workspace",
            &["allow-nodes", "--workspace", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["allow-nodes", "-x"],
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

const GIT_LIST: &str = r#"{"workspace":"acme","credential":null,"warnings":["the app can't see o/x"],"grants":[{"repo":"o/r","nodes":"all"},{"repo":"o/s","nodes":"mini,big","shared":true}],"tokenNote":"tokens last an hour"}"#;
const GIT_REVOKED: &str = r#"{"workspace":"acme","credential":{"kind":"token"},"revoked":["o/r","o/s"],"removed":2,"tokensDeleted":1,"runningJobs":[{"job":"j1","repo":"o/r"}],"note":"done","grants":[]}"#;
const GIT_SET: &str = r#"{"credential":{"kind":"app","appId":12,"app":"acme-bot"},"note":"nodes ask for tokens now"}"#;

fn key_files(r: &Rig) {
    owner_and_port(r);
    std::fs::write(
        r.proj.join("app.pem"),
        "-----BEGIN KEY-----\r\nabc\r\n-----END KEY-----\n",
    )
    .unwrap();
    std::fs::write(r.proj.join("tok.txt"), "  ghp_file\n\n").unwrap();
}

#[test]
fn compute_git_access() {
    let rig = Rig::new(VERB, "git");
    let v = vec![
        // covers: compute git-access
        case(
            "the list",
            &["git-access"],
            &[],
            vec![ok(GIT_LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute git-access --json
        case(
            "the list as JSON",
            &["git-access", "list", "--json"],
            &[],
            vec![ok(GIT_LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute git-access setup
        case(
            "setup with an app key",
            &[
                "git-access",
                "setup",
                "--github-app",
                "12",
                "--key-file",
                "app.pem",
            ],
            &[],
            vec![ok(GIT_SET)],
            key_files,
            0,
            1,
        ),
        case(
            "setup with a token file",
            &["git-access", "setup", "--github-token-file", "tok.txt"],
            &[],
            vec![ok(r#"{"credential":{"kind":"token"}}"#)],
            key_files,
            0,
            1,
        ),
        stdin(
            case(
                "setup with a token on stdin",
                &["git-access", "setup", "--github-token-stdin"],
                &[],
                vec![ok(r#"{"credential":{"kind":"token"},"note":"ok"}"#)],
                owner_and_port,
                0,
                1,
            ),
            b"\t ghp_stdin\r\nmore\r\n\n",
        ),
        case(
            "setup --clear",
            &["git-access", "setup", "--clear", "--github-app", "1"],
            &[],
            vec![ok(r#"{"credential":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "setup alone",
            &["git-access", "setup"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "setup with no key file",
            &["git-access", "setup", "--github-app", "12"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "setup with a key file not there",
            &[
                "git-access",
                "setup",
                "--github-app",
                "12",
                "--key-file",
                "none.pem",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: compute git-access grant
        case(
            "grant",
            &[
                "git-access",
                "grant",
                "--github",
                "o/r",
                "--github",
                "o/s",
                "--nodes",
                "all",
                "--shared",
                "--read-only",
                "--workspace",
                "acme",
            ],
            &[],
            vec![ok(
                r#"{"workspace":"acme","credential":{"kind":"app","appId":12},"granted":["o/r","o/s"],"grants":[{"repo":"o/r","nodes":"all","shared":true}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        sess(case(
            "grant in a session: a refusal",
            &["git-access", "grant", "--github", "o/r"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            port_only,
            3,
            1,
        )),
        case(
            "grant with no repo",
            &["git-access", "grant"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: compute git-access revoke
        case(
            "revoke --all",
            &["git-access", "revoke", "--all"],
            &[],
            vec![ok(GIT_REVOKED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "revoke with nothing",
            &["git-access", "revoke"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown word",
            &["git-access", "show"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--github last",
            &["git-access", "grant", "--github"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "a warning that is no string",
            &["git-access"],
            &[],
            vec![ok(r#"{"workspace":"acme","warnings":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

fn big_lanes(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(r#"{{"name":"lane-{i:05}","summary":"8 cpus, 16G, vm isolation, profiles ci and build, row {i}","approvedBy":"owner"}}"#)
        })
        .collect();
    format!(r#"{{"lanes":[{}]}}"#, rows.join(","))
}

#[test]
fn compute_lanes() {
    let rig = Rig::new(VERB, "lanes");
    let big = big_lanes(3000);
    assert!(big.len() > 256 * 1024);
    let some = r#"{"lanes":[{"name":"ci-standard","summary":"4 cpus","approvedBy":"owner"}]}"#;
    let v = vec![
        // covers: compute lanes
        case(
            "some lanes",
            &["lanes"],
            &[],
            vec![ok(some)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no lanes",
            &["lanes"],
            &[],
            vec![ok(r#"{"lanes":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "3000 lanes",
            &["lanes"],
            &[],
            vec![ok(&big)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute lanes --json
        case(
            "as JSON",
            &["lanes", "--json"],
            &[],
            vec![ok(some)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "3000 lanes as JSON",
            &["lanes", "--json"],
            &[],
            vec![ok(&big)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute lanes set
        case(
            "set",
            &[
                "lanes",
                "set",
                "ci",
                "--from",
                "ci-standard",
                "cpus=4",
                "isolation=vm,net",
                "x=[1, 2.50]",
                "cpus=8",
                "--why",
                "release gate",
                "--json",
            ],
            &[],
            vec![ok(
                r#"{"lane":{"name":"ci","state":"waiting"},"request":"ctr_1"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        sess(case(
            "set in a session",
            &["lanes", "set", "ci"],
            &[],
            vec![ok(r#"{"request":"ctr_2"}"#)],
            port_only,
            0,
            1,
        )),
        case(
            "set with no name",
            &["lanes", "set"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set --from last",
            &["lanes", "set", "ci", "--from"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set with a stray word",
            &["lanes", "set", "ci", "big"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "a lane with no name",
            &["lanes"],
            &[],
            vec![ok(r#"{"lanes":[{"summary":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

const NOT_A_NODE: &str = r#"{"installed":false,"binary":{"path":"/usr/local/bin/k2-node","source":"download"},"elevation":"command","homeCheck":{"home":"/home/a","private":true,"mode":"0700"}}"#;
const A_NODE: &str = r#"{"installed":true,"node":{"name":"mini","state":"online","route":"lan","running":["j1","j2"],"last_error":"disk full","sas":"123456"},"control":{"state":"paused","by":"owner"},"enrollResult":{"ok":false,"message":"bad code"},"packages":{"approved":["cmake@3","node@22"]},"homeCheck":{"home":"/home/a","private":false,"mode":"0755"},"remoteAdmin":null,"remoteAdminCommand":"sudo k2-node admin on","policy":{"max_jobs":2,"gpu":"off"}}"#;
const SAID: &str = r#"{"control":"paused","note":"jobs finish first"}"#;
const MESSAGE: &str = r#"{"message":"enrolled","command":"sudo k2-node install","sas":"424242"}"#;

#[test]
fn compute_local() {
    let rig = Rig::new(VERB, "local");
    let v = vec![
        // covers: compute local
        case(
            "no word: the status",
            &["local"],
            &[],
            vec![ok(NOT_A_NODE)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute local status
        case(
            "a node",
            &["local", "status"],
            &[],
            vec![ok(A_NODE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not a node, nothing known",
            &["local", "status"],
            &[],
            vec![ok(r#"{"installed":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute local status --json
        case(
            "as JSON",
            &["local", "status", "--json"],
            &[],
            vec![ok(A_NODE)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "an elevation that is a list",
            &["local", "status"],
            &[],
            vec![ok(r#"{"installed":false,"elevation":["x"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: compute local pause
        case(
            "pause --now",
            &["local", "pause", "--now"],
            &[],
            vec![ok(SAID)],
            owner_and_port,
            0,
            1,
        ),
        sess(case(
            "pause in a session, over TCP",
            &["local", "pause"],
            &[],
            vec![ok(SAID)],
            port_only,
            0,
            1,
        )),
        // covers: compute local drain
        case(
            "drain",
            &["local", "drain"],
            &[],
            vec![ok(SAID)],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute local resume
        case(
            "resume, refused",
            &["local", "resume"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"run it outside K2"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // covers: compute local policy
        case(
            "policy",
            &["local", "policy"],
            &[],
            vec![ok(A_NODE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "policy set",
            &["local", "policy", "max-jobs=3", "gpu=on", "max-jobs=4"],
            &[],
            vec![ok(r#"{"policy":{"max_jobs":"4","gpu":"on"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "policy with a bare word",
            &["local", "policy", "a=1", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: compute local enable
        stdin(
            case(
                "enable, the code on stdin",
                &[
                    "local",
                    "enable",
                    "--controller",
                    "https://ctl.example",
                    "--code-stdin",
                    "--name",
                    "mini",
                    "--no-private-home",
                    "--remote-admin",
                    "policy",
                ],
                &[],
                vec![ok(MESSAGE)],
                owner_and_port,
                0,
                1,
            ),
            b"CODE.FP1\r\nnext line\n",
        ),
        stdin(
            case(
                "enable, the ticket on stdin",
                &["local", "enable", "--ticket-stdin", "--private-home"],
                &[],
                vec![ok(r#"{"message":"waiting"}"#)],
                owner_and_port,
                0,
                1,
            ),
            b"tkt-9",
        ),
        case(
            "enable, both on argv: two notes",
            &["local", "enable", "--ticket", "T1", "--code", "C.F"],
            &[],
            vec![ok(MESSAGE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "enable, nothing",
            &["local", "enable"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "enable, both on stdin",
            &["local", "enable", "--code-stdin", "--ticket-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        stdin(
            case(
                "enable, no code on stdin",
                &["local", "enable", "--code-stdin"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
            b"\r\n",
        ),
        case(
            "enable, --code-stdin and --enroll",
            &["local", "enable", "--enroll", "X", "--code-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "enable, --ticket-stdin and --ticket",
            &["local", "enable", "--ticket", "T", "--ticket-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "enable, a bad level",
            &["local", "enable", "--remote-admin", "most"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "enable, --name last",
            &["local", "enable", "--name"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "enable, a stray word",
            &["local", "enable", "now"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: compute local remote-admin
        case(
            "remote-admin",
            &["local", "remote-admin", "full"],
            &[],
            vec![ok(
                r#"{"message":"remote admin is full","command":"sudo k2-node admin full"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "remote-admin, no level",
            &["local", "remote-admin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: compute local disable
        case(
            "disable, nothing running",
            &["local", "disable"],
            &[],
            vec![ok(NOT_A_NODE), ok(r#"{"message":"stopped"}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "disable, jobs running",
            &["local", "disable"],
            &[],
            vec![ok(A_NODE)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "disable --yes, jobs running",
            &["local", "disable", "--yes"],
            &[],
            vec![
                ok(A_NODE),
                ok(r#"{"message":"stopped","command":"sudo rm"}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        traceback(case(
            "disable, a running list of numbers",
            &["local", "disable"],
            &[],
            vec![ok(r#"{"node":{"running":[1]}}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "an unknown word",
            &["local", "start"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--json alone",
            &["local", "--json"],
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

/// Help never acts: -h or --help anywhere on a verb that changes
/// something prints help and sends nothing (the front hands it to the
/// shell's help, which answers first).
#[test]
fn compute_setup_help_never_acts() {
    let rig = Rig::new(VERB, "setuphelp");
    const HELP: &[&[&str]] = &[
        &["allow-nodes", "--add", "mini", "-h"],
        &["allow-nodes", "--help", "--clear"],
        &["git-access", "--help"],
        &["git-access", "setup", "--clear", "-h"],
        &["git-access", "grant", "--github", "o/r", "--help"],
        &["git-access", "revoke", "--all", "-h"],
        &["lanes", "set", "ci", "cpus=4", "-h"],
        &["local", "pause", "--help"],
        &["local", "drain", "-h"],
        &["local", "resume", "--help"],
        &["local", "policy", "a=1", "-h"],
        &["local", "enable", "--code", "X", "--help"],
        &["local", "remote-admin", "off", "-h"],
        &["local", "disable", "--yes", "--help"],
    ];
    for args in HELP {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
