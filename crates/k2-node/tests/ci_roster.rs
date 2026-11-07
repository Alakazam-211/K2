//! The owner's node roster on the node (protocol 9, C1; vs-live V11) and
//! lanes vs older servers (C3a), over the loopback fake controller:
//! - the owner server hands the node its signed roster (`node_roster.set`,
//!   an operate command); the node pins the roster key, keeps only a rising
//!   `seq`, refuses another owner's roster, and relays it with each server's
//!   **own** seats to protocol-9 sessions, once per session and on change;
//! - a protocol-8 server (K2 Engine, K2 Cores, Sew on 0.46.9) on a node
//!   that holds a roster and a lanes file sees 0.46.9's node: no
//!   `node-roster` or `lanes` feature, no `node_roster` frame, and its jobs
//!   run as before.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::sync::atomic::Ordering;

use common::*;
use k2_node::admin::config::{self as acfg, AdminFile};
use k2_node_proto::admin::{self, ApprovalSigner, Class, Command};
use k2_node_proto::ci::{self, KeyStatus, SeatState, TaintBasis};
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{self, AdminFrame, Frame, JobState};
use k2_node_proto::lanes::{codes, NodeRosterDoc};
use k2_node_proto::node_roster::{NodeRoster, RosterEntry, SeatStatement};
use serde_json::Value;

fn admin_toml(layout: &k2_node::paths::Layout, key: &SigningKey, level: &str) {
    acfg::write(
        &layout.config.join(acfg::ADMIN_FILE),
        &AdminFile { remote_admin: level.into(), controller_fp: Some(key.fingerprint()), controller_key: Some(crypto::b64(key.spki_der())), ..Default::default() },
    )
    .unwrap();
}

fn cmd(key: &SigningKey, node_fp: &str, verb: &str, args: Value) -> admin::SignedCommand {
    let now = k2_node::util::now();
    let c = Command {
        v: admin::ADMIN_V,
        cmd_id: format!("r{}", &crypto::random_id().unwrap()[..8]),
        node_fp: node_fp.into(),
        controller_fp: key.fingerprint(),
        class: Class::Operate,
        verb: verb.into(),
        args,
        plan: None,
        proposed_by: "controller:node-roster".into(),
        approved_by: None,
        approved_via: None,
        request_id: None,
        iat: now,
        exp: now + 300,
        nonce: admin::new_nonce().unwrap(),
    };
    admin::sign_controller(key, &c).unwrap()
}

async fn ask(c: &mut Conn, s: admin::SignedCommand) -> admin::AdminResult {
    ask_seen(c, s).await.0
}

/// The result and every frame up to it (a relay sent while the verb ran
/// arrives before its result).
async fn ask_seen(c: &mut Conn, s: admin::SignedCommand) -> (admin::AdminResult, Vec<Frame>) {
    let body: Command = serde_json::from_slice(&crypto::unb64(&s.body_b64).unwrap()).unwrap();
    c.send(Frame::Admin(AdminFrame { cmd: s }));
    let fr = c.until(|f| matches!(f, Frame::AdminResult(r) if r.cmd_id == body.cmd_id || r.cmd_id.is_empty())).await;
    let seen = fr.clone();
    let r = match fr.into_iter().last() {
        Some(Frame::AdminResult(r)) => r,
        o => panic!("{o:?}"),
    };
    (r, seen)
}

/// A roster naming this node `n1`, from the owner whose controller key is
/// `owner_fp`, with seats for `clients`.
fn doc(o: &ApprovalSigner, node: &SigningKey, owner_fp: &str, seq: u64, clients: &[&str]) -> NodeRosterDoc {
    let r = NodeRoster {
        v: 1,
        owner_id: "owner.k2.dev".into(),
        owner_server_fp: owner_fp.into(),
        roster_key: o.public().encode(),
        seq,
        issued_at: 1_800_000_000 + seq as i64,
        prev_digest: (seq > 1).then(|| format!("sha256:{}", "0".repeat(64))),
        entries: vec![RosterEntry {
            node_fp: node.fingerprint(),
            node_spki: crypto::b64(node.spki_der()),
            name: "n1".into(),
            key_epoch: 0,
            statement_digest: format!("sha256:{}", "1".repeat(64)),
            status: KeyStatus::Active,
            revoked_reason: None,
            valid_from: 1,
            valid_to: None,
            taint_basis: Some(TaintBasis::OwnerAttested),
            attestation_digest: Some(format!("sha256:{}", "2".repeat(64))),
            host: "h1".into(),
            provider: None,
            site: None,
            admin_reach: vec![],
            pod: None,
            point: None,
        }],
    };
    let seats = clients
        .iter()
        .enumerate()
        .map(|(i, c)| {
            SeatStatement {
                v: 1,
                owner_id: "owner.k2.dev".into(),
                roster_key: o.public().encode(),
                seat_id: format!("seat-{i}"),
                client_fp: c.to_string(),
                client_label: format!("client-{i}"),
                lane: "ci-standard".into(),
                nodes: vec!["n1".into(), "n2".into()],
                roster_seq: seq,
                state: SeatState::Active,
                issued_at: 1_800_000_000,
                pod: None,
                line: None,
            }
            .sign(o)
            .unwrap()
        })
        .collect();
    NodeRosterDoc { roster: r.sign(o).unwrap(), seats }
}

fn rosters(fs: &[Frame]) -> Vec<NodeRosterDoc> {
    fs.iter().filter_map(|f| if let Frame::NodeRoster(d) = f { Some(d.clone()) } else { None }).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_node_pins_keeps_and_relays_the_owners_roster() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "roster-relay", "", |_| {}).await;
    admin_toml(&n.layout, &fake.key, "policy");
    let mut c = fake.next_conn().await;
    let first = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = first.iter().rev().find(|f| matches!(f, Frame::Offer(_))).cloned() else { unreachable!() };
    assert!(o.features.iter().any(|f| f == ci::FEATURE_NODE_ROSTER), "{:?}", o.features);
    let fp = n.node.key.fingerprint();
    let owner = ApprovalSigner::from_seed(&[11u8; 32]).unwrap();
    let other_client = "b".repeat(64);
    let d2 = doc(&owner, &n.node.key, &fake.key.fingerprint(), 2, &[&fake.key.fingerprint(), &other_client]);
    let (r, seen) = ask_seen(&mut c, cmd(&fake.key, &fp, "node_roster.set", serde_json::to_value(&d2).unwrap())).await;
    assert_eq!(r.state, "done", "{r:?}");
    assert_eq!((r.result["seq"].as_u64(), r.result["name"].as_str()), (Some(2), Some("n1")));
    // Relayed at once to this (protocol-9) session, with its own seat only
    // (sent while the verb ran, so before its result).
    let mut fs = rosters(&seen);
    if fs.is_empty() {
        fs = rosters(&c.until(|f| matches!(f, Frame::NodeRoster(_))).await);
    }
    let got = fs.pop().unwrap();
    assert_eq!(got.roster, d2.roster);
    assert_eq!(got.seats.len(), 1, "never another client's seat");
    let st = SeatStatement::open(&got.seats[0], &owner.public()).unwrap();
    assert_eq!(st.client_fp, fake.key.fingerprint());
    assert_eq!(n.node.roster_seq_seen(), Some(2));
    // An older roster: refused. Another key: refused. Another owner: refused.
    let d1 = doc(&owner, &n.node.key, &fake.key.fingerprint(), 1, &[]);
    let r = ask(&mut c, cmd(&fake.key, &fp, "node_roster.set", serde_json::to_value(&d1).unwrap())).await;
    assert_eq!(r.code.as_deref(), Some(codes::ROSTER_OLDER), "{r:?}");
    let evil = ApprovalSigner::from_seed(&[12u8; 32]).unwrap();
    let dx = doc(&evil, &n.node.key, &fake.key.fingerprint(), 9, &[]);
    let r = ask(&mut c, cmd(&fake.key, &fp, "node_roster.set", serde_json::to_value(&dx).unwrap())).await;
    assert_eq!(r.code.as_deref(), Some(codes::ROSTER_KEY_CHANGED), "{r:?}");
    let dy = doc(&owner, &n.node.key, &"c".repeat(64), 3, &[]);
    let r = ask(&mut c, cmd(&fake.key, &fp, "node_roster.set", serde_json::to_value(&dy).unwrap())).await;
    assert_eq!(r.code.as_deref(), Some(codes::ROSTER_WRONG_OWNER), "{r:?}");
    // A tampered roster never verifies.
    let mut bad = d2.clone();
    bad.roster.body = bad.roster.body.replace("\"h1\"", "\"h9\"");
    let r = ask(&mut c, cmd(&fake.key, &fp, "node_roster.set", serde_json::to_value(&bad).unwrap())).await;
    assert_eq!(r.state, "refused", "{r:?}");
    assert_eq!(n.node.roster_seq_seen(), Some(2), "nothing replaced the good one");
    // Receipt v2 says which roster the node held and the lane this server's
    // seat runs under (root's lanes file, by its pinned key).
    let mut lanes = k2_node_proto::lanes::LanesFile::default();
    lanes.lanes.insert("ci-standard".into(), k2_node_proto::lanes::LaneTemplate::ci_standard());
    lanes.seats.push(k2_node_proto::lanes::LaneSeat { seat: "seat-0".into(), server_fp: fake.key.fingerprint(), lane: "ci-standard".into(), label: "c".into() });
    std::fs::write(n.node.lanes_file(), lanes.render().unwrap()).unwrap();
    let mut p = plan("v2lane", &["/bin/sh", "-c", "echo lane"]);
    p.receipt_v = Some(frames::RECEIPT_V2);
    c.send(Frame::Assign(assign(p, 1)));
    let fs = run_to_receipt(&mut c, "v2lane").await;
    let r = fs.iter().find_map(|f| if let Frame::Receipt(r) = f { (r.job_id == "v2lane").then(|| r.clone()) } else { None }).expect("a receipt");
    let rc = r.receipt.verify(n.node.key.spki_der()).unwrap();
    let run = rc.run.expect("v2 run facts");
    assert_eq!((run.roster_seq_seen, run.lane.as_deref()), (Some(2), Some("ci-standard")));
    // A new session gets the held roster at its start.
    c.tx.send(Out::Close).unwrap();
    let mut c2 = fake.next_conn().await;
    let fs = c2.until(|f| matches!(f, Frame::NodeRoster(_))).await;
    assert_eq!(rosters(&fs).pop().unwrap().roster, d2.roster);
}

/// Mixed versions: a protocol-8 server on a node that holds a roster and a
/// lanes file sees 0.46.9's node, and its jobs run.
#[tokio::test(flavor = "multi_thread")]
async fn a_protocol_8_server_on_a_node_with_lanes_still_works() {
    let mut fake = Fake::start().await;
    fake.server_protocol.store(frames::PROTOCOL_GIT, Ordering::SeqCst);
    let n = start_node(&fake, "roster-p8", "", |_| {}).await;
    // The node already holds its owner's roster (relayed to protocol-9
    // servers only) and a lanes file (root's, read by nothing a protocol-8
    // server sees).
    let owner = ApprovalSigner::from_seed(&[13u8; 32]).unwrap();
    let d = doc(&owner, &n.node.key, &fake.key.fingerprint(), 1, &[&fake.key.fingerprint()]);
    n.node.set_roster(&serde_json::to_value(&d).unwrap()).unwrap();
    let mut lanes = k2_node_proto::lanes::LanesFile::default();
    lanes.lanes.insert("ci-standard".into(), k2_node_proto::lanes::LaneTemplate::ci_standard());
    lanes.seats.push(k2_node_proto::lanes::LaneSeat { seat: "seat-0".into(), server_fp: "d".repeat(64), lane: "ci-standard".into(), label: "c".into() });
    std::fs::write(n.layout.config.join("k2-node-lanes.toml"), lanes.render().unwrap()).unwrap();

    let mut c = fake.next_conn().await;
    let first = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = first.iter().rev().find(|f| matches!(f, Frame::Offer(_))).cloned() else { unreachable!() };
    for f in [ci::FEATURE_NODE_ROSTER, ci::FEATURE_LANES] {
        assert!(!o.features.iter().any(|x| x == f), "{f} offered to a protocol-8 server");
    }
    c.send(Frame::Assign(assign(plan("p8job", &["/bin/sh", "-c", "echo still works"]), 1)));
    let fs = run_to_receipt(&mut c, "p8job").await;
    let end = states_of(&fs, "p8job").pop().unwrap();
    assert_eq!((end.state, end.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(0)));
    assert!(!fs.iter().chain(first.iter()).any(|f| matches!(f, Frame::NodeRoster(_))), "no node_roster frame to a protocol-8 server");
    assert!(n.node.roster_for(&n.node.owner()).is_none(), "a protocol-8 session never gets the roster");
}
