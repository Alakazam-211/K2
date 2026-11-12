//! `/cli/approve/*` end to end in the daemon (prd-signed-approvals-v1 S1):
//! the permission, who may be asked, the card and the push, the answer
//! (people only, never an agent), 405 on a GET to a mutating route, and
//! expiry. No test wakes a real agent: outcome lines go to `card`'s sink.

use std::collections::HashMap;

use base64::Engine;
use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay;
use serde_json::{json, Value};

use super::routes::{self, handle};
use super::{card, expiry};
use crate::compute_routes::Who;

struct Ws {
    id: String,
    handle: String,
    conv: String,
}

fn seed(tag: &str) -> Ws {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, format!("/tmp/ap-{handle}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

fn agent(ws: &Ws) -> Who {
    Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) }
}

fn person(login: &str) -> Who {
    Who::Login { username: login.to_string(), role: k2_core::connect_users::role_for_user(login).expect("role") }
}

fn call(who: &Who, path: &str, is_post: bool, body: Value) -> (String, Value) {
    let r = handle(path, who, is_post, body.to_string().as_bytes(), &HashMap::new());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

/// A Connect login (in the temp `$HOME`).
fn login(_home: &crate::test_support::TempHome, name: &str, role: Role) -> String {
    let user = format!("{name}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&user, "password123").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    user
}

/// Link `user` as the owner's account: the person every agent works for.
fn link_owner(_home: &crate::test_support::TempHome, user: &str) {
    let mut s = k2_core::app_settings::load();
    s.owner_accounts = vec![user.to_string()];
    k2_core::app_settings::save(&s).expect("save settings");
}

const KEY: &str = "91ac1312245d73df57fe662bd9502777017b542cf30a9d9569957841c5f58a19";

/// A strict payload answerable for `secs` more seconds, accepting [`KEY`].
fn payload(secs: i64) -> Vec<u8> {
    let now = overlay::now_secs();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let json = format!(
        "{{\"kind\":\"com.example.refund\",\"kind_v\":1,\"rp\":\"shop@test\",\"approver\":\"alex\",\"approver_keys\":[\"{KEY}\"],\"nonce\":\"{nonce}\",\"issued\":\"{}\",\"answer_by\":\"{}\",\"body\":{{\"order\":\"A-1\"}}}}",
        k2_approve::time::format_utc(now - 3600),
        k2_approve::time::format_utc(now + secs)
    );
    let mut b = k2_approve::PREFIX.to_vec();
    b.extend_from_slice(json.as_bytes());
    b
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// A record over `payload` (the daemon checks its shape, never the
/// signature: the relying party does).
fn record(payload: &[u8], key: &str) -> String {
    format!("{{\"v\":1,\"payload_b64\":\"{}\",\"key_id\":\"{key}\",\"alg\":\"ES256\",\"sig_b64\":\"MEUCIQ==\"}}", b64(payload))
}

fn grant(ws: &Ws) {
    let (s, v) = call(&Who::Owner, routes::AGENTS, true, json!({ "workspace": ws.handle, "on": true }));
    assert_eq!(s, "200 OK", "{v}");
}

fn ask(ws: &Ws, to: Option<&str>, payload: &[u8]) -> (String, Value) {
    let mut body = json!({ "payload_b64": b64(payload) });
    if let Some(t) = to {
        body["to"] = json!(t);
    }
    call(&agent(ws), routes::REQUEST, true, body)
}

fn ask_ok(ws: &Ws, to: Option<&str>, payload: &[u8]) -> Value {
    let (s, v) = ask(ws, to, payload);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["state"], "waiting", "{v}");
    v
}

fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap_or_else(|| panic!("no id in {v}")).to_string()
}

fn approve_body(id: &str, rec: &str) -> Value {
    json!({ "id": id, "decision": "approve", "record": rec, "device": "iPhone 16", "via": "phone" })
}

// ── the method guard ────────────────────────────────────────────────

#[test]
fn a_get_on_every_mutating_route_is_405() {
    let ws = seed("apm");
    for path in routes::POST_ROUTES {
        for who in [agent(&ws), Who::Owner] {
            let r = handle(path, &who, false, b"{}", &HashMap::new());
            assert_eq!(r.status, "405 Method Not Allowed", "GET {path} as {}: {}", who.label(), r.body);
        }
    }
    // And a POST to a read is refused the other way.
    for path in [routes::STATUS, routes::WAIT, routes::GET, routes::PENDING] {
        let r = handle(path, &agent(&ws), true, b"{}", &HashMap::new());
        assert_eq!(r.status, "405 Method Not Allowed", "POST {path}: {}", r.body);
    }
}

// ── the permission and who may be asked ─────────────────────────────

#[test]
fn an_agent_without_the_permission_is_refused_with_the_wall_code() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    let ws = seed("apoff");
    let (s, v) = ask(&ws, None, &payload(600));
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], routes::CODE_OFF);
    // A Member can't turn it on; an Admin can.
    let member = login(&home, "mem", Role::Member);
    let (s, v) = call(&person(&member), routes::AGENTS, true, json!({ "workspace": ws.handle, "on": true }));
    assert_eq!(s, "403 Forbidden", "{v}");
    let admin = login(&home, "adm", Role::Admin);
    let (s, v) = call(&person(&admin), routes::AGENTS, true, json!({ "workspace": ws.handle, "on": true }));
    assert_eq!(s, "200 OK", "{v}");
    // An agent can't grant itself.
    let (s, _) = call(&agent(&ws), routes::AGENTS, true, json!({ "workspace": ws.handle, "on": true }));
    assert_eq!(s, "403 Forbidden");
    ask_ok(&ws, None, &payload(600));
}

#[test]
fn by_default_an_agent_asks_only_the_person_it_works_for() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    let gina = login(&home, "gina", Role::Admin);
    let ws = seed("apwho");
    grant(&ws);
    // No --to: the owner's linked login.
    let v = ask_ok(&ws, None, &payload(600));
    assert_eq!(v["person"], owner);
    // Someone else hasn't allowed it.
    let (s, v) = ask(&ws, Some(&gina), &payload(600));
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], "approval_not_allowed");
    // Gina allows this agent, for herself.
    let (s, v) = call(&person(&gina), routes::RULE, true, json!({ "workspace": ws.handle, "rule": "allow" }));
    assert_eq!(s, "200 OK", "{v}");
    let v = ask_ok(&ws, Some(&gina), &payload(600));
    assert_eq!(v["person"], gina);
    // The owner blocks it: even the default person can no longer be asked.
    let (s, _) = call(&person(&owner), routes::RULE, true, json!({ "workspace": ws.handle, "rule": "block" }));
    assert_eq!(s, "200 OK");
    let (s, v) = ask(&ws, None, &payload(600));
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], "approval_not_allowed");
}

#[test]
fn a_payload_that_breaks_the_strict_form_or_the_time_window_is_refused() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    let ws = seed("apshape");
    grant(&ws);
    let mut dup = payload(600);
    // A repeated key: the same strict parser every device uses refuses it.
    let text = String::from_utf8(dup[k2_approve::PREFIX.len()..].to_vec()).expect("utf8");
    let text = text.replacen("\"kind_v\":1,", "\"kind_v\":1,\"kind_v\":1,", 1);
    dup.truncate(k2_approve::PREFIX.len());
    dup.extend_from_slice(text.as_bytes());
    for (bytes, code) in [
        (b"not a payload".to_vec(), "approval_bad_payload"),
        (dup, "approval_bad_payload"),
        (payload(-5), "approval_expired"),
        (payload(2 * 3600), "approval_bad_payload"),
    ] {
        let (s, v) = ask(&ws, None, &bytes);
        assert_eq!(s, "400 Bad Request", "{v}");
        assert_eq!(v["error"]["code"], code, "{v}");
    }
}

// ── the card, the push, the bytes ───────────────────────────────────

#[test]
fn a_request_posts_a_generic_card_pushes_the_phone_and_keeps_the_bytes_exactly() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    k2_core::push_devices::upsert_device(&format!("dev-{owner}"), "apns", &format!("tok-{owner}"), &owner).expect("device");
    let ws = seed("apcard");
    grant(&ws);
    let mark = crate::push_routes::test_push_capture::mark();
    let bytes = payload(600);
    let v = ask_ok(&ws, None, &bytes);
    let id = id_of(&v);
    assert!(id.starts_with("ap_"), "{v}");
    let card_id = v["card"]["id"].as_str().unwrap_or_else(|| panic!("no card in {v}")).to_string();

    let doc = serde_json::to_value(overlay::get_thread_doc(&ws.conv, &card_id).expect("read").expect("card doc")).expect("json");
    assert_eq!(doc["kind"], "approval", "{doc}");
    assert_eq!(doc["body"], "Approval requested: open K2 to read it", "generic words only (SA-15)");
    assert_eq!(doc["approval"]["requestId"], id);
    assert_eq!(doc["approval"]["status"], "waiting");
    assert_eq!(doc["approval"]["requestedBy"], ws.handle);
    assert!(doc.to_string().find("A-1").is_none(), "the payload's words never ride the card: {doc}");

    let pushed: Vec<_> = crate::push_routes::test_push_capture::since(mark)
        .into_iter()
        .filter(|e| matches!(e, k2_core::push::PushEvent::ThreadPost(t) if matches!(&t.budget, k2_core::push::ThreadBudget::Approval { request_id } if *request_id == id)))
        .collect();
    assert_eq!(pushed.len(), 1, "one push to the asked person");
    let e = &pushed[0];
    assert_eq!(e.body(), "Approval requested: open K2 to read it");
    assert_eq!(e.data().get("approval"), Some("1"));

    // The person's device gets the bytes exactly as sent.
    let (s, g) = call(&person(&owner), routes::GET, false, json!({ "id": id }));
    assert_eq!(s, "200 OK", "{g}");
    let got = base64::engine::general_purpose::STANDARD.decode(g["payloadB64"].as_str().expect("b64")).expect("decode");
    assert_eq!(got, bytes);
    assert_eq!(g["requestedBy"], json!({ "agent": ws.handle, "reportedBy": "k2" }));
    let (_, p) = call(&person(&owner), routes::PENDING, false, json!({}));
    assert!(p["requests"].as_array().expect("list").iter().any(|r| r["id"] == id), "{p}");
}

// ── answers: people only ────────────────────────────────────────────

#[test]
fn an_agent_can_never_answer_its_own_request() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    let ws = seed("apself");
    grant(&ws);
    let bytes = payload(600);
    let id = id_of(&ask_ok(&ws, None, &bytes));
    let shell = Who::Shell { project_id: ws.id.clone(), session_id: "shell-1".into() };
    for who in [agent(&ws), shell, Who::Owner] {
        let (s, v) = call(&who, routes::ANSWER, true, approve_body(&id, &record(&bytes, KEY)));
        assert_eq!(s, "403 Forbidden", "{} answered: {v}", who.label());
        assert_eq!(v["error"]["code"], "approval_person_only");
        let (s, _) = call(&who, routes::GET, false, json!({ "id": id }));
        assert_eq!(s, "403 Forbidden", "{} read the bytes", who.label());
    }
    // Another person is "not found", never answered.
    let other = login(&home, "oth", Role::Admin);
    let (s, v) = call(&person(&other), routes::ANSWER, true, approve_body(&id, &record(&bytes, KEY)));
    assert_eq!(s, "404 Not Found", "{v}");
    let (_, st) = call(&agent(&ws), routes::STATUS, false, json!({ "id": id }));
    assert_eq!(st["state"], "waiting", "{st}");
}

#[test]
fn the_person_approves_and_the_agent_gets_the_record_as_posted() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    let ws = seed("apyes");
    grant(&ws);
    let bytes = payload(600);
    let v = ask_ok(&ws, None, &bytes);
    let id = id_of(&v);
    let card_id = v["card"]["id"].as_str().expect("card").to_string();

    // A record over other bytes, or with a key the payload doesn't list.
    let (s, e) = call(&person(&owner), routes::ANSWER, true, approve_body(&id, &record(&payload(600), KEY)));
    assert_eq!((s.as_str(), e["error"]["code"].as_str()), ("400 Bad Request", Some("approval_bad_record")), "{e}");
    let other_key = "414e70f4cee199b526c21e16f612fc746b65dcda3c35419059da92855cd82954";
    let (s, e) = call(&person(&owner), routes::ANSWER, true, approve_body(&id, &record(&bytes, other_key)));
    assert_eq!((s.as_str(), e["error"]["code"].as_str()), ("400 Bad Request", Some("approval_bad_record")), "{e}");

    let rec = record(&bytes, KEY);
    let (s, a) = call(&person(&owner), routes::ANSWER, true, approve_body(&id, &rec));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["state"], "approved");

    let (_, st) = call(&agent(&ws), routes::WAIT, false, json!({ "id": id, "timeout": "1" }));
    assert_eq!(st["state"], "approved", "{st}");
    assert_eq!(st["record"], serde_json::from_str::<Value>(&rec).expect("record json"));
    assert_eq!(st["recordText"], rec, "the record exactly as the phone posted it");
    assert_eq!(st["device"], "iPhone 16");
    assert_eq!(st["keyId"], KEY);
    let doc = serde_json::to_value(overlay::get_thread_doc(&ws.conv, &card_id).expect("read").expect("doc")).expect("json");
    assert_eq!(doc["approval"]["status"], "approved", "{doc}");
    assert_eq!(doc["approval"]["device"], "iPhone 16");
    assert!(card::sent_to(&ws.handle).iter().any(|l| l.starts_with(&format!("[approval:{id}] {owner} approved it"))));

    // A second answer is refused; the first stands.
    let (s, _) = call(&person(&owner), routes::ANSWER, true, json!({ "id": id, "decision": "decline" }));
    assert_eq!(s, "409 Conflict");
}

#[test]
fn a_decline_and_a_cancel_close_the_card() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    let ws = seed("apno");
    grant(&ws);
    let a = id_of(&ask_ok(&ws, None, &payload(600)));
    let (s, v) = call(&person(&owner), routes::ANSWER, true, json!({ "id": a, "decision": "decline" }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["state"], "declined");
    let b = ask_ok(&ws, None, &payload(600));
    let (s, v) = call(&agent(&ws), routes::CANCEL, true, json!({ "id": id_of(&b) }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["state"], "cancelled");
    let doc = serde_json::to_value(overlay::get_thread_doc(&ws.conv, b["card"]["id"].as_str().expect("card")).expect("read").expect("doc")).expect("json");
    assert_eq!(doc["approval"]["status"], "cancelled", "{doc}");
    // Another agent can't cancel or read it.
    let other = seed("apother");
    let (s, _) = call(&agent(&other), routes::CANCEL, true, json!({ "id": a }));
    assert_eq!(s, "404 Not Found");
    let (s, _) = call(&agent(&other), routes::STATUS, false, json!({ "id": a }));
    assert_eq!(s, "404 Not Found");
}

// ── expiry ──────────────────────────────────────────────────────────

#[test]
fn a_request_past_answer_by_expires_and_can_no_longer_be_answered() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    let ws = seed("apexp");
    grant(&ws);
    let bytes = payload(2);
    let v = ask_ok(&ws, None, &bytes);
    let id = id_of(&v);
    // The wait returns at answer_by with the expiry, not at its timeout.
    let started = std::time::Instant::now();
    let (_, st) = call(&agent(&ws), routes::WAIT, false, json!({ "id": id, "timeout": "20" }));
    assert_eq!(st["state"], "expired", "{st}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "woke at answer_by");
    let (s, e) = call(&person(&owner), routes::ANSWER, true, approve_body(&id, &record(&bytes, KEY)));
    assert_eq!(s, "409 Conflict", "{e}");
    assert_eq!(e["error"]["code"], "approval_not_waiting");
    let doc = serde_json::to_value(overlay::get_thread_doc(&ws.conv, v["card"]["id"].as_str().expect("card")).expect("read").expect("doc")).expect("json");
    assert_eq!(doc["approval"]["status"], "expired", "{doc}");
    assert_eq!(expiry::sweep(overlay::now_secs()), 0, "nothing left to expire for it");
}

// ── limits ──────────────────────────────────────────────────────────

#[test]
fn an_agent_has_at_most_three_waiting_and_a_nonce_is_carried_once() {
    let home = crate::test_support::TempHome::new();
    let owner = login(&home, "own", Role::Owner);
    link_owner(&home, &owner);
    let ws = seed("aplim");
    grant(&ws);
    let first = payload(600);
    ask_ok(&ws, None, &first);
    let (s, v) = ask(&ws, None, &first);
    assert_eq!(s, "409 Conflict", "{v}");
    assert_eq!(v["error"]["code"], "approval_duplicate_nonce");
    ask_ok(&ws, None, &payload(600));
    ask_ok(&ws, None, &payload(600));
    let (s, v) = ask(&ws, None, &payload(600));
    assert_eq!(s, "429 Too Many Requests", "{v}");
    assert_eq!(v["error"]["code"], "approval_too_many_waiting");
}
