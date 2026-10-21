//! A1 core tests (PRD rev 3.1 §13.2, §13.3): request → card → answer →
//! compare-and-set apply → outcome turn, on the test kit's actions. The
//! gate is passed (`routes::handle_gated`): the switch paths run in an
//! integration binary (lane B's rule: no `set_server_switch` in lib tests).
//! No test wakes a real agent: turns go to `inject`'s sink.

use std::collections::HashMap;

use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay;
use k2_core::permission::request::{PermissionRequest, RequestState};
use serde_json::{json, Value};

use super::routes::{self, handle_gated};
use super::{answer, expiry, inject, limits, store, testkit};
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
        rusqlite::params![id, handle, format!("/tmp/pc-a1-{handle}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

fn agent(ws: &Ws) -> Who {
    Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) }
}

fn call(who: &Who, path: &str, body: Value) -> (String, Value) {
    let is_post = routes::POST_ROUTES.contains(&path);
    let r = handle_gated(path, who, is_post, body.to_string().as_bytes(), &HashMap::new());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn ask(ws: &Ws, action: &str, extra: Value) -> (String, Value) {
    let mut body = json!({ "action": action, "why": "Run the suite on the minis" });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    call(&agent(ws), routes::REQUEST, body)
}

fn ask_ok(ws: &Ws, action: &str, extra: Value) -> Value {
    let (s, v) = ask(ws, action, extra);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["status"], "pending", "{v}");
    v
}

fn answer_as(author: &str, ws: &Ws, card: &str, body: Value, params: &[(&str, &str)]) -> (String, Value) {
    let mut b = json!({ "addr": ws.handle, "id": card });
    if let (Some(o), Some(e)) = (b.as_object_mut(), body.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &p, b.to_string().as_bytes(), author);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn row(id: &str) -> PermissionRequest {
    store::get_by_display_id(id).expect("db").unwrap_or_else(|| panic!("no request {id}"))
}

fn doc(ws: &Ws, card: &str) -> Value {
    serde_json::to_value(overlay::get_thread_doc(&ws.conv, card).expect("doc read").expect("card doc")).expect("doc json")
}

fn card_of(v: &Value) -> String {
    v["card"].as_str().unwrap_or_else(|| panic!("no card in {v}")).to_string()
}

fn login(name: &str, role: k2_core::connect_users::Role) -> String {
    let user = format!("{name}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&user, "password123").expect("add_user");
    if role != k2_core::connect_users::Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    user
}

// ── the happy path ──────────────────────────────────────────────────

#[test]
fn a_request_posts_a_permission_card_and_allow_applies_it_once() {
    let ws = seed("pca");
    let v = ask_ok(&ws, "test.switch", json!({}));
    let id = v["id"].as_str().expect("id").to_string();
    assert!(id.starts_with("pr_"), "{v}");
    assert_eq!(v["approvers"], "owner or admin");
    assert!(v["routedTo"].is_array(), "routedTo is [{{login, role}}]: {v}");
    assert_eq!(v["text"], format!("Let {} do test things.", ws.handle));
    let card = card_of(&v);
    let now = overlay::now_secs();

    let d = doc(&ws, &card);
    assert_eq!(d["via"], "permission", "{d}");
    assert_eq!(d["from"], ws.handle);
    assert_eq!(d["choice"]["prompt"], format!("{} asks: Let {} do test things (Allow test things).", ws.handle, ws.handle));
    assert_eq!(d["choice"]["options"], json!([{ "label": "Allow" }, { "label": "Deny" }]));
    assert_eq!(d["choice"]["action"]["id"], "permission");
    assert_eq!(d["choice"]["action"]["params"], json!({ "request": id }));
    let exp = d["choice"]["expiresAt"].as_i64().expect("expiresAt");
    assert!((exp - (now + 72 * 3600)).abs() <= 5, "72 h default, the request's own expiry: {exp}");
    assert_eq!(row(&id).expires_at, exp, "the request's expiry IS the card's");
    let view = &d["choice"]["action"]["view"];
    assert_eq!(view["kind"], "permission");
    assert_eq!(view["v"], 1);
    assert_eq!(view["state"], "pending");
    assert_eq!(view["requestId"], id);
    assert_eq!(view["setting"], "Allow test things");
    assert_eq!(view["why"], json!({ "by": ws.handle, "text": "Run the suite on the minis" }));
    assert_eq!(view["approvers"]["floor"], "admin");
    assert_eq!(view["viewer"]["mode"], "wait", "the stored viewer is neutral");
    assert_eq!(view["values"], json!([{ "key": "workspace", "value": ws.handle }]));

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "client": "phone" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["status"], "answered");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(testkit::applied(&ws.id, "test.switch"), 1);
    assert_eq!(testkit::last_actor(&ws.id, "test.switch").as_deref(), Some("owner"), "applied as the answerer");
    let r = row(&id);
    assert_eq!(r.state, RequestState::Approved);
    assert_eq!((r.answered_by.as_deref(), r.answered_via.as_deref()), (Some("owner"), Some("phone")));

    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["status"], "answered");
    assert_eq!(d["choice"]["answer"], "Allow");
    let view = &d["choice"]["action"]["view"];
    assert_eq!(view["state"], "approved", "{d}");
    assert_eq!(view["answer"]["decision"], "allow");
    assert_eq!(view["undo"]["label"], "Turn off again");
    assert_eq!(
        d["choice"]["action"]["result"]["message"],
        format!("Allowed. Turn it off again in Settings → Workspaces → {} → Agent.", ws.handle)
    );

    let turns = inject::sent_to(&ws.handle);
    assert_eq!(
        turns,
        vec![format!(
            "[permission:{id}] approved by owner: Allow test things is now on for {}, until someone turns it off. Run your command again.",
            ws.handle
        )],
        "one turn, never the command line"
    );

    // Single use: a second answer is refused and applies nothing.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }), &[]);
    assert_eq!(s, "409 Conflict", "{a}");
    assert_eq!(testkit::applied(&ws.id, "test.switch"), 1);
}

#[test]
fn two_allows_at_once_apply_once_with_one_audit_line() {
    let home = crate::test_support::TempHome::new();
    let ws = seed("pccas");
    let v = ask_ok(&ws, "test.slow", json!({}));
    let id = v["id"].as_str().expect("id").to_string();
    let card = card_of(&v);
    let mut handles = Vec::new();
    for _ in 0..2 {
        let (h, c, conv, wsid) = (ws.handle.clone(), card.clone(), ws.conv.clone(), ws.id.clone());
        handles.push(std::thread::spawn(move || {
            let w = Ws { id: wsid, handle: h, conv };
            answer_as("owner", &w, &c, json!({ "answer": "Allow" }), &[]).0
        }));
    }
    let mut statuses: Vec<String> = handles.into_iter().map(|h| h.join().expect("thread")).collect();
    statuses.sort();
    assert_eq!(statuses, ["200 OK", "409 Conflict"], "exactly one wins");
    assert_eq!(testkit::applied(&ws.id, "test.slow"), 1, "exactly one apply");
    let audit = std::fs::read_to_string(home.path().join(".k2/auth-audit.jsonl")).expect("audit file");
    let lines: Vec<&str> = audit.lines().filter(|l| l.contains(&id) && l.contains("permission-answer")).collect();
    assert_eq!(lines.len(), 1, "one answer audit line: {audit}");
    assert!(lines[0].contains("via=owner-token"), "{}", lines[0]);
    assert_eq!(row(&id).state, RequestState::Approved);
}

// ── the current state (Rosson Q1) ───────────────────────────────────

#[test]
fn already_on_at_the_ask_stores_nothing() {
    let ws = seed("pcad");
    testkit::set_on(&ws.id, "test.switch", true);
    let (s, v) = ask(&ws, "test.switch", json!({}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["status"], "already_done");
    assert!(store::for_workspace(&ws.id, 10).expect("db").is_empty(), "nothing stored");
}

#[test]
fn already_on_at_the_click_closes_the_card_as_a_no_op() {
    let ws = seed("pcac");
    let v = ask_ok(&ws, "test.switch", json!({}));
    let id = v["id"].as_str().unwrap().to_string();
    let card = card_of(&v);
    // Turned on in Settings meanwhile.
    testkit::set_on(&ws.id, "test.switch", true);
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "already_done");
    assert_eq!(testkit::applied(&ws.id, "test.switch"), 0, "nothing applied");
    let r = row(&id);
    assert_eq!(r.state, RequestState::AlreadyDone);
    assert!(!r.is_answerable());
    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["status"], "answered");
    assert_eq!(d["choice"]["action"]["view"]["state"], "already_done");
    assert_eq!(d["choice"]["action"]["result"], json!({ "message": "Already on. Nothing was applied.", "outcome": "already_done" }));
    assert_eq!(
        inject::sent_to(&ws.handle),
        vec![format!(
            "[permission:{id}] already on: Allow test things was already on for {}. Nothing was applied. Run your command again.",
            ws.handle
        )]
    );
}

#[test]
fn close_answers_an_already_done_card_and_only_that() {
    let ws = seed("pccl");
    let v = ask_ok(&ws, "test.switch", json!({}));
    let id = v["id"].as_str().unwrap().to_string();
    let card = card_of(&v);
    // Not satisfied: Close isn't an answer; the request stays open.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Close" }), &[]);
    assert_eq!(s, "400 Bad Request", "{a}");
    assert_eq!(row(&id).state, RequestState::Pending, "reverted, still answerable");
    assert!(row(&id).is_answerable());
    // A3's sweep stores already_done; Close then closes it.
    testkit::set_on(&ws.id, "test.switch", true);
    assert!(store::set_open_state(&id, RequestState::AlreadyDone).expect("db"));
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Close" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["answer"], "Close");
    assert_eq!(d["choice"]["action"]["view"]["answer"]["decision"], "close");
    assert_eq!(d["choice"]["action"]["result"]["outcome"], "already_done");
    assert_eq!(testkit::applied(&ws.id, "test.switch"), 0);
}

#[test]
fn values_that_moved_or_reworded_text_are_stale_and_nothing_applies() {
    let ws = seed("pcst");
    let v = ask_ok(&ws, "test.moved", json!({}));
    let id = v["id"].as_str().unwrap().to_string();
    testkit::set_moved(&ws.id, true);
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Stale);
    assert_eq!(testkit::applied(&ws.id, "test.moved"), 0);
    assert_eq!(
        inject::sent_to(&ws.handle).last().cloned(),
        Some(format!("[permission:{id}] stale: the widget changed before the click. Nothing was applied."))
    );

    // A card whose K2 text changed between the ask and the click (C35).
    let ws2 = seed("pcsh");
    let v = ask_ok(&ws2, "test.switch", json!({}));
    let id2 = v["id"].as_str().unwrap().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE permission_requests SET text_hash = 'x' WHERE id = ?1", rusqlite::params![id2]).unwrap();
    }
    let (s, a) = answer_as("owner", &ws2, &card_of(&v), json!({ "answer": "Allow" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id2).state, RequestState::Stale);
    assert_eq!(testkit::applied(&ws2.id, "test.switch"), 0);
}

#[test]
fn a_refused_apply_fails_the_request_and_closes_the_card() {
    let ws = seed("pcfl");
    let v = ask_ok(&ws, "test.fail", json!({}));
    let id = v["id"].as_str().unwrap().to_string();
    let card = card_of(&v);
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Failed);
    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["status"], "answered");
    assert_eq!(d["choice"]["action"]["view"]["state"], "failed");
    assert_eq!(
        inject::sent_to(&ws.handle).last().cloned(),
        Some(format!("[permission:{id}] failed: the App shop no longer exists. Nothing was applied."))
    );
}

// ── deny, the 24 h block, update ────────────────────────────────────

#[test]
fn deny_with_a_reason_blocks_the_same_ask_for_a_day_and_update_reopens_once() {
    let ws = seed("pcdn");
    let v = ask_ok(&ws, "test.switch", json!({}));
    let id = v["id"].as_str().unwrap().to_string();
    let card = card_of(&v);
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Deny", "reason": "Use the minis tonight" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(testkit::applied(&ws.id, "test.switch"), 0);
    let r = row(&id);
    assert_eq!((r.state, r.reason.as_deref()), (RequestState::Denied, Some("Use the minis tonight")));
    let line = inject::sent_to(&ws.handle).last().cloned().expect("a turn");
    assert!(line.starts_with(&format!("[permission:{id}] denied by owner: \"Use the minis tonight.\" Asking again is blocked until ")), "{line}");
    assert_eq!(doc(&ws, &card)["choice"]["action"]["view"]["answer"]["reason"], "Use the minis tonight");

    let (s, v2) = ask(&ws, "test.switch", json!({}));
    assert_eq!(s, "409 Conflict", "{v2}");
    assert_eq!(v2["error"]["code"], "recently_denied");
    assert_eq!(v2["denied"]["by"], "owner");
    assert_eq!(v2["denied"]["reason"], "Use the minis tonight");
    assert_eq!(v2["denied"]["id"], id);

    // update: once, on the denied request → a new card, pending again.
    let (s, u) = call(&agent(&ws), routes::UPDATE, json!({ "id": id, "why": "The nightly job moved to 2am" }));
    assert_eq!(s, "200 OK", "{u}");
    let card2 = u["card"].as_str().expect("new card").to_string();
    assert_ne!(card2, card);
    let r = row(&id);
    assert_eq!(r.state, RequestState::Pending);
    assert_eq!(r.updated_why.as_deref(), Some("The nightly job moved to 2am"));
    assert_eq!(doc(&ws, &card2)["choice"]["action"]["view"]["why"]["text"], "The nightly job moved to 2am");
    let (s, a) = answer_as("owner", &ws, &card2, json!({ "answer": "Deny" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    let (s, u) = call(&agent(&ws), routes::UPDATE, json!({ "id": id, "why": "again" }));
    assert_eq!(s, "409 Conflict", "{u}");
    assert_eq!(u["error"]["code"], "already_updated");
}

// ── who may answer (§6.1, §13.2) ────────────────────────────────────

#[test]
fn only_people_at_the_floor_answer() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("pcfr");
    let member = login("pcm", k2_core::connect_users::Role::Member);
    let admin = login("pca", k2_core::connect_users::Role::Admin);

    let v = ask_ok(&ws, "test.owner-switch", json!({}));
    let card = card_of(&v);
    let (s, a) = answer_as(&member, &ws, &card, json!({ "answer": "Allow" }), &[]);
    assert_eq!((s.as_str(), &a["error"]["code"], &a["error"]["required"]), ("403 Forbidden", &json!("action_card_person_only"), &json!("owner")), "{a}");
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }), &[]);
    assert_eq!((s.as_str(), &a["error"]["required"]), ("403 Forbidden", &json!("owner")), "an Admin on an Owner row: {a}");
    // An agent passport, a cell, a Garden widget: never.
    for p in [
        vec![(crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"), ("project_id", ws.id.as_str()), ("from", ws.handle.as_str())],
        vec![("cell_session_id", "sess-x")],
        vec![("origin", "{\"widget\":\"w\",\"garden\":\"g\"}")],
    ] {
        let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }), &p);
        assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("action_card_person_only")), "{p:?}: {a}");
    }
    assert_eq!(testkit::applied(&ws.id, "test.owner-switch"), 0);

    // An Admin answers an Admin row, as itself.
    let v = ask_ok(&ws, "test.switch", json!({}));
    let (s, a) = answer_as(&member, &ws, &card_of(&v), json!({ "answer": "Allow" }), &[]);
    assert_eq!((s.as_str(), &a["error"]["required"]), ("403 Forbidden", &json!("admin")), "{a}");
    let (s, a) = answer_as(&admin, &ws, &card_of(&v), json!({ "answer": "Allow" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(testkit::last_actor(&ws.id, "test.switch"), Some(format!("connect:{admin}")));
    assert_eq!(row(v["id"].as_str().unwrap()).answered_via.as_deref(), Some("web"));
}

#[test]
fn a_confirm_action_takes_its_token_and_a_wrong_one_decides_nothing() {
    let ws = seed("pccf");
    let v = ask_ok(&ws, "test.confirm", json!({ "params": { "request": "ar_22" } }));
    assert_eq!(v["id"], "ar_22", "a held request keeps its own id: {v}");
    let card = card_of(&v);
    let view = doc(&ws, &card)["choice"]["action"]["view"].clone();
    assert_eq!(view["proof"], json!({ "kind": "confirm", "action": "app-approve", "secretId": "ar_22" }));
    assert_eq!(view["grant"], "one-time");
    assert!(view["undo"].is_null());

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }), &[]);
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": "stolen" }), &[]);
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert!(row("ar_22").is_answerable(), "a wrong confirm leaves it open");
    assert_eq!(doc(&ws, &card)["choice"]["status"], "pending");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": "good-token" }), &[]);
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(testkit::applied(&ws.id, "test.confirm"), 1);
    assert_eq!(
        inject::sent_to(&ws.handle).last().cloned(),
        Some(format!("[permission:ar_22] approved by owner: once, for {}. It is running now. Run your command again.", ws.handle))
    );
}

// ── batches (§4.5) ──────────────────────────────────────────────────

#[test]
fn asks_within_five_minutes_share_one_batch_card_and_allow_all_skips_confirm_items() {
    let ws = seed("pcbt");
    let a = ask_ok(&ws, "test.owner-switch", json!({}));
    let card = card_of(&a);
    let b = ask_ok(&ws, "test.confirm", json!({ "params": { "request": "ar_77" } }));
    assert_eq!(card_of(&b), card, "joined the open card: {b}");
    assert_eq!(b["batch"], true);
    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["action"]["id"], "permission.batch");
    assert_eq!(d["choice"]["action"]["params"]["requests"], json!([a["id"], "ar_77"]));
    assert_eq!(d["choice"]["options"], json!([{ "label": "Allow all" }, { "label": "Deny all" }]));
    assert_eq!(d["choice"]["prompt"], format!("{} asks for 2 permissions.", ws.handle));
    let view = &d["choice"]["action"]["view"];
    assert_eq!((view["kind"].as_str(), view["state"].as_str()), (Some("batch"), Some("pending")));
    assert_eq!(view["items"].as_array().map(Vec::len), Some(2));

    // An Admin floor ask goes on its own card (another approver set).
    let c = ask_ok(&ws, "test.switch", json!({}));
    assert_ne!(card_of(&c), card);

    let (s, r) = answer_as("owner", &ws, &card, json!({ "answer": "Allow all" }), &[]);
    assert_eq!(s, "200 OK", "{r}");
    assert_eq!(r["status"], "pending", "the confirm item is still waiting: {r}");
    assert_eq!(testkit::applied(&ws.id, "test.owner-switch"), 1);
    assert_eq!(testkit::applied(&ws.id, "test.confirm"), 0, "Allow all never covers a confirm item");
    let (s, r) = answer_as("owner", &ws, &card, json!({ "item": "ar_77", "answer": "Allow", "confirmToken": "good-token" }), &[]);
    assert_eq!(s, "200 OK", "{r}");
    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["status"], "answered");
    assert_eq!(d["choice"]["action"]["view"]["state"], "done");
    assert_eq!(inject::sent_to(&ws.handle).len(), 2, "one turn per item");
}

#[test]
fn deny_all_closes_every_item() {
    let ws = seed("pcda");
    let a = ask_ok(&ws, "test.mode", json!({ "params": { "mode": "approval" } }));
    let b = ask_ok(&ws, "test.mode", json!({ "params": { "mode": "free", "cores": "db,files" } }));
    let card = card_of(&a);
    assert_eq!(card_of(&b), card);
    let (s, r) = answer_as("owner", &ws, &card, json!({ "answer": "Deny all" }), &[]);
    assert_eq!(s, "200 OK", "{r}");
    for v in [&a, &b] {
        assert_eq!(row(v["id"].as_str().unwrap()).state, RequestState::Denied);
    }
    assert_eq!(doc(&ws, &card)["choice"]["status"], "answered");
    assert_eq!(row(b["id"].as_str().unwrap()).params, json!({ "mode": "free", "cores": ["db", "files"] }));
}

// ── expiry, cancel, switch-off (§2.5, §3.3, C20) ────────────────────

#[test]
fn an_expired_card_expires_its_request_and_wakes_the_agent_once() {
    let ws = seed("pcex");
    let v = ask_ok(&ws, "test.switch", json!({}));
    let id = v["id"].as_str().unwrap().to_string();
    let card = card_of(&v);
    let cb = overlay::expire_card(&ws.conv, &card).expect("expire");
    assert_eq!(expiry::on_cards_expired(&[cb.clone()]), 1);
    assert_eq!(expiry::on_cards_expired(&[cb]), 0, "once");
    assert_eq!(row(&id).state, RequestState::Expired);
    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["status"], "expired");
    assert_eq!(d["choice"]["action"]["view"]["state"], "expired");
    assert_eq!(
        inject::sent_to(&ws.handle),
        vec![format!("[permission:{id}] expired unanswered after 3 days. Still need it? Ask once more.")]
    );
}

/// A request past its time whose card is still open: the request sweep
/// closes both. (A scoped DB: the sweep walks every open request.)
#[test]
fn the_request_sweep_expires_a_request_past_its_time() {
    let _db = k2_core::db::scoped_for_test();
    let w2 = seed("pcex2");
    let v = ask_ok(&w2, "test.switch", json!({}));
    let id2 = v["id"].as_str().unwrap().to_string();
    let later = overlay::now_secs() + 73 * 3600;
    assert!(expiry::sweep_requests(later) >= 1);
    assert_eq!(row(&id2).state, RequestState::Expired);
    assert_eq!(doc(&w2, &card_of(&v))["choice"]["status"], "expired");
}

#[test]
fn the_agent_withdraws_its_request_and_gets_no_turn() {
    let ws = seed("pccn");
    let v = ask_ok(&ws, "test.switch", json!({}));
    let id = v["id"].as_str().unwrap().to_string();
    let (s, c) = call(&agent(&ws), routes::CANCEL, json!({ "id": id }));
    assert_eq!(s, "200 OK", "{c}");
    assert_eq!(c["state"], "cancelled");
    let d = doc(&ws, &card_of(&v));
    assert_eq!(d["choice"]["status"], "voided");
    assert_eq!(d["choice"]["action"]["view"]["state"], "cancelled");
    assert!(inject::sent_to(&ws.handle).is_empty());
    let (s, c) = call(&agent(&ws), routes::CANCEL, json!({ "id": id }));
    assert_eq!((s.as_str(), &c["error"]["code"]), ("409 Conflict", &json!("request_not_pending")));
    // Another workspace can't see or touch it.
    let other = seed("pccx");
    let (s, _) = call(&agent(&other), routes::STATUS, json!({ "id": id }));
    assert_eq!(s, "404 Not Found");
}

#[test]
fn turning_the_switch_off_closes_open_requests_with_one_turn_each() {
    let ws = seed("pcso");
    let a = ask_ok(&ws, "test.switch", json!({}));
    let b = ask_ok(&ws, "test.owner-switch", json!({}));
    assert_eq!(answer::close_for_switch_off(Some(&ws.id)), 2);
    for v in [&a, &b] {
        let id = v["id"].as_str().unwrap();
        assert_eq!(row(id).state, RequestState::Closed);
        let d = doc(&ws, &card_of(v));
        assert_eq!(d["choice"]["status"], "voided");
        assert_eq!(d["choice"]["action"]["view"]["state"], "closed");
    }
    let turns = inject::sent_to(&ws.handle);
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert!(turns[0].ends_with("closed: permission cards were turned off on this server. Nothing was applied. Ask your human the usual way."));
    assert_eq!(answer::close_for_switch_off(Some(&ws.id)), 0);
}

// ── what the agent sends (§3.2) ─────────────────────────────────────

#[test]
fn bad_asks_are_refused_with_their_codes() {
    let ws = seed("pcbad");
    let cases: Vec<(Value, &str, &str)> = vec![
        (json!({ "action": "no.such", "why": "x" }), "400 Bad Request", "bad_action"),
        (json!({ "why": "x" }), "400 Bad Request", "bad_action"),
        (json!({ "action": "test.switch" }), "400 Bad Request", "bad_why"),
        (json!({ "action": "test.switch", "why": "a".repeat(201) }), "400 Bad Request", "bad_why"),
        (json!({ "action": "test.switch", "why": "two\nlines" }), "400 Bad Request", "bad_why"),
        (json!({ "action": "test.switch", "why": "key sk-ant-api03-abcdefghijklmnop" }), "400 Bad Request", "bad_why"),
        (json!({ "action": "test.switch", "why": "x", "params": { "to": "y" } }), "400 Bad Request", "bad_params"),
        (json!({ "action": "test.mode", "why": "x", "params": { "workspace": "other" } }), "400 Bad Request", "bad_params"),
        (json!({ "action": "test.mode", "why": "x" }), "400 Bad Request", "bad_params"),
        (json!({ "action": "test.mode", "why": "x", "params": { "mode": "loud" } }), "400 Bad Request", "bad_params"),
        (json!({ "action": "test.never", "why": "x" }), "403 Forbidden", "not_cardable"),
    ];
    for (body, status, code) in cases {
        let (s, v) = call(&agent(&ws), routes::REQUEST, body.clone());
        assert_eq!((s.as_str(), &v["error"]["code"]), (status, &json!(code)), "{body}: {v}");
    }
    let (_, v) = call(&agent(&ws), routes::REQUEST, json!({ "action": "test.never", "why": "x" }));
    assert_eq!(v["permission"], json!({ "requestable": false, "where": "a terminal outside K2" }));
    assert!(v["error"]["hint"].as_str().unwrap().ends_with("It can't be requested."), "{v}");
    let (_, v) = call(&agent(&ws), routes::REQUEST, json!({ "action": "test.covered", "why": "x" }));
    assert_eq!(v["permission"]["action"], "test.switch", "a covered row names its covering action: {v}");
    assert!(store::for_workspace(&ws.id, 10).expect("db").is_empty(), "nothing stored");

    // dryRun: the words, nothing stored, no card.
    let (s, v) = call(&agent(&ws), routes::REQUEST, json!({ "action": "test.switch", "why": "x", "dryRun": true }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["status"], "dry_run");
    assert_eq!(v["expiresIn"], 72 * 3600);
    assert!(store::for_workspace(&ws.id, 10).expect("db").is_empty());
}

#[test]
fn the_same_ask_dedupes_and_the_limits_hold() {
    let ws = seed("pclim");
    let a = ask_ok(&ws, "test.switch", json!({}));
    let (s, v) = ask(&ws, "test.switch", json!({}));
    assert_eq!(s, "200 OK");
    assert_eq!((v["status"].as_str(), v["id"].as_str()), (Some("already_pending"), a["id"].as_str()), "{v}");
    // 5 pending per workspace.
    for p in [json!({ "mode": "approval" }), json!({ "mode": "free" }), json!({ "mode": "free", "cores": "db" }), json!({ "mode": "approval", "cores": "db" })] {
        ask_ok(&ws, "test.mode", json!({ "params": p }));
    }
    let (s, v) = ask(&ws, "test.mode", json!({ "params": { "mode": "approval", "cores": "files" } }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("429 Too Many Requests", &json!("too_many_requests")), "{v}");

    // A third ask of the same thing within 7 days.
    let w2 = seed("pclim2");
    for _ in 0..2 {
        let v = ask_ok(&w2, "test.owner-switch", json!({}));
        answer_as("owner", &w2, &card_of(&v), json!({ "answer": "Deny" }), &[]);
        let db = k2_core::db::shared();
        let conn = db.lock();
        // The denial was two days ago: the 24 h block is over.
        conn.execute(
            "UPDATE permission_requests SET answered_at = answered_at - 172800, created_at = created_at - 172800 WHERE workspace_id = ?1",
            rusqlite::params![w2.id],
        )
        .unwrap();
    }
    let (s, v) = ask(&w2, "test.owner-switch", json!({}));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("ask_limit")), "{v}");
}

#[test]
fn fifty_open_requests_on_the_server_refuse_the_next() {
    let _db = k2_core::db::scoped_for_test();
    let now = overlay::now_secs();
    for i in 0..50 {
        store::insert(&PermissionRequest {
            id: k2_core::permission::request::RequestId::new(),
            workspace_id: format!("ws-{i}"),
            session_id: None,
            conversation_id: None,
            addr: None,
            card_id: None,
            action: "test.switch".into(),
            params: json!({}),
            params_hash: format!("h{i}"),
            text_hash: "t".into(),
            why: "x".into(),
            state: RequestState::Pending,
            floor: "admin".into(),
            held_ref: None,
            batch_id: None,
            ask_number: 1,
            asked_for: None,
            created_at: now - 7200,
            expires_at: now + 3600,
            answered_at: None,
            answered_by: None,
            answered_via: None,
            reason: None,
            result_message: None,
            updated_why: None,
            updated_at: None,
        })
        .expect("insert");
    }
    let e = limits::check_ask("ws-new", "test.switch", "h", now).expect_err("the 51st");
    assert_eq!((e.status, e.code), (429, "too_many_requests"));
    assert!(limits::check_ask("ws-new", "test.switch", "h", now).is_err());
}

#[test]
fn people_are_not_served_the_agent_verbs() {
    let (s, v) = call(&Who::Owner, routes::REQUEST, json!({ "action": "test.switch", "why": "x" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("forbidden")), "{v}");
    let (s, v) = call(&Who::Owner, routes::ENABLED, json!({}));
    assert_eq!(s, "200 OK");
    assert_eq!(v["enabled"], true);
    // A GET on a POST verb is 405; a POST on a GET verb too.
    let ws = seed("pcmeth");
    let r = handle_gated(routes::REQUEST, &agent(&ws), false, b"{}", &HashMap::new());
    assert_eq!(r.status, "405 Method Not Allowed");
    let r = handle_gated(routes::STATUS, &agent(&ws), true, b"{}", &HashMap::new());
    assert_eq!(r.status, "405 Method Not Allowed");
}

#[test]
fn todays_bodies_match_the_off_goldens() {
    use routes::{today, Transport};
    let r = today(Transport::Tcp, true);
    assert_eq!((r.status, r.body.as_str()), ("405 Method Not Allowed", r#"{"error":"method not allowed for this route"}"#));
    for r in [today(Transport::Tcp, false), today(Transport::Cell, true), today(Transport::Cell, false)] {
        assert_eq!((r.status, r.body.as_str()), ("403 Forbidden", r#"{"error":"Invalid or missing auth token"}"#));
    }
}

// ── GET /cli/permission/agents (people; D2's Settings page) ─────────

#[test]
fn the_agents_route_lists_switches_and_recent_requests_for_owners_and_admins() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("pcag");
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws.id, true).expect("switch on");
    let v = ask_ok(&ws, "test.switch", json!({}));
    let id = v["id"].as_str().unwrap().to_string();
    let admin = login("pcaga", k2_core::connect_users::Role::Admin);
    let member = login("pcagm", k2_core::connect_users::Role::Member);
    let admin_who = Who::Login { username: admin.clone(), role: k2_core::connect_users::Role::Admin };

    // Every agent (server-wide), and this one with `workspace=` (id or handle).
    let (s, all) = call(&Who::Owner, routes::AGENTS, json!({}));
    assert_eq!(s, "200 OK", "{all}");
    let mine = all["agents"].as_array().expect("agents").iter().find(|a| a["workspace"] == ws.id).cloned().expect("listed");
    assert_eq!(mine, json!({ "workspace": ws.id, "handle": ws.handle, "name": ws.handle, "on": true }));
    for token in [ws.id.as_str(), ws.handle.as_str()] {
        let (s, one) = call(&admin_who, routes::AGENTS, json!({ "workspace": token }));
        assert_eq!(s, "200 OK", "{token}: {one}");
        assert_eq!(one["agents"].as_array().map(Vec::len), Some(1), "{one}");
        let r = &one["recent"][0];
        assert_eq!((r["id"].as_str(), r["agent"].as_str(), r["workspace"].as_str()), (Some(id.as_str()), Some(ws.handle.as_str()), Some(ws.id.as_str())));
        assert_eq!((r["setting"].as_str(), r["state"].as_str(), r["action"].as_str()), (Some("Allow test things"), Some("pending"), Some("test.switch")));
        assert_eq!(r["card"], v["card"]);
        assert_eq!(r["addr"], ws.handle);
    }
    let (s, e) = call(&Who::Owner, routes::AGENTS, json!({ "workspace": "no-such-workspace-pcag" }));
    assert_eq!((s.as_str(), &e["error"]["code"]), ("404 Not Found", &json!("workspace_not_found")));

    // A Member and any passport are refused; it's a GET.
    let (s, e) = call(&Who::Login { username: member, role: k2_core::connect_users::Role::Member }, routes::AGENTS, json!({}));
    assert_eq!((s.as_str(), &e["error"]["code"]), ("403 Forbidden", &json!("role_required")), "{e}");
    let (s, e) = call(&agent(&ws), routes::AGENTS, json!({}));
    assert_eq!((s.as_str(), &e["error"]["code"]), ("403 Forbidden", &json!("forbidden")), "a passport: {e}");
    let r = handle_gated(routes::AGENTS, &Who::Owner, true, b"{}", &HashMap::new());
    assert_eq!(r.status, "405 Method Not Allowed");
}
