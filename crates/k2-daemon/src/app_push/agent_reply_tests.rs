//! Agent replies: the coalescer with an explicit clock, the text rules
//! (previews matrix), recipients (thread:read in the room), off by default,
//! and a delivery through the fake vendor with k2Kind agent.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_core::skin::{self, app_push as store};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::super::sender::{install_test_transport, HttpReq, HttpResp, Transport, TransportErr};
use super::super::test_keys::EC_P256_P8;
use super::*;

fn post(seq: i64) -> Post {
    Post {
        room: "room-1".into(),
        room_handle: "sales".into(),
        conversation: "conv-1".into(),
        agent_name: "Sales".into(),
        seq,
        preview: "Your quote is ready".into(),
    }
}

fn key(g: &str) -> Key {
    ("app".into(), "conv-1".into(), g.into())
}

#[test]
fn the_first_post_pushes_later_ones_wait_for_one_trailing_push() {
    let mut w = Windows::default();
    let t0 = Instant::now();
    assert_eq!(w.post(key("dana"), &post(1), t0), Act::Now);
    assert_eq!(w.post(key("dana"), &post(2), t0 + Duration::from_secs(5)), Act::Held);
    assert_eq!(w.post(key("dana"), &post(3), t0 + Duration::from_secs(10)), Act::Held);
    assert_eq!(w.post(key("eli"), &post(3), t0 + Duration::from_secs(10)), Act::Now, "per guest");
    assert!(w.due(t0 + Duration::from_secs(30)).is_empty(), "the window is still open");
    let due = w.due(t0 + WINDOW);
    assert_eq!(due.len(), 1);
    assert_eq!((due[0].0.clone(), due[0].1, due[0].2.seq), (key("dana"), 2, 3), "one trailing push for two held posts");
    assert_eq!(w.post(key("dana"), &post(4), t0 + WINDOW + Duration::from_secs(1)), Act::Held, "the trailing push opened a window");
    assert!(w.due(t0 + WINDOW * 2 + Duration::from_secs(1)).len() == 1);
    assert!(w.due(t0 + Duration::from_secs(4000)).is_empty());
    assert_eq!(w.post(key("dana"), &post(5), t0 + Duration::from_secs(4000)), Act::Now, "a quiet window pushes again");
}

#[test]
fn the_text_follows_the_previews_rules() {
    let n = notification(&post(7), 1, true);
    assert_eq!((n.title.as_deref(), n.body.as_deref()), (Some("Sales"), Some("Your quote is ready")));
    assert_eq!(n.data.get("k2Room").map(String::as_str), Some("sales"));
    assert_eq!(n.data.get("k2Seq").map(String::as_str), Some("7"));
    assert_eq!(n.data.get("k2Conv").map(String::as_str), Some("conv-1"));
    assert_eq!(notification(&post(7), 1, false).body.as_deref(), Some(GENERIC), "previews off: New message");
    assert_eq!(notification(&post(7), 3, true).body.as_deref(), Some("3 new messages"));
    let mut empty = post(8);
    empty.preview = " ".into();
    assert_eq!(notification(&empty, 1, true).body.as_deref(), Some(GENERIC));
    payload::fits(&n).expect("fits 4 KB");
}

#[derive(Default)]
struct Fake {
    seen: Mutex<Vec<J>>,
}

impl Transport for Fake {
    fn post(&self, r: &HttpReq) -> Result<HttpResp, TransportErr> {
        self.seen.lock().expect("lock").push(serde_json::from_slice(&r.body).unwrap_or(J::Null));
        Ok(HttpResp { status: 200, retry_after: None, body: b"{}".to_vec() })
    }
}

#[test]
fn guests_with_thread_read_get_it_with_k2kind_agent_and_previews_per_device() {
    let home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    let ws = "ws-agentreply".to_string();
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let path = home.path().join(&ws);
        std::fs::create_dir_all(&path).expect("dir");
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, path.to_string_lossy()]).expect("project");
        k2_core::published_services::insert(&conn, &ws, "jobs", "node s.js", "/tmp", 4350, "local", "running", "cmd", "").expect("svc").id
    };
    let fake = Arc::new(Fake::default());
    install_test_transport(&app, fake.clone());
    assert!(!store::config(&app).expect("cfg").agent_replies, "off by default");
    assert!(!store::apps_with_agent_replies().expect("list").contains(&app));
    store::set_switches(&app, store::Switches { owner_on: Some(true), agent_replies: Some(true), ..Default::default() }, "owner").expect("on");
    assert!(store::apps_with_agent_replies().expect("list").contains(&app));
    let k = super::super::creds::validate_apns(EC_P256_P8, "ABC123DEFG", "TEAM123456", "com.example.app").expect("apns");
    super::super::creds::store_apns(&app, &k, "owner").expect("store");
    let mut reader = skin::RoomPolicy::new();
    reader.insert(ws.clone(), vec!["thread:read".into()]);
    skin::create_role_for_app("readers", &reader, Some(&app)).expect("role");
    let mut poster = skin::RoomPolicy::new();
    poster.insert(ws.clone(), vec!["files:read".into()]);
    skin::create_role_for_app("files-only", &poster, Some(&app)).expect("role");
    skin::add_principal_for_app("dana", None, &app, Some("readers"), "test").expect("dana");
    skin::add_principal_for_app("eli", None, &app, Some("files-only"), "test").expect("eli");
    let dana = skin::list_principals().expect("p").into_iter().find(|p| p.username == "dana").expect("dana").id;
    let eli = skin::list_principals().expect("p").into_iter().find(|p| p.username == "eli").expect("eli").id;
    assert_eq!(store::room_guests(&app, &ws, Some("thread:read")).expect("guests"), vec![dana.clone()], "only thread:read");
    assert_eq!(store::room_guests(&app, &ws, None).expect("guests").len(), 2);
    for (user, pid, dev, tok) in [("dana", &dana, "d1", "d1"), ("dana", &dana, "d2", "d2"), ("eli", &eli, "e1", "e1")] {
        let s = skin::create_app_session_token(user, &app).expect("session");
        let reg = store::parse_registration(&json!({"deviceId": dev, "platform": "apns", "pushToken": tok.repeat(32)})).expect("reg");
        store::register(&app, pid, &s.meta.id, &reg).expect("register");
    }
    store::set_prefs(&app, &dana, "d2", Some(Some(false)), None).expect("prefs").expect("row");
    let p = Post { room: ws.clone(), room_handle: ws.clone(), conversation: "conv-x".into(), agent_name: "Jobs agent".into(), seq: 3, preview: "Quote ready".into() };
    deliver(&app, &dana, &p, 1);
    assert!(worker::wait_app(&app, Duration::from_secs(20)));
    let seen = fake.seen.lock().expect("lock").clone();
    assert_eq!(seen.len(), 2, "dana's two phones");
    let bodies: Vec<&str> = seen.iter().map(|b| b["aps"]["alert"]["body"].as_str().expect("body")).collect();
    assert!(bodies.contains(&"Quote ready") && bodies.contains(&GENERIC), "the phone that said no gets New message: {bodies:?}");
    for b in &seen {
        assert_eq!(b["k2Kind"], "agent");
        assert_eq!(b["k2Conv"], "conv-x");
        assert_eq!(b["k2Seq"], "3");
        assert_eq!(b["aps"]["alert"]["title"], "Jobs agent");
    }
    // The App's previews off: everyone gets New message.
    store::set_switches(&app, store::Switches { previews: Some(false), ..Default::default() }, "owner").expect("previews off");
    deliver(&app, &dana, &p, 1);
    assert!(worker::wait_app(&app, Duration::from_secs(20)));
    let seen = fake.seen.lock().expect("lock").clone();
    assert!(seen[2..].iter().all(|b| b["aps"]["alert"]["body"] == GENERIC), "{seen:?}");
}

/// Security review B-M1: every push preview (an App guest's and the owner's
/// Companion push) is built from the Thread's stored, redacted text, never
/// the request's raw text.
#[test]
fn previews_come_from_the_redacted_stored_text() {
    use k2_core::overlay::OverlayDoc;
    use k2_core::push::ThreadMsgKind;
    use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore};
    let v = crate::vault::ensure_test_vault();
    const VAL: &str = "k2test_PUSHPREVIEW_77aa88bb99cc0011";
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "PUSH_PREVIEW_KEY", Kind::ApiKey);
    r.hosts = vec!["https://api.example.com:443".to_string()];
    v.put(r, SecretBytes::from_slice(VAL.as_bytes()), &Actor::Owner).expect("put");
    let text = OverlayDoc::text("d1".into(), "sales".into(), "owner".into(), format!("the key is {VAL}"), "thread");
    let stored = text.body.clone().expect("body");
    assert!(!stored.contains(VAL), "the Thread stores it redacted: {stored}");
    let p = crate::overlay_routes::push_preview_text(ThreadMsgKind::Text, &text);
    assert!(!p.contains(VAL), "{p}");
    assert!(p.contains("[k2v:PUSH_PREVIEW_KEY]"), "{p}");
    let choice = OverlayDoc::choice("d2".into(), "sales".into(), "owner".into(), format!("Rotate {VAL}?"), vec!["yes".into(), "no".into()], false);
    let p = crate::overlay_routes::push_preview_text(ThreadMsgKind::Choice, &choice);
    assert!(!p.contains(VAL) && p.contains("[k2v:PUSH_PREVIEW_KEY]"), "a choice card's question too: {p}");
    assert_eq!(crate::overlay_routes::push_preview_text(ThreadMsgKind::Secret, &text), "", "a secret card never previews");
}

/// TC-24 / TA-23: a guest viewing the Thread (push.viewing or its own
/// overlay frames, both under `guest:<principalId>`) gets no agent-reply
/// push; the first post after the 90 s view lapses pushes again.
#[test]
fn a_viewing_guest_gets_no_agent_reply_push_until_the_view_lapses() {
    let home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    let ws = "ws-agentview".to_string();
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let path = home.path().join(&ws);
        std::fs::create_dir_all(&path).expect("dir");
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, path.to_string_lossy()]).expect("project");
        k2_core::published_services::insert(&conn, &ws, "view", "node s.js", "/tmp", 4351, "local", "running", "cmd", "").expect("svc").id
    };
    let fake = Arc::new(Fake::default());
    install_test_transport(&app, fake.clone());
    store::set_switches(&app, store::Switches { owner_on: Some(true), agent_replies: Some(true), ..Default::default() }, "owner").expect("on");
    let k = super::super::creds::validate_apns(EC_P256_P8, "ABC123DEFG", "TEAM123456", "com.example.app").expect("apns");
    super::super::creds::store_apns(&app, &k, "owner").expect("store");
    let mut reader = skin::RoomPolicy::new();
    reader.insert(ws.clone(), vec!["thread:read".into()]);
    skin::create_role_for_app("viewers", &reader, Some(&app)).expect("role");
    skin::add_principal_for_app("vera", None, &app, Some("viewers"), "test").expect("vera");
    let vera = skin::list_principals().expect("p").into_iter().find(|p| p.username == "vera").expect("vera").id;
    let s = skin::create_app_session_token("vera", &app).expect("session");
    let reg = store::parse_registration(&json!({"deviceId": "v1", "platform": "apns", "pushToken": "v1".repeat(32)})).expect("reg");
    store::register(&app, &vera, &s.meta.id, &reg).expect("register");
    let conv = format!("conv-view-{}", uuid::Uuid::new_v4());
    let p = |seq| Post { room: ws.clone(), room_handle: ws.clone(), conversation: conv.clone(), agent_name: "Jobs agent".into(), seq, preview: "Done".into() };
    let sent = || {
        assert!(worker::wait_app(&app, Duration::from_secs(20)));
        fake.seen.lock().expect("lock").len()
    };

    let t0 = Instant::now();
    let socket = crate::thread_viewers::verb_socket(&s.meta.id);
    crate::thread_viewers::on_guest_frame(socket, &guest_key(&vera), &conv, true);
    guest_post(&app, &vera, &p(1), t0);
    assert_eq!(sent(), 0, "viewing: the leading post is quiet");
    // 60 s on: the first window closed, but the view (90 s) still holds.
    guest_post(&app, &vera, &p(2), t0 + Duration::from_secs(60));
    assert_eq!(sent(), 0, "still viewing at 60 s");
    // 106 s on: the view lapsed 16 s ago and the window from 60 s closed.
    guest_post(&app, &vera, &p(3), t0 + Duration::from_secs(106));
    assert_eq!(sent(), 1, "the first post after the view lapses pushes");
    assert_eq!(fake.seen.lock().expect("lock")[0]["k2Seq"], "3");
    crate::thread_viewers::on_guest_frame(socket, &guest_key(&vera), &conv, false);
}
