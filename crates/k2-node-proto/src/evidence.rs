//! The evidence bundle the signer checks (C4, C6b; PRD §4.5, §4.7,
//! vs-live V13, V14, V28): v2, and v3 = v2 plus the build statements
//! (prd-compute-build-attestations-v1 §5, BA-8, slice S3) and a reserved
//! `roster_approvals` list (§18.6). Formats only: `docs/ci-formats-v1.md`
//! §8.
//!
//! `k2 ci evidence <run>` writes it; `k2 signer sign --evidence` reads it.
//! **Nothing in it is trusted because it is in the bundle**: every receipt,
//! statement, roster and seat carries its own signature, the signer pins
//! the keys, and it re-hashes every plan and the commit object itself. The
//! bundle only gathers them in one closed shape.
//!
//! v1 (`compute_release.py evidence-bundle`, k2sign's `compute-evidence`)
//! is unchanged and stays k2sign's: `{"v":1,"commit_object","receipts":{
//! "L1","V1"}}`. A v2 or v3 bundle is refused by k2sign (`v != 1`) and a
//! v1 bundle by [`EvidenceBundle::parse`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ci::{self, Signed};

/// Evidence bundle v2: receipts, self-statements, rosters, seats.
pub const EVIDENCE_V2: u32 = 2;
/// Evidence bundle v3: v2 plus `provenance` (one build statement per
/// receipt label that has one) and the optional `roster_approvals`.
pub const EVIDENCE_V3: u32 = 3;
/// The version `k2 ci evidence` writes. Readers take v2 and v3.
pub const EVIDENCE_V: u32 = EVIDENCE_V3;
/// The bundle's `kind`.
pub const EVIDENCE_KIND: &str = "k2-ci-evidence";
/// Largest bundle file.
pub const MAX_EVIDENCE_BYTES: usize = 8 << 20;
/// Most receipts in one bundle.
pub const MAX_RECEIPTS: usize = 32;
/// Most entries in a v3 bundle's `roster_approvals`.
pub const MAX_ROSTER_APPROVALS: usize = 64;
/// Largest reported file (decoded).
pub const MAX_REPORTED_BYTES: usize = 1 << 20;

/// One release (or CI) run's evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceBundle {
    /// [`EVIDENCE_V2`] or [`EVIDENCE_V3`].
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
    /// v3 (always present there, `{}` when no node signed one): receipt
    /// label → the build statement (DSSE envelope) the node signed with
    /// that receipt. Absent in v2. The signer checks each against its
    /// receipt (slice S4); none is trusted for being here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<BTreeMap<String, crate::provenance::Dsse>>,
    /// v3, optional and reserved (prd-compute-build-attestations-v1 §18.6,
    /// slice A2): the node owner's signed approvals of the rosters in
    /// `rosters`. Their format is A2's. A reader that doesn't know it
    /// carries them unread, which drops no check it has. Absent in v2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roster_approvals: Option<Vec<Value>>,
    /// Files a job wrote that a signer policy reads (`[build.<name>.reported]`,
    /// BA-10): label → artifact name → the file's bytes, base64. Trusted only
    /// as far as the bytes hash to that artifact in the label's node-signed
    /// receipt. Optional: absent in a bundle that carries none, so a bundle
    /// without it has the shape it always had.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reported: BTreeMap<String, BTreeMap<String, String>>,
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
            Some(v) if v == EVIDENCE_V2 as u64 || v == EVIDENCE_V3 as u64 => {}
            Some(1) => return Err("evidence: a v1 bundle (k2sign's S3b path); k2-signer reads v2 and v3".into()),
            other => return Err(format!("evidence: version {other:?}, want {EVIDENCE_V2} or {EVIDENCE_V3}")),
        }
        let b: EvidenceBundle = serde_json::from_value(peek).map_err(|e| format!("evidence: {e}"))?;
        b.check()?;
        Ok(b)
    }

    pub fn check(&self) -> Result<(), String> {
        if !(self.v == EVIDENCE_V2 || self.v == EVIDENCE_V3) || self.kind != EVIDENCE_KIND {
            return Err(format!("evidence: want v{EVIDENCE_V2} or v{EVIDENCE_V3}, kind {EVIDENCE_KIND}"));
        }
        if self.v == EVIDENCE_V2 && (self.provenance.is_some() || self.roster_approvals.is_some()) {
            return Err("evidence: provenance and roster_approvals are v3 keys; a v2 bundle has neither".into());
        }
        if self.v == EVIDENCE_V3 && self.provenance.is_none() {
            return Err("evidence: a v3 bundle has provenance ({} when no node signed a statement)".into());
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
        for (label, env) in self.provenance.iter().flatten() {
            if !self.receipts.contains_key(label) {
                return Err(format!("evidence.provenance.{label}: no receipt has that label"));
            }
            if !env.keyid().is_some_and(ci::is_fp) {
                return Err(format!("evidence.provenance.{label}: not an envelope with one signature and a 64-hex keyid"));
            }
            if env.payload.len() > crate::provenance::MAX_PAYLOAD_BYTES.div_ceil(3) * 4 {
                return Err(format!("evidence.provenance.{label}: the statement is over {} bytes", crate::provenance::MAX_PAYLOAD_BYTES));
            }
        }
        if let Some(a) = &self.roster_approvals {
            if a.len() > MAX_ROSTER_APPROVALS || a.iter().any(|x| !x.is_object()) {
                return Err(format!("evidence.roster_approvals: at most {MAX_ROSTER_APPROVALS} JSON objects"));
            }
        }
        for (label, files) in &self.reported {
            if !self.receipts.contains_key(label) {
                return Err(format!("evidence.reported: {label:?} is not a receipt of this bundle"));
            }
            if files.is_empty() {
                return Err(format!("evidence.reported.{label}: no file"));
            }
            for (name, b) in files {
                ci::check_text(&format!("evidence.reported.{label}"), name, 512)?;
                if name.is_empty() || name.starts_with('/') || name.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
                    return Err(format!("evidence.reported.{label}: {name:?} is not an artifact path"));
                }
                let raw = crate::crypto::unb64(b).map_err(|e| format!("evidence.reported.{label}.{name}: {e}"))?;
                if raw.len() > MAX_REPORTED_BYTES {
                    return Err(format!("evidence.reported.{label}.{name}: over {MAX_REPORTED_BYTES} bytes"));
                }
            }
        }
        Ok(())
    }

    /// The bytes of `name` that `label`'s job reported, if the bundle has them.
    pub fn reported_file(&self, label: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
        match self.reported.get(label).and_then(|f| f.get(name)) {
            Some(b) => crate::crypto::unb64(b).map(Some).map_err(|e| format!("evidence.reported.{label}.{name}: {e}")),
            None => Ok(None),
        }
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

    fn bundle(v: u32) -> EvidenceBundle {
        let receipt = EvidenceReceipt { build: "linux-x86_64".into(), role: Role::Build, body: "{}".into(), sig: "x".into(), plan: serde_json::json!({}) };
        EvidenceBundle {
            v,
            kind: EVIDENCE_KIND.into(),
            project: "acme".into(),
            version: Some("1.2.0".into()),
            run_id: "run-1".into(),
            controller_fp: "c".repeat(64),
            created_at: 1,
            source: EvidenceSource { commit: "1".repeat(40), tree: "2".repeat(40), commit_object: None },
            receipts: BTreeMap::from([("L1".to_string(), receipt)]),
            statements: Vec::new(),
            rotations: Vec::new(),
            attestations: Vec::new(),
            rosters: Vec::new(),
            seats: Vec::new(),
            pairing_pins: BTreeMap::new(),
            provenance: (v == EVIDENCE_V3).then(BTreeMap::new),
            roster_approvals: None,
            reported: BTreeMap::new(),
        }
    }

    fn envelope(keyid: &str) -> crate::provenance::Dsse {
        crate::provenance::Dsse {
            payload_type: crate::provenance::PAYLOAD_TYPE.into(),
            payload: "e30=".into(),
            signatures: vec![crate::provenance::DsseSignature { keyid: keyid.into(), sig: "x".into() }],
        }
    }

    /// v3 = v2 + `provenance` (always there, by receipt label) + the
    /// optional, reserved `roster_approvals`; v2 has neither; both parse.
    #[test]
    fn v3_carries_statements_by_label_and_v2_has_none() {
        let v2 = bundle(EVIDENCE_V2);
        v2.check().unwrap();
        let text = serde_json::to_string(&v2).unwrap();
        assert!(!text.contains("provenance") && !text.contains("roster_approvals"), "v2's bytes as before: {text}");
        assert_eq!(EvidenceBundle::parse(text.as_bytes()).unwrap(), v2);

        let mut v3 = bundle(EVIDENCE_V3);
        v3.provenance.as_mut().unwrap().insert("L1".into(), envelope(&"a".repeat(64)));
        v3.check().unwrap();
        let text = serde_json::to_string(&v3).unwrap();
        assert!(text.contains(r#""provenance":{"L1":{"payloadType""#), "{text}");
        assert!(!text.contains("roster_approvals"), "absent until A2 writes it: {text}");
        assert_eq!(EvidenceBundle::parse(text.as_bytes()).unwrap(), v3);
        let empty = bundle(EVIDENCE_V3);
        assert!(serde_json::to_string(&empty).unwrap().contains(r#""provenance":{}"#), "always present in v3");

        // Reserved for A2: a list of objects, carried as they are.
        let mut a = v3.clone();
        a.roster_approvals = Some(vec![serde_json::json!({"kind": "k2.compute.roster"})]);
        assert_eq!(EvidenceBundle::parse(serde_json::to_string(&a).unwrap().as_bytes()).unwrap(), a);
        a.roster_approvals = Some(vec![serde_json::json!("not an object")]);
        assert!(a.check().unwrap_err().contains("roster_approvals"));

        // The refusals, by name.
        let mut e = v2.clone();
        e.provenance = Some(BTreeMap::new());
        assert!(e.check().unwrap_err().contains("v3 keys"));
        let mut e = v2.clone();
        e.roster_approvals = Some(Vec::new());
        assert!(e.check().unwrap_err().contains("v3 keys"));
        let mut e = v3.clone();
        e.provenance = None;
        assert!(e.check().unwrap_err().contains("a v3 bundle has provenance"));
        let mut e = v3.clone();
        e.provenance.as_mut().unwrap().insert("V1".into(), envelope(&"a".repeat(64)));
        assert!(e.check().unwrap_err().contains("evidence.provenance.V1: no receipt"));
        let mut e = v3.clone();
        e.provenance.as_mut().unwrap().insert("L1".into(), envelope("not-a-fingerprint"));
        assert!(e.check().unwrap_err().contains("keyid"));
        let mut e = bundle(EVIDENCE_V3);
        e.v = 4;
        assert!(EvidenceBundle::parse(serde_json::to_string(&e).unwrap().as_bytes()).unwrap_err().contains("want 2 or 3"));
    }

    /// A v3 bundle with one aarch64 build receipt (L2), for reported files.
    fn reported_bundle() -> EvidenceBundle {
        let mut b = bundle(EVIDENCE_V3);
        b.receipts = BTreeMap::from([(
            "L2".to_string(),
            EvidenceReceipt { build: "linux-aarch64".into(), role: Role::Build, body: "{}".into(), sig: String::new(), plan: serde_json::json!({}) },
        )]);
        b
    }

    #[test]
    fn reported_files_are_optional_and_checked() {
        let plain = reported_bundle();
        plain.check().unwrap();
        let text = serde_json::to_string(&plain).unwrap();
        assert!(!text.contains("reported"), "a bundle without reported files keeps its old shape: {text}");
        assert_eq!(EvidenceBundle::parse(text.as_bytes()).unwrap(), plain);

        let mut b = reported_bundle();
        let info = br#"{"cross":{"zig_tarball_sha256":"aa"}}"#;
        b.reported.insert("L2".into(), BTreeMap::from([("dist/build-info.json".to_string(), crate::crypto::b64(info))]));
        b.check().unwrap();
        assert_eq!(b.reported_file("L2", "dist/build-info.json").unwrap().as_deref(), Some(&info[..]));
        assert_eq!(b.reported_file("V2", "dist/build-info.json").unwrap(), None);
        let back = EvidenceBundle::parse(serde_json::to_string(&b).unwrap().as_bytes()).unwrap();
        assert_eq!(back, b);

        let mut other = b.clone();
        other.reported.insert("V9".into(), other.reported["L2"].clone());
        assert!(other.check().unwrap_err().contains("not a receipt"));
        let mut path = reported_bundle();
        path.reported.insert("L2".into(), BTreeMap::from([("../x".to_string(), crate::crypto::b64(b"x"))]));
        assert!(path.check().unwrap_err().contains("not an artifact path"));
        let mut junk = reported_bundle();
        junk.reported.insert("L2".into(), BTreeMap::from([("dist/x".to_string(), "not base64!".to_string())]));
        assert!(junk.check().is_err());
    }

    #[test]
    fn v1_bundles_are_refused_by_name() {
        let e = EvidenceBundle::parse(br#"{"v":1,"commit_object":"","receipts":{}}"#).unwrap_err();
        assert!(e.contains("v1 bundle"), "{e}");
        assert!(EvidenceBundle::parse(b"{}").is_err());
    }
}
