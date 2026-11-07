//! The CI formats golden vectors (docs/ci-formats-v1.md §10): every
//! document in tests/release/fixtures/ci-formats-v1.json must verify here,
//! parse into its type, be its own canonical form, and pass its shape rules.
//! The Python side (tests/release/k2sign-ci-formats.sh) checks the same file
//! with k2sign's canonical JSON, /usr/bin/openssl and an RFC 8032 Ed25519.
//! A failure here means the Rust types drifted from the frozen formats:
//! change the format on purpose (a new `v`, the spec, the generator), never
//! the vector alone.

use std::collections::BTreeMap;
use std::path::PathBuf;

use k2_node_proto::admin::{ApprovalKey, ApprovalSigner};
use k2_node_proto::ci::{self, Isolation, KeyStatus, Signed, TaintBasis};
use k2_node_proto::ci_config::CiConfig;
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::evidence::{EvidenceBundle, Role};
use k2_node_proto::frames::{Plan, Receipt, SignedReceipt, RECEIPT_V2};
use k2_node_proto::node_roster::{NodeRoster, OwnerAttestation, SeatStatement};
use k2_node_proto::signer_policy::SignerPolicy;
use k2_node_proto::statement::{NodeStatement, RotationStatement};
use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/release/fixtures").join(name)
}

fn read(name: &str) -> String {
    let p = fixture(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

struct Vectors {
    root: Value,
    by_name: BTreeMap<String, Value>,
}

impl Vectors {
    fn load() -> Self {
        let root: Value = serde_json::from_str(&read("ci-formats-v1.json")).expect("ci-formats-v1.json is JSON");
        let mut by_name = BTreeMap::new();
        for v in root["vectors"].as_array().expect("vectors") {
            let name = v["name"].as_str().expect("name").to_string();
            assert!(by_name.insert(name.clone(), v.clone()).is_none(), "vector {name} twice");
        }
        Self { root, by_name }
    }
    fn v(&self, name: &str) -> &Value {
        self.by_name.get(name).unwrap_or_else(|| panic!("no vector {name}"))
    }
    fn signed(&self, name: &str) -> Signed {
        let v = self.v(name);
        Signed { body: s(&v["body"]).into(), sig: s(&v["sig"]).into() }
    }
    fn key(&self, name: &str) -> &Value {
        &self.root["keys"][name]
    }
    fn spki(&self, name: &str) -> Vec<u8> {
        crypto::unb64(s(&self.key(name)["spki_b64"])).unwrap()
    }
    fn roster_signer(&self) -> ApprovalSigner {
        let seed: [u8; 32] = hex_bytes(s(&self.key("roster")["seed_hex"])).try_into().unwrap();
        ApprovalSigner::from_seed(&seed).unwrap()
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn hex_bytes(h: &str) -> Vec<u8> {
    assert!(h.len() % 2 == 0);
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn keys_are_what_they_say() {
    let vx = Vectors::load();
    for name in ["node_a_epoch0", "node_a_epoch1", "node_b", "controller"] {
        let k = vx.key(name);
        let sk = SigningKey::from_pkcs8_pem(s(&k["private_key_pem"])).unwrap();
        assert_eq!(sk.fingerprint(), s(&k["fp"]), "{name}");
        assert_eq!(crypto::b64(sk.spki_der()), s(&k["spki_b64"]), "{name}");
        assert_eq!(crypto::fingerprint_of_spki_pem(s(&k["public_key_pem"])).unwrap(), s(&k["fp"]), "{name}");
    }
    let r = vx.roster_signer();
    assert_eq!(r.public().encode(), s(&vx.key("roster")["public"]));
    assert_eq!(r.public().fingerprint(), s(&vx.key("roster")["fp"]));
}

/// Every vector: the signed bytes, the digest, the signature, the parse and
/// the canonical form.
#[test]
fn every_vector_has_the_exact_bytes_and_signature() {
    let vx = Vectors::load();
    let roster_key = vx.roster_signer();
    for (name, v) in &vx.by_name {
        let body = s(&v["body"]);
        assert_eq!(s(&v["digest"]), ci::digest(body), "{name}: digest");
        match v["alg"].as_str() {
            None => {
                assert_eq!(s(&v["kind"]), "plan", "{name}: only the plan is unsigned");
                continue;
            }
            Some(alg) => {
                let domain = s(&v["domain"]);
                let sb = crypto::signed_bytes(domain, &[body.as_bytes()]);
                assert_eq!(crypto::hex(&sb), s(&v["signed_bytes_hex"]), "{name}: signed bytes");
                match alg {
                    "p256" => {
                        let spki = vx.spki(s(&v["signer"]));
                        assert!(crypto::verify_domain(&spki, domain, &[body.as_bytes()], s(&v["sig"])), "{name}: p256 signature");
                        let tampered = body.replacen(':', ": ", 1);
                        assert!(!crypto::verify_domain(&spki, domain, &[tampered.as_bytes()], s(&v["sig"])), "{name}");
                    }
                    "ed25519" => {
                        // Deterministic: the same seed makes the same bytes.
                        let again = crypto::b64(&roster_key.sign(&ci::owner_message(domain, body)));
                        assert_eq!(again, s(&v["sig"]), "{name}: ed25519 signature");
                    }
                    other => panic!("{name}: alg {other}"),
                }
            }
        }
    }
}

#[test]
fn statements_and_rotation() {
    let vx = Vectors::load();
    let a0 = NodeStatement::open(&vx.signed("node_statement_a_epoch0")).unwrap();
    let a1 = NodeStatement::open(&vx.signed("node_statement_a_epoch1")).unwrap();
    let b = NodeStatement::open(&vx.signed("node_statement_b")).unwrap();
    assert_eq!(a0.node_fp, s(&vx.key("node_a_epoch0")["fp"]));
    assert_eq!(a0.taint, ci::Taint::Unknown);
    assert_eq!((a1.key_epoch, a1.prev_fp.as_deref()), (1, Some(a0.node_fp.as_str())));
    assert_eq!(a1.taint, ci::Taint::Clean);
    assert_eq!(b.host_id.as_deref(), Some(k2_node_proto::statement::host_id("machine-b").as_str()));
    let rot = RotationStatement::open(&vx.signed("rotation_a"), &vx.spki("node_a_epoch0")).unwrap();
    rot.continued_by(&a1).unwrap();
    assert!(RotationStatement::open(&vx.signed("rotation_a"), &vx.spki("node_a_epoch1")).is_err(), "only the old key vouches");
    // A statement opened under another domain is refused.
    let st = vx.signed("node_statement_b");
    assert!(st.open_node::<NodeStatement>(&vx.spki("node_b"), ci::DOMAIN_NODE_ROTATION).is_err());
}

#[test]
fn roster_seat_attestation_link_up() {
    let vx = Vectors::load();
    let key = ApprovalKey::parse(s(&vx.key("roster")["public"])).unwrap();
    let roster = NodeRoster::open(&vx.signed("node_roster"), &key).unwrap();
    let seat = SeatStatement::open(&vx.signed("seat_statement"), &key).unwrap();
    let att = OwnerAttestation::open(&vx.signed("owner_attestation_b"), &key).unwrap();
    assert_eq!(roster.seq, 2);
    assert_eq!(roster.owner_server_fp, s(&vx.root["owner_server_fp"]));
    assert_eq!(roster.key_fp().unwrap(), s(&vx.key("roster")["fp"]));
    let fp = |k: &str| s(&vx.key(k)["fp"]).to_string();
    let a0 = roster.entry(&fp("node_a_epoch0")).unwrap();
    let a1 = roster.entry(&fp("node_a_epoch1")).unwrap();
    let b = roster.entry(&fp("node_b")).unwrap();
    assert_eq!((a0.status, a0.taint_basis, a0.valid_to), (KeyStatus::Retired, None, Some(1_800_000_500)));
    assert_eq!((a1.status, a1.taint_basis), (KeyStatus::Active, Some(TaintBasis::Rekeyed)));
    assert_eq!((b.status, b.taint_basis), (KeyStatus::Active, Some(TaintBasis::OwnerAttested)));
    assert_eq!(a0.statement_digest, ci::digest(s(&vx.v("node_statement_a_epoch0")["body"])));
    assert_eq!(a1.statement_digest, ci::digest(s(&vx.v("node_statement_a_epoch1")["body"])));
    assert_eq!(b.statement_digest, ci::digest(s(&vx.v("node_statement_b")["body"])));
    assert_eq!(b.attestation_digest.as_deref(), Some(ci::digest(s(&vx.v("owner_attestation_b")["body"])).as_str()));
    assert_eq!(att.statement_digest, b.statement_digest);
    assert_eq!(seat.client_fp, fp("controller"));
    assert_eq!(seat.roster_seq, roster.seq);
    for n in &seat.nodes {
        assert!(roster.entries.iter().any(|e| &e.name == n && e.status == KeyStatus::Active), "{n}");
    }
    // Owner documents under one another's domains don't verify.
    assert!(NodeRoster::open(&vx.signed("seat_statement"), &key).is_err());
    assert!(SeatStatement::open(&vx.signed("node_roster"), &key).is_err());
}

#[test]
fn receipts_v2_and_their_plans() {
    let vx = Vectors::load();
    let plan_body = s(&vx.v("plan_receipt_v2")["body"]);
    let plan: Plan = serde_json::from_str(plan_body).unwrap();
    assert_eq!(k2_node_proto::canonical::to_string(&plan).unwrap(), plan_body, "the plan is canonical Rust");
    assert_eq!(plan.receipt_v, Some(RECEIPT_V2));
    assert_eq!(plan.digest().unwrap(), ci::digest(plan_body));
    for (name, key, iso) in [("receipt_v2_l1", "node_a_epoch1", Isolation::JobUid), ("receipt_v2_v1", "node_b", Isolation::Exclusive)] {
        let sv = vx.signed(name);
        let sr = SignedReceipt { body: sv.body.clone(), sig: sv.sig.clone() };
        let r: Receipt = sr.verify(&vx.spki(key)).unwrap_or_else(|| panic!("{name} verifies"));
        assert_eq!(k2_node_proto::canonical::to_string(&r).unwrap(), sr.body, "{name}: canonical");
        r.check_version().unwrap();
        let run = r.run.as_ref().unwrap();
        assert_eq!(run.isolation, iso);
        assert_eq!(r.v, RECEIPT_V2);
        // v2 without run, or v1 with run: refused by name.
        let mut bad = r.clone();
        bad.run = None;
        assert!(bad.check_version().is_err());
        let mut bad = r.clone();
        bad.v = 1;
        assert!(bad.check_version().is_err());
    }
    let l1: Receipt = serde_json::from_str(s(&vx.v("receipt_v2_l1")["body"])).unwrap();
    assert_eq!(l1.plan_digest, plan.digest().unwrap());
    assert_eq!(l1.run.unwrap().statement_digest, ci::digest(s(&vx.v("node_statement_a_epoch1")["body"])));
}

/// The v1 receipt vector stays v1: no `run`, version check passes, and a
/// plan without `receipt_v` keeps the 0.46.2 digest (frames.rs pins it).
#[test]
fn receipt_v1_vector_is_untouched() {
    let v: Value = serde_json::from_str(&read("receipt-vector.json")).unwrap();
    let r: Receipt = serde_json::from_str(s(&v["body"])).unwrap();
    assert!(r.run.is_none());
    r.check_version().unwrap();
    assert!(!s(&v["body"]).contains("\"run\""));
}

#[test]
fn evidence_bundle_parses_and_its_parts_link() {
    let vx = Vectors::load();
    let bytes = serde_json::to_vec(&vx.root["evidence_bundle"]).unwrap();
    let b = EvidenceBundle::parse(&bytes).unwrap();
    assert_eq!(serde_json::to_value(&b).unwrap(), vx.root["evidence_bundle"], "round trip");
    assert_eq!(b.source.commit_object.as_deref(), Some(s(&vx.root["commit_object_b64"])));
    assert_eq!(b.controller_fp, s(&vx.key("controller")["fp"]));
    for (label, r) in &b.receipts {
        let rc: Receipt = serde_json::from_str(&r.body).unwrap();
        assert_eq!(rc.plan_digest, r.plan_digest().unwrap(), "{label}: plan digest");
        assert_eq!(rc.src.as_ref().unwrap().commit, b.source.commit, "{label}");
        assert_eq!(rc.controller_fp, b.controller_fp, "{label}");
        let pin = crypto::unb64(&b.pairing_pins[&rc.node_fp]).unwrap();
        assert!(r.signed().verify(&pin).is_some(), "{label}: verifies with the pairing pin");
    }
    assert_eq!(b.receipts["L1"].role, Role::Build);
    assert_eq!(b.receipts["V1"].role, Role::Verify);
    let st_digests: Vec<String> = b.statements.iter().map(Signed::digest).collect();
    for r in b.receipts.values() {
        let rc: Receipt = serde_json::from_str(&r.body).unwrap();
        assert!(st_digests.contains(&rc.run.unwrap().statement_digest));
    }
    // Shape refusals.
    let mut v = vx.root["evidence_bundle"].clone();
    v["receipts"]["V1"]["role"] = "build".into();
    assert!(EvidenceBundle::parse(&serde_json::to_vec(&v).unwrap()).is_err(), "two build receipts for one build");
    let mut v = vx.root["evidence_bundle"].clone();
    v["source"]["commit"] = "9".repeat(40).into();
    assert!(EvidenceBundle::parse(&serde_json::to_vec(&v).unwrap()).is_err(), "commit object hashes elsewhere");
    let mut v = vx.root["evidence_bundle"].clone();
    v["surprise"] = 1.into();
    assert!(EvidenceBundle::parse(&serde_json::to_vec(&v).unwrap()).is_err(), "unknown key");
}

#[test]
fn toml_examples_parse_round_trip_and_match_the_vectors() {
    let vx = Vectors::load();
    let p = SignerPolicy::parse(&read("ci-formats-v1-signer-policy.toml")).unwrap();
    assert_eq!(SignerPolicy::parse(&p.to_toml().unwrap()).unwrap(), p);
    assert_eq!(p.controllers, vec![s(&vx.key("controller")["fp"]).to_string()]);
    assert_eq!(p.owners, vec![s(&vx.key("roster")["fp"]).to_string()]);
    let b = &p.build["linux-x86_64"];
    let plan: Plan = serde_json::from_str(s(&vx.v("plan_receipt_v2")["body"])).unwrap();
    assert_eq!(b.argv, plan.argv);
    assert_eq!(b.artifacts, plan.artifacts);
    assert_eq!(b.secrets, plan.secret_env);
    let c = CiConfig::parse(&read("ci-formats-v1-ci.toml")).unwrap();
    assert_eq!(CiConfig::parse(&c.to_toml().unwrap()).unwrap(), c);
    let rel = c.release.as_ref().unwrap();
    assert_eq!(rel.project, p.project);
    let rb = &rel.builds["linux-x86_64"];
    assert_eq!((rb.argv.clone(), rb.artifacts.clone(), rb.secrets.clone()), (b.argv.clone(), b.artifacts.clone(), b.secrets.clone()));
    assert_eq!((rb.label.as_str(), rb.verify_label.as_deref()), ("L1", Some("V1")));
}
