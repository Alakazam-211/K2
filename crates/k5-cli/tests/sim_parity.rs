//! `k5-cli sim` against the shell CLI's `cli/k2 sim`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - Every verb's request and output: `status` (a device, none, held, the
//!   lanes' steps), `list-devices`, `boot`, `stop`, `erase --yes`,
//!   `install` (an absolute path), `launch`, `terminate`, `tree`, `tap`,
//!   `type`, `swipe`, `key`, `button`, `rotate`, `appearance`, `open-url`,
//!   `location`, `screenshot`, `logs` (more than 256 KiB of lines), `wait`;
//!   `--json`; `--workspace` as a folder and as a name the daemon resolves.
//! - The CLI's own refusals (`_sim_die`), with and without `--json`.
//! - The daemon's: held (2), a refusal with its closing line (3), a 400
//!   (2), a 5xx (1), not JSON, an answer a reader can't read (a raise).
//! - No answer: a dead port (curl 7), an empty reply (52), a reset (56),
//!   an empty 200, none in time (28), and a POST on the cell socket that
//!   got none (sent once).
//! - Help anywhere on every verb (no request), the gate, a session with
//!   no passport (the id tool), a passport on the cell socket.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "sim";
const LINE: &str = r#"{"line":"tapped e5 é"}"#;
const STATUS: &str = r#"{"lanes":[{"lane":"android","ready":true,"detail":"sdk 35","needs":[]},{"lane":"ios","usable":true,"setup":{"error":"x","licencesAccepted":2},"needs":[{"kind":"licence","title":"Xcode licence","id":"xcode"},{"kind":"install","what":"a runtime"},{"kind":"owner-step","what":"sign in","command":"xcrun x"}]}],"sim":{"sim":"s1","lane":"android","state":"running","profile":{"name":"Pixel 9","widthPt":412,"heightPt":915},"appId":"com.acme.app","hold":{"kind":"person","by":"pat"}},"agentsAllowed":false}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn app_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("app.apk"), b"apk").unwrap();
}

fn big_logs() -> String {
    let lines: Vec<String> = (0..20_000)
        .map(|i| format!("\"11-19 10:00:{:02} I/acme({i}): line {i} é\"", i % 60))
        .collect();
    let body = format!(r#"{{"lines":[{}],"truncated":true}}"#, lines.join(","));
    assert!(body.len() > 256 * 1024, "the large case is above 256 KiB");
    body
}

#[test]
fn sim_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let big: &'static str = Box::leak(big_logs().into_boxed_str());
    let v = vec![
        case(
            "status",
            &["status"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status, no device",
            &["status"],
            &[],
            vec![ok(
                r#"{"lanes":[{"lane":"android","ready":true,"setup":{"running":true,"licencesAccepted":3}}],"sim":{"state":"failed","reason":"boot timed out"},"agentsAllowed":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sim --json
        case(
            "status --json",
            &["status", "--json"],
            &[],
            vec![ok(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status with words",
            &["status", "x", "y"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "list-devices",
            &["devices", "--platform=android"],
            &[],
            vec![ok(
                r#"{"devices":[{"id":"pixel_9","name":"Pixel 9","widthPt":412,"heightPt":915,"os":"android-35","default":true},{"id":"x"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list-devices, none",
            &["list-devices"],
            &[],
            vec![ok(r#"{"devices":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list-devices, a bad platform",
            &["list-devices", "--platform", "web"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "boot",
            &[
                "boot",
                "--platform",
                "android",
                "--device",
                "pixel_9",
                "--no-tab",
            ],
            &[],
            vec![ok(
                r#"{"sim":"s1","lane":"android","state":"running","profile":{"name":"Pixel 9"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "start, booting",
            &["start"],
            &[],
            vec![ok(r#"{"sim":"s1","lane":"ios","state":"booting"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case("stop", &["stop"], &[], vec![ok("{}")], owner_and_port, 0, 1),
        case(
            "erase --yes",
            &["erase", "-y"],
            &[],
            vec![ok(r#"{"line":"Erased."}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "erase with no terminal",
            &["erase"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "erase with no terminal, --json",
            &["erase", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: sim install
        case(
            "install",
            &["install", "./app.apk"],
            &[],
            vec![ok("{}")],
            app_file,
            0,
            1,
        ),
        case(
            "install, no file",
            &["install", "nope.apk"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "launch",
            &["launch", "com.acme"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "kill",
            &["kill", "com.acme"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "launch, no app",
            &["launch"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: sim tree
        case(
            "tree",
            &["tree", "--all", "--find", "Save it", "--boxes", "-w", "."],
            &[],
            vec![ok(r#"{"text":"e1  button \"Save\"\ne5  x"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sim tree --json
        case(
            "tree --json",
            &["tree", "--json"],
            &[],
            vec![ok(r#"{"text":"e1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sim install --json
        case(
            "install --json",
            &["install", "app.apk", "--json"],
            &[],
            vec![ok(r#"{"line":"Installed"}"#)],
            app_file,
            0,
            1,
        ),
        // covers: sim screenshot --json
        case(
            "screenshot --json",
            &["screenshot", "--json"],
            &[],
            vec![ok(r#"{"path":"/tmp/s.png"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tree, no text",
            &["tree"],
            &[],
            vec![ok(r#"{"text":""}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sim tap
        case(
            "tap",
            &["tap", "t3:e5"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tap --at",
            &["tap", "--at=120.5,300"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tap, a bad id",
            &["tap", "x5"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: sim tap --json
        case(
            "tap, a bad id, --json",
            &["tap", "--json", "x5"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "tap, both",
            &["tap", "e1", "--at", "1,2"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("tap, nothing", &["tap"], &[], vec![], owner_and_port, 2, 0),
        case(
            "tap, --at last",
            &["tap", "--at"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "type",
            &["type", "--into", "e3", "--", "-a \"b\" é"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "type --secret",
            &["type", "--secret=pw", "--into=e3"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "type, both",
            &["type", "x", "--secret", "pw"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "type, two texts",
            &["type", "x", "y"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "swipe",
            &["swipe", "up", "--from", "e7"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "swipe, not a direction",
            &["swipe", "in"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "key",
            &["key", "arrow-up"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "key, not a key",
            &["key", "f1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "button",
            &["button", "home"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "rotate",
            &["rotate", "right"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "appearance",
            &["appearance"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "open",
            &["open", "myapp://x?y=1"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "open-url, no scheme",
            &["open-url", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "location",
            &["location", "-37.33,122"],
            &[],
            vec![ok(LINE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "location, bad",
            &["location", "1,2,3"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: sim screenshot
        case(
            "screenshot",
            &["shot", "--region", "e2"],
            &[],
            vec![ok(r#"{"path":"/tmp/s.png"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "screenshot, no path",
            &["screenshot"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "logs, more than 256 KiB",
            &[
                "logs", "--app", "com.acme", "--since", "90s", "--grep", "a b", "--lines", "20000",
            ],
            &[],
            vec![ok(big)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "logs, none",
            &["log"],
            &[],
            vec![ok(r#"{"lines":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "logs, a bad since",
            &["logs", "--since", "2d"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "logs, lines 0",
            &["logs", "--lines", "0"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "wait",
            &["wait", "--for", "Saved", "--timeout=1.5m"],
            &[],
            vec![ok(r#"{"line":"found it"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "wait, too long",
            &["wait", "--timeout", "3m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--workspace, a name",
            &["stop", "--workspace", "docs"],
            &[],
            vec![ok(r#"{"path":"/w/docs"}"#), ok("{}")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "--workspace, a name the daemon doesn't know",
            &["tree", "-w", "a b"],
            &[],
            vec![
                Reply::Answer(404, r#"{"error":"no such"}"#.into()),
                ok("{}"),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "-w last",
            &["tree", "--json", "-w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "held",
            &["tap", "e5"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":"held","code":"sim_held","message":"pat is using the device"}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "held, --json",
            &["tap", "e5", "--json"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"held"}}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a refusal's closing line",
            &["boot"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 400",
            &["tap", "e5"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"stale ids","code":"sim_stale_element"}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a 500, not JSON",
            &["status"],
            &[],
            vec![Reply::Answer(500, " <html>oops</html> ".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 404, nothing said",
            &["stop"],
            &[],
            vec![Reply::Answer(404, String::new())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an empty 200",
            &["stop"],
            &[],
            vec![ok("")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bulletin",
            &["stop"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sim
        case("sim alone: help", &[], &[], vec![], owner_and_port, 0, 0),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case("help with no daemon", &["help"], &[], vec![], nothing, 0, 0),
        case(
            "an unknown verb",
            &["zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("the gate", &["status"], &[], vec![], nothing, 1, 0),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a passport on the cell socket",
                &["tap", "e5"],
                &[],
                vec![ok(LINE)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a passport, a GET on the cell socket",
                &["status"],
                &[],
                vec![ok(STATUS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{sock}".to_string())],
            case(
                "a session with no passport",
                &["status"],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ),
        with(
            vec![("K2_PORT", dead_port())],
            case("a dead port", &["stop"], &[], vec![], owner_only, 1, 0),
        ),
        case(
            "an empty reply",
            &["stop"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["status"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        waited(with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "no answer on the socket: never sent again",
                &["tap", "e5"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
    for (name, args, body) in [
        ("status, a list", &["status"][..], "[1]"),
        ("status, not JSON", &["status"], "nope"),
        (
            "status, a reason that is a number",
            &["status"],
            r#"{"sim":{"state":"failed","reason":5}}"#,
        ),
        (
            "devices, a list of words",
            &["list-devices"],
            r#"{"devices":"ab"}"#,
        ),
        ("boot, a list", &["boot"], "[1]"),
        ("logs, empty", &["logs"], ""),
    ] {
        rig.check(&Case {
            compare: Compare::Traceback,
            ..case(name, leak(args), &[], vec![ok(body)], owner_and_port, 1, 1)
        });
    }
}

fn leak(args: &[&str]) -> &'static [&'static str] {
    let v: Vec<&'static str> = args
        .iter()
        .map(|a| &*Box::leak(a.to_string().into_boxed_str()))
        .collect();
    Box::leak(v.into_boxed_slice())
}

/// Help anywhere before a bare `--` prints the help page and sends
/// nothing, for every verb (each that changes something too). After `--`
/// it is the text (`type -- --help` types it).
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    let lines: &'static [&'static [&'static str]] = &[
        &["boot", "--platform", "android", "-h"],
        &["stop", "--help"],
        &["erase", "--yes", "-h"],
        &["install", "app.apk", "--help"],
        &["launch", "-h", "com.acme"],
        &["terminate", "com.acme", "--help"],
        &["tap", "e5", "-h"],
        &["tap", "--at", "-h"],
        &["type", "-h", "--", "x"],
        &["swipe", "up", "--help"],
        &["key", "enter", "-h"],
        &["button", "home", "--help"],
        &["rotate", "left", "-h"],
        &["appearance", "dark", "--help"],
        &["open-url", "https://x", "-h"],
        &["location", "1,2", "--help"],
        &["screenshot", "-h"],
        &["wait", "--help"],
        &["status", "-h"],
        &["list-devices", "--help"],
        &["tree", "-h"],
        &["logs", "--help"],
        &["zap", "-h"],
        &["-w", "docs", "status", "--help"],
    ];
    for args in lines {
        rig.check(&case(
            "help anywhere",
            args,
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
        rig.check(&case(
            "help anywhere, no daemon",
            args,
            &[],
            vec![],
            nothing,
            0,
            0,
        ));
    }
}

/// None in time (curl 28): `wait --timeout 0s` gives the call 30 s.
#[test]
fn no_answer_in_time() {
    let rig = Rig::new(VERB, "late");
    rig.check(&case(
        "none in time",
        &["wait", "--timeout", "0s"],
        &[],
        vec![Reply::Hang(Duration::from_secs(33))],
        owner_and_port,
        1,
        1,
    ));
}
