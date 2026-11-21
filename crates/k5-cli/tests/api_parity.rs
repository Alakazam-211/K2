//! `k5-cli api` against the shell CLI's `cli/k2 api`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `on` and `off`: the switch's answers, the daemon's error, not JSON,
//!   Python's raise; a heal; a call that failed (`set -e`: its code); no
//!   answer (the line, exit 1).
//! - `off` reads the surface (`/boot-status` `api.enabled`, as `status`
//!   does): forced on by an env flag (the still-ON line on stderr, exit 1,
//!   no OFF line), really off (the OFF line), unknown (the OFF line and the
//!   note); `on` never reads it (`tests/cli/api_off_forced_on.sh`).
//! - `status`: the two reads (`/boot-status` with the token as Bearer,
//!   then `/cli/settings/get`), each answer kind, either one failing.
//! - The words: `${1:-status}`, the usage lines (exit 2), the gate first.
//! - Credentials: the owner from disk, a passport with no session; in a
//!   session with the owner token on disk the shell sends the owner token
//!   (COMPAT-58) and the binary the passport (`owner-token-with-passport`).
//! - What the binary hands to the shell: help, a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "api";
const BOOT_ON: &str = r#"{"version":"0.46.17","api":{"enabled":true}}"#;
const SET_OFF: &str = r#"{"apiEnabled":false,"other":1}"#;
/// `/boot-status` with no `api` object: the surface is unknown.
const BOOT_BARE: &str = r#"{"ok":true}"#;

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
fn api_on_and_off_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "switch");
    let v = vec![
        case(
            "on",
            &["on"],
            &[],
            vec![ok(r#"{"apiEnabled":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "off",
            &["off", "x"],
            &[],
            vec![ok(r#"{"apiEnabled":false}"#), ok(BOOT_BARE)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "on, the answer says off: the surface is read",
            &["on"],
            &[],
            vec![ok("{}"), ok(BOOT_BARE)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the owner-only refusal",
            &["on"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"owner or admin only"}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card",
            &["off"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin",
            &["on"],
            &[],
            vec![ok(BULLETIN), ok(BOOT_BARE)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "not JSON",
            &["on"],
            &[],
            vec![ok("<html>\u{e9}</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty 200",
            &["on"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a list with no error in it",
            &["on"],
            &[],
            vec![ok(r#"["x"]"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a list with an error in it",
            &["on"],
            &[],
            vec![ok(r#"["error"]"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a number",
            &["off"],
            &[],
            vec![ok("5")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a string that says error",
            &["off"],
            &[],
            vec![ok(r#""an error""#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a reset (56): set -e ends it with 56",
            &["on"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        waited(case(
            "an empty reply (52): the line, exit 1",
            &["off"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        // The words and the gates.
        case(
            "an unknown verb",
            &["enable"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown verb and no port: the gate",
            &["enable"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case("no token", &["on"], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &["on"],
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
fn api_status_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "status");
    let v = vec![
        case(
            "status: forced on by the env flag",
            &["status"],
            &[],
            vec![ok(BOOT_ON), ok(SET_OFF)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "no verb is status",
            &[],
            &[],
            vec![
                ok(r#"{"api":{"enabled":false}}"#),
                ok(r#"{"apiEnabled":true}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "an empty verb is status",
            &[""],
            &[],
            vec![ok("{}"), ok("{}")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "neither is JSON",
            &["status"],
            &[],
            vec![ok("nope"), ok("")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the settings refused",
            &["status"],
            &[],
            vec![ok(BOOT_ON), Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            0,
            2,
        ),
        case(
            "boot-status closes, the settings answer",
            &["status"],
            &[],
            vec![Reply::Close, ok(SET_OFF)],
            owner_and_port,
            0,
            2,
        ),
        traceback(case(
            "api is null",
            &["status"],
            &[],
            vec![ok(r#"{"api":null}"#), ok("{}")],
            owner_and_port,
            1,
            2,
        )),
        traceback(case(
            "the settings are a list",
            &["status"],
            &[],
            vec![ok(BOOT_ON), ok("[]")],
            owner_and_port,
            1,
            2,
        )),
        waited(case(
            "the settings never answer: the line, exit 1",
            &["status"],
            &[],
            vec![ok(BOOT_ON), Reply::Answer(503, String::new())],
            owner_and_port,
            1,
            2,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn api_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let v = vec![
        with(
            h(&[]),
            case(
                "no session: the passport",
                &["status"],
                &[],
                vec![ok(BOOT_ON), ok(SET_OFF)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["on"],
                &[],
                vec![heal(&rig), ok(r#"{"apiEnabled":true}"#)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a session with no owner token on disk: the passport",
                &["off"],
                &[],
                vec![ok(r#"{"apiEnabled":false}"#), ok(BOOT_BARE)],
                port_only,
                0,
                2,
            ),
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "a session with the owner token on disk",
                    &["status"],
                    &[],
                    vec![ok(BOOT_ON), ok(SET_OFF)],
                    owner_and_port,
                    0,
                    2,
                ),
            )
        },
        case(
            "a session with no passport: the owner token from disk (an open verb)",
            &["on"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![ok(r#"{"apiEnabled":true}"#)],
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
fn api_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    let v = vec![
        case(
            "status, nothing listens: both read as nothing",
            &["status"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            0,
            0,
        ),
        case(
            "on, nothing listens (7): set -e ends it with 7",
            &["on"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            7,
            0,
        ),
        case(
            "off, nothing listens (7): set -e ends it with 7",
            &["off"],
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

/// `off` after the setting is saved: the surface as `/boot-status` gives
/// it (`api.enabled`, read as `status` reads it). The rows of
/// `tests/cli/api_off_forced_on.sh` (its 13 checks: each row here holds
/// stdout, stderr, the exit and the requests), then more answers.
#[test]
fn api_off_says_when_the_surface_stays_on() {
    let rig = Rig::new(VERB, "surface");
    let saved_off = || ok(r#"{"apiEnabled":false}"#);
    let v = vec![
        // Checks 1-5: exit 1, the still-ON line on stderr, nothing on
        // stdout, the setting still posted, boot-status read.
        case(
            "forced on: the still-ON line, exit 1, no OFF line",
            &["off"],
            &[],
            vec![saved_off(), ok(r#"{"api":{"enabled":true}}"#)],
            owner_and_port,
            1,
            2,
        ),
        // Checks 6-8: exit 0, the OFF line alone, nothing on stderr.
        case(
            "really off: the OFF line alone",
            &["off"],
            &[],
            vec![saved_off(), ok(r#"{"api":{"enabled":false}}"#)],
            owner_and_port,
            0,
            2,
        ),
        // Checks 9-10: exit 0, the OFF line and the note.
        case(
            "boot-status says nothing about the API: the OFF line and the note",
            &["off"],
            &[],
            vec![saved_off(), ok(BOOT_BARE)],
            owner_and_port,
            0,
            2,
        ),
        // Checks 11-13: exit 0, the ON line, no boot-status read.
        case(
            "api on: the ON line, the surface never read",
            &["on"],
            &[],
            vec![ok(r#"{"apiEnabled":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // More answers.
        case(
            "forced on, the full boot-status",
            &["off"],
            &[],
            vec![saved_off(), ok(BOOT_ON)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "on, saved as off, forced on: the still-ON line",
            &["on"],
            &[],
            vec![ok(SET_OFF), ok(BOOT_ON)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "off, saved as on: the ON line, the surface never read",
            &["off"],
            &[],
            vec![ok(r#"{"apiEnabled":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "enabled is 1, not true: unknown",
            &["off"],
            &[],
            vec![saved_off(), ok(r#"{"api":{"enabled":1}}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "api is null: unknown",
            &["off"],
            &[],
            vec![saved_off(), ok(r#"{"api":null}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "boot-status is not JSON: unknown",
            &["off"],
            &[],
            vec![saved_off(), ok("nope")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "boot-status is a list: unknown",
            &["off"],
            &[],
            vec![saved_off(), ok("[true]")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "boot-status closes: unknown",
            &["off"],
            &[],
            vec![saved_off(), Reply::Close],
            owner_and_port,
            0,
            2,
        ),
        case(
            "boot-status answers 500 with the surface on: the still-ON line",
            &["off"],
            &[],
            vec![
                saved_off(),
                Reply::Answer(500, r#"{"api":{"enabled":true}}"#.into()),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "a refusal with a permission card: never reads the surface",
            &["off"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        with(
            with_passport(&[]),
            case(
                "forced on, with a passport: the still-ON line",
                &["off"],
                &[],
                vec![saved_off(), ok(BOOT_ON)],
                port_only,
                1,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The note names the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` it says `xy`), never a typed `k2` or `k5`; the
/// still-ON line names only the daemon's environment variables.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    let off = "Public /v1 API setting: OFF (live now \u{2014} no restart needed)";
    let c = case(
        "brand: unknown",
        &["off"],
        &[],
        vec![ok(r#"{"apiEnabled":false}"#), ok(BOOT_BARE)],
        owner_and_port,
        0,
        2,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stdout,
        format!(
            "{off}\nNote: where the K2_API (or legacy K2_SANDBOX_API) env flag is set, it still forces the surface on; check with: {cli} api status\n"
        )
    );
    assert_eq!((out.stderr.as_str(), out.code), ("", Some(0)));
    assert_eq!(log.len(), 2);
    let c = case(
        "brand: forced on",
        &["off"],
        &[],
        vec![ok(r#"{"apiEnabled":false}"#), ok(BOOT_ON)],
        owner_and_port,
        1,
        2,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(out.stdout, "");
    assert_eq!(
        out.stderr,
        "The public API is still ON: K2_API or K2_SANDBOX_API is set in the daemon's environment and forces it on. The setting is saved as off; to turn the API off, remove that variable from the daemon's service and restart it.\n"
    );
    assert_eq!(out.code, Some(1));
    assert_eq!(log.len(), 2);
    let c = case(
        "brand: usage",
        &["enable"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stderr
            .starts_with(&format!("Usage: {cli} api <on|off|status>\n")),
        "{}",
        out.stderr
    );
    assert!(log.is_empty());
}

/// `/boot-status` gives up after 10 s, read as nothing. Slow.
#[test]
fn api_status_with_a_boot_status_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&case(
        "boot-status hangs (28 after 10 s)",
        &["status"],
        &[],
        vec![Reply::Hang(Duration::from_secs(14)), ok(SET_OFF)],
        owner_and_port,
        0,
        2,
    ));
}

#[test]
fn api_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        // Help never acts (K2's rule): -h or --help anywhere prints the
        // usage on stdout, exit 0, and sends nothing (cmd_api's
        // `_wants_help`). The forms in src/help_captured.txt (`api`, `api
        // on` and `api status` with -h or --help) the binary answers
        // itself; the front hands the others to the shell's help.
        case(
            "--help: the usage, nothing sent",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "on -h: the usage, nothing sent",
            &["on", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "on --help: the usage, nothing sent",
            &["on", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        shell(case(
            "off -h: the usage, nothing sent",
            &["off", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "off --help: the usage, nothing sent",
            &["off", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "-h before on: the usage, nothing sent",
            &["-h", "on"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        case(
            "status -h: the usage, nothing sent",
            &["status", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "-h: the usage, nothing sent",
            &["-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "status --help: the usage, nothing sent",
            &["status", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // curl refuses the URL (3); `set -e` ends the command with it.
        shell(case(
            "a port that is no port: curl's 3",
            &["on"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            3,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
