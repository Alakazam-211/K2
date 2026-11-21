//! The shell's Python blocks at `K2_HOST=https://<name>` with the
//! person's proxy set (K2's ruling): bash's Python tunnels through it
//! (`_K2_PY_PROXY_RULE`'s `ProxyHandler`, `http.client`'s CONNECT), and the
//! binary's urllib families (db, store, calendar, project; preset's row
//! is in `plain_http_daemon_parity.rs`) do the
//! same on the shared CONNECT path, reading the variables and `no_proxy`
//! as urllib reads them:
//!
//! - each family through the proxy: the same output, the same CONNECT
//!   head byte for byte (the proxy's own `user:password` as Basic, never
//!   the owner token or the passport), the same request inside the tunnel
//!   with the token there alone, the daemon's certificate checked;
//! - `no_proxy` naming the host (exactly, among blank-padded tokens, with
//!   its port, or `*`): straight there, the proxy gets nothing;
//! - `all_proxy` alone: Python's urllib never reads it for https, so
//!   straight there;
//! - a proxy that refuses the tunnel (407): one CONNECT, final;
//! - a proxy nothing listens on: the refused connect, as Python's;
//! - a daemon certificate nothing trusts: no byte of the request sent on
//!   either side (the words differ: python-tls-error-text).
//!
//! `workspace` answers with the transport's curl-shaped client, so its row
//! is curl's CONNECT ([`tunnel::assert_same`]).
//!
//! The daemon's name is under `.invalid` (it never resolves), so a side
//! that goes straight there where it should use the proxy fails to
//! connect. The `no_proxy` rows use `0.0.0.0` ([`tunnel::NETWORK`]),
//! which reaches this machine and is no loopback literal.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::tunnel::{self, ProxyMode, ProxyStub, SideRun, TlsStub, HOST, NETWORK};

const STATUS: &str = r#"{"ok":true,"supported":true,"state":"running","installedMajor":17,"listen":"127.0.0.1","port":5432,"clusters":[{"major":17,"port":5432,"role":"primary","state":"online"}]}"#;
const COLLECTIONS: &str = r#"{"ok":true,"collections":["a","é"]}"#;
const CALENDARS: &str = r#"{"ok":true,"address":"a@x.example","calendars":[{"id":"c1","name":"Work","isDefault":true,"rights":{"read":true,"write":true}}]}"#;
const CREATED: &str = r#"{"ok":true,"id":"pg_1","name":"Acme","memberCount":"2"}"#;
const WORKSPACES: &str =
    r#"[{"name":"acme","agentMode":"agent","path":"/srv/acme","agentEnabled":true}]"#;

/// The proxy's own credential, in the proxy URL.
const PROXY_USER: &str = "acme-proxy";
const PROXY_PASS: &str = "pr0xy-s3cret";

/// One row's world: a TLS daemon for [`HOST`] (and [`NETWORK`]) answering
/// `body`, a recording proxy, and the throwaway CA.
struct World {
    rig: Rig,
    daemon: TlsStub,
    proxy: ProxyStub,
    ca: String,
}

impl World {
    fn new(verb: &'static str, tag: &str, body: &'static str, mode: ProxyMode) -> World {
        let rig = Rig::new(verb, tag);
        let pki = tunnel::Pki::new(&rig.dir.join("pki"));
        let daemon = TlsStub::start(&pki, 200, body);
        let proxy = ProxyStub::start(mode);
        let ca = pki.ca_file.to_string_lossy().into_owned();
        World {
            rig,
            daemon,
            proxy,
            ca,
        }
    }

    fn port(&self) -> String {
        self.daemon.port.to_string()
    }

    /// The case at `K2_HOST=https://<host>`, trusting the CA, with `extra`.
    fn case(
        &self,
        name: &'static str,
        args: &'static [&'static str],
        host: &str,
        extra: &[(&'static str, String)],
    ) -> Case {
        let mut c = case(name, args, &[], vec![], owner_only, 0, 0);
        c.env.push(("K2_HOST", format!("https://{host}")));
        c.env.push(("K2_PORT", self.port()));
        c.env.push(("SSL_CERT_FILE", self.ca.clone()));
        c.env.extend(extra.iter().cloned());
        c
    }

    fn both(&self, c: &Case) -> [SideRun; 2] {
        tunnel::both(&self.rig, c, &self.proxy, &self.daemon)
    }
}

fn proxy_env(w: &World) -> (&'static str, String) {
    ("https_proxy", w.proxy.url(Some((PROXY_USER, PROXY_PASS))))
}

/// Through the proxy, as bash's Python goes: the row's family, its line,
/// its answer, and whether it is a urllib family (else curl's CONNECT).
fn through_the_proxy(
    verb: &'static str,
    args: &'static [&'static str],
    body: &'static str,
    python: bool,
    extra: &[(&'static str, String)],
    token: &str,
) {
    let w = World::new(verb, "httpsproxy", body, ProxyMode::Tunnel);
    let mut env = vec![proxy_env(&w)];
    env.extend(extra.iter().cloned());
    let c = w.case(
        "through https_proxy, Basic for the proxy",
        args,
        &format!("{HOST}/"),
        &env,
    );
    let [b, r] = w.both(&c);
    let target = format!("{HOST}:{}", w.port());
    let basic = tunnel::basic_line(PROXY_USER, PROXY_PASS);
    for run in [&b, &r] {
        if python {
            tunnel::assert_tunnel_only_python(c.name, run, &target, Some(&basic), token);
        } else {
            tunnel::assert_tunnel_only(c.name, run, &target, Some(&basic), token);
        }
        assert!(!run.out.stderr.contains("plain http"), "{run:?}");
    }
    if python {
        tunnel::assert_same_python(c.name, &b, &r);
    } else {
        tunnel::assert_same(c.name, &b, &r);
    }
    assert_eq!(r.out.code, Some(0), "{verb}: {r:?}");
    tunnel::assert_unprinted(c.name, &[b, r], &[PROXY_PASS, token]);
}

#[test]
fn db_through_the_proxy() {
    through_the_proxy("db", &["status"], STATUS, true, &[], OWNER);
}

#[test]
fn store_through_the_proxy() {
    through_the_proxy("store", &["list"], COLLECTIONS, true, &[], OWNER);
}

/// The passport, not the owner token, rides the tunnel alone.
#[test]
fn store_through_the_proxy_with_a_passport() {
    let extra = [
        ("K2_HOOK_TOKEN", PASSPORT.to_string()),
        ("K2_SECRETS_DIR", "{secrets}".to_string()),
    ];
    through_the_proxy("store", &["list"], COLLECTIONS, true, &extra, PASSPORT);
}

#[test]
fn calendar_through_the_proxy() {
    through_the_proxy("calendar", &["list"], CALENDARS, true, &[], OWNER);
}

#[test]
fn project_through_the_proxy() {
    through_the_proxy("project", &["create", "Acme"], CREATED, true, &[], OWNER);
}

#[test]
fn workspace_through_the_proxy() {
    through_the_proxy("workspace", &["list"], WORKSPACES, false, &[], OWNER);
}

/// `no_proxy` as urllib reads it, at `K2_HOST=https://0.0.0.0`: the host
/// alone, among blank-padded tokens (curl versions read those apart), with
/// its port, or `*`: straight there on both sides, the proxy gets
/// nothing. With a `no_proxy` that names another host, through the proxy.
#[test]
fn db_no_proxy_as_urllib_reads_it() {
    let w = World::new("db", "noproxy", STATUS, ProxyMode::Tunnel);
    let port = w.port();
    let rows: [(&'static str, String, bool); 6] = [
        ("no_proxy=<host>", NETWORK.to_string(), false),
        (
            "NO_PROXY=<other>, <host>:<port>",
            format!("other.example, {NETWORK}:{port}"),
            false,
        ),
        ("no_proxy=*", "*".to_string(), false),
        ("no_proxy=.0.0", ".0.0".to_string(), false),
        (
            "no_proxy=<host>:<another port>",
            format!("{NETWORK}:1"),
            true,
        ),
        ("no_proxy=<other>", "other.example".to_string(), true),
    ];
    for (name, list, proxied) in rows {
        let var = if name.starts_with("NO_PROXY") {
            "NO_PROXY"
        } else {
            "no_proxy"
        };
        let c = w.case(name, &["status"], NETWORK, &[proxy_env(&w), (var, list)]);
        let [b, r] = w.both(&c);
        tunnel::assert_same_python(name, &b, &r);
        assert_eq!(r.out.code, Some(0), "{name}: {r:?}");
        if proxied {
            let target = format!("{NETWORK}:{port}");
            let basic = tunnel::basic_line(PROXY_USER, PROXY_PASS);
            for run in [&b, &r] {
                tunnel::assert_tunnel_only_python(name, run, &target, Some(&basic), OWNER);
            }
        } else {
            for run in [&b, &r] {
                assert!(run.proxy.is_empty(), "{name}: the proxy got {run:?}");
                assert_eq!(run.tls.len(), 1, "{name}: straight there: {run:?}");
            }
        }
    }
}

/// `all_proxy` alone: urllib has no handler for it, so an https request
/// goes straight there on both sides (curl would use it).
#[test]
fn db_all_proxy_alone_is_not_read() {
    let w = World::new("db", "allproxy", STATUS, ProxyMode::Tunnel);
    let c = w.case(
        "ALL_PROXY alone",
        &["status"],
        NETWORK,
        &[("ALL_PROXY", w.proxy.url(None))],
    );
    let [b, r] = w.both(&c);
    tunnel::assert_same_python(c.name, &b, &r);
    for run in [&b, &r] {
        assert!(run.proxy.is_empty(), "{run:?}");
        assert_eq!(run.tls.len(), 1, "{run:?}");
    }
    assert_eq!(r.out.code, Some(0), "{r:?}");
}

/// A proxy that refuses the tunnel (407): one CONNECT on each side, never
/// tried again (a calendar's loop doesn't try it again either), nothing
/// past it, the same lines.
#[test]
fn a_proxy_that_refuses_the_tunnel() {
    for (verb, args, body) in [
        ("db", &["status"][..], STATUS),
        ("calendar", &["list"][..], CALENDARS),
    ] {
        let w = World::new(verb, "refused", body, ProxyMode::Refuse);
        let c = w.case("the proxy says 407", args, HOST, &[proxy_env(&w)]);
        let [b, r] = w.both(&c);
        tunnel::assert_same_python(c.name, &b, &r);
        let target = format!("{HOST}:{}", w.port());
        for run in [&b, &r] {
            assert_eq!(run.proxy.len(), 1, "{verb}: one CONNECT: {run:?}");
            assert!(
                run.proxy[0].starts_with(&format!("CONNECT {target} HTTP/1.1\r\n")),
                "{run:?}"
            );
            assert!(!run.proxy[0].contains(OWNER), "{run:?}");
            assert!(run.tls.is_empty() && run.tls_errors.is_empty(), "{run:?}");
            assert_ne!(run.out.code, Some(0), "{verb}: {run:?}");
        }
        tunnel::assert_unprinted(c.name, &[b, r], &[OWNER, PROXY_PASS]);
    }
}

/// A proxy nothing listens on: Python's refused connect on both sides (a
/// calendar's loop tries it 4 times, as the shell's does), nothing sent.
#[test]
fn a_proxy_nothing_listens_on() {
    for (verb, args, body) in [
        ("db", &["status"][..], STATUS),
        ("calendar", &["list"][..], CALENDARS),
    ] {
        let w = World::new(verb, "proxydown", body, ProxyMode::Tunnel);
        let dead = format!("http://127.0.0.1:{}", dead_port());
        let c = w.case("the proxy is down", args, HOST, &[("https_proxy", dead)]);
        let [b, r] = w.both(&c);
        tunnel::assert_same_python(c.name, &b, &r);
        for run in [&b, &r] {
            assert!(run.proxy.is_empty() && run.tls.is_empty(), "{run:?}");
            assert_ne!(run.out.code, Some(0), "{verb}: {run:?}");
        }
        tunnel::assert_unprinted(c.name, &[b, r], &[OWNER]);
    }
}

/// The daemon's certificate checked inside the tunnel: with no trust in
/// the throwaway CA, both sides stop at the handshake and send no byte of
/// the request, the token nowhere. The exit is the same; the reason's
/// words are each side's own (python-tls-error-text).
#[test]
fn a_certificate_nothing_trusts_stops_the_request() {
    let w = World::new("db", "badcert", STATUS, ProxyMode::Tunnel);
    let mut c = w.case(
        "an untrusted certificate",
        &["status"],
        HOST,
        &[proxy_env(&w)],
    );
    // The system's roots alone: the throwaway CA is not among them.
    c.env.retain(|(k, _)| *k != "SSL_CERT_FILE");
    let [b, r] = w.both(&c);
    for run in [&b, &r] {
        assert_eq!(run.proxy.len(), 1, "the tunnel opened: {run:?}");
        assert!(run.tls.is_empty(), "a request got through: {run:?}");
        assert!(!run.tls_errors.is_empty(), "no handshake ended: {run:?}");
        assert_ne!(run.out.code, Some(0), "{run:?}");
    }
    assert_eq!(b.out.code, r.out.code, "{b:?} {r:?}");
    assert_eq!(b.proxy, r.proxy, "the CONNECT heads");
    println!(
        "named difference python-tls-error-text: {}: bash {:?}, the binary {:?}",
        c.name, b.out.stderr, r.out.stderr
    );
    tunnel::assert_unprinted(c.name, &[b, r], &[OWNER, PROXY_PASS]);
}
