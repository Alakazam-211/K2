//! The custom App names this box terminates TLS for, shared by both ways
//! in (0.46.8, `prd-custom-domains-path-b-0467-v1` PB-1; vs-live PB-A3).
//!
//! Path A (0.46.6): front443 passes `app.customer.com` to the App TLS door
//! (`apps::tls_door`). Path B (0.46.8): the name points at K2's relays and
//! arrives on the K2 Connect E2E listener (`tunnel_tls_listener`). Both
//! terminators read the same two tables here, so a name can move between
//! paths by DNS alone and keeps its certificate:
//!
//! - **the name table** ([`reload`]): `app:` rows on an attached apex, for
//!   a runtime App that exists, in state `live` (served with its stored
//!   certificate) or `issuing` (routed so the CA's TLS-ALPN-01 hello gets
//!   through; only a challenge hello is answered). Anything else has no
//!   certificate: the handshake fails.
//! - **the challenge registry** ([`Challenge`]): the TLS-ALPN-01
//!   certificate the issuer registered for a name, served only to a hello
//!   that offers `acme-tls/1`.
//!
//! [`reload`] is the one rebuild: front443's 30 s rebuild calls it (through
//! the door), the issuer calls it after a certificate lands, and on a box
//! without front443 (a desktop, a Linux box behind a router) the 30 s
//! [`spawn_refresh`] loop does. Each rebuild also hands the tunnel the
//! Path B names its custom proxy must carry (`k2_core::tunnel::custom_names`,
//! PB-2).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use arc_swap::ArcSwap;
use rustls::sign::CertifiedKey;

use k2_core::domains::{self, STATE_ISSUING, STATE_LIVE};
use k2_core::log_debug;

/// One routed custom name.
#[derive(Clone)]
pub struct CustomName {
    pub app_id: String,
    pub state: String,
    /// The served certificate (`live` names only; `None` = the store has
    /// no usable pair, so the name is routed but every handshake fails).
    pub cert: Option<Arc<CertifiedKey>>,
}

impl std::fmt::Debug for CustomName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomName")
            .field("app_id", &self.app_id)
            .field("state", &self.state)
            .field("cert", &self.cert.is_some())
            .finish()
    }
}

type Names = HashMap<String, CustomName>;

fn names() -> &'static ArcSwap<Names> {
    static N: OnceLock<ArcSwap<Names>> = OnceLock::new();
    N.get_or_init(|| ArcSwap::from_pointee(HashMap::new()))
}

fn challenges() -> &'static Mutex<HashMap<String, Arc<CertifiedKey>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<CertifiedKey>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn norm(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Parse a PEM chain + key into a rustls signing identity.
pub fn certified_key(chain_pem: &str, key_pem: &str) -> Result<Arc<CertifiedKey>, String> {
    let certs = rustls_pemfile::certs(&mut chain_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("certificate PEM: {e}"))?;
    if certs.is_empty() {
        return Err("no certificate in the chain PEM".into());
    }
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .map_err(|e| format!("key PEM: {e}"))?
        .ok_or("no private key in the key PEM")?;
    let signer = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key).map_err(|e| format!("private key: {e}"))?;
    Ok(Arc::new(CertifiedKey::new(certs, signer)))
}

/// What [`reload`] built.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reloaded {
    /// Names routed to a terminator (live + issuing), sorted.
    pub routed: Vec<String>,
    /// Live names whose certificate is missing or unreadable (they are
    /// routed, but no handshake succeeds until the store has a pair).
    pub without_cert: Vec<String>,
    /// The Path B names the tunnel's custom proxy carries (PB-2), sorted.
    pub tunnel: Vec<String>,
}

/// Rebuild the name table from the daemon's state, swap it in, and hand
/// the tunnel its Path B names.
pub fn reload() -> Reloaded {
    let rows = match k2_core::db::try_shared() {
        Some(db) => {
            let conn = db.lock();
            domains::app_domains(&conn).unwrap_or_else(|e| {
                log_debug!("[apps/custom-names] read App domains: {e}");
                Vec::new()
            })
        }
        None => Vec::new(),
    };
    let mut next: Names = HashMap::new();
    let mut out = Reloaded::default();
    for d in rows {
        if !d.apex_attached || !(d.state == STATE_LIVE || d.state == STATE_ISSUING) {
            continue;
        }
        if !runtime_app_exists(&d.app) {
            continue;
        }
        let cert = if d.state == STATE_LIVE {
            match crate::domains::store::load(&d.hostname).map(|p| certified_key(&p.chain_pem, &p.key_pem)) {
                Some(Ok(k)) => Some(k),
                Some(Err(e)) => {
                    log_debug!("[apps/custom-names] {}: stored certificate unusable: {e}", d.hostname);
                    out.without_cert.push(d.hostname.clone());
                    None
                }
                None => {
                    out.without_cert.push(d.hostname.clone());
                    None
                }
            }
        } else {
            None
        };
        if d.via_relay() {
            out.tunnel.push(d.hostname.clone());
        }
        out.routed.push(d.hostname.clone());
        next.insert(d.hostname.clone(), CustomName { app_id: d.app.clone(), state: d.state.clone(), cert });
    }
    out.routed.sort();
    out.tunnel.sort();
    names().store(Arc::new(next));
    if k2_core::tunnel::custom_names::set(out.tunnel.clone()) {
        log_debug!(
            "[apps/custom-names] the tunnel's custom names changed ({}): frpc restarts to carry them (a few seconds' blip)",
            if out.tunnel.is_empty() { "none".to_string() } else { out.tunnel.join(", ") }
        );
    }
    out
}

fn runtime_app_exists(app: &str) -> bool {
    let Some(db) = k2_core::db::try_shared() else { return false };
    let conn = db.lock();
    matches!(k2_core::deploy::store::get_app(&conn, app), Ok(Some(_)))
}

/// The names routed to a terminator (live + issuing), sorted.
pub fn routed_hosts() -> Vec<String> {
    let mut v: Vec<String> = names().load().keys().cloned().collect();
    v.sort();
    v
}

/// The App a LIVE custom name serves.
pub fn app_for_host(host: &str) -> Option<String> {
    names().load().get(&norm(host)).filter(|n| n.state == STATE_LIVE).map(|n| n.app_id.clone())
}

/// A LIVE custom name's served certificate (`None`: not live here, or the
/// store has no usable pair).
pub fn live_cert(host: &str) -> Option<Arc<CertifiedKey>> {
    names().load().get(&norm(host)).filter(|n| n.state == STATE_LIVE).and_then(|n| n.cert.clone())
}

/// The TLS-ALPN-01 certificate registered for `host`, if a challenge waits.
pub fn challenge_cert(host: &str) -> Option<Arc<CertifiedKey>> {
    challenges().lock().unwrap_or_else(|p| p.into_inner()).get(&norm(host)).cloned()
}

/// A TLS-ALPN-01 challenge certificate, served to `acme-tls/1` hellos for
/// its name while this guard lives (on both terminators).
pub struct Challenge {
    host: String,
}

impl Challenge {
    /// Register `key` for `host`'s challenge hellos (replacing any other).
    pub fn register(host: &str, key: Arc<CertifiedKey>) -> Challenge {
        let h = norm(host);
        challenges().lock().unwrap_or_else(|p| p.into_inner()).insert(h.clone(), key);
        Challenge { host: h }
    }
}

impl Drop for Challenge {
    fn drop(&mut self) {
        challenges().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.host);
    }
}

/// Is a TLS-ALPN-01 challenge waiting for `host`?
pub fn challenge_pending(host: &str) -> bool {
    challenges().lock().unwrap_or_else(|p| p.into_inner()).contains_key(&norm(host))
}

/// Test seam: put one name straight into the table (no DB row, no store).
#[cfg(test)]
pub(crate) fn install_for_test(host: &str, app_id: &str, state: &str, cert: Option<Arc<CertifiedKey>>) {
    let mut next: Names = (**names().load()).clone();
    next.insert(norm(host), CustomName { app_id: app_id.to_string(), state: state.to_string(), cert });
    names().store(Arc::new(next));
}

/// Test seam: take one name out of the table.
#[cfg(test)]
pub(crate) fn remove_for_test(host: &str) {
    let mut next: Names = (**names().load()).clone();
    next.remove(&norm(host));
    names().store(Arc::new(next));
}

/// How often [`spawn_refresh`] rebuilds the table.
pub const REFRESH: Duration = Duration::from_secs(30);

/// Boot: rebuild the table now and then every [`REFRESH`] on one detached
/// thread, so a box without front443 (whose 30 s rebuild never runs) still
/// serves its Path B names and picks up renewed certificates (PB-A3).
pub fn spawn_refresh() {
    if k2_core::db::try_shared().is_some() {
        reload();
    }
    let _ = std::thread::Builder::new().name("k2-custom-names".into()).spawn(|| loop {
        std::thread::sleep(REFRESH);
        if k2_core::db::try_shared().is_some() {
            reload();
        }
    });
}
