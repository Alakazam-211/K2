//! `k5-cli compute` admin verbs against the shell CLI's `cli/k2 compute`,
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - `revoke`, `preview on|off`, `placement v1|v2`, `access`, `operate`
//!   and `manage` (one POST over TCP each), `retier` and `tier`.
//! - The refusals: a permission card, the daemon's error, no answer;
//!   Python's raise on an answer the reader can't read.
//! - The words: usage lines (exit 2) and a value flag last (exit 1,
//!   nothing printed); nothing is sent after either.
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

#[test]
fn compute_revoke() {
    let rig = Rig::new(VERB, "revoke");
    let removed = r#"{"node":"mini","workspace":"acme","removed":true,"note":"its queued jobs stay."}"#;
    let v = vec![
        // covers: compute revoke
        case(
            "a grant removed",
            &["revoke", "mini", "--workspace", "acme"],
            &[],
            vec![ok(removed)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no grant, the workspace first",
            &["revoke", "--workspace", "acme", "mini"],
            &[],
            vec![ok(r#"{"removed":false,"note":"nothing to do."}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json is a node name",
            &["revoke", "mini", "--workspace", "acme", "--json"],
            &[],
            vec![ok(removed)],
            owner_and_port,
            0,
            1,
        ),
        in_session(case(
            "a permission card",
            &["revoke", "mini", "--workspace", "acme"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            port_only,
            3,
            1,
        )),
        case(
            "the daemon's error",
            &["revoke", "mini", "--workspace", "acme"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"no_node","hint":"no node mini"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an empty reply",
            &["revoke", "mini", "--workspace", "acme"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        raises(case(
            "a note that is no string",
            &["revoke", "mini", "--workspace", "acme"],
            &[],
            vec![ok(r#"{"removed":false,"note":7}"#)],
            owner_and_port,
            1,
            1,
        )),
        raises(case(
            "an answer that is no JSON",
            &["revoke", "mini", "--workspace", "acme"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        case(
            "no workspace",
            &["revoke", "mini"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--workspace last",
            &["revoke", "mini", "--workspace"],
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

#[test]
fn compute_nothing_listens() {
    let rig = Rig::new(VERB, "adminnone");
    let dead = dead_port();
    const LINES: &[&[&str]] = &[
        &["revoke", "mini", "--workspace", "acme"],
        &["preview", "on"],
        &["access", "off", "--workspace", "acme"],
        &["retier", "j1", "--batch"],
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

#[test]
fn compute_preview_and_placement() {
    let rig = Rig::new(VERB, "preview");
    let v = vec![
        // covers: compute preview
        case(
            "preview on",
            &["preview", "on"],
            &[],
            vec![ok(r#"{"computePreview":true,"computePlacement":"v2"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "preview off, a word after it",
            &["preview", "off", "now"],
            &[],
            vec![ok(r#"{"computePreview":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        in_session(case(
            "preview on in a session: the owner's switch",
            &["preview", "on"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            port_only,
            3,
            1,
        )),
        raises(case(
            "preview: an answer that is a list",
            &["preview", "off"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        // covers: compute placement
        case(
            "placement v1",
            &["placement", "v1"],
            &[],
            vec![ok(r#"{"computePlacement":"v1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "placement v2, none in the answer",
            &["placement", "v2"],
            &[],
            vec![ok(r#"{"computePlacement":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "placement: a name the reader doesn't know",
            &["placement", "v2"],
            &[],
            vec![ok(r#"{"computePlacement":"v3"}"#)],
            owner_and_port,
            0,
            1,
        ),
        raises(case(
            "placement: a list",
            &["placement", "v1"],
            &[],
            vec![ok(r#"{"computePlacement":["v1"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "placement: the daemon's error",
            &["placement", "v1"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"bad_value","message":"v1 or v2"}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_access_operate_manage() {
    let rig = Rig::new(VERB, "access");
    let v = vec![
        // covers: compute access
        case(
            "access on, machines granted",
            &["access", "on", "--workspace", "acme"],
            &[],
            vec![ok(
                r#"{"workspace":"acme","agentsCanUseCompute":true,"granted":["mini"],"kept":["linux-1"]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access on, this checkout's workspace, --all-machines",
            &["access", "on", "--all-machines"],
            &[],
            vec![ok(r#"{"workspace":"proj","agentsCanUseCompute":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access off, grants removed",
            &["access", "off", "--workspace", "acme"],
            &[],
            vec![ok(
                r#"{"workspace":"acme","agentsCanUseCompute":false,"grantsRemoved":3}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access off, no count",
            &["access", "off", "--workspace", "acme"],
            &[],
            vec![ok(r#"{"workspace":"acme","agentsCanUseCompute":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        in_session(case(
            "access in a session: the passport's workspace",
            &["access", "on"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            port_only,
            3,
            1,
        )),
        raises(case(
            "access: granted that is no list",
            &["access", "on", "--workspace", "acme"],
            &[],
            vec![ok(
                r#"{"workspace":"acme","agentsCanUseCompute":true,"granted":"mini"}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        raises(case(
            "access: no switch in the answer",
            &["access", "on", "--workspace", "acme"],
            &[],
            vec![ok(r#"{"workspace":"acme"}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "access status is no action",
            &["access", "status"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "access: an unknown flag",
            &["access", "on", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "access: --workspace last",
            &["access", "on", "--workspace"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: compute operate
        case(
            "operate on",
            &["operate", "on", "--workspace", "acme"],
            &[],
            vec![ok(r#"{"workspace":"acme","agentsCanOperateCompute":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "operate off",
            &["operate", "off"],
            &[],
            vec![ok(r#"{"workspace":"proj","agentsCanOperateCompute":0}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "operate: no action",
            &["operate"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "operate: --all-machines is access's",
            &["operate", "on", "--all-machines"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: compute manage
        case(
            "manage on",
            &["manage", "on", "--workspace", "acme"],
            &[],
            vec![ok(r#"{"workspace":"acme","agentsCanManageCompute":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "manage off",
            &["manage", "off", "--workspace", "acme"],
            &[],
            vec![ok(r#"{"workspace":"acme","agentsCanManageCompute":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        in_session(case(
            "manage in a session: refused",
            &["manage", "on"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"the owner turns it on"}}"#.into(),
            )],
            port_only,
            3,
            1,
        )),
        case(
            "manage: a bad action",
            &["manage", "maybe", "--workspace", "acme"],
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
fn compute_retier() {
    let rig = Rig::new(VERB, "retier");
    let moved = r#"{"job":"j1","from":"normal","tier":"priority","node":"mini","note":"it runs next."}"#;
    let v = vec![
        // covers: compute retier
        case(
            "to priority, with a reason",
            &["retier", "j1", "--priority", "--why", "release gate"],
            &[],
            vec![ok(moved)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tier, to standard",
            &["tier", "--tier", "standard", "j1"],
            &[],
            vec![ok(
                r#"{"job":"j1","from":"batch","tier":"normal","node":"mini","note":"ok."}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "asked for one tier, given another",
            &["retier", "j1", "--critical", "--why", "x"],
            &[],
            vec![ok(
                r#"{"job":"j1","from":"normal","tier":"normal","asked":"priority","node":"linux-1","note":"not yours."}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: compute retier --json
        case(
            "--json",
            &["retier", "j1", "--batch", "--json"],
            &[],
            vec![ok(moved)],
            owner_and_port,
            0,
            1,
        ),
        in_session(case(
            "in a session: the cell socket",
            &["retier", "j1", "--batch"],
            &[],
            vec![ok(moved)],
            port_only,
            0,
            1,
        )),
        case(
            "the daemon's error",
            &["retier", "j1", "--tier", "ci"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"job_started","hint":"it runs already"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        raises(case(
            "a tier that is a list",
            &["retier", "j1", "--batch"],
            &[],
            vec![ok(
                r#"{"job":"j1","from":["normal"],"tier":"batch","node":"mini","note":"x"}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        raises(case(
            "no note",
            &["retier", "j1", "--batch"],
            &[],
            vec![ok(r#"{"job":"j1","from":"normal","tier":"batch","node":"mini"}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "priority with no reason",
            &["retier", "j1", "--priority"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no tier",
            &["retier", "j1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no job",
            &["retier", "--batch"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["retier", "j1", "--fast"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--why last",
            &["retier", "j1", "--batch", "--why"],
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
fn compute_admin_help_never_acts() {
    let rig = Rig::new(VERB, "adminhelp");
    let lines: &[&[&str]] = &[
        &["revoke", "mini", "--workspace", "acme", "-h"],
        &["preview", "on", "--help"],
        &["placement", "-h", "v1"],
        &["access", "on", "--workspace", "acme", "--help"],
        &["operate", "-h", "on"],
        &["manage", "on", "--help"],
        &["retier", "j1", "--batch", "-h"],
        &["tier", "--help", "j1", "--batch"],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
