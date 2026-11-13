//! The owner's roster of node keys, signed with the owner's Ed25519 roster
//! key, and the `{body, sig}` form every signed document travels in.
//!
//! A verifier pins the roster key's fingerprint (SHA-256 of the 32 raw
//! public key bytes, 64 hex), got from the owner out of band. The roster
//! names its own key; it counts only when that key has the pinned
//! fingerprint and the signature verifies with it.

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::dsse::NodeKey;
use crate::util::{digest, is_digest, is_fp, sha256_hex, signed_bytes, unb64};

/// The domain the roster key signs rosters under.
pub const DOMAIN_NODE_ROSTER: &str = "k2-node-roster-v1";
/// The roster version this crate reads.
pub const NODE_ROSTER_V: u32 = 1;
/// Most entries in one roster.
pub const MAX_ENTRIES: usize = 1024;
/// Largest signed body.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// A signed document: `body` is the canonical JSON exactly as signed, `sig`
/// base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    /// The canonical JSON text.
    pub body: String,
    /// Base64 signature over `domain ‖ 0x00 ‖ len(body) ‖ body`.
    pub sig: String,
}

impl Signed {
    /// `sha256:<hex>` of the body: how documents name each other.
    pub fn digest(&self) -> String {
        digest(&self.body)
    }

    /// Read a `{body, sig}` file.
    pub fn from_json(what: &str, text: &str) -> Result<Self, String> {
        let s: Signed = serde_json::from_str(text).map_err(|e| format!("{what}: not a {{body, sig}} document: {e}"))?;
        if s.body.len() > MAX_BODY_BYTES {
            return Err(format!("{what}: body over {MAX_BODY_BYTES} bytes"));
        }
        Ok(s)
    }
}

/// An owner's Ed25519 roster key, `ed25519:<base64 of 32 bytes>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterKey(VerifyingKey);

impl RosterKey {
    /// Parse `ed25519:<b64>`.
    pub fn parse(s: &str) -> Option<Self> {
        let raw = unb64(s.strip_prefix("ed25519:")?).ok()?;
        let arr: [u8; 32] = raw.try_into().ok()?;
        VerifyingKey::from_bytes(&arr).ok().map(Self)
    }

    /// SHA-256 of the 32 raw bytes, 64 hex: what a verifier pins.
    pub fn fingerprint(&self) -> String {
        sha256_hex(self.0.as_bytes())
    }

    fn verify(&self, msg: &[u8], sig_b64: &str) -> bool {
        let Ok(raw) = unb64(sig_b64) else {
            return false;
        };
        let Ok(arr) = <[u8; 64]>::try_from(raw.as_slice()) else {
            return false;
        };
        self.0.verify_strict(msg, &Signature::from_bytes(&arr)).is_ok()
    }
}

/// A roster entry's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStatus {
    /// In use.
    Active,
    /// Rotated away or decommissioned: statements that started after
    /// `valid_to` are void.
    Retired,
    /// Every statement from the key is void.
    Revoked,
}

/// Why a key was revoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum RevokeReason {
    Compromised,
    KeyLost,
    Other,
}

/// Why the owner treats a key as untainted (never used to run a job outside
/// the job wall).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaintBasis {
    /// The node's self-statement says so, and the flag existed since the key was made.
    SelfClean,
    /// The key was made behind the wall by a re-key.
    Rekeyed,
    /// The owner vouched with a signed attestation.
    OwnerAttested,
}

/// The owner's roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct NodeRoster {
    pub v: u32,
    /// The owner server's name.
    pub owner_id: String,
    pub owner_server_fp: String,
    /// `ed25519:<b64>`, the key that signs this roster.
    pub roster_key: String,
    /// Strictly rising per owner, from 1.
    pub seq: u64,
    /// Unix seconds, the owner's clock.
    pub issued_at: i64,
    /// The previous roster's digest; `null` exactly at seq 1.
    pub prev_digest: Option<String>,
    /// Sorted by `node_fp`.
    pub entries: Vec<RosterEntry>,
}

/// One node key in the roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct RosterEntry {
    pub node_fp: String,
    /// P-256 SPKI, base64 DER.
    pub node_spki: String,
    /// The node's stable name, the same across re-keys.
    pub name: String,
    pub key_epoch: u32,
    /// The self-statement the owner approved for this key.
    pub statement_digest: String,
    pub status: KeyStatus,
    pub revoked_reason: Option<RevokeReason>,
    /// Statements that started before this don't count for this key.
    pub valid_from: i64,
    /// Set for a retired key.
    pub valid_to: Option<i64>,
    /// `null`: the owner vouches for nothing about this key.
    pub taint_basis: Option<TaintBasis>,
    pub attestation_digest: Option<String>,
    /// The owner's label for the physical machine.
    pub host: String,
    pub provider: Option<String>,
    pub site: Option<String>,
    pub admin_reach: Vec<String>,
    pub pod: Option<String>,
    pub point: Option<u32>,
}

/// A name: 1–63 bytes of `[a-z0-9._-]`, starting with a letter or digit.
pub fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-')
}

fn name(what: &str, s: &str) -> Result<(), String> {
    if is_name(s) {
        Ok(())
    } else {
        Err(format!("{what}: {s:?} is not a name"))
    }
}

impl NodeRoster {
    /// Verify `s` with the roster key whose fingerprint is `pinned_fp`, then
    /// parse and check it. The roster must name that key.
    pub fn open(s: &Signed, pinned_fp: &str) -> Result<Self, String> {
        // The body names its key: read it to find the key, check the key is
        // the pinned one, verify, and only then trust anything in it.
        let peek: serde_json::Value = serde_json::from_str(&s.body).map_err(|e| format!("roster: {e}"))?;
        let key_text = peek.get("roster_key").and_then(|v| v.as_str()).ok_or("roster: no roster_key")?;
        let key = RosterKey::parse(key_text).ok_or("roster: roster_key is not ed25519:<32 bytes base64>")?;
        if key.fingerprint() != pinned_fp {
            return Err(format!("roster: signed by roster key {}, not the pinned owner key {pinned_fp}", key.fingerprint()));
        }
        if !key.verify(&signed_bytes(DOMAIN_NODE_ROSTER, &[s.body.as_bytes()]), &s.sig) {
            return Err("roster: the signature does not verify with the owner's roster key".into());
        }
        let r: NodeRoster = crate::canonical::parse_exact("roster", s.body.as_bytes())?;
        r.check().map_err(|e| format!("roster: {e}"))?;
        Ok(r)
    }

    /// The shape rules.
    pub fn check(&self) -> Result<(), String> {
        if self.v != NODE_ROSTER_V {
            return Err(format!("v{} (this verifier reads v{NODE_ROSTER_V})", self.v));
        }
        name("owner_id", &self.owner_id)?;
        if !is_fp(&self.owner_server_fp) {
            return Err("owner_server_fp is not 64 hex".into());
        }
        if self.seq == 0 {
            return Err("seq starts at 1".into());
        }
        match (&self.prev_digest, self.seq) {
            (None, 1) => {}
            (Some(d), s) if s > 1 && is_digest(d) => {}
            _ => return Err("prev_digest is a sha256 digest exactly when seq > 1".into()),
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(format!("over {MAX_ENTRIES} entries"));
        }
        if !self.entries.windows(2).all(|w| w[0].node_fp < w[1].node_fp) {
            return Err("entries must be sorted by node_fp and unique".into());
        }
        let mut active: Vec<&str> = self.entries.iter().filter(|e| e.status == KeyStatus::Active).map(|e| e.name.as_str()).collect();
        active.sort();
        if active.windows(2).any(|w| w[0] == w[1]) {
            return Err("two active keys for one node name".into());
        }
        self.entries.iter().try_for_each(RosterEntry::check)
    }

    /// The entry for a node key.
    pub fn entry(&self, node_fp: &str) -> Option<&RosterEntry> {
        self.entries.iter().find(|e| e.node_fp == node_fp)
    }
}

impl RosterEntry {
    /// The shape rules of one entry.
    pub fn check(&self) -> Result<(), String> {
        let at = format!("entry {}", self.name);
        name("entry name", &self.name)?;
        self.node_key().map_err(|e| format!("{at}: {e}"))?;
        if !is_digest(&self.statement_digest) {
            return Err(format!("{at}: statement_digest is not sha256:<64 hex>"));
        }
        match (self.status, self.revoked_reason) {
            (KeyStatus::Revoked, Some(_)) | (KeyStatus::Active | KeyStatus::Retired, None) => {}
            _ => return Err(format!("{at}: revoked_reason is set exactly when status is revoked")),
        }
        match (self.status, self.valid_to) {
            (KeyStatus::Active, Some(_)) => return Err(format!("{at}: an active key has no valid_to")),
            (KeyStatus::Retired, None) => return Err(format!("{at}: a retired key needs valid_to")),
            (_, Some(t)) if t < self.valid_from => return Err(format!("{at}: valid_to before valid_from")),
            _ => {}
        }
        match (self.taint_basis, &self.attestation_digest) {
            (Some(TaintBasis::OwnerAttested), Some(d)) if is_digest(d) => {}
            (Some(TaintBasis::OwnerAttested), _) => return Err(format!("{at}: owner_attested without its attestation digest")),
            (_, Some(_)) => return Err(format!("{at}: an attestation digest without owner_attested")),
            (_, None) => {}
        }
        name(&format!("{at}: host"), &self.host)?;
        for v in [&self.provider, &self.site, &self.pod].into_iter().flatten() {
            name(&at, v)?;
        }
        for a in &self.admin_reach {
            name(&format!("{at}: admin_reach"), a)?;
        }
        if !self.admin_reach.windows(2).all(|w| w[0] < w[1]) {
            return Err(format!("{at}: admin_reach must be sorted and unique"));
        }
        Ok(())
    }

    /// The node's public key, checked against `node_fp`.
    pub fn node_key(&self) -> Result<NodeKey, String> {
        if !is_fp(&self.node_fp) {
            return Err("node_fp is not 64 hex".into());
        }
        let der = unb64(&self.node_spki).map_err(|e| format!("node_spki: {e}"))?;
        let key = NodeKey::from_spki_der(&der).map_err(|e| format!("node_spki: {e}"))?;
        if key.fingerprint() != self.node_fp {
            return Err(format!("node_spki's fingerprint is not {}", self.node_fp));
        }
        Ok(key)
    }
}
