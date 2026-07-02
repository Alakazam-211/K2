//! Phase-4 durable outbox — restart-surviving queue of sealed envelopes.
//!
//! `prd-cross-server-agent-comms.md` §"On-disk": queued signed envelopes at
//! `~/.k2/federation-outbox/<peer-fp>/<msg-uuid>.json`, atomic-rename, 0600.
//! An envelope is enqueued BEFORE the outbound dial; on a confirmed delivery
//! the file is removed; on failure it stays for the retry loop
//! (`k2-daemon::federation_drain`, so a message to an offline peer survives
//! a sending-daemon restart, Risk M1). Terminal failures (peer declined /
//! 4xx-rejected / expired) are DEAD-LETTERED to `<peer-fp>/dead/<msg-uuid>.json`
//! — kept for inspection, never retried. Both sides are bounded
//! ([`MAX_QUEUED_PER_PEER`] / [`MAX_QUEUED_AGE_DAYS`] / [`MAX_DEAD_PER_PEER`]):
//! an unbounded durable queue is its own leak.
//!
//! The on-disk bytes are the EXACT sealed [`FederationEnvelope`] JSON
//! produced by [`crate::federation::envelope::seal`] — already signed
//! end-to-end, so the queue (like the relay) only ever holds ciphertext-grade
//! signed bytes, never a key or plaintext it could tamper with undetectably.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::fs_atomic::atomic_write;
use crate::log_debug;

use super::envelope::FederationEnvelope;

/// Per-peer queue cap. An unbounded durable queue is its own leak: a peer
/// that never comes back would accumulate files forever. Oldest-first
/// eviction (with a log line) keeps the newest messages — the ones most
/// likely still relevant when the peer reconnects.
pub const MAX_QUEUED_PER_PEER: usize = 500;

/// Per-peer age cap (days). Entries older than this are dead-lettered as
/// `expired` by [`prune_expired`] (the drain sweep calls it every tick) —
/// kept for inspection, never dialed again.
pub const MAX_QUEUED_AGE_DAYS: i64 = 7;

/// Per-peer dead-letter cap. Dead letters are kept for inspection, not
/// forever — oldest evicted (with a log line) past this bound.
pub const MAX_DEAD_PER_PEER: usize = 200;

/// `~/.k2/` (honors `$HOME` so tests redirect it).
fn k2_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

/// Root of the durable outbox tree (`~/.k2/federation-outbox/`).
pub fn outbox_dir() -> PathBuf {
    k2_dir().join("federation-outbox")
}

/// One queued envelope read off disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    /// Recipient peer fingerprint (the subdirectory name).
    pub peer_fingerprint: String,
    /// The message UUID (the filename stem; the idempotency key).
    pub msg_uuid: String,
    /// Absolute path to the queued file (pass to [`remove`] on success).
    pub path: PathBuf,
    /// The sealed envelope bytes to POST to the peer's `/cli/federation/inbound`.
    pub bytes: Vec<u8>,
    /// The inner signal's creation time — the per-peer ORDER key. Stable
    /// across a drain re-seal (which only rotates the envelope's `at`/nonce),
    /// so in-order delivery survives re-sealing.
    pub signal_at: DateTime<Utc>,
    /// The envelope's `at` seal/re-seal timestamp — the drain's staleness
    /// signal (older than the receiver's skew window ⇒ re-seal before dial).
    pub sealed_at: DateTime<Utc>,
}

/// One dead-lettered envelope: a TERMINAL delivery failure (peer declined /
/// 4xx-rejected, or the entry expired past [`MAX_QUEUED_AGE_DAYS`]). Kept on
/// disk for inspection under `<peer>/dead/`; never retried.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeadLetter {
    /// The message UUID (idempotency key of the failed envelope).
    pub msg_uuid: String,
    /// Why delivery is terminally failed (decline reason / HTTP status / expiry).
    pub reason: String,
    /// When the entry was dead-lettered.
    pub dead_at: DateTime<Utc>,
    /// The original sealed envelope (for inspection / manual replay).
    pub envelope: FederationEnvelope,
}

/// Sanitize a fingerprint for use as a directory name. Fingerprints are
/// lowercase hex so this is a no-op in practice, but we refuse path
/// separators / traversal defensively (the value is verified key-derived
/// upstream, but the queue must never write outside its tree).
fn safe_component(s: &str) -> String {
    if s.is_empty() {
        return "_invalid".to_string();
    }
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            _ => '_',
        })
        .collect()
}

/// Enqueue `envelope_bytes` (a sealed [`FederationEnvelope`]) for delivery to
/// `peer_fingerprint`. The filename stem is the envelope's `msg_uuid` so an
/// idempotent re-enqueue of the same message overwrites rather than
/// duplicates. Returns the path written.
pub fn enqueue(peer_fingerprint: &str, envelope_bytes: &[u8]) -> Result<PathBuf, String> {
    // Parse to extract the msg_uuid for the filename (and to reject
    // un-sealed garbage before it lands in the durable queue).
    let env: FederationEnvelope = serde_json::from_slice(envelope_bytes)
        .map_err(|e| format!("refusing to enqueue non-envelope bytes: {e}"))?;
    let uuid = safe_component(&env.payload.msg_uuid);
    let dir = outbox_dir().join(safe_component(peer_fingerprint));
    let path = dir.join(format!("{uuid}.json"));
    atomic_write(&path, envelope_bytes).map_err(|e| format!("write outbox {}: {e}", path.display()))?;

    // Bound the queue (oldest-first eviction, logged): an unbounded durable
    // queue to a never-returning peer is a disk leak. Newest-kept because a
    // reconnecting peer cares about the recent messages, not week-old ones.
    let mut queued = list_for_peer(peer_fingerprint);
    if queued.len() > MAX_QUEUED_PER_PEER {
        sort_oldest_first(&mut queued);
        let excess = queued.len() - MAX_QUEUED_PER_PEER;
        for item in queued.into_iter().take(excess) {
            log_debug!(
                "[federation/outbox] EVICTED oldest queued message {} for peer {} — per-peer cap {} reached",
                item.msg_uuid,
                peer_fingerprint,
                MAX_QUEUED_PER_PEER
            );
            let _ = fs::remove_file(&item.path);
        }
    }
    Ok(path)
}

/// Oldest-first per-peer order: the inner signal's creation time (stable
/// across re-seals), msg_uuid as the deterministic tiebreak.
fn sort_oldest_first(items: &mut [OutboxItem]) {
    items.sort_by(|a, b| {
        a.signal_at
            .cmp(&b.signal_at)
            .then_with(|| a.msg_uuid.cmp(&b.msg_uuid))
    });
}

/// List a peer's queue OLDEST-FIRST — the drain's in-order delivery view.
pub fn list_for_peer_ordered(peer_fingerprint: &str) -> Vec<OutboxItem> {
    let mut items = list_for_peer(peer_fingerprint);
    sort_oldest_first(&mut items);
    items
}

/// Fingerprints of every peer with at least one queued envelope — the sweep's
/// near-zero-idle-cost scan (one readdir; peers with empty queues are skipped
/// without any dial or backoff bookkeeping).
pub fn peers_with_queued() -> Vec<String> {
    let root = outbox_dir();
    let mut out = Vec::new();
    let Ok(peers) = fs::read_dir(&root) else {
        return out;
    };
    for peer in peers.flatten() {
        if !peer.path().is_dir() {
            continue;
        }
        let fp = peer.file_name().to_string_lossy().into_owned();
        if !list_for_peer(&fp).is_empty() {
            out.push(fp);
        }
    }
    out.sort();
    out
}

/// List every queued envelope across all peers (unordered; the drain uses
/// [`list_for_peer_ordered`] for its in-order per-peer view).
pub fn list_all() -> Vec<OutboxItem> {
    let root = outbox_dir();
    let mut out = Vec::new();
    let Ok(peers) = fs::read_dir(&root) else {
        return out;
    };
    for peer in peers.flatten() {
        if !peer.path().is_dir() {
            continue;
        }
        let peer_fp = peer.file_name().to_string_lossy().into_owned();
        out.extend(read_peer_dir(&peer.path(), &peer_fp));
    }
    out
}

/// List queued envelopes for a single peer.
pub fn list_for_peer(peer_fingerprint: &str) -> Vec<OutboxItem> {
    let dir = outbox_dir().join(safe_component(peer_fingerprint));
    read_peer_dir(&dir, peer_fingerprint)
}

fn read_peer_dir(dir: &Path, peer_fp: &str) -> Vec<OutboxItem> {
    let mut items = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return items;
    };
    for e in entries.flatten() {
        let path = e.path();
        let is_json = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".json"))
            .unwrap_or(false);
        if !is_json {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let msg_uuid = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        // The queue only ever holds bytes `enqueue` validated, but a file
        // could be corrupted out from under us — skip (never panic) so one
        // bad entry can't wedge the whole drain.
        let Ok(env) = serde_json::from_slice::<FederationEnvelope>(&bytes) else {
            log_debug!(
                "[federation/outbox] WARN: skipping unreadable queued envelope {}",
                path.display()
            );
            continue;
        };
        items.push(OutboxItem {
            peer_fingerprint: peer_fp.to_string(),
            msg_uuid,
            path,
            bytes,
            signal_at: env.payload.signal.at,
            sealed_at: env.payload.at,
        });
    }
    items
}

/// Remove a delivered envelope from the queue. Returns true if a file was
/// removed.
pub fn remove(path: &Path) -> bool {
    fs::remove_file(path).is_ok()
}

// ─────────────────────────────────────────────────────────────────────
// Dead letters — terminal failures, kept for inspection
// ─────────────────────────────────────────────────────────────────────

/// A peer's dead-letter directory (`<outbox>/<peer-fp>/dead/`). Skipped by
/// the live-queue listers (they only read `*.json` FILES in the peer dir).
fn dead_dir(peer_fingerprint: &str) -> PathBuf {
    outbox_dir()
        .join(safe_component(peer_fingerprint))
        .join("dead")
}

/// Dead-letter a queued envelope: a TERMINAL outcome (peer declined, 4xx
/// rejection, expiry). The live file is removed and a [`DeadLetter`] record
/// (reason + timestamp + the original envelope) is kept under `dead/` for
/// inspection — the drain never dials it again. Bounded to
/// [`MAX_DEAD_PER_PEER`] (oldest evicted, logged).
pub fn dead_letter(item: &OutboxItem, reason: &str) -> Result<PathBuf, String> {
    let env: FederationEnvelope = serde_json::from_slice(&item.bytes)
        .map_err(|e| format!("dead-letter: unreadable envelope: {e}"))?;
    let record = DeadLetter {
        msg_uuid: item.msg_uuid.clone(),
        reason: reason.to_string(),
        dead_at: Utc::now(),
        envelope: env,
    };
    let bytes = serde_json::to_vec(&record).map_err(|e| format!("serialize dead letter: {e}"))?;
    let path = dead_dir(&item.peer_fingerprint).join(format!("{}.json", safe_component(&item.msg_uuid)));
    atomic_write(&path, &bytes).map_err(|e| format!("write dead letter {}: {e}", path.display()))?;
    let _ = fs::remove_file(&item.path);
    log_debug!(
        "[federation/outbox] DEAD-LETTERED message {} for peer {} — {}",
        item.msg_uuid,
        item.peer_fingerprint,
        reason
    );

    // Bound the dead-letter set too (it exists for inspection, not archival).
    let mut dead = list_dead_for_peer(&item.peer_fingerprint);
    if dead.len() > MAX_DEAD_PER_PEER {
        dead.sort_by(|a, b| a.dead_at.cmp(&b.dead_at).then_with(|| a.msg_uuid.cmp(&b.msg_uuid)));
        let excess = dead.len() - MAX_DEAD_PER_PEER;
        for d in dead.into_iter().take(excess) {
            log_debug!(
                "[federation/outbox] EVICTED oldest dead letter {} for peer {} — cap {} reached",
                d.msg_uuid,
                item.peer_fingerprint,
                MAX_DEAD_PER_PEER
            );
            let _ = fs::remove_file(dead_dir(&item.peer_fingerprint).join(format!("{}.json", safe_component(&d.msg_uuid))));
        }
    }
    Ok(path)
}

/// List a peer's dead letters (unordered).
pub fn list_dead_for_peer(peer_fingerprint: &str) -> Vec<DeadLetter> {
    let dir = dead_dir(peer_fingerprint);
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return out;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        if let Ok(d) = serde_json::from_slice::<DeadLetter>(&bytes) {
            out.push(d);
        }
    }
    out
}

/// Dead-letter every queued envelope older than [`MAX_QUEUED_AGE_DAYS`]
/// (by the inner signal's creation time). Called by the drain sweep each
/// tick; returns how many entries were expired.
pub fn prune_expired() -> usize {
    let cutoff = Utc::now() - chrono::Duration::days(MAX_QUEUED_AGE_DAYS);
    let mut expired = 0usize;
    for item in list_all() {
        if item.signal_at < cutoff {
            let reason = format!(
                "expired: queued longer than {MAX_QUEUED_AGE_DAYS} days without a successful delivery"
            );
            if dead_letter(&item, &reason).is_ok() {
                expired += 1;
            }
        }
    }
    expired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awareness::{AgentAddress, AgentSignal, SignalKind, WorkspaceId};
    use crate::federation::envelope::seal;
    use crate::tunnel::test_support::with_temp_home;
    use crate::tunnel::tls::load_or_generate_keypair;

    fn sealed_bytes() -> Vec<u8> {
        sealed_bytes_at(chrono::Utc::now())
    }

    /// Seal a fresh signal whose creation time (`signal.at` — the per-peer
    /// order key) is pinned to `at`.
    fn sealed_bytes_at(at: chrono::DateTime<chrono::Utc>) -> Vec<u8> {
        let key = load_or_generate_keypair().expect("keypair");
        let mut signal = AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("a".into()),
                name: "alice".into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId("b".into()),
                name: "bob".into(),
            },
            SignalKind::Msg { text: "queued".into() },
        );
        signal.at = at;
        seal(&signal, &key, "peer", 8).expect("seal")
    }

    #[test]
    fn enqueue_list_remove_round_trip() {
        with_temp_home(|| {
            let bytes = sealed_bytes();
            let fp = "abc123";
            let path = enqueue(fp, &bytes).expect("enqueue");
            assert!(path.exists(), "queued file must exist");

            let items = list_for_peer(fp);
            assert_eq!(items.len(), 1, "one queued item for the peer");
            assert_eq!(items[0].bytes, bytes, "bytes survive round-trip");
            assert_eq!(list_all().len(), 1, "and shows in list_all");

            assert!(remove(&items[0].path), "remove must succeed");
            assert!(list_for_peer(fp).is_empty(), "queue empty after remove");
        });
    }

    #[test]
    fn enqueue_is_idempotent_on_msg_uuid() {
        with_temp_home(|| {
            let bytes = sealed_bytes();
            let fp = "deadbeef";
            enqueue(fp, &bytes).expect("first");
            enqueue(fp, &bytes).expect("second (same uuid)");
            assert_eq!(
                list_for_peer(fp).len(),
                1,
                "same msg_uuid must overwrite, not duplicate"
            );
        });
    }

    #[test]
    fn enqueue_rejects_non_envelope_bytes() {
        with_temp_home(|| {
            let err = enqueue("fp", b"not an envelope").expect_err("garbage must reject");
            assert!(err.contains("non-envelope"), "got: {err}");
        });
    }

    #[test]
    fn list_for_peer_ordered_is_oldest_first_by_signal_at() {
        with_temp_home(|| {
            let fp = "orderfp";
            let base = chrono::Utc::now();
            // Enqueue out of chronological order — the ordered view must
            // sort by the SIGNAL's creation time, not enqueue order.
            let mid = sealed_bytes_at(base - chrono::Duration::seconds(60));
            let old = sealed_bytes_at(base - chrono::Duration::seconds(120));
            let new = sealed_bytes_at(base);
            enqueue(fp, &mid).expect("mid");
            enqueue(fp, &new).expect("new");
            enqueue(fp, &old).expect("old");

            let items = list_for_peer_ordered(fp);
            assert_eq!(items.len(), 3);
            assert!(
                items[0].signal_at < items[1].signal_at && items[1].signal_at < items[2].signal_at,
                "ordered view must be oldest-first by signal_at: {:?}",
                items.iter().map(|i| i.signal_at).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn enqueue_cap_evicts_oldest_first() {
        with_temp_home(|| {
            let fp = "capfp";
            let base = chrono::Utc::now();
            // Fill to the cap + 2 (oldest sealed first so eviction order is
            // deterministic). Track uuids in age order.
            let total = MAX_QUEUED_PER_PEER + 2;
            let mut uuids_oldest_first: Vec<String> = Vec::new();
            for i in 0..total {
                let bytes =
                    sealed_bytes_at(base - chrono::Duration::seconds((total - i) as i64));
                let env: FederationEnvelope = serde_json::from_slice(&bytes).unwrap();
                uuids_oldest_first.push(env.payload.msg_uuid.clone());
                enqueue(fp, &bytes).expect("enqueue");
            }
            let items = list_for_peer(fp);
            assert_eq!(items.len(), MAX_QUEUED_PER_PEER, "cap must bound the queue");
            let kept: std::collections::HashSet<String> =
                items.into_iter().map(|i| i.msg_uuid).collect();
            assert!(
                !kept.contains(&uuids_oldest_first[0]) && !kept.contains(&uuids_oldest_first[1]),
                "the two OLDEST entries must be evicted"
            );
            assert!(
                kept.contains(uuids_oldest_first.last().unwrap()),
                "the newest entry must survive eviction"
            );
        });
    }

    #[test]
    fn dead_letter_removes_live_entry_and_keeps_record() {
        with_temp_home(|| {
            let fp = "deadfp";
            let bytes = sealed_bytes();
            enqueue(fp, &bytes).expect("enqueue");
            let item = list_for_peer(fp).pop().expect("queued item");

            dead_letter(&item, "peer declined: remote_instruction_disabled").expect("dead-letter");

            assert!(list_for_peer(fp).is_empty(), "live queue must be drained");
            let dead = list_dead_for_peer(fp);
            assert_eq!(dead.len(), 1, "dead letter must be kept for inspection");
            assert_eq!(dead[0].msg_uuid, item.msg_uuid);
            assert!(
                dead[0].reason.contains("remote_instruction_disabled"),
                "reason must be recorded: {}",
                dead[0].reason
            );
        });
    }

    #[test]
    fn prune_expired_dead_letters_only_stale_entries() {
        with_temp_home(|| {
            let fp = "prunefp";
            let stale =
                sealed_bytes_at(chrono::Utc::now() - chrono::Duration::days(MAX_QUEUED_AGE_DAYS + 1));
            let fresh = sealed_bytes();
            enqueue(fp, &stale).expect("stale");
            enqueue(fp, &fresh).expect("fresh");

            let expired = prune_expired();
            assert_eq!(expired, 1, "exactly the stale entry must expire");
            assert_eq!(list_for_peer(fp).len(), 1, "the fresh entry must stay queued");
            let dead = list_dead_for_peer(fp);
            assert_eq!(dead.len(), 1);
            assert!(dead[0].reason.starts_with("expired:"), "got: {}", dead[0].reason);
        });
    }

    #[test]
    fn peers_with_queued_skips_empty_and_dead_only_peers() {
        with_temp_home(|| {
            enqueue("busy", &sealed_bytes()).expect("enqueue");
            // A peer whose only content is a dead letter has an EMPTY queue.
            enqueue("drained", &sealed_bytes()).expect("enqueue");
            let item = list_for_peer("drained").pop().unwrap();
            dead_letter(&item, "declined").expect("dead-letter");

            assert_eq!(peers_with_queued(), vec!["busy".to_string()]);
        });
    }
}
