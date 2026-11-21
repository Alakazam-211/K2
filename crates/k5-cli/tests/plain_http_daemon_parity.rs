//! The daemon's host (Rosson's ruling, through the Steward; the shell's
//! prelude, `cli/src/cli-tools-policy.sh` after `_k2_daemon_base`):
//!
//! - `K2_HOST` that isn't this computer as written is allowed over plain
//!   http, with one stderr line per run, before the command's first
//!   output or request, however many requests it makes. None for a
//!   loopback or Tailscale literal, `-h`/`--help`, or
//!   `K2_PLAIN_HTTP_OK=1` (any of its names).
//! - A password flag there is refused, exit 2, nothing sent, the switch or
//!   not, a Tailscale host too.
//!
//! The stub daemon listens on 127.0.0.1; `K2_HOST=0.0.0.0` reaches it
//! (Linux and macOS connect 0.0.0.0 to this machine) and is no loopback
//! literal, so it stands for a host on the network. Rows that send
//! nothing name a port nothing listens on.

#![cfg(unix)]

mod common;

use common::parity::*;

const WHOAMI: &str = r#"{"workspace":"sales","role":"primary","address":"sales","primary":"sales","session":"s1","path":"/w/sales","siblings":{"live":1,"others":[]}}"#;
const SERVE: &str = r#"{"enabled":true,"port":4711,"url":"http://127.0.0.1:4711","publishedLabel":"","generatedAt":"2026-11-20T10:00:00Z"}"#;
const INDEX: &str = r#"{"workspacePath":"/srv/acme","wikiRel":".k2/wiki","generatedAt":"2026-11-20T10:00:00Z","nodes":[{"id":"Home.md","title":"Home"}],"links":[]}"#;

/// A port for rows that send nothing.
const NOWHERE: &str = "4711";

fn warning(host: &str) -> String {
    format!(
        "warning: {} is sending this server's owner token to {host} over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https.\n",
        k2_client::cli_name!()
    )
}

fn refusal(url: &str) -> String {
    format!("Error: a password never goes over plain http:// to another computer; use K2_HOST=https://<name> (got: {url})\n")
}

fn on(c: Case, pairs: &[(&'static str, &str)]) -> Case {
    let mut c = c;
    c.env
        .extend(pairs.iter().map(|(k, v)| (*k, (*v).to_string())));
    c
}

fn on_network(c: Case) -> Case {
    on(c, &[("K2_HOST", "0.0.0.0"), ("K2_PORT", PORT)])
}

/// Runs `c` on both sides, compares them, and returns bash's side.
fn checked(rig: &Rig, c: &Case) -> Out {
    rig.check(c);
    rig.prepare(Side::Bash, c);
    let out = rig.run(true, c);
    rig.read(Side::Bash, false, c.name);
    out
}

#[test]
fn whoami_to_a_daemon_on_the_network() {
    let rig = Rig::new("whoami", "plainhttp");
    let warned = on_network(case(
        "whoami: K2_HOST on the network, the one line",
        &[],
        &[],
        vec![ok(WHOAMI)],
        owner_only,
        0,
        1,
    ));
    let out = checked(&rig, &warned);
    assert!(out.stderr.starts_with(&warning("0.0.0.0")), "{out:?}");
    assert_eq!(out.stderr.matches("over plain http").count(), 1, "{out:?}");
    for (name, var, value) in [
        ("whoami: K2_PLAIN_HTTP_OK=1", "K2_PLAIN_HTTP_OK", "1"),
        ("whoami: K5_PLAIN_HTTP_OK=1", "K5_PLAIN_HTTP_OK", "1"),
        ("whoami: K2SO_PLAIN_HTTP_OK=1", "K2SO_PLAIN_HTTP_OK", "1"),
    ] {
        let c = on(
            on_network(case(name, &[], &[], vec![ok(WHOAMI)], owner_only, 0, 1)),
            &[(var, value)],
        );
        let out = checked(&rig, &c);
        assert!(!out.stderr.contains("plain http"), "{name}: {out:?}");
    }
    // Only `1` silences it, and the first name set is the one read.
    let c = on(
        on_network(case(
            "whoami: K5_PLAIN_HTTP_OK=0 before K2_PLAIN_HTTP_OK=1",
            &[],
            &[],
            vec![ok(WHOAMI)],
            owner_only,
            0,
            1,
        )),
        &[("K5_PLAIN_HTTP_OK", "0"), ("K2_PLAIN_HTTP_OK", "1")],
    );
    let out = checked(&rig, &c);
    assert!(out.stderr.starts_with(&warning("0.0.0.0")), "{out:?}");
}

/// The line comes before a usage error that sends nothing, and never
/// with help.
#[test]
fn whoami_usage_and_help_on_the_network() {
    let rig = Rig::new("whoami", "plainhttpusage");
    let host = [("K2_HOST", "0.0.0.0"), ("K2_PORT", NOWHERE)];
    let c = on(
        case(
            "whoami --bogus: the line, then the usage error",
            &["--bogus"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        &host,
    );
    let out = checked(&rig, &c);
    assert!(out.stderr.starts_with(&warning("0.0.0.0")), "{out:?}");
    assert!(out.stderr.contains("unknown flag '--bogus'"), "{out:?}");
    for (name, args) in [
        ("whoami --help: no line", &["--help"][..]),
        ("whoami -h: no line", &["-h"][..]),
    ] {
        let c = on(case(name, args, &[], vec![], owner_only, 0, 0), &host);
        let out = checked(&rig, &c);
        assert!(!out.stderr.contains("plain http"), "{name}: {out:?}");
    }
}

/// Two requests, one line: once per run.
#[test]
fn wiki_status_to_a_daemon_on_the_network_warns_once() {
    let rig = Rig::new("wiki", "plainhttp");
    let c = on_network(case(
        "wiki status: two requests, the line once",
        &["status"],
        &[],
        vec![ok(SERVE), ok(INDEX)],
        owner_only,
        0,
        2,
    ));
    let out = checked(&rig, &c);
    assert_eq!(out.stderr.matches(&warning("0.0.0.0")).count(), 1, "{out:?}");
}

/// A password never goes over plain http to another computer: exit 2,
/// the one line, nothing sent; `K2_PLAIN_HTTP_OK=1` and a Tailscale
/// host don't change it.
#[test]
fn users_password_flags_to_a_daemon_on_the_network() {
    let rig = Rig::new("users", "plainhttp");
    for (name, args, host, extra) in [
        (
            "users add --password-stdin, K2_HOST on the network",
            &["add", "bob", "--password-stdin"][..],
            "0.0.0.0",
            None,
        ),
        (
            "users add --password-stdin, K2_PLAIN_HTTP_OK=1",
            &["add", "bob", "--password-stdin"][..],
            "0.0.0.0",
            Some(("K2_PLAIN_HTTP_OK", "1")),
        ),
        (
            "users add --password-stdin, a Tailscale host",
            &["add", "bob", "--password-stdin"][..],
            "100.64.0.1",
            None,
        ),
        (
            "users set-password --password-stdin, K2_HOST on the network",
            &["set-password", "bob", "--password-stdin"][..],
            "0.0.0.0",
            None,
        ),
        (
            "users add --password <pw>, K2_HOST on the network",
            &["add", "bob", "--password", "pw-net-5151"][..],
            "0.0.0.0",
            None,
        ),
        (
            "users add --password=<pw>, K2_HOST on the network",
            &["add", "bob", "--password=pw-net-5151"][..],
            "0.0.0.0",
            None,
        ),
    ] {
        let mut c = on(
            case(name, args, &[], vec![], owner_only, 2, 0),
            &[("K2_HOST", host), ("K2_PORT", NOWHERE)],
        );
        if let Some(pair) = extra {
            c = on(c, &[pair]);
        }
        c.stdin = Some(b"pw-net-5151\n");
        let out = checked(&rig, &c);
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (
                Some(2),
                "",
                refusal(&format!("http://{host}:{NOWHERE}")).as_str()
            ),
            "{name}"
        );
    }
}
