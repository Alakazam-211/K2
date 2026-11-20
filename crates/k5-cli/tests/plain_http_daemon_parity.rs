//! The daemon's host over plain http (Rosson's ruling, through the
//! Steward): a `K2_HOST` that is not this computer is allowed, with one
//! stderr line per run at the first request that carries the token. No
//! line for a loopback or Tailscale literal (judged as written), or with
//! `K2_PLAIN_HTTP_OK=1`.
//!
//! The stub daemon listens on 127.0.0.1; `K2_HOST=0.0.0.0` reaches it
//! (Linux and macOS connect 0.0.0.0 to this machine) and is no loopback
//! literal, so it stands for a host on the network.
//!
//! Until cli/k2 carries bash's line, the rows check the binary's side
//! against bash's with that one line added first, and print
//! PENDING bash fix; then they compare both sides exactly.

#![cfg(unix)]

mod common;

use common::parity::*;

const WHOAMI: &str = r#"{"workspace":"sales","role":"primary","address":"sales","primary":"sales","session":"s1","path":"/w/sales","siblings":{"live":1,"others":[]}}"#;
const SERVE: &str = r#"{"enabled":true,"port":4711,"url":"http://127.0.0.1:4711","publishedLabel":"","generatedAt":"2026-11-20T10:00:00Z"}"#;
const INDEX: &str = r#"{"workspacePath":"/srv/acme","wikiRel":".k2/wiki","generatedAt":"2026-11-20T10:00:00Z","nodes":[{"id":"Home.md","title":"Home"}],"links":[]}"#;

fn warning(host: &str) -> String {
    format!(
        "warning: {} is sending this server's owner token to {host} over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https.",
        k2_client::cli_name!()
    )
}

fn on_network(c: Case) -> Case {
    let mut c = c;
    c.env.push(("K2_HOST", "0.0.0.0".to_string()));
    c.env.push(("K2_PORT", PORT.to_string()));
    c
}

/// The row: exact parity once bash warns; until then, the binary's side
/// is bash's with the one line first on stderr.
fn check_warned(rig: &Rig, c: &Case) {
    if common::tunnel::bash_has("is sending this server's owner token to") {
        rig.check(c);
        return;
    }
    println!("PENDING bash fix: {}: bash's side plus the line", c.name);
    let mut outs = Vec::new();
    let mut logs = Vec::new();
    for (bash, side) in [(true, Side::Bash), (false, Side::Rust)] {
        rig.prepare(side, c);
        let out = rig.run(bash, c);
        let port = format!(":{}", rig.stub_of(side).port);
        let log: Vec<String> = rig
            .read(side, !bash, c.name)
            .iter()
            .map(|r| normalise(r, !bash, rig.noun(), c.name).replace(&port, ":{port}"))
            .collect();
        outs.push(out);
        logs.push(log);
    }
    let (b, r) = (&outs[0], &outs[1]);
    assert!(!b.stderr.contains("plain http"), "{}: bash: {b:?}", c.name);
    assert_eq!(logs[0], logs[1], "{}: the requests", c.name);
    assert!(!logs[1].is_empty(), "{}: nothing was sent", c.name);
    assert_eq!(
        (r.code, &r.stdout, &r.stderr),
        (
            b.code,
            &b.stdout,
            &format!("{}\n{}", warning("0.0.0.0"), b.stderr)
        ),
        "{}: the binary, then bash",
        c.name
    );
}

#[test]
fn whoami_to_a_daemon_on_the_network() {
    let rig = Rig::new("whoami", "plainhttp");
    let c = on_network(case(
        "whoami: K2_HOST on the network, the one line",
        &[],
        &[],
        vec![ok(WHOAMI)],
        owner_only,
        0,
        1,
    ));
    check_warned(&rig, &c);
    // Silenced: exact parity, no line on either side.
    let mut c = on_network(case(
        "whoami: K2_HOST on the network, K2_PLAIN_HTTP_OK=1",
        &[],
        &[],
        vec![ok(WHOAMI)],
        owner_only,
        0,
        1,
    ));
    c.env.push(("K2_PLAIN_HTTP_OK", "1".to_string()));
    rig.check(&c);
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
    check_warned(&rig, &c);
}
