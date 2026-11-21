//! `k5-cli compute node ...` against the shell CLI's `cli/k2 compute
//! node ...`, on the parity rig ([`common::parity`]): the same requests,
//! stdout, stderr and exit code.
//!
//! - The one-POST subverbs: `add` (and `--json`), `confirm`, `remove`,
//!   `rm`, `retire`, `rename`, `pause`, `drain`, `resume`, `pinned-only`,
//!   `route` and `alerts`; `node` alone and an unknown subverb.
//! - `wall`, `update`, `rollback`, `rollout`, `policy`, `admin` (propose,
//!   list, approve, deny, withdraw) and `take-owner`: a held change and
//!   where the owner approves it, the approve flow up to the confirm, one
//!   large list.
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

/// A case the binary hands to the shell.
fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
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
        words("--url last", &["node", "add", "mini", "--url"], 2),
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
        words(
            "pinned-only, a bad word",
            &["node", "pinned-only", "mini", "yes"],
            2,
        ),
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
        words(
            "a bad duration",
            &["node", "alerts", "mini", "--after", "soon"],
            2,
        ),
        words(
            "a unit with no digits",
            &["node", "alerts", "mini", "--after", "m"],
            2,
        ),
        words("no node, no --to", &["node", "alerts"], 2),
        words("--to last", &["node", "alerts", "mini", "--to"], 2),
        words("--after last", &["node", "alerts", "mini", "--after"], 2),
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

const HELD: &str = r#"{"held":true,"request":{"id":"0f3c2a10-1111-2222-3333-444455556666","summary":"wall.enable on mini"}}"#;
const HELD_CARD: &str = r#"{"held": true,"request":{"id":"r2","summary":"policy.set on mini"},"permission":{"command":"k2 permission request compute.node-admin request=r2","requestable":true}}"#;

#[test]
fn compute_node_wall_and_policy() {
    let rig = Rig::new(VERB, "nodewall");
    let v = vec![
        // covers: compute node wall
        one(
            "the wall on",
            &["node", "wall", "mini", "--reason", "jobs read the key"],
            r#"{"node":"mini","note":"the job wall is on; k2-node restarts."}"#,
            0,
        ),
        one(
            "no note",
            &["node", "wall", "--no-restart", "mini"],
            r#"{"node":"mini"}"#,
            0,
        ),
        one("held for the owner", &["node", "wall", "mini"], HELD, 2),
        in_session(one(
            "held, in a session",
            &["node", "wall", "mini"],
            HELD,
            2,
        )),
        one(
            "held, a card to ask with",
            &["node", "wall", "mini"],
            HELD_CARD,
            2,
        ),
        raises(one(
            "held with no request",
            &["node", "wall", "mini"],
            r#"{"held":true}"#,
            1,
        )),
        raises(one(
            "held with no summary",
            &["node", "wall", "mini"],
            r#"{"held":true,"request":{"id":"r1"}}"#,
            1,
        )),
        words("wall, no node", &["node", "wall", "--no-restart"], 2),
        words(
            "wall, an unknown flag",
            &["node", "wall", "mini", "--now"],
            2,
        ),
        words(
            "wall, --reason last",
            &["node", "wall", "mini", "--reason"],
            1,
        ),
        // covers: compute node policy show
        one(
            "a policy",
            &["node", "policy", "mini"],
            r#"{"node":"mini","online":true,"remoteAdmin":"on","policy":{"updates":"auto","maxJobs":4,"labels":["gpu","fast"]}}"#,
            0,
        ),
        one(
            "no policy yet",
            &["node", "policy", "mini", "show"],
            r#"{"node":"mini","online":false,"hint":"the node reports its policy when it next checks in."}"#,
            0,
        ),
        one(
            "show --json",
            &["node", "policy", "mini", "show", "--json"],
            r#"{"node":"mini","online":true,"policy":{}}"#,
            0,
        ),
        raises(one(
            "a policy that is a string",
            &["node", "policy", "mini"],
            r#"{"node":"mini","online":true,"policy":"auto"}"#,
            1,
        )),
        words("policy, no node", &["node", "policy"], 2),
        words(
            "policy, an unknown act",
            &["node", "policy", "mini", "--json"],
            2,
        ),
        // covers: compute node policy set
        one(
            "policy set",
            &[
                "node",
                "policy",
                "mini",
                "set",
                "maxJobs=4",
                "labels=gpu,,fast",
                "updates=auto",
                "maxJobs=5",
                "flag=true",
                "--reason",
                "more room",
            ],
            r#"{"node":"mini","note":"sent to the node."}"#,
            0,
        ),
        one(
            "policy set, held",
            &["node", "policy", "mini", "set", "updates=off"],
            HELD,
            2,
        ),
        words(
            "policy set, no pairs",
            &["node", "policy", "mini", "set"],
            2,
        ),
        words(
            "policy set, a bare word",
            &["node", "policy", "mini", "set", "a=1", "b"],
            2,
        ),
        words(
            "policy set, an empty key",
            &["node", "policy", "mini", "set", " =1"],
            2,
        ),
        words(
            "policy set, --reason last",
            &["node", "policy", "mini", "set", "a=1", "--reason"],
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn compute_node_update_and_rollout() {
    let rig = Rig::new(VERB, "nodeupdate");
    let rollout = r#"{"rollout":{"id":"abcdef0123456789","toVersion":"0.46.16","state":"running","trigger":"manual","batchSize":3,"position":{"settled":1,"total":4},"note":"one at a time per OS","nodes":[{"node":"mini","state":"done"},{"node":"linux-1","state":"failed","code":"E1","detail":"disk full"},{"node":"linux-2","state":"pending","code":"waiting_window","detail":"after 02:00"},{"node":"mac-2","state":"running","detail":"downloading"}]},"note":"auto-follow is on."}"#;
    let v = vec![
        // covers: compute node update
        one(
            "one node",
            &["node", "update", "mini", "--canary", "--now"],
            r#"{"node":"mini","update":{"state":"queued","version":"0.46.16"},"note":"it updates when idle."}"#,
            0,
        ),
        one(
            "no update object",
            &["node", "update", "mini"],
            r#"{"node":"mini"}"#,
            0,
        ),
        raises(one(
            "a version that is no string",
            &["node", "update", "mini"],
            r#"{"node":"mini","update":{"state":"queued","version":16}}"#,
            1,
        )),
        one(
            "all, a rollout",
            &["node", "update", "--all", "--batch", "3", "--now"],
            rollout,
            0,
        ),
        one(
            "all, nothing yet, follow off",
            &["node", "update", "--all"],
            r#"{"rollout":null,"follow":false}"#,
            0,
        ),
        shell(case(
            "all, a batch with a leading zero",
            &["node", "update", "--all", "--batch", "03"],
            &[],
            vec![ok(r#"{"rollout":null}"#)],
            owner_and_port,
            0,
            1,
        )),
        words("all with a node", &["node", "update", "mini", "--all"], 2),
        words("rollback --all", &["node", "rollback", "--all"], 2),
        words(
            "--batch without --all",
            &["node", "update", "mini", "--batch", "2"],
            2,
        ),
        words(
            "--batch, not a number",
            &["node", "update", "--all", "--batch", "x"],
            2,
        ),
        words("--batch last", &["node", "update", "--all", "--batch"], 2),
        words("an unknown flag", &["node", "update", "mini", "--force"], 2),
        words("update, no node", &["node", "update"], 2),
        // covers: compute node rollback
        one(
            "rollback",
            &["node", "rollback", "mini"],
            r#"{"node":"mini","update":{"state":"rolling_back"}}"#,
            0,
        ),
        words("rollback, no node", &["node", "rollback", "--now"], 2),
        // covers: compute node rollout
        one("rollout", &["node", "rollout"], rollout, 0),
        one("no rollout", &["node", "rollout", "show"], r#"{}"#, 0),
        // covers: compute node rollout --json
        one("rollout --json", &["node", "rollout", "--json"], rollout, 0),
        one(
            "pause by id",
            &["node", "rollout", "pause", "--id", "abcdef01"],
            r#"{"rollout":{"id":"abcdef01","toVersion":"0.46.16","state":"paused","trigger":"follow","now":true,"position":{}}}"#,
            0,
        ),
        one("stop", &["node", "rollout", "stop"], rollout, 0),
        raises(one(
            "a rollout with no id",
            &["node", "rollout"],
            r#"{"rollout":{"toVersion":"0.46.16"}}"#,
            1,
        )),
        words("rollout, a bad word", &["node", "rollout", "go"], 2),
        words(
            "rollout, --id last",
            &["node", "rollout", "resume", "--id"],
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

const REQUESTS: &str = r#"{"requests":[{"id":"0f3c2a10-1111-2222-3333-444455556666","state":"pending","node":"mini","summary":"script on mini: clear the build cache, then restart the executor so jobs start clean again","reason":"disk full","body":{"steps":[{"verb":"script","args":{"interpreter":"/bin/bash","text":"rm -rf /var/cache/k2\nsystemctl restart k2-exec"}},{"verb":"svc.restart","args":{}}]}},{"id":"9a000000-0000-0000-0000-000000000000","state":"denied","node":null,"summary":"wall.enable","denyReason":"not now"}]}"#;

#[test]
fn compute_node_admin() {
    let rig = Rig::new(VERB, "nodeadmin");
    let v = vec![
        // covers: compute node admin list
        one("requests", &["node", "admin", "list"], REQUESTS, 0),
        one(
            "none",
            &["node", "admin", "ls", "--state", "open"],
            r#"{"requests":[]}"#,
            0,
        ),
        // covers: compute node admin list --json
        one(
            "list --json",
            &[
                "node", "admin", "list", "--node", "mini box", "--limit", "5", "--json",
            ],
            REQUESTS,
            0,
        ),
        raises(one(
            "rows that are a number",
            &["node", "admin", "list"],
            r#"{"requests":0}"#,
            1,
        )),
        words("list, a bad flag", &["node", "admin", "list", "--all"], 2),
        words("list, --node last", &["node", "admin", "list", "--node"], 2),
        // covers: compute node admin propose
        one(
            "a proposal, held",
            &[
                "node",
                "admin",
                "propose",
                "mini",
                "svc.restart",
                "unit=k2-exec",
                "--reason",
                "stuck",
                "--timeout",
                "1h30m",
            ],
            HELD,
            2,
        ),
        one(
            "the PRD shape, held with a card",
            &["node", "admin", "mini", "pkg.install", "names=jq,curl"],
            HELD_CARD,
            2,
        ),
        raises(one(
            "an answer with no request",
            &["node", "admin", "propose", "mini", "svc.restart"],
            r#"{"ok":true}"#,
            1,
        )),
        words("propose, no verb", &["node", "admin", "propose", "mini"], 2),
        words(
            "propose, three words",
            &["node", "admin", "propose", "mini", "a", "b"],
            2,
        ),
        words(
            "propose, a script with no file",
            &["node", "admin", "propose", "mini", "script"],
            2,
        ),
        words(
            "propose, no plan file",
            &[
                "node",
                "admin",
                "propose",
                "mini",
                "--plan",
                "/nonexistent/plan.json",
            ],
            2,
        ),
        words(
            "propose, a bad timeout",
            &["node", "admin", "propose", "mini", "x", "--timeout", "soon"],
            2,
        ),
        words(
            "propose, --timeout last",
            &["node", "admin", "propose", "mini", "x", "--timeout"],
            2,
        ),
        words(
            "propose, an empty key",
            &["node", "admin", "propose", "mini", "x", "=1"],
            2,
        ),
        words(
            "propose, --reason last",
            &["node", "admin", "propose", "mini", "x", "--reason"],
            1,
        ),
        // covers: compute node admin deny
        one(
            "denied",
            &["node", "admin", "deny", "0f3c2a10", "--why", "not now"],
            r#"{"request":{"id":"0f3c2a10-1111","summary":"wall.enable on mini"}}"#,
            0,
        ),
        words("deny, no id", &["node", "admin", "deny", "--why", "x"], 2),
        // covers: compute node admin withdraw
        one(
            "withdrawn",
            &["node", "admin", "withdraw", "0f3c2a10"],
            r#"{"request":{"id":"0f3c2a10-1111"}}"#,
            0,
        ),
        raises(one(
            "withdrawn, no request",
            &["node", "admin", "withdraw", "r1"],
            "{}",
            1,
        )),
        words("withdraw, no id", &["node", "admin", "withdraw"], 2),
        // covers: compute node admin approve
        in_session(case(
            "approve in a session",
            &["node", "admin", "approve", "0f3c2a10"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )),
        one(
            "no request matches",
            &["node", "admin", "approve", "77"],
            REQUESTS,
            3,
        ),
        one(
            "two match",
            &["node", "admin", "approve", "9"],
            r#"{"requests":[{"id":"9a"},{"id":"9b"}]}"#,
            3,
        ),
        raises(one(
            "an id that is a number",
            &["node", "admin", "approve", "1"],
            r#"{"requests":[{"id":1}]}"#,
            3,
        )),
        one(
            "not pending",
            &["node", "admin", "approve", "9a000000"],
            REQUESTS,
            3,
        ),
        words("approve, no id", &["node", "admin", "approve"], 2),
        // covers: compute node take-owner
        // With no terminal to open, the binary hands take-owner to the
        // shell, whose /dev/tty redirect error names the script: no row
        // can compare that line (the D6 class).
        words("take-owner, no node", &["node", "take-owner"], 2),
        words("take-owner, a flag", &["node", "take-owner", "--yes"], 2),
        words(
            "take-owner, two words",
            &["node", "take-owner", "mini", "x"],
            2,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The large-list case: thousands of admin requests, an answer above
/// 256 KiB.
#[test]
fn compute_node_admin_large_list() {
    let rig = Rig::new(VERB, "nodeadminbig");
    let rows: Vec<String> = (0..3000u64)
        .map(|i| {
            format!(
                r#"{{"id":"{i:08x}-0000-0000-0000-000000000000","state":"pending","node":"node-{i}","summary":"svc.restart on node-{i}","reason":"request number {i}"}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"requests":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    let body: &'static str = Box::leak(body.into_boxed_str());
    rig.check(&one("3000 requests", &["node", "admin", "list"], body, 0));
}

/// Help never acts, for the held and confirm verbs too.
#[test]
fn compute_node_admin_help_never_acts() {
    let rig = Rig::new(VERB, "nodeadminhelp");
    let lines: &[&[&str]] = &[
        &["node", "wall", "mini", "-h"],
        &["node", "update", "--all", "--help"],
        &["node", "rollback", "mini", "-h"],
        &["node", "rollout", "stop", "--help"],
        &["node", "policy", "mini", "set", "a=1", "-h"],
        &["node", "admin", "propose", "mini", "svc.restart", "--help"],
        &["node", "admin", "-h"],
        &["node", "admin", "approve", "r1", "-h"],
        &["node", "admin", "deny", "r1", "--help"],
        &["node", "admin", "withdraw", "-h", "r1"],
        &["node", "take-owner", "mini", "--help"],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
