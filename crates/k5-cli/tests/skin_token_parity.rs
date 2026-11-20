//! `k5-cli skin-token` against the shell CLI's `cli/k2 skin-token`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - Each verb (`create`, `list`, `rooms`, `revoke`) with its answers:
//!   the daemon's errors through `_skin_die_if_err` (an object with each
//!   code kind, a string, `ok: false`, not JSON, empty), odd values and
//!   Python's raise part way, and one list above 256 KiB.
//! - The words: usage lines, a value flag as the last word (`shift 2`
//!   fails: exit 1), positional words, `--clear`.
//! - The owner from disk, a passport over TCP (the shell calls
//!   `cli_request` and `cli_post_json` directly: never the cell socket),
//!   a healed passport, a refusal with a permission card, the id tool's
//!   checks; the gates.
//! - No answer: the line and exit 1; a reset, a dead port: curl's code.
//! - What the binary hands to the shell: help, a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "skin-token";
const CREATED: &str = r#"{"id":"t1","name":"site","caps":["thread:read"],"roomHandles":["sales"],"token":"k2skn_abc"}"#;
const LISTED: &str = r#"{"tokens":[{"id":"t1","name":"site","prefix":"k2skn_ab","caps":["thread:read","files:read"],"roomHandles":["sales","ops"]},{"id":"t2","username":"old","revokedAt":"2026-11-01"}]}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn skin_token_list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let big: String = {
        let rows: Vec<String> = (0..3000)
            .map(|i| {
                format!(
                    r#"{{"id":"tok-{i:05}","name":"label {i} é","prefix":"k2skn_{i:04}","caps":["thread:read","thread:post","files:read"],"roomHandles":["room-{i}","sales"]}}"#
                )
            })
            .collect();
        format!(r#"{{"tokens":[{}]}}"#, rows.join(","))
    };
    assert!(big.len() > 256 * 1024, "the large list is {} bytes", big.len());
    let v = vec![
        case("list", &["list"], &[], vec![ok(LISTED)], owner_and_port, 0, 1),
        case(
            "words after list are not read",
            &["list", "--json", "x"],
            &[],
            vec![ok(LISTED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a large list, above 256 KiB",
            &["list"],
            &[],
            vec![ok(&big)],
            owner_and_port,
            0,
            1,
        ),
        case("none", &["list"], &[], vec![ok(r#"{"tokens":[]}"#)], owner_and_port, 0, 1),
        case("no tokens key", &["list"], &[], vec![ok("{}")], owner_and_port, 0, 1),
        case("tokens null", &["list"], &[], vec![ok(r#"{"tokens":null}"#)], owner_and_port, 0, 1),
        case(
            "the owner-only refusal",
            &["list"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"owner_only","hint":"Owner only."}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the refusal already says ask your human",
            &["list"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"gated","message":"ask your human first"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a usage error",
            &["list"],
            &[],
            vec![Reply::Answer(400, r#"{"error":{"code":"usage","hint":"bad"}}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error object with no code",
            &["list"],
            &[],
            vec![Reply::Answer(500, r#"{"error":{}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error that names the owner",
            &["list"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"Owner token needed"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an error that is a number",
            &["list"],
            &[],
            vec![ok(r#"{"error":7}"#)],
            owner_and_port,
            1,
            1,
        ),
        case("ok false", &["list"], &[], vec![ok(r#"{"ok":false,"hint":"no"}"#)], owner_and_port, 1, 1),
        case("not JSON", &["list"], &[], vec![ok("<html>oops</html>")], owner_and_port, 1, 1),
        case("an empty 200", &["list"], &[], vec![ok("")], owner_and_port, 1, 1),
        case(
            "the bulletin",
            &["list"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal with a permission card",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "tokens is a string",
            &["list"],
            &[],
            vec![ok(r#"{"tokens":"ab"}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a list answer",
            &["list"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a cap that is a number, after the first row",
            &["list"],
            &[],
            vec![ok(r#"{"tokens":[{"id":"a"},{"id":"b","caps":[1]}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a hint that is a number",
            &["list"],
            &[],
            vec![ok(r#"{"error":{"code":"x","hint":5}}"#)],
            owner_and_port,
            1,
            1,
        )),
        // The gates.
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
        case("no token", &["list"], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &["list"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn skin_token_create_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "create");
    let v = vec![
        case(
            "create",
            &["create", "--name", "site", "--agent", "sales", "--caps", " thread:read, ,files:read "],
            &[],
            vec![ok(CREATED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "two agents, aliases, a name that needs escaping",
            &["create", "--agents", " sales ", "--name", "s\u{e9} \"x\"", "--agent", "a\rb", "--cap", ""],
            &[],
            vec![ok(r#"{"id":"t","username":"u","room_handles":["a","b"],"token":"k2skn_x"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty agent: no rooms",
            &["create", "--name", "n", "--agent", ""],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case("no name", &["create", "--agent", "a"], &[], vec![], owner_and_port, 2, 0),
        case("an empty name", &["create", "--name", "", "--agent", "a"], &[], vec![], owner_and_port, 2, 0),
        case("no agent", &["create", "--name", "n"], &[], vec![], owner_and_port, 2, 0),
        case("a positional word", &["create", "site"], &[], vec![], owner_and_port, 2, 0),
        case("--user", &["create", "--user", "u"], &[], vec![], owner_and_port, 2, 0),
        case("an unknown flag", &["create", "--x"], &[], vec![], owner_and_port, 1, 0),
        case("a lone dash", &["create", "-"], &[], vec![], owner_and_port, 1, 0),
        case("--name as the last word", &["create", "--name"], &[], vec![], owner_and_port, 1, 0),
        case(
            "--agent as the last word",
            &["create", "--name", "n", "--agent"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "the daemon's error",
            &["create", "--name", "n", "--agent", "a"],
            &[],
            vec![Reply::Answer(400, r#"{"error":{"code":"bad_room","hint":"no such agent"}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 5xx with no body: the empty line",
            &["create", "--name", "n", "--agent", "a"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "caps that is a number",
            &["create", "--name", "n", "--agent", "a"],
            &[],
            vec![ok(r#"{"id":"t","caps":3}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a room that is not a string",
            &["create", "--name", "n", "--agent", "a"],
            &[],
            vec![ok(r#"{"id":"t","roomHandles":[null]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a reset (56): curl's code",
            &["create", "--name", "n", "--agent", "a"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        waited(case(
            "an empty reply (52): the line, exit 1",
            &["create", "--name", "n", "--agent", "a"],
            &[],
            vec![Reply::Close],
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
fn skin_token_rooms_revoke_and_usage_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "rooms");
    let v = vec![
        case(
            "rooms",
            &["rooms", "t1", "--agent", "sales", "--agents", "ops", "extra"],
            &[],
            vec![ok(r#"{"id":"t1","roomHandles":["sales","ops"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "rooms --clear drops the agents",
            &["rooms", "--agent", "a", "--clear", "t1"],
            &[],
            vec![ok(r#"{"id":"t1","roomHandles":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case("rooms with no id", &["rooms", "--clear"], &[], vec![], owner_and_port, 1, 0),
        case("rooms, an unknown flag", &["rooms", "t", "--x"], &[], vec![], owner_and_port, 1, 0),
        case("rooms, --agent last", &["rooms", "t", "--agent"], &[], vec![], owner_and_port, 1, 0),
        case(
            "rooms, the error",
            &["rooms", "t1", "--clear"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"not found"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "revoke",
            &["revoke", "t1", "extra"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "revoke, nothing to revoke",
            &["revoke", "t1"],
            &[],
            vec![ok(r#"{"success":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "revoke, a string answer",
            &["revoke", "t1"],
            &[],
            vec![ok(r#""done""#)],
            owner_and_port,
            1,
            1,
        )),
        case("revoke with no id", &["revoke"], &[], vec![], owner_and_port, 1, 0),
        case("no verb", &[], &[], vec![], owner_and_port, 1, 0),
        case("an empty verb", &[""], &[], vec![], owner_and_port, 1, 0),
        case("an unknown verb", &["nope"], &[], vec![], owner_and_port, 1, 0),
        case("no verb and no port: the gate first", &[], &[], vec![], owner_only, 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn skin_token_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let v = vec![
        with(
            h(&[]),
            case("a host session: the passport over TCP", &["list"], &[], vec![ok(LISTED)], port_only, 0, 1),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a live cell socket: still TCP",
                &["revoke", "t1"],
                &[],
                vec![ok(r#"{"success":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["create", "--name", "n", "--agent", "a"],
                &[],
                vec![heal(&rig), ok(CREATED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &["list"],
                &[],
                vec![ok(LISTED)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["list"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn skin_token_with_no_answer() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    let v = vec![
        case(
            "nothing listens (7): tried again, then curl's code",
            &["list"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            7,
            0,
        ),
        case(
            "nothing listens (7), a POST",
            &["revoke", "t"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            7,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    let rig = Rig::new(VERB, "noanswer");
    rig.check(&waited(case(
        "a 5xx with no body to a GET: the line, exit 1",
        &["list"],
        &[],
        vec![Reply::Answer(502, String::new())],
        owner_and_port,
        1,
        1,
    )));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn skin_token_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case("--help", &["--help"], &[], vec![], owner_and_port, 0, 0)),
        shell(case("create -h", &["create", "-h"], &[], vec![], owner_and_port, 0, 0)),
        shell(case(
            "a port that is no port",
            &["list"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            1,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
