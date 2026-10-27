//! Node admin controller tests (prd-k2-node-admin-v1 §16 "Controller").
//! Fakes only: a scoped in-memory DB, a temp vault with a fake key store
//! (never the keychain, never a real Touch ID sheet), a generated
//! controller key (never `~/.k2/tunnel-key.pem`), and a fake node session.
//! Every emitted command is checked with the node side's own verifier
//! (`k2_node_proto::admin::{open, check_target_and_time, verify_owner}`).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use k2_core::compute::admin as cadmin;
use k2_core::compute::proto::admin as wire;
use k2_core::compute::proto::frames::{self, Frame};
use k2_core::compute::proto::health::{DiskFact, ExecutorFact, Health};
use k2_core::connect_users::Role;
use serde_json::{json, Value};

use super::test_seams;
use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    node: String,
    node_name: String,
    ws: String,
}

fn fx(tag: &str, operate: bool) -> Fx {
    let db = k2_core::db::scoped_for_test();
    let node = format!("node-{tag}-{}", crypto::random_id().unwrap());
    let node_name = format!("mini-{tag}");
    let ws = format!("ws-{tag}-{}", crypto::random_id().unwrap());
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let labels = BTreeMap::from([("os".to_string(), "macos".to_string()), ("arch".to_string(), "aarch64".to_string())]);
        store::insert_pending_node(&conn, &node, &node_name, &fp_of(&node), "pem", &labels, "owner", "123456", 5).unwrap();
        store::confirm_node(&conn, &node, "owner", 6).unwrap();
        store::touch_seen(&conn, &node, "lan", 2, "0.45.5", compute::now());
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![ws, format!("it-{tag}"), format!("/tmp/k2-admin-test/{ws}")],
        )
        .unwrap();
        if operate {
            cadmin::set_agents_can_operate_compute(&conn, &ws, true).unwrap();
        }
    }
    test_seams::register(&node, true, &[wire::FEATURE_ADMIN, wire::FEATURE_HEALTH, wire::FEATURE_SELF_UPDATE]);
    Fx { _db: db, node, node_name, ws }
}

/// A 64-hex node fingerprint per node (the wire checks the shape only on
/// the node; keep it realistic).
fn fp_of(node: &str) -> String {
    crypto::sha256_hex(node.as_bytes())
}

/// Another workspace in the SAME scoped DB (a second `fx` would swap the
/// thread's DB out from under the first).
fn other_ws(tag: &str, operate: bool) -> String {
    let ws = format!("ws-{tag}-{}", crypto::random_id().unwrap());
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![ws, format!("it-{tag}"), format!("/tmp/k2-admin-test/{ws}")],
    )
    .unwrap();
    if operate {
        cadmin::set_agents_can_operate_compute(&conn, &ws, true).unwrap();
    }
    ws
}

fn agent(f: &Fx) -> Who {
    agent_of(&f.ws)
}

fn agent_of(ws: &str) -> Who {
    Who::Agent { project_id: ws.to_string(), session_id: "sess-1".into() }
}

fn call(path: &str, who: &Who, body: Value) -> Result<Value, (u16, String)> {
    handle(path, who, body.to_string().as_bytes(), &HashMap::new(), None).map_err(|e| (e.status, e.code.to_string()))
}

fn get(path: &str, who: &Who, params: &[(&str, &str)]) -> Result<Value, (u16, String)> {
    let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    handle(path, who, b"", &p, None).map_err(|e| (e.status, e.code.to_string()))
}

fn token_for(req: &str) -> String {
    test_seams::vault_handle().confirm.mint(cadmin::CONFIRM_ACTION, Some(req), "owner", "local-auth")
}

fn propose_restart(f: &Fx) -> String {
    let r = call(PROPOSE, &agent(f), json!({"node": f.node_name, "verb": "unit.restart", "args": {"unit": "k2-node"}, "reason": "wedged"})).unwrap();
    assert_eq!(r["held"], true);
    assert_eq!(r["request"]["state"], "pending");
    assert_eq!(r["request"]["class"], "privileged");
    r["request"]["id"].as_str().unwrap().to_string()
}

fn controller() -> crypto::SigningKey {
    crypto::SigningKey::from_pkcs8_pem(&test_seams::key_pem()).unwrap()
}

fn admin_cmds(frames: Vec<Frame>) -> Vec<wire::SignedCommand> {
    frames
        .into_iter()
        .filter_map(|f| match f {
            Frame::Admin(a) => Some(a.cmd),
            _ => None,
        })
        .collect()
}

/// The node side's checks, as the root executor runs them.
fn node_open(f: &Fx, c: &wire::SignedCommand) -> Result<wire::Opened, wire::AdminRefusal> {
    let k = controller();
    let o = wire::open(c.to_json().unwrap().as_bytes(), k.spki_der())?;
    wire::check_target_and_time(&o.cmd, &fp_of(&f.node), &k.fingerprint(), compute::now())?;
    if o.cmd.class == wire::Class::Privileged {
        let pk = wire::ApprovalKey(approval_public_key().unwrap());
        wire::verify_owner(&o, &[pk], compute::now())?;
    }
    Ok(o)
}

fn result(cmd_id: &str, state: &str, code: Option<&str>, exit: Option<i32>, out: Value) -> wire::AdminResult {
    wire::AdminResult {
        cmd_id: cmd_id.into(),
        state: state.into(),
        code: code.map(str::to_string),
        message: None,
        exit,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        audit_seq: Some(7),
        digest: "sha256:x".into(),
        result: out,
    }
}

/// A fake node that answers `policy.show` / `policy.set` from another
/// thread (the route blocks waiting for it). Returns the verbs it saw.
fn answer_policy(node: String, policy: Value, refuse_set: bool) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            for c in admin_cmds(test_seams::take_sent(&node)) {
                let body = crypto::unb64(&c.body_b64).unwrap();
                let cmd: wire::Command = serde_json::from_slice(&body).unwrap();
                seen.push(cmd.verb.clone());
                let r = match (cmd.verb.as_str(), refuse_set) {
                    ("policy.set", true) => result(&cmd.cmd_id, "refused", Some(wire::code::APPROVAL_REQUIRED), None, Value::Null),
                    ("policy.set", false) => result(&cmd.cmd_id, "done", None, Some(0), json!({"policy": cmd.args, "widened": []})),
                    _ => result(&cmd.cmd_id, "done", None, Some(0), json!({"policy": policy})),
                };
                // Waiters are matched by cmd_id before any DB access.
                assert!(super::wait_deliver(&r), "a route waits for {}", cmd.verb);
                return seen;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        seen
    })
}

#[test]
fn agent_proposes_owner_approves_and_a_signed_command_goes_out() {
    let f = fx("happy", true);
    let id = propose_restart(&f);
    assert!(test_seams::take_sent(&f.node).is_empty(), "nothing is sent while held");

    // No passport, no Admin login, no K2 shell tab can approve.
    let shell = Who::Shell { project_id: f.ws.clone(), session_id: "tab".into() };
    let admin_login = Who::Login { username: "lee".into(), role: Role::Admin };
    for who in [agent(&f), shell, admin_login] {
        let e = call(APPROVE, &who, json!({"request": id, "confirmToken": token_for(&id)})).unwrap_err();
        assert_eq!(e, (403, "owner_only".to_string()), "{who:?}");
    }

    // The owner token alone isn't enough (AR14).
    let e = call(APPROVE, &Who::Owner, json!({"request": id})).unwrap_err();
    assert_eq!(e, (403, cadmin::codes::VAULT_CONFIRM_REQUIRED.to_string()));
    // A confirm minted for another request is refused (and burned).
    let other = token_for("car_someone_else");
    let e = call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": other})).unwrap_err();
    assert_eq!(e.1, cadmin::codes::VAULT_CONFIRM_REQUIRED);
    // A confirm for another action is refused.
    let wrong_action = test_seams::vault_handle().confirm.mint("reveal", Some(&id), "owner", "local-auth");
    assert_eq!(call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": wrong_action})).unwrap_err().1, cadmin::codes::VAULT_CONFIRM_REQUIRED);

    let r = call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": token_for(&id)})).unwrap();
    assert_eq!(r["sent"], true, "{r}");
    assert_eq!(r["request"]["state"], "sent");
    assert_eq!(r["request"]["decidedVia"], "local-auth");

    let cmds = admin_cmds(test_seams::take_sent(&f.node));
    assert_eq!(cmds.len(), 1);
    assert!(cmds[0].sig_o.is_some(), "privileged commands carry the owner signature");
    let o = node_open(&f, &cmds[0]).expect("the node accepts it");
    assert_eq!(o.cmd.verb, "unit.restart");
    assert_eq!(o.cmd.class, wire::Class::Privileged);
    assert_eq!(o.cmd.request_id.as_deref(), Some(id.as_str()));
    assert_eq!(o.cmd.approved_by.as_deref(), Some("owner"));
    assert_eq!(o.cmd.proposed_by, format!("agent:{}", crate::compute_routes::ws_handle(&f.ws)));
    // Stripped of the owner signature, the node refuses it.
    let mut stripped = cmds[0].clone();
    stripped.sig_o = None;
    assert_eq!(node_open(&f, &stripped).unwrap_err().code, wire::code::APPROVAL_REQUIRED);

    // A replayed approval finds nothing to approve; nothing more is sent.
    let e = call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": token_for(&id)})).unwrap_err();
    assert_eq!(e, (409, cadmin::codes::NOT_PENDING.to_string()));
    assert!(test_seams::take_sent(&f.node).is_empty());

    // The node reports; the request finishes and the proposer hears both.
    on_admin_result(&f.node, result(&o.cmd.cmd_id, "done", None, Some(0), json!({})));
    let list = get(REQUESTS, &Who::Owner, &[("state", "all")]).unwrap();
    let row = list["requests"].as_array().unwrap().iter().find(|x| x["id"] == id.as_str()).cloned().unwrap();
    assert_eq!(row["state"], "done");
    assert_eq!(row["result"]["auditSeq"], 7);
    let msgs = test_seams::take_messages(&f.ws);
    assert!(msgs.iter().any(|m| m.contains("approved by the owner")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("done (exit 0)")), "{msgs:?}");

    // Audited: proposed, approved, sent, result.
    let kinds: Vec<String> = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::events(&conn, 50).into_iter().filter(|e| e.node_id.as_deref() == Some(&f.node)).map(|e| e.kind).collect()
    };
    for k in ["admin_proposed", "admin_approved", "admin_sent", "admin_result"] {
        assert!(kinds.iter().any(|x| x == k), "missing audit {k}: {kinds:?}");
    }
}

#[test]
fn expired_requests_are_refused() {
    let f = fx("expired", true);
    let id = propose_restart(&f);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE compute_admin_requests SET expires_at = ?2 WHERE id = ?1", rusqlite::params![id, compute::now() - 1]).unwrap();
    }
    let e = call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": token_for(&id)})).unwrap_err();
    assert_eq!(e, (410, cadmin::codes::APPROVAL_EXPIRED.to_string()));
    assert!(test_seams::take_sent(&f.node).is_empty());
    let list = get(REQUESTS, &Who::Owner, &[("state", "all")]).unwrap();
    assert_eq!(list["requests"][0]["state"], "expired");

    // Approved while offline, and the approval ran out before the node came back.
    let id2 = propose_restart(&f);
    test_seams::set_online(&f.node, false);
    let r = call(APPROVE, &Who::Owner, json!({"request": id2, "confirmToken": token_for(&id2)})).unwrap();
    assert_eq!(r["sent"], false);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        assert_eq!(cadmin::expire_unsent(&conn, compute::now() + wire::MAX_LIFETIME_SECS + wire::CLOCK_SKEW_SECS + 5).len(), 1);
    }
    test_seams::set_online(&f.node, true);
    pump(&f.node);
    assert!(test_seams::take_sent(&f.node).is_empty(), "an expired approval is never sent");
}

#[test]
fn approval_waits_for_an_offline_node_then_goes_out() {
    let f = fx("offline", true);
    let id = propose_restart(&f);
    test_seams::set_online(&f.node, false);
    let r = call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": token_for(&id)})).unwrap();
    assert_eq!(r["sent"], false);
    assert_eq!(r["request"]["state"], "approved");
    test_seams::set_online(&f.node, true);
    pump(&f.node);
    let cmds = admin_cmds(test_seams::take_sent(&f.node));
    assert_eq!(cmds.len(), 1);
    assert_eq!(node_open(&f, &cmds[0]).unwrap().cmd.request_id.as_deref(), Some(id.as_str()));
    pump(&f.node);
    assert!(test_seams::take_sent(&f.node).is_empty(), "sent once");
}

#[test]
fn passports_need_the_operate_switch_and_see_only_their_own() {
    let f = fx("switch", true);
    let off = other_ws("switchoff", false);
    let e = call(PROPOSE, &agent_of(&off), json!({"node": f.node_name, "verb": "unit.restart", "args": {"unit": "k2-node"}})).unwrap_err();
    assert_eq!(e, (403, cadmin::codes::OPERATE_OFF.to_string()));
    let e = call(NODE_POLICY_SET, &agent_of(&off), json!({"node": f.node_name, "set": {"mem_cap_gb": 1}})).unwrap_err();
    assert_eq!(e.1, cadmin::codes::OPERATE_OFF);

    let mine = propose_restart(&f);
    // The owner proposes too (no workspace).
    let owners = call(PROPOSE, &Who::Owner, json!({"node": f.node_name, "verb": "control.pause", "args": {}})).unwrap();
    assert_eq!(owners["request"]["class"], "operate");
    let owners = owners["request"]["id"].as_str().unwrap().to_string();
    let list = get(REQUESTS, &agent(&f), &[]).unwrap();
    let ids: Vec<&str> = list["requests"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![mine.as_str()], "an agent sees only its workspace's requests");
    assert_eq!(list["canApprove"], false);
    let all = get(REQUESTS, &Who::Owner, &[]).unwrap();
    assert_eq!(all["requests"].as_array().unwrap().len(), 2);
    assert_eq!(all["canApprove"], true);

    // Withdraw: only the proposer. Deny: humans only, never a passport.
    assert_eq!(call(WITHDRAW, &agent(&f), json!({"request": owners})).unwrap_err().0, 403);
    assert_eq!(call(DENY, &agent(&f), json!({"request": mine})).unwrap_err(), (403, "owner_only".to_string()));
    let d = call(DENY, &Who::Login { username: "lee".into(), role: Role::Admin }, json!({"request": owners, "why": "not now"})).unwrap();
    assert_eq!(d["request"]["state"], "denied");
    call(WITHDRAW, &agent(&f), json!({"request": mine})).unwrap();
    assert_eq!(call(WITHDRAW, &agent(&f), json!({"request": mine})).unwrap_err().1, cadmin::codes::NOT_PENDING);

    // Verbs the node doesn't know and bad args never become requests.
    for v in ["sudo", "script", "power.set"] {
        assert_eq!(call(PROPOSE, &agent(&f), json!({"node": f.node_name, "verb": v, "args": {}})).unwrap_err().1, cadmin::codes::VERB_NOT_ALLOWED, "{v}");
    }
    assert_eq!(
        call(PROPOSE, &agent(&f), json!({"node": f.node_name, "verb": "unit.restart", "args": {"unit": "sshd"}})).unwrap_err().1,
        cadmin::codes::BAD_ARGS
    );
    // Approval-key and alerts are humans only.
    assert_eq!(get(APPROVAL_KEY, &agent(&f), &[]).unwrap_err().0, 403);
    assert_eq!(call(NODE_ALERTS, &agent(&f), json!({"to": f.ws})).unwrap_err().0, 403);
    let k = get(APPROVAL_KEY, &Who::Owner, &[]).unwrap();
    assert!(k["keyId"].as_str().unwrap().starts_with("ed25519:"));
}

#[test]
fn a_plan_is_approved_once_and_runs_one_step_at_a_time() {
    let f = fx("plan", true);
    let r = call(
        PROPOSE,
        &agent(&f),
        json!({"node": f.node_name, "kind": "plan", "steps": [
            {"verb": "policy.set", "args": {"mem_cap_gb": 12}},
            {"verb": "unit.restart", "args": {"unit": "k2-node"}},
            {"verb": "control.resume", "args": {}}
        ]}),
    )
    .unwrap();
    let id = r["request"]["id"].as_str().unwrap().to_string();
    assert!(r["request"]["summary"].as_str().unwrap().starts_with("plan of 3 steps"));
    call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": token_for(&id)})).unwrap();
    let first = admin_cmds(test_seams::take_sent(&f.node));
    assert_eq!(first.len(), 1, "only step 1 is in flight");
    assert!(first[0].plan_b64.is_some(), "plan steps carry the approved plan");
    let c1 = node_open(&f, &first[0]).expect("the node accepts the plan step").cmd;
    assert_eq!(c1.plan.as_ref().map(|p| (p.step, p.of)), Some((0, 3)));
    on_admin_result(&f.node, result(&c1.cmd_id, "done", None, Some(0), json!({})));
    let second = admin_cmds(test_seams::take_sent(&f.node));
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].sig_o, first[0].sig_o, "one owner signature covers the plan");
    let c2 = node_open(&f, &second[0]).unwrap().cmd;
    assert_eq!((c2.verb.as_str(), c2.plan.as_ref().unwrap().step), ("unit.restart", 1));
    // A step changed after approval fails the node's plan check.
    let mut tampered: wire::Command = c2.clone();
    tampered.args = json!({"unit": "k2-node-packages"});
    let forged = wire::sign_controller(&controller(), &tampered).unwrap().with_owner_sig(
        &crypto::unb64(second[0].sig_o.as_deref().unwrap()).unwrap().try_into().unwrap(),
    );
    let forged = wire::SignedCommand { plan_b64: second[0].plan_b64.clone(), ..forged };
    assert_eq!(node_open(&f, &forged).unwrap_err().code, wire::code::PLAN_MISMATCH);

    on_admin_result(&f.node, result(&c2.cmd_id, "failed", Some("failed"), Some(1), Value::Null));
    assert!(test_seams::take_sent(&f.node).is_empty(), "a failed step stops the plan");
    let list = get(REQUESTS, &Who::Owner, &[("state", "all")]).unwrap();
    assert_eq!(list["requests"][0]["state"], "failed");
    assert_eq!(list["requests"][0]["result"]["step"], 2);
    // A result for a command that isn't in flight changes nothing.
    on_admin_result(&f.node, result(&c1.cmd_id, "done", None, Some(0), json!({})));
    assert!(test_seams::take_sent(&f.node).is_empty());
}

#[test]
fn policy_narrowing_goes_at_once_and_widening_is_held() {
    let f = fx("policy", true);
    // Show asks the node (policy.show, read) and caches the answer.
    let node = f.node.clone();
    let responder = answer_policy(node, json!({"mem_cap_gb": 8, "max_parallel": 2}), false);
    let shown = get(NODE_POLICY, &agent(&f), &[("node", &f.node_name)]).unwrap();
    assert_eq!(responder.join().unwrap(), vec!["policy.show"]);
    assert_eq!(shown["policy"]["mem_cap_gb"], 8);
    assert_eq!(shown["live"], true);

    // Lower: sent at once as an operate policy.set, applied, cache updated.
    let responder = answer_policy(f.node.clone(), Value::Null, false);
    let r = call(NODE_POLICY_SET, &agent(&f), json!({"node": f.node_name, "set": {"mem_cap_gb": 6}})).unwrap();
    assert_eq!(responder.join().unwrap(), vec!["policy.set"]);
    assert_eq!((r["class"].as_str(), r["note"].as_str()), (Some("operate"), Some("narrowing applied on the node")), "{r}");

    // Higher: held for the owner, nothing sent.
    let r = call(NODE_POLICY_SET, &agent(&f), json!({"node": f.node_name, "set": {"mem_cap_gb": 10}, "reason": "bigger builds"})).unwrap();
    assert_eq!(r["held"], true);
    assert_eq!(r["request"]["summary"], format!("change job limits on {}: mem_cap_gb=10", f.node_name));
    assert!(test_seams::take_sent(&f.node).is_empty(), "widening waits for the owner");

    // The node counts a change as widening that we didn't: held too.
    let responder = answer_policy(f.node.clone(), Value::Null, true);
    let r = call(NODE_POLICY_SET, &agent(&f), json!({"node": f.node_name, "set": {"mem_cap_gb": 4}})).unwrap();
    assert_eq!(responder.join().unwrap(), vec!["policy.set"]);
    assert_eq!(r["held"], true, "{r}");

    let e = call(NODE_POLICY_SET, &agent(&f), json!({"node": f.node_name, "set": {"rm": 1}})).unwrap_err();
    assert_eq!(e.1, cadmin::codes::BAD_ARGS);

    // A node without the admin feature: a narrowing is refused with a
    // teaching code, not silently skipped.
    test_seams::register(&f.node, true, &[]);
    let e = call(NODE_POLICY_SET, &agent(&f), json!({"node": f.node_name, "set": {"mem_cap_gb": 2}})).unwrap_err();
    assert_eq!(e, (409, cadmin::codes::NODE_LACKS_ADMIN.to_string()));
}

#[test]
fn a_node_without_the_controller_pinned_shows_needs_pin_until_it_answers() {
    let f = fx("pin", true);
    let mut r = result("admin_keys", "refused", Some(wire::code::NOT_CONFIGURED), None, Value::Null);
    r.message = Some("no controller key pinned".into());
    on_admin_result(&f.node, r);
    let h = get(NODE_HEALTH, &Who::Owner, &[("node", &f.node_name)]).unwrap();
    assert_eq!((h["needsPin"].as_bool(), h["pinCommand"].as_str()), (Some(true), Some("sudo k2-node admin pin")));
    // Any executor success clears it.
    on_admin_result(&f.node, result("admin_keys", "done", None, Some(0), Value::Null));
    let h = get(NODE_HEALTH, &Who::Owner, &[("node", &f.node_name)]).unwrap();
    assert_eq!(h["needsPin"], false);
    assert!(h["pinCommand"].is_null());
}

#[test]
fn the_approval_key_is_pinned_once_per_node() {
    let f = fx("keys", true);
    maybe_send_keys(&f.node);
    let sent = test_seams::take_sent(&f.node);
    let Some(Frame::AdminKeys(k)) = sent.into_iter().next() else { panic!("no admin_keys frame") };
    let ours = wire::ApprovalKey(approval_public_key().unwrap());
    assert_eq!(k.keys, vec![ours.encode()]);
    assert_eq!(k.node_fp, fp_of(&f.node));
    assert!(crypto::verify_domain(controller().spki_der(), wire::DOMAIN_KEYS_CONTROLLER, &[&k.signed_part().unwrap()], &k.sig_c));
    maybe_send_keys(&f.node);
    assert!(test_seams::take_sent(&f.node).is_empty(), "at most every 10 minutes");
}

#[test]
fn offline_alert_once_then_back_online() {
    let f = fx("alerts", true);
    call(NODE_ALERTS, &Who::Owner, json!({"to": f.ws})).unwrap();
    call(NODE_ALERTS, &Who::Owner, json!({"node": f.node_name, "after": 600})).unwrap();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let facts = Health { boot_id: "boot-1".into(), disk_encryption: Some("on".into()), ..Default::default() };
        cadmin::store_health(&conn, &f.node, compute::now(), &facts).unwrap();
        conn.execute("UPDATE compute_nodes SET last_seen = ?2 WHERE id = ?1", rusqlite::params![f.node, compute::now() - 1000]).unwrap();
    }
    // Online: no alert however old last_seen is.
    watch_pass();
    assert!(test_seams::take_messages(&f.ws).is_empty());

    test_seams::set_online(&f.node, false);
    watch_pass();
    let m = test_seams::take_messages(&f.ws);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains(&format!("{} offline since", f.node_name)), "{m:?}");
    assert!(m[0].contains("FileVault screen"), "{m:?}");
    watch_pass();
    assert!(test_seams::take_messages(&f.ws).is_empty(), "one offline alert per outage");

    test_seams::set_online(&f.node, true);
    on_attach(&f.node, "boot-2");
    let m = test_seams::take_messages(&f.ws);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains("back after") && m[0].contains("it rebooted"), "{m:?}");
    on_attach(&f.node, "boot-2");
    assert!(test_seams::take_messages(&f.ws).is_empty(), "back-online only after an offline alert");

    let h = get(NODE_HEALTH, &Who::Owner, &[("node", &f.node_name)]).unwrap();
    assert_eq!(h["alertAfterSecs"], 600);
    assert_eq!(h["alertsTo"], crate::compute_routes::ws_handle(&f.ws));
    assert_eq!(call(NODE_ALERTS, &Who::Owner, json!({"node": f.node_name, "after": 30})).unwrap_err().1, cadmin::codes::BAD_ARGS);
}

#[test]
fn health_thresholds_alert_once_per_window() {
    let f = fx("health", true);
    call(NODE_ALERTS, &Who::Owner, json!({"to": f.ws})).unwrap();
    let gib = 1u64 << 30;
    let h = Health {
        at: compute::now(),
        boot_id: "b".into(),
        disks: vec![DiskFact { name: "root".into(), path: "/".into(), total_bytes: 500 * gib, free_bytes: 9 * gib }],
        executor: ExecutorFact { reachable: true, remote_admin: Some("full".into()), approval_key_fps: vec!["abc".into()], ..Default::default() },
        ..Default::default()
    };
    on_health(&f.node, h.clone());
    let m = test_seams::take_messages(&f.ws);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains("9 GB free on / (floor 30 GB)"), "{m:?}");
    on_health(&f.node, h);
    assert!(test_seams::take_messages(&f.ws).is_empty(), "same kind, same node, within 6 h: quiet");
    let shown = get(NODE_HEALTH, &agent(&f), &[("node", &f.node_name)]).unwrap();
    assert_eq!(shown["remoteAdmin"], "full");
    assert_eq!(shown["approvalKeyFp"], "abc");
    assert_eq!(shown["facts"]["disks"][0]["free_bytes"], 9 * gib);
    // A Member login may read health; a passport without switch or grant may not.
    get(NODE_HEALTH, &Who::Login { username: "m".into(), role: Role::Member }, &[("node", &f.node_name)]).unwrap();
    let off = other_ws("healthoff", false);
    assert_eq!(get(NODE_HEALTH, &agent_of(&off), &[("node", &f.node_name)]).unwrap_err().0, 404);
}

fn stage_build(dir: &std::path::Path, comment_asset_os: &str, version: &str) -> Vec<u8> {
    use k2_root_proto::node_build::{fake_macho, format_marker, trusted_comment};
    std::fs::create_dir_all(dir).unwrap();
    let mut bytes = fake_macho("aarch64", &format_marker(version, 2, "macos", "aarch64"));
    bytes.resize(frames::TRANSFER_CHUNK + 1000, 0);
    let sig = k2_root_proto::testsign::sign_with_test_key(&bytes, &trusted_comment(comment_asset_os, "aarch64", version, 2));
    std::fs::write(dir.join("k2-node-macos-aarch64"), &bytes).unwrap();
    std::fs::write(dir.join("k2-node-macos-aarch64.sig"), sig).unwrap();
    bytes
}

#[test]
fn update_offers_a_signed_build_and_records_rollback() {
    let f = fx("update", true);
    call(NODE_ALERTS, &Who::Owner, json!({"to": f.ws})).unwrap();
    let dir = std::env::temp_dir().join(format!("k2-admin-assets-{}", crypto::random_id().unwrap()));
    test_seams::set_assets_dir(Some(dir.clone()));

    // The server offers the build of its own version (cached per version).
    test_seams::set_server_version(Some("0.46.0"));
    // A build whose signature names another file is refused.
    stage_build(&dir.join("0.46.0"), "linux", "0.46.0");
    assert_eq!(call(NODE_UPDATE, &agent(&f), json!({"node": f.node_name})).unwrap_err().1, cadmin::codes::NO_UPDATE);
    // Not newer than what runs (0.45.5): refused (anti-downgrade).
    test_seams::set_server_version(Some("0.45.5"));
    stage_build(&dir.join("0.45.5"), "macos", "0.45.5");
    assert_eq!(call(NODE_UPDATE, &agent(&f), json!({"node": f.node_name})).unwrap_err().1, cadmin::codes::NO_UPDATE);
    // Tampered bytes: refused.
    test_seams::set_server_version(Some("0.46.0"));
    stage_build(&dir.join("0.46.0"), "macos", "0.46.0");
    let mut evil = std::fs::read(dir.join("0.46.0/k2-node-macos-aarch64")).unwrap();
    let last = evil.len() - 1;
    evil[last] ^= 1;
    std::fs::write(dir.join("0.46.0/k2-node-macos-aarch64"), &evil).unwrap();
    assert_eq!(call(NODE_UPDATE, &agent(&f), json!({"node": f.node_name})).unwrap_err().1, cadmin::codes::NO_UPDATE);

    let bytes = stage_build(&dir.join("0.46.0"), "macos", "0.46.0");
    let r = call(NODE_UPDATE, &agent(&f), json!({"node": f.node_name, "canary": true})).unwrap();
    assert_eq!(r["update"]["state"], "offered");
    let sent = test_seams::take_sent(&f.node);
    let (offer, chunks): (Vec<_>, Vec<_>) = sent.into_iter().partition(|x| matches!(x, Frame::UpdateOffer(_)));
    let Some(Frame::UpdateOffer(offer)) = offer.into_iter().next() else { panic!("no update_offer") };
    assert_eq!((offer.version.as_str(), offer.os.as_str(), offer.arch.as_str()), ("0.46.0", "macos", "aarch64"));
    assert_eq!(offer.sha256, crypto::sha256_hex(&bytes));
    assert_eq!(offer.size, bytes.len() as u64);
    // The node side re-checks the build with the same checker.
    k2_root_proto::node_build::verify_node_build(&bytes, &offer.sig, &k2_root_proto::keys::trusted_keys(true), "macos", "aarch64").unwrap();
    let cmd = node_open(&f, &offer.cmd).unwrap().cmd;
    assert_eq!((cmd.verb.as_str(), cmd.args["sha256"].as_str(), cmd.args["trigger"].as_str()), ("node.update", Some(offer.sha256.as_str()), Some("explicit")));
    let mut got = Vec::new();
    for c in &chunks {
        let Frame::UpdateChunk(c) = c else { panic!("unexpected frame") };
        assert_eq!(c.transfer_id, offer.transfer_id);
        got.extend(crypto::unb64(&c.data).unwrap());
    }
    assert_eq!(chunks.len(), 2);
    assert_eq!(got, bytes, "the chunks reassemble to the signed build");

    let rolled = frames::UpdateState {
        transfer_id: Some(offer.transfer_id.clone()),
        cmd_id: Some(cmd.cmd_id.clone()),
        phase: wire::UpdatePhase::RolledBack,
        version: "0.46.0".into(),
        from: Some("0.45.5".into()),
        code: None,
        message: Some("no hello in 120 s".into()),
        at: compute::now(),
        steps: vec![
            wire::UpdateStep { step: "swap".into(), ok: true, at: compute::now(), detail: None },
            wire::UpdateStep { step: "rollback".into(), ok: true, at: compute::now(), detail: Some("why=health_gate".into()) },
        ],
    };
    on_update_state(&f.node, rolled.clone());
    let h = get(NODE_HEALTH, &Who::Owner, &[("node", &f.node_name)]).unwrap();
    assert_eq!(h["update"]["state"], "rolled_back");
    assert_eq!(h["update"]["steps"][1]["step"], "rollback", "the node's update steps are kept: {h}");
    assert_eq!(h["updateRing"], "canary");
    let m = test_seams::take_messages(&f.ws);
    assert!(m.iter().any(|x| x.contains("rolled back")), "{m:?}");
    on_update_state(&f.node, rolled);
    assert!(test_seams::take_messages(&f.ws).is_empty(), "a repeated phase (re-attach) doesn't alert twice");

    // Rollback is operate class: controller signature only.
    let r = call(NODE_ROLLBACK, &agent(&f), json!({"node": f.node_name, "reason": "bad build"})).unwrap();
    assert_eq!(r["update"]["state"], "rollback_requested");
    let cmds = admin_cmds(test_seams::take_sent(&f.node));
    let c = node_open(&f, &cmds[0]).unwrap().cmd;
    assert_eq!((c.verb.as_str(), c.class), ("node.rollback", wire::Class::Operate));
    // Offline: refused, not queued.
    test_seams::set_online(&f.node, false);
    assert_eq!(call(NODE_ROLLBACK, &agent(&f), json!({"node": f.node_name})).unwrap_err(), (409, codes::NODE_OFFLINE.to_string()));
    test_seams::set_assets_dir(None);
    test_seams::set_server_version(None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// After the server updates, its nodes are offered the new server's own
/// k2-node: a cache of another version (or the old unversioned one) is
/// replaced, a matching cache is reused, a tampered one fails the check,
/// and the k2-node bundled beside the daemon is used with the release's
/// `.sig` when it is this version's.
#[test]
fn the_node_build_cache_follows_the_server_version() {
    let tmp = |t: &str| std::env::temp_dir().join(format!("k2-assets-{t}-{}", crypto::random_id().unwrap()));
    let (cache, release) = (tmp("cache"), tmp("release"));
    test_seams::set_assets_dir(Some(cache.clone()));
    test_seams::set_release_dir(Some(release.clone()));
    test_seams::set_server_version(Some("0.46.1"));
    // Left by the server before it updated: an unversioned 0.46.0 build,
    // and a 0.46.0 build under this version's name.
    stage_build(&cache, "macos", "0.46.0");
    stage_build(&cache.join("0.46.1"), "macos", "0.46.0");
    let fresh = stage_build(&release, "macos", "0.46.1");

    let b = load_build("macos", "aarch64").unwrap();
    assert_eq!((b.version.as_str(), b.sha256.as_str()), ("0.46.1", crypto::sha256_hex(&fresh).as_str()), "the stale cache was replaced");
    assert!(!cache.join("k2-node-macos-aarch64").exists(), "the old unversioned cache is gone");

    // Reused: the release is gone, the cache still serves 0.46.1.
    std::fs::remove_dir_all(&release).unwrap();
    assert_eq!(load_build("macos", "aarch64").unwrap().version, "0.46.1", "a matching cache is reused");

    // Tampered: the check fails, nothing is offered.
    let p = cache.join("0.46.1/k2-node-macos-aarch64");
    let mut evil = std::fs::read(&p).unwrap();
    let last = evil.len() - 1;
    evil[last] ^= 1;
    std::fs::write(&p, &evil).unwrap();
    match load_build("macos", "aarch64") {
        Err(e) => assert_eq!(e.code, cadmin::codes::NO_UPDATE, "a tampered cache fails the check"),
        Ok(b) => panic!("a tampered cache was offered: {}", b.version),
    }
    // A pair that fails the check is refilled once the source is back (a
    // crash between the two writes never sticks).
    let again = stage_build(&release, "macos", "0.46.1");
    let b = load_build("macos", "aarch64").unwrap();
    assert_eq!(b.sha256, crypto::sha256_hex(&again), "the broken pair was refilled from the release");

    // Bundled: an empty cache and a release holding only the .sig: the
    // k2-node beside the daemon fills it.
    let (cache2, release2, bundle) = (tmp("cache2"), tmp("release2"), tmp("bundle"));
    let bytes = stage_build(&bundle, "macos", "0.46.1");
    std::fs::create_dir_all(&release2).unwrap();
    std::fs::copy(bundle.join("k2-node-macos-aarch64.sig"), release2.join("k2-node-macos-aarch64.sig")).unwrap();
    test_seams::set_assets_dir(Some(cache2.clone()));
    test_seams::set_release_dir(Some(release2.clone()));
    test_seams::set_bundled_node(Some(bundle.join("k2-node-macos-aarch64")));
    let b = load_build("macos", "aarch64").unwrap();
    assert_eq!((b.version.as_str(), b.sha256.as_str()), ("0.46.1", crypto::sha256_hex(&bytes).as_str()));
    assert!(cache2.join("0.46.1/k2-node-macos-aarch64").exists(), "the bundled build is cached");

    test_seams::set_bundled_node(None);
    test_seams::set_release_dir(None);
    test_seams::set_server_version(None);
    test_seams::set_assets_dir(None);
    for d in [cache, cache2, release2, bundle] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn route_tables_guard_every_admin_route() {
    for p in ROUTES {
        let r = crate::routes::route_policy::lookup(p).unwrap_or_else(|| panic!("{p} has no route_policy row"));
        assert_eq!(POST_ROUTES.contains(p), r.post.is_some(), "{p}: POST floor vs POST-only list");
        assert_eq!(POST_ROUTES.contains(p), r.get.is_none(), "{p}: a mutating route must not admit GET");
        assert_eq!(crate::compute_routes::is_post_route(p), POST_ROUTES.contains(p), "{p}: dispatcher 405 guard");
        assert_eq!(crate::session_token::is_agent_verb(p), AGENT_VERBS.contains(p), "{p}: agent verb table");
    }
    assert!(!crate::session_token::is_agent_verb(APPROVE));
    assert!(!crate::session_token::is_agent_verb(DENY));
}

/// A fake node answering the next admin command with `state`/`code`.
/// Returns the command it saw.
fn answer_next(node: String, state: &'static str, code: Option<&'static str>) -> std::thread::JoinHandle<Option<wire::Command>> {
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(c) = admin_cmds(test_seams::take_sent(&node)).into_iter().next() {
                let body = crypto::unb64(&c.body_b64).unwrap();
                let cmd: wire::Command = serde_json::from_slice(&body).unwrap();
                let out = if state == "done" { json!({"job_user": "k2job"}) } else { Value::Null };
                assert!(super::wait_deliver(&result(&cmd.cmd_id, state, code, None, out)), "a route waits for {}", cmd.verb);
                return Some(cmd);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    })
}

/// Q5 (0.46.4): an operator turns the job wall on at once (operate, the
/// controller signature only, `restart` on by default); a node older than
/// 0.46.4 asks for the owner, so it becomes a held plan with the args that
/// node knows (wall.enable {} + unit.restart k2-node).
#[test]
fn the_job_wall_goes_at_once_and_an_older_node_holds_for_the_owner() {
    let f = fx("wall", true);
    let responder = answer_next(f.node.clone(), "done", None);
    let r = call(NODE_WALL, &agent(&f), json!({"node": f.node_name})).unwrap();
    let cmd = responder.join().unwrap().expect("wall.enable went out");
    assert_eq!((cmd.verb.as_str(), cmd.class, &cmd.args), ("wall.enable", wire::Class::Operate, &json!({"restart": true})));
    assert_eq!(r["ok"], true, "{r}");
    assert!(r["note"].as_str().unwrap().contains("once its running jobs end"), "{r}");

    let responder = answer_next(f.node.clone(), "done", None);
    call(NODE_WALL, &agent(&f), json!({"node": f.node_name, "restart": false})).unwrap();
    assert_eq!(responder.join().unwrap().unwrap().args, json!({"restart": false}));

    // Round 4 item 4: the hold branches on the node's `wall-restart`
    // feature, never on a version string, and without it never plans a
    // `unit.restart` (that would kill unwalled jobs).
    let seen = |version: &str| {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::touch_seen(&conn, &f.node, "lan", 3, version, compute::now());
    };
    let held = |f: &Fx| {
        let responder = answer_next(f.node.clone(), "refused", Some(wire::code::APPROVAL_REQUIRED));
        let r = call(NODE_WALL, &agent(f), json!({"node": f.node_name})).unwrap();
        responder.join().unwrap().expect("tried at once first");
        assert_eq!((r["held"].as_bool(), r["request"]["class"].as_str(), r["request"]["kind"].as_str()), (Some(true), Some("privileged"), Some("command")), "{r}");
        assert_eq!(r["request"]["body"]["verb"], "wall.enable", "{r}");
        assert!(!r.to_string().contains("unit.restart"), "never a restart that kills jobs: {r}");
        r
    };
    // An older node (no feature): the safe hold, its version named.
    let r = held(&f);
    assert_eq!(r["request"]["body"]["args"], json!({}), "{r}");
    let why = r["request"]["reason"].as_str().unwrap_or("").to_string();
    assert!(why.contains("k2-node 0.45.5") && why.contains("held without a restart"), "{why}");
    // A 0.46.4 version without the feature: still the safe hold.
    seen("0.46.4");
    assert_eq!(held(&f)["request"]["body"]["args"], json!({}));
    // No version at all: the reason says so, not "0.46.3 or older".
    seen("");
    let why = held(&f)["request"]["reason"].as_str().unwrap_or("").to_string();
    assert!(why.contains("unknown version") && !why.contains("0.46.3"), "{why}");
    // A dev build saying 0.46.3 but offering `wall-restart` (an owner
    // opt-out): the same command is held, restart included.
    seen("0.46.3");
    test_seams::register(&f.node, true, &[wire::FEATURE_ADMIN, wire::FEATURE_HEALTH, wire::FEATURE_SELF_UPDATE, wire::FEATURE_WALL_RESTART]);
    let r = held(&f);
    assert_eq!(r["request"]["body"]["args"], json!({"restart": true}), "{r}");
    assert!(r["request"]["reason"].as_str().unwrap_or("").contains("turned the job wall off"), "{r}");

    // Any other refusal is an error, not a hold.
    let responder = answer_next(f.node.clone(), "refused", Some(wire::code::BAD_ARGS));
    let e = call(NODE_WALL, &agent(&f), json!({"node": f.node_name})).unwrap_err();
    responder.join().unwrap();
    assert_eq!(e, (409, "node_refused".to_string()));

    // Agents need the operate switch; bad args never leave.
    let ws = other_ws("wall-off", false);
    assert_eq!(call(NODE_WALL, &agent_of(&ws), json!({"node": f.node_name})).unwrap_err().1, cadmin::codes::OPERATE_OFF);
    assert_eq!(call(NODE_WALL, &agent(&f), json!({"node": f.node_name, "restart": "yes"})).unwrap_err().0, 400);
    assert!(test_seams::take_sent(&f.node).is_empty());
}

// ── node sharing: the servers verbs (prd-compute-shared-queue-v1 §9) ──

/// A fake node answering every admin command until it sees `until`:
/// `servers.list` with `list`, everything else `done`. Returns the verbs.
fn answer_servers(node: String, list: Value, until: &'static str) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            for c in admin_cmds(test_seams::take_sent(&node)) {
                let body = crypto::unb64(&c.body_b64).unwrap();
                let cmd: wire::Command = serde_json::from_slice(&body).unwrap();
                seen.push(cmd.verb.clone());
                let out = if cmd.verb == "servers.list" { list.clone() } else { json!({}) };
                assert!(super::wait_deliver(&result(&cmd.cmd_id, "done", None, None, out)), "a route waits for {}", cmd.verb);
                if cmd.verb == until {
                    return seen;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        seen
    })
}

/// The pinned `servers.list` answer (`share::ServersList`).
fn servers_listed() -> Value {
    let entry = |slot: u8, label: &str, limits: pshare::ShareLimits| pshare::ServerListing {
        slot,
        label: label.into(),
        state: pshare::ServerStatus::Active,
        trust: pshare::Trust::Own,
        fp: None,
        fp_prefix: if slot == 0 { "ab".repeat(32) } else { "0123456789abcdef".into() },
        limits,
        expires_at: None,
        expires_in_secs: None,
        added_at: 1,
        added_by: "local:rosson".into(),
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        sas: None,
        refusal: None,
        online: true,
        priority: None,
    };
    serde_json::to_value(pshare::ServersList {
        max_servers: pshare::DEFAULT_MAX_SERVERS,
        solo: false,
        servers: vec![
            entry(0, "rosson.k2.dev", pshare::ShareLimits::owner(2, 8, 16, 100, pshare::Network::On)),
            entry(1, "z3thon.k2.dev", pshare::ShareLimits::new_server(pshare::Trust::Own, true)),
        ],
    })
    .unwrap()
}

/// §4.1, §4.3, §9: an add is always held and goes out owner-signed once
/// approved; narrowing goes at once, widening is held; pause at once;
/// revoke is the owner's. Every act audited; the mirror notes the add.
#[test]
fn servers_add_limits_pause_and_revoke() {
    let f = fx("servers", true);
    test_seams::register(&f.node, true, &[wire::FEATURE_ADMIN, wire::FEATURE_HEALTH, frames::FEATURE_JOB_WALL, pshare::FEATURE_SERVERS]);
    {
        let h = k2_core::db::shared();
        cadmin::set_setting(&h.lock(), cadmin::ALERTS_TO, Some(&f.ws), "owner", 1).unwrap();
    }
    // A Mac takes only your own servers in 0.46.3.
    let e = call(SERVERS_ADD, &agent(&f), json!({"node": f.node_name, "url": "https://z3thon.k2.dev", "code": "ABCDE-FGHJK.0123456789abcdef", "trust": "team"})).unwrap_err();
    assert_eq!(e, (409, pshare::codes::TEAM_NEEDS_TENANT_WALL.to_string()));
    let r = call(SERVERS_ADD, &agent(&f), json!({"node": f.node_name, "url": "https://z3thon.k2.dev", "code": "ABCDE-FGHJK.0123456789abcdef", "trust": "own"})).unwrap();
    assert_eq!((r["held"].as_bool(), r["request"]["class"].as_str()), (Some(true), Some("privileged")), "{r}");
    assert!(r.get("warning").is_none(), "the team warning is for other people's servers");
    let binding = k2_core::compute::proto::pairing::code_binding("ABCDEFGHJK");
    assert!(!r.to_string().contains(&binding), "the binding is never shown");
    assert!(test_seams::take_sent(&f.node).is_empty(), "nothing goes out while held");
    let id = r["request"]["id"].as_str().unwrap().to_string();
    let e = call(SERVERS_ADD, &agent(&f), json!({"node": f.node_name, "url": "https://b.k2.dev", "code": "ABCDE-FGHJK.0123456789abcdef"})).unwrap_err();
    assert_eq!(e, (409, pshare::codes::SERVER_ADD_PENDING.to_string()), "one add at a time");
    // The headless approval (the two-server harness): the existing vault
    // password confirm, never a new path. No confirm at all is refused.
    let e = call(APPROVE, &Who::Owner, json!({"request": id})).unwrap_err();
    assert_eq!(e, (403, cadmin::codes::VAULT_CONFIRM_REQUIRED.to_string()));
    let pw_token = test_seams::vault_handle().confirm.mint(cadmin::CONFIRM_ACTION, Some(&id), "owner", "vault-password");
    let r = call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": pw_token})).unwrap();
    assert_eq!((r["sent"].as_bool(), r["request"]["decidedVia"].as_str()), (Some(true), Some("vault-password")), "{r}");
    let cmds = admin_cmds(test_seams::take_sent(&f.node));
    assert_eq!(cmds.len(), 1);
    let o = node_open(&f, &cmds[0]).expect("the node accepts the owner-signed add");
    assert_eq!((o.cmd.verb.as_str(), o.cmd.class), ("servers.add", wire::Class::Privileged));
    let args: pshare::AddArgs = serde_json::from_value(o.cmd.args.clone()).unwrap();
    assert_eq!((args.code_binding.as_str(), args.fp_prefix.as_str(), args.trust, args.for_secs), (binding.as_str(), "0123456789abcdef", pshare::Trust::Own, None));
    on_admin_result(&f.node, result(&o.cmd.cmd_id, "done", None, Some(0), json!({})));

    // Narrowing: the list is mirrored first, then sent at once.
    let responder = answer_servers(f.node.clone(), servers_listed(), "servers.limits");
    let r = call(SERVERS_LIMITS, &agent(&f), json!({"node": f.node_name, "server": "z3thon.k2.dev", "set": {"max_queued": 5}})).unwrap();
    assert_eq!(responder.join().unwrap(), vec!["servers.list", "servers.limits"]);
    assert_eq!(r["class"], "operate", "{r}");
    let msgs = test_seams::take_messages(&f.ws);
    assert!(msgs.iter().any(|m| m.contains("is now shared with z3thon.k2.dev")), "{msgs:?}");
    // Widening: held, nothing sent.
    let r = call(SERVERS_LIMITS, &agent(&f), json!({"node": f.node_name, "server": "z3thon.k2.dev", "set": {"max_running": 3}})).unwrap();
    assert_eq!(r["held"], true, "{r}");
    assert!(test_seams::take_sent(&f.node).is_empty());
    let e = call(SERVERS_LIMITS, &agent(&f), json!({"node": f.node_name, "server": "nobody.k2.dev", "set": {"max_queued": 1}})).unwrap_err();
    assert_eq!(e.0, 404);
    // Every server starts at weight 1, the owner too; raising the owner's
    // own weight (slot 0) is a widening: held.
    let r = call(SERVERS_LIMITS, &agent(&f), json!({"node": f.node_name, "server": "slot:0", "set": {"weight": 2}})).unwrap();
    assert_eq!(r["held"], true, "{r}");
    assert!(test_seams::take_sent(&f.node).is_empty());
    let e = call(SERVERS_LIMITS, &agent(&f), json!({"node": f.node_name, "server": "z3thon.k2.dev", "set": {"weight": 0}})).unwrap_err();
    assert_eq!(e.1, cadmin::codes::BAD_ARGS, "weight is 1–100");
    // Pause: at once.
    let responder = answer_servers(f.node.clone(), servers_listed(), "servers.pause");
    let r = call(SERVERS_PAUSE, &agent(&f), json!({"node": f.node_name, "server": "z3thon.k2.dev", "action": "pause"})).unwrap();
    assert_eq!(responder.join().unwrap(), vec!["servers.pause"]);
    assert_eq!(r["action"], "servers.pause");
    // Revoke: never an agent; the owner, at once.
    let e = call(SERVERS_REVOKE, &agent(&f), json!({"node": f.node_name, "server": "z3thon.k2.dev"})).unwrap_err();
    assert_eq!(e, (403, "owner_only".to_string()));
    let responder = answer_servers(f.node.clone(), servers_listed(), "servers.revoke");
    call(SERVERS_REVOKE, &Who::Owner, json!({"node": f.node_name, "server": "z3thon.k2.dev", "now": true})).unwrap();
    assert_eq!(responder.join().unwrap(), vec!["servers.revoke"]);
    let e = call(SERVERS_PAUSE, &agent(&f), json!({"node": f.node_name, "server": "slot:0"})).unwrap_err();
    assert_eq!(e.1, cadmin::codes::BAD_ARGS, "slot 0 is this server");
    let kinds: Vec<String> = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::events(&conn, 200).into_iter().filter(|e| e.node_id.as_deref() == Some(&f.node)).map(|e| e.kind).collect()
    };
    for k in ["server_add", "server_limits", "server_pause", "server_revoke", "share_note"] {
        assert!(kinds.iter().any(|x| x == k), "missing audit {k}: {kinds:?}");
    }
    // A node without `servers`: node_too_old.
    test_seams::register(&f.node, true, &[wire::FEATURE_ADMIN, frames::FEATURE_JOB_WALL]);
    let e = call(SERVERS_PAUSE, &agent(&f), json!({"node": f.node_name, "server": "z3thon.k2.dev"})).unwrap_err();
    assert_eq!(e, (409, pshare::codes::NODE_TOO_OLD.to_string()));
}

/// A `servers.list` answer with the target (slot 1) enrolled (its full
/// key) and online or not, and another person's server (slot 2).
fn servers_listed_for_owner(target_fp: &str, online: bool) -> Value {
    let mut v = servers_listed();
    let s = v["servers"].as_array_mut().unwrap();
    s[1]["fp"] = target_fp.into();
    s[1]["online"] = online.into();
    let mut team = s[1].clone();
    team["slot"] = 2.into();
    team["label"] = "baden.k2.dev".into();
    team["trust"] = "team".into();
    team["fp_prefix"] = "1123456789abcdef".into();
    team["fp"] = format!("1123456789abcdef{}", "0".repeat(48)).into();
    s.push(team);
    v
}

/// prd-macos-tenant-compute-v1 §6.4 (MT27, MT28, Q9): only the owner
/// files an owner change, never an agent; it's always held, and once
/// approved the node gets one owner-signed `servers.owner` naming the
/// target's full key. A team or offline target, a node without the
/// feature, or remote admin below `full` are refused up front.
#[test]
fn an_owner_change_is_the_owners_held_and_names_the_key() {
    let f = fx("owner-change", true);
    let target_fp = format!("0123456789abcdef{}", "a".repeat(48));
    let feats = [wire::FEATURE_ADMIN, wire::FEATURE_HEALTH, frames::FEATURE_JOB_WALL, pshare::FEATURE_SERVERS, pshare::FEATURE_SERVERS_OWNER];
    test_seams::register(&f.node, true, &feats);
    let body = json!({"node": f.node_name, "server": "z3thon.k2.dev", "caches": "move", "old": "keep"});

    // Never an agent (even with the operate switch on).
    let e = call(SERVERS_OWNER, &agent(&f), body.clone()).unwrap_err();
    assert_eq!(e, (403, codes::OWNER_ONLY.to_string()));
    // Review F4: nor through the generic proposal, by an agent or the owner,
    // as a command or a plan step; nothing is filed.
    let args = json!({"server": "z3thon.k2.dev", "fp": target_fp});
    for (who, req) in [
        (agent(&f), json!({"node": f.node_name, "verb": "servers.owner", "args": args})),
        (Who::Owner, json!({"node": f.node_name, "verb": "servers.owner", "args": args})),
        (agent(&f), json!({"node": f.node_name, "kind": "plan", "steps": [{"verb": "unit.restart", "args": {"unit": "k2-node"}}, {"verb": "servers.owner", "args": args}]})),
    ] {
        let e = call(PROPOSE, &who, req.clone()).unwrap_err();
        assert_eq!(e, (403, codes::OWNER_ONLY.to_string()), "{req}");
    }
    let filed = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        cadmin::list_requests(&conn, &cadmin::ReqFilter { node_id: Some(&f.node), state: None, limit: 100, ..Default::default() }).len()
    };
    assert_eq!(filed, 0, "nothing was filed");

    // A team target, then an offline one: refused before anything is filed.
    let responder = answer_servers(f.node.clone(), servers_listed_for_owner(&target_fp, true), "servers.list");
    let e = call(SERVERS_OWNER, &Who::Owner, json!({"node": f.node_name, "server": "baden.k2.dev"})).unwrap_err();
    responder.join().unwrap();
    assert_eq!(e, (409, pshare::codes::OWNER_NEEDS_OWN_SERVER.to_string()));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let rows: Vec<k2_core::compute::share::ServerRow> = store::node_servers(&conn, &f.node)
            .into_iter()
            .map(|mut r| {
                if r.slot == 1 {
                    r.usage = Some(json!({"online": false}));
                }
                r
            })
            .collect();
        store::replace_node_servers(&conn, &f.node, &rows, compute::now()).unwrap();
    }
    let e = call(SERVERS_OWNER, &Who::Owner, body.clone()).unwrap_err();
    assert_eq!(e, (409, pshare::codes::OWNER_TARGET_NOT_READY.to_string()), "offline");
    let responder = answer_servers(f.node.clone(), servers_listed_for_owner(&target_fp, true), "servers.list");
    get(SERVERS, &Who::Owner, &[("node", &f.node_name), ("refresh", "1")]).unwrap();
    responder.join().unwrap();

    // Held for the owner; one at a time.
    let r = call(SERVERS_OWNER, &Who::Owner, json!({"node": f.node_name, "server": "z3thon.k2.dev", "old": "revoke", "caches": "wipe", "now": true, "reason": "drill"})).unwrap();
    assert_eq!((r["held"].as_bool(), r["request"]["class"].as_str()), (Some(true), Some("privileged")), "{r}");
    let summary = r["request"]["summary"].as_str().unwrap();
    assert!(summary.contains("make z3thon.k2.dev (key ") && summary.contains(&target_fp) && summary.contains("leaves"), "the card shows the full key: {summary}");
    assert!(test_seams::take_sent(&f.node).is_empty(), "nothing goes out while held");
    let e = call(SERVERS_OWNER, &Who::Owner, body.clone()).unwrap_err();
    assert_eq!(e, (409, "owner_change_pending".to_string()));
    let id = r["request"]["id"].as_str().unwrap().to_string();
    let r = call(APPROVE, &Who::Owner, json!({"request": id, "confirmToken": token_for(&id)})).unwrap();
    assert_eq!(r["sent"], true, "{r}");
    let cmds = admin_cmds(test_seams::take_sent(&f.node));
    assert_eq!(cmds.len(), 1);
    let o = node_open(&f, &cmds[0]).expect("the node accepts the owner-signed change");
    assert_eq!((o.cmd.verb.as_str(), o.cmd.class), ("servers.owner", wire::Class::Privileged));
    let a: pshare::OwnerArgs = serde_json::from_value(o.cmd.args.clone()).unwrap();
    assert_eq!((a.server.as_str(), a.fp.as_deref(), a.old, a.caches, a.now), ("z3thon.k2.dev", Some(target_fp.as_str()), pshare::OldOwner::Revoke, pshare::Caches::Wipe, true));
    let kinds: Vec<String> = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::events(&conn, 200).into_iter().filter(|e| e.node_id.as_deref() == Some(&f.node)).map(|e| e.kind).collect()
    };
    assert!(kinds.iter().any(|k| k == "server_owner"), "audited: {kinds:?}");

    // A node without the feature, then remote admin at `policy`.
    test_seams::register(&f.node, true, &feats[..4]);
    let e = call(SERVERS_OWNER, &Who::Owner, body.clone()).unwrap_err();
    assert_eq!(e, (409, pshare::codes::NODE_TOO_OLD.to_string()));
    test_seams::register(&f.node, true, &feats);
    {
        let h = k2_core::db::shared();
        cadmin::set_reported_admin(&h.lock(), &f.node, Some("policy"), None);
    }
    let e = call(SERVERS_OWNER, &Who::Owner, body).unwrap_err();
    assert_eq!(e, (409, cadmin::codes::REMOTE_ADMIN_OFF.to_string()));
}

/// An offer as the node would send it to this server's session.
fn offer_with(share: Option<pshare::ShareView>, features: &[&str]) -> Offer {
    Offer {
        share,
        revision: 1,
        control: frames::Control::Active,
        control_by: None,
        control_at: None,
        availability: frames::Availability { ok: true, reasons: vec![] },
        caps: frames::Caps { cpu_millis: 1000, mem_bytes: 1 << 30, disk_budget_bytes: 1 << 30, hard: true },
        free: frames::Free { cpu_millis: 1000, mem_bytes: 1 << 30, disk_free_bytes: 1 << 30 },
        slots: vec![],
        running: 0,
        parallel_max: 1,
        refusal: None,
        labels: Default::default(),
        tools: Default::default(),
        protocol: k2_core::compute::proto::PROTOCOL,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.4".into(),
        features: features.iter().map(|f| f.to_string()).collect(),
    }
}

fn share_view(role: pshare::Role) -> pshare::ShareView {
    pshare::ShareView {
        role,
        slot: if role == pshare::Role::Owner { 0 } else { 1 },
        shared: true,
        servers: 2,
        owner_label: (role == pshare::Role::Tenant).then(|| "rosson.k2.dev".to_string()),
        state: pshare::ServerStatus::Active,
        limits: pshare::ShareLimits::new_server(pshare::Trust::Own, false),
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        expires_in_secs: None,
        skew_secs: None,
        quotes: vec![],
        priority: None,
    }
}

/// Security review (cross-branch) + MT36: a tenant node's offer that
/// claims owner-only features (or a share view saying owner) stays a
/// tenant; it becomes this server's only after an owner change both the
/// node confirmed (its live offer says owner) and the person here
/// approved in person (`take-owner` with a fresh confirm for this node).
/// Then the owner label goes and the role change runs.
#[test]
fn a_tenant_node_is_taken_over_only_in_person_after_it_confirms() {
    let f = fx("take", true);
    let owner_feats = [wire::FEATURE_ADMIN, wire::FEATURE_HEALTH, wire::FEATURE_SELF_UPDATE, pshare::FEATURE_QUEUE, pshare::FEATURE_SERVERS];
    let set_offer = |o: Offer| {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::store_offer(&conn, &f.node, &o, compute::now());
        store::store_share(&conn, &f.node, o.share.as_ref(), compute::now());
    };
    let row = || store::node_by_id(&k2_core::db::shared().lock(), &f.node).unwrap();
    {
        let h = k2_core::db::shared();
        store::set_tenant_of(&h.lock(), &f.node, "rosson.k2.dev").unwrap();
    }
    let body = || json!({"node": f.node_name});
    let token = || test_seams::vault_handle().confirm.mint(cadmin::CONFIRM_ACTION, Some(&take_owner_subject(&f.node)), "owner", "local-auth");

    // Still the other server's (its offer says tenant): nothing to take.
    set_offer(offer_with(Some(share_view(pshare::Role::Tenant)), &[pshare::FEATURE_QUEUE, pshare::FEATURE_SERVERS]));
    assert!(!row().owner_claim());
    let e = call(TAKE_OWNER, &Who::Owner, json!({"node": f.node_name, "confirmToken": token()})).unwrap_err();
    assert_eq!(e, (409, NO_OWNER_CLAIM.to_string()));

    // The node claims it: owner-only features with no view, then a view.
    for o in [offer_with(None, &owner_feats), offer_with(Some(share_view(pshare::Role::Owner)), &owner_feats)] {
        set_offer(o);
        let n = row();
        assert!(n.is_tenant() && !n.owned_for_secrets() && n.owner_claim(), "a claim, still a tenant");
    }
    // What the person confirms (verification M1): the node's full key, the
    // owner until now, that this server becomes its admin, and the
    // compute:<ws>/* grants that would reach it. Nothing changes.
    let p = call(TAKE_OWNER, &Who::Owner, json!({"node": f.node_name, "preview": true})).unwrap();
    assert_eq!(p["fingerprint"], fp_of(&f.node), "{p}");
    assert_eq!((p["from"].as_str(), p["becomesAdmin"].as_bool(), p["grantsKnown"].as_bool()), (Some("rosson.k2.dev"), Some(true), Some(true)), "{p}");
    assert_eq!(p["secretGrants"], json!([]), "{p}");
    assert!(row().is_tenant(), "a preview changes nothing");
    // The owner gets the same in its note.
    on_owner_claim(&row());
    let note: String = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.query_row(
            "SELECT json_extract(detail_json, '$.text') FROM compute_events WHERE kind = 'share_note' AND node_id = ?1 ORDER BY rowid DESC LIMIT 1",
            rusqlite::params![f.node],
            |r| r.get(0),
        )
        .unwrap()
    };
    for want in [fp_of(&f.node).as_str(), "rosson.k2.dev", "its admin", "compute:<workspace>/* secret grant", "none today", "type"] {
        assert!(note.contains(want), "the note says {want:?}: {note}");
    }
    // An agent never takes a node over (nor previews it), even one under
    // "operate".
    let e = call(TAKE_OWNER, &agent(&f), body()).unwrap_err();
    assert_eq!(e.0, 403, "{e:?}");
    let e = call(TAKE_OWNER, &agent(&f), json!({"node": f.node_name, "preview": true})).unwrap_err();
    assert_eq!(e.0, 403, "{e:?}");
    // The owner needs a fresh confirm, for this node and this action.
    assert_eq!(call(TAKE_OWNER, &Who::Owner, body()).unwrap_err(), (403, cadmin::codes::VAULT_CONFIRM_REQUIRED.to_string()));
    let other_subject = test_seams::vault_handle().confirm.mint(cadmin::CONFIRM_ACTION, Some(&f.node), "owner", "local-auth");
    let e = call(TAKE_OWNER, &Who::Owner, json!({"node": f.node_name, "confirmToken": other_subject})).unwrap_err();
    assert_eq!(e, (403, cadmin::codes::VAULT_CONFIRM_REQUIRED.to_string()), "a confirm for another subject");
    let wrong_action = test_seams::vault_handle().confirm.mint("reveal", Some(&take_owner_subject(&f.node)), "owner", "local-auth");
    let e = call(TAKE_OWNER, &Who::Owner, json!({"node": f.node_name, "confirmToken": wrong_action})).unwrap_err();
    assert_eq!(e, (403, cadmin::codes::VAULT_CONFIRM_REQUIRED.to_string()), "a confirm for another action");
    // Offline: the claim must come from a live session.
    test_seams::set_online(&f.node, false);
    let e = call(TAKE_OWNER, &Who::Owner, json!({"node": f.node_name, "confirmToken": token()})).unwrap_err();
    assert_eq!(e, (409, codes::NODE_OFFLINE.to_string()));
    test_seams::set_online(&f.node, true);
    assert!(row().is_tenant(), "nothing above changed the row");

    // Confirmed by the node and approved in person: owned.
    let r = call(TAKE_OWNER, &Who::Owner, json!({"node": f.node_name, "confirmToken": token()})).unwrap();
    assert_eq!((r["role"].as_str(), r["from"].as_str(), r["via"].as_str()), (Some("owner"), Some("rosson.k2.dev"), Some("local-auth")), "{r}");
    let n = row();
    assert!(!n.is_tenant() && n.owner_label.is_none() && n.owned_for_secrets());
    let events: Vec<String> = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let mut st = conn.prepare("SELECT kind || ':' || detail_json FROM compute_events WHERE node_id = ?1 ORDER BY rowid").unwrap();
        let rows: Vec<String> = st.query_map(rusqlite::params![f.node], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
        rows
    };
    assert!(events.iter().any(|e| e.starts_with("owner_taken:") && e.contains("local-auth")), "{events:?}");
    assert!(events.iter().any(|e| e.starts_with("role_change:") && e.contains("\"to\":\"owner\"")), "{events:?}");
    // Once only.
    let e = call(TAKE_OWNER, &Who::Owner, json!({"node": f.node_name, "confirmToken": token()})).unwrap_err();
    assert_eq!(e.0, 409);
    // How the grants read for a person (M1).
    let g = Some(vec![("k2".to_string(), "GH_TOKEN".to_string()), ("k2".to_string(), "NPM".to_string()), ("ops".to_string(), "AWS".to_string())]);
    assert_eq!(crate::compute_secrets::star_grants_text(&g), "k2: GH_TOKEN, NPM; ops: AWS");
    assert!(crate::compute_secrets::star_grants_text(&None).contains("vault is closed"), "never \"none\" when they can't be listed");
}
