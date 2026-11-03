//! post-idempotency-v1 (0.46.8): `clientMsgId` on `POST /cli/thread/post`,
//! through the real handler (`dispatch_post`, the TCP and cell door).
//! Every test here holds [`crate::client_msg_id::test_lock`]: the sweep test
//! drops every key older than its cutoff, which must not race another
//! test's repeat.

use super::*;
use rusqlite::params;

fn seed(label: &str) -> (String, String, String) {
    crate::vault::ensure_test_vault();
    let handle = format!("{label}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/ovl-cmid-{handle}-{id}");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running")
        .expect("pin");
    (id, handle, conv)
}

fn post(body: serde_json::Value) -> CliResponse {
    dispatch_post("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes())
}

fn json(resp: &CliResponse) -> serde_json::Value {
    serde_json::from_str(&resp.body).unwrap_or_else(|e| panic!("response JSON parse failed: {e}; body={}", resp.body))
}

fn items(handle: &str) -> Vec<serde_json::Value> {
    let get = dispatch("/cli/thread", &HashMap::from([("addr".to_string(), handle.to_string())])).expect("GET thread");
    assert_eq!(get.status, "200 OK", "{}", get.body);
    json(&get)["items"].as_array().expect("items").clone()
}

fn cmid() -> String {
    format!("cm_{}", uuid::Uuid::new_v4().simple())
}

/// Thread frames on the overlay bus for `conv` (drains the receiver).
fn frames_for(rx: &mut tokio::sync::broadcast::Receiver<crate::overlay_ws::OverlayFrame>, conv: &str) -> Vec<crate::overlay_ws::OverlayFrame> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(f) => {
                if f.collection == "thread" && f.conversation_id.as_deref() == Some(conv) {
                    out.push(f);
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => panic!("overlay bus lagged by {n}: raise the test's drain rate"),
            Err(_) => return out,
        }
    }
}

/// The first post is stored with its id echoed (answer, stored doc, frame);
/// the same key again answers the stored item with `duplicate: true` and
/// sends nothing again: one item, one frame, one inject.
#[test]
fn a_repeat_answers_the_stored_post_and_sends_nothing_again() {
    let _keys = crate::client_msg_id::test_lock();
    let (_, handle, conv) = seed("cmidone");
    let id = cmid();
    let text = format!("send once {}", uuid::Uuid::new_v4());
    let mut rx = crate::overlay_ws::subscribe();

    let first = post(serde_json::json!({ "addr": handle, "text": text, "via": "compose", "clientMsgId": id }));
    assert_eq!(first.status, "200 OK", "{}", first.body);
    let first = json(&first);
    assert_eq!(first["clientMsgId"], id.as_str(), "echoed on the answer: {first}");
    assert!(first.get("duplicate").is_none(), "a first post is no duplicate: {first}");

    let again = post(serde_json::json!({ "addr": handle, "text": text, "via": "compose", "clientMsgId": id }));
    assert_eq!(again.status, "200 OK", "{}", again.body);
    let again = json(&again);
    assert_eq!(again["duplicate"], true, "{again}");
    assert!(again.get("mismatch").is_none(), "same body: no mismatch: {again}");
    for k in ["id", "seq", "from", "to", "kind", "body", "via", "author", "created_at", "conversation_id", "addr", "clientMsgId"] {
        assert_eq!(again[k], first[k], "the repeat answers the stored item's {k}: first={first} again={again}");
    }

    let stored = items(&handle);
    assert_eq!(stored.len(), 1, "one item only: {stored:?}");
    assert_eq!(stored[0]["doc"]["clientMsgId"], id.as_str(), "echoed on the stored doc");

    let frames = frames_for(&mut rx, &conv);
    assert_eq!(frames.len(), 1, "one frame only: {frames:?}");
    let frame_doc = frames[0].doc.as_ref().expect("frame doc");
    assert_eq!(frame_doc.client_msg_id.as_deref(), Some(id.as_str()), "echoed on the frame to its author");

    let injects = recorded_injects().into_iter().filter(|l| l.contains(&text)).count();
    assert_eq!(injects, 1, "the agent gets the line once");
}

/// A repeat whose body differs: the first write wins, `mismatch: true`.
#[test]
fn a_repeat_with_another_body_is_a_mismatch() {
    let _keys = crate::client_msg_id::test_lock();
    let (_, handle, _) = seed("cmidmis");
    let id = cmid();
    let first = post(serde_json::json!({ "addr": handle, "text": "first words", "from": "k2", "clientMsgId": id }));
    assert_eq!(first.status, "200 OK", "{}", first.body);
    let again = post(serde_json::json!({ "addr": handle, "text": "second words", "from": "k2", "clientMsgId": id }));
    assert_eq!(again.status, "200 OK", "{}", again.body);
    let again = json(&again);
    assert_eq!(again["duplicate"], true, "{again}");
    assert_eq!(again["mismatch"], true, "{again}");
    assert_eq!(again["body"], "first words", "the first write wins: {again}");
    assert_eq!(items(&handle).len(), 1);
}

/// The same key from the same sender in another chat: 409, nothing stored.
#[test]
fn a_key_reused_in_another_chat_is_409() {
    let _keys = crate::client_msg_id::test_lock();
    let (_, a, _) = seed("cmida");
    let (_, b, _) = seed("cmidb");
    let id = cmid();
    let first = post(serde_json::json!({ "addr": a, "text": "in a", "via": "compose", "clientMsgId": id }));
    assert_eq!(first.status, "200 OK", "{}", first.body);
    let other = post(serde_json::json!({ "addr": b, "text": "in a", "via": "compose", "clientMsgId": id }));
    assert_eq!(other.status, "409 Conflict", "{}", other.body);
    assert_eq!(json(&other)["error"]["code"], "client_msg_id_reused");
    assert!(items(&b).is_empty(), "nothing stored in the other chat");
}

/// Malformed ids are a 400 `usage` before anything is stored; an old
/// client (no id) is unchanged: no id on the answer or the doc.
#[test]
fn bad_ids_are_400_and_no_id_is_unchanged() {
    let _keys = crate::client_msg_id::test_lock();
    let (_, handle, _) = seed("cmidbad");
    for bad in [
        serde_json::json!(""),
        serde_json::json!("x".repeat(65)),
        serde_json::json!("has space"),
        serde_json::json!("a/b"),
        serde_json::json!("dot.ted"),
        serde_json::json!(12345),
        serde_json::json!(true),
        serde_json::json!(["a"]),
        serde_json::json!({"id": "a"}),
    ] {
        let r = post(serde_json::json!({ "addr": handle, "text": "x", "from": "k2", "clientMsgId": bad }));
        assert_eq!(r.status, "400 Bad Request", "{bad}: {}", r.body);
        assert_eq!(json(&r)["error"]["code"], "usage", "{bad}: {}", r.body);
    }
    // Checked before the address resolves: an unknown address still 400s.
    let r = post(serde_json::json!({ "addr": "no-such-room-xyz", "text": "x", "clientMsgId": "a b" }));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert!(items(&handle).is_empty(), "nothing stored");

    let plain = post(serde_json::json!({ "addr": handle, "text": "old client", "from": "k2" }));
    assert_eq!(plain.status, "200 OK", "{}", plain.body);
    assert!(json(&plain).get("clientMsgId").is_none(), "{}", plain.body);
    assert!(json(&plain).get("duplicate").is_none(), "{}", plain.body);
    let again = post(serde_json::json!({ "addr": handle, "text": "old client", "from": "k2" }));
    assert_eq!(again.status, "200 OK", "{}", again.body);
    let stored = items(&handle);
    assert_eq!(stored.len(), 2, "without an id every post stores: {stored:?}");
    assert!(stored.iter().all(|i| i["doc"].get("clientMsgId").is_none()), "{stored:?}");
}

/// Several posts with one key at once: one item.
#[test]
fn concurrent_posts_with_one_key_store_one_item() {
    let _keys = crate::client_msg_id::test_lock();
    let (_, handle, _) = seed("cmidrace");
    let id = cmid();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|i| {
            let (handle, id, barrier) = (handle.clone(), id.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                let r = post(serde_json::json!({ "addr": handle, "text": format!("racer {i}"), "from": "k2", "clientMsgId": id }));
                assert_eq!(r.status, "200 OK", "{}", r.body);
                json(&r).get("duplicate").is_none()
            })
        })
        .collect();
    let stored = threads.into_iter().map(|t| t.join().expect("racer")).filter(|first| *first).count();
    assert_eq!(stored, 1, "exactly one racer stores");
    assert_eq!(items(&handle).len(), 1, "one item only");
}

/// The key is the sender's: the owner's compose bar, the owner token's
/// shell, a Connect login, an agent passport and an App guest each post
/// with the same id and each gets its own item.
#[test]
fn another_senders_same_key_is_a_new_post() {
    let _keys = crate::client_msg_id::test_lock();
    let (project_id, handle, _) = seed("cmidns");
    let id = cmid();
    let body = |text: &str| serde_json::json!({ "addr": handle, "text": text, "clientMsgId": id }).to_string();
    let compose = serde_json::json!({ "addr": handle, "text": "owner bar", "via": "compose", "clientMsgId": id }).to_string();
    let passport = HashMap::from([
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
        ("project_id".to_string(), project_id.clone()),
        ("from".to_string(), handle.clone()),
    ]);
    let pass = k2_core::skin::SkinPass {
        id: "cmid-pass".into(),
        principal_id: Some(format!("prin-{}", uuid::Uuid::new_v4())),
        username: "guest".into(),
        caps: vec!["thread:read".into(), "thread:post".into()],
        rooms: vec![project_id.clone()],
        session: false,
        room_policy: k2_core::skin::RoomPolicy::new(),
        app: String::new(),
        room_paths: Default::default(),
        expires_at: None,
        chat: false,
    };
    let answers = [
        dispatch_post_as("/cli/thread/post", &HashMap::new(), compose.as_bytes(), "owner"),
        dispatch_post_as("/cli/thread/post", &HashMap::new(), body("owner shell").as_bytes(), "owner"),
        dispatch_post_as("/cli/thread/post", &HashMap::new(), body("connect login").as_bytes(), "cmid-alice"),
        dispatch_post_as("/cli/thread/post", &passport, body("agent").as_bytes(), "owner"),
        with_request_skin(Some(pass), || dispatch_post("/cli/thread/post", &HashMap::new(), body("app guest").as_bytes())),
    ];
    for a in &answers {
        assert_eq!(a.status, "200 OK", "{}", a.body);
        assert!(json(a).get("duplicate").is_none(), "each sender's first post stores: {}", a.body);
    }
    assert_eq!(items(&handle).len(), 5, "five senders, five items");
}

/// The key leaves the conversation out: after the conversation moved to
/// a new key (Codex/Hermes adoption), the retry is still a duplicate, found
/// on the moved conversation.
#[test]
fn a_repeat_after_a_conversation_move_is_still_a_duplicate() {
    let _keys = crate::client_msg_id::test_lock();
    let (project_id, handle, conv) = seed("cmidmove");
    let id = cmid();
    let first = post(serde_json::json!({ "addr": handle, "text": "before the move", "via": "compose", "clientMsgId": id }));
    assert_eq!(first.status, "200 OK", "{}", first.body);
    let first = json(&first);
    let to = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        overlay::move_conversation(&conn, &conv, &to).expect("move").expect("moved");
        conn.execute("UPDATE workspace_sessions SET session_id = ?1 WHERE project_id = ?2", params![to, project_id])
            .expect("re-pin");
    }
    let again = post(serde_json::json!({ "addr": handle, "text": "before the move", "via": "compose", "clientMsgId": id }));
    assert_eq!(again.status, "200 OK", "{}", again.body);
    let again = json(&again);
    assert_eq!(again["duplicate"], true, "{again}");
    assert_eq!(again["id"], first["id"], "the same post: {again}");
    assert_eq!(again["conversation_id"], to.as_str(), "found on the moved conversation: {again}");
    assert_eq!(items(&handle).len(), 1, "one item only");
}

/// The daemon's sweep frees a key after a day: at 23 h a repeat is still a
/// duplicate, after 25 h the same id posts again.
#[test]
fn the_sweep_frees_a_key_after_a_day() {
    let _keys = crate::client_msg_id::test_lock();
    let (_, handle, _) = seed("cmidsweep");
    let id = cmid();
    let sent = chrono::Utc::now().timestamp_millis();
    let r = post(serde_json::json!({ "addr": handle, "text": "kept a day", "from": "k2", "clientMsgId": id }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let hour = 3_600_000i64;
    crate::client_msg_id::sweep(sent + 23 * hour);
    let r = post(serde_json::json!({ "addr": handle, "text": "kept a day", "from": "k2", "clientMsgId": id }));
    assert_eq!(json(&r)["duplicate"], true, "kept at 23 h: {}", r.body);
    crate::client_msg_id::sweep(sent + 25 * hour);
    let r = post(serde_json::json!({ "addr": handle, "text": "kept a day", "from": "k2", "clientMsgId": id }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(json(&r).get("duplicate").is_none(), "swept after 25 h: the id posts again: {}", r.body);
    assert_eq!(items(&handle).len(), 2);
}
