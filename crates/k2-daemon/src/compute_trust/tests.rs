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

/// CI on compute is a preview (0.46.10 ships it dark, `ciPreview`): these
/// tests run with it on (compute and CI), under the one env lock, so no
/// other test sees the switches (they are put back on drop).
fn ci_on() -> k2_core::test_env::EnvLock {
    let g = k2_core::test_env::lock();
    k2_core::compute::set_enabled(true);
    k2_core::compute::set_ci_enabled(true);
    g
}

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
    let _ci = ci_on();
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
    let _ci = ci_on();
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
    let _ci = ci_on();
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
        // V25's inputs from this server's tables: the seat (active, the
        // lane the owner sent, the node), the key's standing, the consent.
        use k2_core::compute::lane_secrets::{self as ls, NodeStanding};
        let v = ls::seats_with_node_in(&h.lock(), id).pop().expect("the node's seat");
        assert_eq!((v.seat_id.as_str(), v.lane.as_str(), v.active, v.lane_secrets), ("client-ci-standard", "ci-standard", true, true));
        assert_eq!(v.isolation, plane::LaneTemplate::ci_standard().isolation, "the owner's lane template came with the offer");
        assert_eq!(ls::node_standing_in(&h.lock(), &v, &row), NodeStanding::Clean);
        assert_eq!(ls::consent_for(&h.lock(), &v).map(|c| c.owner_key_fp), Some(key_fp.clone()), "the card's consent");
        // Consent withdrawn (an agent may narrow): gone at once.
        call(OWNERS_CONSENT, &agent(&cws), json!({"owner": OWNER, "on": false})).unwrap();
        assert!(ls::consent_for(&h.lock(), &v).is_none());
        // Turning it back on is a person's, with a confirm bound to the key.
        assert_eq!(call(OWNERS_CONSENT, &agent(&cws), json!({"owner": OWNER, "on": true})).unwrap_err().1, tcodes::PERSON_ONLY);
        assert_eq!(call(OWNERS_CONSENT, &Who::Owner, json!({"owner": OWNER, "on": true})).unwrap_err().1, "vault_confirm_required");
        let wrong = token(trust::CONFIRM_TRUST, "consent:other", "owner");
        assert_eq!(call(OWNERS_CONSENT, &Who::Owner, json!({"owner": OWNER, "on": true, "confirmToken": wrong})).unwrap_err().1, "vault_confirm_required");
        let tok = token(trust::CONFIRM_TRUST, &format!("consent:{key_fp}"), "owner");
        call(OWNERS_CONSENT, &Who::Owner, json!({"owner": OWNER, "on": true, "confirmToken": tok})).unwrap();
        assert!(ls::consent_for(&h.lock(), &v).is_some());
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
        let v = k2_core::compute::lane_secrets::seat_view(&h.lock(), "client-ci-standard").unwrap();
        assert_eq!(k2_core::compute::lane_secrets::node_standing_in(&h.lock(), &v, &row), k2_core::compute::lane_secrets::NodeStanding::Revoked);
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
    let _ci = ci_on();
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

/// V3: a re-keyed node proves its new key with the old one; the owner files
/// the change for a person, and once approved the row moves in place.
#[test]
fn rotation_repins_in_place_and_files_the_roster_change() {
    let _ci = ci_on();
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
    assert_eq!(on_rotation(&proof).unwrap_err().0, REKEY_PENDING);
    let rq = get(TRUST_REQUESTS, &Who::Owner);
    let id = rq["requests"].as_array().unwrap().iter().find(|r| r["body"]["op"] == "rotate").unwrap()["id"].as_str().unwrap().to_string();
    approve(&id);
    let moved = store::node_by_id(&k2_core::db::shared().lock(), &n1.id).unwrap();
    assert_eq!((moved.id.as_str(), moved.fingerprint.as_str()), (n1.id.as_str(), new.fingerprint().as_str()));
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
    let _ci = ci_on();
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

// ── C3a + C3b end to end ─────────────────────────────────────────────

/// A protocol-9 seat node's offer to the client: lane secrets, a per-job
/// uid, the job wall, artifacts and their delete (V27), as a `trust team`
/// tenant on a Linux node.
fn seat_offer(protocol: u32, lane: bool) -> pframes::Offer {
    use k2_core::compute::proto::frames::{Availability, Caps, Control, Free};
    use k2_core::compute::proto::share::{Role, ServerStatus, ShareLimits, ShareView};
    let mut features: Vec<String> = [
        k2_core::compute::proto::secret::FEATURE_SECRET_ENV,
        pframes::FEATURE_JOB_WALL,
        k2_core::compute::proto::results::FEATURE_ARTIFACTS,
        "stall",
        pshare::FEATURE_QUEUE,
        pshare::FEATURE_SERVERS,
    ]
    .iter()
    .map(|f| f.to_string())
    .collect();
    if lane {
        features.extend([ci::FEATURE_LANE_SECRETS, ci::FEATURE_JOB_UID, ci::FEATURE_ARTIFACT_DELETE].iter().map(|f| f.to_string()));
    }
    let mut limits = ShareLimits::new_server(pshare::Trust::Team, false);
    limits.max_running = 8;
    pframes::Offer {
        share: Some(ShareView {
            role: Role::Tenant,
            slot: 1,
            shared: true,
            servers: 2,
            owner_label: Some(OWNER.into()),
            state: ServerStatus::Active,
            limits,
            running: 0,
            queued: 0,
            cpu_secs_today: 0,
            expires_in_secs: None,
            skew_secs: None,
            disk_used_bytes: None,
            quotes: vec![],
            priority: None,
        }),
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 8000, mem_bytes: 32 << 30, disk_budget_bytes: 100 << 30, hard: true, gpus: 0 },
        free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: 500 << 30, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: Default::default(),
        protocol,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.10".into(),
        features,
        host: None,
        pkg: None,
    }
}

/// Wait (≤ 20 s) for a frame `f` picks out among what the client sent a
/// fake node session.
fn wait_frame<T>(conn: &mut crate::compute_ws::TestConn, mut f: impl FnMut(&Frame) -> Option<T>) -> T {
    let t0 = std::time::Instant::now();
    loop {
        if let Some(x) = conn.frames().iter().find_map(&mut f) {
            return x;
        }
        assert!(t0.elapsed() < std::time::Duration::from_secs(20), "the frame never came");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// C3a + C3b across both halves (V17, V20–V27): the client's agent asks
/// for a seat and its person approves the card (the owner's roster key
/// pinned, the secrets consent given); the owner approves a lane with
/// secrets and a seat on two nodes; the seat nodes enrol on the client;
/// the client grants a lane secret, submits a secret job, and the one
/// controller check (`compute_secrets::secret_gate` over
/// `lane_secrets::TrustSeats`) seals it to that seat node, where the node's
/// one-time key opens it. The node's half reads the owner's lanes file
/// (`k2-node` `FileLanes`): the seated client gets the lane, a protocol-8
/// tenant on the same node never does. Revoking the consent stops the
/// next release at once. A secret job's artifacts are fetched and deleted
/// from the node (V27). Fakes only; the vault is a temp vault.
#[test]
fn lane_secrets_end_to_end_between_two_servers() {
    let _ci = ci_on();
    use k2_core::compute::lane_secrets::{self as ls, TrustSeats};
    use k2_core::compute::proto::secret::{self as psecret, codes as scodes, NodeKey};
    use k2_vault::{Actor, Delivery, GrantRequest, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore, Selector};

    const ENV: &str = "BUILD_GMAIL_CLIENT_ID";
    const VALUE: &str = "k2test-secret-E2E-7Hq2mV9xL4";
    let owner_guard = k2_core::db::scoped_for_test();
    let client_guard = k2_core::db::scoped_for_test();
    let (odb, cdb) = (owner_guard.handle(), client_guard.handle());
    let owner_peer = peer("owner", OWNER);
    let client_peer = peer("client", CLIENT);
    test_seams::add_peer(owner_peer.clone());
    test_seams::add_peer(client_peer.clone());
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

    // Owner: two walled Linux nodes on the roster; the lane, secrets on.
    let (onodes, keys, ows) = on(&odb, OWNER, || {
        let ows = operating_ws("e2e-own");
        let mut nodes = Vec::new();
        let mut keys = Vec::new();
        for n in ["e1", "e2"] {
            let (row, k) = owned_node(n, Taint::Clean, P9);
            approve(&roster_set(n, json!({})));
            nodes.push(row);
            keys.push(k);
        }
        let r = call(LANES_SET, &agent(&ows), json!({"name": "ci-standard", "from": "ci-standard"})).unwrap();
        approve(r["request"]["id"].as_str().unwrap());
        let h = k2_core::db::shared();
        assert!(trust::lane(&h.lock(), "ci-standard").unwrap().secrets, "the lane carries secrets");
        (nodes, keys, ows)
    });

    // Client: an agent asks (secrets on by default); the card says what
    // the person agrees to; a person approves with a fresh confirm.
    let cws = on(&cdb, CLIENT, || operating_ws("e2e-cli"));
    let req = on(&cdb, CLIENT, || call(SEATS_REQUEST, &agent(&cws), json!({"owner": OWNER, "lane": "ci-standard", "why": "release builds"})).unwrap());
    let summary = req["request"]["summary"].as_str().unwrap().to_string();
    assert!(summary.contains("vault secrets") && summary.contains("root"), "the card names the consent: {summary}");
    let creq = req["request"]["id"].as_str().unwrap().to_string();
    let e = on(&cdb, CLIENT, || call(TRUST_APPROVE, &agent(&cws), json!({"request": creq, "confirmToken": token(trust::CONFIRM_TRUST, &creq, "owner")})).unwrap_err());
    assert_eq!(e.1, tcodes::PERSON_ONLY, "an agent never gives the consent");
    let out = on(&cdb, CLIENT, || approve(&creq));
    let oreq = out["result"]["ownerRequest"].as_str().unwrap().to_string();
    let key_fp = on(&odb, OWNER, || roster_key().unwrap().fingerprint());

    // Owner: the seat on both nodes, one approval.
    let seat_out = on(&odb, OWNER, || {
        call(SEATS_OFFER, &agent(&ows), json!({"request": oreq, "nodes": ["e1", "e2"]})).unwrap();
        approve(&oreq)
    });
    assert!(seat_out["result"]["plans"].as_array().unwrap().iter().all(|p| p["queued"] == true), "{seat_out}");
    // The lanes file each node gets (root's, through `lanes.write`).
    let lanes_file: LanesFile = on(&odb, OWNER, || {
        let n = &onodes[0];
        let h = k2_core::db::shared();
        let stored = k2_core::compute::admin::list_requests(&h.lock(), &k2_core::compute::admin::ReqFilter { node_id: Some(&n.id), state: None, limit: 50, ..Default::default() });
        let plan = stored.iter().find(|r| r.commands.iter().any(|c| node_open(n, c).cmd.verb == plane::VERB_LANES_WRITE)).expect("the seat's plan");
        let o = plan.commands.iter().map(|c| node_open(n, c)).find(|o| o.cmd.verb == plane::VERB_LANES_WRITE).unwrap();
        plane::check_write_args(&o.cmd.args).unwrap()
    });
    let client_fp = on(&cdb, CLIENT, || my_fp().unwrap());

    // Client: the seat is active with the owner's lane; both seat nodes
    // enrol with the seat code (V20) and are tied to the seat.
    let ids = on(&cdb, CLIENT, || {
        let h = k2_core::db::shared();
        let s = trust::seat(&h.lock(), "client", "client-ci-standard").unwrap();
        assert_eq!(s.state, "active");
        let binding = s.code_binding.clone().unwrap();
        let mut ids = Vec::new();
        for (k, rname) in keys.iter().zip(["e1", "e2"]) {
            let (seat, name) = seat_enrol(&binding, &k.fingerprint()).unwrap();
            assert_eq!(name, rname);
            let id = format!("client-node-{rname}");
            {
                let c = h.lock();
                let labels = BTreeMap::from([("os".to_string(), "linux".to_string())]);
                store::insert_pending_node(&c, &id, &seat_node_local_name(&name, &seat.id), &k.fingerprint(), &k.spki_pem(), &labels, &format!("seat:{}", seat.id), "000000", 5).unwrap();
                store::set_grant(&c, &id, &cws, &store::GrantLimits::default(), "owner", 6).unwrap();
            }
            seat_enrolled(&id, &seat);
            ids.push(id);
        }
        ids
    });

    // The node's half (k2-node, V24): root's lanes file names the seated
    // client's lane by its full key; a protocol-8 tenant on the same node,
    // or a server with no seat line, never gets lane secrets.
    #[cfg(unix)]
    {
        use k2_node::node::lane_secrets::{lane_refusal, FileLanes, LaneServer, LaneSource};
        let dir = std::env::temp_dir().join(format!("k2-lane-e2e-{}", crypto::random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("k2-node-lanes.toml");
        std::fs::write(&path, lanes_file.render().unwrap()).unwrap();
        let src = FileLanes::new(path);
        let rule = src.lane_for(&LaneServer { slot: 2, fp_prefix: &client_fp[..16], controller_fp: Some(&client_fp) }).expect("the seated client's lane");
        assert!(rule.secrets && rule.lane == "ci-standard");
        assert_eq!(lane_refusal(true, Some(&rule), true, Some(1)), None, "a protocol-9 seated client gets lane secrets");
        assert_eq!(lane_refusal(false, Some(&rule), true, Some(1)).map(|r| r.0), Some(scodes::UNTRUSTED_SERVER), "a protocol-8 tenant never does");
        let stranger = "d".repeat(64);
        assert!(src.lane_for(&LaneServer { slot: 3, fp_prefix: &stranger[..16], controller_fp: Some(&stranger) }).is_none());
        assert_eq!(lane_refusal(true, None, true, Some(1)).map(|r| r.0), Some(scodes::UNTRUSTED_SERVER), "no seat line, no lane secrets");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Client: the controller's book is this server's own tables.
    ls::set_seat_book(Some(std::sync::Arc::new(TrustSeats)));
    let vault = crate::vault::ensure_test_vault();
    on(&cdb, CLIENT, || {
        let mut n1 = crate::compute_ws::test_attach(&ids[0], seat_offer(9, true));
        let mut n2 = crate::compute_ws::test_attach(&ids[1], seat_offer(8, false));
        // A secret, granted to the lane (the CLI form, V26).
        let mut put = PutRequest::new(Namespace::User, Scope::workspace(&cws), "GMAIL_CLIENT_ID", Kind::ApiKey);
        put.default_delivery = Some(Delivery::Fetch);
        put.delivery_ceiling = Some(Delivery::Env);
        put.hosts = vec!["oauth2.googleapis.com".into()];
        let secret_id = vault.put(put, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).unwrap().meta.id;
        let consumer = crate::compute_secrets::consumer(&format!("lane:{OWNER}/ci-standard"), Some(&cws)).expect("the lane consumer");
        assert_eq!(consumer, k2_vault::Consumer::compute_lane(&cws, "client-ci-standard"));
        vault
            .grant(
                GrantRequest {
                    selector: Selector::Id(secret_id.clone()),
                    consumer,
                    delivery: Delivery::Env,
                    env_name: Some(ENV.into()),
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: None,
                    version: None,
                },
                &Actor::Owner,
            )
            .unwrap();
        let name = |id: &str| store::node_by_id(&k2_core::db::shared().lock(), id).unwrap().name;
        let submit = |node: &str| {
            crate::compute_routes::submit_for_ci(
                &cws,
                &format!("run{}", crypto::random_id().unwrap()),
                &json!({"node": node, "argv": ["true"], "noSource": true, "clientJobId": uuid::Uuid::new_v4().simple().to_string(),
                    "secrets": [{"name": "GMAIL_CLIENT_ID"}]}),
            )
        };
        let job_of = |v: &Value| v["job"]["id"].as_str().unwrap().to_string();
        let need = |job: &str, key: &NodeKey| {
            let r = store::job_by_id(&k2_core::db::shared().lock(), job).unwrap();
            let n = pframes::SecretNeed { job_id: job.into(), generation: r.generation as u32, attempt: r.attempt as u32, plan_digest: r.plan_digest.clone(), epk: key.public_b64() };
            (r, n)
        };

        // Submit on the seat node: placed, names only in the plan.
        let job = job_of(&submit(&name(&ids[0])).unwrap_or_else(|e| panic!("{} {}", e.code, e.hint)));
        let plan = wait_frame(&mut n1, |f| match f {
            Frame::Assign(a) if a.fence.job_id == job => Some(a.plan.clone()),
            _ => None,
        });
        assert_eq!(plan.secret_env, vec![ENV.to_string()]);
        // Release: sealed to the node's one-time key, opened there.
        let key = NodeKey::generate().unwrap();
        let (r1, n) = need(&job, &key);
        let (env, names, _) = crate::compute_secrets::release(&ids[0], &n).unwrap_or_else(|e| panic!("refused: {e:?}"));
        assert_eq!(names, vec![ENV.to_string()]);
        let binding = psecret::Binding {
            controller_fp: k2_core::compute::controller_key().unwrap().fingerprint(),
            node_fp: keys[0].fingerprint(),
            job_id: job.clone(),
            generation: r1.generation as u32,
            attempt: r1.attempt as u32,
            plan_digest: r1.plan_digest.clone(),
        };
        let plain = key.open(&env.epk, &env.ct, &binding).expect("opens with the node's key");
        assert_eq!(plain.0, vec![(ENV.to_string(), VALUE.to_string())]);

        // V27: the job ends with a kept artifact; the controller fetches
        // it, checks the hash, and has the node delete its copy.
        let body = b"baked-in client id".to_vec();
        let info = k2_core::compute::proto::results::ArtifactInfo { name: "dist/app.tar.gz".into(), sha256: crypto::sha256_hex(&body), bytes: body.len() as u64 };
        crate::compute_ws::test_frame(
            &ids[0],
            Frame::State(pframes::StateUpdate {
                job_id: job.clone(),
                generation: r1.generation as u32,
                state: pframes::JobState::Done,
                reason: None,
                detail: None,
                at: compute::now(),
                exit: None,
                src: None,
                log_seq: 0,
                outputs: Some(k2_core::compute::proto::results::Outputs { artifacts: vec![info.clone()], ..Default::default() }),
                queue: None,
            }),
        );
        let get = wait_frame(&mut n1, |f| match f {
            Frame::ArtifactGet(g) if g.job_id == job => Some(g.clone()),
            _ => None,
        });
        crate::compute_ws::test_frame(
            &ids[0],
            Frame::ArtifactChunk(pframes::ArtifactChunk { request_id: get.request_id.clone(), seq: 0, data: b64(&body), last: true, total_bytes: body.len() as u64, sha256: info.sha256.clone() }),
        );
        let del = wait_frame(&mut n1, |f| match f {
            Frame::ArtifactDelete(d) => Some(d.clone()),
            _ => None,
        });
        assert_eq!((del.job_id.as_str(), del.generation), (job.as_str(), r1.generation as u32));
        assert!(crate::compute_ws::artifact_cache_path(&job, r1.generation, &info.sha256).is_file(), "held here first");
        #[cfg(unix)]
        {
            // The node's handler: its own copy of that attempt goes.
            let root = std::env::temp_dir().join(format!("k2-lane-e2e-art-{}", crypto::random_id().unwrap()));
            let keep = k2_node::artifacts::dir(&root, &del.job_id, del.generation);
            std::fs::create_dir_all(keep.join("dist")).unwrap();
            std::fs::write(keep.join("dist/app.tar.gz"), &body).unwrap();
            assert!(k2_node::artifacts::delete(&root, &del.job_id, del.generation, true));
            assert!(!keep.exists());
            let _ = std::fs::remove_dir_all(&root);
        }

        // The protocol-8 seat node: the lane grant covers it, the gate says
        // no (at submit, before anything is queued).
        let e = submit(&name(&ids[1])).unwrap_err();
        assert_eq!(e.code, scodes::NEEDS_UPDATE, "{}", e.hint);
        assert!(n2.frames().iter().all(|f| !matches!(f, Frame::Assign(_))), "never assigned");

        // Revoking the consent (an agent may narrow) stops the next
        // release at once.
        let job2 = job_of(&submit(&name(&ids[0])).unwrap_or_else(|e| panic!("{} {}", e.code, e.hint)));
        wait_frame(&mut n1, |f| matches!(f, Frame::Assign(a) if a.fence.job_id == job2).then_some(()));
        call(OWNERS_CONSENT, &agent(&cws), json!({"owner": OWNER, "on": false})).unwrap();
        let (_, n) = need(&job2, &NodeKey::generate().unwrap());
        let refused = crate::compute_secrets::release(&ids[0], &n).err().flatten().expect("refused");
        assert_eq!(refused.0, ci::codes::SECRET_OWNER_NOT_TRUSTED, "{}", refused.1);
        // And the next submit too, naming the person's command.
        let e = submit(&name(&ids[0])).unwrap_err();
        assert_eq!(e.code, ci::codes::SECRET_OWNER_NOT_TRUSTED);
        assert!(e.hint.contains(&format!("k2 compute owners consent {OWNER} on")), "{}", e.hint);
        // Given again by a person (a confirm bound to the pinned key): the
        // seal works again.
        let tok = token(trust::CONFIRM_TRUST, &format!("consent:{key_fp}"), "owner");
        call(OWNERS_CONSENT, &Who::Owner, json!({"owner": OWNER, "on": true, "confirmToken": tok})).unwrap();
        submit(&name(&ids[0])).unwrap_or_else(|e| panic!("{} {}", e.code, e.hint));
        crate::compute_ws::test_detach(&ids[0]);
        crate::compute_ws::test_detach(&ids[1]);
    });
    ls::set_seat_book(None);
    test_seams::set_fed(None);
    drop(client_guard);
    drop(owner_guard);
}

// ── 11-07 trust + secrets review ─────────────────────────────────────

/// A person's vault confirm through the real route, with the Mac sheet
/// faked: what the sheet would say comes back with the answer.
fn sheet(action: &str, secret_id: &str, label: &str) -> (u16, Value, String) {
    use crate::vault::actor::{Channel, VaultCaller};
    static SAID: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    SAID.lock().unwrap().clear();
    crate::vault_routes::test_hooks::set_local_auth_fake(Some(|reason| {
        *SAID.lock().unwrap() = reason.to_string();
        Ok(())
    }));
    let owner = VaultCaller { actor: k2_vault::Actor::Owner, channel: Channel::Plain, session_id: None };
    let body = json!({"action": action, "secretId": secret_id, "method": "local-auth", "label": label}).to_string();
    let r = crate::vault_routes::handle_on(&vault(), crate::vault_routes::CONFIRM_FINISH, Ok(owner), body.as_bytes(), &HashMap::new(), "test", None);
    crate::vault_routes::test_hooks::set_local_auth_fake(None);
    let said = SAID.lock().unwrap().clone();
    (r.status[..3].parse().unwrap(), serde_json::from_str(&r.body).unwrap(), said)
}

/// H1 (11-07 review): the Touch ID sheet for a trust approval, a secrets
/// consent or a signer pairing says what the daemon stored, never words
/// the caller chose (anything holding the owner token could otherwise show
/// "reveal README" and get a token that approves a roster change). A
/// confirm for nothing that waits is never minted.
#[test]
fn the_confirm_sheet_says_what_the_daemon_stored() {
    let _ci = ci_on();
    let _db = k2_core::db::scoped_for_test();
    test_seams::set_owner_id(Some(OWNER));
    owned_node("n1", Taint::Clean, P9);
    let id = roster_set("n1", json!({}));
    let summary = {
        let h = k2_core::db::shared();
        let c = h.lock();
        trust::request_by_id(&c, &id).unwrap().summary
    };
    // The caller's label is ignored; the sheet says the stored request.
    let (code, r, said) = sheet(trust::CONFIRM_TRUST, &id, "reveal README");
    assert_eq!(code, 200, "{r}");
    assert!(!said.contains("README"), "the caller's words never reach the sheet: {said}");
    let head: String = summary.chars().take(40).collect();
    assert!(said.contains(&head), "the sheet says the stored request: {said} / {summary}");
    // No such request, or one already decided: no token at all.
    let (code, r, _) = sheet(trust::CONFIRM_TRUST, "ctr_not_a_request", "approve");
    assert_eq!(code, 400, "{r}");
    approve(&id);
    let (code, r, _) = sheet(trust::CONFIRM_TRUST, &id, "approve");
    assert_eq!(code, 400, "a decided request gets no confirm: {r}");
    // The secrets consent: the sheet names the pinned owner and its root.
    let o = wire::ApprovalSigner::from_seed(&[9u8; 32]).unwrap();
    {
        let h = k2_core::db::shared();
        trust::pin_owner(&h.lock(), &o.public(), "nodes.k2.dev", &"a".repeat(64), "owner", "local-auth", false, 1).unwrap();
    }
    let (code, r, said) = sheet(trust::CONFIRM_TRUST, &format!("consent:{}", o.public().fingerprint()), "turn on the lights");
    assert_eq!(code, 200, "{r}");
    assert!(said.contains("nodes.k2.dev") && said.contains("root") && !said.contains("lights"), "{said}");
    let (code, _, _) = sheet(trust::CONFIRM_TRUST, &format!("consent:{}", "f".repeat(64)), "x");
    assert_eq!(code, 400, "no pinned owner, no consent confirm");
    // Signer pairing: the sheet says what pairs, never the label.
    let (code, r, said) = sheet(trust::CONFIRM_SIGNER, "add:0123456789abcdef", "show a photo");
    assert_eq!(code, 200, "{r}");
    assert!(said.contains("0123456789abcdef") && said.contains("signer") && !said.contains("photo"), "{said}");
    let (code, _, _) = sheet(trust::CONFIRM_SIGNER, "csp_not_a_pairing", "x");
    assert_eq!(code, 400, "no such pairing, no confirm");
    let (code, _, _) = sheet(trust::CONFIRM_SIGNER, "add:not-hex", "x");
    assert_eq!(code, 400);
}

/// The proof a node sends to move from `old` to `new` (epoch `epoch`).
fn rotation_proof(old: &SigningKey, new: &SigningKey, epoch: u32) -> KeyRotation {
    let rot = RotationStatement {
        v: 1,
        old_fp: old.fingerprint(),
        new_fp: new.fingerprint(),
        new_spki: b64(new.spki_der()),
        new_epoch: epoch,
        at: 1_700_000_300,
        reason: ci::RotationReason::Wall,
    };
    let st = NodeStatement { key_born_at: 1_700_000_300, issued_at: 1_700_000_301, ..statement(new, Taint::Clean, epoch, Some(old.fingerprint())) };
    KeyRotation { rotation: rot.sign(old).unwrap(), statement: st.sign(new).unwrap() }
}

fn pinned_fp(node_id: &str) -> String {
    store::node_by_id(&k2_core::db::shared().lock(), node_id).unwrap().fingerprint
}

fn open_rotations() -> Vec<Value> {
    get(TRUST_REQUESTS, &Who::Owner)["requests"].as_array().unwrap().iter().filter(|r| r["body"]["op"] == "rotate").cloned().collect()
}

/// H2 (11-07 review): a node re-key moves the pinned key only after a
/// person approves it (until then the node with the new key isn't let in);
/// a key that isn't active in the roster can't hand over to a new one; and
/// revoking a key as compromised revokes every key it handed over to (a
/// thief who re-keyed first doesn't survive the revoke).
#[test]
fn a_rekey_waits_for_a_person_and_a_compromise_follows_the_chain() {
    let _ci = ci_on();
    let _db = k2_core::db::scoped_for_test();
    test_seams::set_owner_id(Some(OWNER));
    let (n1, old) = owned_node("n1", Taint::Clean, P9);
    approve(&roster_set("n1", json!({})));
    let new = SigningKey::generate().unwrap();
    let proof = rotation_proof(&old, &new, 1);

    // Held: the pin stays on the old key; one request, however often the
    // node knocks.
    let e = on_rotation(&proof).unwrap_err();
    assert_eq!(e.0, "rekey_pending", "{e:?}");
    assert_eq!(pinned_fp(&n1.id), old.fingerprint(), "no person yet: the pin stays");
    assert!(on_rotation(&proof).is_err());
    let held = open_rotations();
    assert_eq!(held.len(), 1, "{held:?}");
    let id = held[0]["id"].as_str().unwrap().to_string();
    // A person approves: now the row moves, and the roster follows.
    approve(&id);
    assert_eq!(pinned_fp(&n1.id), new.fingerprint());
    {
        let h = k2_core::db::shared();
        let r = trust::latest_own_roster(&h.lock()).unwrap();
        assert_eq!(r.roster.entry(&old.fingerprint()).unwrap().status, KeyStatus::Retired);
        assert_eq!(r.roster.entry(&new.fingerprint()).unwrap().status, KeyStatus::Active);
    }

    // A retired key can't re-key (nothing is filed, nothing moves).
    let (n2, k2) = owned_node("n2", Taint::Clean, P9);
    approve(&roster_set("n2", json!({})));
    let r = call(ROSTER_PROPOSE, &Who::Owner, json!({"op": "retire", "node": "n2"})).unwrap();
    approve(r["request"]["id"].as_str().unwrap());
    let e = on_rotation(&rotation_proof(&k2, &SigningKey::generate().unwrap(), 1)).unwrap_err();
    assert_eq!(e.0, tcodes::ROTATION_REFUSED, "{e:?}");
    assert_eq!(pinned_fp(&n2.id), k2.fingerprint());
    assert_eq!(open_rotations().len(), 0);

    // n1's OLD key turns out stolen: revoking it revokes the key it handed
    // over to as well.
    let r = call(ROSTER_PROPOSE, &Who::Owner, json!({"op": "revoke", "node": old.fingerprint(), "reason": "compromised"})).unwrap();
    approve(r["request"]["id"].as_str().unwrap());
    let h = k2_core::db::shared();
    let latest = trust::latest_own_roster(&h.lock()).unwrap();
    assert_eq!(latest.roster.entry(&old.fingerprint()).unwrap().status, KeyStatus::Revoked);
    let next = latest.roster.entry(&new.fingerprint()).unwrap();
    assert_eq!((next.status, next.revoked_reason), (KeyStatus::Revoked, Some(ci::RevokeReason::Compromised)), "the chain follows the revoke");
    assert!(trust::receipt_void(&h.lock(), &new.fingerprint(), Some(1)).is_some());
}
