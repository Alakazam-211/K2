//! Tunnel-ingress listener seam (PRD `prd-connect-login-edge-only-v1.md` I3).
//!
//! The daemon owns a SECOND loopback HTTP listener — the *tunnel-ingress*
//! listener (`k2-daemon/src/tunnel_ingress_listener.rs`) — that runs the
//! same dispatcher tagged `Ingress::Tunnel`. Every byte that arrives from
//! the public tunnel must land there, never on the privileged main
//! listener (which is `Ingress::Loopback`):
//!
//!   * E2E ON: the rustls listener splices decrypted bytes to it (I2).
//!   * E2E OFF: frpc's cleartext `localPort` IS that port (I3) — resolved
//!     through this seam by the (k2-core) connector at tunnel start.
//!
//! Same layering pattern as [`super::tls::register_ensure_https_listener`]:
//! the daemon registers a hook at boot; k2-core calls it without a
//! dependency inversion. The hook is idempotent (returns the live port,
//! re-binding if the listener died) — see the daemon module.
//!
//! **Fallback:** when NO hook is registered (a library/test context with
//! no daemon process — nothing is listening on `default_port` either way)
//! the connector keeps forwarding to `default_port` and logs it. Inside the
//! daemon the hook is always registered before the tunnel can start.

use std::sync::{Arc, RwLock};

/// Ensure the tunnel-ingress listener is up and return its loopback port.
pub type EnsureTunnelIngress = dyn Fn() -> Result<u16, String> + Send + Sync + 'static;

fn slot() -> &'static RwLock<Option<Arc<EnsureTunnelIngress>>> {
    static SLOT: std::sync::OnceLock<RwLock<Option<Arc<EnsureTunnelIngress>>>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Install (or replace) the daemon's ensure-hook. The daemon calls this
/// once at boot; tests may install a stub and [`clear_ensure_tunnel_ingress`]
/// it afterwards.
pub fn register_ensure_tunnel_ingress(hook: Box<EnsureTunnelIngress>) {
    *slot().write().unwrap_or_else(|p| p.into_inner()) = Some(Arc::from(hook));
}

/// Remove the installed hook (tests).
pub fn clear_ensure_tunnel_ingress() {
    *slot().write().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Whether a hook is installed.
pub fn hook_registered() -> bool {
    slot().read().unwrap_or_else(|p| p.into_inner()).is_some()
}

/// Resolve the port cleartext tunnel traffic must be forwarded to.
///
/// With a registered hook: its answer (loud `Err` on bind failure — the
/// connector must NOT fall back to the privileged main listener). Without
/// one: `default_port`, logged, per the module doc.
pub fn ensure_tunnel_ingress_port(default_port: u16) -> Result<u16, String> {
    let hook = slot().read().unwrap_or_else(|p| p.into_inner()).clone();
    match hook {
        Some(h) => h(),
        None => {
            crate::log_debug!(
                "[tunnel/ingress] no tunnel-ingress hook registered (no daemon in this \
                 process) — forwarding cleartext tunnel traffic to :{default_port}"
            );
            Ok(default_port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_default_without_hook_and_uses_hook_when_registered() {
        // The hook slot is process-global: serialize with the connector
        // tests (which run `start()` with e2e:false under the same lock).
        let _g = crate::themes::HOME_LOCK.lock();
        clear_ensure_tunnel_ingress();
        assert!(!hook_registered());
        assert_eq!(ensure_tunnel_ingress_port(4321), Ok(4321));

        register_ensure_tunnel_ingress(Box::new(|| Ok(9999)));
        assert!(hook_registered());
        assert_eq!(ensure_tunnel_ingress_port(4321), Ok(9999));

        register_ensure_tunnel_ingress(Box::new(|| Err("bind failed".to_string())));
        assert_eq!(
            ensure_tunnel_ingress_port(4321),
            Err("bind failed".to_string()),
            "a failing hook must surface, never fall back to the main port"
        );
        clear_ensure_tunnel_ingress();
    }
}
