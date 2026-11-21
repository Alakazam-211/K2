//! A choice card's metadata map (`k2 thread ask --meta`, 0.46.17).
//!
//! - The map is stored with the card and answered on the ask.
//! - It comes back on every read a person, an agent or an App guest makes
//!   (the Thread page, its delta, the catch-up reader), on the overlay
//!   socket's frame and on the room stream's `thread_message`, and it
//!   survives the card's answer.
//! - Each limit is a usage refusal (400 `usage`) and nothing is stored.
//! - A card asked without one is byte for byte what it was before.

use super::*;

fn room(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    (handle, project_id)
}

fn ask(project_id: &str, handle: &str, extra: serde_json::Value) -> CliResponse {
    let mut body = serde_json::json!({ "addr": handle, "prompt": "Ship?", "options": "Ship;Wait" });
    for (k, v) in extra.as_object().expect("extra is an object") {
        body[k] = v.clone();
    }
    post_json("/cli/thread/ask", &passport_params(project_id, handle), body)
}

fn ok_body(resp: &CliResponse) -> serde_json::Value {
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    json_body(resp)
}

fn item_of(page: &serde_json::Value, id: &str) -> serde_json::Value {
    page["items"]
        .as_array()
        .unwrap_or_else(|| panic!("items: {page}"))
        .iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("no item {id}: {page}"))
        .clone()
}

fn guest_seen() -> crate::permission::view::Seen {
    crate::permission::view::Seen::new(k2_core::permission::card::Viewer::Guest { app: None }, 0)
}

const META: &str = r#"{"order":"A-17","qty":3,"rush":true,"weight.kg":1.5}"#;

fn meta_value() -> serde_json::Value {
    serde_json::from_str(META).expect("META parses")
}

#[test]
fn the_map_is_stored_answered_and_read_back_by_people_and_agents() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("cmeta");
    // The caller's key order doesn't matter: keys sort.
    let resp = ask(&project_id, &handle, serde_json::json!({ "meta": {"weight.kg": 1.5, "rush": true, "qty": 3, "order": "A-17"} }));
    let answer = ok_body(&resp);
    assert_eq!(answer["meta"], meta_value(), "{answer}");
    let id = answer["id"].as_str().expect("id").to_string();
    let conv = answer["conversation_id"].as_str().expect("conversation_id").to_string();

    // The stored body, in key order.
    let choice = overlay::get_thread_doc(&conv, &id).expect("doc").expect("card").choice.expect("choice");
    assert_eq!(serde_json::to_string(&choice.meta).expect("json"), META);

    // `k2 thread <addr> --json` is this read.
    let page = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread"));
    assert_eq!(item_of(&page, &id)["doc"]["choice"]["meta"], meta_value(), "{page}");
    // The agent's own read (its passport).
    let mut agent_read = passport_params(&project_id, &handle);
    agent_read.insert("addr".to_string(), handle.clone());
    let page = json_body(&dispatch("/cli/thread", &agent_read).expect("GET thread"));
    assert_eq!(item_of(&page, &id)["doc"]["choice"]["meta"], meta_value(), "{page}");

    // A person answers: the map stays, and the delta's `changed` row has it.
    let r = post_json("/cli/thread/answer", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id, "answer": "Ship" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let delta = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle.as_str()), ("after_seq", "0")])).expect("delta"));
    let row = item_of(&delta, &id);
    assert_eq!(row["doc"]["choice"]["status"], "answered", "{row}");
    assert_eq!(row["doc"]["choice"]["meta"], meta_value(), "{row}");
}

#[test]
fn an_app_guest_reads_the_map_on_the_page_the_catchup_the_socket_and_the_room_stream() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("cmetag");
    let mut rx = crate::overlay_ws::subscribe();
    let answer = ok_body(&ask(&project_id, &handle, serde_json::json!({ "meta": meta_value() })));
    let id = answer["id"].as_str().expect("id").to_string();
    let pass = skin_pass(std::slice::from_ref(&project_id));

    // thread.page (GET /cli/thread under the App pass).
    let page = with_request_skin(Some(pass.clone()), || dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread"));
    let page = ok_body(&page);
    assert_eq!(item_of(&page, &id)["doc"]["choice"]["meta"], meta_value(), "{page}");

    // thread.catchup: every entry is this one reader, for the same pass.
    let caught = with_request_skin(Some(pass.clone()), || {
        thread_read_for(&handle, ThreadReadMode::After(0), 50, None, &guest_seen())
    })
    .unwrap_or_else(|r| panic!("catch-up read: {}", r.body));
    assert_eq!(item_of(&caught, &id)["doc"]["choice"]["meta"], meta_value(), "{caught}");

    // thread.events (the overlay socket) and thread.stream (the room
    // stream): the frame the ask published, as the guest's socket sends it.
    let frames = frames_for(&mut rx, &id);
    let frame = frames.first().unwrap_or_else(|| panic!("the ask published no frame for {id}"));
    let guest = crate::overlay_ws::GuestWire::for_room(pass.clone(), project_id.clone());
    let wire: serde_json::Value =
        serde_json::from_str(&crate::overlay_ws::wire_json_for(frame, Some(&guest), &guest_seen())).expect("socket frame JSON");
    assert_eq!(wire["doc"]["choice"]["meta"], meta_value(), "{wire}");
    let doc = crate::room_stream_ws::doc_for(frame, Some(&guest), &guest_seen());
    let conv = frame.conversation_id.clone().expect("conversation");
    let streamed: serde_json::Value =
        serde_json::from_str(&crate::room_stream_ws::thread_message(&conv, &project_id, frame.seq, &frame.id, 1, doc)).expect("stream JSON");
    assert_eq!(streamed["kind"], "thread_message", "{streamed}");
    assert_eq!(streamed["doc"]["choice"]["meta"], meta_value(), "{streamed}");
}

#[test]
fn each_limit_is_a_usage_refusal_and_nothing_is_stored() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("cmetal");
    let thirty_three: serde_json::Map<String, serde_json::Value> = (0..33).map(|i| (format!("k{i}"), serde_json::json!(i))).collect();
    let cases: Vec<(&str, serde_json::Value, String)> = vec![
        ("an upper-case key", serde_json::json!({ "Order": "x" }), r#"meta key "Order" must match [a-z0-9_.-]{1,64}"#.to_string()),
        ("a 65-byte key", serde_json::json!({ "k".repeat(65): "x" }), format!(r#"meta key "{}" must match [a-z0-9_.-]{{1,64}}"#, "k".repeat(65))),
        ("33 keys", serde_json::Value::Object(thirty_three), "meta takes at most 32 keys".to_string()),
        ("4097 bytes", serde_json::json!({ "k": "v".repeat(4089) }), "meta takes at most 4096 bytes of JSON".to_string()),
        ("a null value", serde_json::json!({ "k": null }), r#"meta value for "k" must be a string, number or boolean"#.to_string()),
        ("an array value", serde_json::json!({ "k": [1] }), r#"meta value for "k" must be a string, number or boolean"#.to_string()),
        ("an object value", serde_json::json!({ "k": {"a": 1} }), r#"meta value for "k" must be a string, number or boolean"#.to_string()),
        ("an array map", serde_json::json!([1, 2]), k2_core::overlay::card_meta::NOT_AN_OBJECT.to_string()),
        ("a number map", serde_json::json!(7), k2_core::overlay::card_meta::NOT_AN_OBJECT.to_string()),
        ("a boolean map", serde_json::json!(true), k2_core::overlay::card_meta::NOT_AN_OBJECT.to_string()),
        ("a string that isn't an object", serde_json::json!("order=A-17"), k2_core::overlay::card_meta::NOT_AN_OBJECT.to_string()),
    ];
    for (what, meta, hint) in cases {
        let r = ask(&project_id, &handle, serde_json::json!({ "meta": meta }));
        assert_error_code(&r, "400 Bad Request", "usage");
        assert_eq!(json_body(&r)["error"]["hint"], hint.as_str(), "{what}: {}", r.body);
    }
    let page = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread"));
    assert_eq!(page["items"], serde_json::json!([]), "a refused ask stores nothing: {page}");

    // At the limits it is taken: 32 keys, 64-byte keys, 4096 bytes.
    let thirty_two: serde_json::Map<String, serde_json::Value> = (0..32).map(|i| (format!("k{i}"), serde_json::json!(i))).collect();
    for meta in [serde_json::Value::Object(thirty_two), serde_json::json!({ "k".repeat(64): "x" }), serde_json::json!({ "k": "v".repeat(4088) })] {
        let a = ok_body(&ask(&project_id, &handle, serde_json::json!({ "meta": meta.clone() })));
        assert_eq!(a["meta"], meta, "{a}");
    }
}

/// The pre-change bytes: the ask's answer, the stored card body and the
/// socket frame's card, for a card asked with no map (and with an empty
/// one, which stores none).
#[test]
fn a_card_with_no_map_is_byte_identical() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("cmetab");
    for extra in [serde_json::json!({}), serde_json::json!({ "meta": {} })] {
        let mut rx = crate::overlay_ws::subscribe();
        let resp = ask(&project_id, &handle, extra.clone());
        let a = ok_body(&resp);
        let (id, conv) = (a["id"].as_str().expect("id").to_string(), a["conversation_id"].as_str().expect("conv").to_string());
        let exp = a["expiresAt"].as_i64().expect("expiresAt");
        // The answer, built exactly as before the map existed.
        let before = serde_json::json!({
            "ok": true,
            "id": id,
            "from": a["from"],
            "prompt": "Ship?",
            "options": ["Ship", "Wait"],
            "allow_custom": false,
            "status": "pending",
            "expiresAt": exp,
            "kind": "choice",
            "seq": a["seq"],
            "conversation_id": conv,
            "addr": handle,
        });
        assert_eq!(resp.body, before.to_string(), "{extra}: the answer's bytes");

        let doc = overlay::get_thread_doc(&conv, &id).expect("doc").expect("card");
        let stored = serde_json::to_string(&doc.choice).expect("json");
        assert_eq!(
            stored,
            format!(r#"{{"prompt":"Ship?","options":[{{"label":"Ship"}},{{"label":"Wait"}}],"allow_custom":false,"status":"pending","expiresAt":{exp}}}"#),
            "{extra}: the stored card body"
        );

        let frames = frames_for(&mut rx, &id);
        let frame = frames.first().unwrap_or_else(|| panic!("no frame for {id}"));
        let seen = crate::permission::view::Seen::new(k2_core::permission::card::Viewer::OwnerToken, 0);
        let wire = crate::overlay_ws::wire_json_for(frame, None, &seen);
        let card = format!(r#""choice":{{"allow_custom":false,"expiresAt":{exp},"options":[{{"label":"Ship"}},{{"label":"Wait"}}],"prompt":"Ship?","status":"pending"}}"#);
        assert!(wire.contains(&card), "{extra}: the frame's card bytes\n want {card}\n in {wire}");
        assert!(!wire.contains("\"meta\""), "{extra}: no meta field anywhere: {wire}");
    }
}
