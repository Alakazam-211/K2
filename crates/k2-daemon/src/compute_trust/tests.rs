//! Node rosters, lanes, seats and signer pairing (0.46.10 C1/C3a). Two
//! servers in one test: the node owner's and a client's scoped DBs, run in
//! turn on this thread (`with_scoped_handle`), joined by an in-process
//! federation seam that calls the other side's `receive` (the envelope
//! itself is the ingress module's, tested there). Fakes only: a temp vault
//! with a fake key store (never the keychain), a generated controller key,
//! fake node sessions. Every sealed node command is opened with the node
//! side's own verifier.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use k2_core::compute::proto::admin as wire;
use k2_core::compute::proto::ci::{KeyStatus, KeyStore, SeatState, Taint, TaintBasis, Wall};
use k2_core::compute::proto::crypto::{b64, SigningKey};
use k2_core::compute::proto::frames::{self as pframes, Frame, KeyRotation};
use k2_core::compute::proto::lanes::{self as plane, LanesFile, NodeRosterDoc};
use k2_core::compute::proto::node_roster::{NodeRoster, OwnerAttestation, SeatStatement};
use k2_core::compute::proto::statement::{KeyExposure, NodeStatement, RotationStatement};
use k2_core::connect_users::Role;
use k2_core::federation::{FederationPeer, PeerTrust};
use parking_lot::ReentrantMutex;
use rusqlite::Connection;
use serde_json::{json, Value};

use super::test_seams;
use super::*;
use crate::compute_admin::test_seams as admin_seams;

const OWNER: &str = "owner.k2.dev";
const CLIENT: &str = "client.k2.dev";

type Db = Arc<ReentrantMutex<Connection>>;

/// Run `f` as one server: its DB and its host.
fn on<T>(db: &Db, host: &str, f: impl FnOnce() -> T) -> T {
    let prev = test_seams::owner_id_now();
    test_seams::set_owner_id(Some(host));
    let out = k2_core::db::with_scoped_handle(db.clone(), f);
    test_seams::set_owner_id(prev.as_deref());
    out
}

fn call(path: &str, who: &Who, body: Value) -> Result<Value, (u16, String, String)> {
    handle(path, who, body.to_string().as_bytes(), &HashMap::new()).map_err(|e| (e.status, e.code.to_string(), e.hint))
}

fn get(path: &str, who: &Who) -> Value {
    handle(path, who, b"", &HashMap::new()).unwrap_or_else(|e| panic!("{path}: {} {}", e.code, e.hint))
}

fn vault() -> Arc<k2_vault::LocalVault> {
    admin_seams::vault_handle()
}

fn token(action: &str, bound: &str, actor: &str) -> String {
    vault().confirm.mint(action, Some(bound), actor, "local-auth")
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: "s1".into() }
}

fn admin_login() -> Who {
    Who::Login { username: "ana".into(), role: Role::Admin }
}

/// A workspace with "operate compute nodes" on (the current DB).
fn operating_ws(tag: &str) -> String {
    let ws = format!("ws-{tag}-{}", crypto::random_id().unwrap());
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![ws, format!("it-{tag}"), format!("/tmp/k2-trust-test/{ws}")]).unwrap();
    k2_core::compute::admin::set_agents_can_operate_compute(&conn, &ws, true).unwrap();
    ws
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

/// An owned, active, protocol-9 node with a statement (current DB), and its
/// fake session with `features`.
fn owned_node(name: &str, taint: Taint, features: &[&str]) -> (store::NodeRow, SigningKey) {
    let k = SigningKey::generate().unwrap();
    let id = format!("node-{name}-{}", crypto::random_id().unwrap());
    let h = k2_core::db::shared();
    let conn = h.lock();
    let labels = BTreeMap::from([("os".to_string(), "linux".to_string()), ("arch".to_string(), "x86_64".to_string())]);
    store::insert_pending_node(&conn, &id, name, &k.fingerprint(), &k.spki_pem(), &labels, "owner", "123456", 5).unwrap();
    store::confirm_node(&conn, &id, "owner", 6).unwrap();
    let st = statement(&k, taint, 0, None).sign(&k).unwrap();
    trust::save_statement(&conn, &id, &st, 7).unwrap();
    k2_core::compute::admin::set_reported_admin(&conn, &id, Some("full"), None);
    drop(conn);
    admin_seams::register(&id, true, features);
    let row = store::node_by_id(&h.lock(), &id).unwrap();
    (row, k)
}

const P9: &[&str] = &[wire::FEATURE_ADMIN, ci::FEATURE_NODE_ROSTER, ci::FEATURE_LANES, pframes::FEATURE_JOB_WALL, pshare::FEATURE_TENANT_WALL, ci::FEATURE_NODE_STATEMENT];

/// The node side's own checks, as root's executor runs them.
fn node_open(node: &store::NodeRow, c: &wire::SignedCommand) -> wire::Opened {
    let ctl = SigningKey::from_pkcs8_pem(&admin_seams::key_pem()).unwrap();
    let raw = c.to_json().unwrap();
    let o = wire::open(raw.as_bytes(), ctl.spki_der()).unwrap();
    wire::check_target_and_time(&o.cmd, &node.fingerprint, &ctl.fingerprint(), compute::now()).unwrap();
    o
}

fn admin_cmds(node_id: &str) -> Vec<wire::SignedCommand> {
    admin_seams::take_sent(node_id)
        .into_iter()
        .filter_map(|f| match f {
            Frame::Admin(a) => Some(a.cmd),
            _ => None,
        })
        .collect()
}

fn approve(id: &str) -> Value {
    call(TRUST_APPROVE, &Who::Owner, json!({"request": id, "confirmToken": token(trust::CONFIRM_TRUST, id, "owner")})).unwrap_or_else(|e| panic!("approve {id}: {e:?}"))
}

fn roster_set(node: &str, extra: Value) -> String {
    let mut b = json!({"op": "set", "node": node, "host": format!("host-{node}"), "provider": "vin", "adminReach": ["fleet-ssh"]});
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    let r = call(ROSTER_PROPOSE, &Who::Owner, b).unwrap();
    assert_eq!(r["held"], true);
    r["request"]["id"].as_str().unwrap().to_string()
}

#[test]
fn routes_agree() {
    for p in POST_ROUTES {
        assert!(ROUTES.contains(p), "{p}");
    }
    for p in ROUTES {
        assert!(crate::compute_routes::is_route(p), "{p} not on the compute router");
        assert!(crate::routes::route_policy::lookup(p).is_some(), "{p} has no route policy row");
        assert_eq!(crate::compute_routes::is_post_route(p), POST_ROUTES.contains(p));
    }
    assert!(crate::routes::route_policy::lookup(FEDERATION_ROUTE).is_some());
}

/// C1: a roster change is filed by anyone who operates compute, decided by
/// a person with a fresh confirm; the roster is signed with the vault's
/// node-roster key, chained, and handed to every node that relays it.
#[test]
fn roster_changes_need_a_person_and_reach_the_nodes() {
    let _db = k2_core::db::scoped_for_test();
    test_seams::set_owner_id(Some(OWNER));
    let ws = operating_ws("o");
    let (n1, _) = owned_node("n1", Taint::Clean, P9);
    let (n2, k2) = owned_node("n2", Taint::Unknown, &[wire::FEATURE_ADMIN]);
    let (n3, _) = owned_node("n3", Taint::Tainted, P9);

    // Taint rules at filing: tainted never; unknown only with an attestation.
    assert_eq!(call(ROSTER_PROPOSE, &agent(&ws), json!({"op": "set", "node": "n3"})).unwrap_err().1, ci::codes::NODE_KEY_TAINTED);
    assert_eq!(call(ROSTER_PROPOSE, &agent(&ws), json!({"op": "set", "node": "n2"})).unwrap_err().1, ci::codes::NODE_KEY_TAINTED);
    // An agent with the operate switch files; it can never approve.
    let r = call(ROSTER_PROPOSE, &agent(&ws), json!({"op": "set", "node": "n1", "host": "host-a"})).unwrap();
    let id1 = r["request"]["id"].as_str().unwrap().to_string();
    let e = call(TRUST_APPROVE, &agent(&ws), json!({"request": id1, "confirmToken": token(trust::CONFIRM_TRUST, &id1, "owner")})).unwrap_err();
    assert_eq!((e.0, e.1.as_str()), (403, tcodes::PERSON_ONLY));
    // A person without a confirm, or with one for another request: refused.
    assert_eq!(call(TRUST_APPROVE, &Who::Owner, json!({"request": id1})).unwrap_err().1, "vault_confirm_required");
    let wrong = token(trust::CONFIRM_TRUST, "ctr_other", "owner");
    assert_eq!(call(TRUST_APPROVE, &Who::Owner, json!({"request": id1, "confirmToken": wrong})).unwrap_err().1, "vault_confirm_required");
    // An agent can't mint the roster key either.
    assert_eq!(call(ROSTER_KEY, &agent(&ws), json!({})).unwrap_err().1, tcodes::PERSON_ONLY);

    let out = approve(&id1);
    assert_eq!(out["result"]["roster"]["seq"], 1);
    let key = roster_key().unwrap();
    let signed = {
        let h = k2_core::db::shared();
        let c = h.lock();
        trust::latest_own_roster(&c).unwrap()
    };
    let r1 = NodeRoster::open(&signed.signed, &key).unwrap();
    assert_eq!((r1.owner_id.as_str(), r1.entries.len(), r1.prev_digest.clone()), (OWNER, 1, None));
    assert_eq!((r1.entries[0].host.as_str(), r1.entries[0].taint_basis), ("host-a", Some(TaintBasis::SelfClean)));
    // Handed to n1 (it relays), as an operate command the node verifies.
    let cmds = admin_cmds(&n1.id);
    assert_eq!(cmds.len(), 1);
    let o = node_open(&n1, &cmds[0]);
    assert_eq!((o.cmd.verb.as_str(), o.cmd.class), (plane::VERB_NODE_ROSTER_SET, wire::Class::Operate));
    let doc: NodeRosterDoc = serde_json::from_value(o.cmd.args.clone()).unwrap();
    assert_eq!(doc.open(Some(&key)).unwrap().roster.seq, 1);
    // A replay of the same approval finds nothing to decide.
    let again = call(TRUST_APPROVE, &Who::Owner, json!({"request": id1, "confirmToken": token(trust::CONFIRM_TRUST, &id1, "owner")})).unwrap_err();
    assert_eq!(again.1, tcodes::NOT_PENDING);

    // V12: n2 says `taint: unknown`: the owner attests instead of a re-key.
    // An Admin login approves (owner or Admin, in person). n2's k2-node
    // doesn't relay rosters: the delivery says so, nothing is sent.
    let id2 = roster_set("n2", json!({"attest": "installed with the job wall; no unwalled job in the ledger"}));
    let ana = admin_login();
    let out = call(TRUST_APPROVE, &ana, json!({"request": id2, "confirmToken": token(trust::CONFIRM_TRUST, &id2, "connect:ana")})).unwrap();
    assert_eq!(out["result"]["roster"]["seq"], 2);
    let r2 = {
        let h = k2_core::db::shared();
        let c = h.lock();
        trust::latest_own_roster(&c).unwrap()
    };
    assert_eq!(r2.roster.prev_digest.as_deref(), Some(signed.signed.digest().as_str()), "chained (V13)");
    let e2 = r2.roster.entry(&k2.fingerprint()).unwrap();
    assert_eq!(e2.taint_basis, Some(TaintBasis::OwnerAttested));
    let att = {
        let h = k2_core::db::shared();
        let c = h.lock();
        trust::entry_by_fp(&c, &k2.fingerprint()).unwrap().attestation.unwrap()
    };
    let a = OwnerAttestation::open(&att, &key).unwrap();
    assert_eq!((a.approved_by.as_str(), e2.attestation_digest.as_deref()), ("login:ana", Some(att.digest().as_str())));
    let skipped = out["result"]["delivered"]["nodes"].as_array().unwrap().iter().find(|x| x["node"] == "n2").unwrap().clone();
    assert_eq!(skipped["sent"], false, "a node that doesn't relay rosters is skipped, never sent an unknown verb");
    assert!(admin_cmds(&n2.id).is_empty());

    // Publish: an unchanged roster re-signs only on request (or when old);
    // a pending change blocks the re-sign.
    let p = call(ROSTER_PUBLISH, &agent(&ws), json!({"resign": true})).unwrap();
    assert_eq!(p["roster"]["seq"], 3);
    let _pending = roster_set("n1", json!({"host": "host-b"}));
    let p = call(ROSTER_PUBLISH, &agent(&ws), json!({})).unwrap();
    assert_eq!(p["roster"]["seq"], 3, "a plain publish re-delivers the newest");
    let _ = n3;
}

/// C1: revoke and retire, and the roster's freshness refresh in the tick.
#[test]
fn revoke_retire_and_refresh() {
    let _db = k2_core::db::scoped_for_test();
    test_seams::set_owner_id(Some(OWNER));
    let (_n1, k1) = owned_node("n1", Taint::Clean, P9);
    let (_n2, k2) = owned_node("n2", Taint::Clean, P9);
    approve(&roster_set("n1", json!({})));
    approve(&roster_set("n2", json!({})));
    let r = call(ROSTER_PROPOSE, &Who::Owner, json!({"op": "revoke", "node": "n1", "reason": "compromised"})).unwrap();
    assert!(r["request"]["summary"].as_str().unwrap().contains("every receipt"));
    approve(r["request"]["id"].as_str().unwrap());
    let r = call(ROSTER_PROPOSE, &Who::Owner, json!({"op": "retire", "node": "n2"})).unwrap();
    approve(r["request"]["id"].as_str().unwrap());
    let h = k2_core::db::shared();
    let latest = trust::latest_own_roster(&h.lock()).unwrap();
    assert_eq!(latest.roster.entry(&k1.fingerprint()).unwrap().status, KeyStatus::Revoked);
    assert_eq!(latest.roster.entry(&k2.fingerprint()).unwrap().status, KeyStatus::Retired);
    assert!(trust::receipt_void(&h.lock(), &k1.fingerprint(), Some(1)).is_some(), "the owner's own receipts view too");
    // The tick re-signs an unchanged roster once it is two days old.
    let seq = latest.roster.seq;
    h.lock().execute("UPDATE compute_node_rosters SET issued_at = issued_at - ?1", rusqlite::params![trust::ROSTER_REFRESH_SECS + 5]).unwrap();
    tick();
    let next = trust::latest_own_roster(&h.lock()).unwrap();
    assert_eq!(next.roster.seq, seq + 1);
    assert_eq!(next.roster.entries, latest.roster.entries, "nothing a person approved changed");
}

fn peer(label: &str, host: &str) -> FederationPeer {
    let k = SigningKey::generate().unwrap();
    let mut p = FederationPeer::pin(label, host, k.spki_pem()).unwrap();
    p.trust = PeerTrust::Trusted;
    p
}

/// C3a end to end across two servers: lane, roster, the client's seat
/// request (agent files, person approves: pin + consent + seat code), the
/// owner's seat (agent names nodes, the owner approves: signed seat, offer,
/// one batch of node plans), the seat nodes' enrolment on the client, V25,
/// a revocation reaching the client, and the refusals on the way.
#[test]
fn a_seat_end_to_end_between_two_servers() {
    let owner_guard = k2_core::db::scoped_for_test();
    let client_guard = k2_core::db::scoped_for_test();
    let (odb, cdb) = (owner_guard.handle(), client_guard.handle());
    let owner_peer = peer("owner", OWNER);
    let client_peer = peer("client", CLIENT);
    test_seams::add_peer(owner_peer.clone());
    test_seams::add_peer(client_peer.clone());
    // The federation seam: each side's `receive`, as the other peer.
    {
        let (odb2, cdb2, ofp, cfp) = (odb.clone(), cdb.clone(), owner_peer.fingerprint.clone(), client_peer.fingerprint.clone());
        test_seams::set_fed(Some(Box::new(move |to: &str, kind: &str, p: &Value| {
            if to == ofp {
                on(&odb2, OWNER, || receive(&cfp, CLIENT, kind, p))
            } else if to == cfp {
                on(&cdb2, CLIENT, || receive(&ofp, OWNER, kind, p))
            } else {
                Err(no("peer_unreachable", 502, "no such peer"))
            }
        })));
    }

    // Owner: three nodes on the roster, one Mac, the lane approved.
    let (nodes, keys, ows) = on(&odb, OWNER, || {
        let ows = operating_ws("own");
        let mut nodes = Vec::new();
        let mut keys = Vec::new();
        for n in ["n1", "n2", "n3"] {
            let (row, k) = owned_node(n, Taint::Clean, P9);
            approve(&roster_set(n, json!({})));
            nodes.push(row);
            keys.push(k);
        }
        // A node without `lanes` (an older k2-node) is never seated.
        let (_old, _) = owned_node("n4", Taint::Clean, &[wire::FEATURE_ADMIN, ci::FEATURE_NODE_ROSTER, pframes::FEATURE_JOB_WALL, pshare::FEATURE_TENANT_WALL]);
        approve(&roster_set("n4", json!({})));
        // A node whose remote admin isn't `full` would refuse half a seat (S7).
        let (n5, _) = owned_node("n5", Taint::Clean, P9);
        approve(&roster_set("n5", json!({})));
        k2_core::compute::admin::set_reported_admin(&k2_core::db::shared().lock(), &n5.id, Some("policy"), None);
        let r = call(LANES_SET, &agent(&ows), json!({"name": "ci-standard", "from": "ci-standard"})).unwrap();
        approve(r["request"]["id"].as_str().unwrap());
        for n in &nodes {
            admin_cmds(&n.id);
        }
        (nodes, keys, ows)
    });

    // Client: an agent asks; the card names the owner's key; a person approves.
    let cws = on(&cdb, CLIENT, || operating_ws("cli"));
    test_seams::take_sent();
    let req = on(&cdb, CLIENT, || call(SEATS_REQUEST, &agent(&cws), json!({"owner": OWNER, "lane": "ci-standard", "why": "release builds"})).unwrap());
    let key_fp = on(&odb, OWNER, || roster_key().unwrap().fingerprint());
    assert!(req["request"]["summary"].as_str().unwrap().contains(&key_fp[..16]), "the card shows the owner's key to compare");
    let creq = req["request"]["id"].as_str().unwrap().to_string();
    let e = on(&cdb, CLIENT, || call(TRUST_APPROVE, &agent(&cws), json!({"request": creq})).unwrap_err());
    assert_eq!(e.1, tcodes::PERSON_ONLY, "an agent never accepts a seat");
    let out = on(&cdb, CLIENT, || approve(&creq));
    assert_eq!(out["result"]["sent"], true, "{out}");
    let oreq = out["result"]["ownerRequest"].as_str().unwrap().to_string();
    // What crossed: the key lookup, then the request (with the seat code
    // for the owner's servers.add), nothing else.
    let sent: Vec<String> = test_seams::take_sent().into_iter().map(|(_, k, _)| k).collect();
    assert_eq!(sent, vec![KIND_OWNER_KEY.to_string(), KIND_SEAT_REQUEST.to_string()]);
    on(&cdb, CLIENT, || {
        let h = k2_core::db::shared();
        let c = h.lock();
        let t = trust::owner_trust(&c, &key_fp).unwrap();
        assert!(t.secrets_consent && t.owner_id == OWNER);
        assert_eq!(trust::seat(&c, "client", "client-ci-standard").unwrap().state, "requested");
    });

    // Owner: the request is held; its code never shows; nodes are named.
    on(&odb, OWNER, || {
        let list = call(TRUST_REQUESTS, &Who::Owner, json!({})).ok();
        let _ = list;
        let rq = get(TRUST_REQUESTS, &Who::Owner);
        let mine = rq["requests"].as_array().unwrap().iter().find(|r| r["id"] == oreq.as_str()).unwrap().clone();
        assert_eq!(mine["body"]["code"], "[redacted]");
        // Not yet: no nodes named.
        assert_eq!(call(TRUST_APPROVE, &Who::Owner, json!({"request": oreq, "confirmToken": token(trust::CONFIRM_TRUST, &oreq, "owner")})).unwrap_err().1, tcodes::BAD_ARGS);
        assert_eq!(call(SEATS_OFFER, &agent(&ows), json!({"request": oreq, "nodes": ["n1", "n4"]})).unwrap_err().1, tcodes::NODE_LACKS_LANES);
        assert_eq!(call(SEATS_OFFER, &agent(&ows), json!({"request": oreq, "nodes": ["n1", "n5"]})).unwrap_err().1, "remote_admin_off");
        call(SEATS_OFFER, &agent(&ows), json!({"request": oreq, "nodes": ["n1", "n2", "n3"]})).unwrap();
        // A seat seals node plans with the owner approval key: owner only.
        let ana = admin_login();
        assert_eq!(call(TRUST_APPROVE, &ana, json!({"request": oreq, "confirmToken": token(trust::CONFIRM_TRUST, &oreq, "connect:ana")})).unwrap_err().1, tcodes::PERSON_ONLY);
    });
    let seat_out = on(&odb, OWNER, || approve(&oreq));
    let batch = seat_out["result"]["batch"].as_str().unwrap().to_string();
    assert!(seat_out["result"]["plans"].as_array().unwrap().iter().all(|p| p["queued"] == true), "{seat_out}");

    // One approval, one batch: every node got lanes.write + servers.add,
    // privileged, signed with the owner approval key, the client's seat code.
    on(&odb, OWNER, || {
        let approval = wire::ApprovalKey(crate::compute_admin::approval_public_key().unwrap());
        for n in &nodes {
            // The first step went out at once (one step in flight per
            // request); the plan holds both, sealed together.
            let sent: Vec<String> = admin_cmds(&n.id).iter().map(|c| node_open(n, c).cmd.verb).collect();
            assert!(sent.iter().any(|v| v == plane::VERB_LANES_WRITE), "{}: {sent:?}", n.name);
            let stored = {
                let h = k2_core::db::shared();
                let c = h.lock();
                k2_core::compute::admin::list_requests(&c, &k2_core::compute::admin::ReqFilter { node_id: Some(&n.id), state: None, limit: 50, ..Default::default() })
            };
            let plan_req = stored.iter().find(|r| r.reason.as_deref().is_some_and(|x| x.contains(&batch))).expect("the batch's plan for this node");
            assert_eq!((plan_req.state.as_str(), plan_req.decided_by.as_deref()), ("sent", Some("owner")));
            let plans: Vec<wire::Opened> = plan_req.commands.iter().map(|c| node_open(n, c)).collect();
            assert_eq!(plans.iter().map(|o| o.cmd.verb.as_str()).collect::<Vec<_>>(), vec![plane::VERB_LANES_WRITE, pshare::VERB_ADD], "{}", n.name);
            for o in &plans {
                assert_eq!(o.cmd.class, wire::Class::Privileged);
                wire::verify_owner(o, &[approval.clone()], compute::now()).unwrap();
            }
            let file: LanesFile = plane::check_write_args(&plans[0].cmd.args).unwrap();
            assert_eq!(file.seats.len(), 1);
            assert_eq!(file.seats[0].seat, "client-ci-standard");
            let add: pshare::AddArgs = serde_json::from_value(plans[1].cmd.args.clone()).unwrap();
            assert_eq!((add.trust, add.url.as_str()), (pshare::Trust::Team, format!("https://{CLIENT}").as_str()));
            let ev: i64 = {
                let h = k2_core::db::shared();
                let c = h.lock();
                c.query_row("SELECT COUNT(*) FROM compute_events WHERE kind = 'admin_approved' AND detail_json LIKE ?1", rusqlite::params![format!("%{batch}%")], |r| r.get(0)).unwrap()
            };
            assert!(ev >= 1);
        }
    });

    // Client: the offer landed: seat active, statement signed for it, roster stored.
    let (seat_row, code_binding) = on(&cdb, CLIENT, || {
        let h = k2_core::db::shared();
        let c = h.lock();
        let s = trust::seat(&c, "client", "client-ci-standard").unwrap();
        assert_eq!((s.state.as_str(), s.nodes.clone()), ("active", vec!["n1".to_string(), "n2".into(), "n3".into()]));
        let st = SeatStatement::open(s.statement.as_ref().unwrap(), &ApprovalKey::parse(&trust::owner_trust(&c, &key_fp).unwrap().roster_key).unwrap()).unwrap();
        assert_eq!((st.state, st.client_fp.clone()), (SeatState::Active, my_fp().unwrap()));
        assert!(trust::latest_roster(&c, &key_fp).is_some());
        (s.clone(), s.code_binding.clone().unwrap())
    });

    // Client: a seat node enrols with the seat code (V20): confirmed without
    // a SAS because its key is the roster's; a stranger with the code is not.
    on(&cdb, CLIENT, || {
        assert!(seat_enrol(&code_binding, &SigningKey::generate().unwrap().fingerprint()).is_none());
        let (seat, name) = seat_enrol(&code_binding, &keys[0].fingerprint()).unwrap();
        assert_eq!(name, "n1");
        let local = seat_node_local_name(&name, &seat.id);
        let h = k2_core::db::shared();
        let id = "client-node-n1";
        {
            let c = h.lock();
            let labels = BTreeMap::from([("os".to_string(), "linux".to_string())]);
            store::insert_pending_node(&c, id, &local, &keys[0].fingerprint(), &keys[0].spki_pem(), &labels, &format!("seat:{}", seat.id), "000000", 5).unwrap();
        }
        seat_enrolled(id, &seat);
        let row = store::node_by_id(&h.lock(), id).unwrap();
        assert_eq!((row.state.as_str(), row.is_tenant()), ("active", true));
        // V25: consent given, seat active, key active with a basis: sealable.
        let seal = trust::lane_seal_check(&h.lock(), &row).unwrap().unwrap();
        assert_eq!((seal.seat.as_str(), seal.lane.as_str()), ("client-ci-standard", "ci-standard"));
        // Consent withdrawn (an agent may narrow): refused, with the code X uses.
        call(OWNERS_CONSENT, &agent(&cws), json!({"owner": OWNER, "on": false})).unwrap();
        assert_eq!(trust::lane_seal_check(&h.lock(), &row).unwrap_err().0, ci::codes::SECRET_OWNER_NOT_TRUSTED);
        // Turning it back on is a person's, with a confirm.
        assert_eq!(call(OWNERS_CONSENT, &agent(&cws), json!({"owner": OWNER, "on": true})).unwrap_err().1, tcodes::PERSON_ONLY);
        let tok = token(trust::CONFIRM_TRUST, &format!("consent:{key_fp}"), "owner");
        call(OWNERS_CONSENT, &Who::Owner, json!({"owner": OWNER, "on": true, "confirmToken": tok})).unwrap();
        assert!(trust::lane_seal_check(&h.lock(), &row).unwrap().is_some());
    });
    let _ = seat_row;

    // Owner revokes n1 as compromised: the client hears it (federation),
    // and the node relay of an older roster is refused as a replay.
    let old_doc = on(&odb, OWNER, || {
        let h = k2_core::db::shared();
        let old = trust::latest_own_roster(&h.lock()).unwrap();
        let r = call(ROSTER_PROPOSE, &Who::Owner, json!({"op": "revoke", "node": "n1", "reason": "compromised"})).unwrap();
        let out = approve(r["request"]["id"].as_str().unwrap());
        assert!(out["result"]["delivered"]["clients"].as_array().unwrap().iter().any(|c| c["sent"] == true), "{out}");
        NodeRosterDoc { roster: old.signed, seats: vec![] }
    });
    on(&cdb, CLIENT, || {
        let h = k2_core::db::shared();
        assert!(trust::key_revoked(&h.lock(), &keys[0].fingerprint()).is_some());
        let row = store::node_by_id(&h.lock(), "client-node-n1").unwrap();
        assert_eq!(trust::lane_seal_check(&h.lock(), &row).unwrap_err().0, tcodes::KEY_REVOKED);
        assert!(receipt_void(&keys[0].fingerprint(), Some(1)).unwrap().contains("compromised"));
        let e = client_take_roster(None, &old_doc, "node").unwrap_err();
        assert_eq!(e.code, ci::codes::ROSTER_REPLAYED);
    });
    test_seams::set_fed(None);
    drop(client_guard);
    drop(owner_guard);
}

/// The client never takes a roster or seat from a key no person pinned, nor
/// a seat it never asked for.
#[test]
fn client_refuses_unpinned_owners_and_unasked_seats() {
    let _db = k2_core::db::scoped_for_test();
    test_seams::set_owner_id(Some(CLIENT));
    let o = wire::ApprovalSigner::from_seed(&[3u8; 32]).unwrap();
    let r = NodeRoster {
        v: 1,
        owner_id: "evil.k2.dev".into(),
        owner_server_fp: "e".repeat(64),
        roster_key: o.public().encode(),
        seq: 1,
        issued_at: 1,
        prev_digest: None,
        entries: vec![],
    };
    let doc = NodeRosterDoc { roster: r.sign(&o).unwrap(), seats: vec![] };
    assert_eq!(receive("f".repeat(64).as_str(), "evil.k2.dev", KIND_NODE_ROSTER, &serde_json::to_value(&doc).unwrap()).unwrap_err().code, tcodes::OWNER_NOT_TRUSTED);
    // Pinned, but for another peer: refused over federation.
    {
        let h = k2_core::db::shared();
        trust::pin_owner(&h.lock(), &o.public(), "evil.k2.dev", &"a".repeat(64), "owner", "local-auth", false, 1).unwrap();
    }
    assert_eq!(receive("f".repeat(64).as_str(), "x", KIND_NODE_ROSTER, &serde_json::to_value(&doc).unwrap()).unwrap_err().code, tcodes::OWNER_NOT_TRUSTED);
    // From the right peer: stored; a seat for a seat nobody asked for is ignored.
    let seat = SeatStatement {
        v: 1,
        owner_id: "evil.k2.dev".into(),
        roster_key: o.public().encode(),
        seat_id: "surprise".into(),
        client_fp: my_fp().unwrap(),
        client_label: "c".into(),
        lane: "ci-standard".into(),
        nodes: vec!["a".into(), "b".into()],
        roster_seq: 1,
        state: SeatState::Active,
        issued_at: 1,
        pod: None,
        line: None,
    }
    .sign(&o)
    .unwrap();
    let doc = NodeRosterDoc { roster: doc.roster, seats: vec![seat] };
    receive(&"a".repeat(64), "x", KIND_NODE_ROSTER, &serde_json::to_value(&doc).unwrap()).unwrap();
    let h = k2_core::db::shared();
    assert!(trust::seat(&h.lock(), "client", "surprise").is_none(), "a seat exists only when a person asked");
}

/// V3: a re-keyed node proves its new key with the old one; the row moves in
/// place and the owner files the roster change for a person.
#[test]
fn rotation_repins_in_place_and_files_the_roster_change() {
    let _db = k2_core::db::scoped_for_test();
    test_seams::set_owner_id(Some(OWNER));
    let (n1, old) = owned_node("n1", Taint::Unknown, P9);
    approve(&roster_set("n1", json!({"attest": "checked"})));
    let new = SigningKey::generate().unwrap();
    let rot = RotationStatement {
        v: 1,
        old_fp: old.fingerprint(),
        new_fp: new.fingerprint(),
        new_spki: b64(new.spki_der()),
        new_epoch: 1,
        at: 1_700_000_300,
        reason: ci::RotationReason::Wall,
    };
    let st = NodeStatement { key_born_at: 1_700_000_300, issued_at: 1_700_000_301, ..statement(&new, Taint::Clean, 1, Some(old.fingerprint())) };
    let proof = KeyRotation { rotation: rot.sign(&old).unwrap(), statement: st.sign(&new).unwrap() };
    let moved = on_rotation(&proof).unwrap();
    assert_eq!((moved.id.as_str(), moved.fingerprint.as_str()), (n1.id.as_str(), new.fingerprint().as_str()));
    let rq = get(TRUST_REQUESTS, &Who::Owner);
    let id = rq["requests"].as_array().unwrap().iter().find(|r| r["body"]["op"] == "rotate").unwrap()["id"].as_str().unwrap().to_string();
    approve(&id);
    let h = k2_core::db::shared();
    let r = trust::latest_own_roster(&h.lock()).unwrap();
    assert_eq!(r.roster.entry(&old.fingerprint()).unwrap().status, KeyStatus::Retired);
    let e = r.roster.entry(&new.fingerprint()).unwrap();
    assert_eq!((e.status, e.taint_basis, e.key_epoch, e.name.as_str()), (KeyStatus::Active, Some(TaintBasis::Rekeyed), 1, "n1"));
    // The same proof again: no node pins the old key now.
    assert!(on_rotation(&proof).is_err());
}

/// V17: signer pairing through `k2_signer::pairing`: a person only, the SAS
/// only to a person with a fresh confirm, recorded after the signer's done.
#[cfg(unix)]
#[test]
fn signer_pairing_is_a_persons_and_the_sas_stays_here() {
    use k2_signer::pairing as sp;
    let _db = k2_core::db::scoped_for_test();
    test_seams::set_owner_id(Some(CLIENT));
    let ws = operating_ws("sig");
    let signer = SigningKey::generate().unwrap();
    let t0 = compute::now();
    let (code, hello, mut pending) = sp::signer_begin(&signer, "z3mbpZ", t0).unwrap();
    let prefix = code.split('.').nth(1).unwrap().to_string();
    let body = |tok: Option<String>| json!({"code": code, "hello": hello, "confirmToken": tok});
    assert_eq!(call(SIGNER_ADD, &agent(&ws), body(Some(token(trust::CONFIRM_SIGNER, &format!("add:{prefix}"), "owner")))).unwrap_err().1, tcodes::PERSON_ONLY);
    assert_eq!(call(SIGNER_ADD, &Who::Owner, body(None)).unwrap_err().1, "vault_confirm_required");
    let added = call(SIGNER_ADD, &Who::Owner, body(Some(token(trust::CONFIRM_SIGNER, &format!("add:{prefix}"), "owner")))).unwrap();
    assert!(added.get("sas").is_none(), "no SAS before the reveal");
    let pairing = added["pairing"].as_str().unwrap().to_string();
    let offer: sp::OfferMsg = serde_json::from_value(added["offer"].clone()).unwrap();
    let (_, reveal) = sp::signer_reveal(&signer, &pending, &offer, t0 + 1).unwrap();
    pending.offer = Some(offer);
    // Whoever carries the reveal without a person's confirm gets no SAS.
    assert_eq!(call(SIGNER_REVEAL, &agent(&ws), json!({"pairing": pairing, "reveal": reveal})).unwrap_err().1, tcodes::PERSON_ONLY);
    assert_eq!(call(SIGNER_REVEAL, &Who::Owner, json!({"pairing": pairing, "reveal": reveal})).unwrap_err().1, "vault_confirm_required");
    let shown = call(SIGNER_REVEAL, &Who::Owner, json!({"pairing": pairing, "reveal": reveal, "confirmToken": token(trust::CONFIRM_SIGNER, &pairing, "owner")})).unwrap();
    let sas = shown["sas"].as_str().unwrap().to_string();
    let (fp, _, label, done) = sp::signer_accept(&signer, &pending, &sas, t0 + 2).unwrap();
    assert_eq!((fp, label), (my_fp().unwrap(), CLIENT.to_string()));
    assert_eq!(call(SIGNER_FINISH, &agent(&ws), json!({"pairing": pairing, "done": done})).unwrap_err().1, tcodes::PERSON_ONLY);
    let fin = call(SIGNER_FINISH, &Who::Owner, json!({"pairing": pairing, "done": done})).unwrap();
    assert_eq!(fin["signer"], signer.fingerprint());
    let list = get(SIGNERS, &Who::Owner);
    assert_eq!(list["signers"][0]["signerFp"], signer.fingerprint());
    assert!(call(SIGNER_FINISH, &Who::Owner, json!({"pairing": pairing, "done": done})).is_err(), "once");
    call(SIGNER_REMOVE, &Who::Owner, json!({"signer": signer.fingerprint()})).unwrap();
    assert!(get(SIGNERS, &Who::Owner)["signers"].as_array().unwrap().is_empty());
}
