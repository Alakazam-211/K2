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
    agent, answer_as, apply_as, ask, ask_ok, ask_refused, below_floor, card_of, id_of, last_turn, login, not_requestable, refused_code, row,
    seed, view, Ws,
};
use super::super::Answerer;
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
    compute_admin::handle(path, who, body.to_string().as_bytes(), &HashMap::new(), None).map_err(|e| (e.status, e.code.to_string()))
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
    let _db = k2_core::db::scoped_for_test();
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
    let _db = k2_core::db::scoped_for_test();
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
    let _db = k2_core::db::scoped_for_test();
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

// ── held node requests in chat (0.46.9, Rosson 2026-11-05) ──────────

fn set_operate(ws: &Ws, on: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    cadmin::set_agents_can_operate_compute(&conn, &ws.id, on).expect("operate");
}

fn set_manage(ws: &Ws, on: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn
        .execute("UPDATE projects SET agents_can_manage_compute = ?1 WHERE id = ?2", rusqlite::params![on as i64, ws.id])
        .expect("manage");
    assert_eq!(n, 1, "the workspace row exists");
}

/// The card names the exact change (the request's own summary, as
/// Settings shows it), its node and the reason filed with it.
#[test]
fn the_card_names_the_change_and_its_reason() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("e2cw");
    let (_node, name) = node_for(&ws);
    let id = propose(&ws, &name);
    let v = ask_ok(&ws, "compute.admin-approve", json!({ "request": id }));
    let vw = view(&ws, &card_of(&v));
    let summary = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        cadmin::request_by_id(&conn, &id).expect("row").summary
    };
    let want = summary[..1].to_uppercase() + &summary[1..];
    assert_eq!(vw["whatChanges"], format!("{}.", want.trim_end_matches('.')), "{vw}");
    assert!(want.contains(&name), "the change names the node: {want}");
    let cons = vw["consequences"].as_str().unwrap_or_else(|| panic!("no consequences: {vw}"));
    assert!(cons.contains(&format!("it runs on {name} as root")), "{cons}");
    assert!(cons.contains(&format!("This is request {id} in Settings → Cores → Compute → Machine settings → Waiting for you")), "{cons}");
    assert!(cons.ends_with("The agent's reason when it filed the request: wedged"), "{cons}");
    let prompt = vw.to_string();
    assert!(!prompt.contains('\u{2014}'), "no em dashes on the card: {vw}");
    assert!(!prompt.contains("{change}") && !prompt.contains("{reason}") && !prompt.contains("{node}"), "every value filled: {vw}");
}

/// Allow on the card approves once; Settings then finds it decided. Deny on
/// the card denies the node request itself; Settings then can't approve it.
#[test]
fn the_cards_allow_and_deny_are_settings_own_and_decide_once() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("e2cd");
    let (node, name) = node_for(&ws);

    // Allow: applied once; a second approval (Settings) is refused.
    let id = propose(&ws, &name);
    let v = ask_ok(&ws, "compute.admin-approve", json!({ "request": id }));
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow", "confirmToken": token_for(&id, "owner") }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert_eq!(state_of(&id), "sent");
    assert_eq!(nodes::take_sent(&node).iter().filter(|f| matches!(f, Frame::Admin(_))).count(), 1, "one signed command");
    let e = call(APPROVE, &Who::Owner, json!({ "request": id, "confirmToken": token_for(&id, "owner") }))
        .expect_err("already decided on the card");
    assert_eq!(e.0, 409, "{e:?}");
    assert!(nodes::take_sent(&node).is_empty(), "never a second apply");

    // Deny: the node request is denied, as the person, with the reason.
    let id = propose(&ws, &name);
    let v = ask_ok(&ws, "compute.admin-approve", json!({ "request": id }));
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Deny", "reason": "not this week" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Denied);
    let decided = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        cadmin::request_by_id(&conn, &id).expect("row")
    };
    assert_eq!((decided.state.as_str(), decided.deny_reason.as_deref()), ("denied", Some("not this week")), "{decided:?}");
    let e = call(APPROVE, &Who::Owner, json!({ "request": id, "confirmToken": token_for(&id, "owner") })).expect_err("denied on the card");
    assert_eq!(e.0, 409, "{e:?}");
    assert!(nodes::take_sent(&node).is_empty(), "nothing sent for a denied request");

    // Denied in Settings first: the open card goes stale at the click.
    let id = propose(&ws, &name);
    let v = ask_ok(&ws, "compute.admin-approve", json!({ "request": id }));
    call(crate::compute_admin::DENY, &Who::Owner, json!({ "request": id })).expect("denied in Settings");
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow", "confirmToken": token_for(&id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Stale);
    assert_eq!(state_of(&id), "denied");
    assert!(nodes::take_sent(&node).is_empty());
    // … and a new ask for it is refused (it isn't waiting).
    assert!(ask_refused(&ws, "compute.admin-approve", json!({ "request": id })).contains("denied"));
}

/// Only an agent that may operate or manage compute nodes brings one to
/// chat: neither switch is refused at the ask, and a switch turned off
/// while the card waits makes the click stale.
#[test]
fn only_an_agent_that_may_act_on_nodes_raises_the_card() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("e2cm");
    let (node, name) = node_for(&ws);
    let id = propose(&ws, &name);

    set_operate(&ws, false);
    let (s, v) = ask(&ws, "compute.admin-approve", json!({ "request": id }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("compute_admin_off")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("Let this agent manage compute nodes"), "{v}");

    // "Allow managing compute nodes" alone is enough.
    set_manage(&ws, true);
    let v = ask_ok(&ws, "compute.admin-approve", json!({ "request": id }));
    // Turned off while the card waits: stale at the click, nothing sent.
    set_manage(&ws, false);
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow", "confirmToken": token_for(&id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Stale);
    assert_eq!(state_of(&id), "pending", "still waiting in Settings");
    assert!(nodes::take_sent(&node).is_empty());
}

// ── compute.allow-nodes (0.46.8) ─────────────────────────────────────

/// An active node, granted to `ws` or not, with a unique name.
fn list_node(ws: &Ws, tag: &str, granted: bool) -> (String, String) {
    let id = format!("node-al-{}", crypto::random_id().expect("id"));
    let name = format!("{tag}-{}", &crypto::random_id().expect("id")[..6]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = BTreeMap::from([("os".to_string(), "linux".to_string())]);
    k2_core::compute::store::insert_pending_node(&conn, &id, &name, &crypto::sha256_hex(id.as_bytes()), "pem", &labels, "owner", "123456", 5)
        .expect("enroll");
    k2_core::compute::store::confirm_node(&conn, &id, "owner", 6).expect("confirm");
    if granted {
        k2_core::compute::store::set_grant(&conn, &id, &ws.id, &Default::default(), "owner", 7).expect("grant");
    }
    (id, name)
}

fn set_list(ws: &Ws, ids: Option<&[String]>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::allow::set(&conn, &ws.id, ids, "owner", 8).expect("list");
}

#[test]
fn allow_nodes_cards_are_a_persons_and_ask_only_when_needed() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::permission::catalog::{Direction, Floor, Proof};
    for id in ["compute.allow-nodes", "compute.allow-nodes-clear"] {
        let r = super::super::find(id).unwrap_or_else(|| panic!("{id} registered"));
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.spec.floor, r.spec.direction, r.spec.proof), (Floor::Admin, Direction::Loosen, Proof::Click), "{id}");
    }
    assert!(crate::routes::route_policy::lookup(crate::compute_allow::SET).and_then(|x| x.post).is_some(), "the route the cards call");

    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("alnd");
    let (a, a_name) = list_node(&ws, "al-a", true);
    let (_b, b_name) = list_node(&ws, "al-b", true);
    let (_c, c_name) = list_node(&ws, "al-c", false);

    // No list: every granted node is allowed already; nothing to clear.
    let (s, v) = ask(&ws, "compute.allow-nodes", json!({ "nodes": b_name }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
    let (s, v) = ask(&ws, "compute.allow-nodes-clear", json!({}));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");

    // A list of one: adding another granted node is a card an Admin answers.
    set_list(&ws, Some(std::slice::from_ref(&a)));
    let (s, v) = ask(&ws, "compute.allow-nodes", json!({ "nodes": a_name }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "on the list already: {v}");
    let hint = ask_refused(&ws, "compute.allow-nodes", json!({ "nodes": c_name }));
    assert!(hint.contains("has no grant on"), "{hint}");
    assert!(ask_refused(&ws, "compute.allow-nodes", json!({ "nodes": "no-such-node" })).contains("no compute node"));
    let v = ask_ok(&ws, "compute.allow-nodes", json!({ "nodes": b_name }));
    let vw = view(&ws, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Add {b_name} to {}'s compute node list.", ws.handle), "{vw}");
    assert!(vw["consequences"].as_str().unwrap_or("").contains("whole source tree"), "{vw}");
    // A Member login is below the card's floor.
    let member = login("alm", Role::Member);
    below_floor(answer_as(&member, &ws, &card_of(&v), json!({ "answer": "Allow" })), "admin");
    // The clear card, while a list is set (another workspace, so the card
    // is its own and not batched with the one above).
    let ws2 = seed("alnc");
    let (a2, _) = list_node(&ws2, "al-d", true);
    set_list(&ws2, Some(std::slice::from_ref(&a2)));
    let v = ask_ok(&ws2, "compute.allow-nodes-clear", json!({}));
    let vw = view(&ws2, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Remove {}'s compute node list.", ws2.handle), "{vw}");

    // Compute off: neither asks.
    let _off = ComputeOn::set(false);
    assert!(ask_refused(&ws, "compute.allow-nodes", json!({ "nodes": b_name })).contains("compute nodes are off"));
}

// ── compute.allow-nodes-for (0.46.18) ────────────────────────────────

/// The hire ledger says `boss`'s agent hired `ws` (`k2 agent hire`).
fn hired(boss: &Ws, ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let inst = format!("inst-{}", crypto::random_id().expect("id"));
    let by = k2_core::hire::HireCaller::Agent { workspace_id: boss.id.clone() };
    k2_core::hire::ledger::insert_applying(&conn, &inst, &by, None, "h", None, None, 1).expect("instance");
    k2_core::hire::ledger::add_member(&conn, &inst, &ws.id, "lead", &ws.handle, "/w", true).expect("member");
}

/// 0.46.18 (Rosson 2026-11-23): an agent that manages another workspace
/// (it hired it, or it may manage this server's compute nodes) asks for
/// one change to that workspace's node list on a card a person answers.
/// Refused at the ask: an agent that doesn't manage it (409
/// `allow_nodes_not_manager`), a node not granted to that workspace, an
/// unknown node, its own workspace, and no change or two. Already done:
/// nothing is asked. The card names the workspace and the nodes; a Member
/// is below its floor; Deny changes nothing.
#[test]
fn allow_nodes_for_is_a_managing_agents_card_a_person_answers() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::permission::catalog::{Direction, Floor, Grant, Proof};
    let r = super::super::find("compute.allow-nodes-for").expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.direction, r.spec.proof, r.spec.grant), (Floor::Admin, Direction::Loosen, Proof::Click, Grant::OneTime));

    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let boss = seed("alfb");
    let eng = seed("alfe");
    let stranger = seed("alfs");
    hired(&boss, &eng);
    let (a, a_name) = list_node(&eng, "alf-a", true);
    let (_b, b_name) = list_node(&eng, "alf-b", true);
    let (_d, d_name) = list_node(&eng, "alf-d", true);
    let (_c, c_name) = list_node(&boss, "alf-c", true); // the boss's, not eng's
    let asks_of = |ws: &Ws| -> i64 {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1", [&ws.id], |r| r.get(0)).expect("count")
    };

    // No list on eng: every granted node is allowed already.
    let (s, v) = ask(&boss, "compute.allow-nodes-for", json!({ "target": eng.handle, "add": b_name }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
    set_list(&eng, Some(std::slice::from_ref(&a)));

    // An agent that doesn't manage eng can't ask; nothing is stored.
    let (s, v) = ask(&stranger, "compute.allow-nodes-for", json!({ "target": eng.handle, "add": b_name }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("allow_nodes_not_manager")), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains(&format!("{} doesn't manage {}", stranger.handle, eng.handle)), "{hint}");
    assert_eq!(asks_of(&stranger), 0);
    // Nor the other way round: eng didn't hire its boss.
    let (s, v) = ask(&eng, "compute.allow-nodes-for", json!({ "target": boss.handle, "clear": "true" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("allow_nodes_not_manager")), "{v}");

    // Values that don't hold: refused before a card.
    assert!(ask_refused(&boss, "compute.allow-nodes-for", json!({ "target": eng.handle, "add": c_name })).contains("allow_nodes_not_granted"));
    assert!(ask_refused(&boss, "compute.allow-nodes-for", json!({ "target": eng.handle, "add": "no-such-node" })).contains("no compute node 'no-such-node'"));
    assert!(ask_refused(&boss, "compute.allow-nodes-for", json!({ "target": boss.handle, "add": c_name })).contains("is your own workspace"));
    assert!(ask_refused(&boss, "compute.allow-nodes-for", json!({ "target": "no-such-ws", "add": b_name })).contains("no workspace 'no-such-ws'"));
    for params in [json!({ "target": eng.handle }), json!({ "target": eng.handle, "add": b_name, "clear": "true" })] {
        assert!(ask_refused(&boss, "compute.allow-nodes-for", params.clone()).contains("say one change"), "{params}");
    }
    let (s, v) = ask(&boss, "compute.allow-nodes-for", json!({ "target": eng.handle, "clear": "yes" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "clear is true only: {v}");
    // Already so: nothing to ask.
    for params in [json!({ "target": eng.handle, "add": a_name }), json!({ "target": eng.handle, "remove": b_name })] {
        let (s, v) = ask(&boss, "compute.allow-nodes-for", params.clone());
        assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{params}: {v}");
    }
    assert!(ask_refused(&boss, "compute.allow-nodes-for", json!({ "target": eng.handle, "remove": a_name })).contains("leave"));
    assert_eq!(asks_of(&boss), 0, "a refused or done ask stores no request");

    // The card: the workspace and the nodes in plain words.
    let v = ask_ok(&boss, "compute.allow-nodes-for", json!({ "target": eng.handle, "add": format!("{b_name},{d_name}") }));
    let card = card_of(&v);
    let vw = view(&boss, &card);
    assert_eq!(vw["whatChanges"], format!("Add {b_name} and {d_name} to {}'s compute node list.", eng.handle), "{vw}");
    assert_eq!(vw["setting"], format!("{}'s compute node list", eng.handle), "{vw}");
    let cons = vw["consequences"].as_str().unwrap_or("");
    assert!(cons.starts_with(&format!("{}'s jobs may then run on {b_name} and {d_name}", eng.handle)), "{cons}");
    assert!(cons.contains("whole source tree") && cons.contains(&format!("{} asks because it hired {}", boss.handle, eng.handle)), "{cons}");
    assert!(!vw.to_string().contains('\u{2014}'), "no em dashes on the card: {vw}");
    let values = values_of(&vw);
    assert_eq!(values[0], ("workspace".to_string(), boss.handle.clone()), "{vw}");
    assert!(values.contains(&("target".to_string(), eng.handle.clone())), "{vw}");
    assert!(values.iter().any(|(k, v)| k == &b_name && v.contains("owned by this server")), "{vw}");
    assert_eq!(vw["approvers"]["floor"], "admin", "{vw}");

    // A Member is below the floor; Deny changes nothing.
    let member = login("alfm", Role::Member);
    below_floor(answer_as(&member, &boss, &card, json!({ "answer": "Allow" })), "admin");
    let admin = login("alfa", Role::Admin);
    let (s, ans) = answer_as(&admin, &boss, &card, json!({ "answer": "Deny", "reason": "not those" }));
    assert_eq!(s, "200 OK", "{ans}");
    assert_eq!(row(&id_of(&v)).state, RequestState::Denied);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(k2_core::compute::allow::get(&conn, &eng.id).expect("read").expect("a list").nodes, vec![a.clone()], "deny changes nothing");
    }

    // Take a node off, and clear: the words say so.
    let v = ask_ok(&boss, "compute.allow-nodes-for", json!({ "target": eng.handle, "clear": "true" }));
    let vw = view(&boss, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Remove {}'s compute node list.", eng.handle), "{vw}");
    let (s, ans) = answer_as(&admin, &boss, &card_of(&v), json!({ "answer": "Deny" }));
    assert_eq!(s, "200 OK", "{ans}");

    // "Let this agent manage compute nodes" may ask too, for any workspace.
    k2_core::workspace::settings::set_agents_can_manage_compute(&stranger.id, true).expect("manage compute");
    let v = ask_ok(&stranger, "compute.allow-nodes-for", json!({ "target": eng.handle, "add": b_name }));
    let vw = view(&stranger, &card_of(&v));
    assert!(vw["consequences"].as_str().unwrap_or("").contains("it may manage this server's compute nodes"), "{vw}");

    // Compute off: nothing asks.
    let _off = ComputeOn::set(false);
    assert!(ask_refused(&boss, "compute.allow-nodes-for", json!({ "target": eng.handle, "add": d_name })).contains("compute nodes are off"));
}

/// A node's offer: its caps for this server (`cpus` cores, `gb` memory).
fn sized_offer(cpus: u64, gb: u64) -> k2_core::compute::proto::frames::Offer {
    use k2_core::compute::proto::frames::{Availability, Caps, Control, Free, Offer};
    const GB: u64 = 1 << 30;
    Offer {
        share: None,
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: cpus * 1000, mem_bytes: gb * GB, disk_budget_bytes: 100 * GB, hard: true, gpus: 0 },
        free: Free { cpu_millis: cpus * 1000, mem_bytes: gb * GB, disk_free_bytes: 500 * GB, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string()), ("arch".to_string(), "x86_64".to_string())].into(),
        tools: Default::default(),
        protocol: 8,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.13".into(),
        features: vec![],
        host: None,
        pkg: None,
        sim: None,
    }
}

/// The card's values as `key → value`.
fn values_of(vw: &Value) -> Vec<(String, String)> {
    vw["values"]
        .as_array()
        .unwrap_or_else(|| panic!("no values: {vw}"))
        .iter()
        .map(|kv| (kv["key"].as_str().expect("key").to_string(), kv["value"].as_str().expect("value").to_string()))
        .collect()
}

/// 0.46.15: the card shows the person the asking workspace and, for each
/// named node, its platform, its size for this server and who owns it,
/// and says the agent's jobs may then run there. A request can only ever
/// be for the asker's own list: naming a workspace is refused before any
/// card, and so is a node that doesn't exist or isn't granted.
#[test]
fn allow_nodes_card_names_each_node_and_only_the_askers_list() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("alnv");
    let other = seed("alno");
    let (a, _) = list_node(&ws, "al-v", true);
    let (b, b_name) = list_node(&ws, "al-w", true);
    let (c, c_name) = list_node(&ws, "al-x", true);
    let (_d, d_name) = list_node(&ws, "al-y", false);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // b reported its size; c is another server's node and never did.
        k2_core::compute::store::store_offer(&conn, &b, &sized_offer(8, 32), k2_core::compute::now());
        k2_core::compute::store::set_tenant_of(&conn, &c, "lab.example.test").expect("tenant");
    }
    set_list(&ws, Some(std::slice::from_ref(&a)));

    // Never another workspace's list, never an unknown or ungranted node.
    for params in [json!({ "nodes": b_name, "workspace": other.id }), json!({ "nodes": b_name, "workspace": ws.id })] {
        let (s, v) = ask(&ws, "compute.allow-nodes", params.clone());
        assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{params}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap_or("").contains("the workspace is always yours"), "{params}: {v}");
    }
    assert!(ask_refused(&ws, "compute.allow-nodes", json!({ "nodes": format!("{b_name},no-such-node") })).contains("no compute node 'no-such-node'"));
    assert!(ask_refused(&ws, "compute.allow-nodes", json!({ "nodes": format!("{b_name},{d_name}") })).contains("has no grant on"));
    let stored: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1", [&ws.id], |r| r.get(0)).expect("count")
    };
    assert_eq!(stored, 0, "a refused ask stores no request and makes no card");

    let v = ask_ok(&ws, "compute.allow-nodes", json!({ "nodes": format!("{b_name},{c_name}") }));
    let vw = view(&ws, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Add {b_name}, {c_name} to {}'s compute node list.", ws.handle), "{vw}");
    assert!(
        vw["consequences"].as_str().unwrap_or("").starts_with(&format!("{}'s jobs may then run on {b_name}, {c_name}", ws.handle)),
        "{vw}"
    );
    assert_eq!(
        values_of(&vw),
        vec![
            ("workspace".to_string(), ws.handle.clone()),
            ("nodes".to_string(), format!("{b_name}, {c_name}")),
            (b_name.clone(), "linux x86_64 · 8 CPUs · 32 GB · owned by this server".to_string()),
            (c_name.clone(), "linux · size not reported yet · owned by lab.example.test".to_string()),
        ],
        "{vw}"
    );
    assert_eq!(vw["direction"], "loosen", "{vw}");
    assert_eq!(vw["approvers"]["floor"], "admin", "{vw}");
}

/// B2: adding a trusted CI branch is a person's card (Admin floor); a
/// branch already trusted (main, by default) asks nothing; a glob is
/// refused at the ask.
#[test]
fn the_cache_trust_card_is_a_persons_and_asks_only_when_needed() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::permission::catalog::{Direction, Floor, Proof};
    let r = super::super::find("ci.cache-trust").expect("ci.cache-trust registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.direction, r.spec.proof), (Floor::Admin, Direction::Loosen, Proof::Click));
    assert!(crate::routes::route_policy::lookup(crate::ci_routes::CACHE_TRUST_SET).and_then(|x| x.post).is_some(), "the route the card calls");

    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("cctr");
    let (s, v) = ask(&ws, "ci.cache-trust", json!({ "branches": "main" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "main is trusted by default: {v}");
    assert!(ask_refused(&ws, "ci.cache-trust", json!({ "branches": "rel/*" })).contains("not a branch name"));
    let v = ask_ok(&ws, "ci.cache-trust", json!({ "branches": "rel" }));
    let vw = view(&ws, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Trust rel for {}'s CI cache.", ws.handle), "{vw}");
    assert!(vw["consequences"].as_str().unwrap_or("").contains("every job"), "{vw}");
    let member = login("cctm", Role::Member);
    below_floor(answer_as(&member, &ws, &card_of(&v), json!({ "answer": "Allow" })), "admin");
    let _off = ComputeOn::set(false);
    assert!(ask_refused(&ws, "ci.cache-trust", json!({ "branches": "rel" })).contains("compute nodes are off"));
}

// ── compute.packages and compute.packages-policy (0.46.9) ────────────

/// One filed package request batch for `ws` on `node_id`, moved to `state`.
/// Returns the batch id (`cpr_…`).
fn file_pkg(ws: &Ws, node_id: &str, state: &str, origin: &str, server: &str) -> String {
    use k2_core::compute::pkgreq as pr;
    let names = vec!["libgtk-3-dev".to_string(), "libssl-dev".to_string()];
    let dry = k2_core::compute::proto::pkgpolicy::DrySummary {
        installs: vec!["a".into(), "b".into(), "c".into()],
        size_mb: Some(180),
        ..Default::default()
    };
    let batch = pr::new_id();
    let now = k2_core::compute::now();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let r = pr::insert(
        &conn,
        &pr::NewRow {
            batch_id: &batch,
            origin,
            node_id,
            server,
            workspace_id: Some(&ws.id),
            asked_by: &ws.handle,
            why: Some("the Tauri tests need GTK"),
            names: &names,
            dry: Some(&dry),
            host: Some("Ubuntu 24.04.1 LTS"),
            now,
            ..Default::default()
        },
    )
    .expect("filed");
    if state != "pending" {
        assert!(pr::set_state(&conn, &r.id, state, Some("owner"), Some("not on this box"), None, now), "{state}");
    }
    batch
}

fn pkg_rows(batch: &str) -> Vec<k2_core::compute::pkgreq::PkgRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::pkgreq::batch(&conn, batch)
}

#[test]
fn package_cards_are_registered_with_their_floors() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::permission::catalog::{Direction, Floor, Grant, Proof};
    let r = super::super::find("compute.packages").expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant, r.spec.direction), (Floor::Admin, Proof::Click, Grant::OneTime, Direction::Loosen));
    assert_eq!(r.spec.expiry.default_secs, k2_core::compute::proto::pkgpolicy::REQUEST_TTL_SECS, "the card lives as long as the request");
    let p = super::super::find("compute.packages-policy").expect("registered");
    k2_core::permission::catalog::validate(p.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((p.spec.floor, p.spec.proof), (Floor::Owner, Proof::Click));
    for route in [crate::compute_pkg::REQUEST, crate::compute_pkg::POLICY, crate::compute_pkg::DECIDE] {
        assert!(crate::routes::route_policy::lookup(route).and_then(|x| x.post).is_some(), "{route} has a POST row");
    }
    assert!(super::batches_alone("compute.packages"));
    assert!(!super::batches_alone("compute.admin-approve") && !super::batches_alone("compute.packages-policy"));
}

/// The card is K2's words from the stored request: what installs, where,
/// for whom, the dependency count and size, and the agent's reason.
#[test]
fn the_package_card_names_what_installs_where_and_for_whom() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("pkgw");
    let (node, name) = list_node(&ws, "pk-w", true);
    let id = file_pkg(&ws, &node, "pending", k2_core::compute::pkgreq::OWN, "own");
    let v = ask_ok(&ws, "compute.packages", json!({ "request": id }));
    assert_eq!(v["id"], id.as_str(), "the package request keeps its own id: {v}");
    let exp = v["expiresAt"].as_i64().expect("expiresAt");
    assert!((exp - (k2_core::overlay::now_secs() + k2_core::compute::proto::pkgpolicy::REQUEST_TTL_SECS)).abs() <= 5, "{v}");
    let vw = view(&ws, &card_of(&v));
    assert_eq!(
        vw["whatChanges"],
        format!(
            "Install 2 packages on {name} (Ubuntu 24.04.1 LTS) for agent {}: libgtk-3-dev, libssl-dev (3 packages with dependencies, 180 MB).",
            ws.handle
        ),
        "{vw}"
    );
    let cons = vw["consequences"].as_str().unwrap_or_else(|| panic!("no consequences: {vw}"));
    assert!(cons.starts_with(&format!("Root installs them on {name} from the distro's signed official repos")), "{cons}");
    assert!(cons.contains(&format!("This is request {id} in Settings → Cores → Compute → Machine settings → Waiting for you")), "{cons}");
    assert!(cons.ends_with("The agent's reason: the Tauri tests need GTK"), "{cons}");
    assert!(!cons.contains("every job on that machine"), "this server's own agent asked: {cons}");
    let all = vw.to_string();
    assert!(!all.contains('\u{2014}'), "no em dashes on the card: {vw}");
    assert!(!all.contains("{change}") && !all.contains("{shared}") && !all.contains("{reason}"), "every value filled: {vw}");

    // Another server's request: the host-wide line.
    let theirs = file_pkg(&ws, &node, "pending", k2_core::compute::pkgreq::SENT, "acme.k2.dev");
    let mut ctx = super::super::Ctx { workspace_id: ws.id.clone(), workspace_handle: ws.handle.clone(), params: Default::default() };
    ctx.params.insert("request".into(), json!(theirs));
    let filled = super::with_computed("compute.packages", &ctx).expect("computed");
    assert_eq!(filled.params["shared"], "This is installed for every job on that machine. ", "{:?}", filled.params);
    assert!(filled.params["change"].as_str().unwrap_or("").contains("for acme.k2.dev's"), "{:?}", filled.params);
}

/// The ask follows the request: pending here asks; installed or approved
/// is already done; denied, expired, sent to another server's owner,
/// another workspace's, or not a package request at all is refused with
/// words. The node admin card never takes a package request.
#[test]
fn the_package_card_follows_the_request() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::compute::pkgreq::{OWN, SENT};
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("pkgs");
    let other = seed("pkgo");
    let (node, name) = list_node(&ws, "pk-s", true);

    for st in ["installed", "approved"] {
        let id = file_pkg(&ws, &node, st, OWN, "own");
        let (s, v) = ask(&ws, "compute.packages", json!({ "request": id }));
        assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{st}: {v}");
    }
    let denied = file_pkg(&ws, &node, "denied", OWN, "own");
    let hint = ask_refused(&ws, "compute.packages", json!({ "request": denied }));
    assert!(hint.contains(&format!("denied on {name} (not on this box)")), "{hint}");
    let expired = file_pkg(&ws, &node, "expired", OWN, "own");
    let hint = ask_refused(&ws, "compute.packages", json!({ "request": expired }));
    assert!(hint.contains(&format!("expired on {name}")), "{hint}");
    let sent = file_pkg(&ws, &node, "pending", SENT, "acme.k2.dev");
    let hint = ask_refused(&ws, "compute.packages", json!({ "request": sent }));
    assert!(hint.contains("went to the node's owner (acme.k2.dev)"), "{hint}");
    let theirs = file_pkg(&other, &node, "pending", OWN, "own");
    let hint = ask_refused(&ws, "compute.packages", json!({ "request": theirs }));
    assert!(hint.contains("no package request"), "{hint}");
    let hint = ask_refused(&ws, "compute.packages", json!({ "request": "car_abc123" }));
    assert!(hint.contains("not a package request"), "{hint}");

    set_operate(&ws, true);
    let open = file_pkg(&ws, &node, "pending", OWN, "own");
    let hint = ask_refused(&ws, "compute.admin-approve", json!({ "request": open }));
    assert!(hint.contains(&format!("k2 permission request compute.packages request={open}")), "{hint}");

    let _off = ComputeOn::set(false);
    let hint = ask_refused(&ws, "compute.packages", json!({ "request": open }));
    assert!(hint.contains("compute nodes are off"), "{hint}");
}

/// Deny on the card denies the request itself, as the person, with the
/// reason. Allow while the node is offline approves the card, leaves the
/// request waiting in Settings, and says so.
#[test]
fn the_package_cards_allow_and_deny_decide_the_request() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("pkgd");
    let (node, name) = list_node(&ws, "pk-d", true);
    let admin = login("pkga", Role::Admin);

    let id = file_pkg(&ws, &node, "pending", k2_core::compute::pkgreq::OWN, "own");
    let v = ask_ok(&ws, "compute.packages", json!({ "request": id }));
    let member = login("pkgm", Role::Member);
    below_floor(answer_as(&member, &ws, &card_of(&v), json!({ "answer": "Allow" })), "admin");
    let (s, a) = answer_as(&admin, &ws, &card_of(&v), json!({ "answer": "Deny", "reason": "not on this box" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Denied);
    let rows = pkg_rows(&id);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        (rows[0].state.as_str(), rows[0].decided_by.clone(), rows[0].note.as_deref()),
        ("denied", Some(format!("login:{admin}")), Some("not on this box")),
        "{rows:?}"
    );

    // Allow with the node offline: nothing is sent, the request still waits.
    let id = file_pkg(&ws, &node, "pending", k2_core::compute::pkgreq::OWN, "own");
    let v = ask_ok(&ws, "compute.packages", json!({ "request": id }));
    let (s, a) = answer_as(&admin, &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Approved);
    assert_eq!(pkg_rows(&id).iter().map(|r| r.state.clone()).collect::<Vec<_>>(), ["pending"]);
    let turn = last_turn(&ws);
    assert!(turn.contains(&format!("{name} is offline, so it waits there")), "{turn}");
}

/// A package card batches only with other package cards, never under an
/// unrelated "Allow all", in either order.
#[test]
fn package_cards_batch_only_with_each_other() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::compute::pkgreq::OWN;
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("pkgb");
    let (node, _) = list_node(&ws, "pk-b", true);
    let (a, _) = list_node(&ws, "pk-b2", true);
    let (_, b_name) = list_node(&ws, "pk-b3", true);
    set_list(&ws, Some(std::slice::from_ref(&a)));
    let other = ask_ok(&ws, "compute.allow-nodes", json!({ "nodes": b_name }));
    let first = ask_ok(&ws, "compute.packages", json!({ "request": file_pkg(&ws, &node, "pending", OWN, "own") }));
    assert_ne!(card_of(&first), card_of(&other), "a package card never joins an unrelated card");
    let second = ask_ok(&ws, "compute.packages", json!({ "request": file_pkg(&ws, &node, "pending", OWN, "own") }));
    assert_eq!(card_of(&second), card_of(&first), "package cards batch with each other");

    let ws2 = seed("pkgc");
    let (node2, _) = list_node(&ws2, "pk-c", true);
    let (c, _) = list_node(&ws2, "pk-c2", true);
    let (_, d_name) = list_node(&ws2, "pk-c3", true);
    set_list(&ws2, Some(std::slice::from_ref(&c)));
    let pkg = ask_ok(&ws2, "compute.packages", json!({ "request": file_pkg(&ws2, &node2, "pending", OWN, "own") }));
    let later = ask_ok(&ws2, "compute.allow-nodes", json!({ "nodes": d_name }));
    assert_ne!(card_of(&later), card_of(&pkg), "an unrelated card never joins a package card");
}

/// The policy card checks its node and level: an unknown node, a node
/// whose k2-node has no package policy yet, or an unknown level is refused.
#[test]
fn the_package_policy_card_checks_its_node() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("pkgp");
    let (_node, name) = list_node(&ws, "pk-p", true);
    let hint = ask_refused(&ws, "compute.packages-policy", json!({ "node": name, "server": "own", "level": "official" }));
    assert!(hint.contains("has no package policy yet"), "{hint}");
    let hint = ask_refused(&ws, "compute.packages-policy", json!({ "node": "no-such-node", "server": "own", "level": "list" }));
    assert!(hint.contains("no active compute node"), "{hint}");
    let (s, v) = ask(&ws, "compute.packages-policy", json!({ "node": name, "server": "own", "level": "everything" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("list, profiles, official"), "{v}");
}

// ── compute.git-access-setup (0.46.10) ───────────────────────────────

/// This server's GitHub credential row, as `compute_git::setup` writes it
/// after K2's own form posted the key (the vault half is the integration
/// test's: `tests/permission_compute_git_setup.rs`).
fn set_git_credential(on: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if on {
        k2_core::compute::git::set_credential(&conn, "app", Some("4242"), "owner", k2_core::compute::now()).expect("credential");
    } else {
        k2_core::compute::git::clear_credential(&conn).expect("clear");
    }
}

const SETUP_ASK: &str = "k2 permission request compute.git-access-setup --why \"<what you need it for>\"";

/// The setup card is a person's (the owner or an Admin), carries no value
/// (the credential goes from K2's form to the setup route, never through
/// the card), is read on its own, and says what the credential needs.
#[test]
fn git_access_setup_is_a_persons_card_with_no_value_on_it() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::permission::catalog::{Floor, Proof};
    let r = super::super::find("compute.git-access-setup").expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof), (Floor::Admin, Proof::Click));
    assert!(r.spec.params.is_empty(), "the card has no param that could carry a key or a token");
    assert!(!crate::permission::present::batchable(&r), "never under Allow all");
    assert!(crate::permission::actions::box_login_grant::never_batched(r.spec.id), "nothing joins it");
    assert!(crate::routes::route_policy::lookup(crate::compute_git::SETUP).and_then(|x| x.post).is_some(), "the form's route has a POST row");

    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("gsu");
    set_git_credential(false);
    // A value can't ride the ask.
    for params in [json!({ "token": "github_pat_11K2TESTnotreal" }), json!({ "app_id": "4242", "key_pem": "x" })] {
        let (s, v) = ask(&ws, "compute.git-access-setup", params.clone());
        assert_eq!(s, "400 Bad Request", "{params}: {v}");
    }
    let v = ask_ok(&ws, "compute.git-access-setup", json!({}));
    let id = id_of(&v);
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], "GitHub access for compute jobs", "{vw}");
    // 0.46.17: the card says it has K2's own form before anyone taps Allow.
    assert_eq!(vw["form"], "git-access-setup", "{vw}");
    assert!(vw["whatChanges"].as_str().unwrap_or("").contains("K2's own form"), "{vw}");
    let cons = vw["consequences"].as_str().unwrap_or_else(|| panic!("no consequences: {vw}"));
    assert!(cons.contains("Contents read-only") && cons.contains(&format!("{} never sees it", ws.handle)), "{cons}");
    assert!(!vw.to_string().contains('\u{2014}'), "no em dashes on the card: {vw}");
    assert_eq!(vw["settingsPage"], "compute");

    // Below the floor: a Member.
    let member = login("gsum", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");

    // An Admin's Allow before the form set anything: refused, the card
    // stays open for the form.
    let admin = login("gsua", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("409 Conflict", &json!("git_access_setup_form_first")), "{a}");
    assert!(a["error"]["hint"].as_str().unwrap_or("").contains("k2 compute git-access setup --github-app"), "{a}");
    assert!(row(&id).is_answerable(), "nothing decided");
    // `apply` itself never sets anything.
    assert_eq!(refused_code(apply_as(&id, &Answerer::OwnerToken, None)), "git_access_setup_form_first");

    // The form set the credential: Allow closes the card, "already on",
    // and the agent is told to run its command again.
    set_git_credential(true);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("already_done")), "{a}");
    assert_eq!(row(&id).state, RequestState::AlreadyDone);
    let turn = last_turn(&ws);
    assert!(turn.contains("Run your command again: the grant is a second card"), "{turn}");
    // A new ask has nothing to ask.
    let (s, v) = ask(&ws, "compute.git-access-setup", json!({}));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
    set_git_credential(false);
}

/// The grant card while the server has no credential: not asked (409,
/// with the exact setup line), and one filed earlier goes stale at the
/// click with that line in the agent's turn.
#[test]
fn the_grant_card_points_at_setup_while_there_is_no_credential() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("gsg");
    set_git_credential(false);
    let (s, v) = ask(&ws, "compute.git-access", json!({ "repos": "acme/app" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("git_access_needs_setup")), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap_or_else(|| panic!("no hint: {v}"));
    assert!(hint.ends_with(SETUP_ASK), "{hint}");

    // Set: the grant is asked as before.
    set_git_credential(true);
    let v = ask_ok(&ws, "compute.git-access", json!({ "repos": "acme/app" }));
    // Cleared before anyone answered (a grant card waiting on a server
    // with no credential): stale at the click.
    set_git_credential(false);
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id_of(&v)).state, RequestState::Stale);
    let turn = last_turn(&ws);
    assert!(turn.contains("stale: this server has no GitHub credential yet") && turn.contains(SETUP_ASK), "{turn}");
    let granted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::compute::git::grants_of(&conn, &ws.id)
    };
    assert!(granted.is_empty(), "nothing granted: {granted:?}");
}

// ── compute.grant (0.46.17) ──────────────────────────────────────────

fn set_compute_switch(ws: &Ws, on: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::set_agents_can_use_compute(&conn, &ws.id, on).expect("the compute switch");
}

/// The limits `k2 compute grant` takes, read the same way on the card.
#[test]
fn compute_grant_reads_the_limits_as_the_cli_takes_them() {
    use super::{hours_words, parse_duration};
    for (raw, secs) in [("4h", Some(14_400)), ("90m", Some(5_400)), ("1h30m", Some(5_400)), ("300s", Some(300)), ("7200", Some(7_200)), ("2h0m", Some(7_200)), ("4", Some(4))] {
        assert_eq!(parse_duration(raw), secs, "{raw}");
    }
    for raw in ["", "h", "4x", "4 hours", "1.5h", "4h5", "-4h", "4H"] {
        assert_eq!(parse_duration(raw), None, "{raw}");
    }
    for (secs, words) in [(14_400, "4 hours"), (3_600, "1 hour"), (5_400, "1 hour 30 minutes"), (90, "1 minute 30 seconds"), (60, "1 minute")] {
        assert_eq!(hours_words(secs), words, "{secs}");
    }
}

/// A person's card (owner or Admin, one click), its words in plain terms:
/// each asked limit with what it is now, the limits that stay, and what a
/// new max-parallel or a longer job does to the node's other jobs.
#[test]
fn compute_grant_card_names_each_limit_now_and_asked() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::permission::catalog::{Direction, Floor, Proof};
    let r = super::super::find("compute.grant").expect("compute.grant registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.direction, r.spec.proof), (Floor::Admin, Direction::Loosen, Proof::Click));
    assert!(crate::routes::route_policy::lookup("/cli/compute/grants/set").and_then(|x| x.post).is_some(), "the route the card calls");
    assert!(super::super::not_cardable().iter().all(|n| n.id != "compute.grant"), "a card now, not a not-cardable row");

    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("cgw");
    let (_, name) = list_node(&ws, "cg-a", true);
    let h = ws.handle.clone();

    let v = ask_ok(&ws, "compute.grant", json!({ "node": name, "max-time": "4h" }));
    let vw = view(&ws, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Let {h} run jobs of up to 4 hours on {name} (now 2 hours)."), "{vw}");
    assert_eq!(
        vw["consequences"],
        format!(
            "Its other limits on {name} stay as they are: one job at a time, up to 10 jobs waiting and 60 GB of disk per job. Other jobs on {name} may wait longer for a slot."
        ),
        "{vw}"
    );
    assert_eq!(vw["setting"], "Compute job limits", "{vw}");
    assert!(vw.get("form").is_none(), "a plain card has no form: {vw}");
    assert_eq!(vw["scope"], format!("{h} on {name}"), "{vw}");
    assert_eq!(vw["approvers"]["floor"], "admin", "{vw}");
    assert_eq!(vw["direction"], "loosen", "{vw}");
    // A Member login is below the card's floor.
    let member = login("cgm", Role::Member);
    below_floor(answer_as(&member, &ws, &card_of(&v), json!({ "answer": "Allow" })), "admin");

    // Several limits at once (another workspace: its own card).
    let ws2 = seed("cgv");
    let (_, name2) = list_node(&ws2, "cg-b", true);
    let h2 = ws2.handle.clone();
    let v = ask_ok(&ws2, "compute.grant", json!({ "node": name2, "max-parallel": "2", "max-queued": "20", "max-disk": "120G" }));
    let vw = view(&ws2, &card_of(&v));
    assert_eq!(
        vw["whatChanges"],
        format!("Let {h2} run 2 jobs at a time (now 1), keep up to 20 jobs waiting (now 10) and use up to 120 GB of disk per job (now 60 GB) on {name2}."),
        "{vw}"
    );
    assert_eq!(
        vw["consequences"],
        format!(
            "Its other limits on {name2} stay as they are: jobs of up to 2 hours. Its jobs already waiting on {name2} are sent again with the new limit, in the same order. Other jobs on {name2} may wait longer for a slot."
        ),
        "{vw}"
    );

    // Lowering a limit: no "wait longer" line.
    let ws3 = seed("cgl");
    let (_, name3) = list_node(&ws3, "cg-c", true);
    let v = ask_ok(&ws3, "compute.grant", json!({ "node": name3, "max-time": "1h" }));
    let vw = view(&ws3, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Let {} run jobs of up to 1 hour on {name3} (now 2 hours).", ws3.handle), "{vw}");
    assert!(!vw["consequences"].as_str().unwrap_or("").contains("wait longer"), "{vw}");
}

/// Only its own workspace's grant, only limits, only a node it holds a
/// grant on (or any active node while its compute switch is on): every
/// other ask is refused before a card. Asking for what it has already is
/// "already done".
#[test]
fn compute_grant_asks_only_for_its_own_grant_and_its_limits() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("cgo");
    let other = seed("cgp");
    let (_, mine) = list_node(&ws, "cg-m", true);
    let (theirs_id, theirs) = list_node(&other, "cg-t", true);

    // Never a workspace (its own or another's), never CI or GPU.
    for params in [json!({ "node": mine, "max-time": "4h", "workspace": other.id }), json!({ "node": mine, "max-time": "4h", "workspace": ws.id })] {
        let (s, v) = ask(&ws, "compute.grant", params.clone());
        assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{params}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap_or("").contains("the workspace is always yours"), "{params}: {v}");
    }
    for params in [json!({ "node": mine, "max-time": "4h", "ci": "true" }), json!({ "node": mine, "gpu": "true" }), json!({ "node": mine, "allow-ci": "1" })] {
        let hint = ask_refused(&ws, "compute.grant", params.clone());
        assert!(hint.contains("compute.grant takes node=, max-time=, max-parallel=, max-queued=, max-disk="), "{params}: {hint}");
    }
    // A node it holds no grant on, its compute switch off: ask for that first.
    let hint = ask_refused(&ws, "compute.grant", json!({ "node": theirs, "max-time": "4h" }));
    assert!(hint.contains(&format!("has no grant on '{theirs}'")) && hint.contains("compute.use"), "{hint}");
    assert!(ask_refused(&ws, "compute.grant", json!({ "node": "no-such-node", "max-time": "4h" })).contains("no active compute node 'no-such-node'"));
    // A limit, and only a good one.
    assert!(ask_refused(&ws, "compute.grant", json!({ "node": mine })).contains("name at least one limit"));
    for (key, raw) in [
        ("max-time", "4x"),
        ("max-time", "30s"),
        ("max-time", "200h"),
        ("max-parallel", "0"),
        ("max-parallel", "65"),
        ("max-parallel", "two"),
        ("max-queued", "1001"),
        ("max-disk", "0"),
        ("max-disk", "10001G"),
        ("max-disk", "1T"),
    ] {
        let hint = ask_refused(&ws, "compute.grant", json!({ "node": mine, key: raw }));
        assert!(hint.starts_with(&format!("{key}={raw} is not ")), "{key}={raw}: {hint}");
    }
    let stored: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1", [&ws.id], |r| r.get(0)).expect("count")
    };
    assert_eq!(stored, 0, "a refused ask stores no request and makes no card");

    // What it has already: nothing to ask (2h, one at a time, 10, 60 GB).
    for params in [json!({ "node": mine, "max-time": "2h" }), json!({ "node": mine, "max-time": "7200", "max-parallel": "1", "max-disk": "60G" })] {
        let (s, v) = ask(&ws, "compute.grant", params.clone());
        assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{params}: {v}");
    }

    // Its compute switch on: any active node may be asked for; the card
    // says it gives a grant there too. The other workspace's grant is not
    // what it reads.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let tight = k2_core::compute::store::GrantLimits { max_job_secs: Some(600), ..Default::default() };
        k2_core::compute::store::set_grant(&conn, &theirs_id, &other.id, &tight, "owner", 8).expect("their grant");
    }
    set_compute_switch(&ws, true);
    let v = ask_ok(&ws, "compute.grant", json!({ "node": theirs, "max-time": "4h" }));
    let vw = view(&ws, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Let {} run jobs of up to 4 hours on {theirs} (now 2 hours).", ws.handle), "{vw}");
    assert!(
        vw["consequences"].as_str().unwrap_or("").starts_with(&format!("{} has no grant on {theirs} yet: this gives it one", ws.handle)),
        "{vw}"
    );

    // Compute off: nothing asks.
    let _off = ComputeOn::set(false);
    assert!(ask_refused(&ws, "compute.grant", json!({ "node": mine, "max-time": "4h" })).contains("compute nodes are off"));
}

/// The grant changed under a waiting card (the owner ran k2 compute grant
/// meanwhile): what the person read is no longer true, so the click is
/// stale and applies nothing.
#[test]
fn compute_grant_card_goes_stale_when_the_grant_moves() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("cgs");
    let (node_id, name) = list_node(&ws, "cg-s", true);
    let v = ask_ok(&ws, "compute.grant", json!({ "node": name, "max-time": "4h" }));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let l = k2_core::compute::store::GrantLimits { max_job_secs: Some(3 * 3600), ..Default::default() };
        k2_core::compute::store::set_grant(&conn, &node_id, &ws.id, &l, "owner", 9).expect("grant moved");
    }
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id_of(&v)).state, RequestState::Stale);
    let g = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::compute::store::grant(&conn, &node_id, &ws.id).expect("grant")
    };
    assert_eq!(g.max_job_secs, 3 * 3600, "nothing applied");
}

// ── compute.node.pinned-only (0.46.18) ───────────────────────────────

fn pinned(node_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::pinned_only::is_on(&conn, node_id)
}

/// The node's `pinned_only` compute events: `(actor, detail)`, oldest first.
fn pinned_events(node_id: &str) -> Vec<(String, Value)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn
        .prepare("SELECT actor, detail_json FROM compute_events WHERE kind = 'pinned_only' AND node_id = ?1 ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, String)> =
        st.query_map([node_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).expect("query").map(|x| x.expect("row")).collect();
    rows.into_iter().map(|(a, d)| (a, serde_json::from_str(&d).expect("detail json"))).collect()
}

/// The card's Access Audit lines for one node.
fn pinned_audit(node_name: &str) -> Vec<Value> {
    let needle = format!("node={node_name} ");
    k2_core::auth_audit::read_all()
        .expect("audit")
        .into_iter()
        .filter(|e| e["event"] == crate::compute_node_flags::PINNED_ONLY_AUDIT && e["outcome"].as_str().unwrap_or("").contains(&needle))
        .collect()
}

/// An Admin card (one click) whose words say the node, the change and what
/// it does; refused values never make a card; asking for what the node
/// already is is "already done"; compute off, nothing asks.
#[test]
fn pinned_only_card_names_the_node_and_the_change() {
    let _db = k2_core::db::scoped_for_test();
    use k2_core::permission::catalog::{Floor, Grant, Proof};
    let r = super::super::find("compute.node.pinned-only").expect("compute.node.pinned-only registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::Click, Grant::OneTime));
    assert!(crate::routes::route_policy::lookup(super::PINNED_ONLY_ROUTE).and_then(|x| x.post).is_some(), "the route the card stands in for");
    assert!(super::super::not_cardable().iter().all(|n| n.id != "compute.node.pinned-only"), "a card, not a not-cardable row");

    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("pinw");
    let (node_id, name) = list_node(&ws, "pin-a", false);

    // Off already: asking for off is done; bad values never make a card.
    let (s, v) = ask(&ws, "compute.node.pinned-only", json!({ "node": name, "on": "false" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
    let hint = ask_refused(&ws, "compute.node.pinned-only", json!({ "node": name, "on": "yes" }));
    assert!(hint.contains("on= is one of true, false"), "{hint}");
    let hint = ask_refused(&ws, "compute.node.pinned-only", json!({ "node": name }));
    assert!(hint.contains("needs on="), "{hint}");
    let hint = ask_refused(&ws, "compute.node.pinned-only", json!({ "node": "no-such-node", "on": "true" }));
    assert!(hint.contains("no compute node 'no-such-node'"), "{hint}");

    let v = ask_ok(&ws, "compute.node.pinned-only", json!({ "node": name, "on": "true" }));
    let vw = view(&ws, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Turn on pinned jobs only for {name}."), "{vw}");
    assert_eq!(
        vw["consequences"],
        format!(
            "Automatic placement stops choosing {name}: a job runs there only when it names {name}. This is for every workspace on this server, not only {}. Jobs already queued or running on {name} are not moved. To turn it off again: k2 compute node pinned-only {name} off (the owner or an admin).",
            ws.handle
        ),
        "{vw}"
    );
    assert_eq!(vw["setting"], "Pinned jobs only", "{vw}");
    assert_eq!(vw["scope"], format!("{name}, for every workspace on this server"), "{vw}");
    assert_eq!(vw["approvers"]["floor"], "admin", "{vw}");
    // The asked values, then one row with the node's facts.
    let values = values_of(&vw);
    let keys: Vec<&str> = values.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["workspace", "node", "on", name.as_str()], "{vw}");
    assert_eq!(values[2].1, "true", "{vw}");
    assert_eq!(values[3].1, "linux · size not reported yet · owned by this server", "{vw}");
    // A Member is below the card's floor; nothing changed.
    let member = login("pinm", Role::Member);
    below_floor(answer_as(&member, &ws, &card_of(&v), json!({ "answer": "Allow" })), "admin");
    assert!(!pinned(&node_id));

    // The off card, its own words, the node by its name though asked by
    // id (another workspace: its own card).
    let ws2 = seed("pinx");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::compute::pinned_only::set(&conn, &node_id, true, "owner", 9).expect("on");
    }
    let v = ask_ok(&ws2, "compute.node.pinned-only", json!({ "node": node_id, "on": "false" }));
    let vw = view(&ws2, &card_of(&v));
    assert_eq!(vw["whatChanges"], format!("Turn off pinned jobs only for {name}."), "{vw}");
    assert!(vw["consequences"].as_str().unwrap_or("").starts_with(&format!("Automatic placement may choose {name} again")), "{vw}");

    // Compute off: nothing asks.
    let _off = ComputeOn::set(false);
    assert!(ask_refused(&ws, "compute.node.pinned-only", json!({ "node": name, "on": "false" })).contains("compute nodes are off"));
}

/// The agent can't flip it itself (the route refuses it, a compute manager
/// too); Deny changes nothing; an Admin's Allow flips it as that Admin,
/// audited with who asked and `via=permission-card`; the owner's Allow
/// turns it back off.
#[test]
fn pinned_only_allow_flips_the_node_as_the_person_and_deny_changes_nothing() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _on = ComputeOn::set(true);
    let ws = seed("pind");
    let ws2 = seed("pina");
    let (node_id, name) = list_node(&ws, "pin-b", false);

    // The agent itself: refused, a compute manager too; nothing changes.
    for manage in [false, true] {
        set_manage(&ws, manage);
        let e = crate::compute_node_flags::node_pinned_only(&agent(&ws), json!({ "node": name, "on": true }).to_string().as_bytes())
            .expect_err("an agent never flips it");
        assert_eq!((e.status, e.code), (403, "owner_only"), "manage={manage}");
    }
    set_manage(&ws, false);
    assert!(!pinned(&node_id));
    assert!(pinned_events(&node_id).is_empty());

    // Deny: nothing changes, nothing is recorded.
    let admin = login("pinadm", Role::Admin);
    let v = ask_ok(&ws, "compute.node.pinned-only", json!({ "node": name, "on": "true" }));
    let (s, a) = answer_as(&admin, &ws, &card_of(&v), json!({ "answer": "Deny", "reason": "not this one" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id_of(&v)).state, RequestState::Denied);
    assert!(!pinned(&node_id), "deny changes nothing");
    assert!(pinned_events(&node_id).is_empty() && pinned_audit(&name).is_empty(), "deny records no change");

    // Allow (an Admin): on, as that Admin, with who asked.
    let v = ask_ok(&ws2, "compute.node.pinned-only", json!({ "node": name, "on": "true" }));
    let (s, a) = answer_as(&admin, &ws2, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(row(&id_of(&v)).state, RequestState::Approved);
    assert!(pinned(&node_id), "allow turned it on");
    let asked_by = format!("agent:{}", ws2.handle);
    assert_eq!(pinned_events(&node_id), vec![(format!("login:{admin}"), json!({ "on": true, "askedBy": asked_by, "via": "permission-card" }))]);
    let audit = pinned_audit(&name);
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["user"], format!("login:{admin}"), "{audit:?}");
    assert_eq!(audit[0]["outcome"], format!("node={name} on=true changed=true askedBy={asked_by} via=permission-card"), "{audit:?}");
    let turn = last_turn(&ws2);
    assert!(turn.contains(&format!("{name} is pinned jobs only now")), "{turn}");
    // Asked again: already done.
    let (s, v) = ask(&ws2, "compute.node.pinned-only", json!({ "node": name, "on": "true" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");

    // The owner's Allow turns it back off.
    let v = ask_ok(&ws2, "compute.node.pinned-only", json!({ "node": name, "on": "false" }));
    let (s, a) = answer_as("owner", &ws2, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert!(!pinned(&node_id), "allow turned it off");
    let last = pinned_events(&node_id).pop().expect("an event");
    assert_eq!((last.0.as_str(), &last.1["on"], &last.1["via"]), ("owner", &json!(false), &json!("permission-card")));
}
