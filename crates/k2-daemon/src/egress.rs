//! The daemon's outbound HTTP gate (`prd-k2-vault-v1.md` KV18, VS5).
//!
//! One place that decides whether the daemon may open a connection to a
//! destination, and builds the client that does it:
//!
//! - **Air-gap.** Under air-gap ([`k2_core::airgap::enabled`]) only an
//!   allowlisted IP literal in a private/LAN range may be reached. Host
//!   names are refused before any DNS lookup, with code `airgap`.
//! - **Resolve once, vet, pin.** A host name is resolved once; every
//!   address is refused if it's loopback, private, link-local, metadata,
//!   CGNAT, multicast, reserved ([`k2_vault::hosts::is_blocked_ip`]) or one
//!   of this box's own interface addresses. The client is then pinned to
//!   the vetted addresses and has no other resolver, so a second lookup
//!   can't return something else. SNI and certificate checks still use the
//!   host name from the URL.
//! - **Client shape.** rustls, TLS ≥ 1.2, built-in roots, no option to
//!   accept invalid certificates; `*_PROXY` env ignored; no automatic
//!   redirects (callers decide); no automatic decompression (callers ask
//!   for `identity` and decode themselves); timeouts always set.
//!
//! Vault fetch is the first user. Other outbound clients move here later.
//! Errors carry only fixed text and the host, never a path or query.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use k2_vault::hosts::{canonical_ip, is_blocked_ip};

/// Who is asking. Sets the connect timeout and the log label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// `k2 vault fetch` (§9.2).
    VaultFetch,
    /// The vault's loopback proxy for LLM CLIs and SDKs (§9.3).
    VaultProxy,
}

impl Purpose {
    pub fn label(self) -> &'static str {
        match self {
            Purpose::VaultFetch => "vault-fetch",
            Purpose::VaultProxy => "vault-proxy",
        }
    }

    fn connect_timeout(self) -> Duration {
        match self {
            Purpose::VaultFetch | Purpose::VaultProxy => Duration::from_secs(10),
        }
    }
}

/// A refusal or failure from the gate. `hint` never holds a URL path,
/// query or header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressError {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
}

impl EgressError {
    fn new(code: &'static str, status: u16, hint: impl Into<String>) -> Self {
        Self { code, status, hint: hint.into() }
    }
}

impl std::fmt::Display for EgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.hint)
    }
}

impl std::error::Error for EgressError {}

impl From<EgressError> for k2_vault::VaultError {
    fn from(e: EgressError) -> Self {
        k2_vault::VaultError::new(e.code, e.status, e.hint)
    }
}

/// A destination that passed the gate. Built only by [`Destination::vet`]
/// (or the test constructor), so a client can't be made for an unvetted
/// address.
#[derive(Debug, Clone)]
pub struct Destination {
    /// The URL's host as reqwest sees it (lowercase A-label; IPv6 in
    /// brackets).
    host: String,
    kind: DestKind,
}

#[derive(Debug, Clone)]
enum DestKind {
    /// The URL names an IP; no lookup happens.
    IpLiteral(IpAddr),
    /// The URL names a host; the connection goes only to these addresses.
    Pinned(Vec<SocketAddr>),
}

fn host_only(url: &url::Url) -> String {
    url.host_str().unwrap_or("").to_string()
}

impl Destination {
    /// Run the gate for `url`.
    ///
    /// `ip_literal_allowlisted`: the caller's policy named this exact IP
    /// (for the vault, an exact-IP site pattern). Only then may an IP
    /// literal in a blocked range be reached, and only then is an IP
    /// literal allowed under air-gap.
    pub fn vet(url: &url::Url, ip_literal_allowlisted: bool) -> Result<Destination, EgressError> {
        let host = host_only(url);
        let port = url
            .port_or_known_default()
            .ok_or_else(|| EgressError::new("bad_host", 400, "URL has no port"))?;
        let literal = match url.host() {
            Some(url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
            Some(url::Host::Ipv6(v6)) => Some(canonical_ip(IpAddr::V6(v6))),
            Some(url::Host::Domain(_)) => None,
            None => return Err(EgressError::new("bad_host", 400, "URL has no host")),
        };

        // Air-gap first: a name under air-gap is refused before any lookup.
        check_airgap(literal, ip_literal_allowlisted, &host)?;

        if let Some(ip) = literal {
            if is_blocked_ip(ip) && !ip_literal_allowlisted {
                return Err(blocked(&host));
            }
            return Ok(Destination { host, kind: DestKind::IpLiteral(ip) });
        }

        let addrs: Vec<SocketAddr> = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|_| EgressError::new("upstream_error", 502, format!("couldn't resolve {host}")))?
            .collect();
        if addrs.is_empty() {
            return Err(EgressError::new("upstream_error", 502, format!("couldn't resolve {host}")));
        }
        let own = own_addresses();
        for a in &addrs {
            let ip = canonical_ip(a.ip());
            if is_blocked_ip(ip) || own.contains(&ip) {
                return Err(blocked(&host));
            }
        }
        Ok(Destination { host, kind: DestKind::Pinned(addrs) })
    }

    /// Test-only: a host name pinned to given addresses, skipping DNS and
    /// the range check (the fake upstream lives on loopback).
    #[cfg(test)]
    pub(crate) fn pinned_for_test(host: &str, addrs: Vec<SocketAddr>) -> Destination {
        Destination { host: host.to_string(), kind: DestKind::Pinned(addrs) }
    }

    // Unused in the binary's test build too (only the lib's tests may
    // read it), so the allow is unconditional.
    #[allow(dead_code)]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The addresses the connection may use (empty for an IP literal).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pinned(&self) -> &[SocketAddr] {
        match &self.kind {
            DestKind::Pinned(a) => a,
            DestKind::IpLiteral(_) => &[],
        }
    }

    fn literal(&self) -> Option<IpAddr> {
        match &self.kind {
            DestKind::IpLiteral(ip) => Some(*ip),
            DestKind::Pinned(_) => None,
        }
    }
}

fn blocked(host: &str) -> EgressError {
    EgressError::new(
        "address_not_allowed",
        403,
        format!("{host} points at a private or local address; only an exact IP site can reach those"),
    )
}

fn check_airgap(literal: Option<IpAddr>, ip_literal_allowlisted: bool, host: &str) -> Result<(), EgressError> {
    if !k2_core::airgap::enabled() {
        return Ok(());
    }
    match literal {
        Some(ip) if ip_literal_allowlisted && is_blocked_ip(ip) => Ok(()),
        _ => Err(EgressError::new(
            "airgap",
            403,
            format!("air-gap is on; {host} is outside this network (only exact LAN IP sites can be reached)"),
        )),
    }
}

/// Build the client for one vetted destination. Re-checks air-gap.
pub fn client(purpose: Purpose, dest: &Destination, timeout: Duration) -> Result<reqwest::blocking::Client, EgressError> {
    check_airgap(dest.literal(), dest.literal().is_some(), &dest.host)?;
    if timeout.is_zero() {
        return Err(EgressError::new("upstream_error", 504, format!("timed out talking to {}", dest.host)));
    }
    let mut b = reqwest::blocking::Client::builder()
        .use_rustls_tls()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .tls_built_in_root_certs(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .pool_max_idle_per_host(0)
        .connect_timeout(purpose.connect_timeout().min(timeout))
        .timeout(timeout)
        // No resolver except the pins below: a name that isn't pinned
        // fails instead of falling back to a fresh lookup.
        .dns_resolver(Arc::new(NoLookup));
    if let DestKind::Pinned(addrs) = &dest.kind {
        b = b.resolve_to_addrs(&dest.host, addrs);
    }
    #[cfg(test)]
    {
        for c in TEST_ROOTS.with(|r| r.borrow().clone()) {
            b = b.add_root_certificate(c);
        }
    }
    b.build()
        .map_err(|_| EgressError::new("egress_unavailable", 500, format!("couldn't set up a {} client", purpose.label())))
}

/// Re-run the air-gap check for a destination a cached client still
/// points at (air-gap may have turned on since it was vetted).
pub fn recheck(dest: &Destination) -> Result<(), EgressError> {
    check_airgap(dest.literal(), dest.literal().is_some(), &dest.host)
}

/// The async client for one vetted destination, for streamed responses
/// (the proxy). Same shape as [`client`]; instead of a total timeout it has
/// an idle timeout between reads (`read_idle`), so a long generation keeps
/// streaming while a stalled upstream still ends. Keeps a small pool to
/// the pinned addresses.
pub fn async_client(purpose: Purpose, dest: &Destination, read_idle: Duration) -> Result<reqwest::Client, EgressError> {
    check_airgap(dest.literal(), dest.literal().is_some(), &dest.host)?;
    if read_idle.is_zero() {
        return Err(EgressError::new("upstream_error", 504, format!("timed out talking to {}", dest.host)));
    }
    let mut b = reqwest::Client::builder()
        .use_rustls_tls()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .tls_built_in_root_certs(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .pool_max_idle_per_host(8)
        .pool_idle_timeout(Duration::from_secs(60))
        .connect_timeout(purpose.connect_timeout())
        .read_timeout(read_idle)
        .dns_resolver(Arc::new(NoLookup));
    if let DestKind::Pinned(addrs) = &dest.kind {
        b = b.resolve_to_addrs(&dest.host, addrs);
    }
    #[cfg(test)]
    {
        for c in TEST_ROOTS.with(|r| r.borrow().clone()) {
            b = b.add_root_certificate(c);
        }
    }
    b.build()
        .map_err(|_| EgressError::new("egress_unavailable", 500, format!("couldn't set up a {} client", purpose.label())))
}

/// The resolver behind the pins: refuses every name.
struct NoLookup;

impl reqwest::dns::Resolve for NoLookup {
    fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(std::future::ready(Err::<reqwest::dns::Addrs, _>("no lookup outside the egress pins".into())))
    }
}

/// This box's own interface addresses (VS5: a public name must not
/// resolve back to the box itself).
pub fn own_addresses() -> Vec<IpAddr> {
    #[cfg(unix)]
    {
        let mut out = Vec::new();
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs fills `head` with a list we free below; we
        // only read sockaddr fields whose family we checked.
        unsafe {
            if libc::getifaddrs(&mut head) != 0 {
                return out;
            }
            let mut cur = head;
            while !cur.is_null() {
                let sa = (*cur).ifa_addr;
                if !sa.is_null() {
                    match (*sa).sa_family as i32 {
                        libc::AF_INET => {
                            let sin = &*(sa as *const libc::sockaddr_in);
                            out.push(IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))));
                        }
                        libc::AF_INET6 => {
                            let sin6 = &*(sa as *const libc::sockaddr_in6);
                            out.push(canonical_ip(IpAddr::V6(std::net::Ipv6Addr::from(sin6.sin6_addr.s6_addr))));
                        }
                        _ => {}
                    }
                }
                cur = (*cur).ifa_next;
            }
            libc::freeifaddrs(head);
        }
        out
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

#[cfg(test)]
thread_local! {
    /// Extra trust roots for the TLS tests' local CA. Test builds only;
    /// production clients trust the built-in roots and nothing else.
    pub(crate) static TEST_ROOTS: std::cell::RefCell<Vec<reqwest::Certificate>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) mod test_util {
    //! A local CA, a TLS server, extra trust roots and the air-gap
    //! switch for tests.
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    pub(crate) struct Ca {
        pub cert: rcgen::Certificate,
        pub key: rcgen::KeyPair,
    }

    pub(crate) fn ca() -> Ca {
        let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).expect("ca params");
        p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        p.distinguished_name.push(rcgen::DnType::CommonName, "k2 egress test ca");
        p.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::CrlSign];
        let key = rcgen::KeyPair::generate().expect("ca key");
        let cert = p.self_signed(&key).expect("ca cert");
        Ca { cert, key }
    }

    pub(crate) fn leaf(ca: &Ca, names: &[&str]) -> (Vec<u8>, Vec<u8>) {
        let mut p = rcgen::CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .expect("leaf params");
        p.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        let key = rcgen::KeyPair::generate().expect("leaf key");
        let cert = p.signed_by(&key, &ca.cert, &ca.key).expect("leaf cert");
        (cert.der().to_vec(), key.serialize_der())
    }

    pub(crate) fn root(ca: &Ca) -> reqwest::Certificate {
        reqwest::Certificate::from_der(ca.cert.der()).expect("root der")
    }

    /// Serve `response` over TLS to each connection; returns the port and
    /// a receiver of the raw request heads.
    pub(crate) fn serve(
        cert_der: Vec<u8>,
        key_der: Vec<u8>,
        response: &'static [u8],
    ) -> (u16, std::sync::mpsc::Receiver<Vec<u8>>) {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let cfg = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .expect("versions")
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(cert_der)],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.into()),
            )
            .expect("server cert");
        let cfg = Arc::new(cfg);
        let l = TcpListener::bind("127.0.0.1:0").expect("bind tls");
        let port = l.local_addr().expect("addr").port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(tcp) = s else { return };
                let cfg = cfg.clone();
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let conn = match rustls::ServerConnection::new(cfg) {
                        Ok(c) => c,
                        Err(_) => return,
                    };
                    let mut tls = rustls::StreamOwned::new(conn, tcp);
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match tls.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = tx.send(head);
                    let _ = tls.write_all(response);
                    let _ = tls.flush();
                    tls.conn.send_close_notify();
                    let _ = tls.flush();
                });
            }
        });
        (port, rx)
    }

    pub(crate) struct Roots;
    impl Roots {
        pub(crate) fn set(c: reqwest::Certificate) -> Roots {
            super::TEST_ROOTS.with(|r| r.borrow_mut().push(c));
            Roots
        }
    }
    impl Drop for Roots {
        fn drop(&mut self) {
            super::TEST_ROOTS.with(|r| r.borrow_mut().clear());
        }
    }

    /// Holds HOME isolation (so the air-gap setting reads a temp file) and
    /// pins `K2_AIRGAP` for the test; restores it on drop.
    pub(crate) struct AirgapEnv {
        prev: Option<std::ffi::OsString>,
        _home: crate::test_support::TempHome,
    }
    impl AirgapEnv {
        pub(crate) fn set(on: bool) -> AirgapEnv {
            let home = crate::test_support::TempHome::new();
            let prev = std::env::var_os(k2_core::airgap::ENV_VAR);
            std::env::set_var(k2_core::airgap::ENV_VAR, if on { "1" } else { "0" });
            assert_eq!(k2_core::airgap::enabled(), on, "air-gap flag took effect");
            AirgapEnv { prev, _home: home }
        }
    }
    impl Drop for AirgapEnv {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(p) => std::env::set_var(k2_core::airgap::ENV_VAR, p),
                None => std::env::remove_var(k2_core::airgap::ENV_VAR),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::{AirgapEnv, Roots};
    use super::*;
    use std::time::Duration;

    const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

    fn u(s: &str) -> url::Url {
        url::Url::parse(s).expect("url")
    }

    #[test]
    fn blocked_literal_needs_an_exact_ip_allowance() {
        let _a = AirgapEnv::set(false);
        let e = Destination::vet(&u("http://127.0.0.1:9/"), false).expect_err("loopback literal");
        assert_eq!(e.code, "address_not_allowed");
        let e = Destination::vet(&u("http://169.254.169.254/latest"), false).expect_err("metadata literal");
        assert_eq!(e.code, "address_not_allowed");
        let d = Destination::vet(&u("http://127.0.0.1:9/"), true).expect("allowlisted literal");
        assert!(d.pinned().is_empty());
    }

    #[test]
    fn names_resolving_to_local_addresses_are_refused() {
        let _a = AirgapEnv::set(false);
        let e = Destination::vet(&u("http://localhost:9/"), true).expect_err("localhost");
        assert_eq!(e.code, "address_not_allowed");
        assert!(!e.hint.contains(":9"), "no port/path in hint: {}", e.hint);
    }

    #[test]
    fn own_interface_addresses_include_loopback() {
        let own = own_addresses();
        assert!(own.contains(&IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)), "{own:?}");
    }

    #[test]
    fn airgap_refuses_names_and_public_literals_before_lookup() {
        let _a = AirgapEnv::set(true);
        for s in ["https://api.stripe.com/v1/charges?key=zzz", "http://localhost:9/", "http://8.8.8.8/"] {
            let e = Destination::vet(&u(s), true).expect_err(s);
            assert_eq!(e.code, "airgap", "{s}");
            assert!(!e.hint.contains("zzz") && !e.hint.contains("/v1"), "{}", e.hint);
        }
        Destination::vet(&u("http://10.0.0.5:8080/x"), true).expect("allowlisted LAN literal");
        assert_eq!(Destination::vet(&u("http://10.0.0.5:8080/x"), false).expect_err("not allowlisted").code, "airgap");
        let pinned = Destination::pinned_for_test("example.test", vec!["127.0.0.1:9".parse().expect("sa")]);
        assert_eq!(client(Purpose::VaultFetch, &pinned, Duration::from_secs(5)).expect_err("name").code, "airgap");
    }

    #[test]
    fn tls_verifies_the_url_host_name_on_a_pinned_address() {
        let _a = AirgapEnv::set(false);
        let ca = test_util::ca();
        let (cert, key) = test_util::leaf(&ca, &["vault-egress.test"]);
        let (port, rx) = test_util::serve(cert, key, OK);
        let _roots = Roots::set(test_util::root(&ca));
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().expect("sa");

        let d = Destination::pinned_for_test("vault-egress.test", vec![addr]);
        let c = client(Purpose::VaultFetch, &d, Duration::from_secs(10)).expect("client");
        let r = c.get(format!("https://vault-egress.test:{port}/x")).send().expect("pinned https");
        assert_eq!(r.status().as_u16(), 200);
        let head = rx.recv_timeout(Duration::from_secs(5)).expect("server saw request");
        let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
        assert!(head.contains(&format!("host: vault-egress.test:{port}")), "{head}");

        // A name the cert doesn't cover fails verification on the same
        // address.
        let d = Destination::pinned_for_test("other-name.test", vec![addr]);
        let c = client(Purpose::VaultFetch, &d, Duration::from_secs(10)).expect("client");
        let e = c.get(format!("https://other-name.test:{port}/")).send().expect_err("wrong name");
        assert!(e.is_connect() || e.is_request(), "tls failure expected");

        // A name that isn't pinned has no resolver to fall back on.
        let d = Destination::pinned_for_test("vault-egress.test", vec![addr]);
        let c = client(Purpose::VaultFetch, &d, Duration::from_secs(10)).expect("client");
        assert!(c.get(format!("https://unpinned.test:{port}/")).send().is_err(), "unpinned name must fail");
    }

    #[test]
    fn tls_refuses_an_untrusted_certificate() {
        let _a = AirgapEnv::set(false);
        let ca = test_util::ca();
        let (cert, key) = test_util::leaf(&ca, &["vault-egress.test"]);
        let (port, _rx) = test_util::serve(cert, key, OK);
        // No extra root: the local CA isn't trusted.
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().expect("sa");
        let d = Destination::pinned_for_test("vault-egress.test", vec![addr]);
        let c = client(Purpose::VaultFetch, &d, Duration::from_secs(10)).expect("client");
        assert!(c.get(format!("https://vault-egress.test:{port}/")).send().is_err(), "untrusted cert must fail");
    }
}
