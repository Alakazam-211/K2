//! `k5-cli front` against the shell CLI's `cli/k2 front`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code for every form.
//!
//! - The help: `help`, `--help`, `-h` wherever each word reader reaches
//!   it (the main verbs', `http`'s, `passthrough`'s); with no daemon set
//!   up (the connection gate still stands), behind the virtual desktop's
//!   guard (`help` doesn't pass it, `--help` does), and in a session with
//!   no passport.
//! - The words: an argument the main verbs don't take, an unknown verb
//!   (`--json` first is one), `disable --mail`, `http` with no mode or a
//!   word it doesn't take, `passthrough` with a verb it doesn't have, an
//!   unknown flag, a value flag at the end, `add` with no host or no
//!   `--to`, `rm` with no host; and the gates in front of them.
//! - Every verb with `--json` and without: `status` (and no verb),
//!   `enable [--mail]`, `disable`, `http on|off`, `passthrough [list|ls]`,
//!   `passthrough add` (every flag, unicode), `passthrough rm|remove`;
//!   the front's whole status; many names, and an answer above 256 KiB
//!   (`passthrough --json`); a 200 that says it failed; the odd answers
//!   (not JSON, a field of another type).
//! - A refusal: `_front_render`'s (403 with the answer's `Ask with:`
//!   line, 400, 401, 404, 500, not JSON, a body its Python can't read,
//!   201) and the passthrough's, which is firewall's renderer (400, 403,
//!   404, 413), with `--json` and without.
//! - The owner from disk, a passport over TCP, a live cell socket (a GET
//!   goes on to TCP after no answer there, a POST only after "not
//!   served"; a POST that may have run is never sent again), a dead
//!   socket file, a bulletin (no line: neither renderer notes anything).
//! - No answer (an empty reply, a reset, not HTTP) and no daemon.
//!
//! The renderers' Python reads the answer from an environment variable,
//! which Linux caps at 128 KiB: the many-names case stays under it
//! (above it the shell stops on "Argument list too long", the binary
//! doesn't).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "front";

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// The front's whole status, every line it can print.
fn status() -> String {
    r#"{"enabled":true,"serving":true,"socket":{"source":"systemd"},"mail":true,
        "http":{"on":true,"serving":false,"next":"the daemon restarts once","helper":"install the :80 socket"},
        "cap":4096,"e2eCap":512,"nofileSoftLimit":65536,
        "routes":[{"host":"a.example","kind":"app","backend":"127.0.0.1:8080","proxyV2":true},{"host":"*","kind":"passthrough","backend":"127.0.0.1:9443"}],
        "port443":{"holder":"caddy","message":"Caddy holds it é"},
        "guard":{"perIpRefused":3,"guardLimited":0},
        "connections":{"accepted":10,"live":2,"overCap":0,"closed":{"eof":5,"reset":0,"timeout":1}},
        "helper":"ok","restartNeeded":true,
        "socketUnit":{"state":{"ActiveState":"active","SubState":"listening","Result":"success"},"journal":["line one","line two"]}}"#
        .to_string()
}

/// The front off, with names to pass through once it is on.
fn off() -> String {
    r#"{"enabled":false,"serving":false,"passthrough":[{"host":"a.example","to":"127.0.0.1:1"},{"host":"*","to":"127.0.0.1:2"}],"port443":{"holder":"free"},"socketUnit":{"state":{}}}"#.to_string()
}

/// The passthrough list with `n` names.
fn names(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"host":"site{i}.example","to":"127.0.0.1:{}","proxyV2":{},"label":"{}"}}"#,
                8000 + i % 1000,
                i % 2 == 0,
                if i % 3 == 0 { "mine é" } else { "" }
            )
        })
        .collect();
    format!(
        r#"{{"passthrough":[{}],"warnings":["one is shadowed"],"next":"routed now"}}"#,
        rows.join(",")
    )
}

/// Many names, under the shell's 128 KiB for the list's Python.
fn many_names() -> String {
    let body = names(1200);
    assert!(
        body.len() > 64 * 1024 && body.len() < 120 * 1024,
        "{} bytes",
        body.len()
    );
    body
}

/// Thousands of names, above 256 KiB.
fn big_names() -> String {
    let body = names(4000);
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    body
}

#[test]
fn front_help_and_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    for (name, args) in [
        ("help", &["help"][..]),
        ("--help", &["--help"]),
        ("-h first, words after", &["-h", "x"]),
        ("--help after the flags", &["enable", "--mail", "--help"]),
        ("an unknown verb's --help", &["nosuch", "--help"]),
        ("http's -h", &["http", "on", "-h"]),
        ("passthrough -h", &["passthrough", "-h"]),
        (
            "passthrough add's --help",
            &["passthrough", "add", "h", "--to", "x", "--help"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    v.push(case(
        "the help with no daemon set up: the gate stands",
        &["--help"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
    v.push(case(
        "help behind the virtual desktop's guard",
        &["help"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    v.push(case(
        "--help passes the guard, not the gate",
        &["passthrough", "--help"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        1,
        0,
    ));
    v.push(case(
        "the help in a session with no passport",
        &["help"],
        &[("K2_HOOK_SOCK", "{sock}")],
        vec![],
        owner_and_port,
        3,
        0,
    ));
    for (name, args) in [
        ("an argument the verbs don't take", &["status", "x"][..]),
        (
            "an unknown argument before -h",
            &["status", "--bogus", "-h"],
        ),
        ("--json first is a verb", &["--json"]),
        ("an unknown verb", &["nosuch"]),
        ("an unknown verb with --mail", &["nosuch", "--mail"]),
        ("disable --mail", &["disable", "--mail", "--json"]),
        ("http with no mode", &["http", "--json"]),
        ("http, a word it doesn't take", &["http", "maybe"]),
        ("http, an unknown word before -h", &["http", "x", "-h"]),
        (
            "passthrough, a verb it doesn't have",
            &["passthrough", "show"],
        ),
        ("passthrough, an unknown flag", &["passthrough", "-x"]),
        ("passthrough add with nothing", &["passthrough", "add"]),
        ("passthrough add with no --to", &["passthrough", "add", "h"]),
        (
            "passthrough add with no host",
            &["passthrough", "add", "--to", "127.0.0.1:1"],
        ),
        ("--to at the end", &["passthrough", "add", "h", "--to"]),
        (
            "--label at the end",
            &["passthrough", "add", "h", "--label"],
        ),
        (
            "passthrough rm with no host",
            &["passthrough", "rm", "--json"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    v.push(case(
        "a usage error with no daemon set up: the gate first",
        &["nosuch"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
    v.push(case(
        "the guard",
        &["status"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    v.push(case(
        "a session with no passport",
        &["passthrough"],
        &[("K2_HOOK_SOCK", "{sock}")],
        vec![],
        owner_and_port,
        3,
        0,
    ));
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn front_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "verbs");
    let changed = r#"{"enabled":true,"mail":true,"restarting":true,"warnings":["voice calls drop once","x"],"next":"wait a few seconds"}"#;
    let v = vec![
        // covers: front
        case(
            "status, no verb",
            &[],
            &[],
            vec![ok(&status())],
            owner_and_port,
            0,
            1,
        ),
        // covers: front status
        case(
            "status, the front off",
            &["status"],
            &[],
            vec![ok(&off())],
            owner_and_port,
            0,
            1,
        ),
        // covers: front status --json
        case(
            "status --json",
            &["status", "--json"],
            &[],
            vec![ok("{\"serving\":false}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        // covers: front enable
        case(
            "enable",
            &["enable"],
            &[],
            vec![ok(changed)],
            owner_and_port,
            0,
            1,
        ),
        // covers: front enable --json
        case(
            "enable --mail --json",
            &["enable", "--mail", "--json"],
            &[],
            vec![ok(changed)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "enable, a restart needed",
            &["enable", "--mail"],
            &[],
            vec![ok(r#"{"enabled":true,"restartNeeded":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: front disable
        case(
            "disable",
            &["disable"],
            &[],
            vec![ok(r#"{"enabled":false,"warnings":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: front http
        case(
            "http on",
            &["http", "off", "on"],
            &[],
            vec![ok(r#"{"on":true,"next":"the daemon restarts once"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: front http --json
        case(
            "http off --json",
            &["http", "--json", "off"],
            &[],
            vec![ok(r#"{"on":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: front disable --json
        case(
            "disable --json",
            &["disable", "--json"],
            &[],
            vec![ok(r#"{"enabled":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: front passthrough rm --json
        case(
            "passthrough rm --json",
            &["passthrough", "rm", "a.example", "--json"],
            &[],
            vec![ok(r#"{"passthrough":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a 200 that says it failed",
            &["enable"],
            &[],
            vec![ok(
                r#"{"ok":false,"error":{"code":"busy","hint":"try later"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 200 with an error and no serving",
            &["status"],
            &[],
            vec![ok(r#"{"error":"no front here"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 200 that is not JSON",
            &["disable"],
            &[],
            vec![ok("  <html> \n")],
            owner_and_port,
            1,
            1,
        ),
        // covers: front passthrough
        case(
            "passthrough",
            &["passthrough"],
            &[],
            vec![ok(
                r#"{"passthrough":[{"host":"a.example","to":"127.0.0.1:8443","proxyV2":true,"label":"mine"},{"host":"*","to":"127.0.0.1:9443"}],"warnings":["w"],"next":"restart"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "passthrough ls, none",
            &["passthrough", "ls"],
            &[],
            vec![ok(r#"{"passthrough":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: front passthrough --json
        case(
            "passthrough --json",
            &["passthrough", "--json"],
            &[],
            vec![ok(r#"{"passthrough":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "passthrough, many names",
            &["passthrough", "list"],
            &[],
            vec![ok(&many_names())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "passthrough --json, thousands of names",
            &["passthrough", "list", "--json"],
            &[],
            vec![ok(&big_names())],
            owner_and_port,
            0,
            1,
        ),
        // covers: front passthrough add
        case(
            "passthrough add, every flag",
            &[
                "passthrough",
                "add",
                "a.example é",
                "--to",
                "127.0.0.1:1",
                "--to=127.0.0.1:8443",
                "--proxy-v2",
                "--label",
                "my \"server\" ☃",
            ],
            &[],
            vec![ok(
                r#"{"passthrough":[{"host":"a.example","to":"127.0.0.1:8443","proxyV2":true,"label":"x"}],"next":"routed now"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: front passthrough add --json
        case(
            "passthrough add '*' --json",
            &["passthrough", "add", "*", "--to", "127.0.0.1:1", "--json"],
            &[],
            vec![ok(r#"{"passthrough":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: front passthrough rm
        case(
            "passthrough rm",
            &["passthrough", "rm", "a.example"],
            &[],
            vec![ok(r#"{"passthrough":[],"next":"the front picks it up"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "passthrough remove --json",
            &["passthrough", "remove", "*", "--json"],
            &[],
            vec![ok(r#"{"passthrough":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "status, an answer that is a list",
            &["status"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "status, a socket that is a list",
            &["status"],
            &[],
            vec![ok(r#"{"serving":1,"socket":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "status, a passthrough name with no host",
            &["status"],
            &[],
            vec![ok(r#"{"serving":0,"passthrough":[{"to":"y"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "passthrough, not JSON",
            &["passthrough"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "passthrough, a label that is a number",
            &["passthrough"],
            &[],
            vec![ok(r#"{"passthrough":[{"host":"a","label":5}]}"#)],
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
fn front_refusals() {
    let rig = Rig::new(VERB, "refuse");
    let v = vec![
        case(
            "403 with the answer's Ask with line",
            &["enable"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "403 --json: the body, exit 3",
            &["disable", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "400: exit 1",
            &["http", "on"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad"},"message":"why","permission":{"requestable":false}}"#
                    .into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "404 --json: exit 1",
            &["status", "--json"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"nf"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "401",
            &["status"],
            &[],
            vec![Reply::Answer(
                401,
                r#"{"error":"Invalid or missing auth token"}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "500, no body",
            &["enable", "--mail"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "403, not JSON",
            &["disable"],
            &[],
            vec![Reply::Answer(
                403,
                format!("  boom é\r\n{}", "x".repeat(300)),
            )],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "403, a body that is a list",
            &["status"],
            &[],
            vec![Reply::Answer(403, "[1]".into())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "201 is no answer the shell takes",
            &["status"],
            &[],
            vec![Reply::Answer(201, r#"{"enabled":true}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "passthrough add 403: firewall's lines",
            &["passthrough", "add", "h", "--to", "127.0.0.1:1"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "passthrough rm 404",
            &["passthrough", "rm", "h"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no such name"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "passthrough 400",
            &["passthrough"],
            &[],
            vec![Reply::Answer(400, r#"{"error":"bad"}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "passthrough --json 413: the body, exit 2",
            &["passthrough", "add", "h", "--to", "x", "--json"],
            &[],
            vec![Reply::Answer(413, r#"{"error":"too big"}"#.into())],
            owner_and_port,
            2,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn front_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a host session: the passport over TCP",
                &["status"],
                &[],
                vec![ok(&off())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a bulletin in the answer: no line",
                &["passthrough"],
                &[],
                vec![ok(
                    r#"{"passthrough":[],"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
                )],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a refused passport: no heal, no probe",
                &["enable"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a GET",
                &["passthrough", "ls"],
                &[],
                vec![ok(r#"{"passthrough":[]}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a POST",
                &["http", "on"],
                &[],
                vec![ok(r#"{"on":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "not served there: a POST goes on to TCP",
                &["passthrough", "rm", "h"],
                &[],
                vec![not_served(), ok(r#"{"passthrough":[]}"#)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "no answer there: a GET goes on to TCP",
                &["status"],
                &[],
                vec![Reply::Close, ok(&off())],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(&[]),
            case(
                "no answer there: a POST is never sent again",
                &["disable"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        waited(with(
            cell(&[]),
            case(
                "a reset there: a POST is never sent again",
                &["passthrough", "add", "h", "--to", "x", "--json"],
                &[],
                vec![Reply::Reset],
                port_only,
                1,
                1,
            ),
        )),
        with(
            cell(&[]),
            case(
                "a refusal there is the answer",
                &["enable", "--mail"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: a POST goes on to TCP",
                &["http", "off"],
                &[],
                vec![ok(r#"{"on":false}"#)],
                port_only,
                0,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn front_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let port = dead_port();
    let v = vec![
        case(
            "an empty reply",
            &["status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["enable", "--json"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["passthrough"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listens",
            &["passthrough", "rm", "h"],
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

/// Help never acts: each verb that changes something (`enable`,
/// `disable`, `http on|off`, `passthrough add`, `passthrough rm`) with
/// `-h` or `--help` first, in the middle or last prints the help and
/// sends nothing.
///
/// The rows named `bash acts:` are a bash bug, copied until bash is
/// fixed (then they flip to the help with no request): a help flag right
/// after `passthrough`'s `--to` or `--label` is that value
/// (`desktop.sh:529`, `desktop.sh:532`, `shift 2`), so the line goes on
/// and its request is sent.
#[test]
fn front_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    let mut v = Vec::new();
    for (name, args) in [
        ("-h before the verb", &["-h", "enable"][..]),
        ("enable -h", &["enable", "-h"]),
        ("enable --mail --help", &["enable", "--mail", "--help"]),
        ("disable --help", &["disable", "--help"]),
        ("disable --json -h", &["disable", "--json", "-h"]),
        ("http -h on", &["http", "-h", "on"]),
        ("http on --help", &["http", "on", "--help"]),
        (
            "http --help off --json",
            &["http", "--help", "off", "--json"],
        ),
        ("passthrough -h add", &["passthrough", "-h", "add", "h"]),
        (
            "passthrough add -h first",
            &["passthrough", "add", "-h", "h", "--to", "127.0.0.1:1"],
        ),
        (
            "passthrough add --help last",
            &[
                "passthrough",
                "add",
                "h",
                "--to",
                "127.0.0.1:1",
                "--proxy-v2",
                "--help",
            ],
        ),
        (
            "passthrough rm --help first",
            &["passthrough", "rm", "--help", "h"],
        ),
        (
            "passthrough remove -h last",
            &["passthrough", "remove", "h", "-h"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    let done = r#"{"passthrough":[],"next":"routed now"}"#;
    for (name, args) in [
        (
            "bash acts: passthrough add, -h as --label's value (desktop.sh:532)",
            &[
                "passthrough",
                "add",
                "h",
                "--to",
                "127.0.0.1:1",
                "--label",
                "-h",
            ][..],
        ),
        (
            "bash acts: passthrough add, --help as --to's value (desktop.sh:529)",
            &["passthrough", "add", "h", "--to", "--help"],
        ),
        (
            "bash acts: passthrough rm, -h as --label's value (desktop.sh:532)",
            &["passthrough", "rm", "h", "--label", "-h"],
        ),
    ] {
        // The front hands every line with -h or --help to the shell
        // (S0's front.rs): both sides' requests are the shell's.
        v.push(Case {
            compare: Compare::Shell,
            ..case(name, args, &[], vec![ok(done)], owner_and_port, 0, 1)
        });
    }
    for c in v {
        rig.check(&c);
    }
}
