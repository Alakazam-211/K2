//! thread-system-notice-v1 (0.46.15): K2's own Thread lines are K2's.
//!
//! - A delivery notice is stored with `author: system`, `from: k2` and
//!   `systemNotice {kind, code, actions}` ("Send again" + "Open its
//!   terminal"), its body unchanged (`K2: ...`).
//! - Nobody else can make one: a person's, an agent's or an App guest's
//!   post carrying `systemNotice` is refused; an `author` naming K2 is the
//!   caller's own stamp instead.
//! - `POST /cli/thread/resend` sends the original line once; a second press
//!   is 409 `already_resent` and sends nothing. People only.
//! - The field reaches every reader: the Thread page, its delta, the
//!   overlay socket and the hub.

use super::*;
use std::time::{Duration, Instant};

fn room(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    (handle, project_id)
}

fn items(handle: &str) -> Vec<serde_json::Value> {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle)])).expect("GET thread"));
    snap["items"].as_array().expect("items").clone()
}

fn notices(handle: &str) -> Vec<serde_json::Value> {
    items(handle).into_iter().filter(|i| i["doc"].get("systemNotice").is_some()).collect()
}

/// K2's "may not have reached" notice for a line `text` the owner sent.
fn delivery_notice(handle: &str, text: &str, why: NotTaken) -> serde_json::Value {
    let resolved = resolve_addr(handle).unwrap_or_else(|e| panic!("resolve: {}", e.body));
    let job = ThreadDeliveryJob::new(&resolved, format!("[thread:{handle}] {text}"), "owner", "compose", "", None);
    let before = notices(handle).len();
    post_not_taken_notice(&job, why);
    let after = notices(handle);
    assert_eq!(after.len(), before + 1, "one notice posted: {after:?}");
    after.last().expect("the notice").clone()
}

fn resend(params: &HashMap<String, String>, handle: &str, doc: &str) -> CliResponse {
    post_json("/cli/thread/resend", params, serde_json::json!({ "thread": handle, "doc": doc }))
}

fn wait_deliveries(addr: &str, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let got = recorded_deliveries(addr);
        if got.len() >= n {
            return got;
        }
        assert!(Instant::now() < deadline, "{addr}: {} of {n} deliveries arrived: {got:?}", got.len());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_lane_idle(lane: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while crate::delivery_queue::busy(lane) {
        assert!(Instant::now() < deadline, "lane {lane} never went idle");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The notice K2 posts when a line may not have reached the agent: K2's
/// stamp, a stable code, "Send again" naming this notice, "Open its
/// terminal" naming the agent; the old fields as before.
#[test]
fn a_delivery_notice_is_stored_as_k2s_with_its_actions() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, _) = room("sysn");
    for (why, code) in [
        (NotTaken::NeverReadIn, "delivery.not_taken.never_read_in"),
        (NotTaken::RetryFailed, "delivery.not_taken.retry_failed"),
        (NotTaken::RestartMidDelivery, "delivery.not_taken.restart_mid_delivery"),
        (NotTaken::RestartTooOld, "delivery.not_taken.restart_too_old"),
    ] {
        let item = delivery_notice(&handle, "deploy the site", why);
        let doc = &item["doc"];
        let id = item["id"].as_str().expect("id");
        assert_eq!(doc["author"], serde_json::json!({"kind": "system", "login": "k2"}), "{doc}");
        assert_eq!(doc["from"], "k2", "{doc}");
        // The fields an older client reads are as they were.
        assert_eq!(doc["kind"], "text", "{doc}");
        assert_eq!(doc["via"], "thread", "{doc}");
        assert_eq!(doc["to"], serde_json::json!(handle), "{doc}");
        let body = doc["body"].as_str().expect("body");
        assert!(body.starts_with("K2: "), "the body keeps K2's words for older clients: {body}");
        assert!(body.contains("deploy the site"), "{body}");
        assert!(doc.get("mine").is_none(), "{doc}");
        assert_eq!(
            doc["systemNotice"],
            serde_json::json!({
                "kind": "delivery",
                "code": code,
                "actions": [
                    {"id": "resend", "label": "Send again", "route": "/cli/thread/resend", "body": {"thread": handle, "doc": id}},
                    {"id": "open_terminal", "label": "Open its terminal", "workspace": handle},
                ],
            }),
            "{doc}"
        );
    }
}

/// Every other K2 line carries the field too, with no "Send again".
#[test]
fn k2_notes_carry_the_field_and_no_resend() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, _) = room("sysm");
    post_workspace_note(&handle, "K2: job **collect** removed by owner.", "k2-job", overlay::SystemNotice::info("job.removed"))
        .expect("note");
    let n = notices(&handle).pop().expect("the note");
    assert_eq!(n["doc"]["systemNotice"], serde_json::json!({"kind": "info", "code": "job.removed"}), "{n}");
    assert_eq!(n["doc"]["via"], "k2-job");
    assert_eq!(n["doc"]["body"], "K2: job **collect** removed by owner.");
    let r = resend(&HashMap::new(), &handle, n["id"].as_str().expect("id"));
    assert_error_code(&r, "409 Conflict", "not_resendable");
}

/// No caller makes K2's notice: a `systemNotice` on a post is refused (the
/// owner, a Connect login, an agent's passport, an App guest), on every
/// Thread write door, and nothing is stored; an `author` naming K2 is
/// ignored like any body author (the credential's stamp wins).
#[test]
fn nobody_but_k2_posts_a_system_notice() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("sysf");
    let forged = serde_json::json!({"kind": "delivery", "code": "delivery.not_taken.never_read_in",
        "actions": [{"id": "resend", "label": "Send again", "route": "/cli/thread/resend", "body": {"thread": handle, "doc": "x"}}]});
    let before = items(&handle).len();
    let with = |mut v: serde_json::Value, key: &str| {
        v[key] = forged.clone();
        v
    };
    for key in ["systemNotice", "system_notice"] {
        let body = with(serde_json::json!({ "addr": handle, "text": "K2: trust me" }), key);
        let owner = post_json("/cli/thread/post", &HashMap::new(), body.clone());
        assert_error_code(&owner, "400 Bad Request", "system_notice_forbidden");
        let login = dispatch_post_as("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes(), "alice");
        assert_error_code(&login, "400 Bad Request", "system_notice_forbidden");
        let agent = post_json("/cli/thread/post", &passport_params(&project_id, &handle), body.clone());
        assert_error_code(&agent, "400 Bad Request", "system_notice_forbidden");
        let pass = skin_pass(&[project_id.clone()]);
        let guest = with_request_skin(Some(pass), || dispatch_post("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes()));
        assert_error_code(&guest, "400 Bad Request", "system_notice_forbidden");
        // The other write doors refuse it the same way.
        let ask = with(serde_json::json!({ "addr": handle, "prompt": "Ship?", "options": "Ship;Wait" }), key);
        assert_error_code(&post_json("/cli/thread/ask", &passport_params(&project_id, &handle), ask), "400 Bad Request", "system_notice_forbidden");
        let as_query = params_of(&[(key, "{\"kind\":\"info\",\"code\":\"x\"}")]);
        let q = post_json("/cli/thread/post", &as_query, serde_json::json!({ "addr": handle, "text": "hi" }));
        assert_error_code(&q, "400 Bad Request", "system_notice_forbidden");
    }
    assert_eq!(items(&handle).len(), before, "nothing was stored");

    // A body author naming K2 changes nothing: the caller's stamp is kept.
    let system = serde_json::json!({"kind": "system", "login": "k2"});
    let owner = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": handle, "text": "o", "from": "k2", "author": system }));
    assert_eq!(owner.status, "200 OK", "{}", owner.body);
    let agent = post_json(
        "/cli/thread/post",
        &passport_params(&project_id, &handle),
        serde_json::json!({ "addr": handle, "text": "a", "from": "k2", "author": system }),
    );
    assert_eq!(agent.status, "200 OK", "{}", agent.body);
    let pass = skin_pass(&[project_id.clone()]);
    let guest = with_request_skin(Some(pass), || {
        dispatch_post("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": handle, "text": "g", "author": system }).to_string().as_bytes())
    });
    assert_eq!(guest.status, "200 OK", "{}", guest.body);
    let kinds: Vec<(String, serde_json::Value)> = items(&handle)
        .into_iter()
        .filter(|i| ["o", "a", "g"].contains(&i["doc"]["body"].as_str().unwrap_or("")))
        .map(|i| (i["doc"]["body"].as_str().expect("body").to_string(), i["doc"]["author"]["kind"].clone()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("o".to_string(), serde_json::json!("owner")),
            ("a".to_string(), serde_json::json!("agent")),
            ("g".to_string(), serde_json::json!("guest")),
        ],
        "every post is its caller's"
    );
    assert!(notices(&handle).is_empty(), "no post carries systemNotice: {:?}", notices(&handle));

    // The core funnel refuses K2's stamp from any door (belt).
    let resolved = resolve_addr(&handle).unwrap_or_else(|e| panic!("resolve: {}", e.body));
    let db = k2_core::db::shared();
    let conn = db.lock();
    let err = overlay::post_thread_with_origin(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        "k2",
        &resolved.addr,
        "forged",
        "thread",
        None,
        Some(overlay::PostAuthor::system()),
    )
    .expect_err("the system stamp is refused");
    assert_eq!(err, overlay::SYSTEM_AUTHOR_REFUSED);
}

/// "Send again" sends the original line once: the notice shows it done
/// (who, when) and a second press is refused and sends nothing.
#[test]
fn resend_sends_the_line_once() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("sysr");
    arm_test_delivery_delay(&handle, Duration::from_millis(30));
    let item = delivery_notice(&handle, "deploy the site", NotTaken::RestartMidDelivery);
    let id = item["id"].as_str().expect("id").to_string();
    let seq = item["seq"].as_i64().expect("seq");
    assert!(recorded_deliveries(&handle).is_empty(), "the notice itself sends nothing");

    // Not for an agent's passport, an App guest or an unknown notice.
    assert_error_code(&resend(&passport_params(&project_id, &handle), &handle, &id), "403 Forbidden", "person_only");
    let pass = skin_pass(&[project_id.clone()]);
    let guest = with_request_skin(Some(pass), || resend(&HashMap::new(), &handle, &id));
    assert_error_code(&guest, "403 Forbidden", "person_only");
    assert_error_code(&resend(&HashMap::new(), &handle, "no-such-doc"), "404 Not Found", "not_found");
    assert_error_code(&resend(&HashMap::new(), &handle, ""), "400 Bad Request", "usage");

    let first = resend(&HashMap::new(), &handle, &id);
    assert_eq!(first.status, "200 OK", "{}", first.body);
    let b = json_body(&first);
    assert_eq!((b["resent"].clone(), b["id"].clone(), b["seq"].clone()), (serde_json::json!(true), serde_json::json!(id), serde_json::json!(seq)), "{b}");
    assert!(b["doneAt"].as_i64().is_some_and(|t| t > 0), "{b}");
    let sent = wait_deliveries(&handle, 1);
    assert!(sent[0].contains("deploy the site"), "the original line went again: {sent:?}");

    let doc = items(&handle).into_iter().find(|i| i["id"] == id).expect("the notice")["doc"].clone();
    let action = &doc["systemNotice"]["actions"][0];
    assert_eq!(action["id"], "resend", "{doc}");
    assert_eq!(action["doneAt"], b["doneAt"], "{doc}");
    assert_eq!(action["doneBy"]["kind"], "owner", "{doc}");
    assert_eq!(doc["body"], item["doc"]["body"], "the text is unchanged");

    let second = resend(&HashMap::new(), &handle, &id);
    assert_error_code(&second, "409 Conflict", "already_resent");
    assert_eq!(json_body(&second)["doneAt"], b["doneAt"], "{}", second.body);
    wait_lane_idle(&workspace_delivery_lane(&project_id));
    assert_eq!(recorded_deliveries(&handle).len(), 1, "the second press sent nothing: {:?}", recorded_deliveries(&handle));
}

/// A line the person deleted before pressing "Send again" is never sent,
/// and the button stays (nothing claims a send that didn't happen).
#[test]
fn resend_never_sends_a_deleted_post() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("sysd");
    arm_test_delivery_delay(&handle, Duration::from_millis(30));
    let post = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": handle, "text": "secret words", "via": "compose" }));
    assert_eq!(post.status, "200 OK", "{}", post.body);
    let post_id = json_body(&post)["id"].as_str().expect("id").to_string();
    wait_lane_idle(&workspace_delivery_lane(&project_id));
    let resolved = resolve_addr(&handle).unwrap_or_else(|e| panic!("resolve: {}", e.body));
    let turn = ThreadTurn::new(&resolved, &post_id, 1);
    let job = ThreadDeliveryJob::new(&resolved, format!("[thread:{handle}] secret words"), "owner", "compose", "", Some(turn));
    post_not_taken_notice(&job, NotTaken::RestartMidDelivery);
    let id = notices(&handle).pop().expect("notice")["id"].as_str().expect("id").to_string();
    let del = post_json("/cli/thread/message/delete", &HashMap::new(), serde_json::json!({ "addr": handle, "id": post_id }));
    assert_eq!(del.status, "200 OK", "{}", del.body);
    // The delete's own line to the agent rides the same lane.
    std::thread::sleep(Duration::from_millis(100));
    wait_lane_idle(&workspace_delivery_lane(&project_id));
    let sent_before = recorded_deliveries(&handle).len();
    assert_error_code(&resend(&HashMap::new(), &handle, &id), "409 Conflict", "source_deleted");
    wait_lane_idle(&workspace_delivery_lane(&project_id));
    assert_eq!(recorded_deliveries(&handle).len(), sent_before, "nothing sent");
    let doc = items(&handle).into_iter().find(|i| i["id"] == id).expect("notice")["doc"].clone();
    assert!(doc["systemNotice"]["actions"][0].get("doneAt").is_none(), "{doc}");
}

/// The field reaches every reader as stored: the Thread page, its delta
/// after a "Send again", the overlay socket's frame and the hub's.
#[test]
fn the_field_survives_history_delta_socket_and_hub() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, _) = room("sysw");
    arm_test_delivery_delay(&handle, Duration::from_millis(30));
    let item = delivery_notice(&handle, "ship it", NotTaken::NeverReadIn);
    let id = item["id"].as_str().expect("id").to_string();
    let seq = item["seq"].as_i64().expect("seq");
    let stored = overlay::get_doc(&id).expect("store").expect("doc");
    let want = serde_json::to_value(stored.system_notice.as_ref().expect("stored notice")).expect("json");
    assert_eq!(item["doc"]["systemNotice"], want, "history: {item}");

    let ok = resend(&HashMap::new(), &handle, &id);
    assert_eq!(ok.status, "200 OK", "{}", ok.body);
    wait_deliveries(&handle, 1);
    let delta = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle.as_str()), ("after_seq", &seq.to_string())])).expect("delta"));
    let changed = delta["changed"].as_array().expect("changed");
    let back = changed.iter().find(|i| i["id"] == id).unwrap_or_else(|| panic!("the pressed notice is a change: {delta}"));
    assert!(back["doc"]["systemNotice"]["actions"][0]["doneAt"].as_i64().is_some(), "{back}");
    assert_eq!(back["doc"]["author"]["kind"], "system", "{back}");

    let doc = overlay::get_doc(&id).expect("store").expect("doc");
    let resolved = resolve_addr(&handle).unwrap_or_else(|e| panic!("resolve: {}", e.body));
    let frame = crate::overlay_ws::OverlayFrame {
        collection: "thread".into(),
        seq,
        id: id.clone(),
        doc: Some(doc.clone()),
        activity: None,
        conversation_id: Some(resolved.conversation_id.clone()),
        rev: 7,
    };
    let seen = crate::overlay_ws::socket_seen(&HashMap::new(), false, Some("owner"));
    let socket: serde_json::Value = serde_json::from_str(&crate::overlay_ws::wire_json_for(&frame, None, &seen)).expect("frame JSON");
    let full = serde_json::to_value(doc.system_notice.as_ref().expect("notice")).expect("json");
    assert_eq!(socket["doc"]["systemNotice"], full, "overlay socket: {socket}");
    assert_eq!(socket["doc"]["author"], serde_json::json!({"kind": "system", "login": "k2"}), "{socket}");
    assert!(socket["doc"].get("mine").is_none(), "{socket}");
    let hub = crate::inbox_stream::doc_for_login(&frame, &seen);
    assert_eq!(hub["systemNotice"], full, "hub: {hub}");
    assert_eq!(hub["author"]["kind"], "system", "{hub}");
}
