//! TOFU pinned-peer trust store + the fail-closed `require_peer` gate.
//!
//! Phase 1 of `prd-cross-server-agent-comms.md`. A **federated peer** is
//! another K2 daemon, identified by the **SHA-256 fingerprint of its ECDSA
//! P-256 SubjectPublicKeyInfo (SPKI)**. Our own key is the federation identity
//! at `~/.k2/federation/identity.pem` (`federation::identity`; before 0.45.2
//! it was the tunnel key, which the identity was copied from). The store pins
//! each peer's public key (TOFU, SSH-`authorized_keys` ergonomics) and a
//! **default-deny capability set**.
//!
//! DECISION-2: a peer is an **asymmetric-key principal**, NEVER a
//! `connect_user`/owner token. Authorization is the per-peer capability set,
//! evaluated by [`PeerStore::require_peer`] — never `token_ok`/`token_is_owner`.
//! The namespaces are provably disjoint: this module knows nothing about
//! session tokens, and the token layer knows nothing about peers.
//!
//! **No network here.** This is a pure on-disk store + gate. Pairing (Phase 2),
//! ingress (Phase 3) and egress (Phase 4) wire it to the wire later.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// On-disk envelope version. An unknown version fails CLOSED on load
/// (mirrors `connect_users::SESSION_RECORD_VERSION` /
/// `session_token::STORE_VERSION`): a store we can't understand grants no
/// peers rather than silently trusting reinterpreted bytes.
pub const STORE_VERSION: u32 = 1;

/// Serializes every load-mutate-save of the peer store in this process
/// ([`PeerStore::transact`]).
static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `~/.k2/` — honors `$HOME` so tests redirect it to a tempdir (matches
/// `tunnel::tls::k2_dir` / `config::config_dir`).
fn k2_dir() -> PathBuf {
    crate::paths::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

/// Path to the persisted trust store (`~/.k2/federation-peers.json`, 0600).
pub fn store_path() -> PathBuf {
    k2_dir().join("federation-peers.json")
}

/// Trust state of a pinned peer. A peer delivers/serves **only** in
/// [`PeerTrust::Trusted`] — `Pending` (TOFU-seen, owner not yet confirmed) and
/// `Blocked` are fail-closed (Risk H4: headless boxes have no UI to approve, so
/// an unconfirmed peer must do nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum PeerTrust {
    /// TOFU-seen but not owner-confirmed. Delivers/serves nothing.
    Pending,
    /// Owner-confirmed. The only state that passes [`PeerStore::require_peer`].
    Trusted,
    /// Explicitly denied. Never promotable without an owner action.
    Blocked,
}

/// A pinned federated peer (one row of the trust store). Modeled on the PRD's
/// `FederationPeer` and `connect_users` (`epoch`-style instant revocation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FederationPeer {
    /// SHA-256(SPKI) lowercase hex — the **authoritative** peer id.
    pub fingerprint: String,
    /// Cosmetic label (e.g. `"rosson@laptop"`). Never trusted for routing/auth.
    pub label: String,
    /// Routing hint. Connect: bare subdomain (`rpm` → `https://rpm.k2.dev`).
    /// LAN/Tailscale: saved host:port (`192.168.1.50:38471`). Identity is the key.
    pub subdomain: String,
    /// Explicit dial base URL (`http://192.168.1.50:38471`). Empty means
    /// derive from [`Self::subdomain`] (Connect zone, unless air-gap).
    /// Additive: missing on old stores (serde default). STORE_VERSION stays 1.
    #[serde(default)]
    pub base_url: String,
    /// Pinned ECDSA P-256 SPKI, PEM-encoded (TOFU). Verification pins THIS.
    pub public_key_pem: String,
    /// Trust state. Only `Trusted` passes the gate.
    pub trust: PeerTrust,
    /// **Default-deny** granted-capability set. A capability the peer is not
    /// granted is denied. Common names (later phases): `"inbound"`, `"roster"`,
    /// or workspace-scoped `"msg:<projects.id-uuid>"` / `"inbox:<uuid>"`.
    /// `may_autospawn`/`may_live` are deliberately NOT granted by Phase-1
    /// defaults (Risk H2 — cross-server never spawns).
    pub capabilities: BTreeSet<String>,
    /// Revocation epoch. Bumping it invalidates in-flight grants from this peer
    /// without a restart (mirrors `connect_users::token_epoch`).
    pub epoch: u64,
    /// When the peer row was first created.
    pub added_at: DateTime<Utc>,
    /// Highest address-notice `seq` accepted from this peer
    /// ([`super::address_notice`]). A notice with a seq at or below it is a
    /// replay. Additive (serde default 0); STORE_VERSION stays 1.
    #[serde(default)]
    pub address_seq: u64,
    /// When a signed address notice last changed this peer's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_updated_at: Option<DateTime<Utc>>,
    /// How this peer was paired: 1 = the fingerprint-only code (before
    /// 0.45.2; "re-pair recommended"), 2 = the commit-then-reveal code
    /// ([`super::pairing_v2`]). Missing on old stores = 1. Both keep
    /// exchanging messages.
    #[serde(default = "pairing_version_v1")]
    pub pairing_version: u32,
}

fn pairing_version_v1() -> u32 {
    1
}

impl FederationPeer {
    /// Build a fresh `Pending`, capability-empty peer pinned to `public_key_pem`.
    /// The fingerprint is **derived from the pinned key**, never caller-supplied
    /// — so the id can't disagree with the bytes it names.
    pub fn pin(
        label: impl Into<String>,
        subdomain: impl Into<String>,
        public_key_pem: impl Into<String>,
    ) -> Result<Self, String> {
        let pem = public_key_pem.into();
        let fingerprint = fingerprint_of_spki_pem(&pem)?;
        Ok(Self {
            fingerprint,
            label: label.into(),
            subdomain: subdomain.into(),
            base_url: String::new(),
            public_key_pem: pem,
            trust: PeerTrust::Pending,
            capabilities: BTreeSet::new(),
            epoch: 0,
            added_at: Utc::now(),
            address_seq: 0,
            address_updated_at: None,
            pairing_version: super::pairing_v2::PAIRING_VERSION,
        })
    }

    /// Set the per-peer dial base URL (F2). Empty clears it.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into().trim().trim_end_matches('/').to_string();
        self
    }
}

/// On-disk shape: a versioned wrapper so an unknown version fails closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPeers {
    version: u32,
    peers: Vec<FederationPeer>,
}

/// Why [`PeerStore::require_peer`] denied. Every variant is a DENY — the gate
/// is fail-closed; there is no "allow on error" path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequirePeerError {
    /// No peer with that fingerprint is pinned. Default-deny for strangers.
    UnknownPeer,
    /// Peer is pinned but not `Trusted` (i.e. `Pending`/`Blocked`).
    NotTrusted(PeerTrust),
    /// Peer is `Trusted` but the specific capability was never granted.
    CapabilityDenied { capability: String },
}

impl std::fmt::Display for RequirePeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPeer => write!(f, "unknown peer (not pinned)"),
            Self::NotTrusted(t) => write!(f, "peer not trusted (state={t:?})"),
            Self::CapabilityDenied { capability } => {
                write!(f, "capability denied: {capability}")
            }
        }
    }
}

impl std::error::Error for RequirePeerError {}

/// The pinned-peer trust store. In-memory `Vec` materialized from
/// `~/.k2/federation-peers.json`; every mutator persists atomically (0600).
#[derive(Debug, Clone, Default)]
pub struct PeerStore {
    peers: Vec<FederationPeer>,
}

impl PeerStore {
    /// Load the persisted store, or an empty one if the file is absent.
    ///
    /// Fails LOUD on a present-but-corrupt/unknown-version file — we never
    /// silently start with an empty (everyone-denied is safe, but a *partially*
    /// reinterpreted store is not) trust set over bytes we can't parse.
    pub fn load() -> Result<Self, String> {
        let path = store_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("read federation peers {}: {e}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        let stored: StoredPeers = serde_json::from_str(&raw)
            .map_err(|e| format!("parse federation peers {}: {e}", path.display()))?;
        if stored.version != STORE_VERSION {
            return Err(format!(
                "federation peers {} has unknown version {} (expected {}) — \
                 refusing to load (fail-closed)",
                path.display(),
                stored.version,
                STORE_VERSION
            ));
        }
        Ok(Self {
            peers: stored.peers,
        })
    }

    /// **The one way to change the persisted store** (0.45.2): under ONE
    /// process-wide lock, load the store, run `f`, and save only when `f`
    /// returns `Ok((value, true))`. `Ok((value, false))` and `Err` write
    /// nothing. The outer `Err` is a load/save failure; the inner result is
    /// `f`'s own. Every writer goes through here, so two writers (threads of
    /// one daemon) can never lose each other's update.
    pub fn transact<T, E>(
        f: impl FnOnce(&mut PeerStore) -> Result<(T, bool), E>,
    ) -> Result<Result<T, E>, String> {
        let _g = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut store = Self::load()?;
        match f(&mut store) {
            Ok((value, true)) => {
                store.write_file()?;
                Ok(Ok(value))
            }
            Ok((value, false)) => Ok(Ok(value)),
            Err(e) => Ok(Err(e)),
        }
    }

    /// Persist this whole store (tests seed fixtures with it). Production
    /// code changes the store only through [`Self::transact`].
    #[cfg(any(test, feature = "test-util"))]
    pub fn save(&self) -> Result<(), String> {
        let _g = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        self.write_file()
    }

    /// Atomic write: a uniquely named temp file (0600), fsync, rename over
    /// the store, then fsync the directory. Callers hold [`STORE_LOCK`].
    fn write_file(&self) -> Result<(), String> {
        use std::io::Write;
        let stored = StoredPeers {
            version: STORE_VERSION,
            peers: self.peers.clone(),
        };
        let json = serde_json::to_vec_pretty(&stored)
            .map_err(|e| format!("serialize federation peers: {e}"))?;
        let dir = k2_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        let path = store_path();
        let tmp = dir.join(format!("federation-peers.json.tmp.{}", uuid::Uuid::new_v4()));
        let written = (|| {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|e| format!("create {}: {e}", tmp.display()))?;
            restrict_mode(&tmp)?;
            f.write_all(&json).map_err(|e| format!("write {}: {e}", tmp.display()))?;
            f.sync_all().map_err(|e| format!("fsync {}: {e}", tmp.display()))?;
            fs::rename(&tmp, &path).map_err(|e| format!("rename into place {}: {e}", path.display()))
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        restrict_mode(&path)?;
        #[cfg(unix)]
        if let Ok(d) = fs::File::open(&dir) {
            // Persist the rename itself; a failure here leaves a valid file.
            let _ = d.sync_all();
        }
        Ok(())
    }

    /// All pinned peers (read-only view).
    pub fn list(&self) -> &[FederationPeer] {
        &self.peers
    }

    /// Look up a peer by fingerprint (no trust/capability check).
    pub fn get(&self, fingerprint: &str) -> Option<&FederationPeer> {
        self.peers.iter().find(|p| p.fingerprint == fingerprint)
    }

    /// Mutable lookup by fingerprint. Crate-private: only verified paths
    /// (the address-notice receiver) edit a row in place.
    pub(crate) fn get_mut(&mut self, fingerprint: &str) -> Option<&mut FederationPeer> {
        self.peers.iter_mut().find(|p| p.fingerprint == fingerprint)
    }

    /// Insert (or replace, by fingerprint) a peer. Returns the fingerprint.
    pub fn upsert(&mut self, peer: FederationPeer) -> String {
        let fp = peer.fingerprint.clone();
        match self.peers.iter_mut().find(|p| p.fingerprint == fp) {
            Some(slot) => *slot = peer,
            None => self.peers.push(peer),
        }
        fp
    }

    /// Remove a peer by fingerprint. Returns true if one was removed.
    pub fn remove(&mut self, fingerprint: &str) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| p.fingerprint != fingerprint);
        self.peers.len() != before
    }

    /// Set a peer's trust state. Returns false if the peer is unknown.
    pub fn set_trust(&mut self, fingerprint: &str, trust: PeerTrust) -> bool {
        match self.peers.iter_mut().find(|p| p.fingerprint == fingerprint) {
            Some(p) => {
                p.trust = trust;
                true
            }
            None => false,
        }
    }

    /// Grant a capability to a peer. Returns false if the peer is unknown.
    pub fn grant(&mut self, fingerprint: &str, capability: impl Into<String>) -> bool {
        match self.peers.iter_mut().find(|p| p.fingerprint == fingerprint) {
            Some(p) => {
                p.capabilities.insert(capability.into());
                true
            }
            None => false,
        }
    }

    /// Revoke a capability from a peer. Returns false if the peer is unknown.
    pub fn revoke_capability(&mut self, fingerprint: &str, capability: &str) -> bool {
        match self.peers.iter_mut().find(|p| p.fingerprint == fingerprint) {
            Some(p) => {
                p.capabilities.remove(capability);
                true
            }
            None => false,
        }
    }

    /// Bump a peer's revocation epoch (instant revoke, restart-surviving once
    /// saved). Returns the new epoch, or None if the peer is unknown.
    pub fn bump_epoch(&mut self, fingerprint: &str) -> Option<u64> {
        match self.peers.iter_mut().find(|p| p.fingerprint == fingerprint) {
            Some(p) => {
                p.epoch = p.epoch.saturating_add(1);
                Some(p.epoch)
            }
            None => None,
        }
    }

    /// **The fail-closed authorization gate.** Returns the peer ONLY when it is
    /// pinned, `Trusted`, AND granted `capability`. Every other path is a DENY.
    ///
    /// This is the seam later phases call before acting on any cross-server
    /// stimulus — NEVER `token_ok`/`token_is_owner` (DECISION-2). Default-deny:
    /// an unknown peer, a `Pending`/`Blocked` peer, and a `Trusted` peer lacking
    /// the capability all return `Err`.
    pub fn require_peer(
        &self,
        fingerprint: &str,
        capability: &str,
    ) -> Result<&FederationPeer, RequirePeerError> {
        let peer = self.get(fingerprint).ok_or(RequirePeerError::UnknownPeer)?;
        if peer.trust != PeerTrust::Trusted {
            return Err(RequirePeerError::NotTrusted(peer.trust));
        }
        if !peer.capabilities.contains(capability) {
            return Err(RequirePeerError::CapabilityDenied {
                capability: capability.to_string(),
            });
        }
        Ok(peer)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Key helpers (shared with `envelope`)
// ─────────────────────────────────────────────────────────────────────

/// SHA-256(SPKI DER) as lowercase hex — the authoritative peer fingerprint.
pub fn fingerprint_of_spki_der(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// SHA-256 fingerprint of a PEM-encoded SPKI public key.
pub fn fingerprint_of_spki_pem(pem: &str) -> Result<String, String> {
    let der = spki_pem_to_der(pem)?;
    Ok(fingerprint_of_spki_der(&der))
}

/// This daemon's own federation fingerprint, derived from the federation
/// identity key ([`crate::federation::identity::load_or_create`]).
pub fn local_fingerprint() -> Result<String, String> {
    crate::federation::identity::fingerprint()
}

/// Decode a PEM `PUBLIC KEY` (SPKI) block to its DER bytes. Fails loud on a
/// missing/empty body — a malformed pin must not silently verify as empty.
pub(crate) fn spki_pem_to_der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_whitespace())
        .collect();
    if body.is_empty() {
        return Err("empty SPKI PEM (no base64 body)".to_string());
    }
    base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .map_err(|e| format!("decode SPKI base64: {e}"))
}

/// Extract the raw uncompressed EC point (`0x04 || X || Y`, 65 bytes for
/// P-256) from a P-256 SPKI DER, as aws-lc-rs's `UnparsedPublicKey` expects for
/// ECDSA verification. A P-256 SPKI is a fixed 26-byte ASN.1 prefix followed by
/// the 65-byte point in the trailing BIT STRING, so we take the final 65 bytes
/// and assert the uncompressed-point tag — rejecting anything that isn't a
/// well-formed P-256 public key rather than feeding garbage to the verifier.
pub(crate) fn raw_point_from_spki_der(der: &[u8]) -> Result<Vec<u8>, String> {
    const POINT_LEN: usize = 65; // 0x04 || 32-byte X || 32-byte Y
    if der.len() < POINT_LEN {
        return Err(format!(
            "SPKI DER too short ({} bytes) to hold a P-256 point",
            der.len()
        ));
    }
    let point = &der[der.len() - POINT_LEN..];
    if point[0] != 0x04 {
        return Err(format!(
            "pinned key is not an uncompressed P-256 point (tag 0x{:02x}, expected 0x04)",
            point[0]
        ));
    }
    Ok(point.to_vec())
}

#[cfg(unix)]
fn restrict_mode(file: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 0600 {}: {e}", file.display()))
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;
    use crate::federation::identity::load_or_create;

    /// Mint a fresh, unrelated SPKI PEM to act as a "peer" public key.
    fn fresh_peer_pem() -> String {
        let kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .expect("generate peer keypair");
        kp.public_key_pem()
    }

    #[test]
    fn fingerprint_is_stable_and_key_bound() {
        let pem = fresh_peer_pem();
        let fp1 = fingerprint_of_spki_pem(&pem).expect("fp");
        let fp2 = fingerprint_of_spki_pem(&pem).expect("fp again");
        assert_eq!(fp1, fp2, "fingerprint of the same key must be stable");
        assert_eq!(fp1.len(), 64, "SHA-256 hex is 64 chars");
        let other = fingerprint_of_spki_pem(&fresh_peer_pem()).expect("fp other");
        assert_ne!(fp1, other, "different keys must fingerprint differently");
    }

    #[test]
    fn pin_derives_fingerprint_from_key() {
        let pem = fresh_peer_pem();
        let peer = FederationPeer::pin("rosson@laptop", "rosson", pem.clone()).expect("pin");
        assert_eq!(peer.fingerprint, fingerprint_of_spki_pem(&pem).unwrap());
        assert_eq!(peer.trust, PeerTrust::Pending, "new peers start Pending");
        assert!(peer.capabilities.is_empty(), "new peers have no caps");
    }

    #[test]
    fn require_peer_default_denies_unknown_peer() {
        let store = PeerStore::default();
        let err = store
            .require_peer("deadbeef", "inbound")
            .expect_err("unknown peer must be denied");
        assert_eq!(err, RequirePeerError::UnknownPeer);
    }

    #[test]
    fn require_peer_denies_pending_and_ungranted_capability() {
        let pem = fresh_peer_pem();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("p", "p", pem).expect("pin"));

        // Pending peer: denied even with no capability asked beyond existence.
        match store.require_peer(&fp, "inbound") {
            Err(RequirePeerError::NotTrusted(PeerTrust::Pending)) => {}
            other => panic!("pending peer must be NotTrusted, got {other:?}"),
        }

        // Promote to Trusted but grant NOTHING → capability still default-denied.
        assert!(store.set_trust(&fp, PeerTrust::Trusted));
        match store.require_peer(&fp, "inbound") {
            Err(RequirePeerError::CapabilityDenied { capability }) => {
                assert_eq!(capability, "inbound");
            }
            other => panic!("ungranted capability must be denied, got {other:?}"),
        }

        // Grant the capability → now (and only now) it passes.
        assert!(store.grant(&fp, "inbound"));
        let peer = store
            .require_peer(&fp, "inbound")
            .expect("granted → allowed");
        assert_eq!(peer.fingerprint, fp);

        // A DIFFERENT capability is still denied (grants are per-capability).
        assert!(matches!(
            store.require_peer(&fp, "roster"),
            Err(RequirePeerError::CapabilityDenied { .. })
        ));
    }

    #[test]
    fn require_peer_denies_blocked_peer() {
        let pem = fresh_peer_pem();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("p", "p", pem).expect("pin"));
        store.grant(&fp, "inbound");
        store.set_trust(&fp, PeerTrust::Blocked);
        assert!(matches!(
            store.require_peer(&fp, "inbound"),
            Err(RequirePeerError::NotTrusted(PeerTrust::Blocked))
        ));
    }

    #[test]
    fn store_persist_reload_round_trip() {
        with_temp_home(|| {
            let pem = fresh_peer_pem();
            let mut store = PeerStore::default();
            let fp =
                store.upsert(FederationPeer::pin("rosson@laptop", "rosson", pem).expect("pin"));
            store.set_trust(&fp, PeerTrust::Trusted);
            store.grant(&fp, "inbound");
            store.grant(&fp, "roster");
            store.save().expect("save");

            // 0600 perms on the persisted store.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(store_path())
                    .expect("stat store")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600, "store must be 0600, got {mode:o}");
            }

            let reloaded = PeerStore::load().expect("reload");
            let peer = reloaded.get(&fp).expect("peer survives reload");
            assert_eq!(peer.trust, PeerTrust::Trusted);
            assert!(peer.base_url.is_empty(), "legacy pin has empty base_url");
            assert!(peer.capabilities.contains("inbound"));
            assert!(peer.capabilities.contains("roster"));
            // The gate still works against the reloaded store.
            assert!(reloaded.require_peer(&fp, "inbound").is_ok());
        });
    }

    #[test]
    fn store_persist_reload_keeps_base_url() {
        with_temp_home(|| {
            let pem = fresh_peer_pem();
            let mut store = PeerStore::default();
            let fp = store.upsert(
                FederationPeer::pin("lan", "192.168.1.50:38471", pem)
                    .expect("pin")
                    .with_base_url("http://192.168.1.50:38471"),
            );
            store.save().expect("save");
            let peer = PeerStore::load().expect("reload").get(&fp).unwrap().clone();
            assert_eq!(peer.base_url, "http://192.168.1.50:38471");
            assert_eq!(peer.subdomain, "192.168.1.50:38471");
        });
    }

    #[test]
    fn old_store_json_without_base_url_loads() {
        with_temp_home(|| {
            let pem = fresh_peer_pem();
            let fp = fingerprint_of_spki_pem(&pem).unwrap();
            let raw = format!(
                r#"{{"version":1,"peers":[{{"fingerprint":"{fp}","label":"legacy","subdomain":"rpm","public_key_pem":{pem},"trust":"pending","capabilities":[],"epoch":0,"added_at":"2026-01-01T00:00:00Z"}}]}}"#,
                fp = fp,
                pem = serde_json::to_string(&pem).unwrap(),
            );
            std::fs::create_dir_all(store_path().parent().unwrap()).unwrap();
            std::fs::write(store_path(), raw).unwrap();
            let loaded = PeerStore::load().expect("v1 peer without base_url must load");
            let p = loaded.get(&fp).expect("peer");
            assert!(p.base_url.is_empty());
            assert_eq!(p.subdomain, "rpm");
        });
    }

    #[test]
    fn unknown_store_version_fails_closed() {
        with_temp_home(|| {
            let bad = r#"{"version":999,"peers":[]}"#;
            std::fs::create_dir_all(store_path().parent().unwrap()).unwrap();
            std::fs::write(store_path(), bad).unwrap();
            let err = PeerStore::load().expect_err("unknown version must fail closed");
            assert!(err.contains("unknown version"), "got: {err}");
        });
    }

    #[test]
    fn bump_epoch_increments() {
        let pem = fresh_peer_pem();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("p", "p", pem).expect("pin"));
        assert_eq!(store.get(&fp).unwrap().epoch, 0);
        assert_eq!(store.bump_epoch(&fp), Some(1));
        assert_eq!(store.bump_epoch(&fp), Some(2));
        assert_eq!(store.bump_epoch("nope"), None);
    }

    #[test]
    fn local_fingerprint_matches_the_federation_identity() {
        with_temp_home(|| {
            let kp = load_or_create().expect("keypair");
            let expected = fingerprint_of_spki_der(&kp.public_key_der());
            let got = local_fingerprint().expect("local fp");
            assert_eq!(got, expected, "local fp must derive from the federation identity");
        });
    }

    #[test]
    fn concurrent_transactions_lose_no_update() {
        with_temp_home(|| {
            let pems: Vec<String> = (0..16).map(|_| fresh_peer_pem()).collect();
            let handles: Vec<_> = pems
                .clone()
                .into_iter()
                .enumerate()
                .map(|(i, pem)| {
                    std::thread::spawn(move || {
                        PeerStore::transact(|s| {
                            let peer = FederationPeer::pin(format!("p{i}"), format!("p{i}"), pem)?;
                            Ok::<_, String>((s.upsert(peer), true))
                        })
                        .expect("load/save")
                        .expect("tx")
                    })
                })
                .collect();
            let fps: Vec<String> = handles.into_iter().map(|h| h.join().expect("join")).collect();
            let store = PeerStore::load().expect("load");
            assert_eq!(store.list().len(), 16, "every concurrent writer's row must survive");
            for fp in &fps {
                assert!(store.get(fp).is_some(), "lost update for {fp}");
            }
            let leftovers: Vec<String> = fs::read_dir(k2_dir())
                .expect("readdir")
                .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("federation-peers.json.tmp"))
                .collect();
            assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
        });
    }

    #[test]
    fn a_transaction_that_does_not_ask_to_write_writes_nothing() {
        with_temp_home(|| {
            let out = PeerStore::transact(|s| {
                s.upsert(FederationPeer::pin("p", "p", fresh_peer_pem()).expect("pin"));
                Ok::<_, String>(((), false))
            })
            .expect("io");
            assert_eq!(out, Ok(()));
            assert!(!store_path().exists(), "no write requested → no file");
            let err = PeerStore::transact(|s| {
                s.upsert(FederationPeer::pin("p", "p", fresh_peer_pem()).expect("pin"));
                Err::<((), bool), _>("refused".to_string())
            })
            .expect("io");
            assert_eq!(err, Err("refused".to_string()));
            assert!(!store_path().exists(), "an error writes nothing");
        });
    }
}
