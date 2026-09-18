//! Live TLS probe for custom-domain cert status (C9/C13).
//! Probe 443 always; 465 when role=mail. Includes `issuer`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub state: String,
    pub self_signed: bool,
    pub names: Vec<String>,
    pub expires_at: Option<i64>,
    pub issuer: Option<String>,
}

impl ProbeResult {
    pub fn missing(hostname: &str) -> Self {
        Self {
            state: "missing".into(),
            self_signed: false,
            names: vec![hostname.to_string()],
            expires_at: None,
            issuer: None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state,
            "selfSigned": self.self_signed,
            "names": self.names,
            "expiresAt": self.expires_at,
            "issuer": self.issuer,
        })
    }
}

/// Combine 443 and (optional) 465. Worst-of: missing < self-signed < expired < issued.
pub fn probe_hostname(hostname: &str, role_mail: bool) -> ProbeResult {
    let p443 = probe_port(hostname, 443);
    if !role_mail {
        return classify(hostname, p443, None);
    }
    let p465 = probe_port(hostname, 465);
    classify(hostname, p443, Some(p465))
}

enum Raw {
    Missing,
    Handshake { leaf_der: Vec<u8> },
}

fn classify(hostname: &str, a: Raw, b: Option<Raw>) -> ProbeResult {
    let parse = |r: Raw| -> ProbeResult {
        match r {
            Raw::Missing => ProbeResult::missing(hostname),
            Raw::Handshake { leaf_der } => match parse_leaf(&leaf_der) {
                Some((issuer, subject, not_after, sans)) => {
                    let names = if sans.is_empty() {
                        vec![hostname.to_string()]
                    } else {
                        sans
                    };
                    let now = chrono::Utc::now().timestamp();
                    let expired = not_after <= now;
                    let self_signed = looks_rcgen(&issuer, &subject);
                    let state = if self_signed {
                        "self-signed"
                    } else if expired {
                        "expired"
                    } else {
                        "issued"
                    };
                    ProbeResult {
                        state: state.into(),
                        self_signed,
                        names,
                        expires_at: Some(not_after),
                        issuer: Some(issuer),
                    }
                }
                None => ProbeResult {
                    state: "issued".into(),
                    self_signed: false,
                    names: vec![hostname.to_string()],
                    expires_at: None,
                    issuer: None,
                },
            },
        }
    };
    let ra = parse(a);
    let Some(rb_raw) = b else {
        return ra;
    };
    let rb = parse(rb_raw);
    // Prefer the worse of the two so mail 465 rcgen is not hidden by 443 issued.
    if rank(&ra) <= rank(&rb) {
        ra
    } else {
        rb
    }
}

fn rank(p: &ProbeResult) -> u8 {
    match p.state.as_str() {
        "missing" => 0,
        "self-signed" => 1,
        "expired" => 2,
        _ => 3,
    }
}

fn looks_rcgen(issuer: &str, subject: &str) -> bool {
    let hay = format!("{issuer} {subject}").to_ascii_lowercase();
    hay.contains("rcgen") || hay.contains("self signed") || hay.contains("self-signed")
}

fn parse_leaf(der: &[u8]) -> Option<(String, String, i64, Vec<String>)> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
    let issuer = cert.issuer().to_string();
    let subject = cert.subject().to_string();
    let not_after = cert.validity().not_after.timestamp();
    let mut sans = Vec::new();
    if let Ok(Some(ext)) = cert.subject_alternative_name() {
        for n in &ext.value.general_names {
            if let x509_parser::extensions::GeneralName::DNSName(d) = n {
                sans.push(d.to_string());
            }
        }
    }
    Some((issuer, subject, not_after, sans))
}

fn probe_port(host: &str, port: u16) -> Raw {
    if host.trim().is_empty() {
        return Raw::Missing;
    }
    let host = host.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("k2-cert-probe".into())
        .spawn(move || {
            let _ = tx.send(probe_inner(&host, port));
        });
    rx.recv_timeout(PROBE_TIMEOUT + Duration::from_millis(400))
        .unwrap_or(Raw::Missing)
}

fn probe_inner(host: &str, port: u16) -> Raw {
    use std::net::ToSocketAddrs;

    #[derive(Debug)]
    struct Capture {
        leaf: std::sync::Mutex<Option<Vec<u8>>>,
    }
    impl ServerCertVerifier for Capture {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            if let Ok(mut g) = self.leaf.lock() {
                *g = Some(end_entity.as_ref().to_vec());
            }
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    let addrs = match (host, port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(_) => return Raw::Missing,
    };
    let mut sock = None;
    for addr in addrs {
        if let Ok(s) = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
            sock = Some(s);
            break;
        }
    }
    let mut sock = match sock {
        Some(s) => s,
        None => return Raw::Missing,
    };
    let _ = sock.set_read_timeout(Some(PROBE_TIMEOUT));
    let _ = sock.set_write_timeout(Some(PROBE_TIMEOUT));
    let capture = Arc::new(Capture {
        leaf: std::sync::Mutex::new(None),
    });
    let Ok(builder) = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions() else {
        return Raw::Missing;
    };
    let config = builder
        .dangerous()
        .with_custom_certificate_verifier(capture.clone())
        .with_no_client_auth();
    let Ok(server_name) = ServerName::try_from(host.to_string()) else {
        return Raw::Missing;
    };
    let mut conn = match ClientConnection::new(Arc::new(config), server_name) {
        Ok(c) => c,
        Err(_) => return Raw::Missing,
    };
    let mut buf = [0u8; 4096];
    for _ in 0..32 {
        while conn.wants_write() {
            if conn.write_tls(&mut sock).is_err() {
                break;
            }
        }
        let _ = sock.flush();
        if !conn.wants_read() {
            break;
        }
        let n = match sock.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut cur = std::io::Cursor::new(&buf[..n]);
        let _ = conn.read_tls(&mut cur);
        let _ = conn.process_new_packets();
        if capture.leaf.lock().ok().and_then(|g| g.clone()).is_some() {
            break;
        }
    }
    match capture.leaf.lock().ok().and_then(|g| g.clone()) {
        Some(leaf) => Raw::Handshake { leaf_der: leaf },
        None => Raw::Missing,
    }
}

/// Status from installed PEM when live probe is missing (inventory).
pub fn from_pem_file(hostname: &str) -> Option<ProbeResult> {
    let installed = super::store::load(hostname)?;
    let der = pem_first_cert_der(&installed.chain_pem)?;
    match parse_leaf(&der) {
        Some((issuer, subject, not_after, sans)) => {
            let names = if sans.is_empty() {
                vec![hostname.to_string()]
            } else {
                sans
            };
            let now = chrono::Utc::now().timestamp();
            let self_signed = looks_rcgen(&issuer, &subject);
            let state = if self_signed {
                "self-signed"
            } else if not_after <= now {
                "expired"
            } else {
                "issued"
            };
            Some(ProbeResult {
                state: state.into(),
                self_signed,
                names,
                expires_at: Some(not_after),
                issuer: Some(issuer),
            })
        }
        None => None,
    }
}

fn pem_first_cert_der(pem: &str) -> Option<Vec<u8>> {
    let mut buf = std::io::Cursor::new(pem.as_bytes());
    let items = rustls_pemfile::certs(&mut buf)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    items.into_iter().next().map(|c| c.as_ref().to_vec())
}

pub fn cert_json_for(hostname: &str, role: &str) -> serde_json::Value {
    let mail = role == k2_core::domains::ROLE_MAIL;
    let live = probe_hostname(hostname, mail);
    if live.state != "missing" {
        return live.to_json();
    }
    if let Some(file) = from_pem_file(hostname) {
        return file.to_json();
    }
    ProbeResult::missing(hostname).to_json()
}
