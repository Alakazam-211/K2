//! Tunnel-ingress listener (PRD `prd-connect-login-edge-only-v1.md` §2).
//!
//! A SECOND loopback-only cleartext HTTP listener (`127.0.0.1:0`,
//! ephemeral) that runs the unmodified route dispatcher tagged
//! [`Ingress::Tunnel`](crate::routes::dispatcher::Ingress). Every byte that
//! arrives from the public K2 Connect tunnel lands HERE, never on the
//! main listener (which is `Ingress::Loopback`):
//!
//!   * **E2E on** — `tunnel_tls_listener::serve_one` splices the decrypted
//!     stream to this port for `Route::Daemon` (I2).
//!   * **E2E off** — the connector renders frpc's cleartext `localPort` as
//!     this port via the `k2_core::tunnel::ingress` seam (I3).
//!
//! The dispatcher then applies the tunnel gate (G1–G6): no HTML account
//! page, and `POST /cli/auth/login` only with a K2 edge attestation.
//!
//! ## Privilege (I5)
//!
//! Tunnel is the MOST restricted ingress. A local process that connects to
//! this port instead of the main one only *lowers* its privileges — it
//! gets everything a remote client gets and nothing more. There is no
//! escalation path through this listener, so its port is not a secret.
//!
//! ## Port record (I1)
//!
//! Mirrors `HTTPS_PORT` in `tunnel_tls_listener.rs`: a re-bindable
//! `Mutex<Option<u16>>`, NOT a `OnceLock`, so a dead accept loop (panic,
//! FD exhaustion) clears the record and [`ensure_port`] re-binds instead of
//! returning a dead port forever (the luzz silent-outage class).

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use k2_core::log_debug;
use tokio::net::{TcpListener, TcpStream};

use crate::routes::dispatcher::Ingress;

static PORT: OnceLock<Mutex<Option<u16>>> = OnceLock::new();
static STATE: OnceLock<Mutex<Option<crate::DaemonState>>> = OnceLock::new();
static SPAWN_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn port_slot() -> &'static Mutex<Option<u16>> {
    PORT.get_or_init(|| Mutex::new(None))
}

fn state_slot() -> &'static Mutex<Option<crate::DaemonState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

fn spawn_lock() -> &'static tokio::sync::Mutex<()> {
    SPAWN_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// The recorded live port, if the accept loop is (believed) up.
pub fn recorded_port() -> Option<u16> {
    *port_slot().lock().unwrap_or_else(|p| p.into_inner())
}

fn set_recorded_port(port: Option<u16>) {
    *port_slot().lock().unwrap_or_else(|p| p.into_inner()) = port;
}

/// Install the daemon state the process-wide listener dispatches with.
/// Called once at boot (before the tunnel can start). Tests that need
/// their own state use [`spawn`] directly and never touch this.
pub fn install_state(state: crate::DaemonState) {
    *state_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(state);
}

fn installed_state() -> Option<crate::DaemonState> {
    state_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// Quick liveness probe of a recorded loopback port.
async fn port_live(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    matches!(
        tokio::time::timeout(
            Duration::from_millis(200),
            TcpStream::connect(("127.0.0.1", port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// Bind a fresh `127.0.0.1:0` listener and run the accept loop for
/// `state`, dispatching every connection as [`Ingress::Tunnel`]. Returns
/// the bound port. Does NOT touch the process-wide record — that is
/// [`ensure_port`]'s job (production) so tests can spawn per-harness
/// listeners with their own state. (Reached from the lib's test harness;
/// the binary itself only goes through `ensure_port`.)
#[allow(dead_code)]
pub async fn spawn(state: crate::DaemonState) -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("bind tunnel-ingress listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("read tunnel-ingress listener addr: {e}"))?
        .port();
    tokio::spawn(accept_loop(listener, state));
    Ok(port)
}

async fn accept_loop(listener: TcpListener, state: crate::DaemonState) {
    loop {
        match listener.accept().await {
            Ok((stream, _peer)) => {
                let st = state.clone();
                tokio::spawn(async move {
                    crate::routes::dispatcher::dispatch(stream, st, Ingress::Tunnel).await;
                });
            }
            Err(e) => {
                let kind = e.kind();
                if matches!(
                    kind,
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                ) {
                    log_debug!("[daemon/tunnel-ingress] transient accept error: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                log_debug!("[daemon/tunnel-ingress] accept loop ending: {e}");
                return;
            }
        }
    }
}

/// Ensure the process-wide tunnel-ingress listener is up and return its
/// port — re-binding when the recorded port no longer accepts. Loud `Err`
/// when no state was installed (a bug: the daemon installs it at boot) or
/// the bind fails; callers must NOT fall back to the main listener.
pub async fn ensure_port() -> Result<u16, String> {
    if let Some(p) = recorded_port() {
        if port_live(p).await {
            return Ok(p);
        }
        log_debug!(
            "[daemon/tunnel-ingress] recorded port {p} is not accepting — re-binding \
             (listener death / silent external outage class)"
        );
    }
    let _g = spawn_lock().lock().await;
    if let Some(p) = recorded_port() {
        if port_live(p).await {
            return Ok(p);
        }
        set_recorded_port(None);
    }
    let state = installed_state().ok_or_else(|| {
        "tunnel-ingress listener has no daemon state installed (install_state not \
         called at boot) — refusing to route tunnel traffic to the privileged main \
         listener"
            .to_string()
    })?;
    let port = spawn_recorded(state).await?;
    log_debug!("[daemon/tunnel-ingress] tunnel-ingress listener up on 127.0.0.1:{port}");
    Ok(port)
}

/// [`spawn`] + record the port, clearing the record when the accept loop
/// ends (so [`ensure_port`] re-binds on the next call).
async fn spawn_recorded(state: crate::DaemonState) -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("bind tunnel-ingress listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("read tunnel-ingress listener addr: {e}"))?
        .port();
    set_recorded_port(Some(port));
    tokio::spawn(async move {
        struct ClearOnDrop(u16);
        impl Drop for ClearOnDrop {
            fn drop(&mut self) {
                if recorded_port() == Some(self.0) {
                    set_recorded_port(None);
                }
            }
        }
        let _guard = ClearOnDrop(port);
        accept_loop(listener, state).await;
    });
    Ok(port)
}

/// Blocking form of [`ensure_port`] for the k2-core connector seam (called
/// from a `spawn_blocking` worker at tunnel start, E2E off — I3).
fn ensure_port_blocking(handle: tokio::runtime::Handle) -> Result<u16, String> {
    handle.block_on(ensure_port())
}

/// Install the k2-core seam so the connector can resolve frpc's cleartext
/// `localPort` to this listener. Call once at boot inside the runtime.
pub fn register_ensure_hook() {
    let handle = tokio::runtime::Handle::current();
    k2_core::tunnel::ingress::register_ensure_tunnel_ingress(Box::new(move || {
        ensure_port_blocking(handle.clone())
    }));
}
