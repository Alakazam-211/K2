use super::*;

fn readable(pairs: &[(&str, &str)]) -> Readable {
    pairs.iter().map(|(r, c)| (r.to_string(), c.to_string())).collect()
}

#[test]
fn path_and_param_are_the_app_surface_rows() {
    assert_eq!(ROOM_STREAM_WS_PATH, "/cli/thread/stream");
    assert_ne!(ROOM_STREAM_WS_PATH, overlay_ws::OVERLAY_WS_PATH, "beside the per-room socket, not instead of it");
    assert_eq!(k2_core::app_surface::ws_param(ROOM_STREAM_WS_PATH), Some(ROOMS_PARAM), "GA1: the socket names its parameter");
    assert_eq!(k2_core::app_surface::verb_for("WS", ROOM_STREAM_WS_PATH), Some("thread.stream"));
}

#[test]
fn rooms_param_is_all_or_a_bounded_list() {
    assert_eq!(parse_rooms_param("all"), Ok(Want::All));
    assert_eq!(parse_rooms_param(" ALL "), Ok(Want::All));
    assert_eq!(parse_rooms_param("a, b,a,,c"), Ok(Want::Some(vec!["a".into(), "b".into(), "c".into()])), "trimmed, deduped");
    assert!(parse_rooms_param("").is_err());
    assert!(parse_rooms_param(" , ").is_err());
    let many: Vec<String> = (0..=MAX_ROOMS).map(|i| format!("r{i}")).collect();
    let err = parse_rooms_param(&many.join(",")).expect_err("201 rooms");
    assert!(err.contains("at most 200"), "{err}");
}

#[test]
fn subscribe_messages_replace_the_set() {
    assert_eq!(parse_subscribe(r#"{"t":"viewing","visible":true}"#), None, "other frames are ignored");
    assert_eq!(parse_subscribe("not json"), None);
    assert_eq!(parse_subscribe(r#"{"t":"inbox","all":true}"#), Some(Ok(Want::All)));
    assert_eq!(parse_subscribe(r#"{"t":"inbox","off":true}"#), Some(Ok(Want::Off)));
    assert_eq!(
        parse_subscribe(r#"{"t":"inbox","rooms":["a"],"conversationIds":["c-2","a"]}"#),
        Some(Ok(Want::Some(vec!["a".into(), "c-2".into()])))
    );
    assert_eq!(parse_subscribe(r#"{"t":"inbox","rooms":"a"}"#).map(|r| r.map_err(|e| e.0)), Some(Err("usage")));
    assert_eq!(parse_subscribe(r#"{"t":"inbox","rooms":[1]}"#).map(|r| r.map_err(|e| e.0)), Some(Err("usage")));
    assert_eq!(parse_subscribe(r#"{"t":"inbox"}"#).map(|r| r.map_err(|e| e.0)), Some(Err("usage")), "an empty set is `off`, said out loud");
    let many: Vec<String> = (0..=MAX_ROOMS).map(|i| format!("\"r{i}\"")).collect();
    let msg = format!(r#"{{"t":"inbox","rooms":[{}]}}"#, many.join(","));
    assert_eq!(parse_subscribe(&msg).map(|r| r.map_err(|e| e.0)), Some(Err("too_many")));
}

/// R3: a requested id that isn't readable is `unknown`, the same answer
/// for a room that exists on the server and one that doesn't; the view
/// never names a room outside `readable`.
#[test]
fn the_view_is_only_readable_rooms() {
    let r = readable(&[("room-a", "conv-a"), ("room-b", "conv-b")]);
    let all = compute_view(&r, &Want::All);
    assert_eq!(all.rooms, r);
    assert!(all.unknown.is_empty());

    let some = compute_view(&r, &Want::Some(vec!["room-a".into(), "conv-b".into(), "room-c".into(), "nope".into()]));
    assert_eq!(some.rooms, r, "a room by id or by its pinned conversation");
    assert_eq!(some.unknown, vec!["room-c".to_string(), "nope".to_string()]);
    assert_eq!(some.room_of("conv-b"), Some("room-b"));
    assert_eq!(some.room_of("conv-c"), None);

    assert_eq!(compute_view(&r, &Want::Off), View::default());

    let big: Readable = (0..MAX_ROOMS + 5).map(|i| (format!("r{i:03}"), format!("c{i}"))).collect();
    assert_eq!(compute_view(&big, &Want::All).rooms.len(), MAX_ROOMS, "all is capped too");
}

#[test]
fn a_thread_message_always_names_its_item_and_fits_the_cap() {
    let small = thread_message("c-1", "room-1", 7, "doc-7", 3, json!({ "id": "doc-7", "body": "hi" }));
    let v: Value = serde_json::from_str(&small).expect("json");
    assert_eq!(v["kind"], "thread_message");
    assert_eq!(v["conversation_id"], "c-1");
    assert_eq!(v["room"], "room-1");
    assert_eq!(v["collection"], "thread");
    assert_eq!((v["seq"].clone(), v["id"].clone(), v["rev"].clone()), (json!(7), json!("doc-7"), json!(3)));
    assert_eq!(v["doc"]["body"], "hi");
    assert!(v.get("truncated").is_none());

    let body = "x".repeat(FRAME_CAP_BYTES + 10);
    let big = thread_message("c-1", "room-1", 8, "doc-8", 4, json!({ "id": "doc-8", "body": body }));
    assert!(big.len() <= FRAME_CAP_BYTES, "{}", big.len());
    let v: Value = serde_json::from_str(&big).expect("json");
    assert_eq!(v["truncated"], true);
    assert_eq!(v["bodyBytes"], FRAME_CAP_BYTES + 10);
    assert!(v["doc"].get("body").is_none() && v["doc"]["id"] == "doc-8", "{v}");

    let huge = thread_message("c-1", "room-1", 9, "doc-9", 5, json!({ "id": "doc-9", "choice": { "prompt": "y".repeat(FRAME_CAP_BYTES) } }));
    let v: Value = serde_json::from_str(&huge).expect("json");
    assert!(v.get("doc").is_none(), "still over: the doc goes");
    assert_eq!((v["conversation_id"].clone(), v["seq"].clone(), v["id"].clone(), v["rev"].clone()), (json!("c-1"), json!(9), json!("doc-9"), json!(5)));
}

/// C4 / R3: a guest's `inbox_subscribed` carries the socket's own `rev`
/// and no `instanceId`; a login's carries the server's.
#[test]
fn subscribed_names_only_the_set() {
    let view = compute_view(&readable(&[("room-a", "conv-a")]), &Want::Some(vec!["room-a".into(), "room-z".into()]));
    let g: Value = serde_json::from_str(&subscribed_json(&view, &Want::Some(vec![]), 0, true)).expect("json");
    assert_eq!(g["kind"], "inbox_subscribed");
    assert_eq!(g["revScope"], "socket");
    assert!(g.get("instanceId").is_none(), "{g}");
    assert_eq!(g["rooms"], json!(["room-a"]));
    assert_eq!(g["conversations"], json!(["conv-a"]));
    assert_eq!(g["resolved"], json!({ "room-a": "conv-a" }));
    assert_eq!(g["unknown"], json!(["room-z"]));
    assert_eq!(g["max"], MAX_ROOMS);
    let l: Value = serde_json::from_str(&subscribed_json(&view, &Want::All, 41, false)).expect("json");
    assert_eq!(l["revScope"], "server");
    assert_eq!(l["all"], true);
    assert_eq!(l["instanceId"].as_str().map(str::len), Some(16), "{l}");
}

#[test]
fn off_a_link_room_slots_are_free() {
    let mut s = RoomSlots { app_id: None, held: 0 };
    assert_eq!(s.resize(150), Ok(()));
    assert_eq!(s.held, 0, "quotas count only on an App's gateway link");
}
