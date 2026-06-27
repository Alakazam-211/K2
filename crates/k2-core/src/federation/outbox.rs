//! Phase-4 durable outbox — restart-surviving queue of sealed envelopes.
//!
//! `prd-cross-server-agent-comms.md` §"On-disk": queued signed envelopes at
//! `~/.k2/federation-outbox/<peer-fp>/<msg-uuid>.json`, atomic-rename, 0600.
//! An envelope is enqueued BEFORE the outbound dial; on a confirmed delivery
//! the file is removed; on failure it stays for the retry loop (so a message
//! to an offline peer survives a sending-daemon restart, Risk M1).
//!
//! The on-disk bytes are the EXACT sealed [`FederationEnvelope`] JSON
//! produced by [`crate::federation::envelope::seal`] — already signed
//! end-to-end, so the queue (like the relay) only ever holds ciphertext-grade
//! signed bytes, never a key or plaintext it could tamper with undetectably.

use std::fs;
use std::path::{Path, PathBuf};

use crate::fs_atomic::atomic_write;

use super::envelope::FederationEnvelope;

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
    Ok(path)
}

/// List every queued envelope across all peers (oldest-first within a peer is
/// not guaranteed; the retry loop treats the queue as a set).
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
        items.push(OutboxItem {
            peer_fingerprint: peer_fp.to_string(),
            msg_uuid,
            path,
            bytes,
        });
    }
    items
}

/// Remove a delivered envelope from the queue. Returns true if a file was
/// removed.
pub fn remove(path: &Path) -> bool {
    fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awareness::{AgentAddress, AgentSignal, SignalKind, WorkspaceId};
    use crate::federation::envelope::seal;
    use crate::tunnel::test_support::with_temp_home;
    use crate::tunnel::tls::load_or_generate_keypair;

    fn sealed_bytes() -> Vec<u8> {
        let key = load_or_generate_keypair().expect("keypair");
        let signal = AgentSignal::new(
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
}
