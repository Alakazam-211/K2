//! `k5-cli push` against the shell CLI's `cli/k2 push`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `devices` (`ls`), `e2e` (read and set), `device e2e`, `device remove`
//!   (`rm`) and `log` (`--device`), each with `--json`; the help.
//! - The answers: rows with every field, none, the daemon's errors and
//!   their exits, `"ok": false`, not JSON, a card, odd values, no answer.
//! - The usage lines; the owner token from disk, a passport (the binary
//!   sends it where the shell sends the owner's: owner-token-with-passport).
//! - One large list (above 256 KiB).
//! - Help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "push";

const DEVICES: &str = r#"{"ok":true,"mode":"sealed-when-supported","relaySealed":true,"devices":[{"deviceId":"d1","platform":"ios","username":"ann","previews":true,"status":"ok","tokenFingerprint":"ab12","e2e":{"keyId":"k1","active":true,"enforce":true,"override":"sealed"}},{"deviceId":"d2","platform":"android","previews":false,"status":"","e2e":{}}]}"#;

const LOG: &str = r#"{"ok":true,"attempts":[{"atMs":1763625600123,"device":"d1","kind":"msg","lane":"apns","bytes":120,"key":"k1","status":200,"path":"sealed","relay":"s1","vendor":"apple","ms":42},{"atMs":1763625500000,"device":"d2","kind":"msg","lane":"fcm","bytes":80,"status":null,"error":"timeout","path":"plain","relay":"s0","ms":30000}],"events":[{"atMs":1763625600123,"action":"add","reason":"login","device":"d1","platform":"ios","login":"ann","session":null,"token":"ab12","app":"crm","detail":"first"}]}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn push_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: push
        case("the help", &[], &[], vec![], owner_and_port, 0, 0),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        // covers: push devices
        case(
            "devices",
            &["devices"],
            &[],
            vec![ok(DEVICES)],
            owner_and_port,
            0,
            1,
        ),
        // covers: push devices --json
        case(
            "devices --json",
            &["ls", "--json"],
            &[],
            vec![ok(DEVICES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no phones, the relay not ready",
            &["devices"],
            &[],
            vec![ok(r#"{"ok":true,"mode":"off","devices":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "devices: --json and a refusal",
            &["devices", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "devices: a card",
            &["devices"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "devices: not found",
            &["devices"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no route"}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "devices: ok false",
            &["devices"],
            &[],
            vec![ok(r#"{"ok":false}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "devices: not JSON",
            &["devices"],
            &[],
            vec![Reply::Answer(502, "  <html>bad gateway</html>\n".into())],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "devices: a row that can't be shown",
                &["devices"],
                &[],
                vec![ok(
                    r#"{"mode":"off","devices":[{"deviceId":5,"status":"ok"}]}"#,
                )],
                owner_and_port,
                1,
                1,
            )
        },
        case(
            "devices: an extra word",
            &["devices", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: push e2e
        case(
            "e2e",
            &["e2e"],
            &[],
            vec![ok(r#"{"ok":true,"mode":"off","relaySealed":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: push e2e --json
        case(
            "e2e sealed-only --json",
            &["e2e", "sealed-only", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"mode":"sealed-only"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "e2e set: a bad request",
            &["e2e", "off"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_mode","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "e2e: a bad mode",
            &["e2e", "on"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "e2e: two words",
            &["e2e", "off", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: push device e2e
        case(
            "device e2e",
            &["device", "e2e", "d1", "default"],
            &[],
            vec![ok(r#"{"ok":true,"deviceId":"d1","override":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: push device e2e --json
        case(
            "device e2e --json",
            &["device", "--json", "e2e", "d\u{e9}\"1", "sealed"],
            &[],
            vec![ok(r#"{"ok":true,"deviceId":"d1","override":"sealed"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "device e2e: a bad mode",
            &["device", "e2e", "d1", "on"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "device: no verb",
            &["device"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: push device remove
        case(
            "device remove",
            &["device", "remove", "d1"],
            &[],
            vec![ok(r#"{"ok":true,"deviceId":"d1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: push device remove --json
        case(
            "device rm --json",
            &["device", "rm", "d1", "--json"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no such device"}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "device remove: no id",
            &["device", "remove", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: push log
        case(
            "log",
            &["log"],
            &[("TZ", "UTC")],
            vec![ok(LOG)],
            owner_and_port,
            0,
            1,
        ),
        // covers: push log --json
        case(
            "log --device --json",
            &["log", "--device", "d 1", "--json"],
            &[],
            vec![ok(LOG)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log: none for a device",
            &["log", "--device=d1"],
            &[],
            vec![ok(r#"{"ok":true,"device":"d1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log: none",
            &["log"],
            &[],
            vec![ok(r#"{"ok":true,"attempts":[],"events":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log: --device with no value",
            &["log", "--device"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "log: a stray word",
            &["log", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
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
            &["devices"],
            &[("K2_PORT", dead_port().as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "an empty reply",
            &["e2e"],
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
                &["devices"],
                &[],
                vec![ok(DEVICES)],
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
                    &["devices"],
                    &[],
                    vec![ok(DEVICES)],
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

/// The large list: above 256 KiB, thousands of phones. Bash hands the
/// body to its Python in the environment, which a body this size may
/// pass; where it doesn't, bash fails with `Argument list too long`.
#[test]
fn push_devices_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"deviceId":"device-{i:05}","platform":"ios","username":"user{i}","previews":true,"status":"ok","tokenFingerprint":"fp{i:05}"}}"#
            )
        })
        .collect();
    let body = format!(
        r#"{{"ok":true,"mode":"off","relaySealed":true,"devices":[{}]}}"#,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024, "{}", body.len());
    let mut want = String::from("encrypted pushes: off \n");
    for i in 0..3000 {
        want.push_str(&format!(
            "  device-{i:05} \u{b7} ios \u{b7} user{i} \u{b7} previews on \u{b7} ok \u{b7} token fp{i:05}\n"
        ));
    }
    let c = case(
        "a large list",
        &["devices"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!((out.code, out.stderr.as_str()), (Some(0), ""), "the binary");
    assert!(out.stdout == want, "the binary's stdout differs");
    assert_eq!(log.len(), 1, "the binary's requests");
    rig.prepare(Side::Bash, &c);
    let bash = rig.run(true, &c);
    assert_eq!(
        rig.read(Side::Bash, false, c.name).len(),
        1,
        "bash's requests"
    );
    if bash.code == Some(0) {
        assert!(
            bash.stdout == want,
            "bash's stdout differs from the binary's"
        );
    } else {
        assert_eq!(bash.code, Some(126), "bash: {}", bash.stderr);
        assert!(
            bash.stderr.contains("Argument list too long"),
            "bash: {}",
            bash.stderr
        );
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn push_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    for (args, want) in [
        (
            &["devices", "x"][..],
            format!("usage: {cli} push devices [--json]\n"),
        ),
        (
            &["log", "x"],
            format!("usage: {cli} push log [--json] [--device <id>] (got 'x')\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// Help never acts: `-h` or `--help` on a line that would change
/// something prints help and sends nothing (the front hands it to the
/// shell, whose loop prints the help first).
#[test]
fn push_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell("e2e set -h", &["e2e", "sealed-only", "-h"]),
        shell(
            "device e2e --help",
            &["device", "e2e", "d1", "off", "--help"],
        ),
        shell("device remove -h", &["device", "remove", "-h", "d1"]),
    ] {
        rig.check(&c);
    }
}
