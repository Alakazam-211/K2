//! `k5-cli firewall` against the shell CLI's `cli/k2 firewall`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code for every form.
//!
//! - The help: `help`, `--help`, `-h` anywhere the reading reaches it;
//!   with no daemon set up (the connection gate still stands), behind the
//!   virtual desktop's guard (`help` doesn't pass it, `--help` does), and
//!   in a session with no passport.
//! - The words: a value flag at the end, an unknown flag, an unknown
//!   verb, each verb's own checks, `guard`'s and `upstream`'s Python
//!   (its usage lines and a digit `int()` refuses); and the gates in
//!   front of them.
//! - Every verb with `--json` and without: `status` (and no verb, and
//!   `--json` first), `ports`, `rules` (each format, `--json` ignored),
//!   `ssh any|allow|port`, `open`, `close`, `guard`, `upstream`: the
//!   bodies (unicode, every flag) and the queries; the view's Python on
//!   many rows; answers above 256 KiB (`ports --json`, `rules`); the
//!   view's odd answers (not JSON, a field of another type).
//! - A refusal: 400, 401, 403 with the answer's own `Ask with:` line or
//!   "It can't be requested.", 404, 413, 500 with no body or not JSON, a
//!   body the refusal's Python can't read (the shell's `set -e`: exit 1,
//!   no body), 201; with `--json` the body
//!   too, never for `rules`.
//! - The owner from disk, a passport over TCP, a live cell socket (a GET
//!   goes on to TCP after no answer there, a POST only after "not
//!   served"; a POST that may have run is never sent again), a dead
//!   socket file, a bulletin (no line: `_firewall_render` notes nothing).
//! - No answer (an empty reply, a reset, not HTTP) and no daemon.
//!
//! The view's Python reads the answer from an environment variable, which
//! Linux caps at 128 KiB: the rows case stays under it (above it the
//! shell stops on "Argument list too long", the binary doesn't).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "firewall";

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

fn view() -> String {
    r#"{"box":"acme","hash":"sha256:ab12","version":3,"public":[
        {"port":22,"proto":"tcp","source":"ssh","allowFrom":["10.0.0.0/8","192.168.1.0/24"],"why":"SSH"},
        {"port":443,"proto":"tcp","source":"front443","why":"the :443 front é"},
        {"port":51820,"proto":"udp","source":"owner"}],
        "knownNotPublic":[{"port":4100,"proto":"tcp","source":"daemon","why":"loopback only"}],
        "upstream":{"managedBy":"k2","applied":{"hash":"sha256:ab12","provider":"acme","current":true}}}"#
        .to_string()
}

/// The view with `n` public rows.
fn rows(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"port":{},"proto":"{}","source":"owner","allowFrom":["10.{}.0.0/16"],"why":"row {i}"}}"#,
                1024 + i,
                if i % 2 == 0 { "tcp" } else { "udp" },
                i % 256
            )
        })
        .collect();
    format!(
        r#"{{"hash":"sha256:{}","version":7,"public":[{}],"knownNotPublic":[],"upstream":{{"managedBy":"owner"}}}}"#,
        "ab".repeat(32),
        rows.join(",")
    )
}

/// Many rows, under the shell's 128 KiB for the view's Python.
fn many_rows() -> String {
    let body = rows(1000);
    assert!(
        body.len() > 64 * 1024 && body.len() < 120 * 1024,
        "{} bytes",
        body.len()
    );
    body
}

/// Thousands of rows, above 256 KiB.
fn big_rows() -> String {
    let body = rows(3500);
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    body
}

/// A rule set above 256 KiB.
fn big_rules() -> String {
    let mut text = String::from("#!/usr/sbin/nft -f\ntable inet acme {\n");
    for i in 0..6000 {
        text.push_str(&format!(
            "  tcp dport {} accept comment \"row {i} é\"\n",
            1024 + i
        ));
    }
    text.push_str("}\n");
    assert!(text.len() > 256 * 1024, "{} bytes", text.len());
    text
}

fn guard_view() -> String {
    r#"{"guard":{"enabled":true,"connsPerIp":64,"writesPerMin":120,"apps":{"zeta":{"enabled":false},"alpha":{"enabled":true,"writesPerMin":30},"mid":{}}},"counters":{"perIpRefused":0}}"#.to_string()
}

#[test]
fn firewall_help_and_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    for (name, args) in [
        ("help", &["help"][..]),
        ("--help", &["--help"]),
        ("-h first, words after", &["-h", "x"]),
        ("--help after a verb", &["ports", "--help"]),
        ("--json then -h", &["--json", "-h"]),
        (
            "-h after a value flag's value",
            &["ssh", "--label", "x", "-h"],
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
        &["--help"],
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
        ("a value flag at the end", &["ports", "--format"][..]),
        ("--label at the end", &["open", "1/tcp", "--label"]),
        ("--managed-by at the end", &["upstream", "--managed-by"]),
        ("an unknown flag", &["ports", "--bogus"]),
        ("an unknown flag before -h", &["ports", "--bogus", "-h"]),
        ("--label=x is no flag", &["open", "--label=x"]),
        ("a lone -", &["status", "-"]),
        ("an unknown verb", &["nosuch"]),
        ("an unknown verb that is a flag", &["--bogus"]),
        (
            "rules, a format it doesn't have",
            &["rules", "--format", "iptables"],
        ),
        ("ssh alone", &["ssh"]),
        ("ssh allow with no address", &["ssh", "allow", "--json"]),
        ("ssh port with no number", &["ssh", "port"]),
        ("ssh port, not digits", &["ssh", "port", "2x"]),
        ("ssh, a form it doesn't have", &["ssh", "none"]),
        ("open with no label", &["open", "8080/tcp"]),
        (
            "open with an empty label",
            &["open", "8080/tcp", "--label", ""],
        ),
        ("open with no port", &["open", "--label", "x"]),
        ("close with two ports", &["close", "1/tcp", "2/tcp"]),
        ("guard, a third word", &["guard", "acme", "on", "x"]),
        (
            "guard, a count that is no number",
            &["guard", "--conns-per-ip", "12a"],
        ),
        ("upstream with nothing", &["upstream", "--provider", "p"]),
        (
            "upstream, a time that is no number",
            &["upstream", "--hash", "h", "--applied-at", "soon"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    v.push(traceback(case(
        "guard, a digit int() refuses",
        &["guard", "--writes-per-min", "\u{b2}"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    )));
    v.push(traceback(case(
        "upstream, a digit int() refuses",
        &["upstream", "--hash", "h", "--applied-at", "\u{b9}"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    )));
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
        &["ports"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    v.push(case(
        "a session with no passport",
        &["ports"],
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
fn firewall_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "verbs");
    let text = "public (applied by: nobody):\n  22/tcp  ssh  anyone\n\nno trailing newline";
    let changed = r#"{"opened":"8080/tcp","ssh":{"port":22,"allowFrom":[]},"note":"open it at your provider too","hash":"sha256:ef"}"#;
    let v = vec![
        // covers: firewall
        case(
            "status, no verb",
            &[],
            &[],
            vec![ok(text)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status",
            &["status", "x"],
            &[],
            vec![ok(text)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status --json",
            &["status", "--json"],
            &[],
            vec![ok("{\"public\":[]}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall --json
        case(
            "--json first",
            &["--json"],
            &[],
            vec![ok(&view())],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall ports
        case(
            "ports",
            &["ports"],
            &[],
            vec![ok(&view())],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall ports --json
        case(
            "ports --json",
            &["ports", "--json"],
            &[],
            vec![ok(&view())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ports, many rows",
            &["ports"],
            &[],
            vec![ok(&many_rows())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ports --json, thousands of rows",
            &["ports", "--json"],
            &[],
            vec![ok(&big_rows())],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall rules
        case(
            "rules, above 256 KiB",
            &["rules"],
            &[],
            vec![ok(&big_rules())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "rules --format ufw --json: the text",
            &["rules", "--format", "ufw", "--json"],
            &[],
            vec![ok("ufw allow 22/tcp\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "rules --format=nftables",
            &["rules", "--format=nftables"],
            &[],
            vec![ok("table inet acme {}\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "rules --format with an empty value",
            &["rules", "--format", ""],
            &[],
            vec![ok("x")],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall ssh any
        case(
            "ssh any",
            &["ssh", "any", "x"],
            &[],
            vec![ok(changed)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall ssh allow --json
        case(
            "ssh allow, two addresses",
            &["ssh", "allow", "10.0.0.0/8", "fd00::/8 é", "--json"],
            &[],
            vec![ok(
                r#"{"ssh":{"port":22,"allowFrom":["10.0.0.0/8","fd00::/8"]}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall ssh port
        case(
            "ssh port, as typed",
            &["ssh", "port", "0022"],
            &[],
            vec![ok(r#"{"ssh":{"port":22}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall open
        case(
            "open, every flag",
            &[
                "open",
                "8080/tcp",
                "--label",
                "web \"é\" ☃",
                "--from",
                "10.0.0.0/8",
                "--from",
                "",
            ],
            &[],
            vec![ok(changed)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall open --json
        case(
            "open --json",
            &["open", "--json", "1/udp", "--label", "x"],
            &[],
            vec![ok(r#"{"opened":"1/udp"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall close
        case(
            "close",
            &["close", "8080/tcp"],
            &[],
            vec![ok(r#"{"closed":"8080/tcp","hash":"sha256:ef"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall guard
        case(
            "guard alone",
            &["guard"],
            &[],
            vec![ok(&guard_view())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "guard, an App and every flag",
            &[
                "guard",
                "off",
                "acme é",
                "on",
                "--conns-per-ip",
                "007",
                "--writes-per-min",
                "\u{663}0",
                "--reset",
            ],
            &[],
            vec![ok(&guard_view())],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall guard --json
        case(
            "guard --json",
            &["guard", "off", "--json"],
            &[],
            vec![ok(&guard_view())],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall upstream
        case(
            "upstream, an apply",
            &[
                "upstream",
                "--hash",
                "sha256:ab12",
                "--provider",
                "acme",
                "--applied-at",
                "1700000000",
            ],
            &[],
            vec![ok(
                r#"{"upstream":{"managedBy":"auto","applied":{"hash":"sha256:ab12","provider":"acme"}}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upstream --managed-by",
            &["upstream", "--managed-by", "owner", "--applied-at", "x"],
            &[],
            vec![ok(r#"{"upstream":{"managedBy":"owner"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall ssh any --json
        case(
            "ssh any --json",
            &["ssh", "any", "--json"],
            &[],
            vec![ok(r#"{"ssh":{"port":22,"allowFrom":[]}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall ssh allow
        case(
            "ssh allow, one address",
            &["ssh", "allow", "10.0.0.0/8"],
            &[],
            vec![ok(r#"{"ssh":{"port":22,"allowFrom":["10.0.0.0/8"]}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall ssh port --json
        case(
            "ssh port --json",
            &["ssh", "port", "2222", "--json"],
            &[],
            vec![ok(r#"{"ssh":{"port":2222}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall close --json
        case(
            "close --json",
            &["close", "8080/tcp", "--json"],
            &[],
            vec![ok(r#"{"closed":"8080/tcp","hash":"sha256:ef"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: firewall upstream --json
        case(
            "upstream --json",
            &["upstream", "--json", "--managed-by", "auto"],
            &[],
            vec![ok(r#"{"upstream":{"managedBy":"auto"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a change's answer with nothing to say",
            &["close", "1/tcp"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the view, odd values",
            &["ports"],
            &[],
            vec![ok(
                r#"{"public":[{"port":"x","proto":1,"source":null,"allowFrom":"ab"}],"version":1,"hash":2.50,"upstream":{"managedBy":"K2"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "ports, not JSON",
            &["ports"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "ports, a row with no port",
            &["ports"],
            &[],
            vec![ok(r#"{"public":[{"proto":"tcp"}],"version":1}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "guard, Apps that are a list",
            &["guard"],
            &[],
            vec![ok(r#"{"guard":{"apps":[1]},"counters":1}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a change's note that is a number",
            &["open", "1/tcp", "--label", "x"],
            &[],
            vec![ok(r#"{"opened":"1/tcp","note":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an answer that is a number",
            &["ssh", "any"],
            &[],
            vec![ok("5")],
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
fn firewall_refusals() {
    let rig = Rig::new(VERB, "refuse");
    let v = vec![
        case(
            "403 with the answer's Ask with line",
            &["open", "1/tcp", "--label", "x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "403 --json: the body too",
            &["close", "1/tcp", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "403 that can't be requested",
            &["guard", "on"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner."},"permission":{"requestable":false}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "401",
            &["ports"],
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
            "400, an error and its message",
            &["ssh", "port", "70000"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"bad_port","message":"1 to 65535"}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "413 --json",
            &["ssh", "allow", "a", "--json"],
            &[],
            vec![Reply::Answer(413, r#"{"error":{"hint":"too big"}}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "404",
            &["upstream", "--managed-by", "k2"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":null}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "500, no body",
            &["status"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "500, not JSON",
            &["status", "--json"],
            &[],
            vec![Reply::Answer(
                500,
                format!("  boom é\r\n{}", "x".repeat(400)),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "rules --json refused: no body",
            &["rules", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "403, a body that is a list: set -e ends it, exit 1, no body",
            &["ports", "--json"],
            &[],
            vec![Reply::Answer(403, "[1]".into())],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "403, a command that is a number: set -e, exit 1",
            &["close", "1/tcp"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only"},"permission":{"command":7}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        )),
        case(
            "201 is no answer the shell takes",
            &["ports"],
            &[],
            vec![Reply::Answer(201, r#"{"public":[],"version":1}"#.into())],
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
fn firewall_with_a_passport() {
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
                &["ports"],
                &[],
                vec![ok(&view())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a bulletin in the answer: no line",
                &["ports", "--json"],
                &[],
                vec![ok(
                    r#"{"public":[],"version":1,"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
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
                &["close", "1/tcp"],
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
                &["status"],
                &[],
                vec![ok("public: none\n")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a POST",
                &["open", "1/tcp", "--label", "x"],
                &[],
                vec![ok(r#"{"opened":"1/tcp"}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "not served there: a POST goes on to TCP",
                &["guard", "acme", "--writes-per-min", "10"],
                &[],
                vec![not_served(), ok(&guard_view())],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "no answer there: a GET goes on to TCP",
                &["rules"],
                &[],
                vec![Reply::Close, ok("table inet acme {}\n")],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(&[]),
            case(
                "no answer there: a POST is never sent again",
                &["close", "1/tcp"],
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
                &["ssh", "any", "--json"],
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
                &["upstream", "--managed-by", "owner"],
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
                &["ssh", "port", "22"],
                &[],
                vec![ok(r#"{"ssh":{"port":22}}"#)],
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
fn firewall_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let port = dead_port();
    let v = vec![
        case(
            "an empty reply",
            &["ports"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["open", "1/tcp", "--label", "x", "--json"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["status"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listens",
            &["guard"],
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

/// Help never acts: each verb that changes something (`ssh`, `open`,
/// `close`, `guard`, `upstream`) with `-h` or `--help` first, in the
/// middle or last prints the help and sends nothing.
///
/// The rows named `bash acts:` are a bash bug, copied until bash is
/// fixed (then they flip to the help with no request): a help flag right
/// after a flag that takes a value is that value (`firewall.sh:103-111`,
/// `shift 2`), so the line goes on and its request is sent. `guard`'s
/// numbers are checked before anything is sent, so there the line is a
/// usage error and nothing goes out.
#[test]
fn firewall_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    let mut v = Vec::new();
    for (name, args) in [
        (
            "-h before the verb",
            &["-h", "open", "1/tcp", "--label", "x"][..],
        ),
        ("ssh -h any", &["ssh", "-h", "any"]),
        ("ssh any --help", &["ssh", "any", "--help"]),
        (
            "ssh allow --help <cidr>",
            &["ssh", "allow", "--help", "10.0.0.0/8"],
        ),
        ("ssh port 22 -h", &["ssh", "port", "22", "-h"]),
        ("open -h first", &["open", "-h", "1/tcp", "--label", "x"]),
        (
            "open --help last",
            &[
                "open",
                "1/tcp",
                "--label",
                "x",
                "--from",
                "10.0.0.0/8",
                "--help",
            ],
        ),
        ("close --help first", &["close", "--help", "1/tcp"]),
        ("close -h last", &["close", "1/tcp", "--json", "-h"]),
        ("guard -h first", &["guard", "-h", "on"]),
        (
            "guard --help last",
            &["guard", "acme", "off", "--reset", "--help"],
        ),
        (
            "upstream --help first",
            &["upstream", "--help", "--managed-by", "owner"],
        ),
        (
            "upstream -h last",
            &["upstream", "--hash", "h", "--provider", "p", "-h"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    for (name, args) in [
        (
            "guard: -h as --conns-per-ip's value is a usage line",
            &["guard", "--conns-per-ip", "-h"][..],
        ),
        (
            "guard: --help as --writes-per-min's value is a usage line",
            &["guard", "acme", "--writes-per-min", "--help"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    let done = r#"{"opened":"1/tcp","hash":"sha256:ef"}"#;
    for (name, args) in [
        (
            "bash acts: open, -h as --label's value (firewall.sh:103)",
            &["open", "1/tcp", "--label", "-h"][..],
        ),
        (
            "bash acts: close, --help as --from's value (firewall.sh:104)",
            &["close", "1/tcp", "--from", "--help"],
        ),
        (
            "bash acts: ssh any, -h as --hash's value (firewall.sh:108)",
            &["ssh", "any", "--hash", "-h"],
        ),
        (
            "bash acts: upstream, -h as --managed-by's value (firewall.sh:111)",
            &["upstream", "--managed-by", "-h"],
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
