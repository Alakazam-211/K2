//! Build statements on the real node (prd-compute-build-attestations-v1
//! S2) against the loopback fake controller:
//! - a protocol-10 server is offered `provenance-v1`; a job whose plan asks
//!   and that keeps artifacts ends with a DSSE envelope beside its receipt,
//!   signed by the node key, that agrees with the receipt by construction;
//! - what the job writes reaches the statement only as subjects: a fake
//!   statement it writes is just another file, and a symlink to the node
//!   key is never followed or kept;
//! - no artifact (or no ask): no statement; a server below protocol 10
//!   never sees the feature, and a plan asking it anyway (or for an unknown
//!   version) is refused before any ledger row.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::sync::atomic::Ordering;

use common::*;
use k2_node_proto::ci::{Isolation, Wall};
use k2_node_proto::frames::{self, Frame, JobState, Receipt};
use k2_node_proto::provenance::{self, FEATURE_PROVENANCE_V1, PROVENANCE_V1};
use k2_node_proto::share::Network;

fn offer_features(fs: &[Frame]) -> Vec<String> {
    fs.iter().rev().find_map(|f| if let Frame::Offer(o) = f { Some(o.features.clone()) } else { None }).expect("an offer")
}

fn receipt_of(fs: &[Frame], job: &str) -> frames::ReceiptFrame {
    fs.iter().find_map(|f| if let Frame::Receipt(r) = f { (r.job_id == job).then(|| r.clone()) } else { None }).expect("a receipt")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_with_artifacts_gets_a_node_signed_statement_that_is_its_receipts() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "prov-e2e", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let first = c.until(|f| matches!(f, Frame::Offer(_))).await;
    assert!(offer_features(&first).iter().any(|f| f == FEATURE_PROVENANCE_V1), "offered to a protocol-10 server");

    // The job writes a real output, a forged statement of its own, and a
    // link to the node key (the collector runs as the node user).
    let key_path = n.layout.key();
    let script = format!(
        "mkdir -p dist && printf 'bin' > dist/a.bin && \
         printf '%s' '{{\"payloadType\":\"application/vnd.in-toto+json\",\"payload\":\"e30=\",\"signatures\":[]}}' > dist/fake.intoto.jsonl && \
         ln -s '{}' dist/key",
        key_path.display()
    );
    let mut p = plan("prov1", &["/bin/sh", "-c", &script]);
    p.artifacts = vec!["dist/*".into()];
    p.provenance = Some(PROVENANCE_V1);
    c.send(Frame::Assign(assign(p.clone(), 1)));
    let fs = run_to_receipt(&mut c, "prov1").await;
    let end = states_of(&fs, "prov1").pop().expect("a state");
    assert_eq!((end.state, end.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(0)));
    let rf = receipt_of(&fs, "prov1");
    let receipt: Receipt = rf.receipt.verify(n.node.key.spki_der()).expect("the receipt verifies");
    let names: Vec<&str> = receipt.artifacts.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["dist/a.bin", "dist/fake.intoto.jsonl"], "the link to the key is never kept");

    let env = rf.provenance.clone().expect("a statement beside the receipt");
    assert_eq!(env.keyid(), Some(n.node.key.fingerprint().as_str()));
    let st = env.open_for_receipt(n.node.key.spki_der(), &rf.receipt).expect("it verifies and agrees with its receipt");
    let subjects: Vec<(&str, &str)> = st.subject.iter().map(|s| (s.name.as_deref().unwrap(), s.digest["sha256"].as_str())).collect();
    assert_eq!(subjects, receipt.artifacts.iter().map(|a| (a.name.as_str(), a.sha256.as_str())).collect::<Vec<_>>());
    assert_eq!(st.subject[0].digest["sha256"], k2_node_proto::crypto::sha256_hex(b"bin"));
    // The forged file is only a subject; the envelope is the node's.
    assert_ne!(env.payload, "e30=");
    // What the node observed: no wall in a test node, the policy's network,
    // its owner server (no roster held), its own version and key.
    let ip = &st.predicate.build_definition.internal_parameters;
    assert_eq!((ip.isolation, ip.wall, ip.tenant_slot, ip.network), (Isolation::SharedUid, Wall::None, 0, Network::On));
    assert_eq!(ip.plan_digest, p.digest().unwrap());
    let rd = &st.predicate.run_details;
    assert_eq!(rd.builder.id, format!("{}{}", provenance::BUILDER_ID_OWNER_SERVER, fake.key.fingerprint()));
    assert_eq!(rd.builder.version.get("k2-node").map(String::as_str), Some(k2_node::node::NODE_VERSION));
    assert_eq!(rd.builder.version.get("protocol").map(String::as_str), Some(frames::PROTOCOL.to_string().as_str()));
    let rc = rd.byproducts.iter().find(|d| d.name.as_deref() == Some(provenance::names::RECEIPT)).unwrap();
    assert_eq!(rc.digest["sha256"], k2_node_proto::crypto::sha256_hex(rf.receipt.body.as_bytes()));
    // Kept with the receipt in the node's ledger (shipped again after a reconnect).
    let row = n.node.owner().ledger.lock().unwrap().get("prov1", 1).unwrap().unwrap();
    assert_eq!(row.provenance.as_ref(), Some(&env));
    assert_eq!(row.receipt.as_ref(), Some(&rf.receipt));
    // Nothing in the job's notes: it worked.
    let notes = end.outputs.as_ref().map(|o| o.notes.clone()).unwrap_or_default();
    assert!(!notes.iter().any(|x| x.contains("build statement")), "{notes:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn no_artifact_or_no_ask_means_no_statement() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "prov-none", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    c.until(|f| matches!(f, Frame::Offer(_))).await;
    // Asked, but nothing kept (the glob matches nothing).
    let mut p = plan("noart", &["/bin/sh", "-c", "echo none"]);
    p.artifacts = vec!["dist/*".into()];
    p.provenance = Some(PROVENANCE_V1);
    c.send(Frame::Assign(assign(p, 1)));
    let fs = run_to_receipt(&mut c, "noart").await;
    let rf = receipt_of(&fs, "noart");
    assert!(rf.receipt.verify(n.node.key.spki_der()).unwrap().artifacts.is_empty());
    assert_eq!(rf.provenance, None, "no artifacts, no statement");
    assert!(!serde_json::to_string(&rf).unwrap().contains("provenance"), "the frame has no key for it");
    // Kept, but not asked.
    let mut p = plan("noask", &["/bin/sh", "-c", "printf x > out.txt"]);
    p.artifacts = vec!["out.txt".into()];
    c.send(Frame::Assign(assign(p, 1)));
    let fs = run_to_receipt(&mut c, "noask").await;
    let rf = receipt_of(&fs, "noask");
    assert_eq!(rf.receipt.verify(n.node.key.spki_der()).unwrap().artifacts.len(), 1);
    assert_eq!(rf.provenance, None, "not asked");
    // An unknown statement version: refused before any row.
    let mut p = plan("v2stmt", &["/bin/sh", "-c", "echo no"]);
    p.provenance = Some(2);
    c.send(Frame::Assign(assign(p, 1)));
    let fs = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "v2stmt")).await;
    let s = states_of(&fs, "v2stmt").pop().unwrap();
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(k2_node::node::provenance::PROVENANCE_V_UNKNOWN)));
    assert!(n.node.owner().ledger.lock().unwrap().get("v2stmt", 1).unwrap().is_none(), "no ledger row");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_protocol_9_server_never_sees_the_feature_and_cant_ask() {
    let mut fake = Fake::start().await;
    fake.server_protocol.store(frames::PROTOCOL_CI, Ordering::SeqCst);
    let n = start_node(&fake, "prov-p9", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let first = c.until(|f| matches!(f, Frame::Offer(_))).await;
    assert!(!offer_features(&first).iter().any(|f| f == FEATURE_PROVENANCE_V1), "not offered below protocol 10");
    let mut p = plan("p9ask", &["/bin/sh", "-c", "printf x > out.txt"]);
    p.artifacts = vec!["out.txt".into()];
    p.provenance = Some(PROVENANCE_V1);
    c.send(Frame::Assign(assign(p, 1)));
    let fs = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "p9ask")).await;
    let s = states_of(&fs, "p9ask").pop().unwrap();
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(k2_node::node::provenance::PROVENANCE_V_UNKNOWN)));
    assert!(n.node.owner().ledger.lock().unwrap().get("p9ask", 1).unwrap().is_none(), "no ledger row");
}
