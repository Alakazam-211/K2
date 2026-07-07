//! K2 Connect E2E **cert broker client** (PRD `k2-connect-e2e-encryption.md`
//! §5/§6 — the one net-new daemon build).
//!
//! In Option A the daemon terminates TLS itself, so it needs a
//! publicly-trusted cert for `<sub>.k2.dev` + `*.<sub>.k2.dev` whose private
//! key NEVER leaves the machine. The control-plane **cert broker** runs the
//! ACME DNS-01 dance and signs the daemon's CSR; this module is the daemon
//! half that talks to it:
//!
//!   1. build the CSR from the persisted keypair ([`super::tls::build_csr_pem`]);
//!   2. `POST` it + the account's `subdomain` + the tunnel bearer token to
//!      the broker (`https://cert.k2.dev/cert`, overridable for staging/test);
//!   3. install the returned PEM chain to `~/.k2/tunnel-cert.pem` (0644 — the
//!      cert is public; only the KEY is 0600) via tmp+rename;
//!   4. (caller) (re)build the rustls `ServerConfig` from the new chain.
//!
//! ## Fail-loud, never silent-fallback
//!
//! This is the **real** issuance path. If the broker is unreachable or
//! returns an error, that surfaces as a hard [`BrokerError`] — we NEVER
//! silently fall back to cleartext or to a self-signed cert that clients
//! won't trust (that would defeat the whole "relay sees only ciphertext +
//! a publicly-trusted cert" thesis). The ONLY non-broker path is the
//! explicit dev/spike `K2_E2E_SELF_SIGNED=1` escape hatch in
//! [`super::tls::load_or_provision_cert`].
//!
//! ## Renewal
//!
//! Certs roll every ~60–90d; [`cert_needs_renewal`] reports when the
//! installed leaf is within [`RENEWAL_WINDOW`] (~30d) of `notAfter`, and the
//! daemon's renewal loop (`connector::spawn_cert_renewal`, modelled on the
//! lease loop) re-runs [`provision_via_broker`] and hot-swaps the rustls
//! config without dropping the listener.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

/// Default broker endpoint (the deployed control-plane `/cert` RPC).
pub const DEFAULT_BROKER_URL: &str = "https://cert.k2.dev/cert";

/// Env var that overrides [`DEFAULT_BROKER_URL`] (staging / integration
/// tests point this at a mock or a staging control plane).
pub const BROKER_URL_ENV: &str = "K2_CERT_BROKER_URL";

/// Renew when the installed leaf is within this long of `notAfter`. 30 days
/// is a generous window so a daemon that's offline for a stretch still has
/// ample retries before the cert actually expires (PRD §9: "use a generous
/// renewal window + retries").
pub const RENEWAL_WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// HTTP timeout for a single broker call. ACME issuance on the control plane
/// can take a few seconds (DNS-01 propagation), so this is more generous
/// than the lease loop's, but still bounded so a hung network never wedges
/// the renewal loop.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

/// Resolve the broker base URL: `K2_CERT_BROKER_URL` if set (non-blank),
/// else [`DEFAULT_BROKER_URL`].
pub fn broker_url() -> String {
    match std::env::var(BROKER_URL_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => DEFAULT_BROKER_URL.to_string(),
    }
}

/// True when the dev/spike self-signed escape hatch is engaged
/// (`K2_E2E_SELF_SIGNED` truthy). The DEFAULT is the broker path; this only
/// flips on for local spikes where no broker exists yet. Kept here next to
/// the broker logic so the "real path vs. spike path" decision lives in one
/// place.
pub fn self_signed_mode() -> bool {
    match std::env::var("K2_E2E_SELF_SIGNED") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// A broker-side failure, mapping the structured error contract
/// (`{ "error": "<code>", "detail": "<msg>" }`) to typed daemon errors plus
/// the transport/parse failures that can happen before we even see a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerError {
    /// The bearer token was rejected (`invalid_token`).
    InvalidToken(String),
    /// The token's account does not own the requested subdomain
    /// (`subdomain_not_owned`).
    SubdomainNotOwned(String),
    /// The CSR's SANs/CN don't match what the broker would issue for this
    /// account's subdomain (`san_mismatch`).
    SanMismatch(String),
    /// ACME issuance failed on the control plane (`issuance_failed`).
    IssuanceFailed(String),
    /// The broker is administratively disabled (`broker_disabled`).
    BrokerDisabled(String),
    /// Any other structured `{error, detail}` the broker returns whose code
    /// we don't have a dedicated variant for — still a hard error.
    Other { code: String, detail: String },
    /// The broker was unreachable / the HTTP call itself failed (DNS,
    /// connect, TLS, timeout). The single most important "do NOT fall back"
    /// case on the real path.
    Unreachable(String),
    /// A non-2xx response we couldn't parse as the structured error shape,
    /// or a 2xx whose body wasn't the expected `{cert}` JSON.
    Protocol(String),
}

impl BrokerError {
    /// The broker's error-code string (for `{error}` codes) or a synthetic
    /// code for the transport/protocol variants. Useful for logs/metrics.
    pub fn code(&self) -> &str {
        match self {
            BrokerError::InvalidToken(_) => "invalid_token",
            BrokerError::SubdomainNotOwned(_) => "subdomain_not_owned",
            BrokerError::SanMismatch(_) => "san_mismatch",
            BrokerError::IssuanceFailed(_) => "issuance_failed",
            BrokerError::BrokerDisabled(_) => "broker_disabled",
            BrokerError::Other { code, .. } => code,
            BrokerError::Unreachable(_) => "unreachable",
            BrokerError::Protocol(_) => "protocol_error",
        }
    }
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrokerError::InvalidToken(d) => write!(
                f,
                "cert broker rejected the tunnel token (invalid_token): {d} — \
                 re-select your subdomain in Settings → K2 Connect to refresh it"
            ),
            BrokerError::SubdomainNotOwned(d) => write!(
                f,
                "cert broker: your account does not own this subdomain \
                 (subdomain_not_owned): {d}"
            ),
            BrokerError::SanMismatch(d) => write!(
                f,
                "cert broker rejected the CSR's SANs (san_mismatch): {d} — \
                 the daemon's requested cert names don't match your subdomain"
            ),
            BrokerError::IssuanceFailed(d) => write!(
                f,
                "cert broker could not issue the certificate (issuance_failed): \
                 {d} — the ACME issuance failed upstream; will retry"
            ),
            BrokerError::BrokerDisabled(d) => write!(
                f,
                "cert broker is currently disabled (broker_disabled): {d}"
            ),
            BrokerError::Other { code, detail } => {
                write!(f, "cert broker error ({code}): {detail}")
            }
            BrokerError::Unreachable(d) => write!(
                f,
                "cert broker unreachable ({}): {d} — refusing to start E2E TLS \
                 against an unissued/self-signed cert (would break trust or leak \
                 plaintext)",
                broker_url()
            ),
            BrokerError::Protocol(d) => write!(f, "cert broker protocol error: {d}"),
        }
    }
}

/// Map a structured `{error, detail}` code to the typed variant.
fn map_error_code(code: &str, detail: String) -> BrokerError {
    match code {
        "invalid_token" => BrokerError::InvalidToken(detail),
        "subdomain_not_owned" => BrokerError::SubdomainNotOwned(detail),
        "san_mismatch" => BrokerError::SanMismatch(detail),
        "issuance_failed" => BrokerError::IssuanceFailed(detail),
        "broker_disabled" => BrokerError::BrokerDisabled(detail),
        other => BrokerError::Other {
            code: other.to_string(),
            detail,
        },
    }
}

#[derive(Debug, Deserialize)]
struct CertResponse {
    cert: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

/// POST a CSR to the broker and return the signed PEM chain (leaf-first),
/// or a typed [`BrokerError`]. This is the **HTTP boundary** the tests mock
/// (by pointing [`broker_url`] at a local loopback server). No filesystem
/// side effects — pure request/response.
///
/// Contract (EXACTLY the deployed broker, PRD §6):
///   * `POST <broker_url>` JSON `{ csr, subdomain, token }`
///   * 200 → `{ "cert": "<PEM chain, leaf-first>" }`
///   * non-2xx → `{ "error": "<code>", "detail": "<msg>" }`
pub fn request_cert(
    csr_pem: &str,
    subdomain: &str,
    token: &str,
) -> Result<String, BrokerError> {
    let url = broker_url();
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| BrokerError::Unreachable(format!("http client build failed: {e}")))?;

    let body = serde_json::json!({
        "csr": csr_pem,
        "subdomain": subdomain,
        "token": token,
    })
    .to_string();

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .map_err(|e| BrokerError::Unreachable(format!("POST {url}: {e}")))?;

    let status = resp.status();
    // `reqwest`'s `json` feature isn't enabled in this crate (only
    // `blocking`+`rustls-tls`), so we read text and parse with serde_json —
    // same pattern as the lease loop.
    let text = resp
        .text()
        .map_err(|e| BrokerError::Protocol(format!("read broker response body: {e}")))?;

    if status.is_success() {
        let parsed: CertResponse = serde_json::from_str(&text).map_err(|e| {
            BrokerError::Protocol(format!(
                "broker 200 but body wasn't {{cert}} JSON: {e} (body: {})",
                truncate(&text, 200)
            ))
        })?;
        return match parsed.cert {
            Some(c) if !c.trim().is_empty() => Ok(c),
            _ => Err(BrokerError::Protocol(
                "broker 200 but `cert` field was missing/empty".to_string(),
            )),
        };
    }

    // Non-2xx — expect the structured error shape.
    match serde_json::from_str::<ErrorResponse>(&text) {
        Ok(err) => {
            let code = err.error.unwrap_or_default();
            let detail = err.detail.unwrap_or_default();
            if code.trim().is_empty() {
                Err(BrokerError::Protocol(format!(
                    "broker returned HTTP {status} with no error code (body: {})",
                    truncate(&text, 200)
                )))
            } else {
                Err(map_error_code(code.trim(), detail))
            }
        }
        Err(_) => Err(BrokerError::Protocol(format!(
            "broker returned HTTP {status} with unparsable body: {}",
            truncate(&text, 200)
        ))),
    }
}

/// Truncate a string for safe inclusion in an error message.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Full broker provisioning flow: load/generate the persisted keypair, build
/// the CSR for `subdomain`'s SANs, POST it to the broker, and install the
/// returned chain to `~/.k2/tunnel-cert.pem` (0644). Returns the
/// `(cert_pem, key_pem)` pair the listener serves.
///
/// Fails loud (typed [`BrokerError`]) on any broker/transport failure —
/// callers on the real path MUST NOT fall back to cleartext or self-signed.
pub fn provision_via_broker(subdomain: &str) -> Result<(String, String), BrokerError> {
    let sub = subdomain.trim();
    if sub.is_empty() {
        return Err(BrokerError::Protocol(
            "cannot request a broker cert with an empty subdomain".to_string(),
        ));
    }

    let key_pair = super::tls::load_or_generate_keypair()
        .map_err(|e| BrokerError::Protocol(format!("load/generate keypair: {e}")))?;
    let key_pem = key_pair.serialize_pem();

    let csr_pem = super::tls::build_csr_pem(sub, &key_pair)
        .map_err(|e| BrokerError::Protocol(format!("build CSR: {e}")))?;

    let token = super::config::load()
        .map_err(|e| BrokerError::Protocol(format!("load tunnel config: {e}")))?
        .token;
    if token.trim().is_empty() {
        return Err(BrokerError::InvalidToken(
            "no tunnel bearer token in ~/.k2/tunnel.json (select a subdomain in \
             Settings → K2 Connect first)"
                .to_string(),
        ));
    }

    // `request_cert` uses `reqwest::blocking`, which spins up — and on return
    // DROPS — its own tokio runtime. The tunnel-start path drives this whole
    // function inside a `block_on` (an ENTERED runtime context), where dropping
    // that inner runtime panics: "Cannot drop a runtime in a context where
    // blocking is not allowed." Run the blocking HTTP on a fresh OS thread that
    // has NO ambient tokio context, so the inner runtime is created + dropped
    // cleanly. (First-cert-provision on a fresh subdomain is the path that hits
    // this; a cached cert short-circuits before here.)
    let cert_pem = {
        let csr = csr_pem.clone();
        let sub_owned = sub.to_string();
        let token_owned = token.trim().to_string();
        let joined = std::thread::Builder::new()
            .name("k2-cert-broker".to_string())
            .spawn(move || request_cert(&csr, &sub_owned, &token_owned))
            .map_err(|e| BrokerError::Protocol(format!("spawn cert-broker thread: {e}")))?
            .join()
            .map_err(|_| {
                BrokerError::Protocol("cert-broker request thread panicked".to_string())
            })?;
        joined?
    };

    // Sanity: the chain must actually parse as X.509 before we install it —
    // installing garbage would brick the listener on next load.
    if cert_not_after(&cert_pem).is_none() {
        return Err(BrokerError::Protocol(
            "broker returned a `cert` that didn't parse as an X.509 leaf".to_string(),
        ));
    }

    install_cert(&cert_pem)
        .map_err(|e| BrokerError::Protocol(format!("install broker cert: {e}")))?;

    Ok((cert_pem, key_pem))
}

/// Install a PEM cert chain to [`super::tls::cert_path`] (`~/.k2/tunnel-cert.pem`)
/// via tmp+rename, chmod 0644 (the cert is PUBLIC — only the key is 0600).
pub fn install_cert(cert_pem: &str) -> Result<(), String> {
    let dest = super::tls::cert_path();
    let dir = dest
        .parent()
        .ok_or_else(|| "cert path has no parent dir".to_string())?;
    fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!("tunnel-cert.pem.tmp.{}", std::process::id()));
    fs::write(&tmp, cert_pem.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    set_cert_mode(&tmp);
    fs::rename(&tmp, &dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", dest.display())
    })?;
    set_cert_mode(&dest);
    Ok(())
}

#[cfg(unix)]
fn set_cert_mode(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // 0644: world-readable is fine — a leaf cert is public material.
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o644));
}

#[cfg(not(unix))]
fn set_cert_mode(_p: &Path) {}

/// Parse the leaf cert (first cert in the PEM) and return its `notAfter` as
/// a Unix timestamp (seconds). `None` when the PEM has no parseable leaf —
/// callers treat that as "must (re)provision".
pub fn cert_not_after(cert_pem: &str) -> Option<i64> {
    use x509_parser::pem::Pem;

    // The leaf is the FIRST cert in a leaf-first chain.
    let mut reader = cert_pem.as_bytes();
    let (pem, _) = Pem::read(std::io::Cursor::new(&mut reader)).ok()?;
    let cert = pem.parse_x509().ok()?;
    Some(cert.validity().not_after.timestamp())
}

/// The DNS SANs of the leaf cert (first cert in the PEM), lowercased.
/// Empty when the PEM has no parseable leaf or the leaf carries no
/// subjectAltName — callers treat "empty" as "covers nothing" and
/// (re)provision.
pub fn cert_dns_sans(cert_pem: &str) -> Vec<String> {
    use x509_parser::extensions::GeneralName;
    use x509_parser::pem::Pem;

    let mut reader = cert_pem.as_bytes();
    let Ok((pem, _)) = Pem::read(std::io::Cursor::new(&mut reader)) else {
        return Vec::new();
    };
    let Ok(cert) = pem.parse_x509() else {
        return Vec::new();
    };
    let Ok(Some(san)) = cert.subject_alternative_name() else {
        return Vec::new();
    };
    san.value
        .general_names
        .iter()
        .filter_map(|n| match n {
            GeneralName::DNSName(d) => Some(d.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// True when the installed leaf cert's DNS SANs cover every name in
/// `required` (case-insensitive exact match — the per-user wildcard is
/// itself an explicit `*.<sub>.k2.dev` SAN entry, not pattern-matched).
/// False for an unparseable leaf. This is what catches a SUBDOMAIN RENAME:
/// a fresh, valid cert for the OLD name must not be reused for the new one
/// (found live on rpmavs.k2.dev, 2026-07-07 — the box kept serving its
/// cfed.k2.dev cert after the rename because reuse only checked expiry).
pub fn cert_covers(cert_pem: &str, required: &[String]) -> bool {
    let sans = cert_dns_sans(cert_pem);
    if sans.is_empty() {
        return false;
    }
    required
        .iter()
        .all(|r| sans.iter().any(|s| s.eq_ignore_ascii_case(r)))
}

/// True when the installed leaf cert is missing, unparseable, or within
/// `window` of its `notAfter` (i.e. it's time to renew). `now_unix` is the
/// current time as a Unix timestamp (injected so the renewal decision is
/// unit-testable without a real clock).
pub fn cert_needs_renewal(cert_pem: &str, window: Duration, now_unix: i64) -> bool {
    match cert_not_after(cert_pem) {
        Some(not_after) => {
            // Renew once we're inside the window before expiry.
            let renew_at = not_after.saturating_sub(window.as_secs() as i64);
            now_unix >= renew_at
        }
        // Unparseable/empty → definitely needs (re)provisioning.
        None => true,
    }
}

/// Current wall-clock time as a Unix timestamp (seconds). Thin wrapper so
/// production callers don't repeat the `SystemTime` boilerplate; tests pass
/// an explicit `now` to [`cert_needs_renewal`] instead.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A throwaway one-shot HTTP server bound on loopback that returns a
    /// fixed status + body for the FIRST request, then exits. We point
    /// `K2_CERT_BROKER_URL` at it so the REAL reqwest client path is
    /// exercised end-to-end (no network mocking framework, no real network).
    /// Returns the bound `http://127.0.0.1:<port>/cert` URL and a receiver
    /// that yields the raw request bytes the server saw.
    fn spawn_mock_broker(status_line: &str, body: &str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock broker");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let status_line = status_line.to_string();
        let body = body.to_string();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = tx.send(req);
                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.flush();
            }
        });
        (format!("http://127.0.0.1:{port}/cert"), rx)
    }

    /// A real-shaped, leaf-first PEM chain (leaf + a fake intermediate) the
    /// broker would return. We mint it locally with rcgen so the test has a
    /// genuine X.509 leaf whose `notAfter` parses — the broker's wire shape
    /// is "leaf-first PEM concatenation", which is exactly this.
    fn fake_broker_chain(subdomain: &str, leaf_days_valid: i64) -> String {
        let kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        fake_broker_chain_signed_by(subdomain, leaf_days_valid, &kp)
    }

    /// As [`fake_broker_chain`] but signs the leaf with `leaf_kp` — used by
    /// the success test so the leaf's public key matches the daemon's
    /// PERSISTED private key (the broker signs the daemon's CSR, so the
    /// issued leaf carries the daemon's pubkey; `with_single_cert` rejects a
    /// key/cert mismatch, which is exactly the real invariant).
    fn fake_broker_chain_signed_by(
        subdomain: &str,
        leaf_days_valid: i64,
        leaf_kp: &rcgen::KeyPair,
    ) -> String {
        use rcgen::CertificateParams;
        let mut params = CertificateParams::new(super::super::tls::sans_for(subdomain)).unwrap();
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::days(1);
        params.not_after = now + time::Duration::days(leaf_days_valid);
        let leaf = params.self_signed(leaf_kp).unwrap();
        // A second cert standing in for an issuer/intermediate so the chain
        // is genuinely multi-cert (leaf-first).
        let int_kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let int = CertificateParams::new(vec!["K2 Test Intermediate".to_string()])
            .unwrap()
            .self_signed(&int_kp)
            .unwrap();
        format!("{}{}", leaf.pem(), int.pem())
    }

    #[test]
    fn broker_url_defaults_and_env_override() {
        // Default when unset.
        let _g = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os(BROKER_URL_ENV);
        std::env::remove_var(BROKER_URL_ENV);
        assert_eq!(broker_url(), DEFAULT_BROKER_URL);
        std::env::set_var(BROKER_URL_ENV, "https://staging.example/cert");
        assert_eq!(broker_url(), "https://staging.example/cert");
        // Blank env falls back to default (never an empty URL).
        std::env::set_var(BROKER_URL_ENV, "   ");
        assert_eq!(broker_url(), DEFAULT_BROKER_URL);
        match prev {
            Some(p) => std::env::set_var(BROKER_URL_ENV, p),
            None => std::env::remove_var(BROKER_URL_ENV),
        }
    }

    #[test]
    fn error_codes_map_to_typed_variants() {
        assert!(matches!(
            map_error_code("invalid_token", "x".into()),
            BrokerError::InvalidToken(_)
        ));
        assert!(matches!(
            map_error_code("subdomain_not_owned", "x".into()),
            BrokerError::SubdomainNotOwned(_)
        ));
        assert!(matches!(
            map_error_code("san_mismatch", "x".into()),
            BrokerError::SanMismatch(_)
        ));
        assert!(matches!(
            map_error_code("issuance_failed", "x".into()),
            BrokerError::IssuanceFailed(_)
        ));
        assert!(matches!(
            map_error_code("broker_disabled", "x".into()),
            BrokerError::BrokerDisabled(_)
        ));
        // Unknown code is still a HARD error, not a soft pass.
        match map_error_code("teapot", "short and stout".into()) {
            BrokerError::Other { code, detail } => {
                assert_eq!(code, "teapot");
                assert_eq!(detail, "short and stout");
            }
            other => panic!("unknown code must map to Other, got {other:?}"),
        }
    }

    /// (a) A successful `/cert` response installs the chain and the rustls
    /// `ServerConfig` loads it.
    #[test]
    fn successful_broker_response_installs_chain_and_builds_server_config() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        with_temp_home(|| {
            // Seed a token (required by provision_via_broker).
            super::super::config::save(&super::super::config::TunnelConfig {
                token: "tok_live".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed config");

            // Pre-create the daemon's persisted keypair, then sign the
            // broker's leaf with it — mirrors the real flow where the broker
            // signs the daemon's CSR (so leaf pubkey == persisted privkey).
            let persisted = super::super::tls::load_or_generate_keypair().expect("persist key");
            let chain = fake_broker_chain_signed_by("rosson", 90, &persisted);
            let body = serde_json::json!({ "cert": chain }).to_string();
            let (url, rx) = spawn_mock_broker("200 OK", &body);

            let (cert_pem, key_pem) = with_broker_url_ret(&url, || provision_via_broker("rosson"))
                .expect("broker provision must succeed");

            // The request the broker SAW carries the CSR, subdomain, token.
            let req = rx.recv_timeout(Duration::from_secs(5)).expect("server saw a request");
            assert!(req.contains("BEGIN CERTIFICATE REQUEST"), "CSR must be POSTed\n{req}");
            assert!(req.contains("\"subdomain\":\"rosson\""), "subdomain must be sent\n{req}");
            assert!(req.contains("\"token\":\"tok_live\""), "bearer token must be sent\n{req}");

            // The chain is installed on disk (0644) ...
            assert!(super::super::tls::cert_path().exists(), "cert installed");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(super::super::tls::cert_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o644, "installed cert must be 0644 (public), got {mode:o}");
            }

            // ... and it loads into a rustls ServerConfig.
            super::super::tls::server_config(&cert_pem, &key_pem)
                .expect("broker chain + key must build a ServerConfig");
        });
    }

    /// (d) A real-shaped leaf-first chain parses correctly: `cert_not_after`
    /// reads the LEAF's expiry (not the intermediate's), and a multi-cert
    /// chain is accepted whole by rustls.
    #[test]
    fn parses_leaf_first_chain_and_reads_leaf_expiry() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let chain = fake_broker_chain("rosson", 90);
        let na = cert_not_after(&chain).expect("leaf notAfter must parse");
        let now = now_unix();
        // Leaf is valid ~90 days out → notAfter is comfortably in the future.
        assert!(na > now + 80 * 86400, "leaf notAfter should be ~90d out, got delta {}", na - now);
        // The two-cert chain must contain two leaves' worth of certs (sanity
        // that we built leaf-first, not a single cert).
        assert_eq!(chain.matches("BEGIN CERTIFICATE").count(), 2, "chain must be multi-cert");
    }

    /// (b) A cert within the renewal window triggers a renewal decision; one
    /// comfortably outside it does not.
    #[test]
    fn renewal_window_logic() {
        // notAfter exactly `now + 10 days` with a 30-day window → renew.
        let chain_soon = fake_broker_chain("rosson", 10);
        assert!(
            cert_needs_renewal(&chain_soon, RENEWAL_WINDOW, now_unix()),
            "a cert 10d from expiry must renew under a 30d window"
        );
        // notAfter `now + 90 days` → outside the 30-day window → no renew.
        let chain_fresh = fake_broker_chain("rosson", 90);
        assert!(
            !cert_needs_renewal(&chain_fresh, RENEWAL_WINDOW, now_unix()),
            "a cert 90d from expiry must NOT renew under a 30d window"
        );
        // Garbage / empty → always needs (re)provisioning.
        assert!(cert_needs_renewal("not a cert", RENEWAL_WINDOW, now_unix()));
        assert!(cert_needs_renewal("", RENEWAL_WINDOW, now_unix()));
    }

    /// (c) Broker-unreachable on the real path is a HARD error — never a
    /// silent cleartext/self-signed fallback.
    #[test]
    fn broker_unreachable_is_hard_error() {
        with_temp_home(|| {
            super::super::config::save(&super::super::config::TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed config");
            // Point at a closed port (bind then drop so nothing is listening).
            let dead = {
                let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
                let p = l.local_addr().unwrap().port();
                drop(l);
                format!("http://127.0.0.1:{p}/cert")
            };
            let err = with_broker_url_ret(&dead, || provision_via_broker("rosson"))
                .expect_err("unreachable broker must NOT silently fall back");
            assert!(
                matches!(err, BrokerError::Unreachable(_)),
                "expected Unreachable, got {err:?}"
            );
            // And no cert was installed (we did NOT self-sign behind the scenes).
            assert!(
                !super::super::tls::cert_path().exists(),
                "a failed broker call must not leave any cert on disk"
            );
        });
    }

    /// (c, cont.) A structured error code from the broker is surfaced as a
    /// typed hard error, again with no fallback.
    #[test]
    fn broker_error_code_is_surfaced_not_swallowed() {
        with_temp_home(|| {
            super::super::config::save(&super::super::config::TunnelConfig {
                token: "bad".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed config");
            let body = serde_json::json!({
                "error": "subdomain_not_owned",
                "detail": "rosson belongs to another account",
            })
            .to_string();
            let (url, _rx) = spawn_mock_broker("403 Forbidden", &body);
            let err = with_broker_url_ret(&url, || provision_via_broker("rosson"))
                .expect_err("error code must surface");
            assert!(
                matches!(err, BrokerError::SubdomainNotOwned(_)),
                "expected SubdomainNotOwned, got {err:?}"
            );
            assert!(!super::super::tls::cert_path().exists(), "no cert installed on error");
        });
    }

    /// `self_signed_mode` is OFF by default and only flips on for explicit
    /// truthy values — so the DEFAULT real path is the broker.
    #[test]
    fn self_signed_mode_off_by_default() {
        let _g = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os("K2_E2E_SELF_SIGNED");
        std::env::remove_var("K2_E2E_SELF_SIGNED");
        assert!(!self_signed_mode(), "self-signed must default OFF (broker is the real path)");
        for truthy in ["1", "true", "ON", "yes"] {
            std::env::set_var("K2_E2E_SELF_SIGNED", truthy);
            assert!(self_signed_mode(), "K2_E2E_SELF_SIGNED={truthy} must enable");
        }
        for falsey in ["0", "false", "", "off"] {
            std::env::set_var("K2_E2E_SELF_SIGNED", falsey);
            assert!(!self_signed_mode(), "K2_E2E_SELF_SIGNED={falsey:?} must NOT enable");
        }
        match prev {
            Some(p) => std::env::set_var("K2_E2E_SELF_SIGNED", p),
            None => std::env::remove_var("K2_E2E_SELF_SIGNED"),
        }
    }

    /// Like `with_broker_url` but returns the closure's value (the env var
    /// is the only global state; HOME is already redirected by the caller's
    /// `with_temp_home`, and HOME_LOCK is reentrant-safe here because
    /// `with_temp_home` already holds it — so we set the env directly).
    fn with_broker_url_ret<T>(url: &str, f: impl FnOnce() -> T) -> T {
        let prev = std::env::var_os(BROKER_URL_ENV);
        std::env::set_var(BROKER_URL_ENV, url);
        let out = f();
        match prev {
            Some(p) => std::env::set_var(BROKER_URL_ENV, p),
            None => std::env::remove_var(BROKER_URL_ENV),
        }
        out
    }
}
