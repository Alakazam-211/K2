//! Store rules for node rosters, lanes, seats and signers (0274). An
//! isolated in-memory DB, generated keys (test keys only).

use std::collections::BTreeMap;

use super::*;
use crate::compute::proto::admin::ApprovalSigner;
use crate::compute::proto::ci::{KeyStore, RotationReason, SeatState, Wall};
use crate::compute::proto::crypto::{b64, SigningKey};
use crate::compute::proto::statement::KeyExposure;
use crate::compute::store;

fn conn() -> Connection {
    crate::db::isolated_test_connection()
}

fn statement(k: &SigningKey, taint: Taint, epoch: u32, prev: Option<String>) -> NodeStatement {
    NodeStatement {
        v: 1,
        node_fp: k.fingerprint(),
        node_spki: b64(k.spki_der()),
        key_epoch: epoch,
        key_born_at: 1_700_000_000,
        prev_fp: prev,
        issued_at: 1_700_000_100,
        wall: Wall::JobUser,
        dev: false,
        taint,
        key_exposure: KeyExposure { store: KeyStore::File, owner: "k2node".into(), mode: Some(0o600), job_readable: false },
        k2node_version: "0.46.9".into(),
        k2node_commit: Some("c".repeat(40)),
        protocol: 9,
        os: "linux".into(),
        arch: "x86_64".into(),
        distro: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        kernel: Some("6.8.0".into()),
        ptrace_scope: Some(1),
        host_id: None,
    }
}

fn add_node(c: &Connection, id: &str, name: &str, k: &SigningKey) -> NodeRow {
    let labels = BTreeMap::from([("os".to_string(), "linux".to_string()), ("arch".to_string(), "x86_64".to_string())]);
    store::insert_pending_node(c, id, name, &k.fingerprint(), &k.spki_pem(), &labels, "owner", "123456", 5).unwrap();
    store::confirm_node(c, id, "owner", 6).unwrap();
    store::node_by_id(c, id).unwrap()
}

fn entry_row(k: &SigningKey, name: &str, st: &Signed, basis: Option<TaintBasis>) -> EntryRow {
    EntryRow {
        entry: RosterEntry {
            node_fp: k.fingerprint(),
            node_spki: b64(k.spki_der()),
            name: name.into(),
            key_epoch: 0,
            statement_digest: st.digest(),
            status: KeyStatus::Active,
            revoked_reason: None,
            valid_from: 100,
            valid_to: None,
            taint_basis: basis,
            attestation_digest: None,
            host: format!("host-{name}"),
            provider: Some("vin".into()),
            site: None,
            admin_reach: vec!["fleet-ssh".into()],
            pod: None,
            point: None,
        },
        node_id: None,
        attestation: None,
        updated_at: 100,
    }
}

#[test]
fn taint_basis_rules() {
    let k = SigningKey::generate().unwrap();
    assert_eq!(taint_basis_for(&statement(&k, Taint::Clean, 0, None), false, false).unwrap(), TaintBasis::SelfClean);
    assert_eq!(taint_basis_for(&statement(&k, Taint::Unknown, 0, None), true, false).unwrap(), TaintBasis::OwnerAttested);
    assert_eq!(taint_basis_for(&statement(&k, Taint::Unknown, 0, None), false, false).unwrap_err().0, codes::NODE_KEY_TAINTED);
    assert_eq!(taint_basis_for(&statement(&k, Taint::Tainted, 0, None), true, false).unwrap_err().0, codes::NODE_KEY_TAINTED, "an attestation never covers a tainted key");
    let old = SigningKey::generate().unwrap();
    assert_eq!(taint_basis_for(&statement(&k, Taint::Clean, 1, Some(old.fingerprint())), false, true).unwrap(), TaintBasis::Rekeyed);
    let dev = NodeStatement { dev: true, ..statement(&k, Taint::Clean, 0, None) };
    assert!(taint_basis_for(&dev, true, false).is_err());
    let unwalled = NodeStatement { wall: Wall::None, ..statement(&k, Taint::Clean, 0, None) };
    assert!(taint_basis_for(&unwalled, true, false).is_err());
}

#[test]
fn rosters_chain_and_entries_change_detection() {
    let c = conn();
    let o = ApprovalSigner::from_seed(&[1u8; 32]).unwrap();
    let (a, b) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
    let sa = statement(&a, Taint::Clean, 0, None).sign(&a).unwrap();
    let sb = statement(&b, Taint::Unknown, 0, None).sign(&b).unwrap();
    assert!(!entries_changed(&c), "nothing yet");
    put_entry(&c, &entry_row(&a, "n1", &sa, Some(TaintBasis::SelfClean)), 100).unwrap();
    put_entry(&c, &entry_row(&b, "n2", &sb, None), 100).unwrap();
    // Two active keys under one name: refused.
    let c2 = SigningKey::generate().unwrap();
    assert!(put_entry(&c, &entry_row(&c2, "n1", &sa, None), 100).unwrap_err().contains("already has an active key"));
    assert!(entries_changed(&c));
    let r1 = next_roster(&c, "owner.k2.dev", &"e".repeat(64), &o.public(), 1000).unwrap();
    assert_eq!((r1.seq, r1.prev_digest.clone()), (1, None));
    assert!(r1.entries.windows(2).all(|w| w[0].node_fp < w[1].node_fp));
    let s1 = r1.sign(&o).unwrap();
    assert!(store_roster(&c, &s1, &r1, "self", 1000).unwrap());
    assert!(!store_roster(&c, &s1, &r1, "self", 1000).unwrap(), "idempotent");
    assert!(!entries_changed(&c));
    assert!(!roster_stale(&c, 1000 + ROSTER_REFRESH_SECS - 1));
    assert!(roster_stale(&c, 1000 + ROSTER_REFRESH_SECS));
    let r2 = next_roster(&c, "owner.k2.dev", &"e".repeat(64), &o.public(), 2000).unwrap();
    assert_eq!(r2.seq, 2);
    assert_eq!(r2.prev_digest.as_deref(), Some(s1.digest().as_str()));
    assert_eq!(r2.entries, r1.entries, "an unchanged re-sign");
    let s2 = r2.sign(&o).unwrap();
    store_roster(&c, &s2, &r2, "self", 2000).unwrap();
    assert_eq!(latest_own_roster(&c).unwrap().roster.seq, 2);
    // A clock that went back never makes issued_at fall.
    let r3 = next_roster(&c, "owner.k2.dev", &"e".repeat(64), &o.public(), 10).unwrap();
    assert_eq!(r3.issued_at, 2000);
}

#[test]
fn requests_decide_once_and_expire() {
    let c = conn();
    let body = json!({"op": "set", "node": "n1"});
    let mk = |by: &str, ws: Option<&str>| {
        insert_request(
            &c,
            &NewTrustRequest { kind: "roster", subject: "n1", body: &body, summary: "s", reason: None, proposed_by: by, proposer_workspace: ws, now: 100, ttl: 50 },
        )
    };
    let r = mk("agent:a", Some("ws-a")).unwrap();
    assert!(r.id.starts_with("ctr_") && r.state == "pending");
    assert_eq!(decide(&c, &r.id, "approved", "owner", Some("local-auth"), None, 120).unwrap().state, "approved");
    assert_eq!(decide(&c, &r.id, "approved", "owner", None, None, 121).unwrap_err(), codes::NOT_PENDING, "a replay finds nothing");
    let late = mk("agent:a", Some("ws-a")).unwrap();
    assert_eq!(decide(&c, &late.id, "approved", "owner", None, None, 150).unwrap_err(), codes::APPROVAL_EXPIRED);
    assert_eq!(request_by_id(&c, &late.id).unwrap().state, "expired");
    let open = mk("agent:a", Some("ws-a")).unwrap();
    assert_eq!(expire_due(&c, 151).len(), 1);
    assert_eq!(request_by_id(&c, &open.id).unwrap().state, "expired");
    for _ in 0..MAX_PENDING_PER_PROPOSER {
        mk("agent:b", Some("ws-b")).unwrap();
    }
    assert_eq!(mk("agent:b", Some("ws-b")).unwrap_err().0, codes::TOO_MANY_PENDING);
    assert!(insert_request(
        &c,
        &NewTrustRequest { kind: "nope", subject: "x", body: &body, summary: "s", reason: None, proposed_by: "o", proposer_workspace: None, now: 1, ttl: 5 }
    )
    .is_err());
    let p = mk("owner", None).unwrap();
    let re = rewrite_pending(&c, &p.id, &json!({"nodes": ["a", "b"]}), "now with nodes").unwrap();
    assert_ne!(re.digest, p.digest);
    decide(&c, &p.id, "denied", "owner", None, Some("no"), 110).unwrap();
    assert_eq!(rewrite_pending(&c, &p.id, &json!({}), "x").unwrap_err(), codes::NOT_PENDING);
}

fn owner_seat(id: &str, client: &str, nodes: &[&str], state: &str) -> SeatRow {
    SeatRow {
        id: id.into(),
        side: "owner".into(),
        peer_fp: "f".repeat(64),
        peer_host: format!("{id}.k2.dev"),
        owner_id: "owner.k2.dev".into(),
        owner_key_fp: None,
        client_fp: client.into(),
        client_label: format!("{id}.k2.dev"),
        client_url: format!("https://{id}.k2.dev"),
        lane: "ci-standard".into(),
        nodes: nodes.iter().map(|s| s.to_string()).collect(),
        state: state.into(),
        statement: None,
        roster_seq: None,
        code_binding: None,
        request_id: None,
        batch_id: None,
        note: None,
        created_at: 1,
        updated_at: 1,
    }
}

#[test]
fn lanes_file_holds_every_live_seat_on_the_node() {
    let c = conn();
    set_lane(&c, "ci-standard", &LaneTemplate::ci_standard(), "owner", "local-auth", None, 1).unwrap();
    put_seat(&c, &owner_seat("seat-b", &"b".repeat(64), &["n1", "n2"], "active")).unwrap();
    put_seat(&c, &owner_seat("seat-a", &"a".repeat(64), &["n1", "n3"], "offered")).unwrap();
    put_seat(&c, &owner_seat("seat-c", &"c".repeat(64), &["n1", "n2"], "ended")).unwrap();
    let f = lanes_file_for(&c, "n1").unwrap();
    assert_eq!(f.seats.iter().map(|s| s.seat.as_str()).collect::<Vec<_>>(), vec!["seat-a", "seat-b"], "sorted by fp; ended seats gone");
    assert_eq!(clients_on(&c, "n1"), 2);
    assert_eq!(lanes_file_for(&c, "n3").unwrap().seats.len(), 1);
    assert!(lanes_file_for(&c, "n9").unwrap().seats.is_empty());
    put_seat(&c, &SeatRow { lane: "gone".into(), ..owner_seat("seat-d", &"d".repeat(64), &["n4", "n5"], "active") }).unwrap();
    assert!(lanes_file_for(&c, "n4").unwrap_err().contains("isn't approved"));
}

/// The client's side: pin, receive, replay, revoke, the seat code, V25.
#[test]
fn client_receives_rosters_and_checks_lane_seals() {
    let c = conn();
    let o = ApprovalSigner::from_seed(&[7u8; 32]).unwrap();
    let a = SigningKey::generate().unwrap();
    let b = SigningKey::generate().unwrap();
    let node = add_node(&c, "node-a", "n1", &a);
    let sa = statement(&a, Taint::Clean, 0, None).sign(&a).unwrap();
    let sb = statement(&b, Taint::Clean, 0, None).sign(&b).unwrap();
    let mut entries = vec![entry_row(&a, "n1", &sa, Some(TaintBasis::SelfClean)).entry, entry_row(&b, "n2", &sb, Some(TaintBasis::SelfClean)).entry];
    entries.sort_by(|x, y| x.node_fp.cmp(&y.node_fp));
    // Fresh (M5: a client trusts a roster for lane secrets for three days).
    let t0 = crate::compute::now();
    let roster = |seq: u64, entries: Vec<RosterEntry>, prev: Option<String>| NodeRoster {
        v: 1,
        owner_id: "owner.k2.dev".into(),
        owner_server_fp: "e".repeat(64),
        roster_key: o.public().encode(),
        seq,
        issued_at: t0 + seq as i64,
        prev_digest: prev,
        entries,
    };
    let s1 = roster(2, entries.clone(), Some(format!("sha256:{}", "0".repeat(64)))).sign(&o).unwrap();
    // Nobody pinned the key: refused.
    assert_eq!(receive_roster(&c, &s1, "node", 1).unwrap_err().0, codes::OWNER_NOT_TRUSTED);
    pin_owner(&c, &o.public(), "owner.k2.dev", &"9".repeat(64), "owner", "local-auth", false, 1).unwrap();
    let fx = receive_roster(&c, &s1, "node", 2).unwrap();
    assert!(fx.stored && fx.seq == 2 && fx.revoked.is_empty());
    assert!(!receive_roster(&c, &s1, "peer", 3).unwrap().stored, "the same seq twice: no-op");
    // A lower seq than the high-water mark: refused (V13).
    let s0 = roster(1, entries.clone(), None).sign(&o).unwrap();
    assert_eq!(receive_roster(&c, &s0, "node", 4).unwrap_err().0, codes::ROSTER_REPLAYED);
    // Another key under the same owner name: refused.
    let evil = ApprovalSigner::from_seed(&[8u8; 32]).unwrap();
    let forged = NodeRoster { roster_key: evil.public().encode(), ..roster(9, entries.clone(), None) }.sign(&evil).unwrap();
    assert_eq!(receive_roster(&c, &forged, "node", 5).unwrap_err().0, codes::OWNER_NOT_TRUSTED);

    // A client seat on n1 + n2, with its enrol code.
    let mut seat = SeatRow {
        side: "client".into(),
        owner_key_fp: Some(o.public().fingerprint()),
        code_binding: Some("bind-1".into()),
        created_at: 10,
        ..owner_seat("seat-x", &"c".repeat(64), &["n1", "n2"], "active")
    };
    put_seat(&c, &seat).unwrap();
    assert_eq!(seat_code_match(&c, "bind-1", &a.fingerprint(), 20).unwrap().1, "n1");
    assert!(seat_code_match(&c, "bind-2", &a.fingerprint(), 20).is_none(), "another binding");
    assert!(seat_code_match(&c, "bind-1", &SigningKey::generate().unwrap().fingerprint(), 20).is_none(), "a key not on the roster");
    assert!(seat_code_match(&c, "bind-1", &a.fingerprint(), 10 + SEAT_CODE_TTL_SECS).is_none(), "expired");

    // V25, the gate's inputs from these tables (lane_secrets::TrustSeats):
    // not a seat node → no seat (today's rules).
    use crate::compute::lane_secrets::{self as ls, NodeStanding};
    assert!(ls::seats_with_node_in(&c, "node-a").is_empty());
    set_node_seat(&c, "node-a", Some("seat-x")).unwrap();
    let v = ls::seats_with_node_in(&c, "node-a").pop().expect("the node's seat");
    assert_eq!((v.seat_id.as_str(), v.lane.as_str(), v.active, v.node_ids.clone()), ("seat-x", "ci-standard", true, vec!["node-a".to_string()]));
    assert_eq!(v.owner_key_fp, o.public().fingerprint());
    assert!(!v.lane_secrets && v.isolation.is_empty(), "no lane template from the owner yet: no secrets");
    assert!(set_seat_lane(&c, "client", "seat-x", &LaneTemplate::ci_standard()).unwrap());
    assert!(!set_seat_lane(&c, "client", "seat-nope", &LaneTemplate::ci_standard()).unwrap());
    let v = ls::seat_view(&c, "seat-x").unwrap();
    assert!(v.lane_secrets && v.isolation == LaneTemplate::ci_standard().isolation);
    assert_eq!(ls::seat_named_in(&c, "owner.k2.dev", "ci-standard").map(|s| s.seat_id), Some("seat-x".into()));
    assert_eq!(ls::seat_named_in(&c, "owner.k2.dev", "other"), None);
    assert_eq!(ls::node_standing_in(&c, &v, &node), NodeStanding::Clean);
    assert!(ls::consent_for(&c, &v).is_none(), "no consent");
    assert!(give_consent(&c, &o.public().fingerprint(), "owner", 6).unwrap());
    assert_eq!(ls::consent_for(&c, &v).unwrap().owner_key_fp, o.public().fingerprint());
    assert!(!give_consent(&c, &"f".repeat(64), "owner", 6).unwrap(), "only a pinned key");
    // A seat update keeps the owner's lane template (an upsert).
    seat.state = "suspended".into();
    put_seat(&c, &seat).unwrap();
    let v2 = ls::seat_view(&c, "seat-x").unwrap();
    assert!(!v2.active && v2.lane_secrets);
    seat.state = "active".into();
    put_seat(&c, &seat).unwrap();
    // A seat that doesn't list the node's roster name: not this seat's key.
    let other_seat = SeatRow { nodes: vec!["n2".into(), "n3".into()], ..seat.clone() };
    put_seat(&c, &other_seat).unwrap();
    assert_eq!(ls::node_standing_in(&c, &ls::seat_view(&c, "seat-x").unwrap(), &node), NodeStanding::Unlisted);
    put_seat(&c, &seat).unwrap();

    // The owner revokes n1's key as compromised: recorded, seals refused,
    // every receipt from it void; the seat code no longer admits it.
    let mut revoked = entries.clone();
    for e in revoked.iter_mut().filter(|e| e.node_fp == a.fingerprint()) {
        e.status = KeyStatus::Revoked;
        e.revoked_reason = Some(RevokeReason::Compromised);
    }
    // The chain is checked on arrival: seq 3 must name seq 2's digest, and
    // seq 2 can't come again with other contents.
    let unchained = roster(3, revoked.clone(), Some(format!("sha256:{}", "1".repeat(64)))).sign(&o).unwrap();
    assert_eq!(receive_roster(&c, &unchained, "peer", 7).unwrap_err().0, codes::ROSTER_FORKED);
    let fork = roster(2, revoked.clone(), None).sign(&o).unwrap();
    assert_eq!(receive_roster(&c, &fork, "peer", 7).unwrap_err().0, codes::ROSTER_FORKED);
    // An owner name the pin doesn't have: refused (M2).
    let renamed = NodeRoster { owner_id: "rosson.k2.dev".into(), ..roster(3, revoked.clone(), Some(s1.digest())) }.sign(&o).unwrap();
    assert_eq!(receive_roster(&c, &renamed, "peer", 7).unwrap_err().0, codes::OWNER_NOT_TRUSTED);
    let s3 = roster(3, revoked, Some(s1.digest())).sign(&o).unwrap();
    let fx = receive_roster(&c, &s3, "peer", 7).unwrap();
    assert_eq!(fx.revoked, vec![a.fingerprint()]);
    assert_eq!(key_revoked(&c, &a.fingerprint()), Some(RevokeReason::Compromised));
    assert_eq!(ls::node_standing_in(&c, &ls::seat_view(&c, "seat-x").unwrap(), &node), NodeStanding::Revoked);
    assert!(receipt_void(&c, &a.fingerprint(), Some(1)).unwrap().contains("compromised"), "past receipts too");
    assert!(receipt_void(&c, &b.fingerprint(), Some(1)).is_none());
    assert!(seat_code_match(&c, "bind-1", &a.fingerprint(), 20).is_none());
    assert!(withdraw_consent(&c, &o.public().fingerprint()).unwrap());
    assert!(!owner_trust(&c, &o.public().fingerprint()).unwrap().secrets_consent);
    assert!(ls::consent_for(&c, &v).is_none(), "withdrawn at once");
}

#[test]
fn retired_keys_void_only_later_receipts() {
    let c = conn();
    record_revocation(&c, &"a".repeat(64), &"k".repeat(64), RevokeReason::KeyLost, 4, 500);
    assert!(receipt_void(&c, &"a".repeat(64), Some(400)).is_none(), "started before the revocation");
    assert!(receipt_void(&c, &"a".repeat(64), Some(600)).is_some());
    assert!(receipt_void(&c, &"a".repeat(64), None).is_some(), "unknown start: void");
}

#[test]
fn rotation_moves_the_row_in_place() {
    let c = conn();
    let old = SigningKey::generate().unwrap();
    let new = SigningKey::generate().unwrap();
    let node = add_node(&c, "node-r", "nr", &old);
    let rot = RotationStatement {
        v: 1,
        old_fp: old.fingerprint(),
        new_fp: new.fingerprint(),
        new_spki: b64(new.spki_der()),
        new_epoch: 1,
        at: 1_700_000_200,
        reason: RotationReason::Wall,
    };
    let st = NodeStatement { key_born_at: 1_700_000_200, issued_at: 1_700_000_201, ..statement(&new, Taint::Clean, 1, Some(old.fingerprint())) };
    let proof = KeyRotation { rotation: rot.sign(&old).unwrap(), statement: st.sign(&new).unwrap() };
    // Signed by another key than the pinned one: refused.
    let other = SigningKey::generate().unwrap();
    let forged = KeyRotation { rotation: RotationStatement { old_fp: other.fingerprint(), ..rot.clone() }.sign(&other).unwrap(), ..proof.clone() };
    assert_eq!(apply_rotation(&c, &node, &forged, 9).unwrap_err().0, codes::ROTATION_REFUSED);
    // A statement that doesn't continue the rotation (wrong epoch): refused.
    let st2 = NodeStatement { key_epoch: 2, ..st.clone() };
    let bad = KeyRotation { statement: st2.sign(&new).unwrap(), ..proof.clone() };
    assert!(apply_rotation(&c, &node, &bad, 9).is_err());
    let moved = apply_rotation(&c, &node, &proof, 10).unwrap();
    assert_eq!((moved.id.as_str(), moved.fingerprint.clone()), ("node-r", new.fingerprint()));
    assert_eq!(node_key_facts(&c, "node-r").0, 1);
    assert_eq!(node_key_facts(&c, "node-r").1, Some(old.fingerprint()));
    assert_eq!(statement_of(&c, "node-r").unwrap().1.node_fp, new.fingerprint());
    // The same proof again: the row no longer pins the old key.
    assert!(apply_rotation(&c, &moved, &proof, 11).is_err());
    // A compromised old key never vouches for a new one.
    let k3 = SigningKey::generate().unwrap();
    let n3 = add_node(&c, "node-s", "ns", &k3);
    record_revocation(&c, &k3.fingerprint(), &"k".repeat(64), RevokeReason::Compromised, 1, 5);
    let k4 = SigningKey::generate().unwrap();
    let rot3 = RotationStatement { old_fp: k3.fingerprint(), new_fp: k4.fingerprint(), new_spki: b64(k4.spki_der()), ..rot };
    let st3 = NodeStatement { key_born_at: 1_700_000_200, issued_at: 1_700_000_201, ..statement(&k4, Taint::Clean, 1, Some(k3.fingerprint())) };
    let p3 = KeyRotation { rotation: rot3.sign(&k3).unwrap(), statement: st3.sign(&k4).unwrap() };
    assert_eq!(apply_rotation(&c, &n3, &p3, 12).unwrap_err().0, codes::KEY_REVOKED);
    // H2: any revocation stops a hand-over (a lost key too), and a key
    // re-keyed from one later revoked as compromised counts as revoked.
    let k5 = SigningKey::generate().unwrap();
    let n5 = add_node(&c, "node-t", "nt", &k5);
    record_revocation(&c, &k5.fingerprint(), &"k".repeat(64), RevokeReason::KeyLost, 1, 5);
    let rot5 = RotationStatement { old_fp: k5.fingerprint(), new_fp: k4.fingerprint(), new_spki: b64(k4.spki_der()), ..rot3 };
    let st5 = NodeStatement { key_born_at: 1_700_000_200, issued_at: 1_700_000_201, ..statement(&k4, Taint::Clean, 1, Some(k5.fingerprint())) };
    let p5 = KeyRotation { rotation: rot5.sign(&k5).unwrap(), statement: st5.sign(&k4).unwrap() };
    assert_eq!(check_rotation(&c, &n5, &p5).unwrap_err().0, codes::KEY_REVOKED, "a lost key hands over to nothing");
    assert_eq!(rotation_descendants(&c, &old.fingerprint()), vec![new.fingerprint()], "the applied re-key is recorded");
    assert_eq!(key_revoked(&c, &new.fingerprint()), None);
    record_revocation(&c, &old.fingerprint(), &"k".repeat(64), RevokeReason::Compromised, 2, 20);
    assert_eq!(key_revoked(&c, &new.fingerprint()), Some(RevokeReason::Compromised), "the chain follows the compromise");
    assert!(receipt_void(&c, &new.fingerprint(), Some(1)).unwrap().contains("compromised"));
}

#[test]
fn rotated_entries_retire_the_old_key_and_keep_the_facts() {
    let old = SigningKey::generate().unwrap();
    let new = SigningKey::generate().unwrap();
    let s_old = statement(&old, Taint::Unknown, 0, None).sign(&old).unwrap();
    let mut row = entry_row(&old, "n1", &s_old, Some(TaintBasis::OwnerAttested));
    row.entry.attestation_digest = Some(format!("sha256:{}", "a".repeat(64)));
    let st = statement(&new, Taint::Clean, 1, Some(old.fingerprint()));
    let (retired, fresh) = rotated_entries(&row, &st, &format!("sha256:{}", "b".repeat(64)), 900).unwrap();
    assert_eq!((retired.entry.status, retired.entry.valid_to), (KeyStatus::Retired, Some(900)));
    retired.entry.check().unwrap();
    assert_eq!((fresh.entry.status, fresh.entry.taint_basis, fresh.entry.key_epoch), (KeyStatus::Active, Some(TaintBasis::Rekeyed), 1));
    assert_eq!((fresh.entry.name.as_str(), fresh.entry.host.as_str()), ("n1", "host-n1"));
    assert_eq!(fresh.entry.attestation_digest, None);
    fresh.entry.check().unwrap();
}

#[test]
fn signers_and_statements() {
    let c = conn();
    add_signer(&c, &"a".repeat(64), "spki", "z3mbpZ", "owner", "local-auth", 5).unwrap();
    assert!(add_signer(&c, "short", "spki", "x", "owner", "local-auth", 5).is_err());
    assert_eq!(signers(&c, false).len(), 1);
    assert!(remove_signer(&c, &"a".repeat(64), 6).unwrap());
    assert!(!remove_signer(&c, &"a".repeat(64), 7).unwrap());
    assert!(signers(&c, false).is_empty());
    assert_eq!(signers(&c, true).len(), 1);

    let k = SigningKey::generate().unwrap();
    add_node(&c, "node-s", "ns", &k);
    let s = statement(&k, Taint::Unknown, 0, None).sign(&k).unwrap();
    save_statement(&c, "node-s", &s, 1).unwrap();
    let (back, st) = statement_of(&c, "node-s").unwrap();
    assert_eq!((back, st.taint), (s, Taint::Unknown));

    let o = ApprovalSigner::from_seed(&[2u8; 32]).unwrap();
    let att = attestation_for("owner.k2.dev", &o.public(), &st, &statement_of(&c, "node-s").unwrap().0.digest(), "install log", "owner", 9);
    att.sign(&o).unwrap();
    let seat = owner_seat("seat-z", &"c".repeat(64), &["n2", "n1", "n1"], "active");
    let ss = seat_statement_for(&seat, &o.public(), 3, SeatState::Active, 9);
    assert_eq!(ss.nodes, vec!["n1".to_string(), "n2".to_string()], "sorted, unique");
    ss.sign(&o).unwrap();
}

/// M4 + M5 (11-07 review), the client: the lane gate's standing reads the
/// node's own latest statement (a key that says it ran unwalled is
/// tainted, and stays so after it says "clean" again), and never trusts an
/// owner roster older than three days (a missed revocation).
#[test]
fn a_stale_roster_or_a_tainted_statement_stops_lane_secrets() {
    use crate::compute::lane_secrets::{self as ls, NodeStanding};
    let c = conn();
    let o = ApprovalSigner::from_seed(&[11u8; 32]).unwrap();
    let a = SigningKey::generate().unwrap();
    let node = add_node(&c, "node-m", "m1", &a);
    let sa = statement(&a, Taint::Clean, 0, None).sign(&a).unwrap();
    let entry = entry_row(&a, "m1", &sa, Some(TaintBasis::SelfClean)).entry;
    pin_owner(&c, &o.public(), "owner.k2.dev", &"9".repeat(64), "owner", "local-auth", false, 1).unwrap();
    let roster = |seq: u64, at: i64, prev: Option<String>| {
        NodeRoster {
            v: 1,
            owner_id: "owner.k2.dev".into(),
            owner_server_fp: "e".repeat(64),
            roster_key: o.public().encode(),
            seq,
            issued_at: at,
            prev_digest: prev,
            entries: vec![entry.clone()],
        }
        .sign(&o)
        .unwrap()
    };
    let seat = SeatRow { side: "client".into(), owner_key_fp: Some(o.public().fingerprint()), ..owner_seat("seat-m", &"c".repeat(64), &["m1"], "active") };
    put_seat(&c, &seat).unwrap();
    set_node_seat(&c, "node-m", Some("seat-m")).unwrap();
    let v = ls::seat_view(&c, "seat-m").unwrap();
    // A roster four days old: not trusted for lane secrets.
    let r1 = roster(1, crate::compute::now() - ls::ROSTER_MAX_AGE_SECS - 3600, None);
    receive_roster(&c, &r1, "peer", 1).unwrap();
    assert_eq!(ls::node_standing_in(&c, &v, &node), NodeStanding::Unlisted, "a stale roster vouches for nothing");
    let r2 = roster(2, crate::compute::now(), Some(r1.digest()));
    receive_roster(&c, &r2, "peer", 2).unwrap();
    assert_eq!(ls::node_standing_in(&c, &v, &node), NodeStanding::Clean, "a fresh one does");
    // The node says it ran a job without the wall: tainted at once, the
    // roster notwithstanding, and remembered.
    save_statement(&c, "node-m", &statement(&a, Taint::Tainted, 0, None).sign(&a).unwrap(), 3).unwrap();
    assert_eq!(ls::node_standing_in(&c, &v, &node), NodeStanding::Tainted);
    save_statement(&c, "node-m", &sa, 4).unwrap();
    assert_eq!(ls::node_standing_in(&c, &v, &node), NodeStanding::Tainted, "saying clean again doesn't clear it");
    assert!(taint_remembered(&c, &a.fingerprint()).is_some());
    assert_eq!(taint_basis_checked(&c, &statement(&a, Taint::Clean, 0, None), false, false).unwrap_err().0, codes::NODE_KEY_TAINTED);
}

/// M4, the owner: a listed key whose node later says it ran unwalled loses
/// its vouch in the working entries (and the memory keeps it out).
#[test]
fn a_listed_key_that_goes_tainted_loses_its_vouch() {
    let c = conn();
    let a = SigningKey::generate().unwrap();
    add_node(&c, "node-o", "o1", &a);
    let sa = statement(&a, Taint::Clean, 0, None).sign(&a).unwrap();
    save_statement(&c, "node-o", &sa, 1).unwrap();
    let mut row = entry_row(&a, "o1", &sa, Some(TaintBasis::SelfClean));
    row.node_id = Some("node-o".into());
    put_entry(&c, &row, 2).unwrap();
    assert!(narrow_tainted(&c, 3).is_empty(), "a clean key keeps its vouch");
    save_statement(&c, "node-o", &statement(&a, Taint::Tainted, 0, None).sign(&a).unwrap(), 4).unwrap();
    assert_eq!(narrow_tainted(&c, 5), vec!["o1".to_string()]);
    let e = entry_by_fp(&c, &a.fingerprint()).unwrap();
    assert_eq!((e.entry.status, e.entry.taint_basis), (KeyStatus::Active, None));
    save_statement(&c, "node-o", &sa, 6).unwrap();
    assert!(taint_remembered(&c, &a.fingerprint()).is_some(), "the memory outlives a clean statement");
}

/// 11-07 review (Low): a store error reading the revocations (or the taint
/// memory) reads as revoked (tainted), never as a clean "not revoked".
#[test]
fn a_store_error_reads_as_revoked_not_clean() {
    let c = conn();
    let fp = "a".repeat(64);
    assert_eq!(key_revoked(&c, &fp), None, "control: nothing recorded");
    c.execute_batch("DROP TABLE compute_key_revocations; DROP TABLE compute_tainted_keys;").unwrap();
    assert!(key_revoked(&c, &fp).is_some(), "unreadable: revoked");
    assert!(receipt_void(&c, &fp, Some(1)).is_some(), "unreadable: void");
    assert!(taint_remembered(&c, &fp).is_some(), "unreadable: tainted");
}

/// M2 (11-07 review): a roster key is pinned for one owner name and one
/// paired server, for good; one owner name never gets a second key.
#[test]
fn a_pin_is_never_moved_or_doubled() {
    let c = conn();
    let a = ApprovalSigner::from_seed(&[21u8; 32]).unwrap().public();
    let b = ApprovalSigner::from_seed(&[22u8; 32]).unwrap().public();
    pin_owner(&c, &a, "o.k2.dev", &"1".repeat(64), "owner", "local-auth", false, 1).unwrap();
    pin_owner(&c, &a, "o.k2.dev", &"1".repeat(64), "owner", "local-auth", false, 2).expect("the same pin again");
    assert!(pin_owner(&c, &a, "rosson.k2.dev", &"1".repeat(64), "owner", "local-auth", false, 3).is_err(), "no rename");
    assert!(pin_owner(&c, &a, "o.k2.dev", &"2".repeat(64), "owner", "local-auth", false, 3).is_err(), "no move to another server");
    assert!(pin_owner(&c, &b, "o.k2.dev", &"3".repeat(64), "owner", "local-auth", false, 3).is_err(), "no second key under one name");
    assert!(pin_owner(&c, &b, "p.k2.dev", &"1".repeat(64), "owner", "local-auth", false, 3).is_err(), "no second key for one server");
    assert_eq!(owner_trust(&c, &a.fingerprint()).unwrap().owner_id, "o.k2.dev");
    assert!(owner_trust(&c, &b.fingerprint()).is_none());
}
