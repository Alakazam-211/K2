//! E2 tests: `compute.admin-approve` end to end on the real node admin
//! route (fakes only: the compute admin test vault, a generated controller
//! key and a fake node session; compute is turned on for this thread only,
//! never process-wide).

use std::collections::{BTreeMap, HashMap};

use k2_core::compute::admin as cadmin;
use k2_core::compute::proto::admin as wire;
use k2_core::compute::proto::crypto;
use k2_core::compute::proto::frames::Frame;
use k2_core::connect_users::Role;
use k2_core::permission::request::RequestState;
use serde_json::{json, Value};

use super::super::held::tests::{
    agent, answer_as, ask, ask_ok, ask_refused, below_floor, card_of, last_turn, login, not_requestable, row, seed, view, Ws,
};
use super::test_seams::COMPUTE_ON;
use crate::compute_admin::{self, test_seams as nodes, APPROVE, PROPOSE, WITHDRAW};
use crate::compute_routes::Who;

/// Compute on (or off) for this thread; restored on drop.
struct ComputeOn;
impl ComputeOn {
    fn set(on: bool) -> ComputeOn {
        COMPUTE_ON.with(|c| c.set(Some(on)));
        ComputeOn
    }
}
impl Drop for ComputeOn {
    fn drop(&mut self) {
        COMPUTE_ON.with(|c| c.set(None));
    }
}

/// An active node with admin, online, and `ws` allowed to operate nodes.
fn node_for(ws: &Ws) -> (String, String) {
    let node = format!("node-e2-{}", crypto::random_id().expect("id"));
    let name = format!("mini-e2-{}", &crypto::random_id().expect("id")[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = BTreeMap::from([("os".to_string(), "macos".to_string()), ("arch".to_string(), "aarch64".to_string())]);
    k2_core::compute::store::insert_pending_node(&conn, &node, &name, &crypto::sha256_hex(node.as_bytes()), "pem", &labels, "owner", "123456", 5)
        .expect("enroll");
    k2_core::compute::store::confirm_node(&conn, &node, "owner", 6).expect("confirm");
    k2_core::compute::store::touch_seen(&conn, &node, "lan", 2, "0.45.5", k2_core::compute::now());
    cadmin::set_agents_can_operate_compute(&conn, &ws.id, true).expect("operate");
    drop(conn);
    nodes::register(&node, true, &[wire::FEATURE_ADMIN, wire::FEATURE_HEALTH, wire::FEATURE_SELF_UPDATE]);
    (node, name)
}

fn call(path: &str, who: &Who, body: Value) -> Result<Value, (u16, String)> {
    compute_admin::handle(path, who, body.to_string().as_bytes(), &HashMap::new()).map_err(|e| (e.status, e.code.to_string()))
}

/// The agent proposes a privileged step: a held node admin request.
fn propose(ws: &Ws, node_name: &str) -> String {
    let r = call(PROPOSE, &agent(ws), json!({ "node": node_name, "verb": "unit.restart", "args": { "unit": "k2-node" }, "reason": "wedged" }))
        .expect("proposed");
    assert_eq!(r["request"]["state"], "pending", "{r}");
    r["request"]["id"].as_str().expect("id").to_string()
}

fn token_for(id: &str, stamp: &str) -> String {
    nodes::vault_handle().confirm.mint(cadmin::CONFIRM_ACTION, Some(id), stamp, "local-auth")
}

fn state_of(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    cadmin::request_by_id(&conn, id).expect("the node admin request").state
}

#[test]
fn compute_admin_approve_is_registered_and_the_other_compute_rows_arent_cards() {
    use k2_core::permission::catalog::{ConfirmAction, Floor, Proof};
    let r = super::super::find("compute.admin-approve").expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof), (Floor::Owner, Proof::SettingsConfirm { action: ConfirmAction::NodeAdmin }));
    assert!(crate::routes::route_policy::lookup(APPROVE).and_then(|x| x.post).is_some(), "{APPROVE} has a POST row");
    assert_eq!(r.spec.machine_hints.iter().map(|h| h.code).collect::<Vec<_>>(), ["remote_admin_off"]);

    let ws = seed("e2cn");
    not_requestable(&ws, &["compute.node.remove", "compute.node.retire", "compute.local", "compute.node.remote-admin", "compute.admin-deny"]);
    let (s, v) = ask(&ws, "compute.agents", json!({}));
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["permission"]["action"], "compute.use", "covered by compute.use: {v}");
}

#[test]
fn compute_admin_approve_sends_the_signed_step_as_the_owner() {
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("e2cp");
    let (node, name) = node_for(&ws);
    let id = propose(&ws, &name);

    let v = ask_ok(&ws, "compute.admin-approve", json!({ "request": id }));
    assert_eq!(v["id"], id.as_str(), "the node admin request keeps its own id: {v}");
    let exp = v["expiresAt"].as_i64().expect("expiresAt");
    assert!((exp - (k2_core::overlay::now_secs() + cadmin::REQUEST_TTL_SECS)).abs() <= 5, "the card lives as long as the request (24 h): {v}");
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": "node-admin", "secretId": id }));
    assert_eq!(vw["scope"], format!("node request {id}"));
    assert_eq!(vw["settingsPage"], "compute");

    // An Admin: below the floor at the card and at the route.
    let admin = login("e2cpa", Role::Admin);
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": token_for(&id, &format!("connect:{admin}")) })), "owner");
    let e = call(APPROVE, &Who::Login { username: admin.clone(), role: Role::Admin }, json!({ "request": id, "confirmToken": token_for(&id, "owner") }))
        .expect_err("an Admin login never approves a node root step");
    assert_eq!(e, (403, "owner_only".to_string()));

    // A confirm for another request decides nothing.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": token_for("car_someone_else", "owner") }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert!(row(&id).is_answerable());
    assert_eq!(state_of(&id), "pending");
    assert!(nodes::take_sent(&node).is_empty(), "nothing is sent while held");

    // The owner, confirmed: signed, sent, and the agent hears once.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": token_for(&id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(state_of(&id), "sent");
    let sent: Vec<Frame> = nodes::take_sent(&node);
    assert_eq!(sent.iter().filter(|f| matches!(f, Frame::Admin(_))).count(), 1, "one signed command went out");
    assert_eq!(row(&id).state, RequestState::Approved);
    assert_eq!(last_turn(&ws), format!("[permission:{id}] approved by owner: once, for node request {id}. It is on its way to the node now."));
}

#[test]
fn compute_admin_approve_is_off_while_compute_is_dark_and_follows_the_request() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("e2cs");
    let other = seed("e2co");
    let (_node, name) = node_for(&ws);
    let (_onode, oname) = node_for(&other);
    let id = {
        let _on = ComputeOn::set(true);
        propose(&ws, &name)
    };
    {
        let _off = ComputeOn::set(false);
        assert!(ask_refused(&ws, "compute.admin-approve", json!({ "request": id })).contains("compute nodes are off"));
    }
    let _on = ComputeOn::set(true);
    let theirs = propose(&other, &oname);
    assert!(ask_refused(&ws, "compute.admin-approve", json!({ "request": theirs })).contains("no node admin request"));

    // Withdrawn while the card waits → stale; nothing is signed.
    let v = ask_ok(&ws, "compute.admin-approve", json!({ "request": id }));
    call(WITHDRAW, &agent(&ws), json!({ "request": id })).expect("withdrawn");
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow", "confirmToken": token_for(&id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Stale);
    assert_eq!(last_turn(&ws), format!("[permission:{id}] stale: node admin request {id} was withdrawn meanwhile. Nothing was applied."));

    // Approved in Settings first: nothing to ask.
    let done = propose(&ws, &name);
    call(APPROVE, &Who::Owner, json!({ "request": done, "confirmToken": token_for(&done, "owner") })).expect("approved in Settings");
    let (s, v) = ask(&ws, "compute.admin-approve", json!({ "request": done }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
}
