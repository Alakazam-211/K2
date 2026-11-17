//! Cross-Server Agent Communication (federation): two K2 servers whose
//! owners paired them let their agents message each other.
//!
//! SSOT: `.k2/prds/prd-cross-server-agent-comms.md`. What lives here, and
//! where the daemon wires it:
//!
//!   - [`peers`] — the pinned-peer trust store (`federation-peers.json` under
//!     `k2_home()`, 0600, atomic-rename) with a fail-closed `require_peer`
//!     capability gate. A peer is an asymmetric-key principal, never a
//!     `connect_user` or owner token.
//!   - [`identity`] — the box's federation signing key
//!     (`federation/identity.pem`, split from the tunnel TLS key in 0.45.2).
//!   - [`envelope`] — seal / open an [`AgentSignal`](crate::awareness::AgentSignal)
//!     into an envelope signed by that key and checked against the peer's
//!     pinned key.
//!   - [`pairing_v2`] and [`sas`] — pairing with the commit-reveal SAS (v2,
//!     0.45.2; [`pairing`] keeps the v1 code for old peers).
//!   - [`ingress`] — inbound checks: signature, ±[`DEFAULT_SKEW_SECS`] clock
//!     skew, a nonce cache, control-byte strip and the capability gate.
//!   - [`outbox`] (durable, capped, dead letters), [`seen`] (delivered
//!     message ids, so a redelivery dedupes) and [`dial`] (who may be dialed,
//!     and how).
//!   - [`roster`] (the agents a peer may address) and [`tray`] (packages that
//!     land in the inbox).
//!
//! The daemon's `federation_routes` serves the network side: `/federation/*`
//! inbound and `/cli/federation/*` for the owner and agents. Inbound text
//! reaches an agent only through the recipient workspace's "allow remote
//! instruction" consent, and every delivered message gets a `msg_deliveries`
//! row (`via = federation`, 0.46.16).
//!
//! ## Switch
//!
//! [`enabled`] is the default-OFF switch: the persisted `federationEnabled`
//! setting (synced by [`set_enabled`]) or the `K2_FEDERATION` env force-on.
//! The routes refuse while it is off; the primitives below stay testable
//! regardless.

pub mod address_notice;
pub mod dial;
pub mod envelope;
pub mod identity;
pub mod ingress;
pub mod outbox;
pub mod pairing;
pub mod pairing_v2;
pub mod peers;
pub mod roster;
pub mod sas;
pub mod seen;
pub mod tray;
pub mod vault_recipient;

pub use dial::{
    advertised_federation_base, assert_may_dial, host_port, peer_address_host, resolve_peer_base_url,
    ADVERTISE_URL_ENV, AIRGAP_CONNECT_REFUSE, LAN_HTTPS_REFUSE,
};
pub use envelope::{open, reseal, seal, seal_with_msg_uuid, FederationEnvelope, FederationError, SignedPayload};
pub use ingress::{ingest, IngressError, NonceCache, CAP_INBOUND, DEFAULT_SKEW_SECS};
pub use outbox::{DeadLetter, OutboxItem};
pub use pairing::{
    apply_pair_confirm, apply_pair_request, sas_code, PairOutcome, PairRequest,
    DEFAULT_CONFIRM_CAPS,
};
pub use peers::{
    local_fingerprint, FederationPeer, PeerStore, PeerTrust, RequirePeerError, STORE_VERSION,
};
pub use roster::{
    build_local_roster, sign_roster_request, verify_roster_request, LocalRoster, RosterAgent,
    RosterAuthError, CAP_ROSTER, DEFAULT_ROSTER_SKEW_SECS,
};
pub use tray::{
    decode_spec as decode_tray_spec, encode_spec as encode_tray_spec, parse_payload as parse_tray_payload,
    DecodedTray, TrayFile, TraySpec, TRAY_MAX_BYTES, TRAY_SIGNAL_KIND,
};

use std::sync::atomic::{AtomicBool, Ordering};

/// Runtime mirror of the persisted `AppSettings.federation_enabled` switch.
/// The daemon syncs this at boot and after every settings update, so
/// [`enabled`] stays a pure, cheap, thread-safe check (no per-request disk
/// read) yet is togglable from the UI (K2 Connect → Enable federation)
/// per-server without a restart.
static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the persisted federation switch into this process. The daemon calls
/// this at boot and after a `/cli/settings/update` that changes the flag.
pub fn set_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

/// The in-process setting alone (no env force), for `test_env`'s per-test
/// snapshot of process switches.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn setting_raw() -> bool {
    SETTING_ENABLED.load(Ordering::Relaxed)
}

/// True iff federation is enabled — via the `K2_FEDERATION` env var
/// (`1`/`true`/`yes`/`on`, case-insensitive) OR the persisted app-level
/// `federation_enabled` switch synced through [`set_enabled`]. **Default OFF.**
///
/// The single switch the cross-server surface consults so it stays dormant
/// until explicitly enabled (env force-on for headless boxes; the Settings
/// toggle for everyone else).
pub fn enabled() -> bool {
    if let Ok(v) = std::env::var("K2_FEDERATION") {
        if matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ) {
            return true;
        }
    }
    SETTING_ENABLED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_flag_defaults_off() {
        // The load-bearing default: with K2_FEDERATION unset, federation is OFF.
        // Holds the ONE env lock for the whole test (the env var and the
        // in-memory switch are both process-global); the guards restore
        // K2_FEDERATION on drop (LIFO), also on panic.
        let _lock = crate::test_env::lock();
        let _unset = crate::test_env::EnvVar::remove("K2_FEDERATION");
        assert!(!enabled(), "K2_FEDERATION must default OFF");
        let _on = crate::test_env::EnvVar::set("K2_FEDERATION", "1");
        assert!(enabled(), "K2_FEDERATION=1 must enable");
        let _off = crate::test_env::EnvVar::set("K2_FEDERATION", "off");
        assert!(!enabled(), "K2_FEDERATION=off must disable");

        // The persisted app-level switch (synced via set_enabled) also turns it
        // on with the env var off/unset, and back off — the UI-toggle path.
        let _unset_again = crate::test_env::EnvVar::remove("K2_FEDERATION");
        set_enabled(true);
        assert!(enabled(), "federation_enabled setting must enable");
        set_enabled(false);
        assert!(!enabled(), "federation_enabled OFF must disable");
    }
}
