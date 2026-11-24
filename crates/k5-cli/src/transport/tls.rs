//! TLS for a server on another machine: `https://` in
//! the shell's `BASE_URL`, as curl makes the connection.
//!
//! rustls with the ring provider, over our own HTTP/1.1 client
//! ([`super::wire`]): no aws-lc-rs, no reqwest, no async runtime. The
//! roots are the OS's, as curl trusts them (rustls-native-certs, which
//! reads `SSL_CERT_FILE` and `SSL_CERT_DIR` first), and the certificates
//! in `CURL_CA_BUNDLE` as extra roots, as curl reads that variable. curl's
//! own config files (`~/.curlrc`, `CURL_HOME`) are NEVER read: one can
//! carry options that change where the token goes (the named difference
//! `curl-config-files`). One small piece every
//! family that reaches another machine shares: `talk` and `read` at
//! `ws@host`, a Connect host's tray upload, and wiki publish's Connect
//! fallback.
//!
//! What curl does, and this does too:
//!
//! - The handshake is part of the connect: `--connect-timeout` and
//!   `--max-time` bound the TCP connect and the handshake together.
//! - The certificate is always checked: its chain to a root, its dates
//!   and the host name. A certificate that fails is curl's exit 60.
//! - Any other handshake failure (a server that speaks no TLS, a reset,
//!   no protocol in common) is curl's exit 35.
//! - No root store at all (the OS has none and `SSL_CERT_FILE` names
//!   nothing usable), or a `CURL_CA_BUNDLE` with no certificate it can
//!   read, is curl's exit 77.
//!
//! None of these sent a byte of the request, so none may be applied.
//!
//! Where it differs: ALPN offers `http/1.1` only (curl offers `h2` too
//! when it is built with it), so the answer is always HTTP/1.1. A peer
//! that closes without TLS's `close_notify` ends the answer as the end of
//! the stream does in plain HTTP.

use super::wire::{curl, Failure};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::io;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// curl 35: the TLS handshake failed.
pub const SSL_CONNECT_ERROR: i32 = 35;
/// curl 60: the server's certificate failed the check.
pub const PEER_FAILED_VERIFICATION: i32 = 60;
/// curl 77: no root certificates could be read.
pub const SSL_CACERT_BADFILE: i32 = 77;

/// An open TLS connection over TCP.
pub type Stream = StreamOwned<ClientConnection, TcpStream>;

/// The config for these roots: ring, TLS 1.2 and 1.3, ALPN `http/1.1`,
/// no client certificate.
pub fn config(roots: RootCertStore) -> Result<Arc<ClientConfig>, String> {
    let mut cfg =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("TLS setup: {e}"))?
            .with_root_certificates(roots)
            .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

/// The OS's roots, plus `CURL_CA_BUNDLE`'s ([`roots_with`]).
pub fn native_roots() -> Result<RootCertStore, String> {
    let bundle = std::env::var_os("CURL_CA_BUNDLE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let found = rustls_native_certs::load_native_certs();
    let why = found.errors.first().map(ToString::to_string);
    roots_with(found.certs, why, bundle.as_deref())
}

/// `os` (the OS's roots; `os_error` the first error reading them), plus
/// the certificates in `bundle` (`CURL_CA_BUNDLE`) as extra roots.
/// Certificates that don't parse are skipped, as curl's bundle loading
/// skips them. A bundle that can't be read or holds no certificate is an
/// error (curl's 77), and so is no root at all.
pub fn roots_with(
    os: Vec<CertificateDer<'static>>,
    os_error: Option<String>,
    bundle: Option<&Path>,
) -> Result<RootCertStore, String> {
    let mut roots = RootCertStore::empty();
    let (added, _skipped) = roots.add_parsable_certificates(os);
    if let Some(path) = bundle {
        let extra = rustls_native_certs::load_certs_from_paths(Some(path), None);
        if let Some(e) = extra.errors.first() {
            return Err(format!("CURL_CA_BUNDLE {}: {e}", path.display()));
        }
        let (more, _skipped) = roots.add_parsable_certificates(extra.certs);
        if more == 0 {
            return Err(format!(
                "CURL_CA_BUNDLE {}: no certificate in it",
                path.display()
            ));
        }
        return Ok(roots);
    }
    if added == 0 {
        return Err(match os_error {
            Some(e) => format!("no root certificates: {e}"),
            None => "no root certificates".to_string(),
        });
    }
    Ok(roots)
}

/// The config every `https://` call uses, read once per process.
fn shared() -> Result<Arc<ClientConfig>, Failure> {
    static SHARED: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    SHARED
        .get_or_init(|| native_roots().and_then(config))
        .clone()
        .map_err(|why| Failure::Broken {
            curl: SSL_CACERT_BADFILE,
            why,
        })
}

/// The handshake on `tcp` for `host`, with the OS's roots, before
/// `limit`.
pub fn connect(tcp: TcpStream, host: &str, limit: Instant) -> Result<Stream, Failure> {
    connect_with(shared()?, tcp, host, limit)
}

/// [`connect`] with a given config (the tests' own roots).
pub fn connect_with(
    cfg: Arc<ClientConfig>,
    mut tcp: TcpStream,
    host: &str,
    limit: Instant,
) -> Result<Stream, Failure> {
    let name = ServerName::try_from(host.to_string()).map_err(|e| Failure::Broken {
        curl: SSL_CONNECT_ERROR,
        why: format!("{host} isn't a name a certificate can be checked against ({e})"),
    })?;
    let mut conn = ClientConnection::new(cfg, name).map_err(|e| Failure::Broken {
        curl: SSL_CONNECT_ERROR,
        why: plain_words(&e),
    })?;
    while conn.is_handshaking() {
        let budget = limit
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(Failure::ConnectTimedOut)?;
        set_timeouts(&tcp, budget).map_err(|e| Failure::Broken {
            curl: SSL_CONNECT_ERROR,
            why: format!("TLS: set a timeout: {e}"),
        })?;
        if let Err(e) = conn.complete_io(&mut tcp) {
            return Err(handshake_failure(&e));
        }
    }
    Ok(StreamOwned::new(conn, tcp))
}

fn set_timeouts(tcp: &TcpStream, d: Duration) -> io::Result<()> {
    tcp.set_read_timeout(Some(d))?;
    tcp.set_write_timeout(Some(d))
}

/// curl's code for a handshake that failed.
fn handshake_failure(e: &io::Error) -> Failure {
    if matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        return Failure::ConnectTimedOut;
    }
    let tls = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>());
    let code = match tls {
        Some(rustls::Error::InvalidCertificate(_)) => PEER_FAILED_VERIFICATION,
        _ => SSL_CONNECT_ERROR,
    };
    let why = match tls {
        Some(t) => plain_words(t),
        None if e.kind() == io::ErrorKind::UnexpectedEof => {
            "the server hung up before the secure connection was set up".to_string()
        }
        None => format!("the secure connection failed: {e}"),
    };
    Failure::Broken { curl: code, why }
}

/// What a person reads for a TLS error: short plain words, never rustls's
/// own text (much of it is Rust's debug form, `UnknownIssuer` and the
/// like). A certificate check that failed says which check; anything else
/// says the secure connection couldn't be set up, and why when the why
/// is clear.
fn plain_words(e: &rustls::Error) -> String {
    use rustls::Error as E;
    let words = match e {
        E::InvalidCertificate(c) => return certificate_words(c),
        E::NoCertificatesPresented => "the server sent no certificate",
        E::AlertReceived(_) => "the server refused the secure connection",
        E::PeerIncompatible(_) => {
            "the server and this computer have no TLS version or cipher in common"
        }
        E::InvalidMessage(_)
        | E::InappropriateMessage { .. }
        | E::InappropriateHandshakeMessage { .. } => {
            "the server didn't answer in TLS (is it plain http on this port?)"
        }
        E::PeerMisbehaved(_) | E::PeerSentOversizedRecord => {
            "the server broke the TLS rules during the handshake"
        }
        E::DecryptError => "the server's TLS messages couldn't be decrypted",
        E::FailedToGetCurrentTime => "this computer's clock couldn't be read",
        _ => "the secure connection couldn't be set up",
    };
    words.to_string()
}

/// [`plain_words`] for a certificate that failed the check.
fn certificate_words(c: &rustls::CertificateError) -> String {
    use rustls::CertificateError as C;
    let words = match c {
        C::UnknownIssuer => {
            "the server's certificate isn't from an authority this computer trusts. \
             If you trust a private CA, set SSL_CERT_FILE to its certificate"
        }
        C::Expired | C::ExpiredContext { .. } => "the server's certificate has expired",
        C::NotValidYet | C::NotValidYetContext { .. } => {
            "the server's certificate isn't valid yet. Check this computer's clock"
        }
        C::NotValidForName | C::NotValidForNameContext { .. } => {
            "the certificate is for a different name"
        }
        C::Revoked => "the server's certificate was revoked",
        C::InvalidPurpose | C::InvalidPurposeContext { .. } => {
            "the server's certificate isn't one for a server"
        }
        C::BadSignature => "the signature on the server's certificate is wrong",
        C::UnsupportedSignatureAlgorithmContext { .. }
        | C::UnsupportedSignatureAlgorithmForPublicKeyContext { .. } => {
            "the server's certificate has a signature this computer can't check"
        }
        C::BadEncoding => "the server's certificate couldn't be read",
        _ => "the server's certificate failed the check",
    };
    words.to_string()
}

/// Sets the read and write timeouts of the TCP under `s`.
pub fn stream_timeouts(s: &Stream, d: Duration) -> io::Result<()> {
    set_timeouts(&s.sock, d)
}

/// A read where a peer that closed without `close_notify` is the end of
/// the stream, as in plain HTTP.
pub fn read(s: &mut Stream, buf: &mut [u8]) -> io::Result<usize> {
    match io::Read::read(s, buf) {
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
        r => r,
    }
}

/// True for curl's codes that this module gives a failed handshake: no
/// byte of the request went out.
pub fn before_request(code: i32) -> bool {
    matches!(
        code,
        SSL_CONNECT_ERROR | PEER_FAILED_VERIFICATION | SSL_CACERT_BADFILE
    ) || code == curl::COULDNT_RESOLVE_HOST
        || code == super::proxy::COULDNT_RESOLVE_PROXY
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::wire::{self, Body, Conn, Method, Request};
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A CA named `ca`, and a server certificate for `localhost` and
    /// 127.0.0.1 signed by it. Two CAs with one name would make the other
    /// one's certificate a bad signature, not an unknown issuer.
    struct Pki {
        ca: CertificateDer<'static>,
        leaf: CertificateDer<'static>,
        key: PrivateKeyDer<'static>,
    }

    fn pki() -> Pki {
        pki_named("Acme test CA")
    }

    fn pki_named(ca: &str) -> Pki {
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.distinguished_name.push(DnType::CommonName, ca);
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let leaf_params =
            CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
        let leaf_key = KeyPair::generate().unwrap();
        let leaf = leaf_params.signed_by(&leaf_key, &ca, &ca_key).unwrap();
        Pki {
            ca: ca.der().clone(),
            leaf: leaf.der().clone(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
        }
    }

    fn roots(ca: &CertificateDer<'static>) -> RootCertStore {
        let mut r = RootCertStore::empty();
        r.add(ca.clone()).unwrap();
        r
    }

    /// One TLS server for one connection: reads a request head, answers
    /// `answer`. Returns its port and what it read.
    fn server(pki: &Pki, answer: &'static [u8]) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![pki.leaf.clone()], pki.key.clone_key())
        .unwrap();
        let cfg = Arc::new(cfg);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let conn = rustls::ServerConnection::new(cfg).unwrap();
            let mut s = StreamOwned::new(conn, tcp);
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => return got,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            s.write_all(answer).unwrap();
            s.conn.send_close_notify();
            s.flush().unwrap();
            got
        });
        (port, h)
    }

    fn limit() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    #[test]
    fn a_whole_exchange_over_tls_reads_the_answer() {
        let pki = pki();
        let (port, h) = server(&pki, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let s = connect_with(config(roots(&pki.ca)).unwrap(), tcp, "localhost", limit()).unwrap();
        let mut conn = Conn::Tls(Box::new(s));
        let req = Request {
            method: Method::Post,
            target: "/cli/terminal/read?id=a".into(),
            host: "localhost".into(),
            user_agent: "k5/test".into(),
            headers: vec![("Authorization".into(), "Bearer t".into())],
            body: Some(Body {
                bytes: b"x=1".to_vec(),
                curl_form_type: true,
            }),
        };
        let a = wire::exchange(&mut conn, &req, limit()).unwrap();
        assert_eq!((a.status, a.body.as_slice()), (200, &b"ok"[..]));
        let got = h.join().unwrap();
        let head = String::from_utf8(got).unwrap();
        assert!(
            head.starts_with("POST /cli/terminal/read?id=a HTTP/1.1\r\nHost: localhost\r\n"),
            "{head}"
        );
    }

    #[test]
    fn an_answer_to_the_end_of_the_stream_ends_at_close_notify() {
        let pki = pki();
        let (port, h) = server(&pki, b"HTTP/1.1 200 OK\r\n\r\nall of it");
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let s = connect_with(config(roots(&pki.ca)).unwrap(), tcp, "127.0.0.1", limit()).unwrap();
        let mut conn = Conn::Tls(Box::new(s));
        let req = Request {
            method: Method::Get,
            target: "/x".into(),
            host: "127.0.0.1".into(),
            user_agent: "k5/test".into(),
            headers: vec![],
            body: None,
        };
        let a = wire::exchange(&mut conn, &req, limit()).unwrap();
        assert_eq!(a.body, b"all of it");
        h.join().unwrap();
    }

    #[test]
    fn a_certificate_from_another_ca_is_curl_60_and_nothing_was_sent() {
        let pki = pki();
        let other = pki_named("Globex test CA");
        let (port, h) = server(&pki, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let Err(f) = connect_with(config(roots(&other.ca)).unwrap(), tcp, "localhost", limit())
        else {
            panic!("a certificate from another CA was taken");
        };
        assert_eq!(f.curl_code(), PEER_FAILED_VERIFICATION, "{f:?}");
        assert!(!f.sent(), "{f:?}");
        assert_eq!(f.to_string(), UNKNOWN_ISSUER);
        assert!(h.join().unwrap().is_empty());
    }

    #[test]
    fn a_name_the_certificate_does_not_hold_is_curl_60() {
        let pki = pki();
        let (port, h) = server(&pki, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let Err(f) = connect_with(config(roots(&pki.ca)).unwrap(), tcp, "box.example", limit())
        else {
            panic!("a name the certificate doesn't hold was taken");
        };
        assert_eq!(f.curl_code(), PEER_FAILED_VERIFICATION, "{f:?}");
        assert_eq!(f.to_string(), "the certificate is for a different name");
        h.join().unwrap();
    }

    #[test]
    fn a_server_that_speaks_plain_http_is_curl_35() {
        let pki = pki();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = tcp.read(&mut buf);
            let _ = tcp.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        });
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let Err(f) = connect_with(config(roots(&pki.ca)).unwrap(), tcp, "localhost", limit())
        else {
            panic!("plain HTTP was taken for TLS");
        };
        assert_eq!(f.curl_code(), SSL_CONNECT_ERROR, "{f:?}");
        assert!(!f.sent(), "{f:?}");
        assert_plain(&f.to_string());
        h.join().unwrap();
    }

    const UNKNOWN_ISSUER: &str = "the server's certificate isn't from an authority this \
        computer trusts. If you trust a private CA, set SSL_CERT_FILE to its certificate";

    /// Fails on text a person shouldn't see: Rust's debug form (braces,
    /// `::`, a `CamelCase` name such as `UnknownIssuer` or `Broken`) or
    /// curl's numeric code.
    fn assert_plain(text: &str) {
        assert!(!text.is_empty());
        for bad in ["{", "}", "::", "Broken", "curl", "why:"] {
            assert!(!text.contains(bad), "{bad:?} in {text:?}");
        }
        for word in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            let camel = word
                .as_bytes()
                .windows(2)
                .any(|p| p[0].is_ascii_lowercase() && p[1].is_ascii_uppercase());
            assert!(!camel, "{word:?} in {text:?}");
        }
    }

    #[test]
    fn each_certificate_check_has_its_own_plain_words() {
        use rustls::pki_types::UnixTime;
        use rustls::CertificateError as C;
        let t = UnixTime::since_unix_epoch(Duration::from_secs(1_700_000_000));
        let cases = [
            (C::UnknownIssuer, UNKNOWN_ISSUER),
            (C::Expired, "the server's certificate has expired"),
            (
                C::ExpiredContext {
                    time: t,
                    not_after: t,
                },
                "the server's certificate has expired",
            ),
            (
                C::NotValidForName,
                "the certificate is for a different name",
            ),
            (
                C::NotValidForNameContext {
                    expected: ServerName::try_from("acme.example").unwrap(),
                    presented: vec!["globex.example".into()],
                },
                "the certificate is for a different name",
            ),
            (
                C::NotValidYet,
                "the server's certificate isn't valid yet. Check this computer's clock",
            ),
            (C::Revoked, "the server's certificate was revoked"),
            (
                C::BadSignature,
                "the signature on the server's certificate is wrong",
            ),
            (
                C::InvalidPurpose,
                "the server's certificate isn't one for a server",
            ),
            (C::BadEncoding, "the server's certificate couldn't be read"),
            (
                C::UnknownRevocationStatus,
                "the server's certificate failed the check",
            ),
        ];
        for (c, want) in cases {
            let e = rustls::Error::InvalidCertificate(c.clone());
            assert_eq!(plain_words(&e), want, "{c:?}");
            assert_plain(want);
            let f = handshake_failure(&io::Error::new(io::ErrorKind::InvalidData, e));
            assert_eq!(f.curl_code(), PEER_FAILED_VERIFICATION, "{c:?}");
            assert_eq!(f.to_string(), want, "{c:?}");
        }
    }

    #[test]
    fn other_handshake_failures_are_plain_words_too() {
        use rustls::{AlertDescription, InvalidMessage, PeerIncompatible, PeerMisbehaved};
        let cases = [
            (
                rustls::Error::AlertReceived(AlertDescription::HandshakeFailure),
                "the server refused the secure connection",
            ),
            (
                rustls::Error::InvalidMessage(InvalidMessage::InvalidContentType),
                "the server didn't answer in TLS (is it plain http on this port?)",
            ),
            (
                rustls::Error::PeerIncompatible(PeerIncompatible::NoCipherSuitesInCommon),
                "the server and this computer have no TLS version or cipher in common",
            ),
            (
                rustls::Error::PeerMisbehaved(PeerMisbehaved::IncorrectBinder),
                "the server broke the TLS rules during the handshake",
            ),
            (
                rustls::Error::NoCertificatesPresented,
                "the server sent no certificate",
            ),
            (
                rustls::Error::General("x".into()),
                "the secure connection couldn't be set up",
            ),
        ];
        for (e, want) in cases {
            let f = handshake_failure(&io::Error::new(io::ErrorKind::InvalidData, e));
            assert_eq!(f.curl_code(), SSL_CONNECT_ERROR, "{f:?}");
            assert_eq!(f.to_string(), want);
            assert_plain(want);
        }
        let eof = handshake_failure(&io::Error::from(io::ErrorKind::UnexpectedEof));
        assert_eq!(
            eof.to_string(),
            "the server hung up before the secure connection was set up"
        );
        let reset = handshake_failure(&io::Error::from(io::ErrorKind::ConnectionReset));
        assert!(
            reset
                .to_string()
                .starts_with("the secure connection failed: "),
            "{reset}"
        );
        assert_plain(&reset.to_string());
    }

    #[test]
    fn every_failure_reads_as_plain_words() {
        let all = [
            Failure::Connect(io::Error::from(io::ErrorKind::ConnectionRefused)),
            Failure::ConnectTimedOut,
            Failure::NotSent(String::new()),
            Failure::NotSent("send: closed".into()),
            Failure::TimedOut,
            Failure::Empty,
            Failure::Broken {
                curl: PEER_FAILED_VERIFICATION,
                why: UNKNOWN_ISSUER.into(),
            },
            Failure::Tunnel("the proxy closed".into()),
        ];
        for f in &all {
            assert_plain(&f.to_string());
        }
        assert_eq!(all[6].to_string(), UNKNOWN_ISSUER);
    }

    fn pem_file(name: &str, ders: &[&CertificateDer<'static>]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("k5-tls-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bundle.pem");
        let mut text = String::new();
        for d in ders {
            let b64 = crate::transport::proxy::base64(d.as_ref());
            text.push_str("-----BEGIN CERTIFICATE-----\n");
            for line in b64.as_bytes().chunks(64) {
                text.push_str(std::str::from_utf8(line).unwrap());
                text.push('\n');
            }
            text.push_str("-----END CERTIFICATE-----\n");
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn curl_ca_bundle_adds_roots_to_the_os_ones() {
        let os = pki();
        let extra = pki();
        let path = pem_file("extra", &[&extra.ca]);
        let roots = roots_with(vec![os.ca.clone()], None, Some(&path)).unwrap();
        assert_eq!(roots.len(), 2, "the OS's root and the bundle's");
        // Both servers' certificates are taken.
        for p in [&os, &extra] {
            let (port, h) = server(p, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
            connect_with(config(roots.clone()).unwrap(), tcp, "localhost", limit()).unwrap();
            drop(h);
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_curl_ca_bundle_with_nothing_usable_is_curl_77() {
        let os = pki();
        let found = || vec![os.ca.clone()];
        let missing = std::env::temp_dir().join("k5-tls-no-such-bundle.pem");
        let Err(why) = roots_with(found(), None, Some(&missing)) else {
            panic!("a missing CURL_CA_BUNDLE was taken");
        };
        assert!(why.starts_with("CURL_CA_BUNDLE "), "{why}");
        let path = pem_file("empty", &[]);
        let Err(why) = roots_with(found(), None, Some(&path)) else {
            panic!("a CURL_CA_BUNDLE with no certificate was taken");
        };
        assert!(why.ends_with("no certificate in it"), "{why}");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        // Unset: the OS's roots alone; none at all is an error.
        assert_eq!(roots_with(found(), None, None).unwrap().len(), 1);
        assert!(roots_with(Vec::new(), None, None).is_err());
    }

    #[test]
    fn a_server_that_never_answers_the_hello_is_a_connect_timeout() {
        let pki = pki();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let held = listener.accept().unwrap();
        let soon = Instant::now() + Duration::from_millis(300);
        let Err(f) = connect_with(config(roots(&pki.ca)).unwrap(), tcp, "localhost", soon) else {
            panic!("a silent server finished a handshake");
        };
        assert!(matches!(f, Failure::ConnectTimedOut), "{f:?}");
        assert_eq!(f.curl_code(), curl::OPERATION_TIMEDOUT);
        drop(held);
    }
}
