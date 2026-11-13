//! Slice S4 (prd-compute-build-attestations-v1 §3.3, §7.2, §14): the signer
//! ties each receipt's build statement to the receipt, the pinned owner and
//! the key's roster entry, per the policy's `provenance`. Every statement
//! here is made the way the node makes it (`Statement::for_receipt`) and
//! signed the only way a node can (`SigningKey::sign_statement`); each test
//! bends one thing.

mod common;

use std::collections::BTreeMap;

use common::*;
use k2_node_proto::ci::{Isolation, KeyStatus, Wall};
use k2_node_proto::evidence::{EvidenceBundle, EVIDENCE_V3};
use k2_node_proto::frames::Plan;
use k2_node_proto::provenance::{Dsse, NodeFacts, Owner, Statement};
use k2_node_proto::share::Network;
use k2_node_proto::signer_policy::{ProvenanceRule, SignerPolicy};
use k2_signer::check::{check, Input, Summary, Target};
use k2_signer::provenance::StatementStatus;

struct Run {
    policy: SignerPolicy,
    rehearse: bool,
}

impl Run {
    fn new(w: &World, mode: ProvenanceRule) -> Self {
        let mut policy = w.policy();
        policy.provenance = mode;
        Self { policy, rehearse: false }
    }

    fn go(&self, w: &World, b: &EvidenceBundle) -> Result<Summary, String> {
        let source = w.source();
        let target = if self.rehearse {
            Target::Rehearse
        } else {
            Target::Release { version: VERSION, base: &source.base, opened: Some(NOW - 3600), source: Some(&source) }
        };
        let mut trust = w.trust(&[]);
        accept_from(&mut trust, b, &["node-a", "node-b", "node-c"]);
        let marks = BTreeMap::new();
        check(&Input { policy: &self.policy, trust: &trust, marks: &marks, bundle: b, now: NOW, target }).map_err(|e| e.message().to_string())
    }

    #[track_caller]
    fn passes(&self, w: &World, b: &EvidenceBundle) -> Summary {
        self.go(w, b).unwrap_or_else(|e| panic!("refused: {e}"))
    }

    #[track_caller]
    fn refuses(&self, w: &World, b: &EvidenceBundle, text: &str) -> String {
        match self.go(w, b) {
            Ok(_) => panic!("accepted, want a refusal containing {text:?}"),
            Err(e) => {
                assert!(e.contains(text), "refusal {e:?} does not contain {text:?}");
                e
            }
        }
    }
}

/// What the node knows that the receipt doesn't (its owner, the network).
fn facts(w: &World, node: &str) -> NodeFacts {
    NodeFacts {
        owner: Owner::RosterKey(w.owner_fp.clone()),
        network: Network::None,
        isolation: Isolation::JobUid,
        wall: Wall::JobUser,
        tenant_slot: 1,
        lane: Some("ci-standard".into()),
        k2node_version: "0.46.10".into(),
        k2node_commit: Some("4".repeat(40)),
        protocol: 10,
        key_epoch: 0,
        statement_digest: None,
        node_name: Some(node.into()),
    }
}

/// The roster name of the node whose key signed `label`'s receipt.
fn node_of(w: &World, b: &EvidenceBundle, label: &str) -> String {
    let rc: k2_node_proto::frames::Receipt = serde_json::from_str(&b.receipts[label].body).unwrap();
    w.nodes.iter().find(|(_, k)| k.fingerprint() == rc.node_fp).map(|(n, _)| n.clone()).expect("a test node signed it")
}

/// `label`'s statement as its node writes it, after `edit_facts` and `edit`.
fn statement(w: &World, b: &EvidenceBundle, label: &str, edit_facts: impl FnOnce(&mut NodeFacts), edit: impl FnOnce(&mut Statement)) -> Statement {
    let er = &b.receipts[label];
    let plan: Plan = serde_json::from_value(er.plan.clone()).unwrap();
    let mut f = facts(w, &node_of(w, b, label));
    edit_facts(&mut f);
    let mut st = Statement::for_receipt(&er.signed(), &plan, &f).unwrap().expect("the job kept a file");
    edit(&mut st);
    st
}

fn sign(w: &World, node: &str, st: &Statement) -> Dsse {
    w.nodes[node].sign_statement(st).unwrap()
}

/// Make `b` a v3 bundle with no statements yet.
fn v3(mut b: EvidenceBundle) -> EvidenceBundle {
    b.v = EVIDENCE_V3;
    b.provenance = Some(BTreeMap::new());
    b
}

fn put(b: &mut EvidenceBundle, label: &str, env: Dsse) {
    b.provenance.as_mut().unwrap().insert(label.into(), env);
}

/// A v3 bundle whose `labels` carry their node's honest statement.
fn with(w: &World, b: EvidenceBundle, labels: &[&str]) -> EvidenceBundle {
    let mut b = v3(b);
    for l in labels {
        let st = statement(w, &b, l, |_| {}, |_| {});
        let env = sign(w, &node_of(w, &b, l), &st);
        put(&mut b, l, env);
    }
    b
}

fn good(w: &World) -> EvidenceBundle {
    with(w, w.build(w.spec()), &["L1", "V1"])
}

fn status(s: &Summary, label: &str) -> StatementStatus {
    s.jobs[label].provenance.status.clone()
}

#[test]
fn required_checks_every_statement_and_names_each_check() {
    let w = World::new();
    let mut r = Run::new(&w, ProvenanceRule::Required);
    r.policy.min_k2node = Some("0.46.10".into());
    r.policy.build.get_mut("linux-x86_64").unwrap().network = vec![Network::None];
    r.policy.check().unwrap();
    let s = r.passes(&w, &good(&w));
    for label in ["L1", "V1"] {
        let p = &s.jobs[label].provenance;
        assert_eq!(p.status, StatementStatus::Checked, "{label}");
        let names: Vec<&str> = p.checks.iter().map(|c| c.check.as_str()).collect();
        assert_eq!(names, ["signature", "receipt", "owner", "key active", "time", "k2-node", "network"], "{label}");
        assert!(p.checks.iter().all(|c| c.ok && !c.detail.is_empty()), "{p:?}");
        assert!(p.line().starts_with("build statement checked: signature passed, receipt passed, owner passed"), "{}", p.line());
    }
    let detail = |label: &str, check: &str| s.jobs[label].provenance.checks.iter().find(|c| c.check == check).unwrap().detail.clone();
    assert!(detail("L1", "signature").contains("node-a"), "{}", detail("L1", "signature"));
    assert!(detail("V1", "signature").contains("node-b"));
    assert!(detail("L1", "receipt").contains("1 file(s)"));
    assert!(detail("L1", "owner").contains("owner.example"));
    assert!(detail("L1", "time").contains("first approval"));
    assert!(detail("L1", "network").contains("network none"));
    assert!(s.notes.is_empty(), "{:?}", s.notes);
    // The verdict and the registered summary carry it as JSON.
    let v = serde_json::to_value(&s.jobs["L1"]).unwrap();
    assert_eq!(v["provenance"]["status"], "checked");
    assert_eq!(v["provenance"]["checks"][0]["check"], "signature");
    assert_eq!(v["provenance"]["checks"][0]["ok"], true);

    // A rehearsal has no approval to anchor the start; the time check says so.
    let mut r = Run::new(&w, ProvenanceRule::Required);
    r.rehearse = true;
    let s = r.passes(&w, &good(&w));
    let t = s.jobs["L1"].provenance.checks.iter().find(|c| c.check == "time").unwrap();
    assert!(t.ok && t.detail.contains("rehearsal"), "{t:?}");
}

#[test]
fn required_refuses_a_receipt_without_a_statement() {
    let w = World::new();
    let r = Run::new(&w, ProvenanceRule::Required);
    let b = with(&w, w.build(w.spec()), &["L1"]);
    let e = r.refuses(&w, &b, "provenance_missing: V1: the bundle has no build statement for it");
    assert!(e.contains("provenance = \"required\""), "{e}");
    let b = v3(w.build(w.spec()));
    r.refuses(&w, &b, "provenance_missing: L1:");
}

#[test]
fn required_refuses_a_v2_bundle_by_name() {
    let w = World::new();
    let r = Run::new(&w, ProvenanceRule::Required);
    let e = r.refuses(&w, &w.build(w.spec()), "provenance_v2_bundle: the evidence is a v2 bundle");
    assert!(e.contains("k2 ci evidence"), "{e}");
}

#[test]
fn a_statement_that_disagrees_with_its_receipt_is_refused() {
    let w = World::new();
    let r = Run::new(&w, ProvenanceRule::Required);
    // One file's digest differs from the receipt's (PRD §14).
    let mut b = good(&w);
    let st = statement(&w, &b, "L1", |_| {}, |st| {
        st.subject[0].digest.insert("sha256".into(), "9".repeat(64));
    });
    put(&mut b, "L1", sign(&w, "node-a", &st));
    let e = r.refuses(&w, &b, "provenance_mismatch: L1: build statement: receipt failed: the statement's subjects (artifacts) is not the receipt's");
    assert!(e.ends_with("(passed: signature)"), "{e}");
    // Another commit.
    let mut b = good(&w);
    let st = statement(&w, &b, "L1", |_| {}, |st| {
        st.predicate.build_definition.external_parameters.source.as_mut().unwrap().digest.insert("gitCommit".into(), "3".repeat(40));
    });
    put(&mut b, "L1", sign(&w, "node-a", &st));
    r.refuses(&w, &b, "receipt failed: the statement's commit or tree is not the receipt's");
    // The verify build's statement filed under the build's label: honest,
    // but another node's, so it doesn't verify with L1's key.
    let mut b = good(&w);
    let v1 = b.provenance.as_ref().unwrap()["V1"].clone();
    put(&mut b, "L1", v1);
    r.refuses(&w, &b, "provenance_mismatch: L1: build statement: signature failed");
    // L1's own statement, re-signed by another node in the roster.
    let mut b = good(&w);
    let st = statement(&w, &b, "L1", |_| {}, |_| {});
    put(&mut b, "L1", sign(&w, "node-c", &st));
    r.refuses(&w, &b, "provenance_mismatch: L1: build statement: signature failed");
    // A payload edited after signing.
    let mut b = good(&w);
    let mut env = b.provenance.as_ref().unwrap()["L1"].clone();
    let body = String::from_utf8(k2_node_proto::crypto::unb64(&env.payload).unwrap()).unwrap();
    env.payload = k2_node_proto::crypto::b64(body.replace("\"attempt\":1", "\"attempt\":2").as_bytes());
    put(&mut b, "L1", env);
    r.refuses(&w, &b, "L1: build statement: signature failed: the signature does not verify");
}

#[test]
fn builder_id_must_name_the_pinned_roster_key() {
    let w = World::new();
    let r = Run::new(&w, ProvenanceRule::Required);
    // The owner-server form: the node held no roster when it signed.
    let mut b = good(&w);
    let st = statement(&w, &b, "L1", |f| f.owner = Owner::Server("e".repeat(64)), |_| {});
    put(&mut b, "L1", sign(&w, "node-a", &st));
    let e = r.refuses(&w, &b, "provenance_owner: L1: build statement: owner failed: builder.id is the owner-server form");
    assert!(e.ends_with("(passed: signature, receipt)"), "{e}");
    // Another owner's roster key.
    let mut b = good(&w);
    let st = statement(&w, &b, "L1", |f| f.owner = Owner::RosterKey(w.owner2_fp.clone()), |_| {});
    put(&mut b, "L1", sign(&w, "node-a", &st));
    r.refuses(&w, &b, &format!("owner failed: builder.id names roster key {}, not the pinned owner key {}", w.owner2_fp, w.owner_fp));
}

#[test]
fn the_node_key_must_be_active_for_the_whole_job() {
    let w = World::new();
    let r = Run::new(&w, ProvenanceRule::Required);
    // Retired while the job ran (started NOW-1800, ended NOW-600): the
    // receipt's own rule (not retired before it started) passes, but the
    // statement was signed at the end, after the key was retired.
    let mut s = w.spec();
    let fp = w.fp("node-a");
    let e = s.roster.entries.iter_mut().find(|e| e.node_fp == fp).unwrap();
    e.status = KeyStatus::Retired;
    e.valid_to = Some(NOW - 1200);
    let b = with(&w, w.build(s), &["L1", "V1"]);
    let msg = r.refuses(&w, &b, "provenance_key_inactive: L1: build statement: key active failed: node node-a's key was retired");
    assert!(msg.contains("roster seq 1") && msg.contains("(passed: signature, receipt, owner)"), "{msg}");
    // Retired after the job ended: still valid for it, and said so.
    let mut s = w.spec();
    let e = s.roster.entries.iter_mut().find(|e| e.node_fp == fp).unwrap();
    e.status = KeyStatus::Retired;
    e.valid_to = Some(NOW - 300);
    let sum = r.passes(&w, &with(&w, w.build(s), &["L1", "V1"]));
    let k = sum.jobs["L1"].provenance.checks.iter().find(|c| c.check == "key active").unwrap();
    assert!(k.ok && k.detail.contains("retired, but still valid"), "{k:?}");
}

#[test]
fn the_network_must_be_one_the_build_accepts() {
    let w = World::new();
    let mut r = Run::new(&w, ProvenanceRule::Required);
    r.policy.build.get_mut("linux-x86_64").unwrap().network = vec![Network::None, Network::NoLan];
    let mut b = good(&w);
    let st = statement(&w, &b, "V1", |f| f.network = Network::On, |_| {});
    put(&mut b, "V1", sign(&w, "node-b", &st));
    let e = r.refuses(&w, &b, "provenance_network: V1: build statement: network failed: the job ran with network on, and build linux-x86_64 accepts none, no-lan");
    assert!(e.contains("passed: signature, receipt, owner, key active, time"), "{e}");
    // Under prefer, a build that names its networks still needs the
    // statement: nothing else records the network.
    r.policy.provenance = ProvenanceRule::Prefer;
    let b = with(&w, w.build(w.spec()), &["L1"]);
    r.refuses(&w, &b, "provenance_missing: V1: the bundle has no build statement for it, and build linux-x86_64 accepts only some networks (network = [none, no-lan])");
    r.refuses(&w, &w.build(w.spec()), "provenance_missing: L1: the bundle is v2");
}

#[test]
fn min_k2node_holds_for_every_receipt_whatever_provenance_says() {
    let w = World::new();
    for mode in [ProvenanceRule::Off, ProvenanceRule::Prefer, ProvenanceRule::Required] {
        let mut r = Run::new(&w, mode);
        r.policy.min_k2node = Some("0.46.13".into());
        let e = r.refuses(&w, &good(&w), "k2node_too_old: L1: node node-a ran k2-node 0.46.10, and project acme-desktop accepts 0.46.13 or later");
        assert!(e.contains("min_k2node"), "{mode:?}: {e}");
        r.policy.min_k2node = Some("0.46.10".into());
        r.passes(&w, &good(&w));
    }
}

/// Bundle v2/v3 × off/prefer/required, with and without statements.
#[test]
fn v2_and_v3_bundles_under_each_policy() {
    use ProvenanceRule::{Off, Prefer, Required};
    use StatementStatus::{Absent, Checked, NotRead};
    let w = World::new();
    let bundles: [(&str, EvidenceBundle); 4] = [
        ("v2", w.build(w.spec())),
        ("v3 none", v3(w.build(w.spec()))),
        ("v3 L1 only", with(&w, w.build(w.spec()), &["L1"])),
        ("v3 both", good(&w)),
    ];
    // (bundle, mode) → Ok([L1, V1] statuses, a note containing) or Err(refusal containing).
    type Want = Result<([StatementStatus; 2], Option<&'static str>), &'static str>;
    let cases: [(&str, ProvenanceRule, Want); 12] = [
        ("v2", Off, Ok(([NotRead, NotRead], None))),
        ("v2", Prefer, Ok(([Absent, Absent], Some("V1: the bundle is v2, which carries no build statements (node node-b; provenance = prefer)")))),
        ("v2", Required, Err("provenance_v2_bundle")),
        ("v3 none", Off, Ok(([NotRead, NotRead], None))),
        ("v3 none", Prefer, Ok(([Absent, Absent], Some("L1: the bundle has no build statement for it")))),
        ("v3 none", Required, Err("provenance_missing: L1")),
        ("v3 L1 only", Off, Ok(([NotRead, NotRead], Some("provenance = off: the bundle's 1 build statement(s) were not read")))),
        ("v3 L1 only", Prefer, Ok(([Checked, Absent], Some("V1: the bundle has no build statement for it")))),
        ("v3 L1 only", Required, Err("provenance_missing: V1")),
        ("v3 both", Off, Ok(([NotRead, NotRead], Some("the bundle's 2 build statement(s) were not read")))),
        ("v3 both", Prefer, Ok(([Checked, Checked], None))),
        ("v3 both", Required, Ok(([Checked, Checked], None))),
    ];
    for (name, mode, want) in cases {
        let b = &bundles.iter().find(|(n, _)| *n == name).unwrap().1;
        let r = Run::new(&w, mode);
        match (r.go(&w, b), want) {
            (Ok(s), Ok((statuses, note))) => {
                assert_eq!([status(&s, "L1"), status(&s, "V1")], statuses, "{name} {mode:?}");
                match note {
                    Some(n) => assert!(s.notes.iter().any(|x| x.contains(n)), "{name} {mode:?}: notes {:?} lack {n:?}", s.notes),
                    None => assert!(s.notes.is_empty(), "{name} {mode:?}: {:?}", s.notes),
                }
            }
            (Err(e), Err(text)) => assert!(e.contains(text), "{name} {mode:?}: {e:?} lacks {text:?}"),
            (got, want) => panic!("{name} {mode:?}: got {:?}, want {want:?}", got.map(|s| s.jobs.keys().cloned().collect::<Vec<_>>())),
        }
    }
    // A statement that is there must pass under prefer; under off it is
    // not read at all.
    let mut bad = good(&w);
    let st = statement(&w, &bad, "L1", |f| f.owner = Owner::Server("e".repeat(64)), |_| {});
    put(&mut bad, "L1", sign(&w, "node-a", &st));
    Run::new(&w, Prefer).refuses(&w, &bad, "provenance_owner: L1");
    let s = Run::new(&w, Off).passes(&w, &bad);
    assert_eq!(status(&s, "L1"), NotRead);
}
