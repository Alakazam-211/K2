//! `k5-cli api-key` against the shell CLI's `cli/k2 api-key`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - `list` (and one list above 256 KiB), `revoke`, `disable`, `enable`:
//!   answers, the daemon's errors, Python's exception messages (the
//!   readers catch them: exact), no answer (the line, then SIGPIPE).
//! - `create`: the body from the flags, the old argv key's warning, the
//!   no-grant note, `--status`, the usage errors, a value flag last.
//!   (The key on stdin is `tests/cli/api_key_stdin_off_argv.sh`'s: the rig
//!   runs with stdin from /dev/null, so here `--key-stdin` reads nothing.)
//! - The owner from disk, a passport, a heal, the id tool's checks, the
//!   gates; what the binary hands to the shell (`create --hire`,
//!   `hire-ceiling`, help, a port that is no port).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "api-key";
const KEYS: &str = r#"{"keys":[{"id":"k1","label":"wiki","anthropicKeySet":true,"scope":"*","capabilities":{"hostSessions":true}},{"id":"k2","provider":"openai","anthropicKeySet":true,"baseUrl":"https://llm.example","disabledAt":1,"capabilities":{"hire":true,"db":true},"hireLimits":{"apps":[{"app":"acme/site","rooms":["self"],"roles":["member"]},{"app":"x"}]},"legacyNewWorkspace":true},{"id":"k3","revokedAt":"t","capabilities":{"hire":true},"hireLimits":{}}]}"#;
const CREATED: &str = r#"{"id":"ak1","key":"k2sk_secret","capabilities":{"hostSessions":true,"canonicalMessage":false},"hireLimits":{"maxMembersPerDay":20,"db":"off","folder":"f","apps":[{"app":"a","rooms":[],"roles":["r1","r2"]}]}}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// No answer on the left of the pipe: the line, then SIGPIPE (141);
/// macOS's bash 3.2 runs the reader on an empty input instead.
fn in_pipe(c: Case) -> Case {
    Case {
        compare: Compare::Bash32PipeReader {
            stdout: "",
            stderr: "Error: Expecting value: line 1 column 1 (char 0)\n",
        },
        ..c
    }
}

#[test]
fn api_key_list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let big = {
        let rows: Vec<String> = (0..2500)
            .map(|i| format!(r#"{{"id":"key-{i:05}","label":"label {i} é","anthropicKeySet":{},"scope":"ws-{i}","capabilities":{{"hostSessions":true,"sandboxes":true}},"baseUrl":"https://llm{i}.example"}}"#, i % 2 == 0))
            .collect();
        format!(r#"{{"keys":[{}]}}"#, rows.join(","))
    };
    assert!(
        big.len() > 256 * 1024,
        "the large list is {} bytes",
        big.len()
    );
    let v = vec![
        case("list", &["list"], &[], vec![ok(KEYS)], owner_and_port, 0, 1),
        // covers: api-key list
        case(
            "a large list, above 256 KiB",
            &["list"],
            &[],
            vec![ok(&big)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key list
        case(
            "no keys",
            &["list"],
            &[],
            vec![ok(r#"{"keys":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key list
        case(
            "the daemon's error",
            &["list"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "a refusal with a permission card",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "the bulletin",
            &["list"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key list
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "a trailing comma",
            &["list"],
            &[],
            vec![ok(r#"{"keys":[],}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "an empty 200",
            &["list"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "a list answer",
            &["list"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "keys is a string",
            &["list"],
            &[],
            vec![ok(r#"{"keys":"ab"}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "keys is a number",
            &["list"],
            &[],
            vec![ok(r#"{"keys":5}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "capabilities is a list, after a row",
            &["list"],
            &[],
            vec![ok(r#"{"keys":[{"id":"a"},{"id":"b","capabilities":[1]}]}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key list
        case(
            "a room that is a number",
            &["list"],
            &[],
            vec![ok(
                r#"{"keys":[{"id":"a","capabilities":{"hire":1},"hireLimits":{"apps":[{"rooms":[7]}]}}]}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        in_pipe(case(
            "an empty reply (52): the line, SIGPIPE",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
        case("no token", &["list"], &[], vec![], port_only, 1, 0),
        case("no verb: the usage", &[], &[], vec![], owner_and_port, 1, 0),
        case(
            "an unknown verb: the usage",
            &["mint"],
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
fn api_key_revoke_disable_enable_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "ids");
    let v = vec![
        // covers: api-key revoke
        case(
            "revoke",
            &["revoke", "k1", "x"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key revoke
        case(
            "revoke, none",
            &["revoke", "k1"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key
        case(
            "disable",
            &["disable", "k \"1\""],
            &[],
            vec![ok(r#"{"success":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key disable
        case(
            "disable, none",
            &["disable", "k1"],
            &[],
            vec![ok(r#"{"success":0}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key enable
        case(
            "enable",
            &["enable", "k1"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key enable
        case(
            "enable, none",
            &["enable", "k1"],
            &[],
            vec![ok(r#"{"success":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key revoke
        case(
            "the daemon's error",
            &["revoke", "k1"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no such key"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key enable
        case(
            "a string that says error",
            &["enable", "k1"],
            &[],
            vec![ok(r#""an error""#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key disable
        case(
            "a number",
            &["disable", "k1"],
            &[],
            vec![ok("5")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key revoke
        case(
            "a list",
            &["revoke", "k1"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key revoke
        case(
            "revoke with no id",
            &["revoke"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: api-key disable
        case(
            "disable with an empty id",
            &["disable", ""],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: api-key enable
        case(
            "enable with no id",
            &["enable"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        in_pipe(case(
            "an empty reply (52): the line, SIGPIPE",
            &["revoke", "k1"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn api_key_create_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "create");
    let v = vec![
        case(
            "create, every flag but the key",
            &[
                "create",
                "--label",
                "wiki chat",
                "--provider",
                "openai",
                "--base-url",
                "https://llm.example ",
                "--workspaces",
                " a, ,b ",
                "--allow",
                "host-sessions,db",
                "--deny",
                ",",
            ],
            &[],
            vec![ok(CREATED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the key on argv: the warning, and no grant: the note",
            &[
                "create",
                "--llm-key",
                " sk-test ",
                "--anthropic-key",
                "sk-last",
            ],
            &[],
            vec![ok(r#"{"id":"ak2","key":"k2sk_y"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key create
        case(
            "every workspace",
            &["create", "--workspaces", "*"],
            &[],
            vec![ok(r#"{"id":"ak3","key":"k"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key create
        case(
            "the daemon's error object",
            &["create", "--workspaces", "*"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_provider","hint":"try openai"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key create
        case(
            "capabilities is a list, after the first line",
            &["create", "--workspaces", "*"],
            &[],
            vec![ok(r#"{"id":"a","capabilities":[1]}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key create
        case(
            "not JSON",
            &["create"],
            &[],
            vec![ok("{\"id\" 1}")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key create
        case(
            "an empty answer",
            &["create"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key create
        case(
            "--key-stdin and nothing on stdin",
            &["create", "--key-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key create
        case(
            "--key-stdin and --llm-key",
            &["create", "--key-stdin", "--llm-key", "k"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key create
        case(
            "a hire limit with no --hire",
            &["create", "--max-live", "3"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key create
        case(
            "an empty hire limit is none",
            &["create", "--reach", "", "--workspaces", "*"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key create
        case(
            "an unknown flag",
            &["create", "--label", "x", "wiki"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: api-key create
        case(
            "a value flag last",
            &["create", "--provider"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: api-key create
        case(
            "a status key",
            &["create", "--status", "--label", "fleet"],
            &[],
            vec![ok(r#"{"id":"sk1","key":"k2sk_status"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key create
        case(
            "a status key, the daemon's error",
            &["create", "--status"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"owner only"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key create
        case(
            "a status key, not JSON",
            &["create", "--status"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key create
        traceback(case(
            "a status key, a number",
            &["create", "--status"],
            &[],
            vec![ok("5")],
            owner_and_port,
            1,
            1,
        )),
        // covers: api-key create
        case(
            "a status key and another flag",
            &["create", "--status", "--allow", "db"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key create
        case(
            "a reset: the reader's line",
            &["create", "--workspaces", "*"],
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
fn api_key_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let v = vec![
        with(
            h(&[]),
            // covers: api-key list
            case(
                "a host session: the passport over TCP",
                &["list"],
                &[],
                vec![ok(KEYS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            // covers: api-key revoke
            case(
                "a live cell socket: still TCP",
                &["revoke", "k1"],
                &[],
                vec![ok(r#"{"success":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            // covers: api-key disable
            case(
                "a refused passport heals once",
                &["disable", "k1"],
                &[],
                vec![heal(&rig), ok(r#"{"success":true}"#)],
                port_only,
                0,
                2,
            ),
        ),
        // covers: api-key list
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        // covers: api-key list
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
fn api_key_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    let v = vec![
        // covers: api-key list
        case(
            "list, nothing listens: the reader's line",
            &["list"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        // covers: api-key create
        case(
            "create, nothing listens: sent once",
            &["create"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn api_key_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "--help",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "list -h",
            &["list", "-h"],
            &[],
            vec![ok(KEYS)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "create --hire --status",
            &["create", "--hire", "--status"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "create --hire after an unknown flag",
            &["create", "--hire", "--nope"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "hire-ceiling",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![ok(r#"{"id":"k1","hireLimits":{"apps":[]}}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "app-access with no id",
            &["app-access"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
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

/// Help never acts (K2's rule): -h or --help anywhere on a verb that
/// changes something prints help and sends nothing. bash's `api-key)` arm
/// has no help check (cli/src/router.sh, `api-key)`): `revoke`, `disable`
/// and `enable` take the first word as the id and post at once
/// (cli/src/talk-and-workspace.sh, cmd_api_key `revoke)`, `disable)`,
/// `enable)`), and `create --label -h` mints a key labelled `-h`. The rows
/// marked BASH BUG make the request: the binary hands every -h line to
/// the shell and does the same until K2 fixes the bash; then these rows
/// send nothing.
#[test]
fn api_key_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "BASH BUG, help acts: revoke -h revokes a key named -h",
            &["revoke", "-h"],
            &[],
            vec![ok(r#"{"success":false}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "BASH BUG, help acts: revoke k1 --help revokes k1",
            &["revoke", "k1", "--help"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "BASH BUG, help acts: disable k1 -h disables k1",
            &["disable", "k1", "-h"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "BASH BUG, help acts: enable --help enables a key named --help",
            &["enable", "--help"],
            &[],
            vec![ok(r#"{"success":false}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "BASH BUG, help acts: create --label -h mints a key",
            &["create", "--label", "-h", "--workspaces", "*"],
            &[],
            vec![ok(r#"{"id":"k9","key":"k2sk_x","scope":"*"}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "create -h: an unknown flag, nothing sent",
            &["create", "-h"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "hire-ceiling k1 --help: an unknown flag, nothing sent",
            &["hire-ceiling", "k1", "--help"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "hire-ceiling -h: the usage, nothing sent",
            &["hire-ceiling", "-h"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
