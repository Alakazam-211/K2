//! Daemon-side TLS material for K2 Connect end-to-end encryption
//! (PRD `k2-connect-e2e-encryption.md` §4 Option A, §6 cert broker).
//!
//! In Option A the **daemon** — not the relay — terminates TLS for the
//! user's `<sub>.k2.dev` subdomain, so the relay only ever carries
//! ciphertext. That requires three pieces, all owned here:
//!
//!   1. an ECDSA P-256 **keypair**, persisted 0600 at `~/.k2/tunnel-key.pem`
//!      and reused across boots (the private key NEVER leaves the daemon);
//!   2. a **CSR** for the SANs `<sub>.k2.dev` + `*.<sub>.k2.dev` (the
//!      per-user wildcard covers every nested Pro subdomain) — this is what
//!      the future control-plane ACME broker (§6) signs;
//!   3. for the **Phase-0 spike only**, a **self-signed cert** for those
//!      same SANs so the rustls listener works end-to-end *locally* before
//!      the broker exists.
//!
//! ## The broker seam
//!
//! [`load_or_provision_cert`] is the single seam the later broker task
//! swaps. Today it self-signs; tomorrow it will POST the CSR over the
//! authenticated control-plane channel and install the returned
//! broker-issued cert. **The rustls listener calls only
//! [`load_or_provision_cert`] / [`server_config`] — it never knows whether
//! the cert is self-signed or broker-issued**, so the swap touches this
//! file and nothing in `k2-daemon`.

use std::fs;
use std::path::{Path, PathBuf};

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};

/// Directory holding the tunnel TLS material (`~/.k2/`). Honors `$HOME`
/// so tests can redirect it to a tempdir (matches `config::config_dir`).
fn k2_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

/// Path to the persisted ECDSA P-256 private key (PEM, 0600).
pub fn key_path() -> PathBuf {
    k2_dir().join("tunnel-key.pem")
}

/// Path to the installed leaf cert (PEM). In the spike this is the
/// self-signed cert; the broker task will overwrite it with the
/// LE/GTS-issued chain — same path, same listener.
pub fn cert_path() -> PathBuf {
    k2_dir().join("tunnel-cert.pem")
}

/// Path to the file the daemon publishes its rustls HTTPS-listener port
/// into (`~/.k2/tunnel-https.port`). The tunnel connector reads this when
/// E2E is on so frpc's `localPort` targets the daemon's TLS listener
/// rather than its cleartext HTTP port. Mirrors `~/.k2/daemon.port`.
pub fn https_port_path() -> PathBuf {
    k2_dir().join("tunnel-https.port")
}

/// Publish the daemon's chosen HTTPS-listener port for the connector to
/// read. Written via tmp+rename so a reader never sees a half-written
/// value. Called by the daemon right after it binds the rustls listener.
pub fn publish_https_port(port: u16) -> Result<(), String> {
    let dir = k2_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let path = https_port_path();
    let tmp = dir.join(format!("tunnel-https.port.tmp.{}", std::process::id()));
    fs::write(&tmp, port.to_string().as_bytes())
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", path.display())
    })
}

/// Read the daemon's published HTTPS-listener port, if any. `None` when the
/// file is absent (E2E listener never started) or unparsable.
pub fn read_https_port() -> Option<u16> {
    let raw = fs::read_to_string(https_port_path()).ok()?;
    raw.trim().parse::<u16>().ok().filter(|p| *p != 0)
}

/// The two SANs every K2 Connect cert covers for a subdomain `sub`:
/// the apex `<sub>.k2.dev` and the per-user wildcard `*.<sub>.k2.dev`
/// (so one cert covers all of the user's nested Pro subdomains).
pub fn sans_for(subdomain: &str) -> Vec<String> {
    let sub = subdomain.trim();
    vec![
        format!("{sub}.{}", super::config::SUBDOMAIN_HOST),
        format!("*.{sub}.{}", super::config::SUBDOMAIN_HOST),
    ]
}

#[cfg(unix)]
fn restrict_mode(file: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 0600 {}: {e}", file.display()))
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) -> Result<(), String> {
    Ok(())
}

/// Load the persisted keypair, or generate + persist a fresh ECDSA P-256
/// one at [`key_path`] (0600) and return it. Reused across boots so the
/// daemon's identity — and thus the cert the broker signs — is stable.
///
/// Fails loud: an unreadable/malformed key file is an error (we never
/// silently regenerate over a key that might already be certified — that
/// would orphan a live cert).
pub fn load_or_generate_keypair() -> Result<KeyPair, String> {
    let path = key_path();
    if path.exists() {
        let pem = fs::read_to_string(&path)
            .map_err(|e| format!("read tunnel key {}: {e}", path.display()))?;
        return KeyPair::from_pem(&pem)
            .map_err(|e| format!("parse tunnel key {}: {e}", path.display()));
    }

    // Fresh keypair. rcgen's default algorithm for a plain `generate()` is
    // ECDSA P-256 / SHA-256 (PKCS_ECDSA_P256_SHA256) — exactly the curve
    // the PRD calls for; we pin it explicitly so a future rcgen default
    // change can't silently swap the algorithm.
    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|e| format!("generate ECDSA P-256 keypair: {e}"))?;

    let dir = k2_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    // tmp+rename so a crash mid-write never leaves a half key on disk.
    let tmp = dir.join(format!("tunnel-key.pem.tmp.{}", std::process::id()));
    fs::write(&tmp, key_pair.serialize_pem().as_bytes())
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp)?;
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", path.display())
    })?;
    restrict_mode(&path)?;
    Ok(key_pair)
}

/// Build a PEM CSR for `subdomain`'s SANs, signed by `key_pair`. This is
/// the artifact the control-plane ACME broker (§6) will sign; in the spike
/// it's also fed to [`self_sign`] so the listener works without a broker.
///
/// The CSR carries BOTH SANs (`<sub>.k2.dev` + `*.<sub>.k2.dev`); the CN
/// is set to the apex so older verifiers that still read CN see a sane
/// value. Fails loud on an empty subdomain — a CSR with no host is useless
/// and would mask a misconfigured tunnel.
pub fn build_csr_pem(subdomain: &str, key_pair: &KeyPair) -> Result<String, String> {
    let sub = subdomain.trim();
    if sub.is_empty() {
        return Err(
            "cannot build a tunnel CSR with an empty subdomain — \
             select/claim a subdomain first"
                .to_string(),
        );
    }
    let mut params = csr_params(sub)?;
    // serialize_request consumes the SAN/DN we set and signs with the key.
    let csr = params
        .serialize_request(key_pair)
        .map_err(|e| format!("serialize CSR: {e}"))?;
    csr.pem().map_err(|e| format!("encode CSR PEM: {e}"))
}

/// Shared `CertificateParams` for both the CSR and the spike self-signed
/// cert, so they carry the IDENTICAL SAN set (the broker signs exactly the
/// SANs the listener will serve).
fn csr_params(sub: &str) -> Result<CertificateParams, String> {
    let mut params = CertificateParams::default();
    let mut san_types = Vec::with_capacity(2);
    for name in sans_for(sub) {
        san_types.push(
            SanType::DnsName(
                name.clone()
                    .try_into()
                    .map_err(|e| format!("invalid DNS SAN {name:?}: {e}"))?,
            ),
        );
    }
    params.subject_alt_names = san_types;

    let mut dn = DistinguishedName::new();
    dn.push(
        DnType::CommonName,
        format!("{sub}.{}", super::config::SUBDOMAIN_HOST),
    );
    params.distinguished_name = dn;
    Ok(params)
}

/// **Spike-only.** Self-sign a leaf cert for `subdomain`'s SANs with
/// `key_pair`, returning the cert PEM. Lets the rustls listener stand up
/// end-to-end locally before the ACME broker exists. The broker task
/// replaces the call site in [`load_or_provision_cert`] — this stays as a
/// dev/test fallback.
pub fn self_sign(subdomain: &str, key_pair: &KeyPair) -> Result<String, String> {
    let sub = subdomain.trim();
    if sub.is_empty() {
        return Err("cannot self-sign a tunnel cert with an empty subdomain".to_string());
    }
    let params = csr_params(sub)?;
    let cert = params
        .self_signed(key_pair)
        .map_err(|e| format!("self-sign tunnel cert: {e}"))?;
    Ok(cert.pem())
}

/// **THE BROKER SEAM.** Return the cert+key PEM pair the rustls listener
/// should serve for `subdomain`, provisioning via the cert broker if needed.
///
/// ## The real path (default): broker-issued
///
/// 1. load the persisted key (or generate it);
/// 2. if `~/.k2/tunnel-cert.pem` already holds a broker-issued chain that's
///    NOT within the renewal window, reuse it;
/// 3. otherwise POST the CSR ([`build_csr_pem`]) to the control-plane
///    `/cert` broker ([`super::cert_broker::provision_via_broker`]) and
///    install the returned publicly-trusted chain.
///
/// A broker-unreachable / broker-error on this path is a **LOUD error** — we
/// NEVER silently fall back to cleartext or to a self-signed cert clients
/// won't trust (that would defeat E2E). The error is the broker's typed
/// failure, stringified.
///
/// ## The spike path (opt-in): self-signed
///
/// Only when `K2_E2E_SELF_SIGNED=1` (an explicit dev/spike escape hatch) do
/// we self-sign locally so the listener can stand up without a live broker.
/// This stays for local development; production is always broker-issued.
///
/// The listener only ever calls this fn + [`server_config`], so neither
/// branch is visible to `k2-daemon`. Returns `(cert_pem, key_pem)`.
pub fn load_or_provision_cert(subdomain: &str) -> Result<(String, String), String> {
    if super::cert_broker::self_signed_mode() {
        return load_or_provision_self_signed(subdomain);
    }
    load_or_provision_broker(subdomain)
}

/// Real path: prefer an installed broker-issued cert that's still fresh,
/// else (re)provision via the broker. See [`load_or_provision_cert`].
fn load_or_provision_broker(subdomain: &str) -> Result<(String, String), String> {
    let key_pair = load_or_generate_keypair()?;
    let key_pem = key_pair.serialize_pem();

    let cert_file = cert_path();
    if cert_file.exists() {
        let cert_pem = fs::read_to_string(&cert_file)
            .map_err(|e| format!("read tunnel cert {}: {e}", cert_file.display()))?;
        if cert_pem.trim().is_empty() {
            return Err(format!(
                "tunnel cert {} is empty — delete it to re-provision",
                cert_file.display()
            ));
        }
        // Reuse only if it's a real cert that isn't near expiry. A cert
        // inside the renewal window (or unparseable) falls through to a fresh
        // broker request so we never serve a cert that's about to die.
        if !super::cert_broker::cert_needs_renewal(
            &cert_pem,
            super::cert_broker::RENEWAL_WINDOW,
            super::cert_broker::now_unix(),
        ) {
            return Ok((cert_pem, key_pem));
        }
        crate::log_debug!(
            "[tunnel/tls] installed cert is within the renewal window — \
             requesting a fresh broker-issued chain"
        );
    }

    // No usable cert → the real issuance path. Fail loud on any broker error.
    let (cert_pem, key_pem) = super::cert_broker::provision_via_broker(subdomain)
        .map_err(|e| e.to_string())?;
    Ok((cert_pem, key_pem))
}

/// Spike path: reuse an existing self-signed cert or mint a fresh one.
/// Only reachable via `K2_E2E_SELF_SIGNED=1`. See [`load_or_provision_cert`].
fn load_or_provision_self_signed(subdomain: &str) -> Result<(String, String), String> {
    let key_pair = load_or_generate_keypair()?;
    let key_pem = key_pair.serialize_pem();

    let cert_file = cert_path();
    if cert_file.exists() {
        let cert_pem = fs::read_to_string(&cert_file)
            .map_err(|e| format!("read tunnel cert {}: {e}", cert_file.display()))?;
        if cert_pem.trim().is_empty() {
            return Err(format!(
                "tunnel cert {} is empty — delete it to re-provision",
                cert_file.display()
            ));
        }
        return Ok((cert_pem, key_pem));
    }

    let cert_pem = self_sign(subdomain, &key_pair)?;
    let dir = k2_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!("tunnel-cert.pem.tmp.{}", std::process::id()));
    fs::write(&tmp, cert_pem.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, &cert_file).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", cert_file.display())
    })?;
    Ok((cert_pem, key_pem))
}

/// Parse a cert+key PEM pair into a rustls [`ServerConfig`] ready for a
/// `TlsAcceptor`. Fails loud on an empty chain or unparsable key — a TLS
/// listener with no usable identity must NOT silently start.
///
/// Requires a rustls `CryptoProvider` to be installed in the process
/// (the daemon installs aws-lc-rs at boot, `main.rs:223`).
pub fn server_config(
    cert_pem: &str,
    key_pem: &str,
) -> Result<rustls::ServerConfig, String> {
    use rustls_pemfile::Item;

    // Certs (the leaf, plus any chain the broker returns).
    let mut cert_chain = Vec::new();
    for item in rustls_pemfile::read_all(&mut cert_pem.as_bytes()) {
        match item.map_err(|e| format!("parse cert PEM: {e}"))? {
            Item::X509Certificate(der) => cert_chain.push(der),
            _ => {}
        }
    }
    if cert_chain.is_empty() {
        return Err("no X.509 certificate found in tunnel cert PEM".to_string());
    }

    // Private key (PKCS#8 / SEC1 — rcgen emits PKCS#8 for ECDSA).
    let key_der = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .map_err(|e| format!("parse tunnel key PEM: {e}"))?
        .ok_or_else(|| "no private key found in tunnel key PEM".to_string())?;

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key_der)
        .map_err(|e| format!("build rustls ServerConfig: {e}"))?;

    // Advertise ALPN so passthrough clients negotiate HTTP/2 directly with THIS
    // daemon's TLS terminator. On the E2E path the relay only forwards ciphertext
    // and never sees ALPN, so if we don't offer h2 here, clients silently fall
    // back to HTTP/1.1. Offer h2 first, then http/1.1. (PRD §9 / red-team GAP 7.)
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn sans_cover_apex_and_wildcard() {
        let sans = sans_for("rosson");
        assert_eq!(sans, vec!["rosson.k2.dev", "*.rosson.k2.dev"]);
    }

    #[test]
    fn csr_carries_both_sans() {
        with_temp_home(|| {
            let kp = load_or_generate_keypair().expect("keypair");
            let pem = build_csr_pem("rosson", &kp).expect("csr");
            assert!(
                pem.contains("BEGIN CERTIFICATE REQUEST"),
                "must be a PEM CSR\n{pem}"
            );
            // Decode the PEM body to DER and assert BOTH SAN dNSNames are
            // encoded in it (the subjectAltName extension stores each name as
            // raw ASCII bytes). A CSR with only the apex would silently drop
            // every Pro subdomain, so both must be present. We search the DER
            // bytes rather than re-parsing to avoid pulling rcgen's
            // `x509-parser` feature in just for a test.
            let der = decode_pem_body(&pem);
            assert!(
                contains_ascii(&der, b"rosson.k2.dev"),
                "apex SAN `rosson.k2.dev` not encoded in CSR DER"
            );
            assert!(
                contains_ascii(&der, b"*.rosson.k2.dev"),
                "wildcard SAN `*.rosson.k2.dev` not encoded in CSR DER"
            );
        });
    }

    /// Strip the PEM armor + whitespace and base64-decode the body to DER.
    fn decode_pem_body(pem: &str) -> Vec<u8> {
        let b64: String = pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .flat_map(|l| l.chars())
            .filter(|c| !c.is_whitespace())
            .collect();
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .expect("decode CSR base64")
    }

    /// True when `needle`'s bytes appear contiguously in `haystack`.
    fn contains_ascii(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn empty_subdomain_csr_fails_loud() {
        with_temp_home(|| {
            let kp = load_or_generate_keypair().expect("keypair");
            let err = build_csr_pem("  ", &kp).expect_err("empty subdomain must fail");
            assert!(err.contains("empty subdomain"), "got: {err}");
        });
    }

    #[test]
    fn keypair_persists_and_is_reused() {
        with_temp_home(|| {
            let kp1 = load_or_generate_keypair().expect("first keypair");
            assert!(key_path().exists(), "key must be persisted");
            let kp2 = load_or_generate_keypair().expect("reload keypair");
            assert_eq!(
                kp1.serialize_pem(),
                kp2.serialize_pem(),
                "second load must reuse the SAME persisted key, not regenerate"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn persisted_key_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home(|| {
            load_or_generate_keypair().expect("keypair");
            let mode = std::fs::metadata(key_path())
                .expect("stat key")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "private key must be chmod 0600, got {mode:o}");
        });
    }

    /// Run `f` with the self-signed spike escape hatch engaged
    /// (`K2_E2E_SELF_SIGNED=1`). Must be called from inside `with_temp_home`
    /// (which holds `HOME_LOCK`) so the global env mutation is serialized.
    fn with_self_signed<T>(f: impl FnOnce() -> T) -> T {
        let prev = std::env::var_os("K2_E2E_SELF_SIGNED");
        std::env::set_var("K2_E2E_SELF_SIGNED", "1");
        let out = f();
        match prev {
            Some(p) => std::env::set_var("K2_E2E_SELF_SIGNED", p),
            None => std::env::remove_var("K2_E2E_SELF_SIGNED"),
        }
        out
    }

    #[test]
    fn self_signed_cert_loads_into_rustls_server_config() {
        // Ensure a crypto provider is installed (the daemon does this at
        // boot; tests must do it too or with_single_cert panics).
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        with_temp_home(|| {
            let (cert_pem, key_pem) = with_self_signed(|| load_or_provision_cert("rosson"))
                .expect("provision self-signed cert");
            assert!(
                cert_pem.contains("BEGIN CERTIFICATE"),
                "cert PEM malformed\n{cert_pem}"
            );
            let cfg = server_config(&cert_pem, &key_pem)
                .expect("self-signed cert+key must build a rustls ServerConfig");
            // The config MUST advertise ALPN h2 + http/1.1 so passthrough clients
            // negotiate HTTP/2 directly with the daemon (PRD §9 / red-team GAP 7).
            assert_eq!(
                cfg.alpn_protocols,
                vec![b"h2".to_vec(), b"http/1.1".to_vec()],
                "server_config must advertise ALPN h2 then http/1.1"
            );
        });
    }

    #[test]
    fn provision_reuses_existing_cert_file() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        with_temp_home(|| {
            with_self_signed(|| {
                let (cert1, _) = load_or_provision_cert("rosson").expect("first provision");
                assert!(cert_path().exists(), "cert must be persisted");
                let (cert2, _) = load_or_provision_cert("rosson").expect("second provision");
                assert_eq!(
                    cert1, cert2,
                    "second provision must reuse the persisted cert, not re-sign"
                );
            });
        });
    }

    /// The DEFAULT path (no `K2_E2E_SELF_SIGNED`) is the broker: with no
    /// broker reachable and no token, `load_or_provision_cert` must FAIL
    /// LOUD — never silently self-sign. This is the core anti-fallback
    /// guarantee at the seam.
    #[test]
    fn default_path_does_not_self_sign_when_broker_absent() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        with_temp_home(|| {
            // Ensure the spike hatch is OFF for this test.
            let prev = std::env::var_os("K2_E2E_SELF_SIGNED");
            std::env::remove_var("K2_E2E_SELF_SIGNED");
            // Point the broker at a closed port so the call fails fast.
            let prev_url = std::env::var_os(super::super::cert_broker::BROKER_URL_ENV);
            let dead = {
                use std::net::TcpListener;
                let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
                let p = l.local_addr().unwrap().port();
                drop(l);
                format!("http://127.0.0.1:{p}/cert")
            };
            std::env::set_var(super::super::cert_broker::BROKER_URL_ENV, &dead);
            super::super::config::save(&super::super::config::TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed config");

            let err = load_or_provision_cert("rosson")
                .expect_err("default path must NOT self-sign — broker is the real path");
            assert!(
                err.contains("cert broker"),
                "expected a broker error, got: {err}"
            );
            assert!(
                !cert_path().exists(),
                "a failed broker call must not leave a (self-signed) cert behind"
            );

            match prev {
                Some(p) => std::env::set_var("K2_E2E_SELF_SIGNED", p),
                None => std::env::remove_var("K2_E2E_SELF_SIGNED"),
            }
            match prev_url {
                Some(p) => std::env::set_var(super::super::cert_broker::BROKER_URL_ENV, p),
                None => std::env::remove_var(super::super::cert_broker::BROKER_URL_ENV),
            }
        });
    }

    #[test]
    fn server_config_rejects_empty_cert() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let err = server_config("", "")
            .expect_err("empty cert+key must not build a ServerConfig");
        assert!(
            err.contains("no X.509 certificate"),
            "expected loud empty-cert error, got: {err}"
        );
    }
}
