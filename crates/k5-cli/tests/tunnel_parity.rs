//! `k5-cli tunnel` against the shell CLI's `cli/k2 tunnel`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code for every form of `cmd_tunnel`.
//!
//! - Each action: `start` (`--subdomain`, a repair), `stop`, `disable`,
//!   `enable` (the auto-start words, running or not), `switch` (the token
//!   on argv and on stdin, its steps, a refusal, a roll back, each word it
//!   stops on), `release` and `forget` (without `--confirm`, with
//!   `tunnel.json` and without; with it, each answer), `status` (healthy,
//!   failing, released, disabled, locked, the relay list, the voice lane,
//!   the listener; `--json`), `subdomains` (names, none, an empty map;
//!   `--json`; one answer above 256 KiB), `boot-status` (shown, set, a
//!   word it doesn't take), `help`, an unknown action, no action.
//! - The answers a reader can't read (empty, not JSON) and those of a
//!   shape it doesn't expect (Python's traceback).
//! - The owner from disk and a passport; the gates; air-gap on.
//! - No answer: an empty reply, a reset, nothing listening.
//! - What the binary hands to the shell: help with `-h` or `--help` in a
//!   place that is not a help page. (A value flag as the last word is the
//!   shell's too; its `$2: unbound variable` line names the script's path,
//!   which differs between the two sides, so no row here: K2's D6 class.)

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "tunnel";

const STATUS: &str = r#"{"running":true,"frpc_healthy":true,"loginIngress":"edge","bootStatus":"ghost","public_url":"https://acme.k2.dev","visitorIps":{"words":"visitor addresses: passed","relayAsClient":2},"local_port":4100,"server_addr":"relay.k2.dev","server_port":7000,"relay_primary":true,"failover":{"detail":"waiting for DNS"},"relay_list":{"relays":["a.example","b.example"],"source":"fetched","replaced":["192.0.2.1"],"relay_default":"auto","relay_default_set_by":"tunnel.json","config_file":"/k/tunnel.json","checked_at":1763600000,"active":"a.example","notice":"not listed"},"voiceLane":{"state":"up","turnHost":"turn.example","serverAddr":"a.example","serverPort":3478,"adopted":true,"mediaPath":{"state":"on","sewHasRoute":false}},"autoStart":true,"subdomain":"Acme","e2eListener":{"subdomain":"old","port":8443}}"#;

const FAILING: &str = r#"{"running":true,"frpc_healthy":false,"frpc_consecutive_failures":4,"frpc_last_error":"connection refused é","frpc_config_present":false,"frpc_config":"/k/frpc.toml","relay_list":{"relays":[],"relay_default":"legacy","relay_default_set_by":"env","last_error":"401 unauthorized"},"voiceLane":{"state":"restarting","lastError":"exit 1","mediaPath":{"state":"off"}},"autoStart":false,"e2eListener":null}"#;

const SUBDOMAINS: &str = r#"{"primary":"acme","targets":{"shop":{"target":"127.0.0.1:3000","projectId":"p1"},"docs":"127.0.0.1:4000","api":{"projectId":"p2"},"z":{"target":"http://[::1]:9","projectId":"p3"}}}"#;
const PROJECTS: &str = r#"[{"id":"p1","name":"Shop é"},{"id":"p2"},"x",{"id":"p3","name":""}]"#;

fn reply(body: &str) -> Reply {
    ok(body)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn stdin(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

fn tunnel_json(r: &Rig) {
    owner_and_port(r);
    std::fs::write(
        r.k2home.join("tunnel.json"),
        "{\"subdomain\": \"  acme-shop\\n\", \"token\": \"x\"}\n",
    )
    .unwrap();
}

fn tunnel_json_odd(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.k2home.join("tunnel.json"), "{\"subdomain\": 7}").unwrap();
}

/// A routing map with thousands of names: its answer is above 256 KiB.
fn many_targets() -> String {
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#""label-{i:05}-{}":{{"target":"127.0.0.1:{}","projectId":"p{}"}}"#,
                "é".repeat(i % 30),
                3000 + i % 1000,
                i % 7
            )
        })
        .collect();
    format!(r#"{{"primary":"acme","targets":{{{}}}}}"#, rows.join(","))
}

#[test]
fn tunnel_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        case(
            "start",
            &["start"],
            &[],
            vec![reply(r#"{"public_url":"https://acme.k2.dev"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "start --subdomain, repaired, no url",
            &["start", "x", "--subdomain", "Acmé shop&1"],
            &[],
            vec![reply(
                r#"{"repaired":"frpc was not running","public_url":""}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "start: the daemon's error",
            &["start", "--subdomain", ""],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":"tunnel token missing"}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "start: a refusal with a permission card",
            &["start"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "stop",
            &["stop"],
            &[],
            vec![reply(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stop: not ok",
            &["stop"],
            &[],
            vec![reply(r#"{"ok":false}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: tunnel disable
        case(
            "disable",
            &["disable"],
            &[],
            vec![reply("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "disable: an error",
            &["disable"],
            &[],
            vec![reply(r#"{"error":[1,"two"]}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: tunnel enable
        case(
            "enable: running",
            &["enable"],
            &[],
            vec![reply(
                r#"{"running":true,"public_url":"https://a.k2.dev","autoStart":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "enable: not running, auto-start off",
            &["enable", "--no-auto-start", "--subdomain", "acme"],
            &[],
            vec![reply(r#"{"running":false,"autoStart":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "enable: auto-start neither",
            &["enable", "--no-auto-start", "--auto-start", "x"],
            &[],
            vec![reply(r#"{"autoStart":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: tunnel switch
        case(
            "switch: steps and the new name",
            &["switch", "acme", "--token", "tk-1", "--start"],
            &[],
            vec![reply(
                r#"{"ok":true,"subdomain":"acme","steps":[{"ok":true,"step":"bind","detail":"held"},{"step":"cert"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "switch: refused",
            &["switch", "acme", "--no-start"],
            &[],
            vec![reply(r#"{"error":"not yours"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "switch: failed and rolled back",
            &["switch", "é"],
            &[],
            vec![reply(
                r#"{"ok":false,"rolledBack":true,"steps":[{"ok":false,"step":"bind","detail":null}],"error":"taken"}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        stdin(
            b"tk-\\stdin 1\nsecond line\n",
            case(
                "switch: the token on stdin",
                &["switch", "acme", "--token-stdin"],
                &[],
                vec![reply(r#"{"ok":true,"subdomain":"acme"}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        stdin(
            b"tk-no-newline",
            case(
                "switch: a token with no newline",
                &["switch", "acme", "--token-stdin"],
                &[],
                vec![reply(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        case(
            "switch: no token on stdin",
            &["switch", "acme", "--token-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        stdin(
            b"\nlater",
            case(
                "switch: an empty first line",
                &["switch", "acme", "--token-stdin"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
        ),
        case(
            "switch: no subdomain",
            &["switch"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "switch: an unknown flag",
            &["switch", "acme", "--force"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "switch: both tokens",
            &["switch", "acme", "--token", "t", "--token-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: tunnel release
        case(
            "release without --confirm, no tunnel.json",
            &["release"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "release without --confirm, tunnel.json names it",
            &["release", "x"],
            &[],
            vec![],
            tunnel_json,
            1,
            0,
        ),
        case(
            "release without --confirm, a subdomain that is no string",
            &["release"],
            &[],
            vec![],
            tunnel_json_odd,
            1,
            0,
        ),
        case(
            "release --confirm",
            &["release", "--confirm"],
            &[],
            vec![reply(
                r#"{"subdomain":" acme ","upstreamReported":true,"note":"bye"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "release --confirm: already released, queued",
            &["release", "--confirm"],
            &[],
            vec![reply("{\"alreadyReleased\":1,\"queued\":true}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "release --confirm: an error",
            &["release", "--confirm"],
            &[],
            vec![reply(r#"{"error":"busy"}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: tunnel forget
        case(
            "forget without --confirm",
            &["forget"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "forget --confirm",
            &["forget", "--confirm"],
            &[],
            vec![reply(r#"{"localOnly":true,"subdomain":"old"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status",
            &["status"],
            &[],
            vec![reply(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no action is status",
            &[],
            &[],
            vec![reply(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: tunnel
        case(
            "an empty action is status",
            &[""],
            &[],
            vec![reply(FAILING)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status: released and locked",
            &["status", "x"],
            &[],
            vec![reply(
                r#"{"running":false,"released":true,"token_locked":"open the vault","autoStart":0}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status: disabled, relays a string",
            &["status"],
            &[],
            vec![reply(
                r#"{"enabled":false,"relay_list":{"relays":"ab","source":"built-in","last_error":"timeout"},"bootStatus":"compat","bootStatusExplicit":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status --json: as it came",
            &["status", "--json"],
            &[],
            vec![reply("{\"running\": true}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        // covers: tunnel subdomains
        case(
            "subdomains",
            &["subdomains"],
            &[],
            vec![reply(SUBDOMAINS), reply(PROJECTS)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "subdomains: no names (an error answer)",
            &["subdomains", "x"],
            &[],
            vec![
                reply(r#"{"targets":{"b":"x","a":{"target":"y","projectId":"p1"}}}"#),
                Reply::Answer(500, r#"{"error":"nope"}"#.into()),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "subdomains: an empty map",
            &["subdomains"],
            &[],
            vec![reply("{}"), reply("[]")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "subdomains: only the primary",
            &["subdomains"],
            &[],
            vec![reply(r#"{"primary":" acme "}"#), reply(PROJECTS)],
            owner_and_port,
            0,
            2,
        ),
        // covers: tunnel subdomains --json
        case(
            "subdomains --json",
            &["subdomains", "--json"],
            &[],
            vec![reply(SUBDOMAINS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "subdomains: a large map, above 256 KiB",
            &["subdomains"],
            &[],
            vec![reply(&many_targets()), reply(PROJECTS)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "boot-status",
            &["boot-status"],
            &[],
            vec![reply(r#"{"bootStatus":"ghost"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "boot-status: compat, explicit",
            &["boot-status", ""],
            &[],
            vec![reply(
                r#"{"bootStatus":"compat","bootStatusExplicit":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "boot-status: an older daemon",
            &["boot-status"],
            &[],
            vec![reply("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "boot-status ghost",
            &["boot-status", "ghost"],
            &[],
            vec![reply(r#"{"bootStatusPublic":"ghost"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "boot-status compat: the daemon's error",
            &["boot-status", "compat"],
            &[],
            vec![reply(r#"{"error":"owner only"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "boot-status ghost: not JSON",
            &["boot-status", "ghost"],
            &[],
            vec![reply("<html>\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "boot-status: a word it doesn't take",
            &["boot-status", "loud"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("help", &["help", "x"], &[], vec![], owner_and_port, 0, 0),
        case(
            "an unknown action",
            &["nope", "--json"],
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

/// Answers a reader can't read, and shapes its Python doesn't expect.
#[test]
fn tunnel_reads_what_it_can() {
    let rig = Rig::new(VERB, "odd");
    let v = vec![
        case(
            "start: empty",
            &["start"],
            &[],
            vec![reply("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "stop: not JSON",
            &["stop"],
            &[],
            vec![reply("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "enable: cut short",
            &["enable"],
            &[],
            vec![reply("{\"running\": tr")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "release --confirm: extra data",
            &["release", "--confirm"],
            &[],
            vec![reply("{} {}")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status: not JSON, two lines",
            &["status"],
            &[],
            vec![reply("ok\nnot json")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "subdomains: a bad escape",
            &["subdomains"],
            &[],
            vec![reply(r#"{"primary":"\x"}"#), reply("[]")],
            owner_and_port,
            1,
            2,
        ),
        case(
            "boot-status: a missing comma",
            &["boot-status"],
            &[],
            vec![reply(r#"{"a":1 "b":2}"#)],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "start: a list",
            &["start"],
            &[],
            vec![reply("[]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "start: a repair that is no string",
            &["start"],
            &[],
            vec![reply(r#"{"repaired":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "switch: steps that are a string",
            &["switch", "a"],
            &[],
            vec![reply(r#"{"steps":"ab"}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "status: null",
            &["status"],
            &[],
            vec![reply("null")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "status: a relay that is no string",
            &["status"],
            &[],
            vec![reply(r#"{"relay_list":{"relays":["a",1]}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "status: a listener that is no object",
            &["status"],
            &[],
            vec![reply(r#"{"e2eListener":"x"}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "subdomains: a target with no length",
            &["subdomains"],
            &[],
            vec![reply(r#"{"targets":{"a":{"target":5}}}"#), reply("[]")],
            owner_and_port,
            1,
            2,
        )),
        traceback(case(
            "subdomains: a primary that is no string",
            &["subdomains"],
            &[],
            vec![reply(r#"{"primary":3}"#), reply("[]")],
            owner_and_port,
            1,
            2,
        )),
        traceback(case(
            "boot-status ghost: a number",
            &["boot-status", "ghost"],
            &[],
            vec![reply("5")],
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
fn tunnel_with_a_passport_and_the_gates() {
    let rig = Rig::new(VERB, "pass");
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a host session: the passport over TCP",
                &["start"],
                &[],
                vec![reply("{}")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a cell socket: status still over TCP",
                &["status"],
                &[],
                vec![reply(STATUS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a re-minted passport is used from the start",
                &["subdomains"],
                &[],
                vec![reply(SUBDOMAINS), reply(PROJECTS)],
                healed_on_disk,
                0,
                2,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: an open tool goes on",
            &["stop"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![reply(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no port: the gate",
            &["status"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "no token: the gate",
            &["release"],
            &[],
            vec![],
            port_only,
            1,
            0,
        ),
        case(
            "air-gap on: enable",
            &["enable"],
            &[("K2_AIRGAP", "1")],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "air-gap on: switch, before its words",
            &["switch"],
            &[("K2_AIRGAP", "")],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "air-gap on: enable with no daemon, before the gates",
            &["enable", "--subdomain"],
            &[("K2_AIRGAP", "1")],
            vec![],
            nothing,
            2,
            0,
        ),
        case(
            "air-gap on: switch with no daemon, the gate first",
            &["switch", "acme"],
            &[("K2_AIRGAP", "1")],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "air-gap on: start is not refused",
            &["start"],
            &[("K2_AIRGAP", "1")],
            vec![reply("{}")],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn tunnel_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let waited = |c: Case| Case {
        compare: Compare::Waited,
        ..c
    };
    let v = vec![
        case(
            "start: an empty reply",
            &["start"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "stop: a reset",
            &["stop"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "release --confirm: an empty reply",
            &["release", "--confirm"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        waited(case(
            "status: an empty reply, the line",
            &["status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        case(
            "status: a reset, curl's 56",
            &["status"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "subdomains: no answer for the names, no line",
            &["subdomains"],
            &[],
            vec![reply(SUBDOMAINS), Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        case(
            "subdomains: a reset for the names",
            &["subdomains"],
            &[],
            vec![reply(SUBDOMAINS), Reply::Reset],
            owner_and_port,
            56,
            2,
        ),
        // A no answer on the left of the pipe: macOS's bash 3.2 runs the
        // reader on an empty input (`bash32-pipe-reader`).
        Case {
            compare: Compare::Bash32PipeReader {
                stdout: "",
                stderr: "Error: daemon returned an unparseable response: ''\n",
            },
            ..case(
                "boot-status ghost: an empty reply",
                &["boot-status", "ghost"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            )
        },
        case(
            "boot-status: a reset",
            &["boot-status"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn tunnel_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "start: nothing listens",
        &["start"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
    rig.check(&case(
        "status: nothing listens, curl's 7",
        &["status"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        7,
        0,
    ));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn tunnel_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    // Help never acts (K2's rule): -h or --help anywhere prints the
    // tunnel help on stdout, exit 0, and sends nothing (`cmd_tunnel`'s
    // `_wants_help`).
    let v = vec![
        shell(case(
            "status --help: the help, nothing sent",
            &["status", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "start --help: the help, nothing sent",
            &["start", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "stop -h: the help, nothing sent",
            &["stop", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "disable --help: the help, nothing sent",
            &["disable", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "enable -h: the help, nothing sent",
            &["enable", "-h", "--no-auto-start"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "release --confirm --help: the help, nothing sent",
            &["release", "--confirm", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "forget -h --confirm: the help, nothing sent",
            &["forget", "-h", "--confirm"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "boot-status ghost --help: the help, nothing sent",
            &["boot-status", "ghost", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "switch --help: the help, nothing sent",
            &["switch", "acme", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "release --help: the help, nothing sent",
            &["release", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
