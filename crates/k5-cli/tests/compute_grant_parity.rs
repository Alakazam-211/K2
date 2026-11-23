//! `k5-cli compute grant` against the shell CLI's `cli/k2 compute grant`,
//! on the parity rig ([`common::parity`]): the same requests (one POST,
//! over TCP), stdout, stderr and exit code.
//!
//! - Every flag, the fewest, a session's own workspace (no
//!   `--workspace`), a disk size with a point; the queued jobs sent again.
//! - The refusals: a permission card (the closing line), the daemon's
//!   error; Python's raise on the answer and on a number that is none.
//! - The words: no node, no workspace outside a session, a bad duration
//!   (also `--max-time` last), a bad per-submitter cap, an unknown flag.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";
const HELP: &[&[&str]] = &[
    &["grant", "mini", "--workspace", "acme", "-h"],
    &["grant", "--help", "mini"],
    &["grant", "mini", "--max-time", "2h", "--ci", "--help"],
];
const GRANTED: &str = r#"{"node":"mini","workspace":"acme","grant":{"maxJobSecs":7200,"maxParallel":2,"maxQueued":10,"maxDiskGb":60,"submitterMaxRunning":1,"allowCi":true,"allowUserns":true,"allowNoAddrFilter":true},"note":"agents there can run jobs.","resent":2}"#;
const PLAIN: &str = r#"{"node":"mini","workspace":"acme","grant":{"maxJobSecs":3600,"maxParallel":1,"maxQueued":10,"maxDiskGb":40},"note":"ok."}"#;

#[test]
fn compute_grant() {
    let rig = Rig::new(VERB, "grant");
    let v = vec![
        // covers: compute grant
        case(
            "every flag",
            &[
                "grant",
                "mini",
                "--workspace",
                "acme",
                "--max-time",
                "2h",
                "--max-disk",
                "60G",
                "--max-parallel",
                "2",
                "--max-queued",
                "10",
                "--submitter-max-running",
                "1",
                "--ci",
                "--no-gpu",
                "--userns",
                "--no-addr-filter",
            ],
            &[],
            vec![ok(GRANTED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--addr-filter (0.46.18)",
            &["grant", "mini", "--workspace", "acme", "--addr-filter"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--no-addr-filter without --userns: no_addr_filter_invalid, exit 2",
            &["grant", "mini", "--workspace", "acme", "--no-addr-filter"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"no_addr_filter_invalid","hint":"--no-addr-filter is granted only with --userns"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "--no-userns (0.46.18)",
            &["grant", "mini", "--workspace", "acme", "--no-userns"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--userns on a node where this server is a tenant: userns_owner_only",
            &["grant", "mini", "--workspace", "acme", "--userns"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"userns_owner_only","hint":"--userns is only for the node owner's own server: this server is a tenant of mini, so its jobs can't make user namespaces there","node":"mini"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the fewest",
            &["grant", "--workspace", "acme", "mini"],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a disk size with a point",
            &[
                "grant",
                "mini",
                "--workspace",
                "acme",
                "--max-disk",
                "1.5g",
                "--max-time",
                "007",
            ],
            &[],
            vec![ok(PLAIN)],
            owner_and_port,
            0,
            1,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case(
                "a session's own workspace",
                &["grant", "mini"],
                &[],
                vec![ok(PLAIN)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case(
                "a permission card",
                &["grant", "mini"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                3,
                1,
            )
        },
        case(
            "the daemon's error",
            &["grant", "mini", "--workspace", "acme"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_cap","hint":"max-parallel is 1 to 8"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "an answer with no grant",
                &["grant", "mini", "--workspace", "acme"],
                &[],
                vec![ok(r#"{"node":"mini"}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "a number that is none",
                &[
                    "grant",
                    "mini",
                    "--workspace",
                    "acme",
                    "--max-parallel",
                    "two",
                ],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            )
        },
        case(
            "no node",
            &["grant", "--workspace", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no workspace outside a session",
            &["grant", "mini"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad duration",
            &["grant", "mini", "--workspace", "a", "--max-time", "2d"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--max-time last",
            &["grant", "mini", "--max-time"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bad per-submitter cap",
            &["grant", "mini", "--submitter-max-running", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--workspace last",
            &["grant", "mini", "--workspace"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["grant", "mini", "--nope"],
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

/// Help never acts: -h or --help anywhere prints help and sends nothing
/// (the binary hands it to the shell, whose `_wants_help` answers first).
#[test]
fn compute_grant_help_never_acts() {
    let rig = Rig::new(VERB, "granthelp");
    for args in HELP {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
