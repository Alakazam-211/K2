//! History, restore and snapshot route tests (prd-garden-board-needs-v1
//! §14 "k2-daemon"): through `zen_data_routes::handle`, so every gate runs.
//! Temp HOME + scoped DB.

use serde_json::{json, Value};

use k2_core::garden_data::MadeBy;

use crate::garden_data::agent_routes::tests::{agent, env, err, ok, person_grant};
use crate::garden_data::call::door_tests::{builder, door};
use crate::zen_data_routes::DataWho;

fn owner() -> DataWho {
    DataWho::Owner
}

fn put(who: &DataWho, key: &str, value: Value) -> i64 {
    ok(who, "POST", "/cli/zen/data/put", json!({ "widget": "board", "key": key, "value": value }), &[])["rev"]
        .as_i64()
        .expect("a rev")
}

fn get(key: &str) -> Value {
    ok(&owner(), "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", key)])
}

#[test]
fn the_widget_door_reads_history_restores_and_snapshots() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    d.call("store.put", json!(["board/b1/obj/1", { "x": 1 }]));
    d.call("store.put", json!(["board/b1/obj/1", { "x": 2 }]));
    d.call("store.put", json!(["board/b1/versions/v1", { "label": "v1" }]));
    let h = d.call("store.history", json!([{ "prefix": "board/b1/obj/", "limit": 10 }]));
    assert_eq!((h["head"].as_i64(), h["oldest"].as_i64()), (Some(3), Some(0)), "{h}");
    let items = h["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0], json!({
        "key": "board/b1/obj/1", "rev": 2, "deleted": false, "value": { "x": 2 }, "prev": { "x": 1 },
        "prevKind": "value", "by": "you via board", "at": items[0]["at"],
    }));
    assert_eq!(items[1]["prevKind"], "none");
    assert_eq!(h["next"], Value::Null);
    // A dry run counts and writes nothing.
    let dry = d.call("store.restore", json!([{ "toRev": 1, "prefixes": ["board/b1/"], "except": ["board/b1/versions/"], "dryRun": true }]));
    assert_eq!(dry, json!({ "id": null, "rev": null, "written": 1, "deleted": 0, "families": ["board/"], "keys": ["board/b1/obj/1"] }));
    assert_eq!(d.call("store.get", json!(["board/b1/obj/1"]))["value"], json!({ "x": 2 }));
    // The restore: one new rev, marked, the versions left alone.
    let r = d.call("store.restore", json!([{ "toRev": 1, "prefixes": ["board/b1/"], "except": ["board/b1/versions/"] }]));
    let id = r["id"].as_str().expect("an id").to_string();
    assert_eq!((r["rev"].as_i64(), r["written"].as_i64()), (Some(4), Some(1)));
    assert_eq!(d.call("store.get", json!(["board/b1/obj/1"]))["value"], json!({ "x": 1 }));
    assert_eq!(d.call("store.get", json!(["board/b1/versions/v1"]))["value"], json!({ "label": "v1" }));
    // The mark reaches store.changes (what subscribe pages) with the restorer.
    let ch = d.call("store.changes", json!([{ "since": 3 }]));
    assert_eq!(ch["items"][0]["restore"], json!(id), "{ch}");
    assert_eq!(ch["items"][0]["by"], "you via board");
    let plain = d.call("store.changes", json!([{ "since": 0 }]));
    assert!(plain["items"].as_array().unwrap().iter().filter(|c| c["rev"].as_i64() < Some(4)).all(|c| c.get("restore").is_none()));
    // A second restore within 10 s is rate limited.
    assert_eq!(d.refuse("store.restore", json!([{ "toRev": 3, "prefixes": ["board/"] }])), "rate_limited");
    super::forget_restore_slots();
    // Snapshots: a pending file in this widget's data; keep it, change, restore.
    let s = d.call("store.snapshot", json!([{ "prefixes": ["board/b1/"], "except": ["board/b1/versions/"], "label": "first\u{202e}" }]));
    assert_eq!((s["rev"].as_i64(), s["keys"].as_i64()), (Some(4), Some(1)), "{s}");
    let sref = s["ref"].as_str().expect("a ref").to_string();
    d.call("store.put", json!(["board/b1/versions/v2", { "label": "v2", "ref": sref }]));
    d.call("store.put", json!(["board/b1/obj/1", { "x": 9 }]));
    // Another key set than the snapshot's own is refused (R1).
    assert_eq!(d.refuse("store.restore", json!([{ "snapshot": sref, "prefixes": ["board/"] }])), "bad_request");
    let back = d.call("store.restore", json!([{ "snapshot": sref }]));
    assert_eq!(back["written"], 1, "{back}");
    assert_eq!(d.call("store.get", json!(["board/b1/obj/1"]))["value"], json!({ "x": 1 }));
    assert!(d.call("store.get", json!(["board/b1/versions/v2"])).is_object(), "the version survives its own restore");
    // A pre-op snapshot was taken before each restore (V16: listed and pruned).
    let pre = k2_core::garden_data::snapshots::list_pre_op(&crate::garden_data::export_routes::snapshot_dir()).expect("list");
    assert_eq!(pre.iter().filter(|n| n.contains("-restore")).count(), 2, "{pre:?}");
    // Options are checked.
    assert_eq!(d.refuse("store.restore", json!([{ "toRev": 1, "at": 5 }])), "bad_request");
    assert_eq!(d.refuse("store.restore", json!([{ "prefixes": ["board/"] }])), "bad_request");
    assert_eq!(d.refuse("store.history", json!([{ "limit": 201 }])), "bad_request");
    assert_eq!(d.refuse("store.history", json!([{ "nope": 1 }])), "bad_request");
}

#[test]
fn the_widget_door_needs_the_caps_and_its_makers_rights() {
    let d = door(&["store:read"]);
    d.stamp(MadeBy::Owner);
    d.call("store.history", json!([{}]));
    assert_eq!(d.refuse("store.restore", json!([{ "toRev": 0 }])), "cap_not_granted");
    assert_eq!(d.refuse("store.snapshot", json!([{}])), "cap_not_granted");
    // An agent-made widget is limited to its maker's grants.
    crate::garden_data::call::door_tests::set_caps(&d.f, &["store:read", "store:write"]);
    d.stamp(builder());
    person_grant("board", "w-builder", "write", "pub/");
    put(&owner(), "pub/a", json!(1));
    put(&owner(), "private/b", json!(1));
    put(&owner(), "pub/a", json!(2));
    put(&owner(), "private/b", json!(2));
    let h = d.call("store.history", json!([{}]));
    assert!(h["items"].as_array().unwrap().iter().all(|i| i["key"].as_str().unwrap().starts_with("pub/")), "{h}");
    assert_eq!(d.refuse("store.restore", json!([{ "toRev": 2 }])), "not_granted", "the whole space touches private/");
    assert_eq!(get("private/b")["value"], 2, "nothing written");
    let r = d.call("store.restore", json!([{ "toRev": 2, "prefixes": ["pub/"] }]));
    assert_eq!(r["written"], 1);
    assert_eq!(d.refuse("store.snapshot", json!([{}])), "not_granted", "a snapshot never leaves keys out silently");
}

#[test]
fn restore_skips_keep_access_keys_unless_named() {
    let d = door(&["store:read", "store:write"]);
    std::fs::write(
        d.f.widget_dir("board").join("manifest.json"),
        "{\"schema\": 1, \"name\": \"Board\", \"entry\": \"index.html\", \"caps\": [\"store:read\", \"store:write\"], \"restoreSkips\": [\"access/\"]}\n",
    )
    .expect("manifest");
    d.stamp(MadeBy::Owner);
    d.call("store.put", json!(["access/boards/b1", { "who": ["ann"] }]));
    d.call("store.put", json!(["board/b1/obj/1", 1]));
    d.call("store.put", json!(["access/boards/b1", { "who": ["ann", "bob"] }]));
    d.call("store.put", json!(["board/b1/obj/1", 2]));
    let r = d.call("store.restore", json!([{ "toRev": 2 }]));
    assert_eq!(r["families"], json!(["board/"]), "{r}");
    assert_eq!(d.call("store.get", json!(["access/boards/b1"]))["value"], json!({ "who": ["ann", "bob"] }));
    super::forget_restore_slots();
    d.call("store.restore", json!([{ "toRev": 2, "prefixes": ["access/"] }]));
    assert_eq!(d.call("store.get", json!(["access/boards/b1"]))["value"], json!({ "who": ["ann"] }));
}

#[test]
fn the_agent_door_keeps_agents_to_prefixes_and_grants() {
    let _e = env();
    person_grant("board", "w-builder", "write", "board/");
    let a = agent("w-builder");
    put(&a, "board/b1/obj/1", json!(1));
    put(&owner(), "secret/x", json!(1));
    put(&a, "board/b1/obj/1", json!(2));
    put(&owner(), "secret/x", json!(2));
    // History is filtered to the agent's read prefixes.
    let h = ok(&a, "GET", "/cli/zen/data/history", json!({}), &[("widget", "board")]);
    assert!(h["items"].as_array().unwrap().iter().all(|i| i["key"].as_str().unwrap().starts_with("board/")), "{h}");
    assert_eq!(h["items"][0]["by"], "agent:board-builder");
    // A whole-space restore is the person's (R2), even as a dry run.
    let (code, e) = err(&a, "POST", "/cli/zen/data/restore", json!({ "widget": "board", "toRev": 0 }), &[]);
    assert_eq!((code, e.as_str()), (403, "person_only"));
    // Outside its grants: not_granted, nothing written.
    let (code, e) = err(&a, "POST", "/cli/zen/data/restore", json!({ "widget": "board", "toRev": 2, "prefixes": ["secret/"] }), &[]);
    assert_eq!((code, e.as_str()), (403, "not_granted"));
    assert_eq!(get("secret/x")["value"], 2);
    // Inside them: done, marked with the agent as the restorer.
    let r = ok(&a, "POST", "/cli/zen/data/restore", json!({ "widget": "board", "toRev": 1, "prefixes": ["board/b1/"] }), &[]);
    assert_eq!(r["written"], 1);
    let c = ok(&owner(), "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board"), ("since", "4")]);
    assert_eq!((c["items"][0]["restore"].as_str(), c["items"][0]["by"].as_str()), (r["id"].as_str(), Some("agent:board-builder")), "{c}");
    // The person restores the whole space; atLocal is the CLI's --at.
    super::forget_restore_slots();
    let p = ok(&owner(), "POST", "/cli/zen/data/restore", json!({ "widget": "board", "toRev": 0, "dryRun": true }), &[]);
    assert_eq!((p["deleted"].as_i64(), p["families"].clone()), (Some(2), json!(["board/", "secret/"])));
    let p = ok(&owner(), "POST", "/cli/zen/data/restore", json!({ "widget": "board", "toRev": 3 }), &[]);
    assert_eq!(p["rev"], 6, "{p}");
    let (_, e) = err(&owner(), "POST", "/cli/zen/data/restore", json!({ "widget": "board", "atLocal": "yesterday" }), &[]);
    assert_eq!(e, "bad_request");
    // Strict bodies; a rev past the head.
    let (_, e) = err(&owner(), "POST", "/cli/zen/data/restore", json!({ "widget": "board", "toRev": 0, "path": "/x" }), &[]);
    assert_eq!(e, "bad_request");
    let (_, e) = err(&owner(), "POST", "/cli/zen/data/restore", json!({ "widget": "board", "toRev": 99, "dryRun": true }), &[]);
    assert_eq!(e, "bad_request");
    // A snapshot by an agent covers only what it can read; its own
    // snapshot restore names prefixes, so it may restore it.
    let s = ok(&a, "POST", "/cli/zen/data/snapshot", json!({ "widget": "board", "prefixes": ["board/"], "label": "before" }), &[]);
    assert_eq!(s["keys"], 1);
    let (code, e) = err(&a, "POST", "/cli/zen/data/snapshot", json!({ "widget": "board" }), &[]);
    assert_eq!((code, e.as_str()), (403, "not_granted"));
}
