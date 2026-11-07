//! Protocol 9 on the node (prd-ci-on-compute-for-clients-v1 slice C0,
//! vs-live V7–V10), against a loopback controller, mixed versions both
//! ways:
//! - a protocol-8 server (0.46.9) sees 0.46.9's node: no protocol-9
//!   feature, no `node_statement` frame, v1 receipts with no `run` and no
//!   statement beside them; a `receipt_v` it could never have sent is
//!   refused before any ledger row;
//! - a protocol-9 server gets the signed self-statement (once per session
//!   and when it changes), receipt v2 only when the plan asks, with the
//!   statement its `run.statement_digest` names riding beside it;
//! - V10: a job starting under the wall `none` taints the node key, and
//!   the statement says so from then on.
//! The other direction (a protocol-9 server never sends `receipt_v` to a
//! node without `receipt-v2`) is the daemon's placement, tested there.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use k2_node_proto::ci::{self, Isolation, Signed, Taint};
use k2_node_proto::frames::{self, Frame, JobState, Receipt};
use k2_node_proto::statement::NodeStatement;

const V9_FEATURES: &[&str] = &[ci::FEATURE_NODE_STATEMENT, ci::FEATURE_RECEIPT_V2];

fn offer_features(fs: &[Frame]) -> Vec<String> {
    fs.iter().rev().find_map(|f| if let Frame::Offer(o) = f { Some(o.features.clone()) } else { None }).expect("an offer")
}

fn receipt_of(fs: &[Frame], job: &str) -> frames::ReceiptFrame {
    fs.iter().find_map(|f| if let Frame::Receipt(r) = f { (r.job_id == job).then(|| r.clone()) } else { None }).expect("a receipt")
}

fn opened(r: &frames::ReceiptFrame, node_spki: &[u8]) -> Receipt {
    r.receipt.verify(node_spki).expect("the receipt verifies with the node key")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_protocol_8_server_sees_the_0469_node() {
    let mut fake = Fake::start().await;
    fake.server_protocol.store(frames::PROTOCOL_GIT, Ordering::SeqCst);
    let n = start_node(&fake, "p9-old-server", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let first = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let feats = offer_features(&first);
    for f in V9_FEATURES.iter().chain(ci::FEATURES) {
        assert!(!feats.iter().any(|x| x == f), "{f} offered to a protocol-8 server: {feats:?}");
    }
    // A job: v1, no run facts, nothing beside it, and never a statement frame.
    c.send(Frame::Assign(assign(plan("old1", &["/bin/sh", "-c", "echo ok"]), 1)));
    let fs = run_to_receipt(&mut c, "old1").await;
    assert!(!fs.iter().chain(first.iter()).any(|f| matches!(f, Frame::NodeStatement(_))), "no node_statement to a protocol-8 server");
    let r = receipt_of(&fs, "old1");
    assert_eq!(r.statement, None);
    assert!(!serde_json::to_string(&r).unwrap().contains("statement"), "the frame is 0.46.9's");
    let rc = opened(&r, n.node.key.spki_der());
    assert_eq!((rc.v, rc.run.is_none()), (frames::RECEIPT_V1, true));
    assert!(!r.receipt.body.contains("\"run\""), "{}", r.receipt.body);
    rc.check_version().unwrap();
    // A v2 plan from a session below 9 can't be honest: refused at once.
    let mut p = plan("old2", &["/bin/sh", "-c", "echo no"]);
    p.receipt_v = Some(frames::RECEIPT_V2);
    c.send(Frame::Assign(assign(p, 1)));
    let fs = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "old2")).await;
    let s = states_of(&fs, "old2").pop().unwrap();
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(k2_node::node::ci::RECEIPT_V_UNKNOWN)));
    let t = n.node.owner();
    assert!(t.ledger.lock().unwrap().get("old2", 1).unwrap().is_none(), "no ledger row");
    // The node itself still speaks 9 (its build marker and status say so).
    assert_eq!(k2_node::status::read(&n.layout.status()).unwrap().protocol, frames::PROTOCOL);
    assert_eq!(frames::PROTOCOL, frames::PROTOCOL_CI);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_protocol_9_server_gets_the_statement_and_receipt_v2_on_request() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "p9-new-server", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let first = c.until(|f| matches!(f, Frame::NodeStatement(_))).await;
    let feats = offer_features(&first);
    for f in V9_FEATURES {
        assert!(feats.iter().any(|x| x == f), "{f} not offered: {feats:?}");
    }
    let Some(Frame::NodeStatement(s0)) = first.last().cloned() else { unreachable!() };
    let st0 = NodeStatement::open(&s0).expect("the statement opens with its own key");
    assert_eq!(st0.node_fp, n.node.key.fingerprint(), "it names the node's own key");
    assert_eq!((st0.v, st0.key_epoch, st0.prev_fp.clone()), (1, 0, None));
    assert_eq!(st0.protocol, frames::PROTOCOL);
    assert_eq!(st0.k2node_version, k2_node::node::NODE_VERSION);
    assert_eq!(st0.k2node_commit.as_deref(), k2_node::build_commit());
    assert_eq!(st0.wall, ci::Wall::None, "a test node has no wall");
    assert!(st0.dev && st0.key_exposure.job_readable);
    // The key predates this process (the harness made it): its history is
    // unknown until a job taints it.
    assert_eq!(st0.taint, Taint::Unknown);
    assert_eq!(st0.os, k2_node::sysinfo::os_label());
    // A plain job: still receipt v1 (V7), and no statement beside it.
    c.send(Frame::Assign(assign(plan("v1job", &["/bin/sh", "-c", "echo one"]), 1)));
    let fs = run_to_receipt(&mut c, "v1job").await;
    let r = receipt_of(&fs, "v1job");
    let rc = opened(&r, n.node.key.spki_der());
    assert_eq!((rc.v, rc.run.is_none(), r.statement.is_none()), (frames::RECEIPT_V1, true, true));
    // V10: that job started without a wall, so the key is tainted now, and
    // a new statement says so (sent on change).
    let mut seen = fs.clone();
    let tainted = loop {
        if let Some(s) = seen.iter().filter_map(|f| if let Frame::NodeStatement(s) = f { Some(s.clone()) } else { None }).last() {
            let st = NodeStatement::open(&s).unwrap();
            if st.taint == Taint::Tainted {
                break s;
            }
        }
        seen = vec![tokio::time::timeout(Duration::from_secs(20), c.rx.recv()).await.expect("a new statement").expect("session")];
    };
    assert_ne!(tainted.digest(), s0.digest());
    let marks = k2_node::statement::read_taint(&n.layout.state().join(k2_node::statement::TAINT_FILE)).unwrap();
    assert!(marks.keys.contains_key(&n.node.key.fingerprint()), "the node's own mark");
    // Receipt v2, asked for: the run facts and the statement they name.
    let mut p = plan("v2job", &["/bin/sh", "-c", "echo two"]);
    p.receipt_v = Some(frames::RECEIPT_V2);
    c.send(Frame::Assign(assign(p.clone(), 1)));
    let fs = run_to_receipt(&mut c, "v2job").await;
    let end = states_of(&fs, "v2job").pop().expect("a state");
    assert_eq!((end.state, end.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(0)));
    let r = receipt_of(&fs, "v2job");
    let rc = opened(&r, n.node.key.spki_der());
    assert_eq!(rc.v, frames::RECEIPT_V2);
    rc.check_version().unwrap();
    let run = rc.run.clone().unwrap();
    assert_eq!(run.isolation, Isolation::SharedUid, "no wall: one uid for everything");
    assert_eq!((run.uid_index, run.wall, run.tenant_slot), (None, ci::Wall::None, 0));
    assert_eq!(run.k2node_version, k2_node::node::NODE_VERSION);
    assert_eq!(run.statement_digest, tainted.digest(), "the statement in force when it started");
    assert_eq!((run.roster_seq_seen, run.lane.clone()), (None, None));
    let st = r.statement.clone().expect("the statement rides beside a v2 receipt");
    let doc = rc.check_statement(&st).unwrap();
    assert_eq!(doc.taint, Taint::Tainted);
    // A doctored statement or another one fails the check.
    assert!(rc.check_statement(&s0).is_err(), "not the digest the receipt names");
    let forged = Signed { body: st.body.replace("\"dev\":true", "\"dev\":false"), sig: st.sig.clone() };
    assert!(rc.check_statement(&forged).is_err());
    // The plan digest still binds receipt_v (an older node would refuse it).
    assert_eq!(rc.plan_digest, p.digest().unwrap());
    // An exclusive v2 job says so.
    let mut p = plan("v2ex", &["/bin/sh", "-c", "echo ex"]);
    p.receipt_v = Some(frames::RECEIPT_V2);
    p.exclusive = true;
    c.send(Frame::Assign(assign(p, 1)));
    let fs = run_to_receipt(&mut c, "v2ex").await;
    let rc = opened(&receipt_of(&fs, "v2ex"), n.node.key.spki_der());
    assert_eq!(rc.run.unwrap().isolation, Isolation::Exclusive);
    // An unknown receipt version: refused before any row.
    let mut p = plan("v3job", &["/bin/sh", "-c", "echo three"]);
    p.receipt_v = Some(3);
    c.send(Frame::Assign(assign(p, 1)));
    let fs = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "v3job")).await;
    let s = states_of(&fs, "v3job").pop().unwrap();
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(k2_node::node::ci::RECEIPT_V_UNKNOWN)));
    // Status shows what the statement says.
    let status = k2_node::status::read(&n.layout.status()).unwrap();
    assert_eq!(status.taint.as_deref(), Some("tainted"));
    assert_eq!(status.statement_digest.as_deref(), Some(tainted.digest().as_str()));
}

/// The statement is sent again on every new session, and a stored one is
/// found by its digest after the node signed newer ones (a receipt shipped
/// after a reconnect still carries the statement its job started under).
#[tokio::test(flavor = "multi_thread")]
async fn the_statement_comes_with_every_session_and_old_ones_stay_findable() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "p9-resend", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let fs = c.until(|f| matches!(f, Frame::NodeStatement(_))).await;
    let Some(Frame::NodeStatement(s0)) = fs.last().cloned() else { unreachable!() };
    // Drop the session: the next one gets the statement in force again.
    c.tx.send(Out::Close).unwrap();
    let mut c2 = fake.next_conn().await;
    let fs = c2.until(|f| matches!(f, Frame::NodeStatement(_))).await;
    let Some(Frame::NodeStatement(s1)) = fs.last().cloned() else { unreachable!() };
    assert_eq!(s1, s0, "nothing changed: the same signed statement, same digest");
    let found = k2_node::statement::lookup(&n.layout.state(), &s0.digest()).expect("kept by digest");
    assert_eq!(found, s0);
}
