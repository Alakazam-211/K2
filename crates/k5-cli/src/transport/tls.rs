//! TLS for a server on another machine (K2's ruling D5): `https://` in
//! the shell's `BASE_URL`, as curl makes the connection.
//!
//! rustls with the ring provider, over our own HTTP/1.1 client
//! ([`super::wire`]): no aws-lc-rs, no reqwest, no async runtime. The
//! roots are the OS's, as curl trusts them (rustls-native-certs, which
//! reads `SSL_CERT_FILE` and `SSL_CERT_DIR` first). One small piece every
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
//!   nothing usable) is curl's exit 77.
//!
//! None of these sent a byte of the request, so none may be applied.
//!
//! Where it differs: ALPN offers `http/1.1` only (curl offers `h2` too
//! when it is built with it), so the answer is always HTTP/1.1. A peer
//! that closes without TLS's `close_notify` ends the answer as the end of
//! the stream does in plain HTTP.

use super::wire::{curl, Failure};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::io;
use std::net::TcpStream;
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

/// The OS's roots. Certificates that don't parse are skipped, as curl's
/// bundle loading skips them; none at all is an error.
pub fn native_roots() -> Result<RootCertStore, String> {
    let found = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    let (added, _skipped) = roots.add_parsable_certificates(found.certs);
    if added == 0 {
        return Err(match found.errors.first() {
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
        why: format!("TLS: {host} is no server name: {e}"),
    })?;
    let mut conn = ClientConnection::new(cfg, name).map_err(|e| Failure::Broken {
        curl: SSL_CONNECT_ERROR,
        why: format!("TLS: {e}"),
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
    Failure::Broken {
        curl: code,
        why: format!("TLS handshake: {e}"),
    }
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
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A CA, and a server certificate for `localhost` and 127.0.0.1
    /// signed by it.
    struct Pki {
        ca: CertificateDer<'static>,
        leaf: CertificateDer<'static>,
        key: PrivateKeyDer<'static>,
    }

    fn pki() -> Pki {
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
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
        let other = self::pki();
        let (port, h) = server(&pki, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let Err(f) = connect_with(config(roots(&other.ca)).unwrap(), tcp, "localhost", limit())
        else {
            panic!("a certificate from another CA was taken");
        };
        assert_eq!(f.curl_code(), PEER_FAILED_VERIFICATION, "{f:?}");
        assert!(!f.sent(), "{f:?}");
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
        h.join().unwrap();
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
