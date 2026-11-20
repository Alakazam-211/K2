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
//! - `create --hire` and `hire-ceiling` (`app-access`): the owner's
//!   confirm over the vault socket on the vault rig ([`common::vault`]),
//!   the limits' body, a limit Python can't read, a refused confirm, no
//!   vault socket; `hire-ceiling`'s usage lines, readers and tracebacks.
//! - The owner from disk, a passport, a heal, the id tool's checks, the
//!   gates; what the binary hands to the shell (help, a port that is no
//!   port).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use common::vault::{VCase, VRig};

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
        shell(case(
            "BASH BUG, help acts: hire-ceiling k1 --app-login -h sends an App named -h",
            &["hire-ceiling", "k1", "--app-login", "-h"],
            &[],
            vec![ok(r#"{"id":"k1","hireLimits":{"apps":[{"app":"-h"}]}}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "create --hire -h: an unknown flag, nothing sent",
            &["create", "--hire", "-h"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

const WIDENED: &str = r#"{"id":"k1","change":"raise","hireLimits":{"apps":[{"app":"acme/site","rooms":["self","lobby"],"roles":["member"]},{"app":"globex","rooms":[]}]}}"#;

#[test]
fn api_key_hire_ceiling_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "ceiling");
    let v = vec![
        // covers: api-key hire-ceiling
        case(
            "--none",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![ok(r#"{"id":"k1","hireLimits":{"apps":[]}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key hire-ceiling
        case(
            "two App logins, lowered: no confirm",
            &[
                "hire-ceiling",
                "k1",
                "--app-login",
                " acme/site :self, ,lobby:member",
                "--app-login",
                "globex:",
            ],
            &[],
            vec![ok(WIDENED.replace("raise", "lower").as_str())],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key hire-ceiling
        case(
            "app-access, the alias",
            &["app-access", "k1", "--app-login", "globex"],
            &[],
            vec![ok(r#"{"id":"k1","change":"raise"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key hire-ceiling
        case(
            "the daemon's error, a string",
            &["hire-ceiling", "k9", "--none"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no such key"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key hire-ceiling
        case(
            "the daemon's error, a dict",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"not_hire_key","hint":"mint one with --hire"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key hire-ceiling
        case(
            "a refusal with a permission card",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key hire-ceiling
        case(
            "not JSON",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key hire-ceiling
        case(
            "an empty answer",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        // covers: api-key hire-ceiling
        traceback(case(
            "a list: Python's AttributeError",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        // covers: api-key hire-ceiling
        traceback(case(
            "a room that is a number: Python's TypeError",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![ok(r#"{"id":"k1","hireLimits":{"apps":[{"app":"a","rooms":[5]}]}}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: api-key hire-ceiling
        case(
            "the bulletin",
            &["hire-ceiling", "k1", "--none"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        // covers: api-key hire-ceiling
        case(
            "no id",
            &["hire-ceiling"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key hire-ceiling
        case(
            "an id that starts with -",
            &["app-access", "--none"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key hire-ceiling
        case(
            "neither --none nor --app-login",
            &["hire-ceiling", "k1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key hire-ceiling
        case(
            "both --none and --app-login",
            &["hire-ceiling", "k1", "--app-login", "a", "--none"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key hire-ceiling
        case(
            "an unknown flag",
            &["hire-ceiling", "k1", "--all"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key hire-ceiling
        case(
            "--app-login last",
            &["hire-ceiling", "k1", "--app-login"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: api-key hire-ceiling
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["hire-ceiling", "k1", "--none"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        // covers: api-key create
        case(
            "create --hire --status",
            &["create", "--hire", "--status"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: api-key create
        case(
            "create --hire after an unknown flag",
            &["create", "--hire", "--nope"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: api-key create
        case(
            "create --hire, a value flag last",
            &["create", "--hire", "--max-live"],
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
fn api_key_hire_ceiling_with_no_daemon() {
    let rig = Rig::new(VERB, "ceilingdead");
    let port = dead_port();
    let v = vec![
        // covers: api-key hire-ceiling
        case(
            "nothing listens: curl's code",
            &["hire-ceiling", "k1", "--none"],
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
}

/// A case on the vault rig: the daemon's and the vault socket's answers,
/// stdin, the exit.
fn confirming(
    name: &'static str,
    args: &[&'static str],
    daemon: Vec<Reply>,
    vault: Vec<Reply>,
    stdin: &[u8],
    code: i32,
) -> VCase {
    let mut c = VCase::new(name, args);
    c.noun = VERB;
    c.daemon_requests = daemon.len();
    c.daemon = daemon;
    c.vault_requests = vault.len();
    c.vault = Some(vault);
    c.stdin = stdin.to_vec();
    c.code = code;
    c
}

fn secrets(mut c: VCase, s: &[&'static str]) -> VCase {
    c.secrets = s.to_vec();
    c
}

fn methods(m: &str) -> Reply {
    ok(&format!(
        r#"{{"methods":["{m}"],"confirmText":"an API key that can hire"}}"#
    ))
}

fn token(t: &str) -> Reply {
    ok(&format!(r#"{{"token":"{t}"}}"#))
}

/// The confirm the daemon asks for before it widens a key's App access.
fn confirm_required() -> Reply {
    Reply::Answer(
        403,
        r#"{"error":{"code":"vault_confirm_required","hint":"widening needs the owner"}}"#.into(),
    )
}

#[test]
fn api_key_hire_asks_for_the_owner_s_confirm() {
    let rig = VRig::new();
    let v = vec![
        secrets(
            // covers: api-key create
            confirming(
                "create --hire: the vault password, every limit and two App logins",
                &[
                    "create",
                    "--hire",
                    "--label",
                    "hirer",
                    "--workspaces",
                    "*",
                    "--max-per-day",
                    "5",
                    "--max-live",
                    " 0_9 ",
                    "--max-per-team",
                    "+3",
                    "--hire-db",
                    "read",
                    "--link",
                    "--link-targets",
                    "a, ,b",
                    "--retire",
                    "--reach",
                    "message",
                    "--folder",
                    "sub",
                    "--app-login",
                    "acme/site:self,lobby:member",
                    "--app-login",
                    "globex",
                ],
                vec![ok(CREATED)],
                vec![methods("vault-password"), token("tk-hire-1")],
                b"pw-hire-1\n",
                0,
            ),
            &["pw-hire-1", "tk-hire-1"],
        ),
        secrets(
            // covers: api-key create
            confirming(
                "create --hire --key-stdin: the key, then the password",
                &["create", "--hire", "--key-stdin", "--workspaces", "*"],
                vec![ok(CREATED)],
                vec![methods("vault-password"), token("tk-hire-2")],
                b"sk-llm-hire-2\npw-hire-2\n",
                0,
            ),
            &["sk-llm-hire-2", "pw-hire-2", "tk-hire-2"],
        ),
        // covers: api-key create
        confirming(
            "create --hire: this Mac's check, and no --workspaces",
            &["create", "--hire"],
            vec![ok(r#"{"id":"ak2","key":"k2sk_h","capabilities":{"hire":true}}"#)],
            vec![methods("local-auth"), token("tk-hire-3")],
            b"",
            0,
        ),
        // covers: api-key create
        confirming(
            "create --hire: the confirm is refused",
            &["create", "--hire", "--workspaces", "*"],
            vec![],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"vault_denied","hint":"not now"}}"#.into(),
            )],
            b"",
            3,
        ),
        // covers: api-key create
        confirming(
            "create --hire: no confirm method",
            &["create", "--hire", "--workspaces", "*"],
            vec![],
            vec![ok(r#"{"methods":[]}"#)],
            b"",
            3,
        ),
        {
            // covers: api-key create
            let mut c = confirming(
                "create --hire: no vault socket",
                &["create", "--hire", "--workspaces", "*"],
                vec![],
                vec![],
                b"",
                3,
            );
            c.vault = None;
            c
        },
        // covers: api-key create
        confirming(
            "create --hire: a limit Python can't read, the empty body refused",
            &["create", "--hire", "--max-live", "lots", "--workspaces", "*"],
            vec![Reply::Answer(400, r#"{"error":"bad body"}"#.into())],
            vec![methods("vault-password"), token("tk-hire-4")],
            b"pw\n",
            1,
        ),
        // covers: api-key create
        confirming(
            "create --hire: a limit Python can't read, the empty body answered",
            &["create", "--hire", "--max-per-team", "1e3", "--workspaces", "*"],
            vec![ok(CREATED)],
            vec![methods("vault-password"), token("tk-hire-5")],
            b"pw\n",
            2,
        ),
        // covers: api-key create
        confirming(
            "create --hire: the daemon's error",
            &["create", "--hire", "--workspaces", "*"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"hire_off","hint":"turn hiring on first"}}"#.into(),
            )],
            vec![methods("vault-password"), token("tk-hire-6")],
            b"pw\n",
            1,
        ),
        secrets(
            // covers: api-key hire-ceiling
            confirming(
                "hire-ceiling: widening asks for the confirm, then sends again",
                &[
                    "hire-ceiling",
                    "k1",
                    "--app-login",
                    "acme/site:self,lobby:member",
                    "--app-login",
                    "globex",
                ],
                vec![confirm_required(), ok(WIDENED)],
                vec![methods("vault-password"), token("tk-widen-1")],
                b"pw-widen-1\n",
                0,
            ),
            &["pw-widen-1", "tk-widen-1"],
        ),
        // covers: api-key hire-ceiling
        confirming(
            "app-access: the confirm is refused",
            &["app-access", "k1", "--app-login", "globex"],
            vec![confirm_required()],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"vault_denied","hint":"not now"}}"#.into(),
            )],
            b"",
            3,
        ),
        // covers: api-key hire-ceiling
        confirming(
            "hire-ceiling: confirmed, then the daemon's error",
            &["hire-ceiling", "k1", "--app-login", "globex"],
            vec![
                confirm_required(),
                Reply::Answer(
                    409,
                    r#"{"error":{"code":"stale","hint":"list the keys again"}}"#.into(),
                ),
            ],
            vec![methods("local-auth"), token("tk-widen-2")],
            b"",
            1,
        ),
        // covers: api-key hire-ceiling
        confirming(
            "hire-ceiling: confirmed, then no answer",
            &["hire-ceiling", "k1", "--app-login", "globex"],
            vec![confirm_required(), Reply::Close],
            vec![methods("local-auth"), token("tk-widen-3")],
            b"",
            52,
        ),
        // covers: api-key hire-ceiling
        confirming(
            "hire-ceiling: asked again after the token",
            &["hire-ceiling", "k1", "--none"],
            vec![confirm_required(), confirm_required()],
            vec![methods("local-auth"), token("tk-widen-4")],
            b"",
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}
