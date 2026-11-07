//! The evidence bundle v2 the signer checks (C4, C6b; PRD §4.5, §4.7,
//! vs-live V13, V14, V28). Formats only: `docs/ci-formats-v1.md` §7.
//!
//! `k2 ci evidence <run>` writes it; `k2 signer sign --evidence` reads it.
//! **Nothing in it is trusted because it is in the bundle**: every receipt,
//! statement, roster and seat carries its own signature, the signer pins
//! the keys, and it re-hashes every plan and the commit object itself. The
//! bundle only gathers them in one closed shape.
//!
//! v1 (`compute_release.py evidence-bundle`, k2sign's `compute-evidence`)
//! is unchanged and stays k2sign's: `{"v":1,"commit_object","receipts":{
//! "L1","V1"}}`. A v2 bundle is refused by k2sign (`v != 1`) and a v1
//! bundle by [`EvidenceBundle::parse`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ci::{self, Signed};

/// Version of [`EvidenceBundle`].
pub const EVIDENCE_V: u32 = 2;
/// The bundle's `kind`.
pub const EVIDENCE_KIND: &str = "k2-ci-evidence";
/// Largest bundle file.
pub const MAX_EVIDENCE_BYTES: usize = 8 << 20;
/// Most receipts in one bundle.
pub const MAX_RECEIPTS: usize = 32;

/// One release (or CI) run's evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceBundle {
    /// [`EVIDENCE_V`].
    pub v: u32,
    /// [`EVIDENCE_KIND`].
    pub kind: String,
    /// The signer project (`k2`, `acme-desktop`).
    pub project: String,
    /// The release version; `null` for a CI run that signs nothing.
    pub version: Option<String>,
    /// The controller's run id (`k2 ci release` / `k2 ci run`).
    pub run_id: String,
    /// The client server that sent every job (each receipt signs it too).
    pub controller_fp: String,
    pub created_at: i64,
    pub source: EvidenceSource,
    /// By label (`L1`, `V1`, `L3`: `^[A-Z][A-Z0-9]{0,7}$`).
    pub receipts: BTreeMap<String, EvidenceReceipt>,
    /// Node self-statements the receipts name (`run.statement_digest`).
    pub statements: Vec<Signed>,
    /// Rotation statements linking a pinned key to a receipt's key.
    pub rotations: Vec<Signed>,
    /// Owner attestations the roster entries name.
    pub attestations: Vec<Signed>,
    /// The newest node roster the controller holds from each owner involved.
    pub rosters: Vec<Signed>,
    /// This client's seat statement(s) on those owners.
    pub seats: Vec<Signed>,
    /// Node fp → SPKI (base64 DER) the client server pinned at pairing: the
    /// cross-check of PRD §4.1.6 (a receipt key must be in a trusted roster
    /// **and**, when listed here, equal to this pin).
    pub pairing_pins: BTreeMap<String, String>,
}

/// What was built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSource {
    /// 40 hex: the commit every build ran.
    pub commit: String,
    /// 40 hex: its tree.
    pub tree: String,
    /// The raw git commit object (`git cat-file commit`), base64; the signer
    /// hashes it to `commit` and reads tree and parents itself. `null` when
    /// the forge tag names the commit directly.
    pub commit_object: Option<String>,
}

/// A build's job in the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The build whose files are signed (L1, L3).
    Build,
    /// A dual build that must reproduce `Build`'s `dual` files byte for byte
    /// on an independent node (V1).
    Verify,
}

/// One signed receipt with its plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReceipt {
    /// The signer policy's build name (`[build.<name>]`).
    pub build: String,
    pub role: Role,
    /// The receipt's canonical JSON exactly as the node signed it.
    pub body: String,
    /// The node's signature (base64 DER) over
    /// `signed_bytes("k2-compute-receipt-v1", [body])`.
    pub sig: String,
    /// The job's plan as JSON. Kept as a value: the signer hashes its
    /// canonical form to the receipt's `plan_digest` and refuses a key it
    /// doesn't know.
    pub plan: Value,
}

/// A receipt label: `^[A-Z][A-Z0-9]{0,7}$`.
pub fn is_label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 8 && s.as_bytes()[0].is_ascii_uppercase() && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

impl EvidenceReceipt {
    /// The signed receipt, for [`crate::frames::SignedReceipt::verify`].
    pub fn signed(&self) -> crate::frames::SignedReceipt {
        crate::frames::SignedReceipt { body: self.body.clone(), sig: self.sig.clone() }
    }

    /// `sha256:` of the plan's canonical JSON (compare with the receipt's
    /// `plan_digest`).
    pub fn plan_digest(&self) -> Result<String, String> {
        Ok(format!("sha256:{}", crate::crypto::sha256_hex(&crate::canonical::to_vec(&self.plan)?)))
    }
}

impl EvidenceBundle {
    /// Parse a bundle file and check its shape (no signature is checked
    /// here: that is the signer's work, with its own pins).
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_EVIDENCE_BYTES {
            return Err(format!("evidence: over {MAX_EVIDENCE_BYTES} bytes"));
        }
        let peek: Value = serde_json::from_slice(bytes).map_err(|e| format!("evidence: {e}"))?;
        match peek.get("v").and_then(Value::as_u64) {
            Some(v) if v == EVIDENCE_V as u64 => {}
            Some(1) => return Err("evidence: a v1 bundle (k2sign's S3b path); k2-signer reads v2".into()),
            other => return Err(format!("evidence: version {other:?}, want {EVIDENCE_V}")),
        }
        let b: EvidenceBundle = serde_json::from_value(peek).map_err(|e| format!("evidence: {e}"))?;
        b.check()?;
        Ok(b)
    }

    pub fn check(&self) -> Result<(), String> {
        if self.v != EVIDENCE_V || self.kind != EVIDENCE_KIND {
            return Err(format!("evidence: want v{EVIDENCE_V} kind {EVIDENCE_KIND}"));
        }
        ci::check_name("evidence.project", &self.project)?;
        if let Some(v) = &self.version {
            ci::check_version_text("evidence.version", v)?;
        }
        ci::check_name("evidence.run_id", &self.run_id)?;
        ci::check_fp("evidence.controller_fp", &self.controller_fp)?;
        ci::check_commit("evidence.source.commit", &self.source.commit)?;
        ci::check_commit("evidence.source.tree", &self.source.tree)?;
        if let Some(o) = &self.source.commit_object {
            let raw = crate::crypto::unb64(o).map_err(|e| format!("evidence.source.commit_object: {e}"))?;
            if git_commit_sha1(&raw) != self.source.commit {
                return Err("evidence.source.commit_object does not hash to source.commit".into());
            }
        }
        if self.receipts.is_empty() || self.receipts.len() > MAX_RECEIPTS {
            return Err(format!("evidence.receipts: 1-{MAX_RECEIPTS}"));
        }
        let mut builds: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (label, r) in &self.receipts {
            if !is_label(label) {
                return Err(format!("evidence.receipts: {label:?} is not a label (L1, V1, L3)"));
            }
            ci::check_name(&format!("evidence.receipts.{label}.build"), &r.build)?;
            if !r.plan.is_object() {
                return Err(format!("evidence.receipts.{label}.plan is not an object"));
            }
            if r.body.len() > crate::frames::MAX_FRAME_BYTES {
                return Err(format!("evidence.receipts.{label}.body is too large"));
            }
            let c = builds.entry(r.build.as_str()).or_default();
            match r.role {
                Role::Build => c.0 += 1,
                Role::Verify => c.1 += 1,
            }
        }
        for (b, (n_build, _)) in &builds {
            if *n_build != 1 {
                return Err(format!("evidence: build {b} has {n_build} build receipts, want exactly 1"));
            }
        }
        for (fp, spki) in &self.pairing_pins {
            ci::check_fp("evidence.pairing_pins", fp)?;
            ci::check_spki(&format!("evidence.pairing_pins.{fp}"), spki, fp)?;
        }
        Ok(())
    }
}

/// git's object id of a commit object's bytes (`sha1("commit <n>\0" ‖ raw)`).
pub fn git_commit_sha1(raw: &[u8]) -> String {
    use aws_lc_rs::digest;
    let mut c = digest::Context::new(&digest::SHA1_FOR_LEGACY_USE_ONLY);
    c.update(format!("commit {}\0", raw.len()).as_bytes());
    c.update(raw);
    crate::crypto::hex(c.finish().as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        for ok in ["L1", "V1", "L3", "A", "ABCDEFG1"] {
            assert!(is_label(ok), "{ok}");
        }
        for bad in ["", "l1", "1L", "L-1", "ABCDEFGHI"] {
            assert!(!is_label(bad), "{bad}");
        }
    }

    #[test]
    fn git_commit_sha1_matches_git() {
        // `git hash-object -t commit` of this exact object.
        let raw = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nauthor t <t@example.com> 1700000000 +0000\ncommitter t <t@example.com> 1700000000 +0000\n\nempty\n";
        assert_eq!(git_commit_sha1(raw), "8cb826031568e8917a2cf8590ca5665726e0fc63");
    }

    #[test]
    fn v1_bundles_are_refused_by_name() {
        let e = EvidenceBundle::parse(br#"{"v":1,"commit_object":"","receipts":{}}"#).unwrap_err();
        assert!(e.contains("v1 bundle"), "{e}");
        assert!(EvidenceBundle::parse(b"{}").is_err());
    }
}
