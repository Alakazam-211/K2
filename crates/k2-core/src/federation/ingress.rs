//! Phase-3 inbound ingress — the SECURITY CORE.
//!
//! `prd-cross-server-agent-comms.md` Phase 3. [`ingest`] is the single entry
//! point a peer's bytes pass through on arrival. It enforces, in order and
//! each fail-closed:
//!
//!   1. **decode** the envelope (read the *claimed* sender + replay fields);
//!   2. look up the claimed peer to fetch its **pinned** key (key material
//!      only — authorization is step 4);
//!   3. **`open`** — verify the ECDSA signature against the pinned key
//!      (AUTHENTICITY; tamper / wrong key / fingerprint-mismatch → reject);
//!   4. **`require_peer(fp, "inbound")`** — `Trusted` + holds the capability
//!      (AUTHORIZATION; DECISION-2 — NEVER a `token_ok`/owner check);
//!   5. **replay / skew / ttl / loop** guards (Risks M1/M2);
//!   6. **control-byte sanitize** the payload (Risk H3 — neutralize
//!      bracketed-paste / CSI injection across the trust boundary);
//!   7. **deliver INTO THE LOCAL INBOX ONLY** via `awareness::egress`
//!      ([`Delivery::Inbox`] is FORCED) — never live-inject, never spawn
//!      (Risks H2/C1).
//!
//! Every non-`Ok` return is a REJECT. There is no "accept on error" path.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::OnceLock;

use chrono::Utc;
use parking_lot::Mutex;

use crate::awareness::{egress, AgentSignal, Delivery, SignalKind};

use super::envelope::{open, FederationEnvelope};
use super::peers::{PeerStore, RequirePeerError};

/// Capability a peer must hold to deliver an inbound cross-server message.
pub const CAP_INBOUND: &str = "inbound";

/// Default acceptable clock skew between the envelope's `at` and now
/// (seconds). A message older/newer than this is rejected (Risk M1).
pub const DEFAULT_SKEW_SECS: i64 = 300;

/// Per-peer cap on remembered nonces (the replay LRU bound, Risk M1).
pub const NONCE_LRU_CAP: usize = 1024;

/// Why ingress rejected. Every variant is a DENY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngressError {
    /// Envelope bytes were not well-formed.
    Decode(String),
    /// No peer pinned with the claimed fingerprint (default-deny strangers).
    UnknownPeer,
    /// Peer is pinned but not `Trusted` (`Pending`/`Blocked`).
    NotTrusted,
    /// Peer is `Trusted` but lacks the `inbound` capability.
    CapabilityDenied,
    /// Signature did not verify against the pinned key (tamper / wrong signer
    /// / claimed fingerprint ≠ pinned key).
    BadSignature,
    /// Duplicate nonce from this peer (replay).
    Replay,
    /// Envelope `at` is outside the acceptable skew window.
    SkewTooLarge { skew_secs: i64 },
    /// Hop budget exhausted (`ttl == 0`).
    TtlExpired,
    /// This daemon's own fingerprint already appears in `trace` (cycle).
    LoopDetected,
}

impl std::fmt::Display for IngressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "decode envelope: {e}"),
            Self::UnknownPeer => write!(f, "unknown peer (not pinned)"),
            Self::NotTrusted => write!(f, "peer not trusted"),
            Self::CapabilityDenied => write!(f, "peer lacks the inbound capability"),
            Self::BadSignature => write!(f, "signature verification failed"),
            Self::Replay => write!(f, "replayed nonce"),
            Self::SkewTooLarge { skew_secs } => {
                write!(f, "clock skew {skew_secs}s exceeds the accepted window")
            }
            Self::TtlExpired => write!(f, "ttl exhausted"),
            Self::LoopDetected => write!(f, "delegation loop (self in trace)"),
        }
    }
}

impl std::error::Error for IngressError {}

/// Bounded per-peer seen-nonce set for replay defense (Risk M1). In-memory;
/// a daemon restart resets it (the skew window bounds the replay opportunity
/// that opens across a restart).
#[derive(Default)]
pub struct NonceCache {
    inner: Mutex<HashMap<String, VecDeque<String>>>,
}

impl NonceCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `nonce` for `fingerprint`. Returns `true` if it was NEW (and is
    /// now remembered), `false` if it was already seen (a replay). Bounded to
    /// [`NONCE_LRU_CAP`] per peer (oldest evicted).
    pub fn check_and_record(&self, fingerprint: &str, nonce: &str) -> bool {
        let mut map = self.inner.lock();
        let q = map.entry(fingerprint.to_string()).or_default();
        if q.iter().any(|n| n == nonce) {
            return false;
        }
        q.push_back(nonce.to_string());
        while q.len() > NONCE_LRU_CAP {
            q.pop_front();
        }
        true
    }
}

/// Process-global replay cache (the daemon's default). Tests construct their
/// own [`NonceCache`] for isolation.
pub fn global_nonce_cache() -> &'static NonceCache {
    static C: OnceLock<NonceCache> = OnceLock::new();
    C.get_or_init(NonceCache::new)
}

/// Strip control bytes from cross-server text (Risk H3). Removes every C0
/// control char AND DEL/ESC — so `\x1b[201~` (a bracketed-paste close that
/// could break out of a paste bracket) is neutralized to `[201~` — while
/// keeping `\n`/`\t` so legitimate multi-line text survives. Inbox bytes are
/// inert regardless; this is defense-in-depth for any future live path.
pub fn sanitize_text(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect()
}

/// Return a copy of `signal` with control bytes stripped from every
/// text-bearing field.
fn sanitize_signal(mut signal: AgentSignal) -> AgentSignal {
    signal.kind = match signal.kind {
        SignalKind::Msg { text } => SignalKind::Msg {
            text: sanitize_text(&text),
        },
        SignalKind::Status { text } => SignalKind::Status {
            text: sanitize_text(&text),
        },
        SignalKind::Reservation { paths, action } => SignalKind::Reservation {
            paths: paths.iter().map(|p| sanitize_text(p)).collect(),
            action,
        },
        SignalKind::TaskLifecycle { phase, task_ref } => SignalKind::TaskLifecycle {
            phase,
            task_ref: task_ref.map(|r| sanitize_text(&r)),
        },
        SignalKind::Custom { kind, payload } => SignalKind::Custom {
            kind: sanitize_text(&kind),
            payload,
        },
        // Presence carries no free text.
        other => other,
    };
    signal
}

/// Ingest a raw inbound federation envelope. See the module docs for the
/// fail-closed order. On success the (sanitized, inbox-forced) signal has
/// been delivered into the local awareness inbox and is returned.
///
/// `local_fp` is THIS daemon's fingerprint (for the loop check). `skew_secs`
/// is the accepted clock-skew window ([`DEFAULT_SKEW_SECS`] in production).
pub fn ingest(
    bytes: &[u8],
    store: &PeerStore,
    nonce_cache: &NonceCache,
    inbox_root: &Path,
    local_fp: &str,
    skew_secs: i64,
) -> Result<AgentSignal, IngressError> {
    // 1. DECODE — read the claimed sender + replay fields (untrusted).
    let envelope: FederationEnvelope =
        serde_json::from_slice(bytes).map_err(|e| IngressError::Decode(e.to_string()))?;
    let claimed_fp = envelope.payload.from_fingerprint.clone();

    // 2. Look up the claimed peer ONLY to fetch its pinned key (authz is #4).
    let pinned_pem = store
        .get(&claimed_fp)
        .map(|p| p.public_key_pem.clone())
        .ok_or(IngressError::UnknownPeer)?;

    // 3. AUTHENTICITY — verify the signature against the pinned key. `open`
    //    also re-binds the claimed fingerprint to the pinned key, so a
    //    fingerprint/key mismatch is rejected here too.
    let signal = open(bytes, &pinned_pem).map_err(|_| IngressError::BadSignature)?;

    // 4. AUTHORIZATION — Trusted + holds `inbound` (fail-closed; DECISION-2:
    //    NEVER token_ok / token_is_owner).
    store
        .require_peer(&claimed_fp, CAP_INBOUND)
        .map_err(|e| match e {
            RequirePeerError::UnknownPeer => IngressError::UnknownPeer,
            RequirePeerError::NotTrusted(_) => IngressError::NotTrusted,
            RequirePeerError::CapabilityDenied { .. } => IngressError::CapabilityDenied,
        })?;

    // 5. REPLAY / SKEW / TTL / LOOP.
    let p = &envelope.payload;
    if p.ttl == 0 {
        return Err(IngressError::TtlExpired);
    }
    if !local_fp.is_empty() && p.trace.iter().any(|f| f == local_fp) {
        return Err(IngressError::LoopDetected);
    }
    let skew = (Utc::now() - p.at).num_seconds().abs();
    if skew > skew_secs {
        return Err(IngressError::SkewTooLarge { skew_secs: skew });
    }
    // Replay is recorded LAST among the guards so a message rejected for
    // skew/ttl/loop doesn't burn its nonce (and a valid retry can succeed).
    if !nonce_cache.check_and_record(&claimed_fp, &p.nonce) {
        return Err(IngressError::Replay);
    }

    // 6. SANITIZE (Risk H3).
    let mut delivered = sanitize_signal(signal);

    // 7. INBOX ONLY — force the delivery mode so a sender can NEVER drive a
    //    live-inject / wake / spawn across the federation boundary (H2/C1).
    delivered.delivery = Delivery::Inbox;
    let _report = egress::deliver(&delivered, inbox_root);

    Ok(delivered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awareness::{inbox, AgentAddress, SignalKind, WorkspaceId};
    use crate::federation::envelope::{seal, FederationEnvelope};
    use crate::federation::peers::{FederationPeer, PeerStore, PeerTrust};
    use crate::tunnel::test_support::with_temp_home;
    use crate::tunnel::tls::load_or_generate_keypair;

    fn signal_to(agent: &str, text: &str) -> AgentSignal {
        AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("ws-a".into()),
                name: "alice".into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId("ws-b".into()),
                name: agent.into(),
            },
            SignalKind::Msg { text: text.into() },
        )
    }

    /// Build a Trusted+inbound peer pinned to `pem`, returning the store.
    fn trusted_store(pem: &str) -> (PeerStore, String) {
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("peer", "peer", pem).expect("pin"));
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, CAP_INBOUND);
        (store, fp)
    }

    #[test]
    fn valid_envelope_from_trusted_peer_delivers_to_inbox() {
        with_temp_home(|| {
            crate::db::init_for_tests();
            let key = load_or_generate_keypair().expect("keypair");
            let pem = key.public_key_pem();
            let (store, _fp) = trusted_store(&pem);
            let cache = NonceCache::new();
            let inbox_root = std::env::temp_dir().join(format!(
                "k2-fed-inbox-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));

            let bytes = seal(&signal_to("bob", "hello peer"), &key, "peer", 8).expect("seal");
            let delivered = ingest(&bytes, &store, &cache, &inbox_root, "local", DEFAULT_SKEW_SECS)
                .expect("valid envelope must deliver");
            assert_eq!(delivered.delivery, Delivery::Inbox, "must be forced to inbox");

            // The signal landed in bob's inbox (inbox-only, no spawn/live).
            let drained = inbox::drain(&inbox_root, "bob");
            assert_eq!(drained.len(), 1, "exactly one inbox item");
            assert_eq!(drained[0].delivery, Delivery::Inbox);
            let _ = std::fs::remove_dir_all(&inbox_root);
        });
    }

    #[test]
    fn rejects_unknown_peer() {
        with_temp_home(|| {
            let key = load_or_generate_keypair().expect("keypair");
            let bytes = seal(&signal_to("bob", "hi"), &key, "peer", 8).expect("seal");
            let store = PeerStore::default(); // nobody pinned
            let cache = NonceCache::new();
            let err = ingest(&bytes, &store, &cache, std::path::Path::new("/tmp/x"), "l", 300)
                .expect_err("unknown peer must reject");
            assert_eq!(err, IngressError::UnknownPeer);
        });
    }

    #[test]
    fn rejects_untrusted_pending_peer() {
        with_temp_home(|| {
            let key = load_or_generate_keypair().expect("keypair");
            let pem = key.public_key_pem();
            let mut store = PeerStore::default();
            // Pinned + granted but left Pending → fail-closed.
            let fp = store.upsert(FederationPeer::pin("p", "p", &pem).expect("pin"));
            store.grant(&fp, CAP_INBOUND);
            let bytes = seal(&signal_to("bob", "hi"), &key, "peer", 8).expect("seal");
            let cache = NonceCache::new();
            let err = ingest(&bytes, &store, &cache, std::path::Path::new("/tmp/x"), "l", 300)
                .expect_err("pending peer must reject");
            assert_eq!(err, IngressError::NotTrusted);
        });
    }

    #[test]
    fn rejects_trusted_peer_without_capability() {
        with_temp_home(|| {
            let key = load_or_generate_keypair().expect("keypair");
            let pem = key.public_key_pem();
            let mut store = PeerStore::default();
            let fp = store.upsert(FederationPeer::pin("p", "p", &pem).expect("pin"));
            store.set_trust(&fp, PeerTrust::Trusted); // Trusted but NO inbound cap
            let bytes = seal(&signal_to("bob", "hi"), &key, "peer", 8).expect("seal");
            let cache = NonceCache::new();
            let err = ingest(&bytes, &store, &cache, std::path::Path::new("/tmp/x"), "l", 300)
                .expect_err("missing cap must reject");
            assert_eq!(err, IngressError::CapabilityDenied);
        });
    }

    #[test]
    fn rejects_bad_signature() {
        with_temp_home(|| {
            let key = load_or_generate_keypair().expect("keypair");
            let pem = key.public_key_pem();
            let (store, _fp) = trusted_store(&pem);
            let cache = NonceCache::new();
            // Tamper the payload after sealing → signature no longer matches.
            let bytes = seal(&signal_to("bob", "original"), &key, "peer", 8).expect("seal");
            let mut env: FederationEnvelope = serde_json::from_slice(&bytes).unwrap();
            env.payload.signal.kind = SignalKind::Msg {
                text: "TAMPERED".into(),
            };
            let tampered = serde_json::to_vec(&env).unwrap();
            let err = ingest(&tampered, &store, &cache, std::path::Path::new("/tmp/x"), "l", 300)
                .expect_err("tamper must reject");
            assert_eq!(err, IngressError::BadSignature);
        });
    }

    #[test]
    fn rejects_replayed_nonce() {
        with_temp_home(|| {
            crate::db::init_for_tests();
            let key = load_or_generate_keypair().expect("keypair");
            let pem = key.public_key_pem();
            let (store, _fp) = trusted_store(&pem);
            let cache = NonceCache::new();
            let inbox_root = std::env::temp_dir().join(format!(
                "k2-fed-replay-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let bytes = seal(&signal_to("bob", "once"), &key, "peer", 8).expect("seal");
            // First delivery succeeds; the SAME bytes (same nonce) replay-fail.
            ingest(&bytes, &store, &cache, &inbox_root, "l", 300).expect("first ok");
            let err = ingest(&bytes, &store, &cache, &inbox_root, "l", 300)
                .expect_err("replay must reject");
            assert_eq!(err, IngressError::Replay);
            let _ = std::fs::remove_dir_all(&inbox_root);
        });
    }

    #[test]
    fn rejects_ttl_zero() {
        with_temp_home(|| {
            let key = load_or_generate_keypair().expect("keypair");
            let pem = key.public_key_pem();
            let (store, _fp) = trusted_store(&pem);
            let cache = NonceCache::new();
            let bytes = seal(&signal_to("bob", "hi"), &key, "peer", 0).expect("seal");
            let err = ingest(&bytes, &store, &cache, std::path::Path::new("/tmp/x"), "l", 300)
                .expect_err("ttl=0 must reject");
            assert_eq!(err, IngressError::TtlExpired);
        });
    }

    #[test]
    fn rejects_self_in_trace_loop() {
        with_temp_home(|| {
            let key = load_or_generate_keypair().expect("keypair");
            let pem = key.public_key_pem();
            let (store, _fp) = trusted_store(&pem);
            let cache = NonceCache::new();
            // seal() seeds trace with the sender fp; treat THAT as our local fp
            // to simulate the envelope having already passed through us.
            let bytes = seal(&signal_to("bob", "hi"), &key, "peer", 8).expect("seal");
            let env: FederationEnvelope = serde_json::from_slice(&bytes).unwrap();
            let me = env.payload.trace[0].clone();
            let err = ingest(&bytes, &store, &cache, std::path::Path::new("/tmp/x"), &me, 300)
                .expect_err("self-in-trace must reject");
            assert_eq!(err, IngressError::LoopDetected);
        });
    }

    #[test]
    fn sanitize_strips_escape_and_bracketed_paste() {
        let dirty = "before\x1b[201~after\x07end";
        let clean = sanitize_text(dirty);
        assert!(!clean.contains('\x1b'), "ESC must be stripped");
        assert!(!clean.contains('\x07'), "BEL must be stripped");
        assert!(clean.contains("before") && clean.contains("after") && clean.contains("end"));
    }
}
