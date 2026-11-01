use super::*;

fn frame(collection: &str, conv: &str, id: &str, activity: Option<Value>, rev: u64) -> OverlayFrame {
    OverlayFrame {
        collection: collection.into(),
        seq: 3,
        id: id.into(),
        doc: (collection == "thread").then(|| {
            k2_core::overlay::OverlayDoc::text(id.into(), "cortana".into(), "owner".into(), "hello".into(), "thread")
        }),
        activity,
        conversation_id: Some(conv.into()),
        rev,
    }
}

fn code(text: &str) -> Option<Result<ViewFrame, &'static str>> {
    parse(text).map(|r| r.map_err(|e| e.0))
}

#[test]
fn viewing_frames_are_parsed_loudly() {
    assert_eq!(
        code(r#"{"t":"viewing","conversationId":" c-1 ","visible":true}"#),
        Some(Ok(ViewFrame { conversation: "c-1".into(), visible: true })),
        "trimmed"
    );
    assert_eq!(
        code(r#"{"t":"viewing","conversationId":"c-1","visible":false}"#),
        Some(Ok(ViewFrame { conversation: "c-1".into(), visible: false }))
    );
    // Not ours: ignored.
    assert_eq!(code(r#"{"t":"inbox","all":true}"#), None);
    assert_eq!(code("not json"), None);
    // Ours but wrong: answered.
    assert_eq!(code(r#"{"t":"viewing","visible":true}"#), Some(Err("usage")), "the overlay's frame has no id");
    assert_eq!(code(r#"{"t":"viewing","conversationId":"","visible":true}"#), Some(Err("usage")));
    assert_eq!(code(r#"{"t":"viewing","conversationId":7,"visible":true}"#), Some(Err("usage")));
    assert_eq!(code(r#"{"t":"viewing","conversationId":"c-1","visible":"yes"}"#), Some(Err("usage")));
    assert_eq!(code(r#"{"t":"viewing","conversationId":"c-1"}"#), Some(Err("usage")));
    let at_max = "c".repeat(MAX_ID_CHARS);
    let over = "c".repeat(MAX_ID_CHARS + 1);
    assert_eq!(
        code(&json!({ "t": "viewing", "conversationId": at_max, "visible": true }).to_string()),
        Some(Ok(ViewFrame { conversation: at_max.clone(), visible: true }))
    );
    assert_eq!(code(&json!({ "t": "viewing", "conversationId": over, "visible": true }).to_string()), Some(Err("usage")));
    // Over 4 KiB: ignored, not parsed.
    let big = json!({ "t": "viewing", "conversationId": "c-1", "visible": true, "pad": "x".repeat(MAX_FRAME_BYTES) }).to_string();
    assert!(big.len() > MAX_FRAME_BYTES);
    assert_eq!(code(&big), None);
}

#[test]
fn only_a_login_views_and_never_a_hidden_grok_bot_thread() {
    let _home = crate::test_support::TempHome::new();
    assert_eq!(authorize(None, "c-1").map_err(|e| e.0), Err("unauthorized"));
    assert_eq!(authorize(Some("owner"), "c-1").map_err(|e| e.0), Ok(()));
    assert_eq!(authorize(Some("gina"), "c-1").map_err(|e| e.0), Ok(()), "a Connect login reads every Thread");
    let gb = k2_core::external_agents::collect::conversation_id("Bot-1");
    assert_eq!(authorize(Some("owner"), &gb).map_err(|e| e.0), Err("forbidden"), "Show is off by default");
    assert_eq!(authorize(Some("owner"), &gb.to_ascii_uppercase()).map_err(|e| e.0), Err("forbidden"));
    k2_core::external_agents::settings::save(&k2_core::external_agents::settings::LocalGrokBotsSettings { show: true, send: false })
        .expect("save");
    assert_eq!(authorize(Some("owner"), &gb).map_err(|e| e.0), Ok(()));
    assert_eq!(authorize(None, &gb).map_err(|e| e.0), Err("unauthorized"));
}

#[test]
fn the_wire_shapes_are_the_phone_contract() {
    let opened = Opened { rev: 41, conv_rev: 39, turn: Some(json!({ "turnId": "t1", "state": "working" })) };
    let v: Value = serde_json::from_str(&ok_json("c-1", &opened, None)).expect("json");
    assert_eq!(
        v,
        json!({
            "kind": "viewing_ok",
            "conversation_id": "c-1",
            "rev": 41,
            "convRev": 39,
            "instanceId": crate::boot_status::instance_id(),
            "turn": { "turnId": "t1", "state": "working" },
        })
    );
    let idle = Opened { rev: 5, conv_rev: 0, turn: None };
    let v: Value = serde_json::from_str(&ok_json("c-2", &idle, Some("lagged"))).expect("json");
    assert_eq!(v["turn"], Value::Null, "{v}");
    assert_eq!(v["reason"], "lagged", "{v}");
    assert!(v.as_object().expect("object").contains_key("turn"), "turn is always there: {v}");

    let e: Value = serde_json::from_str(&error_json("forbidden", "no", Some("c-1"))).expect("json");
    assert_eq!(e, json!({ "kind": "viewing_error", "code": "forbidden", "hint": "no", "conversation_id": "c-1" }));
    let e: Value = serde_json::from_str(&error_json("usage", "bad", None)).expect("json");
    assert!(e.get("conversation_id").is_none(), "{e}");

    let f = frame("activity", "c-1", "turn-1", Some(json!({ "turnId": "turn-1", "phase": "tool" })), 0);
    let a: Value = serde_json::from_str(&activity_json("c-1", &f)).expect("json");
    assert_eq!(
        a,
        json!({
            "kind": "thread_activity",
            "conversation_id": "c-1",
            "collection": "activity",
            "seq": 0,
            "id": "turn-1",
            "activity": { "turnId": "turn-1", "phase": "tool" },
        })
    );
    let snap = json!({ "turnId": "turn-2", "state": "working" });
    let r: Value = serde_json::from_str(&activity_reset_json("c-1", Some(&snap))).expect("json");
    assert_eq!(
        r,
        json!({
            "kind": "thread_activity_reset",
            "conversation_id": "c-1",
            "collection": "activity_reset",
            "seq": 0,
            "id": "turn-2",
            "activity": snap,
        })
    );
    let r: Value = serde_json::from_str(&activity_reset_json("c-1", None)).expect("json");
    assert_eq!((r["id"].clone(), r["activity"].clone()), (Value::Null, Value::Null), "{r}");
    for k in VIEWING_KINDS {
        assert!(!crate::inbox_stream::INBOX_KINDS.contains(k), "{k} is not an inbox kind");
    }
}

#[test]
fn a_view_coalesces_repeats_and_lapses_with_the_registry_ttl() {
    let t0 = Instant::now();
    let mut v = HubView::new("c-1", 10, t0);
    assert_eq!(v.until(), t0 + thread_viewers::VIEW_TTL, "the registry's 90 s");
    assert!(!v.repeat(t0 + Duration::from_secs(4)), "inside 5 s: no registry touch");
    assert_eq!(v.until(), t0 + thread_viewers::VIEW_TTL, "a coalesced repeat doesn't move the TTL");
    assert!(v.repeat(t0 + Duration::from_secs(30)), "the 30 s repeat refreshes");
    assert_eq!(v.until(), t0 + Duration::from_secs(30) + thread_viewers::VIEW_TTL);
    assert!(!v.repeat(t0 + Duration::from_secs(31)));
}

#[test]
fn a_view_counts_only_its_own_new_frames_and_follows_a_move() {
    let mut v = HubView::new("c-1", 10, Instant::now());
    // Covered by viewing_ok (rev <= 10): never forwarded.
    assert_eq!(v.hit(&frame("thread", "c-1", "d1", None, 10)), None);
    assert_eq!(v.hit(&frame("thread", "c-1", "d2", None, 11)), Some(Hit::Thread { conversation: "c-1".into() }));
    assert_eq!(v.hit(&frame("thread", "c-9", "d3", None, 12)), None, "another chat");
    assert_eq!(v.hit(&frame("chatter", "c-1", "d4", None, 13)), None, "chatter never");
    let addr = frame("address", "c-1", "c-1", Some(json!({ "address": "sales/2", "previous": "sales/1" })), 14);
    assert!(matches!(v.hit(&addr), Some(Hit::Address { .. })));
    // A move is published under both keys; only the `from` copy counts.
    let mv = |key: &str, rev: u64| frame("moved", key, "c-2", Some(json!({ "from": "c-1", "to": "c-2" })), rev);
    assert_eq!(v.hit(&mv("c-1", 15)), Some(Hit::Moved { from: "c-1".into(), to: "c-2".into() }));
    assert_eq!(v.hit(&mv("c-2", 16)), None);
    assert_eq!(v.apply_move("c-9", "c-8"), None, "another chat's move");
    assert_eq!(v.apply_move("c-1", "c-2"), Some(("c-1".into(), "c-2".into())));
    assert_eq!(v.conversation, "c-2");
    assert_eq!(v.hit(&frame("thread", "c-2", "d5", None, 17)), Some(Hit::Thread { conversation: "c-2".into() }));
    assert_eq!(v.hit(&frame("thread", "c-1", "d6", None, 18)), None, "the old key is gone");
    assert!(v.owns_activity(&frame("activity", "c-2", "t", Some(json!({})), 0)));
    assert!(!v.owns_activity(&frame("activity", "c-1", "t", Some(json!({})), 0)));
}

/// The slot is a `thread_viewers` socket id (never the hub's subscriber
/// id) and dropping it clears every view it held.
#[test]
fn dropping_the_slot_clears_its_views() {
    let conv = format!("hub-slot-{}", uuid::Uuid::new_v4());
    let slot = Slot::new();
    let id = slot.0;
    assert!(Slot::new().0 > id, "ids come from thread_viewers::next_socket_id");
    thread_viewers::global().set(id, "owner", &conv, true, std::time::Instant::now());
    assert!(thread_viewers::global().is_viewing("owner", &conv, std::time::Instant::now()));
    drop(slot);
    assert!(!thread_viewers::global().is_viewing("owner", &conv, std::time::Instant::now()));
}

#[test]
fn convrev_is_the_last_rev_of_that_chat_and_never_above_rev() {
    let a = format!("hub-cr-a-{}", uuid::Uuid::new_v4());
    let b = format!("hub-cr-b-{}", uuid::Uuid::new_v4());
    assert_eq!(overlay_ws::conv_rev(&a), 0, "nothing published");
    let publish = |conv: &str, collection: &str| {
        overlay_ws::publish(frame(collection, conv, &uuid::Uuid::new_v4().to_string(), None, 0));
    };
    publish(&a, "thread");
    let after_a = overlay_ws::conv_rev(&a);
    assert!(after_a > 0);
    publish(&b, "thread");
    publish(&b, "chatter");
    assert_eq!(overlay_ws::conv_rev(&a), after_a, "another chat's frames don't move it");
    let (rev, conv_rev) = overlay_ws::rev_snapshot(&a);
    assert_eq!(conv_rev, after_a);
    assert!(rev > conv_rev, "rev is global: {rev} vs {conv_rev}");
    let b_thread = overlay_ws::conv_rev(&b);
    publish(&b, "chatter");
    assert_eq!(overlay_ws::conv_rev(&b), b_thread, "chatter is not the Thread");
    let bumped = overlay_ws::bump_rev("import", &a);
    assert_eq!(overlay_ws::conv_rev(&a), bumped, "a frameless Thread change counts");
}
