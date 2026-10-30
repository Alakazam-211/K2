use super::*;

fn frame(collection: &str, conv: &str, id: &str, activity: Option<Value>, with_doc: bool) -> OverlayFrame {
    OverlayFrame {
        collection: collection.into(),
        seq: 5,
        id: id.into(),
        doc: with_doc.then(|| {
            k2_core::overlay::OverlayDoc::text(id.into(), "cortana".into(), "owner".into(), "hello".into(), "thread")
        }),
        activity,
        conversation_id: Some(conv.into()),
        rev: 77,
    }
}

fn some(ids: &[&str], addrs: &[&str]) -> Want {
    Want::Some { ids: ids.iter().map(|s| s.to_string()).collect(), addrs: addrs.iter().map(|s| s.to_string()).collect() }
}

fn resolver(addr: &str) -> Option<String> {
    match addr {
        "sales" => Some("c-sales".into()),
        "ops/reviewer" => Some("c-ops-rev".into()),
        _ => None,
    }
}

#[test]
fn subscribe_messages_are_parsed_loudly() {
    assert_eq!(parse_subscribe(r#"{"t":"viewing"}"#), None, "other client text is ignored");
    assert_eq!(parse_subscribe("not json"), None);
    assert_eq!(parse_subscribe(r#"{"t":"inbox","all":true}"#), Some(Ok(Want::All)));
    assert_eq!(parse_subscribe(r#"{"t":"inbox","off":true}"#), Some(Ok(Want::Off)));
    assert_eq!(
        parse_subscribe(r#"{"t":"inbox","conversationIds":["c-1","c-1"," c-2 "],"addrs":["sales"]}"#),
        Some(Ok(some(&["c-1", "c-2"], &["sales"]))),
        "trimmed, deduped"
    );
    let code = |s: &str| parse_subscribe(s).map(|r| r.map_err(|e| e.0));
    assert_eq!(code(r#"{"t":"inbox"}"#), Some(Err("usage")), "an empty set is said out loud");
    assert_eq!(code(r#"{"t":"inbox","addrs":"sales"}"#), Some(Err("usage")));
    assert_eq!(code(r#"{"t":"inbox","conversationIds":[1]}"#), Some(Err("usage")));
    assert_eq!(code(r#"{"t":"inbox","conversationIds":[""]}"#), Some(Err("usage")));
    // 200 in total across both lists; 201 is too many.
    let ids: Vec<String> = (0..150).map(|i| format!("\"c{i}\"")).collect();
    let addrs: Vec<String> = (0..50).map(|i| format!("\"a{i}\"")).collect();
    let ok = format!(r#"{{"t":"inbox","conversationIds":[{}],"addrs":[{}]}}"#, ids.join(","), addrs.join(","));
    assert!(matches!(parse_subscribe(&ok), Some(Ok(Want::Some { .. }))));
    let addrs: Vec<String> = (0..51).map(|i| format!("\"a{i}\"")).collect();
    let over = format!(r#"{{"t":"inbox","conversationIds":[{}],"addrs":[{}]}}"#, ids.join(","), addrs.join(","));
    assert_eq!(code(&over), Some(Err("too_many")));
    let e: Value = serde_json::from_str(&error_json("too_many", "x")).expect("json");
    assert_eq!((e["kind"].clone(), e["code"].clone(), e["max"].clone()), (json!("inbox_error"), json!("too_many"), json!(200)));
}

#[test]
fn the_set_resolves_addrs_and_names_the_unknown() {
    let set = build_set(&some(&["c-1"], &["sales", "ops/reviewer", "nope"]), &resolver).expect("set");
    assert_eq!(set.conversations(), vec!["c-1", "c-ops-rev", "c-sales"]);
    assert_eq!(set.unknown, vec!["nope".to_string()]);
    assert!(set.contains("c-sales") && set.contains("c-1") && !set.contains("c-other"));
    assert!(build_set(&Want::Off, &resolver).is_none());
    assert!(build_set(&Want::All, &resolver).expect("all").contains("anything"));

    let v: Value = serde_json::from_str(&subscribed_json(Some(&set), 48_200)).expect("json");
    assert_eq!(v["kind"], "inbox_subscribed");
    assert_eq!(v["rev"], 48_200);
    assert_eq!(v["instanceId"].as_str().map(str::len), Some(16), "{v}");
    assert_eq!(v["resolved"], json!({ "sales": "c-sales", "ops/reviewer": "c-ops-rev" }));
    assert_eq!(v["unknown"], json!(["nope"]));
    assert_eq!(v["max"], 200);
    assert_eq!(v["all"], false);
    let off: Value = serde_json::from_str(&subscribed_json(None, 1)).expect("json");
    assert_eq!(off["conversations"], json!([]), "off: an empty set");
}

/// Stream scoping: only the set's Thread items, moves and address changes
/// reach the socket; another chat's frames, `chatter`, `chatterlog` and
/// `activity` never do.
#[test]
fn only_the_sets_thread_frames_are_forwarded() {
    let set = build_set(&some(&["c-1"], &["sales"]), &resolver).expect("set");
    assert_eq!(
        classify(&frame("thread", "c-1", "it-1", None, true), &set),
        Some(Hit::Thread { conversation: "c-1".into() })
    );
    assert_eq!(
        classify(&frame("thread", "c-sales", "it-2", None, true), &set),
        Some(Hit::Thread { conversation: "c-sales".into() }),
        "by address"
    );
    assert_eq!(classify(&frame("thread", "c-other", "it-3", None, true), &set), None, "not subscribed");
    assert_eq!(classify(&frame("thread", "c-1", "it-4", None, false), &set), None, "no doc, no message");
    assert_eq!(classify(&frame("chatter", "c-1", "it-5", None, true), &set), None, "agent-to-agent chatter never");
    let act = Some(json!({ "turnId": "t", "state": "working" }));
    assert_eq!(classify(&frame("activity", "c-1", "t", act, false), &set), None, "activity never");
    let mut chatterlog = frame("chatterlog", "c-1", "x", None, true);
    chatterlog.conversation_id = None;
    assert_eq!(classify(&chatterlog, &set), None, "host-wide chatterlog never");
    // `all` gets every Thread item, still never chatter.
    let all = build_set(&Want::All, &resolver).expect("all");
    assert!(classify(&frame("thread", "c-zz", "it-6", None, true), &all).is_some());
    assert_eq!(classify(&frame("chatter", "c-zz", "it-7", None, true), &all), None);

    // An address change of a subscribed chat; not of another.
    let addr = Some(json!({ "address": "sales-2", "previous": "sales" }));
    assert_eq!(
        classify(&frame(overlay_ws::ADDRESS_COLLECTION, "c-sales", "c-sales", addr.clone(), false), &set),
        Some(Hit::Address { conversation: "c-sales".into(), address: json!("sales-2"), previous: json!("sales") })
    );
    assert_eq!(classify(&frame(overlay_ws::ADDRESS_COLLECTION, "c-9", "c-9", addr, false), &set), None);
}

/// One `thread_moved` per move (the bus publishes it under both keys), and
/// the set swaps `from` for `to` in place.
#[test]
fn a_move_is_one_frame_and_swaps_the_set_in_place() {
    let mut set = build_set(&some(&["c-1", "c-2"], &["sales"]), &resolver).expect("set");
    let mv = |under: &str, from: &str, to: &str| frame(overlay_ws::MOVED_COLLECTION, under, to, Some(json!({ "from": from, "to": to })), false);
    assert_eq!(classify(&mv("c-1", "c-1", "c-1b"), &set), Some(Hit::Moved { from: "c-1".into(), to: "c-1b".into() }));
    assert_eq!(classify(&mv("c-1b", "c-1", "c-1b"), &set), None, "the `to` copy is not a second frame");
    assert_eq!(classify(&mv("c-x", "c-x", "c-y"), &set), None, "not ours");
    assert!(set.apply_move("c-1", "c-1b"));
    assert_eq!(set.ids, vec!["c-1b".to_string(), "c-2".to_string()], "in place, count unchanged");
    // Into a chat already in the set: `from` just leaves.
    assert!(set.apply_move("c-1b", "c-2"));
    assert_eq!(set.ids, vec!["c-2".to_string()]);
    // A resolved address follows its chat.
    assert!(set.apply_move("c-sales", "c-sales-2"));
    assert_eq!(set.resolved["sales"], "c-sales-2");

    let m: Value = serde_json::from_str(&moved_message("c-1", "c-1b", 40, 48_215)).expect("json");
    assert_eq!(m, json!({ "kind": "thread_moved", "from": "c-1", "to": "c-1b", "seq": 40, "rev": 48_215 }));
}

#[test]
fn re_resolving_reports_a_changed_address() {
    let mut set = build_set(&some(&[], &["sales", "nope"]), &resolver).expect("set");
    assert!(!reresolve(&mut set, &resolver), "same answers: no new frame");
    let moved = |a: &str| match a {
        "sales" => Some("c-sales-new".to_string()),
        "nope" => Some("c-nope".to_string()),
        _ => None,
    };
    assert!(reresolve(&mut set, &moved));
    assert_eq!(set.resolved["sales"], "c-sales-new");
    assert_eq!(set.resolved["nope"], "c-nope", "an unknown addr that now resolves joins");
    assert!(set.unknown.is_empty());
}

/// Every `thread_message` names its item, and the 32 KiB cap drops the
/// body first, then the doc.
#[test]
fn a_thread_message_always_names_its_item_and_fits_the_cap() {
    let small: Value = serde_json::from_str(&thread_message("c-1", 12, "6b1e", 48_213, json!({ "id": "6b1e", "body": "hi" }))).expect("json");
    assert_eq!(
        small,
        json!({ "kind": "thread_message", "conversation_id": "c-1", "collection": "thread", "seq": 12, "id": "6b1e", "rev": 48_213, "doc": { "id": "6b1e", "body": "hi" } })
    );
    let cap = crate::room_stream_ws::FRAME_CAP_BYTES;
    let big = thread_message("c-1", 13, "77aa", 48_214, json!({ "id": "77aa", "body": "x".repeat(cap + 1) }));
    assert!(big.len() <= cap, "{}", big.len());
    let v: Value = serde_json::from_str(&big).expect("json");
    assert_eq!((v["truncated"].clone(), v["bodyBytes"].clone()), (json!(true), json!(cap + 1)));
    assert!(v["doc"].get("body").is_none());
    assert_eq!((v["conversation_id"].clone(), v["seq"].clone(), v["id"].clone(), v["rev"].clone()), (json!("c-1"), json!(13), json!("77aa"), json!(48_214)));
}

/// The doc is the overlay socket's own doc for a login (`wire_json_for`).
#[test]
fn the_doc_is_the_overlay_sockets_doc() {
    let f = frame("thread", "c-1", "it-1", None, true);
    let seen = crate::overlay_ws::socket_seen(&std::collections::HashMap::new(), false, Some("owner"));
    let overlay: Value = serde_json::from_str(&crate::overlay_ws::wire_json_for(&f, None, &seen)).expect("json");
    assert_eq!(doc_for_login(&f, &seen), overlay["doc"]);
    assert_eq!(doc_for_login(&f, &seen)["body"], "hello");
}

#[test]
fn reset_and_kinds() {
    assert_eq!(reset_json("lagged"), r#"{"kind":"inbox_reset","reason":"lagged"}"#);
    assert_eq!(INBOX_KINDS.len(), 6);
}
