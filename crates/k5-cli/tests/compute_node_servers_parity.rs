//! `k5-cli compute node health|servers ...` against the shell CLI's
//! `cli/k2 compute node ...`, on the parity rig ([`common::parity`]): the
//! same requests, stdout, stderr and exit code.
//!
//! - `health`: the full report, an offline node with no facts, `--json`,
//!   raises.
//! - `servers`: the list (and `--json`, `--refresh`), `add` (the code on
//!   stdin, `--code -`, the environment's, `--code`; limits; a held
//!   change), `approve`, `deny`, `limits` (a held change, `--json`),
//!   `pause`, `resume`, `revoke` and `owner`; the usage lines and a value
//!   flag last.
//! - One large list (thousands of servers, above 256 KiB).
//! - Help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";

fn raises(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn one(name: &'static str, args: &'static [&'static str], body: &'static str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn stdin(c: Case, input: &'static [u8]) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

/// A fixed zone, so the `since HH:MM` line is the same on each side.
fn utc(c: Case) -> Case {
    let mut env = c.env.clone();
    env.push(("TZ", "UTC".to_string()));
    Case { env, ..c }
}

const HEALTH: &str = r#"{"node":"mini","online":true,"nodeVersion":"0.46.16","remoteAdmin":"on","pendingApprovals":2,"needsPin":true,"facts":{"uptime_secs":200000,"disks":[{"path":"/","free_bytes":53687091200,"total_bytes":107374182400},{"name":"data","free_bytes":1.5,"total_bytes":true}],"mem_total_bytes":17179869184,"mem_available_bytes":8589934592,"memory_pressure":"low","disk_encryption":"on","sleep":{"autorestart":1},"updates":{"pending":3,"security":null,"reboot_required":true},"executor":{"reachable":false,"wall_restart_pending":true},"stuck_jobs":[{"job_id":"0123456789abcdef","quiet_secs":600}],"leftovers":{"count":2,"procs":[{"pid":41,"age_secs":30,"job_id":"fedcba9876543210","cmd":"cargo build"},{"pid":42}]},"security":{"wall":"job_user","job_user":"k2job","network_none":false,"open_homes":["/home/a"],"notes":["the key is readable"]}},"update":{"state":"rolling","version":"0.46.17","reason":"canary"},"alertAfterSecs":600,"alertsTo":"ops"}"#;

#[test]
fn compute_node_health() {
    let rig = Rig::new(VERB, "nodehealth");
    let v = vec![
        // covers: compute node health
        one("the full report", &["node", "health", "mini"], HEALTH, 0),
        utc(one(
            "offline, no facts",
            &["node", "health", "mini"],
            r#"{"node":"mini","online":false,"lastSeen":1763600000,"facts":{},"update":null}"#,
            0,
        )),
        one(
            "facts with no wall and leftovers none",
            &["node", "health", "mini"],
            r#"{"node":"mini","online":true,"facts":{"leftovers":{"count":0},"sleep":{"autorestart":0},"executor":{"reachable":true},"disk_encryption":"off"},"alertsTo":null}"#,
            0,
        ),
        // covers: compute node health --json
        one("--json", &["node", "health", "mini", "--json"], HEALTH, 0),
        one(
            "--json as the node",
            &["node", "health", "--json"],
            r#"{"x":1}"#,
            0,
        ),
        raises(one(
            "no online",
            &["node", "health", "mini"],
            r#"{"node":"mini"}"#,
            1,
        )),
        raises(one(
            "uptime a string",
            &["node", "health", "mini"],
            r#"{"node":"m","online":true,"facts":{"uptime_secs":"long"}}"#,
            1,
        )),
        raises(one(
            "a disk with no free_bytes",
            &["node", "health", "mini"],
            r#"{"node":"m","online":true,"facts":{"disks":[{"path":"/"}]}}"#,
            1,
        )),
        case(
            "the daemon's error",
            &["node", "health", "mini"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"no_node","hint":"none"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        words("no node", &["node", "health"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

const SERVERS: &str = r#"{"node":"mini","servers":[{"slot":0,"label":"this server","state":"active","trust":"own","solo":true},{"slot":1,"label":"globex build server with a long name","state":"paused","trust":"team","sas":"123456","limits":{"weight":2,"max_running":2,"max_queued":10,"cpu_hours_day":24,"max_job_secs":7200,"max_cpus":8,"max_mem_gb":16,"network":"none"},"usage":{"running":1,"queued":0,"cpu_secs_today":5400},"priority":true}]}"#;

const HELD: &str = r#"{"held":true,"request":{"id":"0f3c2a10-1111","summary":"servers.add globex on mini"},"warning":"the code expires soon","note":"old caches move"}"#;

#[test]
fn compute_node_servers() {
    let rig = Rig::new(VERB, "nodeservers");
    let v = vec![
        // covers: compute node servers
        one("the list", &["node", "servers", "mini"], SERVERS, 0),
        one(
            "list --refresh twice",
            &["node", "servers", "mini", "list", "--refresh", "--refresh"],
            SERVERS,
            0,
        ),
        one(
            "ls",
            &["node", "servers", "mini", "ls"],
            r#"{"role":"tenant","node":"mini","owner":"globex","line":"run 2","share":{"expires_in_secs":7200}}"#,
            0,
        ),
        // covers: compute node servers --json
        one("--json", &["node", "servers", "mini", "--json"], SERVERS, 0),
        one(
            "an empty word is the list",
            &["node", "servers", "mini", ""],
            SERVERS,
            0,
        ),
        raises(one("no JSON", &["node", "servers", "mini"], "<html>", 1)),
        words("no node", &["node", "servers"], 2),
        words("a flag for the node", &["node", "servers", "--json"], 2),
        words("an unknown act", &["node", "servers", "mini", "frob"], 2),
        // covers: compute node servers pause
        one(
            "pause",
            &["node", "servers", "mini", "pause", "globex"],
            r#"{"server":"globex","node":"mini","action":"servers.pause","note":"no new jobs"}"#,
            0,
        ),
        raises(one(
            "pause, an action that is a number",
            &["node", "servers", "mini", "pause", "g"],
            r#"{"server":"g","node":"m","action":5,"note":"n"}"#,
            1,
        )),
        words("pause, no server", &["node", "servers", "mini", "pause"], 2),
        // covers: compute node servers resume
        one(
            "resume --json",
            &["node", "servers", "mini", "resume", "globex", "--json"],
            r#"{"server":"globex","node":"mini","action":"resume","note":"ok"}"#,
            0,
        ),
        // covers: compute node servers revoke
        one(
            "revoke",
            &[
                "node", "servers", "mini", "revoke", "globex", "--now", "--wipe",
            ],
            r#"{"server":"globex","node":"mini","note":"gone"}"#,
            0,
        ),
        one(
            "revoke --json",
            &["node", "servers", "mini", "revoke", "--json", "globex"],
            r#"{"server":"globex"}"#,
            0,
        ),
        words(
            "revoke, no server",
            &["node", "servers", "mini", "revoke", "--now"],
            2,
        ),
        words(
            "revoke, an unknown flag",
            &["node", "servers", "mini", "revoke", "g", "--frob"],
            2,
        ),
        // covers: compute node servers limits
        one(
            "limits",
            &[
                "node",
                "servers",
                "mini",
                "limits",
                "globex",
                "--weight",
                "3",
                "max_running=2",
                "network=none",
                "max_job_secs=2h",
                "names=a,b,,c",
                "for=7d",
                "priority=yes",
                "--reason",
                "fair",
            ],
            r#"{"server":"globex","node":"mini","note":"set"}"#,
            0,
        ),
        one(
            "limits --for none, --json",
            &[
                "node", "servers", "mini", "limits", "globex", "--for", "none", "--json",
            ],
            r#"{"node":"mini"}"#,
            0,
        ),
        one(
            "limits, no note",
            &[
                "node", "servers", "mini", "limits", "globex", "x=[1,2]", "y= 3",
            ],
            r#"{"node":"mini"}"#,
            0,
        ),
        case(
            "limits held, not at a terminal",
            &["node", "servers", "mini", "limits", "globex", "priority=0"],
            &[],
            vec![ok(HELD)],
            owner_and_port,
            2,
            1,
        ),
        words(
            "limits, nothing to set",
            &["node", "servers", "mini", "limits", "globex"],
            2,
        ),
        words(
            "limits, no server",
            &["node", "servers", "mini", "limits", "--weight", "2"],
            2,
        ),
        words(
            "limits, a bad priority",
            &["node", "servers", "mini", "limits", "g", "priority=maybe"],
            2,
        ),
        words(
            "limits, a bad duration",
            &["node", "servers", "mini", "limits", "g", "max_job_secs=xh"],
            2,
        ),
        one(
            "limits, a duration that is a string",
            &["node", "servers", "mini", "limits", "g", "max_job_secs=2x"],
            r#"{"node":"m"}"#,
            0,
        ),
        words(
            "limits, a bad --for",
            &["node", "servers", "mini", "limits", "g", "--for", "soon"],
            2,
        ),
        // Days take digits only (0f25406366): anything else is the usage
        // line, exit 2, before anything is sent; a leading 0 is decimal.
        words(
            "limits, days that are not digits",
            &["node", "servers", "mini", "limits", "g", "for=1.5d"],
            2,
        ),
        words(
            "limits, --for days that name a variable",
            &["node", "servers", "mini", "limits", "g", "--for", "HOMEd"],
            2,
        ),
        one(
            "limits, days with a leading 0",
            &["node", "servers", "mini", "limits", "g", "for=010d"],
            r#"{"node":"m"}"#,
            0,
        ),
        case(
            "add, days that are not digits",
            &[
                "node", "servers", "mini", "add", "--url", "u", "--code", "C.FP", "--for", "x1d",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        words(
            "limits, an unknown word",
            &["node", "servers", "mini", "limits", "g", "frob"],
            2,
        ),
        words(
            "limits, an empty key",
            &["node", "servers", "mini", "limits", "g", "=3"],
            2,
        ),
        words(
            "limits, --reason last",
            &["node", "servers", "mini", "limits", "g", "--reason"],
            2,
        ),
        // covers: compute node servers owner
        case(
            "owner, held",
            &[
                "node", "servers", "mini", "owner", "globex", "--old", "revoke", "--caches",
                "wipe", "--now", "--reason", "sold",
            ],
            &[],
            vec![ok(HELD)],
            owner_and_port,
            2,
            1,
        ),
        words("owner, no server", &["node", "servers", "mini", "owner"], 2),
        words(
            "owner, a bad --old",
            &["node", "servers", "mini", "owner", "g", "--old", "x"],
            2,
        ),
        words(
            "owner, a bad --caches",
            &["node", "servers", "mini", "owner", "g", "--caches", "x"],
            2,
        ),
        words(
            "owner, --old last",
            &["node", "servers", "mini", "owner", "g", "--old"],
            2,
        ),
        // covers: compute node servers approve
        words(
            "approve, no request",
            &["node", "servers", "mini", "approve"],
            2,
        ),
        one(
            "approve, none matches",
            &["node", "servers", "mini", "approve", "77"],
            r#"{"requests":[]}"#,
            3,
        ),
        // covers: compute node servers deny
        one(
            "deny",
            &["node", "servers", "mini", "deny", "0f3c2a10", "--why", "no"],
            r#"{"request":{"id":"0f3c2a10-1111","summary":"servers.add"}}"#,
            0,
        ),
        words("deny, no id", &["node", "servers", "mini", "deny"], 2),
        // covers: compute node servers add
        stdin(
            case(
                "add, the code on stdin, held",
                &[
                    "node",
                    "servers",
                    "mini",
                    "add",
                    "--url",
                    "https://globex.example",
                    "--code-stdin",
                    "--trust",
                    "team",
                    "--for",
                    "7d",
                    "--cpus",
                    "4",
                    "--mem",
                    "16G",
                    "--disk",
                    "100g",
                    "--network",
                    "none",
                    "--gate",
                    "--exclusive",
                    "--max-time",
                    "1h30m",
                    "--weight",
                    "2",
                    "--max-running",
                    "1",
                    "--max-queued",
                    "5",
                    "--cpu-hours",
                    "10",
                    "--reason",
                    "ci",
                ],
                &[],
                vec![ok(HELD)],
                owner_and_port,
                2,
                1,
            ),
            b"ABCD.ff00\r\n",
        ),
        stdin(
            case(
                "add, --code -",
                &[
                    "node", "servers", "mini", "add", "--url", "u", "--code", "-", "--for", "90m",
                ],
                &[],
                vec![ok(r#"{"held":false,"request":{"id":"r2","summary":"s"}}"#)],
                owner_and_port,
                2,
                1,
            ),
            b"EFGH.00",
        ),
        case(
            "add, the environment's code",
            &["node", "servers", "mini", "add", "--url", "u"],
            &[("K2_NODE_ENROLL_CODE", "ENV.11")],
            vec![ok(HELD)],
            owner_and_port,
            2,
            1,
        ),
        stdin(
            words(
                "add, no code on stdin",
                &[
                    "node",
                    "servers",
                    "mini",
                    "add",
                    "--url",
                    "u",
                    "--code-stdin",
                ],
                2,
            ),
            b"\n",
        ),
        words(
            "add, --code and --code-stdin",
            &[
                "node",
                "servers",
                "mini",
                "add",
                "--url",
                "u",
                "--code",
                "C",
                "--code-stdin",
            ],
            2,
        ),
        words(
            "add, no url",
            &["node", "servers", "mini", "add", "--code", "C"],
            2,
        ),
        words(
            "add, an unknown flag",
            &["node", "servers", "mini", "add", "--frob"],
            2,
        ),
        words(
            "add, a bad --max-time",
            &[
                "node",
                "servers",
                "mini",
                "add",
                "--url",
                "u",
                "--code",
                "C",
                "--max-time",
                "soon",
            ],
            2,
        ),
        words(
            "add, --url last",
            &["node", "servers", "mini", "add", "--url"],
            2,
        ),
        raises(one(
            "add, an answer that is a list",
            &[
                "node", "servers", "mini", "add", "--url", "u", "--code", "C",
            ],
            "[1]",
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The large-list case: thousands of servers, an answer above 256 KiB.
#[test]
fn compute_node_servers_large_list() {
    let rig = Rig::new(VERB, "nodeserversbig");
    let rows: Vec<String> = (0..3000u64)
        .map(|i| {
            format!(
                r#"{{"slot":{i},"label":"server number {i} of the fleet","state":"active","trust":"team","limits":{{"weight":1,"max_running":2,"max_queued":4,"cpu_hours_day":8,"max_job_secs":3600,"max_cpus":4,"max_mem_gb":8,"network":"none"}}}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"node":"mini","servers":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    let body: &'static str = Box::leak(body.into_boxed_str());
    rig.check(&one("3000 servers", &["node", "servers", "mini"], body, 0));
}

/// The lines name the command by the brand's name.
#[test]
fn the_node_servers_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "nodeserversbrand");
    for (args, want) in [
        (
            &["node", "health"][..],
            format!("usage: {cli} compute node health <node> [--json]\n"),
        ),
        (
            &["node", "servers", "mini", "pause"],
            format!("usage: {cli} compute node servers <node> pause <server>\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// Help never acts.
#[test]
fn compute_node_servers_help_never_acts() {
    let rig = Rig::new(VERB, "nodeservershelp");
    let lines: &[&[&str]] = &[
        &[
            "node", "servers", "mini", "add", "--url", "u", "--code", "C", "-h",
        ],
        &["node", "servers", "mini", "approve", "r1", "--help"],
        &["node", "servers", "mini", "deny", "r1", "-h"],
        &["node", "servers", "mini", "limits", "g", "x=1", "--help"],
        &["node", "servers", "mini", "pause", "g", "-h"],
        &["node", "servers", "mini", "resume", "-h", "g"],
        &["node", "servers", "mini", "revoke", "g", "--help"],
        &["node", "servers", "mini", "owner", "g", "-h"],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
