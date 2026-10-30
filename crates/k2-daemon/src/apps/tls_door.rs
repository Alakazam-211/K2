//! The App TLS door (0.46.6, `prd-app-custom-domains-v1` D1/D2; vs-live
//! CD-A7, CD-A8, CD-A9, CD-A10).
//!
//! One loopback TLS listener on the FIXED port
//! [`k2_core::front443::APP_DOOR_PORT`] that terminates custom App names
//! (`app.customer.com`) itself, with each name's own certificate from the
//! box store (`~/.k2/certs/<host>/`). front443 routes every routed custom
//! name here (`routes::Kind::App`, PROXY v2 on); the door reads that
//! header ([`crate::client_ip::read_tunnel_preamble`], loopback peers only:
//! the same trust as the E2E listener, CD-A9), terminates TLS, and hands
//! the stream to the one App path (`apps::router::serve_app_conn_with`:
//! client-IP rewrite, the pairing link files, the counted splice), exactly
//! as the K2 Connect E2E listener does for `*.<sub>.k2.dev`.
//!
//! - **Which names**: [`reload`] builds the table from `domain_names`:
//!   `app:` rows on an attached apex, for a runtime App that exists, in
//!   state `live` (served with their certificate) or `issuing` (routed so
//!   the CA's TLS-ALPN-01 hello reaches the door, but only a challenge
//!   hello is answered). Anything else has no certificate here: the
//!   handshake fails and the connection closes.
//! - **TLS-ALPN-01** (RFC 8737, CD-A10): a ClientHello offering
//!   `acme-tls/1` is answered ONLY with the challenge certificate the
//!   issuer registered for that name ([`Challenge`]); with none, the
//!   handshake fails. After a challenge handshake the connection closes.
//! - **ALPN**: `http/1.1` only for real traffic (the App path rewrites
//!   HTTP/1.1; WebSocket upgrades and long-lived streams pass through with
//!   no idle timeout, as on the E2E listener).
//! - **Hand-off** (CD-A7): the listener passes to a successor as
//!   `handoff::Role::AppDoor`; front443's table keeps `App` routes because
//!   their backend is fixed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use arc_swap::ArcSwap;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;

use k2_core::domains::{self, STATE_ISSUING, STATE_LIVE};
use k2_core::log_debug;

/// RFC 8737's ALPN protocol id.
pub const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";
/// One deadline for the PROXY header and the TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CAP_FLOOR: usize = 256;
const CAP_CEILING: usize = 16_384;

/// One routed custom name.
#[derive(Clone)]
pub struct DoorName {
    pub app_id: String,
    pub state: String,
    /// The served certificate (`live` names only; `None` = the store has
    /// no usable pair, so the name is routed but every handshake fails).
    pub cert: Option<Arc<CertifiedKey>>,
}

impl std::fmt::Debug for DoorName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoorName").field("app_id", &self.app_id).field("state", &self.state).field("cert", &self.cert.is_some()).finish()
    }
}

type Names = HashMap<String, DoorName>;

fn names() -> &'static ArcSwap<Names> {
    static N: OnceLock<ArcSwap<Names>> = OnceLock::new();
    N.get_or_init(|| ArcSwap::from_pointee(HashMap::new()))
}

fn challenges() -> &'static Mutex<HashMap<String, Arc<CertifiedKey>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<CertifiedKey>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
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
    /// Names routed to the door (live + issuing), sorted.
    pub routed: Vec<String>,
    /// Live names whose certificate is missing or unreadable (they are
    /// routed, but no handshake succeeds until the store has a pair).
    pub without_cert: Vec<String>,
}

/// Rebuild the door's name table from the daemon's state and swap it in.
/// Called by `front443::rebuild` (every 30 s, and on each bind, unbind and
/// state change) and by the issuer after a certificate lands.
pub fn reload() -> Reloaded {
    let rows = match k2_core::db::try_shared() {
        Some(db) => {
            let conn = db.lock();
            domains::app_domains(&conn).unwrap_or_else(|e| {
                log_debug!("[apps/door] read App domains: {e}");
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
                    log_debug!("[apps/door] {}: stored certificate unusable: {e}", d.hostname);
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
        out.routed.push(d.hostname.clone());
        next.insert(d.hostname.clone(), DoorName { app_id: d.app.clone(), state: d.state.clone(), cert });
    }
    out.routed.sort();
    names().store(Arc::new(next));
    out
}

fn runtime_app_exists(app: &str) -> bool {
    let Some(db) = k2_core::db::try_shared() else { return false };
    let conn = db.lock();
    matches!(k2_core::deploy::store::get_app(&conn, app), Ok(Some(_)))
}

/// The names front443 must route to the door (live + issuing), sorted.
pub fn routed_hosts() -> Vec<String> {
    let mut v: Vec<String> = names().load().keys().cloned().collect();
    v.sort();
    v
}

/// The App a LIVE custom name serves.
pub fn app_for_host(host: &str) -> Option<String> {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    names().load().get(&h).filter(|n| n.state == STATE_LIVE).map(|n| n.app_id.clone())
}

/// A TLS-ALPN-01 challenge certificate, served to `acme-tls/1` hellos for
/// its name while this guard lives.
pub struct Challenge {
    host: String,
}

impl Challenge {
    /// Register `key` for `host`'s challenge hellos (replacing any other).
    pub fn register(host: &str, key: Arc<CertifiedKey>) -> Challenge {
        let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
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
    challenges().lock().unwrap_or_else(|p| p.into_inner()).contains_key(&host.to_ascii_lowercase())
}

/// The door's certificate pick: a challenge hello gets the challenge
/// certificate (or nothing); any other hello gets the live name's own.
#[derive(Debug)]
struct Resolver;

impl ResolvesServerCert for Resolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let sni = hello.server_name()?.trim_end_matches('.').to_ascii_lowercase();
        let acme = hello.alpn().is_some_and(|mut a| a.any(|p| p == ACME_TLS_ALPN));
        if acme {
            return challenges().lock().unwrap_or_else(|p| p.into_inner()).get(&sni).cloned();
        }
        names().load().get(&sni).filter(|n| n.state == STATE_LIVE).and_then(|n| n.cert.clone())
    }
}

/// The door's rustls config (one per process; the resolver reads the live
/// tables on every handshake, so it never needs rebuilding).
pub fn server_config() -> Arc<ServerConfig> {
    static CFG: OnceLock<Arc<ServerConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let mut c = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("rustls default protocol versions")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Resolver));
        // http/1.1 for real traffic (the App path is an HTTP/1.1 rewrite,
        // never h2), acme-tls/1 for the CA's TLS-ALPN-01 hello.
        c.alpn_protocols = vec![b"http/1.1".to_vec(), ACME_TLS_ALPN.to_vec()];
        Arc::new(c)
    })
    .clone()
}

/// Serve one connection the door accepted.
pub async fn serve_conn(mut tcp: TcpStream, cfg: Arc<ServerConfig>) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
    let peer = tcp.peer_addr().map_err(|e| format!("peer address: {e}"))?;
    // A relay hop like the E2E listener: no Nagle on the visitor side.
    crate::tunnel_tls_listener::no_nagle(&tcp);
    // front443 writes PROXY v2 (Kind::App); only a loopback peer's header
    // is read (CD-A9: the same trust as the E2E listener).
    let client_ip = crate::client_ip::read_tunnel_preamble(&mut tcp, peer, deadline).await?;
    // G1 (0.46.6, PA-A3): the visitor's open connections, before the
    // handshake; no visitor (a local caller) is not counted. Released when
    // the connection ends.
    let Some(_visitor_slot) = crate::apps::guard::door_conn(client_ip) else {
        return Err(format!("{} is over the per-visitor connection cap (k2 firewall guard)", client_ip.map(|i| i.to_string()).unwrap_or_default()));
    };
    let mut tls = match tokio::time::timeout_at(deadline, TlsAcceptor::from(cfg).accept(tcp)).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Err(format!("TLS handshake failed: {e}")),
        Err(_) => return Err(format!("TLS handshake timed out after {HANDSHAKE_TIMEOUT:?}")),
    };
    let (alpn, sni) = {
        let (_, conn) = tls.get_ref();
        (conn.alpn_protocol().map(<[u8]>::to_vec), conn.server_name().map(str::to_string))
    };
    if alpn.as_deref() == Some(ACME_TLS_ALPN) {
        // RFC 8737 §3: a challenge handshake carries no application data.
        let _ = tls.shutdown().await;
        return Ok(());
    }
    let Some(app) = sni.as_deref().and_then(app_for_host) else {
        let _ = tls.shutdown().await;
        return Err(format!("no live App name {sni:?}"));
    };
    // Mode::Door: always rewritten (visitor headers, `X-Forwarded-Proto:
    // https`, G2), `Host` untouched, no redirect of K2's own.
    crate::apps::router::serve_app_conn_with(&mut tls, &app, crate::client_ip::Mode::Door(client_ip)).await;
    Ok(())
}

fn conn_cap() -> &'static Arc<Semaphore> {
    static S: OnceLock<Arc<Semaphore>> = OnceLock::new();
    S.get_or_init(|| Arc::new(Semaphore::new(k2_core::connection_cap(CAP_FLOOR, CAP_CEILING))))
}

/// Accept on `l` until the hand-off quiesces the daemon (or the socket
/// dies). Each connection lives on the splice runtime (it outlives a
/// hand-off) and holds a permit of the door's cap.
pub async fn accept_loop(l: Arc<TcpListener>) {
    loop {
        let accepted = tokio::select! {
            r = l.accept() => r,
            _ = crate::handoff::quiesced() => return,
        };
        let (tcp, peer) = match accepted {
            Ok(x) => x,
            Err(e) => {
                let fatal = matches!(e.kind(), std::io::ErrorKind::NotConnected | std::io::ErrorKind::BrokenPipe)
                    || matches!(e.raw_os_error(), Some(9) | Some(22));
                log_debug!("[apps/door] accept: {e}");
                if fatal {
                    return;
                }
                continue;
            }
        };
        let Ok(permit) = Arc::clone(conn_cap()).try_acquire_owned() else {
            log_debug!("[apps/door] connection cap reached — dropping {peer}");
            continue;
        };
        let Ok(std_tcp) = tcp.into_std() else { continue };
        let cfg = server_config();
        crate::handoff::splice_handle().spawn(async move {
            let _permit = permit;
            let _splice = crate::handoff::splice_guard();
            let Ok(tcp) = TcpStream::from_std(std_tcp) else { return };
            if let Err(e) = serve_conn(tcp, cfg).await {
                log_debug!("[apps/door] connection from {peer} ended: {e}");
            }
        });
    }
}

fn running() -> &'static Mutex<Option<tokio::task::JoinHandle<()>>> {
    static R: OnceLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

/// The door serves (its accept loop runs).
pub fn serving() -> bool {
    running().lock().unwrap_or_else(|p| p.into_inner()).as_ref().is_some_and(|h| !h.is_finished())
}

/// Start the door (idempotent): the socket a hand-off predecessor passed,
/// else a fresh bind on 127.0.0.1:[`k2_core::front443::APP_DOOR_PORT`].
/// Called when front443 starts serving. Never fails boot.
pub fn start() {
    let mut g = running().lock().unwrap_or_else(|p| p.into_inner());
    if g.as_ref().is_some_and(|h| !h.is_finished()) {
        return;
    }
    let std_l = match crate::handoff::take_inherited(crate::handoff::Role::AppDoor) {
        Some(l) => l,
        None => match std::net::TcpListener::bind(("127.0.0.1", k2_core::front443::APP_DOOR_PORT)) {
            Ok(l) => l,
            Err(e) => {
                log_debug!("[apps/door] bind 127.0.0.1:{}: {e} — custom App names are not served", k2_core::front443::APP_DOOR_PORT);
                return;
            }
        },
    };
    if let Err(e) = std_l.set_nonblocking(true) {
        log_debug!("[apps/door] set_nonblocking: {e}");
        return;
    }
    let l = match TcpListener::from_std(std_l) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            log_debug!("[apps/door] adopt socket: {e}");
            return;
        }
    };
    crate::handoff::register(crate::handoff::Role::AppDoor, &l);
    log_debug!("[apps/door] serving custom App names on 127.0.0.1:{}", k2_core::front443::APP_DOOR_PORT);
    *g = Some(tokio::spawn(accept_loop(l)));
}

#[cfg(test)]
#[path = "tls_door_tests.rs"]
mod tests;
