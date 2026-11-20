//! 0.46.17: who a card asks (`--to`, the owner's `permissionApprovers`),
//! on A1's test-kit actions (`test.switch` is Admin floor,
//! `test.owner-switch` Owner floor). The gate is passed
//! (`routes::handle_gated`); pushes go to `push_routes::test_push_capture`;
//! no test wakes a real agent.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay;
use k2_core::permission::card::Viewer;
use k2_core::permission::CARDS_CONTRACT;
use k2_core::push::PushEvent;
use serde_json::{json, Value};

use crate::compute_routes::Who;
use crate::permission::routes::{self, handle_gated};
use crate::permission::{approvers, store, view, waiting};
use crate::push_routes::test_push_capture;

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
        rusqlite::params![id, handle, format!("/tmp/pc-to-{handle}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

fn agent(ws: &Ws) -> Who {
    Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) }
}

fn ask_raw(ws: &Ws, action: &str, to: Option<Value>) -> (String, Value) {
    let mut body = json!({ "action": action, "why": "Run the suite on the minis", "params": {} });
    if let Some(t) = to {
        body["to"] = t;
    }
    let r = handle_gated(routes::REQUEST, &agent(ws), true, body.to_string().as_bytes(), &HashMap::new());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn ask(ws: &Ws, action: &str, to: Option<Value>) -> Value {
    let (s, v) = ask_raw(ws, action, to);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["status"], "pending", "{v}");
    v
}

fn login(name: &str, role: Role) -> String {
    let user = format!("{name}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&user, "password123").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    user
}

fn phone(user: &str) {
    k2_core::push_devices::upsert_device(&format!("dev-{user}"), "apns", &format!("tok-{user}"), user).expect("device");
}

/// Who was pushed for this conversation since `mark` (people keys).
fn pushed(mark: usize, conv: &str) -> Vec<String> {
    let mut out: Vec<String> = test_push_capture::since(mark)
        .into_iter()
        .filter_map(|e| match e {
            PushEvent::ThreadPost(t) if t.conversation_id == conv => Some(t.principal),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

fn tickets(ws: &Ws) -> Vec<k2_core::feedback::FeedbackItem> {
    k2_core::feedback::list_for_project(&ws.id, &k2_core::feedback::ListFilter::All).expect("tickets")
}

fn routed(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v["routedTo"].as_array().expect("routedTo").iter().map(|x| x["login"].as_str().unwrap().to_string()).collect();
    out.sort();
    out
}

fn waiting_ids(who: &Who, id: &str) -> usize {
    let r = waiting::pending(who);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    v["requests"].as_array().expect("rows").iter().filter(|x| x["requestId"] == id).count()
}

/// The card as `viewer` sees it: its `view.viewer.mode`.
fn mode_for(ws: &Ws, card: &str, viewer: Viewer) -> String {
    let doc = overlay::get_thread_doc(&ws.conv, card).expect("read").expect("card");
    let seen = view::project(&doc, &viewer, CARDS_CONTRACT);
    let v = serde_json::to_value(seen).expect("json");
    v["choice"]["action"]["view"]["viewer"]["mode"].as_str().unwrap_or_else(|| panic!("no mode: {v}")).to_string()
}

fn answer_as(author: &str, ws: &Ws, card: &str, answer: &str) -> (String, Value) {
    let b = json!({ "addr": ws.handle, "id": card, "answer": answer });
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), b.to_string().as_bytes(), author);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn set_default(names: Value) {
    k2_core::app_settings::update(json!({ "permissionApprovers": names })).expect("permissionApprovers");
}

#[test]
fn to_asks_only_the_people_it_names() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let ada = login("toada", Role::Admin);
    let bo = login("tobo", Role::Admin);
    let own = login("toown", Role::Owner);
    for u in [&ada, &bo, &own] {
        phone(u);
    }
    let ws = seed("toname");
    let mark = test_push_capture::mark();
    let v = ask(&ws, "test.switch", Some(json!(ada.clone())));
    let (id, card) = (v["id"].as_str().unwrap().to_string(), v["card"].as_str().unwrap().to_string());
    assert_eq!(routed(&v), vec![ada.clone()], "{v}");
    assert_eq!(pushed(mark, &ws.conv), vec![ada.clone()], "only ada is pushed: never bo, the Owner login or the owner person");
    let t = tickets(&ws);
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(t[0].assignees, vec![ada.clone()], "the Tickets item is ada's alone");

    // On the card: ada gets Allow and Deny; bo and the Owner login wait.
    assert_eq!(mode_for(&ws, &card, Viewer::Login { user: ada.clone(), role: "admin".into() }), "approve");
    assert_eq!(mode_for(&ws, &card, Viewer::Login { user: bo.clone(), role: "admin".into() }), "wait");
    assert_eq!(mode_for(&ws, &card, Viewer::Login { user: own.clone(), role: "owner".into() }), "wait");
    assert_eq!(mode_for(&ws, &card, Viewer::OwnerToken), "approve", "the owner token is the server itself");
    let stored = overlay::get_thread_doc(&ws.conv, &card).expect("read").expect("card");
    let stored = serde_json::to_value(stored).expect("json");
    assert_eq!(stored["choice"]["action"]["view"]["approvers"]["names"], json!([ada.clone()]), "{stored}");

    // "Waiting on you": ada's, the owner token's, never bo's.
    assert_eq!(waiting_ids(&Who::Login { username: ada.clone(), role: Role::Admin }, &id), 1);
    assert_eq!(waiting_ids(&Who::Login { username: bo.clone(), role: Role::Admin }, &id), 0);
    assert_eq!(waiting_ids(&Who::Login { username: own.clone(), role: Role::Owner }, &id), 0);
    assert_eq!(waiting_ids(&Who::Owner, &id), 1);

    // It narrows who is asked, never who may answer: bo still may.
    let (s, a) = answer_as(&bo, &ws, &card, "Allow");
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");

    // `owner` names the owner token's person, and a list.
    let ws2 = seed("tonameb");
    let mark = test_push_capture::mark();
    let v = ask(&ws2, "test.owner-switch", Some(json!(format!("owner,{own}"))));
    assert_eq!(routed(&v), vec!["owner".to_string(), own.clone()], "sorted: {v}");
    assert_eq!(pushed(mark, &ws2.conv), vec![own.clone()], "own's phone (the owner person has none here)");
}

#[test]
fn an_unknown_or_ineligible_name_is_refused_and_nothing_is_asked() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let ada = login("toxa", Role::Admin);
    let mem = login("toxm", Role::Member);
    let ws = seed("torefuse");
    let refused = |action: &str, to: Value, code: &str| {
        let (s, v) = ask_raw(&ws, action, Some(to.clone()));
        assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some(code)), "{to}: {v}");
    };
    refused("test.switch", json!("nobody-here"), approvers::code::UNKNOWN);
    refused("test.switch", json!(format!("{ada},nobody-here")), approvers::code::UNKNOWN);
    refused("test.switch", json!(mem.clone()), approvers::code::NOT_ELIGIBLE);
    // An Admin can't answer an Owner-floor card.
    refused("test.owner-switch", json!(ada.clone()), approvers::code::NOT_ELIGIBLE);
    refused("test.switch", json!(7), approvers::code::BAD);
    assert!(store::open(Some(&ws.id)).expect("open").is_empty(), "nothing was asked");
    // A dry run lists who it would ask.
    let body = json!({ "action": "test.switch", "why": "x", "params": {}, "to": ada, "dryRun": true });
    let r = handle_gated(routes::REQUEST, &agent(&ws), true, body.to_string().as_bytes(), &HashMap::new());
    let v: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!((r.status, v["status"].as_str()), ("200 OK", Some("dry_run")), "{v}");
    assert_eq!(routed(&v), vec![ada.clone()]);
}

#[test]
fn the_owners_default_asks_its_people_and_an_owner_card_none_of_them_may_answer_goes_to_owner() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let ada = login("toda", Role::Admin);
    let bo = login("todb", Role::Admin);
    let own = login("todo", Role::Owner);
    for u in [&ada, &bo, &own] {
        phone(u);
    }
    // No default: everyone who may answer, as before.
    let ws0 = seed("todnone");
    let v = ask(&ws0, "test.switch", None);
    let mut all = vec![ada.clone(), bo.clone(), own.clone()];
    all.sort();
    assert_eq!(routed(&v), all, "{v}");

    set_default(json!([ada.clone()]));
    assert_eq!(k2_core::app_settings::load().permission_approvers, vec![ada.clone()]);
    let ws = seed("toddef");
    let mark = test_push_capture::mark();
    let v = ask(&ws, "test.switch", None);
    assert_eq!(routed(&v), vec![ada.clone()], "{v}");
    assert_eq!(pushed(mark, &ws.conv), vec![ada.clone()]);
    // --to still wins over the default.
    let ws1 = seed("toddto");
    let v = ask(&ws1, "test.switch", Some(json!(bo.clone())));
    assert_eq!(routed(&v), vec![bo.clone()], "{v}");

    // An Owner-floor card: ada can't answer it, so it asks the owner token's
    // person, and files no Tickets item (one with no assignee is everyone's).
    let ws2 = seed("toddown");
    let mark = test_push_capture::mark();
    let v = ask(&ws2, "test.owner-switch", None);
    assert_eq!(routed(&v), vec!["owner".to_string()], "{v}");
    assert!(pushed(mark, &ws2.conv).is_empty(), "no Owner login is pushed: {:?}", pushed(mark, &ws2.conv));
    assert!(tickets(&ws2).is_empty(), "{:?}", tickets(&ws2));
    let id = v["id"].as_str().unwrap();
    assert_eq!(waiting_ids(&Who::Login { username: own.clone(), role: Role::Owner }, id), 0);
    assert_eq!(waiting_ids(&Who::Owner, id), 1);

    // Cleared: everyone again.
    set_default(json!([]));
    let ws3 = seed("toddclr");
    assert_eq!(routed(&ask(&ws3, "test.switch", None)), all);

    // The setting takes owners, admins and `owner` only.
    let mem = login("todm", Role::Member);
    for bad in [json!([mem]), json!(["nobody-here"]), json!("ada"), json!([1])] {
        assert!(k2_core::app_settings::update(json!({ "permissionApprovers": bad.clone() })).is_err(), "{bad}");
    }
    set_default(json!(["Owner", ada.to_uppercase()]));
    assert_eq!(k2_core::app_settings::load().permission_approvers, vec!["owner".to_string(), ada.clone()], "stored lowercased");
    // Only the Owner changes it.
    let r = crate::settings_routes::handle_settings_update(
        json!({ "permissionApprovers": [] }).to_string().as_bytes(),
        crate::settings_routes::SettingsActor::Admin,
    );
    assert!(r.status.starts_with("403"), "{}", r.body);
    set_default(json!([]));
}

#[test]
fn requests_for_different_people_never_share_a_batch_card() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let ada = login("toba", Role::Admin);
    let bo = login("tobb", Role::Admin);
    let ws = seed("tobatch");
    let a = ask(&ws, "test.switch", Some(json!(ada.clone())));
    let b = ask(&ws, "test.slow", Some(json!(bo.clone())));
    assert_ne!(a["card"], b["card"], "ada's card and bo's card stay apart: {a} {b}");
    let c = ask(&ws, "test.owner-switch", None);
    assert_ne!(c["card"], a["card"]);
    // The same people within the window: one card.
    let ws2 = seed("tobatch2");
    let a = ask(&ws2, "test.switch", Some(json!(ada.clone())));
    let b = ask(&ws2, "test.slow", Some(json!(ada.clone())));
    assert_eq!(a["card"], b["card"], "{a} {b}");
    assert_eq!(b["batch"], json!(true), "{b}");
}
