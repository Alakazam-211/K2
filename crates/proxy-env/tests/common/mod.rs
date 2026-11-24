//! Stub proxies on loopback for the tests: each records every byte of
//! each connection's handshake, then answers as its [`Mode`] says.
//!
//! - [`Mode::Http`]: an HTTP proxy: a request head, then this status.
//! - [`Mode::Https`]: the same inside TLS, with a certificate from a
//!   throwaway CA ([`Pki`]) for `localhost`, `127.0.0.1` and `::1`.
//! - [`Mode::Socks5`]: SOCKS5 (RFC 1928), with a user and password it
//!   requires (RFC 1929) or none, then this reply code.
//! - [`Mode::Socks4`]: SOCKS4 and SOCKS4a, then this reply code.
//! - [`Mode::Raw`]: reads whatever comes for a moment, then closes.
//! - [`Mode::Silent`]: accepts and never answers.
//!
//! After a handshake that opens the tunnel (a 2xx, reply 0, code 90), the
//! stub answers `ping` with `pong`.

#![allow(dead_code)]

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A throwaway CA and a certificate it signed for `localhost`,
/// `127.0.0.1` and `::1`.
pub struct Pki {
    pub ca: CertificateDer<'static>,
    pub ca_pem: String,
    leaf: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Pki {
    pub fn new() -> Pki {
        Pki::for_names(&["localhost", "127.0.0.1", "::1"])
    }

    /// A CA of its own and a certificate for `names` alone.
    pub fn for_names(names: &[&str]) -> Pki {
        use rcgen::{
            BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
            KeyUsagePurpose,
        };
        // Names of their own: a leaf whose subject is its issuer's reads
        // as self-signed to OpenSSL, and curl refuses it.
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Acme throwaway test CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_key = KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let mut leaf_params =
            CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
                .unwrap();
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, "localhost");
        leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        leaf_params.use_authority_key_identifier_extension = true;
        let leaf_key = KeyPair::generate().unwrap();
        let leaf = leaf_params.signed_by(&leaf_key, &ca, &ca_key).unwrap();
        Pki {
            ca_pem: ca.pem(),
            ca: ca.der().clone(),
            leaf: leaf.der().clone(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
        }
    }

    fn server(&self) -> Arc<rustls::ServerConfig> {
        Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![self.leaf.clone()], self.key.clone_key())
            .unwrap(),
        )
    }

    /// A client config that trusts this CA (or nothing, with `false`).
    pub fn client(&self, trust: bool) -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        if trust {
            roots.add(self.ca.clone()).unwrap();
        }
        Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
        )
    }
}

/// What a stub does.
#[derive(Clone)]
pub enum Mode {
    /// Reads a request head, answers `HTTP/1.1 <status>`.
    Http(&'static str),
    /// [`Mode::Http`] inside TLS.
    Https(&'static str, Arc<rustls::ServerConfig>),
    /// SOCKS5: the user and password it requires (none: no
    /// authentication), then this reply code.
    Socks5(Option<(Vec<u8>, Vec<u8>)>, u8),
    /// SOCKS4 and SOCKS4a, then this reply code (90 opens).
    Socks4(u8),
    /// Reads what comes for 300 ms, then closes.
    Raw,
    /// Accepts and never answers.
    Silent,
}

impl Mode {
    pub fn https(status: &'static str, pki: &Pki) -> Mode {
        Mode::Https(status, pki.server())
    }
}

/// One connection's bytes, as they came.
pub type Log = Arc<Mutex<Vec<Arc<Mutex<Vec<u8>>>>>>;

/// A stub proxy.
pub struct Stub {
    pub addr: SocketAddr,
    log: Log,
}

impl Stub {
    /// On 127.0.0.1, a free port.
    pub fn start(mode: Mode) -> Stub {
        Stub::bind("127.0.0.1:0", mode)
    }

    /// On `addr`; panics when it can't bind.
    pub fn bind(addr: &str, mode: Mode) -> Stub {
        let l = TcpListener::bind(addr).unwrap_or_else(|e| panic!("bind {addr}: {e}"));
        let addr = l.local_addr().unwrap();
        let log: Log = Arc::default();
        let keep = Arc::clone(&log);
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(c) = c else { continue };
                let entry: Arc<Mutex<Vec<u8>>> = Arc::default();
                keep.lock().unwrap().push(Arc::clone(&entry));
                let mode = mode.clone();
                std::thread::spawn(move || serve(c, mode, &entry));
            }
        });
        Stub { addr, log }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Each connection's bytes since the last take.
    pub fn take(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.log.lock().unwrap())
            .into_iter()
            .map(|e| e.lock().unwrap().clone())
            .collect()
    }
}

/// A port nothing listens on.
pub fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Rec<'a, S> {
    s: S,
    log: &'a Mutex<Vec<u8>>,
}

impl<S: Read + Write> Rec<'_, S> {
    fn byte(&mut self) -> Option<u8> {
        let mut b = [0u8; 1];
        match self.s.read(&mut b) {
            Ok(1) => {
                self.log.lock().unwrap().push(b[0]);
                Some(b[0])
            }
            _ => None,
        }
    }

    fn exact(&mut self, n: usize) -> Option<Vec<u8>> {
        (0..n).map(|_| self.byte()).collect()
    }

    fn until_nul(&mut self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            match self.byte()? {
                0 => return Some(out),
                b => out.push(b),
            }
        }
    }

    fn head(&mut self) -> Option<()> {
        let mut got = Vec::new();
        while !got.ends_with(b"\r\n\r\n") {
            got.push(self.byte()?);
        }
        Some(())
    }

    fn put(&mut self, bytes: &[u8]) {
        let _ = self.s.write_all(bytes).and_then(|()| self.s.flush());
    }

    /// The opened tunnel: `ping` in, `pong` out.
    fn pong(&mut self) {
        let mut b = [0u8; 4];
        if self.s.read_exact(&mut b).is_ok() && &b == b"ping" {
            self.put(b"pong");
        }
    }
}

fn serve(c: TcpStream, mode: Mode, log: &Mutex<Vec<u8>>) {
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    match mode {
        Mode::Http(status) => http(Rec { s: c, log }, status),
        Mode::Https(status, cfg) => {
            let Ok(conn) = rustls::ServerConnection::new(cfg) else {
                return;
            };
            http(
                Rec {
                    s: rustls::StreamOwned::new(conn, c),
                    log,
                },
                status,
            );
        }
        Mode::Socks5(auth, rep) => socks5(Rec { s: c, log }, auth.as_ref(), rep),
        Mode::Socks4(cd) => socks4(Rec { s: c, log }, cd),
        Mode::Raw => {
            c.set_read_timeout(Some(Duration::from_millis(300)))
                .unwrap();
            let mut r = Rec { s: c, log };
            while r.byte().is_some() {}
        }
        Mode::Silent => std::thread::sleep(Duration::from_secs(30)),
    }
}

fn http<S: Read + Write>(mut r: Rec<'_, S>, status: &str) {
    if r.head().is_none() {
        return;
    }
    if status.starts_with('2') {
        r.put(format!("HTTP/1.1 {status}\r\n\r\n").as_bytes());
        r.pong();
    } else {
        r.put(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        );
    }
}

fn socks5<S: Read + Write>(mut r: Rec<'_, S>, auth: Option<&(Vec<u8>, Vec<u8>)>, rep: u8) {
    let Some(h) = r.exact(2) else { return };
    let Some(methods) = r.exact(usize::from(h[1])) else {
        return;
    };
    let pick = match auth {
        Some(_) if methods.contains(&2) => 2,
        None if methods.contains(&0) => 0,
        _ => 0xff,
    };
    r.put(&[5, pick]);
    if pick == 0xff {
        return;
    }
    if pick == 2 {
        let Some(v) = r.exact(2) else { return };
        let Some(user) = r.exact(usize::from(v[1])) else {
            return;
        };
        let Some(plen) = r.exact(1) else { return };
        let Some(pass) = r.exact(usize::from(plen[0])) else {
            return;
        };
        let ok = auth.is_some_and(|(u, p)| *u == user && *p == pass);
        r.put(&[1, if ok { 0 } else { 1 }]);
        if !ok {
            return;
        }
    }
    let Some(req) = r.exact(4) else { return };
    let len = match req[3] {
        1 => 4,
        4 => 16,
        3 => match r.exact(1) {
            Some(l) => usize::from(l[0]),
            None => return,
        },
        _ => return,
    };
    if r.exact(len + 2).is_none() {
        return;
    }
    r.put(&[5, rep, 0, 1, 0, 0, 0, 0, 0, 0]);
    if rep == 0 {
        r.pong();
    }
}

fn socks4<S: Read + Write>(mut r: Rec<'_, S>, cd: u8) {
    let Some(h) = r.exact(8) else { return };
    if r.until_nul().is_none() {
        return;
    }
    // 0.0.0.x with x not 0: SOCKS4a, a name follows.
    if h[4..7] == [0, 0, 0] && h[7] != 0 && r.until_nul().is_none() {
        return;
    }
    r.put(&[0, cd, 0, 0, 0, 0, 0, 0]);
    if cd == 90 {
        r.pong();
    }
}

/// Bytes as text, with anything else escaped.
pub fn shown(b: &[u8]) -> String {
    b.iter()
        .flat_map(|&c| std::ascii::escape_default(c))
        .map(char::from)
        .collect()
}
