//! A real HTTPS request through the app's TLS stack, with no internet
//! (Kessel Steward R9, R10).
//!
//! graviola is installed the way the app's `main` does it (the real CPU
//! check), then the app's two reqwest lines talk to a local rustls server:
//! reqwest 0.12 (the app's own clients: daemon, updater, Connect, Linux
//! packages) and reqwest 0.13 (Tauri's updater plugin). The server's
//! certificate comes from a test-only CA in tests/fixtures/tls (localhost
//! and 127.0.0.1, valid to 2126; the CA's key was thrown away).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;

use k2_lib::tls_provider::{self, TlsChoice};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{NamedGroup, ProtocolVersion, SupportedCipherSuite};

const CA_PEM: &[u8] = include_bytes!("fixtures/tls/ca.pem");
const LEAF_PEM: &[u8] = include_bytes!("fixtures/tls/localhost.pem");
const LEAF_KEY_PEM: &[u8] = include_bytes!("fixtures/tls/localhost-key.pem");
const BODY: &str = "hello over graviola";

/// What the server saw on one connection.
#[derive(Debug)]
struct Seen {
    version: ProtocolVersion,
    group: NamedGroup,
    suite_is_graviolas: bool,
    request_line: String,
}

fn same_suite(a: &SupportedCipherSuite, b: &SupportedCipherSuite) -> bool {
    match (a, b) {
        (SupportedCipherSuite::Tls13(x), SupportedCipherSuite::Tls13(y)) => std::ptr::eq(*x, *y),
        (SupportedCipherSuite::Tls12(x), SupportedCipherSuite::Tls12(y)) => std::ptr::eq(*x, *y),
        _ => false,
    }
}

/// The process default provider is graviola's: the same cipher suite
/// statics, in the same order.
fn default_provider_is_graviola() -> bool {
    let default = CryptoProvider::get_default().expect("a process default provider is installed");
    let graviola = rustls_graviola::default_provider();
    default.cipher_suites.len() == graviola.cipher_suites.len()
        && default.cipher_suites.iter().zip(graviola.cipher_suites.iter()).all(|(a, b)| same_suite(a, b))
}

fn install_like_main() {
    assert_eq!(
        tls_provider::install(),
        TlsChoice::Graviola,
        "this machine's CPU must pass graviola::util::is_cpu_supported to run this test"
    );
    assert_eq!(tls_provider::active(), Some(TlsChoice::Graviola));
    assert!(default_provider_is_graviola(), "the process default provider must be graviola's");
}

/// One HTTPS connection on 127.0.0.1, served with graviola's provider.
fn serve_once() -> (u16, JoinHandle<Result<Seen, String>>) {
    let cert = CertificateDer::from_pem_slice(LEAF_PEM).expect("leaf certificate PEM");
    let key = PrivateKeyDer::from_pem_slice(LEAF_KEY_PEM).expect("leaf key PEM");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls_graviola::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server certificate and key");
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().map_err(|e| format!("accept: {e}"))?;
        let conn = rustls::ServerConnection::new(config).map_err(|e| format!("server connection: {e}"))?;
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = tls.read(&mut buf).map_err(|e| format!("read the request: {e}"))?;
            if n == 0 {
                return Err("the client closed before sending a whole request".to_string());
            }
            request.extend_from_slice(&buf[..n]);
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
            BODY.len()
        );
        tls.write_all(response.as_bytes()).map_err(|e| format!("write the response: {e}"))?;
        tls.conn.send_close_notify();
        tls.flush().map_err(|e| format!("flush: {e}"))?;
        let graviola = rustls_graviola::default_provider();
        let suite = tls.conn.negotiated_cipher_suite().expect("a negotiated suite");
        Ok(Seen {
            version: tls.conn.protocol_version().expect("a negotiated version"),
            group: tls.conn.negotiated_key_exchange_group().expect("a key exchange group").name(),
            suite_is_graviolas: graviola.cipher_suites.iter().any(|g| same_suite(g, &suite)),
            request_line: String::from_utf8_lossy(&request).lines().next().expect("a request line").to_string(),
        })
    });
    (port, handle)
}

fn assert_graviola_handshake(seen: &Seen) {
    assert_eq!(seen.version, ProtocolVersion::TLSv1_3, "{seen:?}");
    // X25519MLKEM768 first in graviola's list, and rustls' ring provider has
    // no ML-KEM: the client's side of this handshake ran on graviola.
    assert_eq!(seen.group, NamedGroup::X25519MLKEM768, "{seen:?}");
    assert!(seen.suite_is_graviolas, "{seen:?}");
}

#[test]
fn the_apps_reqwest_0_12_client_does_https_on_graviola() {
    install_like_main();
    let (port, server) = serve_once();
    let client = reqwest::blocking::Client::builder()
        .use_rustls_tls()
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(CA_PEM).expect("CA PEM"))
        .build()
        .expect("reqwest 0.12 client");
    let response = client.get(format!("https://127.0.0.1:{port}/updates")).send().expect("HTTPS GET");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.text().expect("body"), BODY);
    let seen = server.join().expect("server thread").expect("the server served the request");
    assert_eq!(seen.request_line, "GET /updates HTTP/1.1");
    assert_graviola_handshake(&seen);
}

#[test]
fn the_updaters_reqwest_0_13_client_does_https_on_graviola() {
    install_like_main();
    let (port, server) = serve_once();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    let body = runtime.block_on(async {
        let client = reqwest_0_13::Client::builder()
            .tls_certs_only([reqwest_0_13::Certificate::from_pem(CA_PEM).expect("CA PEM")])
            .build()
            .expect("reqwest 0.13 client");
        let response = client.get(format!("https://127.0.0.1:{port}/latest.json")).send().await.expect("HTTPS GET");
        assert_eq!(response.status(), reqwest_0_13::StatusCode::OK);
        response.text().await.expect("body")
    });
    assert_eq!(body, BODY);
    let seen = server.join().expect("server thread").expect("the server served the request");
    assert_eq!(seen.request_line, "GET /latest.json HTTP/1.1");
    assert_graviola_handshake(&seen);
}

#[test]
fn a_certificate_from_an_unknown_ca_is_refused() {
    install_like_main();
    let (port, server) = serve_once();
    // webpki roots only (the app's own clients): the test CA isn't in them.
    let client = reqwest::blocking::Client::builder().use_rustls_tls().build().expect("reqwest 0.12 client");
    let err = client.get(format!("https://127.0.0.1:{port}/")).send().expect_err("an untrusted certificate must fail");
    assert!(format!("{err:?}").contains("UnknownIssuer"), "{err:?}");
    let served = server.join().expect("server thread");
    assert!(served.is_err(), "the server must not have served a request: {served:?}");
}
