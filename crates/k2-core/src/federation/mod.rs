//! Cross-Server Agent Communication (federation) — **Phase 1: internal
//! cryptographic + trust-store primitives ONLY**.
//!
//! SSOT: `.k2/prds/prd-cross-server-agent-comms.md`. This module implements
//! the *foundation* the later phases stand on, and **opens nothing on the
//! network**:
//!
//!   - [`peers`] — a TOFU pinned-peer trust store (`~/.k2/federation-peers.json`,
//!     0600, atomic-rename) with owner-side CRUD and a **fail-closed
//!     `require_peer` capability gate** (default-deny). DECISION-2: a peer is an
//!     **asymmetric-key principal**, never a `connect_user`/owner token.
//!   - [`envelope`] — pure [`seal`](envelope::seal) / [`open`](envelope::open)
//!     of an [`AgentSignal`](crate::awareness::AgentSignal) into a signed
//!     envelope, signed by the **existing daemon tunnel keypair**
//!     ([`crate::tunnel::tls::load_or_generate_keypair`]) and verified against a
//!     peer's **pinned** ECDSA P-256 public key.
//!
//! ## What is deliberately NOT here (later phases — see the PRD phase map)
//!
//! There is **no `UnixListener`/`TcpListener`, no dial code, no
//! `routes/dispatcher.rs` touch, no pairing handshake, no ingress/egress, no
//! replay/skew/TTL enforcement** in Phase 1. Those open the trust boundary and
//! land later (Phase 2 pairing, Phase 3 ingress, Phase 4 egress) behind their
//! own review. These primitives are a *library*: nothing in the daemon calls
//! them yet, so the whole module is inert in a shipped 0.40.x build.
//!
//! ## Feature flag
//!
//! [`enabled`] reads the default-OFF `K2_FEDERATION` env flag (mirroring
//! `session_token::scoped_hooks_enabled` / `K2_HOOK_SCOPED`). It exists so the
//! later ingress/egress wiring can gate itself on a single switch; Phase 1
//! ships it OFF and unconsulted. The pure primitives below remain unit-testable
//! regardless of the flag — inertness comes from having no caller, not from a
//! runtime branch inside the crypto.

pub mod dial;
pub mod envelope;
pub mod ingress;
pub mod outbox;
pub mod pairing;
pub mod peers;
pub mod roster;
pub mod seen;

pub use dial::{
    advertised_federation_base, assert_may_dial, host_port, peer_address_host, resolve_peer_base_url,
    ADVERTISE_URL_ENV, AIRGAP_CONNECT_REFUSE, LAN_HTTPS_REFUSE,
};
pub use envelope::{open, reseal, seal, FederationEnvelope, FederationError, SignedPayload};
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
        // (Serialized via the env lock that the crypto tests also hold — but this
        // test only reads, and the default-unset case is what production ships.)
        let prev = std::env::var_os("K2_FEDERATION");
        std::env::remove_var("K2_FEDERATION");
        assert!(!enabled(), "K2_FEDERATION must default OFF");
        std::env::set_var("K2_FEDERATION", "1");
        assert!(enabled(), "K2_FEDERATION=1 must enable");
        std::env::set_var("K2_FEDERATION", "off");
        assert!(!enabled(), "K2_FEDERATION=off must disable");

        // The persisted app-level switch (synced via set_enabled) also turns it
        // on with the env var off/unset, and back off — the UI-toggle path.
        std::env::remove_var("K2_FEDERATION");
        set_enabled(true);
        assert!(enabled(), "federation_enabled setting must enable");
        set_enabled(false);
        assert!(!enabled(), "federation_enabled OFF must disable");

        match prev {
            Some(p) => std::env::set_var("K2_FEDERATION", p),
            None => std::env::remove_var("K2_FEDERATION"),
        }
    }
}
