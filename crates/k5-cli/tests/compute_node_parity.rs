//! `k5-cli compute node ...` against the shell CLI's `cli/k2 compute
//! node ...`, on the parity rig ([`common::parity`]): the same requests,
//! stdout, stderr and exit code.
//!
//! - The one-POST subverbs: `add` (and `--json`), `confirm`, `remove`,
//!   `rm`, `retire`, `rename`, `pause`, `drain`, `resume`, `pinned-only`,
//!   `route` and `alerts`; `node` alone and an unknown subverb.
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

/// One POST answered `body`: exit `code`.
fn one(name: &'static str, args: &'static [&'static str], body: &'static str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

/// Words that stop before anything is sent: exit `code`.
fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

const ENROLL: &str = r#"{"name":"mini","enroll":"ABCD-1234.ff00","expiresAt":1790000000,"controllerUrl":"http://10.0.0.2:4100","install":{"linux":"curl -fsS http://10.0.0.2:4100/node/install.sh | sudo sh -s -- --code-file code-file","macos":"sudo k2-node enroll --code-file code-file","servers":"k2-node servers add --code-file code-file"},"confirm":"k2 compute node confirm mini <6-digit code>"}"#;

#[test]
fn compute_node_add() {
    let rig = Rig::new(VERB, "nodeadd");
    let v = vec![
        // covers: compute node add
        one("an enroll code", &["node", "add", "mini"], ENROLL, 0),
        one(
            "a url, no servers line",
            &["node", "add", "--url", "http://10.0.0.2:4100", "mini"],
            r#"{"name":"mini","enroll":"C","install":{"linux":"l","macos":"m","servers":""},"confirm":"c"}"#,
            0,
        ),
        // covers: compute node add --json
        one(
            "the scripts' object",
            &["node", "add", "mini", "--json"],
            ENROLL,
            0,
        ),
        raises(one(
            "--json with no controller",
            &["node", "add", "mini", "--json"],
            r#"{"enroll":"C","expiresAt":1,"name":"mini"}"#,
            1,
        )),
        raises(one(
            "an install that is no dict",
            &["node", "add", "mini"],
            r#"{"name":"mini","enroll":"C","install":"sh"}"#,
            1,
        )),
        raises(one(
            "an answer that is no JSON",
            &["node", "add", "mini"],
            "<html>",
            1,
        )),
        case(
            "the daemon's error",
            &["node", "add", "mini"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"node_exists","hint":"mini is enrolled"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an empty reply",
            &["node", "add", "mini"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a permission card",
            &["node", "add", "mini"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        words("no name", &["node", "add", "--json"], 2),
        words("--url last", &["node", "add", "mini", "--url"], 1),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_node_one_post_verbs() {
    let rig = Rig::new(VERB, "nodepost");
    let v = vec![
        // covers: compute node
        words("node alone", &["node"], 2),
        words("an unknown subverb", &["node", "frobnicate", "mini"], 2),
        // covers: compute node confirm
        one(
            "confirmed",
            &["node", "confirm", "mini", "123", "456"],
            r#"{"node":"mini","next":"it takes jobs now."}"#,
            0,
        ),
        raises(one(
            "confirm with no next",
            &["node", "confirm", "mini", "123456"],
            r#"{"node":"mini"}"#,
            1,
        )),
        words("confirm with no code", &["node", "confirm", "mini"], 2),
        // covers: compute node remove
        one(
            "removed",
            &["node", "remove", "mini"],
            r#"{"node":"mini","jobsCancelled":2}"#,
            0,
        ),
        one(
            "rm, a float count",
            &["node", "rm", "mini"],
            r#"{"node":"mini","jobsCancelled":2.7}"#,
            0,
        ),
        raises(one(
            "a count that is no number",
            &["node", "rm", "mini"],
            r#"{"node":"mini","jobsCancelled":"two"}"#,
            1,
        )),
        words("remove with no name", &["node", "remove"], 2),
        // covers: compute node retire
        one(
            "retired at once",
            &["node", "retire", "mini"],
            r#"{"node":"mini","state":"revoked"}"#,
            0,
        ),
        one(
            "draining, no count",
            &["node", "retire", "mini"],
            r#"{"node":"mini","state":"draining"}"#,
            0,
        ),
        one(
            "draining, two running",
            &["node", "retire", "mini"],
            r#"{"node":"mini","state":"draining","running":2}"#,
            0,
        ),
        words("retire with no name", &["node", "retire"], 2),
        // covers: compute node rename
        one(
            "renamed",
            &["node", "rename", "mini", "mini-2"],
            r#"{"ok":true}"#,
            0,
        ),
        one(
            "renamed, an answer that is no JSON",
            &["node", "rename", "mini", "mini-2"],
            "<html>",
            0,
        ),
        words("rename with one name", &["node", "rename", "mini"], 2),
        // covers: compute node pause
        one(
            "paused",
            &["node", "pause", "mini"],
            r#"{"node":"mini","controllerPause":"paused","note":"queued jobs wait."}"#,
            0,
        ),
        // covers: compute node drain
        one(
            "draining",
            &["node", "drain", "mini"],
            r#"{"node":"mini","controllerPause":"draining","note":"running jobs end."}"#,
            0,
        ),
        // covers: compute node resume
        one(
            "resumed",
            &["node", "resume", "mini"],
            r#"{"node":"mini","controllerPause":null,"note":"it takes jobs."}"#,
            0,
        ),
        raises(one(
            "resume with no note",
            &["node", "resume", "mini"],
            r#"{"node":"mini"}"#,
            1,
        )),
        words("drain with no name", &["node", "drain"], 2),
        // covers: compute node pinned-only
        one(
            "pinned-only on",
            &["node", "pinned-only", "mini", "on"],
            r#"{"node":"mini","pinnedOnly":true,"changed":true,"note":"only jobs that name it run there"}"#,
            0,
        ),
        one(
            "pinned-only off, already",
            &["node", "pinned-only", "mini", "off"],
            r#"{"node":"mini","pinnedOnly":false,"note":"placement may choose it"}"#,
            0,
        ),
        words("pinned-only, a bad word", &["node", "pinned-only", "mini", "yes"], 2),
        words(
            "pinned-only, three words",
            &["node", "pinned-only", "mini", "on", "x"],
            2,
        ),
        // covers: compute node route
        one(
            "a route added",
            &["node", "route", "add", "mini", "http://10.0.0.3:7070"],
            r#"{"node":"mini","routes":["http://10.0.0.3:7070","http://mini.lan:7070"]}"#,
            0,
        ),
        one(
            "routes rm, none left",
            &["node", "routes", "rm", "mini", "http://10.0.0.3:7070"],
            r#"{"node":"mini","routes":[]}"#,
            0,
        ),
        one(
            "route remove",
            &["node", "route", "remove", "mini", "http://10.0.0.3:7070"],
            r#"{"node":"mini","routes":["a"]}"#,
            0,
        ),
        raises(one(
            "routes that are no strings",
            &["node", "route", "add", "mini", "u"],
            r#"{"node":"mini","routes":[1,2]}"#,
            1,
        )),
        words("route, a bad op", &["node", "route", "set", "mini", "u"], 2),
        words("route, two words", &["node", "route", "add", "mini"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_node_alerts() {
    let rig = Rig::new(VERB, "nodealerts");
    let both = r#"{"node":"mini","alertAfterSecs":600,"alertsTo":"ops"}"#;
    let v = vec![
        // covers: compute node alerts
        one(
            "after ten minutes",
            &["node", "alerts", "mini", "--after", "10m"],
            r#"{"node":"mini","alertAfterSecs":600}"#,
            0,
        ),
        one(
            "after 1h30m and to a workspace",
            &["node", "alerts", "mini", "--after", "1h30m", "--to", "ops"],
            both,
            0,
        ),
        one(
            "plain seconds",
            &["node", "alerts", "--after", "0090", "mini"],
            r#"{"node":"mini","alertAfterSecs":90.5}"#,
            0,
        ),
        one(
            "off",
            &["node", "alerts", "mini", "--after", "off"],
            r#"{"node":"mini","alertAfterSecs":null}"#,
            0,
        ),
        one(
            "none",
            &["node", "alerts", "mini", "--after", "none"],
            r#"{"node":"mini"}"#,
            0,
        ),
        one(
            "to only, unset",
            &["node", "alerts", "--to", ""],
            r#"{"alertsTo":null}"#,
            0,
        ),
        one(
            "to only, an --after with no node is not read",
            &["node", "alerts", "--after", "soon", "--to", "ops"],
            r#"{"alertsTo":"ops"}"#,
            0,
        ),
        raises(one(
            "an answer that is a number",
            &["node", "alerts", "mini"],
            "7",
            1,
        )),
        raises(one(
            "a node with no name",
            &["node", "alerts", "mini"],
            r#"{"alertsTo":"ops","node":null,"alertAfterSecs":"x"}"#,
            1,
        )),
        words("a bad duration", &["node", "alerts", "mini", "--after", "soon"], 2),
        words("a unit with no digits", &["node", "alerts", "mini", "--after", "m"], 2),
        words("no node, no --to", &["node", "alerts"], 2),
        words("--to last", &["node", "alerts", "mini", "--to"], 1),
        words("--after last", &["node", "alerts", "mini", "--after"], 1),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_node_nothing_listens() {
    let rig = Rig::new(VERB, "nodenone");
    let dead = dead_port();
    const LINES: &[&[&str]] = &[
        &["node", "add", "mini"],
        &["node", "pause", "mini"],
        &["node", "alerts", "--to", "ops"],
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

/// Help never acts: -h or --help anywhere prints help and sends nothing
/// (the binary hands it to the shell, whose `_wants_help` answers first).
#[test]
fn compute_node_help_never_acts() {
    let rig = Rig::new(VERB, "nodehelp");
    let lines: &[&[&str]] = &[
        &["node", "-h"],
        &["node", "add", "mini", "--help"],
        &["node", "confirm", "mini", "-h", "123456"],
        &["node", "remove", "--help", "mini"],
        &["node", "rm", "mini", "-h"],
        &["node", "retire", "mini", "--help"],
        &["node", "rename", "mini", "-h", "mini-2"],
        &["node", "pause", "-h", "mini"],
        &["node", "drain", "mini", "--help"],
        &["node", "resume", "mini", "-h"],
        &["node", "pinned-only", "mini", "on", "--help"],
        &["node", "route", "add", "mini", "u", "-h"],
        &["node", "alerts", "mini", "--after", "10m", "--help"],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
