//! The owner's node roster, seats and attestations (protocol 9, C1/C3a;
//! PRD §4.1, §4.3, vs-live V11–V13, V19–V21, B8). Formats only:
//! `docs/ci-formats-v1.md` §4–§6.
//!
//! All three are signed by the node owner's **node-roster key** (Ed25519,
//! vault engine `compute/node-roster`, [`crate::ci::ROSTER_KEY_NAME`]): a
//! sibling of the node-admin approval key, never the tunnel key or a
//! passport. Named "node roster" everywhere: federation owns bare "roster".
//!
//! - [`NodeRoster`]: one signed document per owner, `seq` strictly rising,
//!   with `issued_at` for freshness (V13: the signer keeps the high-water
//!   `seq` per owner and refuses a lower one, and refuses one older than
//!   its policy's `roster_max_age_hours`). `prev_digest` chains each
//!   roster to the one before it (the transparency log, C14, builds on it).
//! - [`SeatStatement`]: which of the owner's nodes one client's server may
//!   use, under which lane. One per client, so a client never sees another
//!   client's identity. Names nodes by their stable roster `name`, so a
//!   re-key needs a new roster but no new seat.
//! - [`OwnerAttestation`]: the owner's vouch that an existing key never ran
//!   a job without the wall (V12), the alternative to a re-key.

use serde::{Deserialize, Serialize};

use crate::admin::{ApprovalKey, ApprovalSigner};
use crate::ci::{self, KeyStatus, RevokeReason, SeatState, Signed, TaintBasis};

/// Version of [`NodeRoster`].
pub const NODE_ROSTER_V: u32 = 1;
/// Version of [`SeatStatement`].
pub const SEAT_V: u32 = 1;
/// Version of [`OwnerAttestation`].
pub const ATTESTATION_V: u32 = 1;
/// Most entries in one roster (keys, retired ones included).
pub const MAX_ENTRIES: usize = 1024;
/// Most nodes in one seat.
pub const MAX_SEAT_NODES: usize = 16;

/// The owner's roster of node keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRoster {
    /// [`NODE_ROSTER_V`].
    pub v: u32,
    /// The owner server's host (`rosson.k2.dev`).
    pub owner_id: String,
    /// The owner server's tunnel-key fingerprint (what its nodes pin).
    pub owner_server_fp: String,
    /// The key that signs this roster, `ed25519:<b64>`. Must be the pinned one.
    pub roster_key: String,
    /// Strictly rising per owner, from 1.
    pub seq: u64,
    pub issued_at: i64,
    /// [`ci::digest`] of the previous roster's body; `null` exactly at seq 1.
    pub prev_digest: Option<String>,
    /// One entry per key, sorted by `node_fp`. A rotated node keeps its old
    /// key's entry (retired) so old receipts stay checkable.
    pub entries: Vec<RosterEntry>,
}

/// One node key in the roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterEntry {
    pub node_fp: String,
    /// P-256 SPKI, base64 DER.
    pub node_spki: String,
    /// The node's stable name (`k2n7-vin`): the same across re-keys. At most
    /// one `active` entry per name.
    pub name: String,
    pub key_epoch: u32,
    /// [`ci::digest`] of the self-statement the owner approved for this key.
    pub statement_digest: String,
    pub status: KeyStatus,
    /// Set exactly when `status` is `revoked`.
    pub revoked_reason: Option<RevokeReason>,
    /// Receipts that started before this don't count for this entry.
    pub valid_from: i64,
    /// Set for `retired` (receipts that started after it are void); `null`
    /// while active.
    pub valid_to: Option<i64>,
    /// Why the owner treats this key as untainted; `null` = the owner vouches
    /// for nothing, and the key never counts for a CI lane (a key that ran
    /// without the wall, or a retired `unknown` key kept for old receipts).
    pub taint_basis: Option<TaintBasis>,
    /// [`ci::digest`] of the [`OwnerAttestation`]; set exactly when
    /// `taint_basis` is `owner_attested`.
    pub attestation_digest: Option<String>,
    /// The owner's word for the physical machine (VMs on one host share it).
    pub host: String,
    pub provider: Option<String>,
    pub site: Option<String>,
    /// Who holds root on the node (names, e.g. a fleet SSH key), sorted.
    pub admin_reach: Vec<String>,
    /// 0.46.11 pods: the pod and its point; `null` in 0.46.10.
    pub pod: Option<String>,
    pub point: Option<u32>,
}

impl NodeRoster {
    pub fn check(&self) -> Result<(), String> {
        if self.v != NODE_ROSTER_V {
            return Err(format!("node roster v{} (this build knows v{NODE_ROSTER_V})", self.v));
        }
        ci::check_name("node_roster.owner_id", &self.owner_id)?;
        ci::check_fp("node_roster.owner_server_fp", &self.owner_server_fp)?;
        ApprovalKey::parse(&self.roster_key).ok_or("node_roster.roster_key: not ed25519:<32 bytes b64>")?;
        if self.seq == 0 {
            return Err("node_roster.seq starts at 1".into());
        }
        match (&self.prev_digest, self.seq) {
            (None, 1) => {}
            (Some(d), s) if s > 1 => ci::check_digest("node_roster.prev_digest", d)?,
            _ => return Err("node_roster: prev_digest is set exactly when seq > 1".into()),
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(format!("node_roster: over {MAX_ENTRIES} entries"));
        }
        let fps: Vec<&str> = self.entries.iter().map(|e| e.node_fp.as_str()).collect();
        ci::check_sorted_unique("node_roster.entries (by node_fp)", &fps)?;
        let mut active: Vec<&str> = self.entries.iter().filter(|e| e.status == KeyStatus::Active).map(|e| e.name.as_str()).collect();
        active.sort();
        if active.windows(2).any(|w| w[0] == w[1]) {
            return Err("node_roster: two active keys for one node name".into());
        }
        for e in &self.entries {
            e.check()?;
        }
        Ok(())
    }

    /// The pinned key's fingerprint (what `k2 signer trust-owner` pins).
    pub fn key_fp(&self) -> Result<String, String> {
        Ok(ApprovalKey::parse(&self.roster_key).ok_or("node_roster.roster_key: bad key")?.fingerprint())
    }

    pub fn entry(&self, node_fp: &str) -> Option<&RosterEntry> {
        self.entries.iter().find(|e| e.node_fp == node_fp)
    }

    pub fn sign(&self, key: &ApprovalSigner) -> Result<Signed, String> {
        self.check()?;
        if key.public().encode() != self.roster_key {
            return Err("node_roster: signed by a key that is not roster_key".into());
        }
        Signed::sign_owner(key, ci::DOMAIN_NODE_ROSTER, self)
    }

    /// Open with the key the verifier pinned; the roster must name it.
    pub fn open(s: &Signed, pinned: &ApprovalKey) -> Result<Self, String> {
        let r: NodeRoster = s.open_owner(pinned, ci::DOMAIN_NODE_ROSTER)?;
        r.check()?;
        if r.roster_key != pinned.encode() {
            return Err("node_roster: roster_key is not the verifying key".into());
        }
        Ok(r)
    }
}

impl RosterEntry {
    pub fn check(&self) -> Result<(), String> {
        let at = format!("node_roster entry {}", self.name);
        ci::check_fp(&format!("{at}: node_fp"), &self.node_fp)?;
        ci::check_spki(&format!("{at}: node_spki"), &self.node_spki, &self.node_fp)?;
        ci::check_name("node_roster entry name", &self.name)?;
        ci::check_digest(&format!("{at}: statement_digest"), &self.statement_digest)?;
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
            (Some(TaintBasis::OwnerAttested), Some(d)) => ci::check_digest(&format!("{at}: attestation_digest"), d)?,
            (Some(TaintBasis::OwnerAttested), None) => return Err(format!("{at}: owner_attested without its attestation")),
            (_, Some(_)) => return Err(format!("{at}: an attestation digest without owner_attested")),
            (_, None) => {}
        }
        ci::check_name(&format!("{at}: host"), &self.host)?;
        for (w, v) in [("provider", &self.provider), ("site", &self.site), ("pod", &self.pod)] {
            if let Some(v) = v {
                ci::check_name(&format!("{at}: {w}"), v)?;
            }
        }
        for a in &self.admin_reach {
            ci::check_name(&format!("{at}: admin_reach"), a)?;
        }
        ci::check_sorted_unique(&format!("{at}: admin_reach"), &self.admin_reach)?;
        Ok(())
    }

    /// The P-256 SPKI DER (checked against `node_fp`).
    pub fn spki_der(&self) -> Result<Vec<u8>, String> {
        ci::check_spki("node_roster entry node_spki", &self.node_spki, &self.node_fp)
    }
}

/// One client's seat on the owner's nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeatStatement {
    /// [`SEAT_V`].
    pub v: u32,
    pub owner_id: String,
    /// `ed25519:<b64>`, the same key as the owner's roster.
    pub roster_key: String,
    pub seat_id: String,
    /// The client server's tunnel-key fingerprint (the controller).
    pub client_fp: String,
    /// The client's display label (its host).
    pub client_label: String,
    pub lane: String,
    /// Roster entry `name`s, sorted, at least 2 (a dual build must fit).
    pub nodes: Vec<String>,
    /// The roster `seq` this seat was issued against: every name above has
    /// an active entry there.
    pub roster_seq: u64,
    pub state: SeatState,
    pub issued_at: i64,
    /// 0.46.11 pods: the pod and the line; `null` in 0.46.10.
    pub pod: Option<String>,
    pub line: Option<String>,
}

impl SeatStatement {
    pub fn check(&self) -> Result<(), String> {
        if self.v != SEAT_V {
            return Err(format!("seat v{} (this build knows v{SEAT_V})", self.v));
        }
        ci::check_name("seat.owner_id", &self.owner_id)?;
        ApprovalKey::parse(&self.roster_key).ok_or("seat.roster_key: not ed25519:<32 bytes b64>")?;
        ci::check_name("seat.seat_id", &self.seat_id)?;
        ci::check_fp("seat.client_fp", &self.client_fp)?;
        ci::check_text("seat.client_label", &self.client_label, 253)?;
        ci::check_name("seat.lane", &self.lane)?;
        if self.nodes.len() < 2 || self.nodes.len() > MAX_SEAT_NODES {
            return Err(format!("seat.nodes: 2-{MAX_SEAT_NODES} nodes"));
        }
        for n in &self.nodes {
            ci::check_name("seat.nodes", n)?;
        }
        ci::check_sorted_unique("seat.nodes", &self.nodes)?;
        if self.roster_seq == 0 {
            return Err("seat.roster_seq starts at 1".into());
        }
        for (w, v) in [("pod", &self.pod), ("line", &self.line)] {
            if let Some(v) = v {
                ci::check_name(&format!("seat.{w}"), v)?;
            }
        }
        Ok(())
    }

    pub fn sign(&self, key: &ApprovalSigner) -> Result<Signed, String> {
        self.check()?;
        if key.public().encode() != self.roster_key {
            return Err("seat: signed by a key that is not roster_key".into());
        }
        Signed::sign_owner(key, ci::DOMAIN_NODE_SEAT, self)
    }

    pub fn open(s: &Signed, pinned: &ApprovalKey) -> Result<Self, String> {
        let st: SeatStatement = s.open_owner(pinned, ci::DOMAIN_NODE_SEAT)?;
        st.check()?;
        if st.roster_key != pinned.encode() {
            return Err("seat: roster_key is not the verifying key".into());
        }
        Ok(st)
    }
}

/// The owner's vouch for an existing key (V12): installed with the wall,
/// and no job ever started without it. A person approves it on a card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerAttestation {
    /// [`ATTESTATION_V`].
    pub v: u32,
    pub owner_id: String,
    /// `ed25519:<b64>`, the owner's roster key.
    pub roster_key: String,
    pub node_fp: String,
    pub key_epoch: u32,
    /// [`ci::digest`] of the self-statement vouched for (it may say
    /// `taint: unknown`: that is what this replaces).
    pub statement_digest: String,
    /// The node was installed with the job wall (no `--no-job-wall`).
    pub installed_with_wall: bool,
    /// Any job start under `Wall::None` seen in the node's audit/ledger.
    /// Must be `false`: an attestation never vouches for a tainted key.
    pub unwalled_jobs_seen: bool,
    /// What the owner checked, in plain words (≤ 500 bytes).
    pub checked: String,
    /// The person who approved it (K2 user name).
    pub approved_by: String,
    pub at: i64,
}

impl OwnerAttestation {
    pub fn check(&self) -> Result<(), String> {
        if self.v != ATTESTATION_V {
            return Err(format!("attestation v{} (this build knows v{ATTESTATION_V})", self.v));
        }
        ci::check_name("attestation.owner_id", &self.owner_id)?;
        ApprovalKey::parse(&self.roster_key).ok_or("attestation.roster_key: not ed25519:<32 bytes b64>")?;
        ci::check_fp("attestation.node_fp", &self.node_fp)?;
        ci::check_digest("attestation.statement_digest", &self.statement_digest)?;
        if !self.installed_with_wall || self.unwalled_jobs_seen {
            return Err(format!("{}: nothing to attest: re-key it under the wall", ci::codes::NODE_KEY_TAINTED));
        }
        ci::check_text("attestation.checked", &self.checked, 500)?;
        if self.checked.trim().is_empty() {
            return Err("attestation.checked: say what was checked".into());
        }
        ci::check_text("attestation.approved_by", &self.approved_by, 64)?;
        if self.approved_by.trim().is_empty() {
            return Err("attestation.approved_by: a person approves it".into());
        }
        Ok(())
    }

    pub fn sign(&self, key: &ApprovalSigner) -> Result<Signed, String> {
        self.check()?;
        if key.public().encode() != self.roster_key {
            return Err("attestation: signed by a key that is not roster_key".into());
        }
        Signed::sign_owner(key, ci::DOMAIN_NODE_ATTEST, self)
    }

    pub fn open(s: &Signed, pinned: &ApprovalKey) -> Result<Self, String> {
        let a: OwnerAttestation = s.open_owner(pinned, ci::DOMAIN_NODE_ATTEST)?;
        a.check()?;
        if a.roster_key != pinned.encode() {
            return Err("attestation: roster_key is not the verifying key".into());
        }
        Ok(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{b64, SigningKey};

    fn entry(k: &SigningKey, name: &str) -> RosterEntry {
        RosterEntry {
            node_fp: k.fingerprint(),
            node_spki: b64(k.spki_der()),
            name: name.into(),
            key_epoch: 0,
            statement_digest: format!("sha256:{}", "1".repeat(64)),
            status: KeyStatus::Active,
            revoked_reason: None,
            valid_from: 1_700_000_000,
            valid_to: None,
            taint_basis: Some(TaintBasis::SelfClean),
            attestation_digest: None,
            host: format!("host-{name}"),
            provider: Some("hetzner".into()),
            site: None,
            admin_reach: vec!["fleet-ssh".into()],
            pod: None,
            point: None,
        }
    }

    fn roster(o: &ApprovalSigner, mut entries: Vec<RosterEntry>) -> NodeRoster {
        entries.sort_by(|a, b| a.node_fp.cmp(&b.node_fp));
        NodeRoster {
            v: 1,
            owner_id: "owner.k2.dev".into(),
            owner_server_fp: "e".repeat(64),
            roster_key: o.public().encode(),
            seq: 1,
            issued_at: 1_700_000_000,
            prev_digest: None,
            entries,
        }
    }

    #[test]
    fn roster_signs_opens_and_refuses_another_key() {
        let o = ApprovalSigner::from_seed(&[1u8; 32]).unwrap();
        let (a, b) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
        let r = roster(&o, vec![entry(&a, "n1"), entry(&b, "n2")]);
        let s = r.sign(&o).unwrap();
        assert_eq!(NodeRoster::open(&s, &o.public()).unwrap(), r);
        assert_eq!(r.key_fp().unwrap(), o.public().fingerprint());
        let other = ApprovalSigner::from_seed(&[2u8; 32]).unwrap();
        assert!(NodeRoster::open(&s, &other.public()).is_err());
        assert!(r.sign(&other).is_err());
        // A roster signed by another key that names that key: valid
        // signature, but not the pinned key.
        let r2 = NodeRoster { roster_key: other.public().encode(), ..r.clone() };
        let s2 = r2.sign(&other).unwrap();
        assert!(NodeRoster::open(&s2, &o.public()).is_err());
        assert!(r.entry(&a.fingerprint()).is_some());
    }

    #[test]
    fn roster_rules() {
        let o = ApprovalSigner::from_seed(&[1u8; 32]).unwrap();
        let (a, b) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
        let r = roster(&o, vec![entry(&a, "n1"), entry(&b, "n2")]);
        r.check().unwrap();
        assert!(NodeRoster { seq: 0, ..r.clone() }.check().is_err());
        assert!(NodeRoster { seq: 2, ..r.clone() }.check().is_err(), "seq 2 without prev_digest");
        NodeRoster { seq: 2, prev_digest: Some(format!("sha256:{}", "2".repeat(64))), ..r.clone() }.check().unwrap();
        let mut unsorted = r.clone();
        unsorted.entries.reverse();
        assert!(unsorted.check().is_err());
        // Two active keys for one name: refused; the old one retired: fine.
        let mut dup = roster(&o, vec![entry(&a, "n1"), entry(&b, "n1")]);
        assert!(dup.check().is_err());
        let i = dup.entries.iter().position(|e| e.node_fp == a.fingerprint()).unwrap();
        dup.entries[i].status = KeyStatus::Retired;
        dup.entries[i].valid_to = Some(1_700_000_500);
        dup.check().unwrap();
        let e = entry(&a, "n1");
        assert!(RosterEntry { status: KeyStatus::Revoked, ..e.clone() }.check().is_err());
        RosterEntry { status: KeyStatus::Revoked, revoked_reason: Some(RevokeReason::Compromised), ..e.clone() }.check().unwrap();
        assert!(RosterEntry { status: KeyStatus::Retired, ..e.clone() }.check().is_err());
        assert!(RosterEntry { valid_to: Some(1), ..e.clone() }.check().is_err());
        assert!(RosterEntry { taint_basis: Some(TaintBasis::OwnerAttested), ..e.clone() }.check().is_err());
        RosterEntry { taint_basis: None, ..e.clone() }.check().unwrap();
        assert!(RosterEntry { attestation_digest: Some(format!("sha256:{}", "3".repeat(64))), ..e.clone() }.check().is_err());
        assert!(RosterEntry { admin_reach: vec!["b".into(), "a".into()], ..e.clone() }.check().is_err());
        assert!(RosterEntry { node_fp: b.fingerprint(), ..e }.check().is_err(), "spki is not node_fp's");
    }

    #[test]
    fn seat_and_attestation() {
        let o = ApprovalSigner::from_seed(&[1u8; 32]).unwrap();
        let seat = SeatStatement {
            v: 1,
            owner_id: "owner.k2.dev".into(),
            roster_key: o.public().encode(),
            seat_id: "seat-1".into(),
            client_fp: "c".repeat(64),
            client_label: "client.k2.dev".into(),
            lane: "ci-standard".into(),
            nodes: vec!["n1".into(), "n2".into()],
            roster_seq: 1,
            state: SeatState::Active,
            issued_at: 1_700_000_000,
            pod: None,
            line: None,
        };
        let s = seat.sign(&o).unwrap();
        assert_eq!(SeatStatement::open(&s, &o.public()).unwrap(), seat);
        // The same body under the roster domain does not verify as a seat.
        let as_roster = Signed::sign_owner(&o, ci::DOMAIN_NODE_ROSTER, &seat).unwrap();
        assert!(SeatStatement::open(&as_roster, &o.public()).is_err());
        assert!(SeatStatement { nodes: vec!["n1".into()], ..seat.clone() }.check().is_err());
        assert!(SeatStatement { nodes: vec!["n2".into(), "n1".into()], ..seat.clone() }.check().is_err());
        assert!(SeatStatement { roster_seq: 0, ..seat }.check().is_err());

        let att = OwnerAttestation {
            v: 1,
            owner_id: "owner.k2.dev".into(),
            roster_key: o.public().encode(),
            node_fp: "d".repeat(64),
            key_epoch: 0,
            statement_digest: format!("sha256:{}", "4".repeat(64)),
            installed_with_wall: true,
            unwalled_jobs_seen: false,
            checked: "install log and job ledger".into(),
            approved_by: "owner".into(),
            at: 1_700_000_000,
        };
        let s = att.sign(&o).unwrap();
        assert_eq!(OwnerAttestation::open(&s, &o.public()).unwrap(), att);
        assert!(OwnerAttestation { unwalled_jobs_seen: true, ..att.clone() }.check().unwrap_err().contains("node_key_tainted"));
        assert!(OwnerAttestation { installed_with_wall: false, ..att.clone() }.check().is_err());
        assert!(OwnerAttestation { approved_by: " ".into(), ..att }.check().is_err());
    }
}
