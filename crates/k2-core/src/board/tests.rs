//! The Board's rules (plan-board-garden-v1; Rosson 2026-11-01): the
//! cascades, templates, `apply` id mapping, typed refusals (never a cut
//! value or a skipped op), the lock rule, people by stable key, and the
//! export envelope.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Map, Value};

use super::export::{envelope, BlobBytes};
use super::ops::{add_vote, cascade, check_format, map_ids, parse_ops, plan, toggle_reaction, upgrade_plan, view, Board, Ctx, Op, Stored, Write};
use super::schema::{self, read_shape};
use super::templates::{build, TEMPLATES};
use super::{meta_key, obj_key, BoardError};

fn ctx() -> Ctx {
    Ctx { by: "agent:pat".into(), at: "2026-11-01T12:00:00Z".into(), batch: "cli-0000000001".into() }
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().expect("an object").clone()
}

/// A board holding these objects (rev 1 each).
fn board(objs: &[Value]) -> Board {
    let entries = objs.iter().map(|o| Stored { key: obj_key("b1a2b3c", o["id"].as_str().expect("id")), rev: 1, value: o.clone() });
    Board::from_entries("b1a2b3c", entries)
}

/// Apply writes to a board in memory (as the store would, rev 2).
fn after(b: &Board, ws: &[Write]) -> Board {
    let mut entries: Vec<Stored> = b.objects.values().cloned().collect();
    if let Some(m) = &b.meta {
        entries.push(m.clone());
    }
    for w in ws {
        entries.retain(|e| e.key != w.key);
        if let Some(v) = &w.value {
            entries.push(Stored { key: w.key.clone(), rev: w.if_rev + 1, value: v.clone() });
        }
    }
    Board::from_entries(&b.id, entries)
}

fn err(r: Result<Vec<Write>, BoardError>) -> BoardError {
    r.expect_err("refused")
}

fn sample() -> Board {
    board(&[
        json!({"id": "h-aaaaaaaaaa", "type": "shape", "x": 0, "y": 0, "w": 100, "h": 50, "z": 1}),
        json!({"id": "h-bbbbbbbbbb", "type": "shape", "x": 300, "y": 0, "w": 100, "h": 50, "z": 2}),
        json!({"id": "c-cccccccccc", "type": "connector", "from": {"id": "h-aaaaaaaaaa", "anchor": "auto"}, "to": {"id": "h-bbbbbbbbbb", "anchor": "auto"}, "z": 3}),
        json!({"id": "m-dddddddddd", "type": "mind", "text": "root", "parent": "", "z": 4}),
        json!({"id": "m-eeeeeeeeee", "type": "mind", "text": "kid", "parent": "m-dddddddddd", "z": 5}),
        json!({"id": "m-ffffffffff", "type": "mind", "text": "grandkid", "parent": "m-eeeeeeeeee", "z": 6}),
        json!({"id": "c-gggggggggg", "type": "connector", "from": {"id": "m-ffffffffff", "anchor": "auto"}, "to": {"x": 5, "y": 5}, "z": 7}),
        json!({"id": "q-hhhhhhhhhh", "type": "comment", "text": "why?", "target": "h-aaaaaaaaaa", "z": 8}),
        json!({"id": "s-iiiiiiiiii", "type": "sticky", "text": "alone", "z": 9}),
    ])
}

#[test]
fn deleting_an_object_takes_its_connectors_mind_children_and_pinned_comments() {
    let b = sample();
    assert_eq!(cascade(&b, "h-aaaaaaaaaa"), ["h-aaaaaaaaaa", "c-cccccccccc", "q-hhhhhhhhhh"]);
    // Mind children, their children, and a connector on a grandchild.
    assert_eq!(cascade(&b, "m-dddddddddd"), ["m-dddddddddd", "m-eeeeeeeeee", "m-ffffffffff", "c-gggggggggg"]);
    assert_eq!(cascade(&b, "s-iiiiiiiiii"), ["s-iiiiiiiiii"]);
    // The plan deletes each key with the rev it read.
    let ids = cascade(&b, "h-aaaaaaaaaa");
    let named: HashSet<String> = HashSet::from(["h-aaaaaaaaaa".to_string()]);
    let ops: Vec<Op> = ids.iter().map(|id| Op::Del { id: id.clone() }).collect();
    let ws = plan(&b, &ops, Some(&named), &ctx()).expect("planned");
    assert_eq!(ws.len(), 3);
    assert!(ws.iter().all(|w| w.value.is_none() && w.if_rev == 1));
    let left = after(&b, &ws);
    assert!(left.object("h-aaaaaaaaaa").is_none() && left.object("c-cccccccccc").is_none() && left.object("q-hhhhhhhhhh").is_none());
    assert!(left.object("h-bbbbbbbbbb").is_some(), "the other end stays");
}

#[test]
fn a_locked_object_refuses_by_name_but_follows_its_parent() {
    let mut objs: Vec<Value> = sample().objects.values().map(|s| s.value.clone()).collect();
    for o in &mut objs {
        if o["id"] == "c-cccccccccc" || o["id"] == "h-bbbbbbbbbb" {
            o["locked"] = json!(true);
        }
    }
    let b = board(&objs);
    // Named: refused.
    let e = err(plan(&b, &[Op::Del { id: "h-bbbbbbbbbb".into() }], None, &ctx()));
    assert_eq!(e.code, "locked");
    let e = err(plan(&b, &[Op::Set { id: "h-bbbbbbbbbb".into(), f: obj(json!({"x": 5})) }], None, &ctx()));
    assert_eq!(e.code, "locked");
    // Unlock, votes and reactions still go through.
    plan(&b, &[Op::Set { id: "h-bbbbbbbbbb".into(), f: obj(json!({"locked": false})) }], None, &ctx()).expect("unlock");
    plan(&b, &[Op::Set { id: "h-bbbbbbbbbb".into(), f: obj(json!({"reactions": {"agent:pat": ["👍"]}})) }], None, &ctx()).expect("react");
    // Unlock then edit in one apply.
    plan(
        &b,
        &[Op::Set { id: "h-bbbbbbbbbb".into(), f: obj(json!({"locked": false})) }, Op::Set { id: "h-bbbbbbbbbb".into(), f: obj(json!({"x": 9})) }],
        None,
        &ctx(),
    )
    .expect("locks are tracked through the batch: unlock, then edit, in one apply");
    // A locked connector goes with the shape it hangs off.
    let named: HashSet<String> = HashSet::from(["h-aaaaaaaaaa".to_string()]);
    let ops: Vec<Op> = cascade(&b, "h-aaaaaaaaaa").into_iter().map(|id| Op::Del { id }).collect();
    plan(&b, &ops, Some(&named), &ctx()).expect("the cascade follows its parent");
}

#[test]
fn add_stamps_people_by_key_defaults_and_z() {
    let b = sample();
    let ws = plan(&b, &[Op::Add(obj(json!({"type": "sticky", "text": "hi", "x": "10"})))], None, &ctx()).expect("added");
    assert_eq!(ws.len(), 1);
    let v = ws[0].value.as_ref().expect("a value");
    assert_eq!(ws[0].if_rev, 0, "a new key");
    assert!(v["id"].as_str().expect("id").starts_with("s-") && v["id"].as_str().expect("id").len() == 12);
    assert_eq!(ws[0].key, obj_key("b1a2b3c", v["id"].as_str().expect("id")));
    assert_eq!(v["x"], 10, "a CLI string becomes a number");
    assert_eq!(v["color"], "yellow", "the default");
    assert_eq!(v["z"], 10.0, "above everything");
    for k in ["createdBy", "updatedBy"] {
        assert_eq!(v[k], "agent:pat", "{k}: a stable key, never a display name");
    }
    assert_eq!(v["batch"], "cli-0000000001");
    // An id already there is refused, never skipped.
    let e = err(plan(&b, &[Op::Add(obj(json!({"id": "s-iiiiiiiiii", "type": "sticky"})))], None, &ctx()));
    assert_eq!(e.code, "exists");
    let e = err(plan(&b, &[Op::Add(obj(json!({"type": "blob"})))], None, &ctx()));
    assert_eq!(e.code, "unknown_type");
}

#[test]
fn refusals_are_typed_and_nothing_is_cut() {
    let b = sample();
    let add = |f: Value| plan(&b, &[Op::Add(obj(f))], None, &ctx());
    let long = "x".repeat(5001);
    let e = err(add(json!({"type": "sticky", "text": long})));
    assert_eq!((e.code, e.field.as_deref(), e.max), ("too_long", Some("text"), Some(5000)));
    add(json!({"type": "sticky", "text": "x".repeat(5000)})).expect("at the limit");
    // Code points, as the widget counts (Array.from): 5000 emoji fit.
    add(json!({"type": "sticky", "text": "😀".repeat(5000)})).expect("5000 emoji fit");
    assert_eq!(err(add(json!({"type": "sticky", "text": "😀".repeat(5001)}))).code, "too_long");
    // A connector end keeps its last point beside the id; a bad anchor is refused.
    let ws = add(json!({"type": "connector", "from": {"id": "h-aaaaaaaaaa", "anchor": "top", "x": 10.04, "y": 20}, "to": {"x": 1, "y": 2}})).expect("ends");
    assert_eq!(ws[0].value.as_ref().expect("value")["from"], json!({"id": "h-aaaaaaaaaa", "anchor": "top", "x": 10, "y": 20}));
    assert_eq!(err(add(json!({"type": "connector", "from": {"id": "h-aaaaaaaaaa", "anchor": "middle"}}))).code, "bad_field");
    let tags: Vec<String> = (0..201).map(|i| format!("t{i}")).collect();
    let e = err(add(json!({"type": "sticky", "tags": tags})));
    assert_eq!((e.code, e.max), ("too_many", Some(200)));
    let rows: Vec<Vec<&str>> = (0..101).map(|_| vec![""]).collect();
    assert_eq!(err(add(json!({"type": "table", "rows": rows}))).code, "too_many");
    let wide: Vec<Vec<String>> = vec![(0..31).map(|_| String::new()).collect()];
    assert_eq!(err(add(json!({"type": "table", "rows": wide}))).code, "too_many");
    let checks: Vec<Value> = (0..51).map(|i| json!({"text": format!("c{i}")})).collect();
    assert_eq!(err(add(json!({"type": "card", "checks": checks}))).code, "too_many");
    let pts: Vec<[i32; 2]> = (0..5001).map(|i| [i, i]).collect();
    assert_eq!(err(add(json!({"type": "pen", "points": pts}))).code, "too_many");
    assert_eq!(err(add(json!({"type": "sticky", "color": "mauve"}))).code, "bad_field");
    assert_eq!(err(add(json!({"type": "sticky", "colour": "red"}))).code, "unknown_field");
    assert_eq!(err(add(json!({"type": "shape", "link": "ftp://x"}))).code, "bad_field");
    assert_eq!(err(add(json!({"type": "image", "crop": {"x": 0.5, "y": 0, "w": 0.6, "h": 1}}))).code, "bad_field", "outside the image");
    assert_eq!(err(add(json!({"type": "image", "crop": {"x": -0.1, "y": 0, "w": 0.5, "h": 1}}))).code, "bad_field", "never clamped");
    // Images: blob refs only; a data: URL is refused with the way in.
    let e = err(add(json!({"type": "image", "src": "data:image/png;base64,AAAA"})));
    assert!(e.code == "bad_field" && e.message.contains("k2 store blob put"), "{e}");
    let r = format!("k2blob:sha256-{}", "a".repeat(64));
    add(json!({"type": "image", "src": r, "crop": {"x": 0.1, "y": 0.1, "w": 0.5, "h": 0.5}})).expect("a blob ref, crop in fractions");
    // One bad op refuses the whole apply: nothing is planned.
    let e = err(plan(
        &b,
        &[Op::Add(obj(json!({"type": "sticky", "text": "fine"}))), Op::Set { id: "s-nothere000".into(), f: obj(json!({"x": 1})) }],
        None,
        &ctx(),
    ));
    assert_eq!(e.code, "no_object");
    assert_eq!(err(plan(&b, &[Op::Del { id: "s-nothere000".into() }], None, &ctx())).code, "no_object");
    // Over 60 KiB stored is refused.
    let big: Vec<String> = (0..30).map(|_| "y".repeat(1990)).collect();
    let rows: Vec<Vec<String>> = (0..2).map(|_| big.clone()).collect();
    assert_eq!(err(add(json!({"type": "table", "rows": rows}))).code, "value_too_large");
}

#[test]
fn item_ids_are_minted_and_kept_and_tables_keep_row_and_column_ids_in_step() {
    let b = sample();
    let ws = plan(
        &b,
        &[Op::Add(obj(json!({"id": "k-card000001", "type": "card", "checks": [{"text": "a"}, "b", {"id": "ck-mine", "text": "c", "done": true}]})))],
        None,
        &ctx(),
    )
    .expect("added");
    let v = ws[0].value.as_ref().expect("value");
    let ids: Vec<&str> = v["checks"].as_array().expect("checks").iter().map(|c| c["id"].as_str().expect("id")).collect();
    assert!(ids[0].starts_with("ck-") && ids[0].len() == 11 && ids[1].starts_with("ck-"));
    assert_eq!(ids[2], "ck-mine", "a given id stays");
    assert_eq!(v["checks"][1], json!({"id": ids[1], "text": "b", "done": false}));
    assert_eq!(err(plan(&b, &[Op::Add(obj(json!({"type": "card", "checks": [{"id": "x", "text": "a"}, {"id": "x", "text": "b"}]})))], None, &ctx())).code, "bad_field");
    // Tables.
    let ws = plan(&b, &[Op::Add(obj(json!({"id": "b-table00001", "type": "table"})))], None, &ctx()).expect("a default table");
    let t = ws[0].value.as_ref().expect("value");
    assert_eq!(t["rowIds"].as_array().expect("rowIds").len(), 3);
    assert_eq!(t["colIds"].as_array().expect("colIds").len(), 3);
    let b2 = after(&b, &ws);
    let ws = plan(&b2, &[Op::Set { id: "b-table00001".into(), f: obj(json!({"rows": [["a", "b", "c", "d"], ["e"]]})) }], None, &ctx()).expect("set");
    let t2 = ws[0].value.as_ref().expect("value");
    assert_eq!(t2["rowIds"][0], t["rowIds"][0], "row ids still there stay");
    assert_eq!(t2["rowIds"].as_array().expect("rows").len(), 2);
    assert_eq!(t2["colIds"].as_array().expect("cols").len(), 4);
    assert_eq!(t2["colIds"][2], t["colIds"][2]);
    // Replies get ids and keep their author key.
    let ws = plan(&b, &[Op::Set { id: "q-hhhhhhhhhh".into(), f: obj(json!({"replies": [{"by": "agent:pat", "at": "2026-11-01", "text": "because"}]})) }], None, &ctx()).expect("set");
    let r = &ws[0].value.as_ref().expect("value")["replies"][0];
    assert!(r["id"].as_str().expect("id").starts_with("rp-"));
    assert_eq!(r["by"], "agent:pat");
}

#[test]
fn older_shapes_read_in_todays_shapes() {
    let old = json!({
        "id": "k-old0000001", "type": "card",
        "reactions": {"👍": ["Pat", "agent:sam"], "🎉": ["Pat"]},
        "checks": [{"text": "a", "done": false}, {"text": "b", "done": true}],
    });
    let v = read_shape(&old);
    assert_eq!(v["reactions"], json!({"Pat": ["👍", "🎉"], "agent:sam": ["👍"]}), "per person");
    assert_eq!(v["checks"][0]["id"], "ck-0");
    assert_eq!(v["checks"][1]["id"], "ck-1");
    let t = read_shape(&json!({"id": "b-old", "type": "table", "rows": [["1", "2"], ["3", "4"], ["5"]]}));
    assert_eq!(t["rowIds"], json!(["rw-0", "rw-1", "rw-2"]));
    assert_eq!(t["colIds"], json!(["cl-0", "cl-1"]));
    let c = read_shape(&json!({"id": "q-old", "type": "comment", "replies": [{"by": "Pat", "text": "x"}]}));
    assert_eq!(c["replies"][0]["id"], "rp-0");
    // Today's shape is left alone.
    let now = json!({"id": "s-x", "type": "sticky", "reactions": {"person:owner": ["👍"]}});
    assert_eq!(read_shape(&now), now);
    // The old shape is read, never written.
    let b = sample();
    let e = err(plan(&b, &[Op::Set { id: "s-iiiiiiiiii".into(), f: obj(json!({"reactions": {"👍": ["Pat"]}})) }], None, &ctx()));
    assert_eq!(e.code, "bad_field");
}

#[test]
fn reactions_and_votes_are_per_person() {
    let mut b = sample();
    let o = b.object("s-iiiiiiiiii").expect("sticky").clone();
    let r = toggle_reaction(&o, "agent:pat", "👍").expect("on");
    assert_eq!(r, json!({"agent:pat": ["👍"]}));
    let mut o2 = o.clone();
    o2.insert("reactions".into(), r);
    assert_eq!(toggle_reaction(&o2, "agent:pat", "👍").expect("off"), json!({}), "toggling off leaves no key");
    assert_eq!(toggle_reaction(&o, "agent:pat", "🍕").expect_err("not a reaction").code, "bad_field");
    // Votes need voting on, and count per person.
    assert_eq!(add_vote(&b, "s-iiiiiiiiii", "agent:pat").expect_err("off").code, "voting_off");
    b.meta = Some(Stored { key: meta_key(&b.id), rev: 1, value: json!({"format": 1, "voting": {"active": true, "perPerson": 1}}) });
    let v = add_vote(&b, "s-iiiiiiiiii", "agent:pat").expect("a vote");
    assert_eq!(v, json!({"agent:pat": 1}));
    let ws = plan(&b, &[Op::Set { id: "s-iiiiiiiiii".into(), f: obj(json!({"votes": v})) }], None, &ctx()).expect("set");
    let b = after(&b, &ws);
    assert_eq!(add_vote(&b, "h-aaaaaaaaaa", "agent:pat").expect_err("used").code, "no_votes_left");
    add_vote(&b, "h-aaaaaaaaaa", "person:owner").expect("the person has their own");
}

#[test]
fn apply_maps_labels_to_fresh_ids_across_references() {
    let mut ops = parse_ops(&json!({"ops": [
        {"op": "add", "obj": {"id": "a", "type": "shape", "text": "A"}},
        {"op": "add", "obj": {"id": "b", "type": "shape", "text": "B"}},
        {"op": "add", "obj": {"id": "l", "type": "connector", "from": "a", "to": {"id": "b", "anchor": "left"}}},
        {"op": "add", "obj": {"id": "r", "type": "mind", "text": "root"}},
        {"op": "add", "obj": {"type": "mind", "text": "kid", "parent": "r"}},
        {"op": "add", "obj": {"type": "comment", "text": "?", "target": "a"}},
        {"op": "set", "id": "a", "f": {"fill": "blue"}},
        {"op": "meta", "f": {"title": "Plan"}},
    ]}))
    .expect("parses");
    let map = map_ids(&mut ops);
    assert_eq!(map.len(), 4);
    let a = map["a"].clone();
    assert!(a.starts_with("h-") && a.len() == 12);
    assert!(map["l"].starts_with("c-") && map["r"].starts_with("m-"));
    let b = sample();
    let ws = plan(&b, &ops, None, &ctx()).expect("planned");
    let by_id = |id: &str| ws.iter().find(|w| w.key == obj_key("b1a2b3c", id)).and_then(|w| w.value.clone()).expect("written");
    let l = by_id(&map["l"]);
    assert_eq!(l["from"], json!({"id": a, "anchor": "auto"}));
    assert_eq!(l["to"], json!({"id": map["b"], "anchor": "left"}));
    assert_eq!(by_id(&a)["fill"], "blue", "the set followed the label");
    assert!(ws.iter().any(|w| w.value.as_ref().is_some_and(|v| v["parent"] == json!(map["r"]))));
    assert!(ws.iter().any(|w| w.value.as_ref().is_some_and(|v| v["target"] == json!(a))));
    let m = ws.iter().find(|w| w.key == meta_key("b1a2b3c")).and_then(|w| w.value.clone()).expect("meta");
    assert_eq!((m["title"].clone(), m["format"].clone()), (json!("Plan"), json!(1)));
    assert_eq!(parse_ops(&json!({"ops": [{"op": "zap"}]})).expect_err("bad").code, "bad_ops");
    assert_eq!(parse_ops(&json!({"nope": 1})).expect_err("bad").code, "bad_ops");
}

#[test]
fn every_template_builds_clean_objects_with_fresh_ids() {
    let b = Board { id: "b1a2b3c".into(), ..Board::default() };
    for (name, _) in TEMPLATES {
        let objs = build(name, 100.0, 200.0).unwrap_or_else(|| panic!("{name}"));
        assert!(!objs.is_empty(), "{name}");
        let ids: HashSet<&str> = objs.iter().map(|o| o["id"].as_str().expect("id")).collect();
        assert_eq!(ids.len(), objs.len(), "{name}: unique ids");
        let ops: Vec<Op> = objs.iter().map(|o| Op::Add(o.as_object().expect("object").clone())).collect();
        let ws = plan(&b, &ops, None, &ctx()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(ws.len(), objs.len());
        // Connectors, mind parents: point at objects in the same template.
        for o in &objs {
            for k in ["from", "to"] {
                if let Some(id) = o.get(k).and_then(|e| e.get("id")).and_then(Value::as_str) {
                    assert!(ids.contains(id), "{name}: {k} → {id}");
                }
            }
            if let Some(p) = o.get("parent").and_then(Value::as_str) {
                assert!(ids.contains(p), "{name}: parent {p}");
            }
        }
    }
    assert!(build("nope", 0.0, 0.0).is_none());
    let names: Vec<&str> = TEMPLATES.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["kickoff", "kanban", "retro", "flowchart", "storymap", "brainstorm", "mindmap"]);
}

#[test]
fn pen_points_set_the_box() {
    let b = sample();
    let ws = plan(&b, &[Op::Add(obj(json!({"type": "pen", "x": 100, "y": 100, "points": [[10, 20], [30, 60], [20, 25]]})))], None, &ctx()).expect("pen");
    let v = ws[0].value.as_ref().expect("value");
    assert_eq!(v["points"], json!([[0, 0], [20, 40], [10, 5]]));
    assert_eq!((v["x"].clone(), v["y"].clone(), v["w"].clone(), v["h"].clone()), (json!(110), json!(120), json!(20), json!(40)));
}

#[test]
fn the_export_has_people_tokens_blob_refs_and_no_k2_identifiers() {
    let hex = "c".repeat(64);
    let png = [137u8, 80, 78, 71, 13, 10, 26, 10];
    let data = format!("data:image/png;base64,{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png));
    let mut b = board(&[
        json!({"id": "s-two", "type": "sticky", "z": 2, "createdBy": "person:owner", "updatedBy": "agent:pat", "batch": "w-1",
               "reactions": {"person:owner": ["👍"], "agent:pat": ["🎉"]}, "votes": {"agent:pat": 2}}),
        json!({"id": "s-one", "type": "sticky", "z": 1, "createdBy": "agent:pat", "updatedBy": "agent:pat"}),
        json!({"id": "i-new", "type": "image", "z": 3, "src": format!("k2blob:sha256-{hex}"), "createdBy": "Pat"}),
        json!({"id": "i-old", "type": "image", "z": 3, "src": data}),
        json!({"id": "q-c", "type": "comment", "z": 4, "replies": [{"id": "rp-1", "by": "agent:sam", "at": "x", "text": "hi"}]}),
    ]);
    b.meta = Some(Stored { key: meta_key(&b.id), rev: 1, value: json!({"format": 1, "title": "Plan", "batch": "w-2", "timer": {"state": "off"}}) });
    let names = |k: &str| -> (String, &'static str) {
        match k {
            "person:owner" => ("Robin".into(), "person"),
            k if k.starts_with("agent:") => (k[6..].to_string(), "agent"),
            other => (other.to_string(), "person"),
        }
    };
    let mut asked = Vec::new();
    let env = envelope(&b, "Plan", &names, &mut |h: &str| {
        asked.push(h.to_string());
        Ok(BlobBytes { mime: "image/png".into(), name: Some("shot.png".into()), bytes: png.to_vec() })
    })
    .expect("exported");
    assert_eq!(asked, [hex.clone()]);
    assert_eq!((env["format"].clone(), env["version"].clone(), env["title"].clone()), (json!("k2-board"), json!(1), json!("Plan")));
    assert_eq!(env["meta"], json!({"timer": {"state": "off"}}), "no format, title or batch");
    let order: Vec<&str> = env["objects"].as_array().expect("objects").iter().map(|o| o["id"].as_str().expect("id")).collect();
    assert_eq!(order, ["s-one", "s-two", "i-new", "i-old", "q-c"], "by z, then id");
    // Tokens in order of first appearance.
    assert_eq!(env["people"], json!({
        "p1": {"name": "pat", "kind": "agent"},
        "p2": {"name": "Robin", "kind": "person"},
        "p3": {"name": "Pat", "kind": "person"},
        "p4": {"name": "sam", "kind": "agent"},
    }));
    let two = &env["objects"][1];
    assert_eq!((two["createdBy"].clone(), two["updatedBy"].clone()), (json!("p2"), json!("p1")));
    assert_eq!(two["reactions"], json!({"p1": ["🎉"], "p2": ["👍"]}));
    assert_eq!(two["votes"], json!({"p1": 2}));
    assert!(two.get("batch").is_none());
    assert_eq!(env["objects"][2]["src"], json!({"blob": format!("sha256-{hex}")}));
    let old = &env["objects"][3]["src"]["blob"];
    let hash: String = <sha2::Sha256 as sha2::Digest>::digest(png).iter().map(|x| format!("{x:02x}")).collect();
    assert_eq!(old, &json!(format!("sha256-{hash}")), "a data: image becomes a blob too");
    assert_eq!(env["blobs"][format!("sha256-{hex}")]["mime"], "image/png");
    assert_eq!(env["objects"][4]["replies"][0]["by"], "p4");
    let text = env.to_string();
    for banned in ["k2blob:", "person:owner", "agent:", "b1a2b3c", "data:image", "\"batch\""] {
        assert!(!text.contains(banned), "the export carries {banned}");
    }
}

#[test]
fn view_shortens_data_images_and_fields_describes_every_type() {
    let v = view(&json!({"id": "i-x", "type": "image", "src": format!("data:image/png;base64,{}", "A".repeat(4096)), "batch": "w"}), false);
    assert_eq!(v["src"], "<data: 4 KB>");
    assert!(v.get("batch").is_none());
    let d = schema::describe(None).expect("all");
    assert_eq!(d.as_object().expect("types").len(), schema::TYPES.len());
    assert_eq!(d["table"]["fields"]["rowIds"].as_str().map(|s| s.starts_with("ids")), Some(true));
    assert_eq!(schema::describe(Some("nope")).expect_err("unknown").code, "unknown_type");
}


// ---------- stable ids and the format (the widget's test uses the same fixtures) ----------

const OLDER: &str = include_str!("../../../../tests/board/fixtures/older-board.json");
const UPGRADED: &str = include_str!("../../../../tests/board/fixtures/older-board.upgraded.json");

fn seed(s: &str) -> Vec<Stored> {
    let v: Value = serde_json::from_str(s).expect("fixture JSON");
    v["seed"].as_array().expect("seed").iter().map(|e| Stored { key: e[0].as_str().expect("key").to_string(), rev: 1, value: e[1].clone() }).collect()
}

fn without_batch(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("batch");
    }
    v
}

const OLD_BOARD: &str = "b0f0f0f";

/// The fixture's keys under the board, and its list row.
fn older() -> (Vec<Stored>, Stored) {
    let all = seed(OLDER);
    let row = all.iter().find(|e| e.key == "boards/b0f0f0f").expect("row").clone();
    (all.into_iter().filter(|e| e.key.starts_with("board/b0f0f0f/")).collect(), row)
}

#[test]
fn ids_are_lowercase_letter_and_hex_inside_the_file_rule() {
    for ty in schema::type_names() {
        let id = schema::new_id(ty);
        let p = schema::prefix(ty).expect("a letter");
        assert!(p.is_ascii_lowercase(), "{ty}: {p}");
        assert_eq!(id.len(), 12, "{id}");
        assert!(id.starts_with(&format!("{p}-")) && id[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{id}");
        assert!(schema::valid_item_id(&id), "{id}");
    }
    let letters: HashSet<char> = schema::type_names().iter().map(|t| schema::prefix(t).expect("a letter")).collect();
    assert_eq!(letters.len(), schema::type_names().len(), "one letter per type");
    for p in ["ck", "rp", "rw", "cl"] {
        let id = schema::new_item_id(p);
        assert_eq!(id.len(), 11, "{id}");
        assert!(schema::valid_item_id(&id), "{id}");
    }
}

#[test]
fn an_older_list_item_never_takes_an_id_another_item_holds() {
    let v = read_shape(&json!({"id": "k-1", "type": "card", "checks": [{"id": "ck-1", "text": "a"}, {"text": "b"}]}));
    assert_eq!(v["checks"][0]["id"], "ck-1");
    assert_eq!(v["checks"][1]["id"], "ck-1x", "the widget's slotId gives the same");
    let u = schema::upgrade_object(&json!({"id": "q-1", "type": "comment", "replies": [{"id": "rp-1", "by": "a", "text": "x"}, {"by": "b", "text": "y"}]}), None);
    assert_eq!(u["replies"][1]["id"], "rp-1x");
}

#[test]
fn an_older_board_upgrades_to_the_pinned_shape_with_nothing_lost_and_then_plans_nothing() {
    let (entries, row) = older();
    assert_eq!(entries.iter().find(|e| e.key.ends_with("/meta")).expect("meta").value.get("format"), None, "really the older shape");
    assert_eq!(row.value.get("format"), None);
    let ws = upgrade_plan(OLD_BOARD, &entries, Some(&row), "cli-test");
    // Format last, so a stop part way runs it all again.
    let n = ws.len();
    assert_eq!(n, 6, "4 objects that change, then the meta and the row: {:?}", ws.iter().map(|w| &w.key).collect::<Vec<_>>());
    assert!(ws[n - 2].key.ends_with("/meta") && ws[n - 1].key == "boards/b0f0f0f");
    assert!(ws.iter().all(|w| w.if_rev == 1 && w.value.is_some()));

    // Laid over the older keys: exactly the pinned shape, the same one the widget's test checks.
    let mut after: BTreeMap<String, Value> = entries.iter().chain([&row]).map(|e| (e.key.clone(), e.value.clone())).collect();
    for w in &ws {
        after.insert(w.key.clone(), w.value.clone().expect("a put"));
    }
    let want = seed(UPGRADED);
    for e in want.iter().filter(|e| after.contains_key(&e.key)) {
        assert_eq!(without_batch(&after[&e.key]), without_batch(&e.value), "{}", e.key);
    }
    assert_eq!(want.iter().filter(|e| after.contains_key(&e.key)).count(), after.len(), "nothing else appeared");

    // Nothing lost: every older field is there unchanged, but for the reactions turned round.
    for e in entries.iter().chain([&row]) {
        for (k, v) in e.value.as_object().expect("an object") {
            if k == "batch" {
                continue;
            }
            let now = &after[&e.key][k];
            match k.as_str() {
                "reactions" => assert_eq!(now, &json!({"Ada": ["👍", "🎉"], "agent:helper": ["👍"]}), "{}", e.key),
                "checks" | "replies" => {
                    for (a, b) in v.as_array().expect("a list").iter().zip(now.as_array().expect("a list")) {
                        for (f, x) in a.as_object().expect("an item") {
                            assert_eq!(&b[f], x, "{} {k}.{f}", e.key);
                        }
                    }
                    assert_eq!(v.as_array().map(Vec::len), now.as_array().map(Vec::len));
                }
                _ => assert_eq!(now, v, "{} {k}", e.key),
            }
        }
    }
    assert_eq!(after["board/b0f0f0f/obj/s-1a2b3c4d5e"]["keptField"], json!({"from": "a newer tool"}));
    assert_eq!(after["board/b0f0f0f/obj/k-4d5e6f7081"]["updatedBy"], "Ada", "an upgrade is nobody's edit");

    // Twice: nothing to do.
    let again: Vec<Stored> = after.iter().filter(|(k, _)| k.starts_with("board/")).map(|(k, v)| Stored { key: k.clone(), rev: 2, value: v.clone() }).collect();
    let row2 = Stored { key: row.key.clone(), rev: 2, value: after[&row.key].clone() };
    assert!(upgrade_plan(OLD_BOARD, &again, Some(&row2), "cli-test").is_empty());
    for e in again.iter().filter(|e| e.key.contains("/obj/")) {
        assert_eq!(schema::upgrade_object(&e.value, None), e.value, "{}", e.key);
    }
}

#[test]
fn an_upgrade_leaves_what_it_cant_read_as_stored() {
    // Not the older reactions shape, or a list item with an id it wouldn't pick: kept.
    let odd = json!({"id": "s-1", "type": "sticky", "reactions": {"👍": "not a list", "other": ["x"]}, "checks": [{"id": 7, "text": "a"}], "extra": [1]});
    assert_eq!(schema::upgrade_object(&odd, None), odd);
    // A key that can't be an id names nothing.
    let no_id = json!({"type": "sticky", "text": "x"});
    assert_eq!(schema::upgrade_object(&no_id, Some("Not An Id")), no_id);
    assert_eq!(schema::upgrade_object(&no_id, Some("s-abc"))["id"], "s-abc");
    // A value that wouldn't fit stays as it is; the format still goes on.
    let big = json!({"id": "k-big", "type": "card", "checks": [{"text": "x".repeat(schema::MAX_VALUE - 60)}]});
    let entries = vec![
        Stored { key: obj_key(OLD_BOARD, "k-big"), rev: 1, value: big },
        Stored { key: crate::board::meta_key(OLD_BOARD), rev: 1, value: json!({"title": "T"}) },
    ];
    let ws = upgrade_plan(OLD_BOARD, &entries, None, "cli-a-long-batch-id-that-tips-it-over");
    assert_eq!(ws.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(), vec!["board/b0f0f0f/meta"]);
}

#[test]
fn an_object_known_only_by_its_key_is_read_and_a_set_stores_its_ids() {
    let entries = vec![
        Stored { key: obj_key("b1a2b3c", "k-old0000001"), rev: 1, value: json!({"type": "card", "title": "T", "checks": [{"text": "a", "done": false}]}) },
        Stored { key: "board/b1a2b3c/obj/Not An Id".into(), rev: 1, value: json!({"type": "card"}) },
    ];
    let b = Board::from_entries("b1a2b3c", entries);
    assert_eq!(b.objects.keys().map(String::as_str).collect::<Vec<_>>(), vec!["k-old0000001"]);
    let ws = plan(&b, &[Op::Set { id: "k-old0000001".into(), f: obj(json!({"title": "U"})) }], None, &ctx()).expect("set");
    let v = ws[0].value.as_ref().expect("a put");
    assert_eq!(v["id"], "k-old0000001");
    assert_eq!(v["checks"][0]["id"], "ck-0", "the id its readers gave it, kept");
    // A table's readers' ids stay too: only a new row mints one.
    let t = board(&[json!({"id": "b-old0000001", "type": "table", "rows": [["a"], ["b"]]})]);
    let ws = plan(&t, &[Op::Set { id: "b-old0000001".into(), f: obj(json!({"rows": [["a"], ["b"], ["c"]]})) }], None, &ctx()).expect("set");
    let ids = ws[0].value.as_ref().expect("a put")["rowIds"].as_array().expect("ids").clone();
    assert_eq!((ids[0].as_str(), ids[1].as_str()), (Some("rw-0"), Some("rw-1")));
    assert!(ids[2].as_str().expect("an id").starts_with("rw-") && ids[2] != "rw-2", "{ids:?}");
}

#[test]
fn a_newer_format_is_refused_for_writes_and_the_export_and_never_stamped_down() {
    let meta = |f: Value| Stored { key: crate::board::meta_key("b1a2b3c"), rev: 1, value: json!({"format": f, "title": "T"}) };
    let o = Stored { key: obj_key("b1a2b3c", "s-iiiiiiiiii"), rev: 1, value: json!({"id": "s-iiiiiiiiii", "type": "sticky", "text": "later", "shiny": 1}) };
    for f in [json!(2), json!(99), json!("two"), json!(1.5), json!(0)] {
        let b = Board::from_entries("b1a2b3c", vec![meta(f.clone()), o.clone()]);
        for op in [
            Op::Set { id: "s-iiiiiiiiii".into(), f: obj(json!({"text": "over it"})) },
            Op::Del { id: "s-iiiiiiiiii".into() },
            Op::Add(obj(json!({"type": "sticky"}))),
            Op::Meta(obj(json!({"title": "Mine now"}))),
        ] {
            let e = err(plan(&b, &[op], None, &ctx()));
            assert_eq!(e.code, "newer_format", "{f}");
            assert!(e.message.contains("read only") && e.message.contains("Update K2"), "{}", e.message);
        }
        let env = envelope(&b, "T", &|k| (k.to_string(), "person"), &mut |_| panic!("no blobs"));
        assert_eq!(env.expect_err("refused").code, "newer_format", "{f}");
        assert!(upgrade_plan("b1a2b3c", &[meta(f.clone())], None, "x").is_empty(), "it has a format: never stamped");
    }
    for ok in [None, Some(&Value::Null), Some(&json!(1))] {
        check_format(ok).expect("readable");
    }
    // Format 1 stays as it was: a meta write keeps it.
    let b = Board::from_entries("b1a2b3c", vec![meta(json!(1))]);
    let ws = plan(&b, &[Op::Meta(obj(json!({"title": "U"})))], None, &ctx()).expect("meta");
    assert_eq!(ws[0].value.as_ref().expect("a put")["format"], 1);
}

#[test]
fn numbers_finer_than_the_schema_keeps_are_refused_never_rounded() {
    let add = |f: Value| {
        let mut o = obj(f);
        o.insert("type".into(), json!(o.get("type").cloned().unwrap_or(json!("sticky"))));
        plan(&Board { id: "b1a2b3c".into(), ..Board::default() }, &[Op::Add(o)], None, &ctx())
    };
    for f in [
        json!({"x": 12.34}),
        json!({"y": "1.25"}),
        json!({"w": 0.05}),
        json!({"z": 1.0001}),
        json!({"type": "table", "colW": [100.25]}),
        json!({"type": "connector", "from": {"x": 1.25, "y": 2}, "to": {"x": 0, "y": 0}}),
        json!({"type": "connector", "from": {"id": "s-1", "x": 1.25, "y": 2}, "to": {"x": 0, "y": 0}}),
        json!({"type": "pen", "points": [[0, 0], [1.5, 2]]}),
        json!({"type": "image", "crop": {"x": 0.12345, "y": 0, "w": 0.5, "h": 0.5}}),
    ] {
        let e = err(add(f.clone()));
        assert_eq!(e.code, "bad_field", "{f}: {}", e.message);
        assert!(e.message.contains("nothing was rounded"), "{f}: {}", e.message);
    }
    // What it keeps goes in exactly, float noise from arithmetic included.
    let ws = add(json!({"x": 12.3, "y": -0.5, "w": 12.399999999999, "h": 7})).expect("kept");
    let v = ws[0].value.as_ref().expect("a put");
    assert_eq!((v["x"].as_f64(), v["y"].as_f64(), v["w"].as_f64(), v["h"].as_f64()), (Some(12.3), Some(-0.5), Some(12.4), Some(7.0)));
    let ws = add(json!({"type": "image", "crop": {"x": 0.1234, "y": 0, "w": 0.5, "h": 0.5}})).expect("four places");
    assert_eq!(ws[0].value.as_ref().expect("a put")["crop"]["x"].as_f64(), Some(0.1234));
}
