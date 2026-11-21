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
//! - `K2_HOST=https://<name>` reaches the daemon over TLS, through the
//!   person's proxy for a name that isn't this computer (its user and
//!   password are allowed: K2's ruling), with no warning. A proxy that
//!   refuses the tunnel (407) is final: one CONNECT, curl 8.5's exit 56.
//!
//! The stub daemon listens on 127.0.0.1; `K2_HOST=0.0.0.0` reaches it
//! (Linux and macOS connect 0.0.0.0 to this machine) and is no loopback
//! literal, so it stands for a host on the network. Rows that send
//! nothing name a port nothing listens on.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::tunnel::{self, ProxyMode, ProxyStub, SideRun, TlsStub, HOST};

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
    assert_eq!(
        out.stderr.matches(&warning("0.0.0.0")).count(),
        1,
        "{out:?}"
    );
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

/// The virtual desktop's guard exits before the shell's plain-http step:
/// `explain <id>` and `schema` under it print the guard's two lines and
/// exit 3, with no warning before them. Without the guard both warn
/// first, as any verb that isn't one of the guard's offline words does.
#[test]
fn explain_and_schema_under_the_guard_on_the_network() {
    let guard: String = k5_cli::prelude::VIRTUAL_DESKTOP
        .iter()
        .map(|l| format!("{l}\n"))
        .collect();
    for (verb, args, name, unguarded) in [
        (
            "explain",
            &["app_exists"][..],
            "explain app_exists: the guard, no line",
            "explain app_exists: no guard, the line first",
        ),
        (
            "schema",
            &[][..],
            "schema: the guard, no line",
            "schema: no guard, the line first",
        ),
    ] {
        let rig = Rig::new(verb, "plainhttpguard");
        let host = [("K2_HOST", "0.0.0.0"), ("K2_PORT", NOWHERE)];
        let c = on(
            case(name, args, &[], vec![], owner_and_port, 3, 0),
            &[host[0], host[1], ("K2_NO_OWNER_FALLBACK", "1")],
        );
        let out = checked(&rig, &c);
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (Some(3), "", guard.as_str()),
            "{name}"
        );
        let c = on(
            case(unguarded, args, &[], vec![], owner_and_port, 0, 0),
            &host,
        );
        let out = checked(&rig, &c);
        assert_eq!(out.stderr, warning("0.0.0.0"), "{unguarded}");
        assert!(!out.stdout.is_empty(), "{unguarded}");
    }
}

fn tls_case(name: &'static str, pairs: &[(&'static str, &str)]) -> Case {
    on(case(name, &[], &[], vec![], owner_only, 0, 0), pairs)
}

fn no_warning(name: &str, runs: &[&SideRun]) {
    for run in runs {
        assert!(!run.out.stderr.contains("plain http"), "{name}: {run:?}");
        assert!(run.daemon.is_empty(), "{name}: the plain daemon: {run:?}");
    }
}

/// `K2_HOST=https://localhost`: TLS straight to it (loopback never uses
/// a proxy, and a proxy is set), no warning.
#[test]
fn whoami_over_tls_to_an_https_host() {
    let rig = Rig::new("whoami", "httpshost");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let daemon = TlsStub::start(&pki, 200, WHOAMI);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let port = daemon.port.to_string();
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let px = proxy.url(None);
    let c = tls_case(
        "whoami: K2_HOST=https://localhost",
        &[
            ("K2_HOST", "https://localhost"),
            ("K2_PORT", port.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
            ("https_proxy", px.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
    tunnel::assert_same(c.name, &b, &r);
    no_warning(c.name, &[&b, &r]);
    for run in [&b, &r] {
        assert!(run.proxy.is_empty(), "loopback never via a proxy: {run:?}");
        assert_eq!(run.tls.len(), 1, "{run:?}");
        assert!(
            run.tls[0]
                .0
                .contains(&format!("\r\nHost: localhost:{port}\r\n")),
            "{run:?}"
        );
        assert!(
            run.tls[0]
                .0
                .contains(&format!("\r\nAuthorization: Bearer {OWNER}\r\n")),
            "{run:?}"
        );
    }
    assert_eq!(r.out.code, Some(0), "{r:?}");
    assert!(r.out.stdout.contains("sales"), "{r:?}");
}

/// `K2_HOST=https://<a name>`: through the person's proxy, Basic in the
/// CONNECT (a proxy's user and password are allowed, with no refusal and
/// no warning), the owner token only inside the tunnel.
#[test]
fn whoami_over_tls_through_the_proxy() {
    let rig = Rig::new("whoami", "httpsproxy");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let daemon = TlsStub::start(&pki, 200, WHOAMI);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let port = daemon.port.to_string();
    let target = format!("{HOST}:{port}");
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let px = proxy.url(Some(("acme-proxy", "pr0xy-s3cret")));
    let k2_host = format!("https://{HOST}/");
    let c = tls_case(
        "whoami: K2_HOST=https://<name>/ through https_proxy",
        &[
            ("K2_HOST", k2_host.as_str()),
            ("K2_PORT", port.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
            ("https_proxy", px.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
    let basic = tunnel::basic_line("acme-proxy", "pr0xy-s3cret");
    for run in [&r, &b] {
        tunnel::assert_tunnel_only(c.name, run, &target, Some(&basic), OWNER);
    }
    tunnel::assert_same(c.name, &b, &r);
    no_warning(c.name, &[&b, &r]);
    assert_eq!(r.out.code, Some(0), "{r:?}");
    tunnel::assert_unprinted(c.name, &[b, r], &["pr0xy-s3cret", OWNER]);
}

const PRESETS: &str = r#"[{"id":"claude","label":"Claude Code","command":"claude","enabled":true,"is_built_in":true}]"#;

/// The shell's Python blocks open its `BASE_URL`: with
/// `K2_HOST=https://localhost`, TLS straight there (loopback never uses a
/// proxy, and one is set), the URL's netloc as `Host`, no warning. The
/// binary's urllib adapter does the same: `preset list` here.
#[test]
fn preset_list_over_tls_to_an_https_host() {
    let rig = Rig::new("preset", "httpshost");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let daemon = TlsStub::start(&pki, 200, PRESETS);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let port = daemon.port.to_string();
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let px = proxy.url(None);
    let c = on(
        case(
            "preset list: K2_HOST=https://localhost",
            &["list"],
            &[],
            vec![],
            owner_only,
            0,
            0,
        ),
        &[
            ("K2_HOST", "https://localhost"),
            ("K2_PORT", port.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
            ("https_proxy", px.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
    tunnel::assert_same(c.name, &b, &r);
    no_warning(c.name, &[&b, &r]);
    for run in [&b, &r] {
        assert!(run.proxy.is_empty(), "loopback never via a proxy: {run:?}");
        assert_eq!(run.tls.len(), 1, "{run:?}");
        let head = &run.tls[0].0;
        assert!(head.starts_with("GET /cli/presets/list "), "{head:?}");
        assert!(
            head.contains(&format!("\r\nHost: localhost:{port}\r\n")),
            "{run:?}"
        );
        assert!(
            head.contains(&format!("\r\nAuthorization: Bearer {OWNER}\r\n")),
            "{run:?}"
        );
    }
    assert_eq!(r.out.code, Some(0), "{r:?}");
    assert!(r.out.stdout.contains("Claude Code"), "{r:?}");
}

/// `K2_HOST=https://<a name>` with the person's proxy: the shell's Python
/// tunnels through it as http.client does; the binary's urllib adapter
/// doesn't copy that tunnel, so it sends nothing at all (no plain or
/// direct request in its place, the token nowhere) and fails (the
/// adapter's `URLError` says why: `urllib::tests::an_https_daemon`).
/// Bash's side is checked to have tunnelled.
#[test]
fn preset_list_over_tls_through_the_proxy_sends_nothing() {
    let rig = Rig::new("preset", "httpsproxy");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let daemon = TlsStub::start(&pki, 200, PRESETS);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let port = daemon.port.to_string();
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let px = proxy.url(None);
    let k2_host = format!("https://{HOST}");
    let c = on(
        case(
            "preset list: K2_HOST=https://<name> through https_proxy",
            &["list"],
            &[],
            vec![],
            owner_only,
            0,
            0,
        ),
        &[
            ("K2_HOST", k2_host.as_str()),
            ("K2_PORT", port.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
            ("https_proxy", px.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
    assert_eq!(b.out.code, Some(0), "{b:?}");
    assert!(!b.proxy.is_empty() && b.tls.len() == 1, "{b:?}");
    // preset's Python reads any failed urlopen as no answer.
    assert!(r.out.stderr.contains("no response from the "), "{r:?}");
    assert_ne!(r.out.code, Some(0), "{r:?}");
    assert!(
        r.proxy.is_empty() && r.tls.is_empty() && r.tls_errors.is_empty() && r.daemon.is_empty(),
        "the binary sent: {r:?}"
    );
    tunnel::assert_unprinted(c.name, &[r], &[OWNER]);
}

/// A proxy that refuses the tunnel (407) is final: the binary sends one
/// CONNECT and stops with curl 8.5's exit 56, as bash's curl 8.5 does.
/// Newer curl exits 7 there, which `_k2_curl` tries 4 times: the named
/// difference curl-version-proxy-exit (scripts/cli-parity.sh). On such a
/// node only the binary's side is checked here, and the row says so.
#[test]
fn whoami_through_a_proxy_that_refuses_the_tunnel() {
    let rig = Rig::new("whoami", "httpsrefused");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let daemon = TlsStub::start(&pki, 200, WHOAMI);
    let proxy = ProxyStub::start(ProxyMode::Refuse);
    let port = daemon.port.to_string();
    let target = format!("{HOST}:{port}");
    let k2_host = format!("https://{HOST}");
    let px = proxy.url(None);
    let c = tls_case(
        "whoami: K2_HOST=https://<name>, the proxy says 407",
        &[
            ("K2_HOST", k2_host.as_str()),
            ("K2_PORT", port.as_str()),
            ("https_proxy", px.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
    assert_eq!(r.proxy.len(), 1, "one CONNECT, never tried again: {r:?}");
    assert!(
        r.proxy[0].starts_with(&format!("CONNECT {target} HTTP/1.1\r\n")),
        "{r:?}"
    );
    no_warning(c.name, &[&b, &r]);
    for run in [&b, &r] {
        assert!(run.tls.is_empty(), "nothing past the refusal: {run:?}");
        assert_ne!(run.out.code, Some(0), "{run:?}");
    }
    tunnel::assert_unprinted(c.name, &[b.clone(), r.clone()], &[OWNER]);
    match b.proxy.len() {
        1 => tunnel::assert_same(c.name, &b, &r),
        4 => println!(
            "named difference curl-version-proxy-exit: {}: bash's curl tried the CONNECT 4 times; the binary's side alone",
            c.name
        ),
        n => panic!("{}: bash sent {n} CONNECTs, want 1 (curl 8.5) or 4 (newer curl): {b:?}", c.name),
    }
}

// The shared client's CONNECT tunnel (the Steward's conditions for it):
// the certificate is checked inside the tunnel, `no_proxy` and loopback
// go direct, and a proxy that won't tunnel is final. The daemon's name
// here is `0.0.0.0`: no loopback name, so it is proxied, yet a direct
// connect reaches the TLS stub, so a direct try in place of the tunnel
// shows in the stub's log. Its certificate holds `0.0.0.0`.

/// A daemon's PKI for `0.0.0.0` and `127.0.0.1` in its own folder.
fn ip_pki(rig: &Rig, name: &str) -> tunnel::Pki {
    tunnel::Pki::for_names(&rig.dir.join(name), &["0.0.0.0", "127.0.0.1"])
}

/// Nothing reached the TLS stub (no request, no connection) and the
/// command failed, on both sides; the token printed nowhere.
fn nothing_past_the_proxy(name: &str, b: &SideRun, r: &SideRun) {
    no_warning(name, &[b, r]);
    for run in [b, r] {
        assert!(
            run.tls.is_empty() && run.tls_errors.is_empty(),
            "{name}: the daemon was reached: {run:?}"
        );
        assert_ne!(run.out.code, Some(0), "{name}: {run:?}");
    }
    tunnel::assert_unprinted(name, &[b.clone(), r.clone()], &[OWNER]);
}

/// The proxy tunnels to a daemon whose certificate is from a CA the
/// person doesn't trust: refused inside the tunnel as it is direct
/// (curl's 60), nothing sent, the token never in the clear.
#[test]
fn whoami_through_the_proxy_to_a_certificate_from_another_ca() {
    let rig = Rig::new("whoami", "httpsotherca");
    let trusted = ip_pki(&rig, "trusted");
    let other = ip_pki(&rig, "other");
    let daemon = TlsStub::start(&other, 200, WHOAMI);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let port = daemon.port.to_string();
    let ca = trusted.ca_file.to_string_lossy().into_owned();
    let px = proxy.url(None);
    let c = tls_case(
        "whoami: K2_HOST=https://0.0.0.0 through https_proxy, another CA's certificate",
        &[
            ("K2_HOST", "https://0.0.0.0"),
            ("K2_PORT", port.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
            ("https_proxy", px.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
    let target = format!("0.0.0.0:{port}");
    for run in [&b, &r] {
        assert!(
            !run.proxy.is_empty()
                && run
                    .proxy
                    .iter()
                    .all(|h| h.starts_with(&format!("CONNECT {target} HTTP/1.1\r\n"))
                        && !h.contains(OWNER)),
            "{}: the CONNECTs: {run:?}",
            c.name
        );
        assert!(
            run.tls.is_empty(),
            "{}: a request was sent: {run:?}",
            c.name
        );
        // The handshake reached the stub through the tunnel, and only
        // through it: one try per CONNECT, none direct.
        assert!(
            !run.tls_errors.is_empty() && run.tls_errors.len() <= run.proxy.len(),
            "{}: {run:?}",
            c.name
        );
        assert_ne!(run.out.code, Some(0), "{}: {run:?}", c.name);
    }
    assert_eq!(b.out.code, r.out.code, "{}: bash, then the binary", c.name);
    no_warning(c.name, &[&b, &r]);
    tunnel::assert_unprinted(c.name, &[b, r], &[OWNER]);
}

/// `no_proxy` naming the daemon's host: straight there, the proxy sees
/// nothing (curl's rules: `proxy::tests::no_proxy_matches_as_curl_does`).
#[test]
fn whoami_with_no_proxy_naming_the_host_goes_direct() {
    let rig = Rig::new("whoami", "httpsnoproxy");
    let pki = ip_pki(&rig, "pki");
    let daemon = TlsStub::start(&pki, 200, WHOAMI);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let port = daemon.port.to_string();
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let px = proxy.url(None);
    let c = tls_case(
        "whoami: K2_HOST=https://0.0.0.0, https_proxy, no_proxy=0.0.0.0",
        &[
            ("K2_HOST", "https://0.0.0.0"),
            ("K2_PORT", port.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
            ("https_proxy", px.as_str()),
            ("no_proxy", "0.0.0.0"),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
    tunnel::assert_same(c.name, &b, &r);
    no_warning(c.name, &[&b, &r]);
    for run in [&b, &r] {
        assert!(
            run.proxy.is_empty(),
            "{}: the proxy was used: {run:?}",
            c.name
        );
        assert_eq!(run.tls.len(), 1, "{}: {run:?}", c.name);
        assert!(
            run.tls[0]
                .0
                .contains(&format!("\r\nAuthorization: Bearer {OWNER}\r\n")),
            "{}: {run:?}",
            c.name
        );
    }
    assert_eq!(r.out.code, Some(0), "{r:?}");
    assert!(r.out.stdout.contains("sales"), "{r:?}");
}

/// A loopback address with a proxy set: straight there, the proxy never
/// touched (`whoami_over_tls_to_an_https_host` is the `localhost` row).
#[test]
fn whoami_on_a_loopback_address_never_uses_the_proxy() {
    let rig = Rig::new("whoami", "httpsloopback");
    let pki = ip_pki(&rig, "pki");
    let daemon = TlsStub::start(&pki, 200, WHOAMI);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let port = daemon.port.to_string();
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let px = proxy.url(None);
    let c = tls_case(
        "whoami: K2_HOST=https://127.0.0.1 with https_proxy set",
        &[
            ("K2_HOST", "https://127.0.0.1"),
            ("K2_PORT", port.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
            ("https_proxy", px.as_str()),
            ("HTTPS_PROXY", px.as_str()),
            ("all_proxy", px.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
    tunnel::assert_same(c.name, &b, &r);
    no_warning(c.name, &[&b, &r]);
    for run in [&b, &r] {
        assert!(
            run.proxy.is_empty(),
            "{}: the proxy was used: {run:?}",
            c.name
        );
        assert_eq!(run.tls.len(), 1, "{}: {run:?}", c.name);
    }
    assert_eq!(r.out.code, Some(0), "{r:?}");
}

/// A proxy that answers the CONNECT with anything but a 2xx is final:
/// nothing past it, no direct or plain try in its place (curl 8.5's 56;
/// newer curl's 7 is tried 4 times by `_k2_curl`: the named difference
/// curl-version-proxy-exit, as for the 407 row).
#[test]
fn whoami_through_a_proxy_that_answers_403_or_502() {
    for (name, line) in [
        (
            "whoami: K2_HOST=https://0.0.0.0, the proxy says 403",
            "403 Forbidden",
        ),
        (
            "whoami: K2_HOST=https://0.0.0.0, the proxy says 502",
            "502 Bad Gateway",
        ),
    ] {
        let rig = Rig::new("whoami", "httpsproxystatus");
        let pki = ip_pki(&rig, "pki");
        let daemon = TlsStub::start(&pki, 200, WHOAMI);
        let proxy = ProxyStub::start(ProxyMode::Status(line));
        let port = daemon.port.to_string();
        let ca = pki.ca_file.to_string_lossy().into_owned();
        let px = proxy.url(None);
        let c = tls_case(
            name,
            &[
                ("K2_HOST", "https://0.0.0.0"),
                ("K2_PORT", port.as_str()),
                ("SSL_CERT_FILE", ca.as_str()),
                ("https_proxy", px.as_str()),
            ],
        );
        let [b, r] = tunnel::both(&rig, &c, &proxy, &daemon);
        assert_eq!(
            r.proxy.len(),
            1,
            "{name}: one CONNECT, never tried again: {r:?}"
        );
        assert!(
            r.proxy[0].starts_with(&format!("CONNECT 0.0.0.0:{port} HTTP/1.1\r\n")),
            "{name}: {r:?}"
        );
        nothing_past_the_proxy(name, &b, &r);
        match b.proxy.len() {
            1 => tunnel::assert_same(name, &b, &r),
            4 => println!(
                "named difference curl-version-proxy-exit: {name}: bash's curl tried the CONNECT 4 times; the binary's side alone"
            ),
            n => panic!("{name}: bash sent {n} CONNECTs, want 1 (curl 8.5) or 4 (newer curl): {b:?}"),
        }
    }
}

/// A proxy nothing listens on: the command fails, and nothing goes to
/// the daemon direct in its place.
#[test]
fn whoami_through_a_proxy_that_refuses_the_connection() {
    let rig = Rig::new("whoami", "httpsproxydown");
    let pki = ip_pki(&rig, "pki");
    let daemon = TlsStub::start(&pki, 200, WHOAMI);
    // Records nothing: only its log is read, and it is never the proxy.
    let unused = ProxyStub::start(ProxyMode::Tunnel);
    let port = daemon.port.to_string();
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let px = format!("http://127.0.0.1:{}", common::stub::dead_port());
    let c = tls_case(
        "whoami: K2_HOST=https://0.0.0.0, https_proxy at a closed port",
        &[
            ("K2_HOST", "https://0.0.0.0"),
            ("K2_PORT", port.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
            ("https_proxy", px.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &unused, &daemon);
    nothing_past_the_proxy(c.name, &b, &r);
    assert_eq!(
        (b.out.code, &b.out.stdout, &b.out.stderr),
        (r.out.code, &r.out.stdout, &r.out.stderr),
        "{}: bash, then the binary",
        c.name
    );
    assert_eq!(b.daemon, r.daemon, "{}: the daemon's requests", c.name);
}
