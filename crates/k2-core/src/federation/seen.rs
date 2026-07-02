//! Durable per-peer seen-`msg_uuid` store — RECEIVE-side idempotency for
//! at-least-once federation delivery.
//!
//! The in-memory [`super::ingress::NonceCache`] replay guard is a SECURITY
//! bound (per-peer nonce LRU) that resets on a daemon restart, and a drain
//! re-seal rotates the nonce anyway — so neither protects a canonical chat
//! from seeing the SAME message twice when the sender's outbox drain
//! redelivers an envelope whose first delivery succeeded but whose response
//! was lost. The `msg_uuid` is the stable end-to-end idempotency key (signed
//! into the payload, preserved across [`super::envelope::reseal`]), so this
//! store remembers the last [`SEEN_PER_PEER_CAP`] delivered uuids per peer at
//! `~/.k2/federation-seen.json` (atomic-rename, 0600 like the peer store) and
//! the inbound handler 409s a duplicate BEFORE it can drive a chat twice.
//!
//! Deliberately tiny: one JSON file, FIFO-bounded per peer, read+write per
//! inbound message (federation traffic is human-scale). Not a security
//! boundary — the ingress gate has already authenticated the peer by the
//! time this is consulted.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fs_atomic::atomic_write;
use crate::log_debug;

/// Per-peer cap on remembered delivered `msg_uuid`s (FIFO, oldest evicted).
/// 512 uuids ≈ 20 KB per peer — bounded, and far deeper than any plausible
/// redelivery window (the sender's queue itself caps at 500).
pub const SEEN_PER_PEER_CAP: usize = 512;

/// `~/.k2/federation-seen.json` (honors `$HOME` so tests redirect it).
fn store_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("federation-seen.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SeenStore {
    /// peer fingerprint → delivered msg_uuids, oldest-first.
    #[serde(default)]
    peers: HashMap<String, VecDeque<String>>,
}

fn load() -> SeenStore {
    let path = store_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return SeenStore::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        // A corrupt store must not brick inbound delivery — start fresh
        // (worst case: one redelivery window loses its dedupe memory).
        log_debug!(
            "[federation/seen] WARN: unreadable {} ({e}) — resetting",
            path.display()
        );
        SeenStore::default()
    })
}

fn save(store: &SeenStore) {
    let Ok(bytes) = serde_json::to_vec(store) else { return };
    if let Err(e) = atomic_write(&store_path(), &bytes) {
        log_debug!("[federation/seen] WARN: write {}: {e}", store_path().display());
    }
}

/// Has `msg_uuid` from `peer_fingerprint` already been terminally handled
/// (delivered or declined)? Read-only — pairs with [`record`].
pub fn already_seen(peer_fingerprint: &str, msg_uuid: &str) -> bool {
    load()
        .peers
        .get(peer_fingerprint)
        .map(|q| q.iter().any(|u| u == msg_uuid))
        .unwrap_or(false)
}

/// Remember `msg_uuid` from `peer_fingerprint` as terminally handled, so a
/// redelivery (sender drain retry after a lost response) dedupes instead of
/// driving the canonical chat twice. FIFO-bounded per peer.
pub fn record(peer_fingerprint: &str, msg_uuid: &str) {
    if peer_fingerprint.is_empty() || msg_uuid.is_empty() {
        return;
    }
    let mut store = load();
    let q = store.peers.entry(peer_fingerprint.to_string()).or_default();
    if q.iter().any(|u| u == msg_uuid) {
        return;
    }
    q.push_back(msg_uuid.to_string());
    while q.len() > SEEN_PER_PEER_CAP {
        q.pop_front();
    }
    save(&store);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn record_then_seen_survives_reload() {
        with_temp_home(|| {
            assert!(!already_seen("fp", "uuid-1"), "fresh uuid must be unseen");
            record("fp", "uuid-1");
            // Both queries load from disk — this IS the restart-survival
            // property the in-memory nonce cache lacks.
            assert!(already_seen("fp", "uuid-1"), "recorded uuid must be seen");
            assert!(!already_seen("fp", "uuid-2"), "other uuids stay unseen");
            assert!(!already_seen("other", "uuid-1"), "dedup is per-peer");
        });
    }

    #[test]
    fn record_is_fifo_bounded_per_peer() {
        with_temp_home(|| {
            for i in 0..(SEEN_PER_PEER_CAP + 1) {
                record("fp", &format!("uuid-{i}"));
            }
            assert!(
                !already_seen("fp", "uuid-0"),
                "oldest uuid must be evicted past the cap"
            );
            assert!(
                already_seen("fp", &format!("uuid-{SEEN_PER_PEER_CAP}")),
                "newest uuid must be remembered"
            );
        });
    }
}
