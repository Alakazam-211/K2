//! CI on compute for clients (protocol 9, prd-ci-on-compute-for-clients-v1
//! and its vs-live review; slice C-1). **The frozen formats** every other
//! slice builds against. Human-readable spec with the rules and golden
//! vectors: `docs/ci-formats-v1.md`; vectors in
//! `tests/release/fixtures/ci-formats-v1.json`.
//!
//! This module holds what the formats share: the protocol-9 offer features,
//! the signing domains, the enums, the one signed envelope ([`Signed`]) and
//! receipt v2's [`RunFacts`]. The documents themselves live in
//! [`crate::statement`] (node self-statement, rotation),
//! [`crate::node_roster`] (roster, seat, owner attestation),
//! [`crate::evidence`] (the bundle the signer checks),
//! [`crate::signer_policy`] and [`crate::ci_config`] (the two TOML files).
//!
//! **Signing rule (every document here).** The body is the document's
//! canonical JSON ([`crate::canonical`]) and travels as that exact text
//! ([`Signed::body`]); the signature covers
//! `crypto::signed_bytes(domain, [body])`. Node documents are signed with
//! the node key (P-256 ECDSA, SHA-256, DER, base64), owner documents with
//! the owner's node-roster key (Ed25519, raw 64 bytes, base64). A verifier
//! checks the signature over the received text, parses it with unknown
//! fields refused, and refuses a body that is not its own canonical form.
//!
//! **Formatting rule (new documents).** No floats. Every key is always
//! present: an unset optional is `null`, an empty list `[]`. A new field is
//! a new `v` (verifiers refuse a `v` they don't know). The two pre-existing
//! structs that grow here, `Plan` and `Receipt`, keep their old rule
//! instead: new fields are absent when unset, so older bytes never change.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::admin::{ApprovalKey, ApprovalSigner};
use crate::crypto::{self, SigningKey};

// ── protocol 9: offer features ───────────────────────────────────────

/// The node signs a [`crate::statement::NodeStatement`] and sends it in its
/// offer (C0).
pub const FEATURE_NODE_STATEMENT: &str = "node-statement";
/// The node emits receipt v2 when `Plan.receipt_v = 2` (C0, V7).
pub const FEATURE_RECEIPT_V2: &str = "receipt-v2";
/// The node holds its owner's pinned roster key and relays the owner's
/// signed roster to every server it serves (C1, V11).
pub const FEATURE_NODE_ROSTER: &str = "node-roster";
/// Node and server understand key rotation (C1, V3). A node re-keys only
/// when every active server on it offers this.
pub const FEATURE_NODE_REKEY: &str = "node-rekey";
/// The node reads `/etc/k2-node-lanes.toml` and takes the lanes verb (C3a,
/// V22).
pub const FEATURE_LANES: &str = "lanes";
/// Secret jobs may run as a per-job uid from the slot's pool (C2, V15).
pub const FEATURE_JOB_UID: &str = "job-uid";
/// `trust team` tenants may get secret env under lane rules (C3b, V24).
pub const FEATURE_LANE_SECRETS: &str = "lane-secrets";
/// The controller may delete a secret job's artifacts after its fetch (V27).
pub const FEATURE_ARTIFACT_DELETE: &str = "artifact-delete";

/// Every protocol-9 feature. A node offers one only to a server that said
/// protocol ≥ [`crate::frames::PROTOCOL_CI`]; a server sends a field or
/// frame only to a node offering its feature.
pub const FEATURES: &[&str] = &[
    FEATURE_NODE_STATEMENT,
    FEATURE_RECEIPT_V2,
    FEATURE_NODE_ROSTER,
    FEATURE_NODE_REKEY,
    FEATURE_LANES,
    FEATURE_JOB_UID,
    FEATURE_LANE_SECRETS,
    FEATURE_ARTIFACT_DELETE,
];

// ── signing domains ──────────────────────────────────────────────────

/// Node self-statement, node key (P-256).
pub const DOMAIN_NODE_STATEMENT: &str = "k2-node-statement-v1";
/// Key rotation, signed by the **old** node key (P-256).
pub const DOMAIN_NODE_ROTATION: &str = "k2-node-rotation-v1";
/// The owner's node roster, node-roster key (Ed25519). Never bare
/// "roster": federation owns that word (vs-live B8).
pub const DOMAIN_NODE_ROSTER: &str = "k2-node-roster-v1";
/// A client's seat, node-roster key (Ed25519).
pub const DOMAIN_NODE_SEAT: &str = "k2-node-seat-v1";
/// The owner's vouch for an existing key instead of a re-key (V12),
/// node-roster key (Ed25519).
pub const DOMAIN_NODE_ATTEST: &str = "k2-node-attest-v1";
/// The SAS between `k2 signer pair` and `k2 ci signer add` (V29). Same
/// scheme as [`crate::pairing`], its own domain.
pub const DOMAIN_SIGNER_SAS: &str = "k2-signer-sas-v1";
/// Receipt v2 keeps v1's domain ([`crate::frames::DOMAIN_RECEIPT`]): the
/// domain names the document (a receipt), the signed `v` names its schema.
pub const DOMAIN_RECEIPT: &str = crate::frames::DOMAIN_RECEIPT;

/// The vault engine key that signs owner documents:
/// `engine_sign(ROSTER_KEY_ENGINE, ROSTER_KEY_NAME, signed_bytes(…))`.
pub const ROSTER_KEY_ENGINE: &str = "compute";
pub const ROSTER_KEY_NAME: &str = "node-roster";

/// Largest signed body of any document here.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// Stable refusal codes named by the PRD and its review, frozen here so
/// every slice spells them the same.
pub mod codes {
    pub const NO_INDEPENDENT_NODES: &str = "no_independent_nodes";
    pub const SEAT_DEGRADED: &str = "seat_degraded";
    pub const SEAT_MOVING: &str = "seat_moving";
    pub const NO_FREE_SEAT: &str = "no_free_seat";
    pub const NODE_CLIENT_CAP: &str = "node_client_cap";
    pub const NODE_KEY_TAINTED: &str = "node_key_tainted";
    pub const REKEY_BLOCKED_OLD_SERVER: &str = "rekey_blocked_old_server";
    pub const SECRET_OWNER_NOT_TRUSTED: &str = "secret_owner_not_trusted";
    pub const MAC_SEAT_NOT_OFFERED: &str = "mac_seat_not_offered";
    pub const ROSTER_STALE: &str = "node_roster_stale";
    pub const ROSTER_REPLAYED: &str = "node_roster_replayed";
}

// ── enums ────────────────────────────────────────────────────────────

/// The job wall (`k2-node` `wall.rs` labels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wall {
    /// Linux: jobs run as a separate user in a transient unit.
    JobUser,
    /// macOS: `sandbox-exec`, one uid for every tenant.
    Seatbelt,
    /// No wall (`--dev`, executor not ready, `--no-job-wall`).
    None,
}

/// How a job was kept apart from other jobs (receipt v2, signer policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// A uid from the slot's pool, held by this job alone (C2).
    JobUid,
    /// Nothing else ran on the node while it ran (node-wide, vs-live B7).
    Exclusive,
    /// The tenant's one `k2job-<slot>` uid, shared with its other jobs.
    TenantUid,
    /// One uid for every tenant (macOS Seatbelt, `--dev`).
    SharedUid,
}

/// The node's own taint flag for its current key (V10). Self-reported: it
/// catches mistakes, not a hostile root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Taint {
    /// No job ever started under `Wall::None` with this key.
    Clean,
    /// One did. Never cleared for this key.
    Tainted,
    /// The key predates the flag. Lanes treat it as tainted unless the owner
    /// attests ([`TaintBasis::OwnerAttested`]).
    Unknown,
}

/// Where the node key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStore {
    /// `state/node-key.pem`.
    File,
    /// macOS keychain.
    Keychain,
}

/// A roster entry's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStatus {
    Active,
    /// Decommissioned or rotated away: voids receipts started after
    /// `valid_to`.
    Retired,
    /// Voids every receipt from the key, past and future, when the reason is
    /// `compromised`.
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokeReason {
    Compromised,
    KeyLost,
    Other,
}

/// Why the owner's roster treats a key as untainted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaintBasis {
    /// The key's self-statement says `clean` (the flag existed since the
    /// key was made).
    SelfClean,
    /// The key was made under the wall by `rekey`.
    Rekeyed,
    /// The owner vouched with a signed [`crate::node_roster::OwnerAttestation`]
    /// (V12): existing nodes, without a re-key.
    OwnerAttested,
}

/// Why a node key rotated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationReason {
    Scheduled,
    /// The owner asked (`k2 compute node rekey`).
    Owner,
    /// Re-key under the wall to clear `tainted` / `unknown`.
    Wall,
    /// Suspected compromise (the owner revokes the old key too).
    Compromise,
}

/// A seat's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatState {
    Offered,
    Active,
    Suspended,
    Ended,
}

/// What two nodes must differ in for a dual build (PRD §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Node,
    Host,
    Site,
    Provider,
    AdminReach,
    Owner,
}

// ── the signed envelope ──────────────────────────────────────────────

/// A signed document: `body` is the canonical JSON text exactly as signed,
/// `sig` base64 (P-256 DER for node documents, raw Ed25519 for owner
/// documents). Kept as text so verification never re-serializes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    pub body: String,
    pub sig: String,
}

/// `sha256:<hex>` of a body's bytes: how documents name each other.
pub fn digest(body: &str) -> String {
    format!("sha256:{}", crypto::sha256_hex(body.as_bytes()))
}

impl Signed {
    /// Sign `doc` with a node key under `domain`.
    pub fn sign_node<T: Serialize>(key: &SigningKey, domain: &str, doc: &T) -> Result<Self, String> {
        let body = crate::canonical::to_string(doc)?;
        let sig = key.sign_domain(domain, &[body.as_bytes()])?;
        Ok(Self { body, sig })
    }

    /// Sign `doc` with an owner node-roster key under `domain` (tests and
    /// any path holding a seed; the daemon signs through the vault engine
    /// over [`owner_message`]).
    pub fn sign_owner<T: Serialize>(key: &ApprovalSigner, domain: &str, doc: &T) -> Result<Self, String> {
        let body = crate::canonical::to_string(doc)?;
        let sig = key.sign(&owner_message(domain, &body));
        Ok(Self { body, sig: crypto::b64(&sig) })
    }

    /// From an Ed25519 signature the vault made over [`owner_message`].
    pub fn from_owner_sig(body: String, sig: &[u8; 64]) -> Self {
        Self { body, sig: crypto::b64(sig) }
    }

    pub fn digest(&self) -> String {
        digest(&self.body)
    }

    /// Verify with a node key, then parse. Refuses a bad signature, unknown
    /// fields and a body that is not its own canonical form.
    pub fn open_node<T: Serialize + DeserializeOwned>(&self, spki_der: &[u8], domain: &str) -> Result<T, String> {
        self.size_ok()?;
        if !crypto::verify_domain(spki_der, domain, &[self.body.as_bytes()], &self.sig) {
            return Err(format!("{domain}: the signature does not verify with that node key"));
        }
        self.parse(domain)
    }

    /// Verify with an owner node-roster key, then parse (as [`Self::open_node`]).
    pub fn open_owner<T: Serialize + DeserializeOwned>(&self, key: &ApprovalKey, domain: &str) -> Result<T, String> {
        self.size_ok()?;
        let sig = crypto::unb64(&self.sig).map_err(|e| format!("{domain}: signature {e}"))?;
        if sig.len() != 64 || !key.verify(&owner_message(domain, &self.body), &sig) {
            return Err(format!("{domain}: the signature does not verify with that roster key"));
        }
        self.parse(domain)
    }

    /// Parse without verifying: only to find which key to verify with
    /// (a self-statement names its own key). Never trust the result.
    pub fn peek<T: Serialize + DeserializeOwned>(&self, domain: &str) -> Result<T, String> {
        self.size_ok()?;
        self.parse(domain)
    }

    fn size_ok(&self) -> Result<(), String> {
        if self.body.len() > MAX_BODY_BYTES {
            return Err(format!("signed body over {MAX_BODY_BYTES} bytes"));
        }
        Ok(())
    }

    fn parse<T: Serialize + DeserializeOwned>(&self, domain: &str) -> Result<T, String> {
        let doc: T = serde_json::from_str(&self.body).map_err(|e| format!("{domain}: {e}"))?;
        if crate::canonical::to_string(&doc)? != self.body {
            return Err(format!("{domain}: the body is not its canonical form"));
        }
        Ok(doc)
    }
}

/// The bytes an owner's node-roster key signs for a body under `domain`.
pub fn owner_message(domain: &str, body: &str) -> Vec<u8> {
    crypto::signed_bytes(domain, &[body.as_bytes()])
}

// ── receipt v2 ───────────────────────────────────────────────────────

/// Receipt v2's facts (`Receipt.run`): how and where the job ran. All
/// signed with the receipt; every key always present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFacts {
    pub isolation: Isolation,
    /// The uid pool index for [`Isolation::JobUid`] (V15); `null` otherwise.
    pub uid_index: Option<u32>,
    pub wall: Wall,
    /// The tenant slot the job ran in (0 = the node's owner server).
    pub tenant_slot: u32,
    pub k2node_version: String,
    /// The k2-node build commit (V8 marker), 40 hex; `null` without one.
    pub k2node_commit: Option<String>,
    /// The signing key's epoch (0 = the enrolment key).
    pub key_epoch: u32,
    /// [`digest`] of the node's self-statement in force when the job
    /// started (it names the key, wall, taint, version).
    pub statement_digest: String,
    /// The highest owner roster `seq` the node holds; `null` before it holds
    /// one.
    pub roster_seq_seen: Option<u64>,
    /// The lane the job ran under; `null` for a job outside any lane.
    pub lane: Option<String>,
}

impl RunFacts {
    pub fn check(&self) -> Result<(), String> {
        match (self.isolation, self.uid_index) {
            (Isolation::JobUid, None) => return Err("run: job_uid without a uid_index".into()),
            (Isolation::JobUid, Some(_)) | (_, None) => {}
            (_, Some(_)) => return Err("run: a uid_index without job_uid".into()),
        }
        if self.isolation == Isolation::JobUid && self.wall != Wall::JobUser {
            return Err("run: job_uid needs the job_user wall".into());
        }
        check_version_text("run.k2node_version", &self.k2node_version)?;
        if let Some(c) = &self.k2node_commit {
            check_commit("run.k2node_commit", c)?;
        }
        check_digest("run.statement_digest", &self.statement_digest)?;
        if let Some(l) = &self.lane {
            check_name("run.lane", l)?;
        }
        Ok(())
    }
}

// ── shared field checks ──────────────────────────────────────────────

/// 64 lowercase hex (a key fingerprint).
pub fn is_fp(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn check_fp(what: &str, s: &str) -> Result<(), String> {
    if is_fp(s) {
        Ok(())
    } else {
        Err(format!("{what}: not 64 lowercase hex"))
    }
}

pub(crate) fn check_commit(what: &str, s: &str) -> Result<(), String> {
    if s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(())
    } else {
        Err(format!("{what}: not a 40-hex commit"))
    }
}

pub(crate) fn check_digest(what: &str, s: &str) -> Result<(), String> {
    match s.strip_prefix("sha256:") {
        Some(h) if is_fp(h) => Ok(()),
        _ => Err(format!("{what}: not sha256:<64 hex>")),
    }
}

/// A name: 1–63 bytes of `[a-z0-9._-]`, starting with a letter or digit
/// (lanes, nodes, builds, projects, seats, owners' hosts).
pub fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-')
}

pub(crate) fn check_name(what: &str, s: &str) -> Result<(), String> {
    if is_name(s) {
        Ok(())
    } else {
        Err(format!("{what}: {s:?} is not a name ([a-z0-9._-], 1-63, starts alphanumeric)"))
    }
}

pub(crate) fn check_version_text(what: &str, s: &str) -> Result<(), String> {
    if !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'+') {
        Ok(())
    } else {
        Err(format!("{what}: not a version string"))
    }
}

/// Short free text (labels, notes): 0–`max` bytes, no control characters.
pub(crate) fn check_text(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.len() <= max && !s.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(format!("{what}: over {max} bytes or has control characters"))
    }
}

/// A P-256 SPKI in base64; returns the DER and checks `fp` names it.
pub(crate) fn check_spki(what: &str, spki_b64: &str, fp: &str) -> Result<Vec<u8>, String> {
    let der = crypto::unb64(spki_b64).map_err(|e| format!("{what}: {e}"))?;
    crypto::p256_point_from_spki(&der).map_err(|e| format!("{what}: {e}"))?;
    if crypto::fingerprint_of_spki_der(&der) != fp {
        return Err(format!("{what}: the key's fingerprint is not {fp}"));
    }
    Ok(der)
}

/// Sorted, unique (the canonical order of every set-like list).
pub(crate) fn check_sorted_unique<T: Ord>(what: &str, v: &[T]) -> Result<(), String> {
    if v.windows(2).all(|w| w[0] < w[1]) {
        Ok(())
    } else {
        Err(format!("{what}: must be sorted and unique"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_are_unique_and_new() {
        let mut f: Vec<&str> = FEATURES.to_vec();
        f.sort();
        f.dedup();
        assert_eq!(f.len(), FEATURES.len());
        // None collides with a protocol 1–8 feature.
        for old in [
            crate::admin::FEATURE_ADMIN,
            crate::share::FEATURE_SERVERS,
            crate::secret::FEATURE_SECRET_ENV,
            crate::gitcred::FEATURE_GIT_ACCESS,
            crate::frames::FEATURE_JOB_WALL,
        ] {
            assert!(!FEATURES.contains(&old), "{old}");
        }
    }

    #[test]
    fn protocol_ci_is_the_next_protocol() {
        assert_eq!(crate::frames::PROTOCOL_CI, 9);
        assert!(crate::frames::PROTOCOL_CI > crate::frames::PROTOCOL_GIT);
        assert!(crate::frames::PROTOCOL <= crate::frames::PROTOCOL_CI);
    }

    #[test]
    fn domains_are_distinct() {
        let d = [
            DOMAIN_NODE_STATEMENT,
            DOMAIN_NODE_ROTATION,
            DOMAIN_NODE_ROSTER,
            DOMAIN_NODE_SEAT,
            DOMAIN_NODE_ATTEST,
            DOMAIN_SIGNER_SAS,
            DOMAIN_RECEIPT,
            crate::admin::DOMAIN_CMD_OWNER,
            crate::admin::DOMAIN_PLAN_OWNER,
            crate::admin::DOMAIN_KEYS_OWNER,
            crate::pairing::SAS_DOMAIN_COMPUTE,
        ];
        let mut s = d.to_vec();
        s.sort();
        s.dedup();
        assert_eq!(s.len(), d.len());
    }

    #[test]
    fn signed_refuses_other_domain_other_key_and_non_canonical_bodies() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        #[serde(deny_unknown_fields)]
        struct D {
            a: u32,
            b: Option<String>,
        }
        let k = SigningKey::generate().unwrap();
        let s = Signed::sign_node(&k, DOMAIN_NODE_STATEMENT, &D { a: 1, b: None }).unwrap();
        assert_eq!(s.body, r#"{"a":1,"b":null}"#);
        assert_eq!(s.open_node::<D>(k.spki_der(), DOMAIN_NODE_STATEMENT).unwrap(), D { a: 1, b: None });
        assert!(s.open_node::<D>(k.spki_der(), DOMAIN_NODE_ROTATION).is_err());
        assert!(s.open_node::<D>(SigningKey::generate().unwrap().spki_der(), DOMAIN_NODE_STATEMENT).is_err());
        // Signed, but not canonical (whitespace): refused after the signature.
        let loose = Signed { body: r#"{"a":1, "b":null}"#.into(), sig: String::new() };
        let loose = Signed { sig: k.sign_domain(DOMAIN_NODE_STATEMENT, &[loose.body.as_bytes()]).unwrap(), ..loose };
        assert!(loose.open_node::<D>(k.spki_der(), DOMAIN_NODE_STATEMENT).unwrap_err().contains("canonical"));
        // Signed, but an unknown field: refused.
        let extra = r#"{"a":1,"b":null,"c":2}"#.to_string();
        let extra = Signed { sig: k.sign_domain(DOMAIN_NODE_STATEMENT, &[extra.as_bytes()]).unwrap(), body: extra };
        assert!(extra.open_node::<D>(k.spki_der(), DOMAIN_NODE_STATEMENT).is_err());
        // Owner side: Ed25519, deterministic.
        let o = ApprovalSigner::from_seed(&[9u8; 32]).unwrap();
        let so = Signed::sign_owner(&o, DOMAIN_NODE_ROSTER, &D { a: 2, b: Some("x".into()) }).unwrap();
        assert_eq!(so, Signed::sign_owner(&o, DOMAIN_NODE_ROSTER, &D { a: 2, b: Some("x".into()) }).unwrap());
        assert!(so.open_owner::<D>(&o.public(), DOMAIN_NODE_ROSTER).is_ok());
        assert!(so.open_owner::<D>(&o.public(), DOMAIN_NODE_SEAT).is_err());
        assert!(so.open_owner::<D>(&ApprovalSigner::from_seed(&[8u8; 32]).unwrap().public(), DOMAIN_NODE_ROSTER).is_err());
    }

    #[test]
    fn run_facts_rules() {
        let ok = RunFacts {
            isolation: Isolation::JobUid,
            uid_index: Some(3),
            wall: Wall::JobUser,
            tenant_slot: 1,
            k2node_version: "0.46.10".into(),
            k2node_commit: Some("a".repeat(40)),
            key_epoch: 0,
            statement_digest: format!("sha256:{}", "b".repeat(64)),
            roster_seq_seen: None,
            lane: Some("ci-standard".into()),
        };
        ok.check().unwrap();
        assert!(RunFacts { uid_index: None, ..ok.clone() }.check().is_err());
        assert!(RunFacts { isolation: Isolation::Exclusive, ..ok.clone() }.check().is_err());
        RunFacts { isolation: Isolation::Exclusive, uid_index: None, ..ok.clone() }.check().unwrap();
        assert!(RunFacts { wall: Wall::Seatbelt, ..ok.clone() }.check().is_err());
        assert!(RunFacts { statement_digest: "sha256:x".into(), ..ok.clone() }.check().is_err());
        assert!(RunFacts { k2node_commit: Some("A".repeat(40)), ..ok.clone() }.check().is_err());
        assert!(RunFacts { lane: Some("Bad Lane".into()), ..ok }.check().is_err());
    }

    #[test]
    fn names() {
        for ok in ["k2n7-vin", "ci-standard", "a", "rosson.k2.dev", "linux-x86_64"] {
            assert!(is_name(ok), "{ok}");
        }
        for bad in ["", "-a", "A", "a b", "a/b", &"a".repeat(64)] {
            assert!(!is_name(bad), "{bad:?}");
        }
    }
}
