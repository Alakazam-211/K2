//! `k5-cli preset` against the shell CLI's `cli/k2 preset`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - `list`, `show`, `add`, `set` and `remove`, each with `--json`; the
//!   help; the selector by id, label and command token, ambiguous and not
//!   found; built-ins that can't be removed.
//! - The daemon's errors (a string, an object, a 403), an answer that is
//!   not the list, no answer.
//! - The usage lines (compact JSON on stderr, exit 2).
//! - The owner token from disk, a passport (the binary sends it where the
//!   shell sends the owner's: owner-token-with-passport).
//! - One large list (above 256 KiB).
//! - Help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "preset";

const PRESETS: &str = r#"[{"id":"claude","label":"Claude Code","command":"claude --dangerously-skip-permissions","enabled":true,"is_built_in":true,"danger_flags":"[\"--dangerously-skip-permissions\"]","readiness":"bracketed-paste"},{"id":"aider","label":"Aider","command":"aider --yes-always --model sonnet --no-auto-commits --map-tokens 2048","enabled":false,"is_built_in":false,"env":"{\"B\":\"2\",\"A\":\"1 é\"}","icon":"🛠","inject_flow":"[{\"key\":\"paste\"}]"},{"id":"cx","label":"Codex","command":"codex","enabled":true,"danger_flags":"oops"},{"id":"cx2","label":"codex","command":"codex --full-auto","enabled":true}]"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn preset_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let created = r#"{"id":"mine","command":"mine --go","label":"mine"}"#;
    let v = vec![
        case("the help", &[], &[], vec![], owner_and_port, 0, 0),
        // covers: preset list
        case("list", &["list"], &[], vec![ok(PRESETS)], owner_and_port, 0, 1),
        // covers: preset list --json
        case(
            "list --json",
            &["list", "--json"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            0,
            1,
        ),
        case("list: none", &["list"], &[], vec![ok("[]")], owner_and_port, 0, 1),
        case(
            "list: not a list",
            &["list"],
            &[],
            vec![ok(r#"{"presets":[]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "list: the daemon's error",
            &["list"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"db locked"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "list: refused",
            &["list"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"owner_only","hint":"no"}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "list: an error page",
            &["list"],
            &[],
            vec![Reply::Answer(502, "<html>".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "list: not JSON",
            &["list"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "list: a stray word",
            &["list", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: preset show
        case(
            "show by id",
            &["show", "aider"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: preset show --json
        case(
            "show by label --json",
            &["show", " claude code ", "--json"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show: a malformed column",
            &["show", "cx"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show: an ambiguous label",
            &["show", "Codex"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "show by command token",
            &["show", "aider"],
            &[],
            vec![ok(r#"[{"id":"a1","label":"A","command":"aider -x"}]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show: not found",
            &["show", "nope"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            4,
            1,
        ),
        case("show: no selector", &["show"], &[], vec![], owner_and_port, 2, 0),
        case(
            "show: a flag",
            &["show", "a", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "show: two selectors",
            &["show", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: preset add
        case(
            "add",
            &[
                "add",
                "--id",
                "mine",
                "--command",
                "mine --go",
                "--danger-flag",
                "--go",
                "--env",
                "K=V=W",
                "--env",
                "K=2",
                "--readiness",
                "settle:500",
                "--icon",
                "x",
            ],
            &[],
            vec![ok(created)],
            owner_and_port,
            0,
            1,
        ),
        // covers: preset add --json
        case(
            "add --json",
            &["add", "--json", "--id", "mine", "--command", "mine", "--label", "Mine é"],
            &[],
            vec![ok(created)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add: a bad --env",
            &["add", "--id", "m", "--command", "m", "--env", "NOEQ"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: an empty --env name",
            &["add", "--id", "m", "--command", "m", "--env", "=v"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: no command",
            &["add", "--id", "m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: --id with no value",
            &["add", "--id"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: an unknown word",
            &["add", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: the daemon refuses",
            &["add", "--id", "m", "--command", "m"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"forbidden"}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: preset set
        case(
            "set",
            &[
                "set",
                "aider",
                "--disable",
                "--clear-env",
                "--command",
                "aider",
                "--inject-flow",
                "[ {\"key\": \"paste\"} ]",
            ],
            &[],
            vec![ok(PRESETS), ok(r#"{"id":"aider"}"#)],
            owner_and_port,
            0,
            2,
        ),
        // covers: preset set --json
        case(
            "set --json",
            &["set", "claude", "--enable", "--json", "--clear-danger-flags", "--clear-readiness", "--clear-inject-flow"],
            &[],
            vec![ok(PRESETS), ok(r#"{"id":"claude","enabled":1}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "set: nothing to set",
            &["set", "claude"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "set: a bad inject flow",
            &["set", "claude", "--inject-flow", "nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set: a flag and its clear",
            &["set", "claude", "--readiness", "x", "--clear-readiness"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set: not found",
            &["set", "zzz", "--enable"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            4,
            1,
        ),
        // covers: preset remove
        case(
            "remove",
            &["remove", "aider"],
            &[],
            vec![ok(PRESETS), ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            2,
        ),
        // covers: preset remove --json
        case(
            "remove --json",
            &["remove", "--json", "codex"],
            &[],
            vec![ok(r#"[{"id":"c","label":"C","command":"codex"}]"#), ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "remove: a built-in",
            &["remove", "claude"],
            &[],
            vec![ok(PRESETS)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an unknown verb",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no answer",
            &["list"],
            &[("K2_PORT", dead_port().as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "an empty reply",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        with(
            with_passport(&[]),
            case(
                "a passport outside a session",
                &["list"],
                &[],
                vec![ok(PRESETS)],
                port_only,
                0,
                1,
            ),
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "a session: the shell sends the owner token from disk",
                    &["list"],
                    &[],
                    vec![ok(PRESETS)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn preset_list_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"id":"preset-{i:05}","label":"Preset {i}","command":"agent-{i} --flag-one --flag-two --flag-three --long","enabled":true,"is_built_in":false,"readiness":"settle:{i}"}}"#
            )
        })
        .collect();
    let body = format!("[{}]", rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&case(
        "a large list",
        &["list"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn preset_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    for args in [&["list", "x"][..], &["nope"]] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert!(out.stderr.contains(&format!("{cli} preset")), "{args:?}: {}", out.stderr);
        assert!(!out.stderr.contains("k5 "), "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// Help never acts: `-h` or `--help` on a line that would change
/// something prints help and sends nothing.
#[test]
fn preset_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell("add -h", &["add", "--id", "x", "--command", "x", "-h"]),
        shell("set --help", &["set", "claude", "--disable", "--help"]),
        shell("remove -h", &["remove", "aider", "-h"]),
        shell("add, -h as a value", &["add", "--id", "-h"]),
    ] {
        rig.check(&c);
    }
}
