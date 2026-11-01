//! `k2 board` through the Garden data door (`zen_data_routes::handle`, so
//! every gate runs): the Board core switch, grants per board, the delete
//! cascade, templates, `apply`, typed refusals, people by stable key, the
//! log and the export.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::zen_data_routes::{handle, DataWho};

struct Env {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
    _m: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Env {
    fn drop(&mut self) {
        crate::docs::cores::tests::all_off();
    }
}

/// Temp HOME, a scoped DB, agents `pat` (w-pat) and `sam` (w-sam), and the
/// Board core on.
fn env() -> Env {
    let m = crate::docs::cores::tests::mirror_lock();
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (id, handle) in [("w-pat", "pat"), ("w-sam", "sam")] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                rusqlite::params![id, handle, format!("/tmp/board-{handle}")],
            )
            .expect("agent row");
        }
    }
    crate::docs::cores::set_for_test("board", true);
    Env { _home: home, _db: db, _m: m }
}

fn agent(id: &str) -> DataWho {
    DataWho::Agent { project_id: id.into() }
}

fn call(who: &DataWho, body: Value) -> (u16, Value) {
    let r = handle(crate::board_routes::ROUTE, who.clone(), body.to_string().as_bytes(), &HashMap::new(), "");
    let code: u16 = r.status.split(' ').next().expect("status").parse().expect("code");
    (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn ok(who: &DataWho, body: Value) -> Value {
    let (code, v) = call(who, body.clone());
    assert_eq!(code, 200, "{body}: {v}");
    v
}

fn refused(who: &DataWho, body: Value) -> (u16, String) {
    let (code, v) = call(who, body.clone());
    assert_ne!(code, 200, "{body} should refuse: {v}");
    (code, v["error"]["code"].as_str().unwrap_or("").to_string())
}

fn grant(agent_id: &str, level: &str, prefix: &str) {
    let r = handle(
        "/cli/zen/data/grant",
        DataWho::Owner,
        json!({ "widget": "k2:board", "agent": agent_id, "level": level, "prefix": prefix }).to_string().as_bytes(),
        &HashMap::new(),
        "",
    );
    assert!(r.status.starts_with("200"), "grant: {}", r.body);
}

fn objects(who: &DataWho, board: &str) -> Vec<Value> {
    ok(who, json!({ "cmd": "list", "board": board }))["objects"].as_array().expect("objects").clone()
}

#[test]
fn the_board_core_closes_and_reopens_the_data_without_losing_it() {
    let _e = env();
    let o = DataWho::Owner;
    let b = ok(&o, json!({ "cmd": "board-new", "name": "Plan" }))["board"].as_str().expect("id").to_string();
    ok(&o, json!({ "cmd": "add", "board": b, "type": "sticky", "fields": { "text": "kept" } }));
    crate::docs::cores::set_for_test("board", false);
    for body in [
        json!({ "cmd": "boards" }),
        json!({ "cmd": "list", "board": b }),
        json!({ "cmd": "add", "board": b, "type": "sticky" }),
        json!({ "cmd": "board-new", "name": "Another" }),
    ] {
        assert_eq!(refused(&o, body.clone()), (403, "core_off".into()), "{body}");
        assert_eq!(refused(&agent("w-pat"), body.clone()).1, "core_off", "{body}");
    }
    // Raw store calls on the Board's data too, from the agent door.
    let r = handle("/cli/zen/data/list", o.clone(), b"", &HashMap::from([("widget".to_string(), "k2:board".to_string())]), "");
    assert!(r.status.starts_with("403") && r.body.contains("core_off"), "{}", r.body);
    // The static help still answers.
    ok(&o, json!({ "cmd": "fields", "type": "sticky" }));
    // Granting still works while it's off (Settings shows the line).
    grant("w-pat", "read", &format!("board/{b}/"));
    crate::docs::cores::set_for_test("board", true);
    let objs = objects(&o, &b);
    assert_eq!(objs.len(), 1);
    assert_eq!(objs[0]["text"], "kept", "turning the core off deleted nothing");
}

#[test]
fn an_agent_reaches_only_the_boards_it_was_granted() {
    let _e = env();
    let o = DataWho::Owner;
    let pat = agent("w-pat");
    let mine = ok(&o, json!({ "cmd": "board-new", "name": "Mine" }))["board"].as_str().expect("id").to_string();
    let other = ok(&o, json!({ "cmd": "board-new", "name": "Other" }))["board"].as_str().expect("id").to_string();
    assert_eq!(refused(&pat, json!({ "cmd": "list", "board": mine })).1, "not_granted", "no grant, no board");
    grant("w-pat", "write", &format!("board/{mine}/"));
    // The only board it can see is the default; it can't list the others.
    let boards = ok(&pat, json!({ "cmd": "boards" }))["boards"].as_array().expect("boards").clone();
    assert_eq!(boards.iter().map(|b| b["id"].as_str().expect("id")).collect::<Vec<_>>(), [mine.as_str()]);
    let added = ok(&pat, json!({ "cmd": "add", "type": "sticky", "fields": { "text": "hi", "x": "40" } }));
    assert_eq!(added["board"], mine.as_str());
    let s = &added["object"];
    assert_eq!((s["createdBy"].clone(), s["updatedBy"].clone()), (json!("agent:pat"), json!("agent:pat")), "a stable key, never a name");
    assert_eq!(s["x"], 40);
    assert_eq!(refused(&pat, json!({ "cmd": "list", "board": other })).1, "no_board");
    assert_eq!(refused(&pat, json!({ "cmd": "add", "board": other, "type": "sticky" })).1, "no_board");
    assert_eq!(refused(&pat, json!({ "cmd": "board-new", "name": "Mine too" })).1, "not_granted", "boards/ is the person's");
    // A read grant reads but never writes.
    grant("w-sam", "read", &format!("board/{mine}/"));
    let sam = agent("w-sam");
    assert_eq!(objects(&sam, &mine).len(), 1);
    assert_eq!(refused(&sam, json!({ "cmd": "add", "board": mine, "type": "sticky" })).1, "not_granted");
}

#[test]
fn delete_takes_connectors_mind_children_and_pinned_comments() {
    let _e = env();
    let o = DataWho::Owner;
    let b = ok(&o, json!({ "cmd": "board-new", "name": "Flow" }))["board"].as_str().expect("id").to_string();
    let r = ok(&o, json!({ "cmd": "apply", "board": b, "ops": [
        { "op": "add", "obj": { "id": "a", "type": "shape", "text": "A" } },
        { "op": "add", "obj": { "id": "z", "type": "shape", "text": "Z", "x": 400 } },
        { "op": "add", "obj": { "id": "l", "type": "connector", "from": "a", "to": "z" } },
        { "op": "add", "obj": { "id": "q", "type": "comment", "text": "why", "target": "a" } },
        { "op": "add", "obj": { "id": "root", "type": "mind", "text": "root" } },
        { "op": "add", "obj": { "id": "kid", "type": "mind", "text": "kid", "parent": "root" } },
    ] }));
    let ids = r["ids"].as_object().expect("label → id").clone();
    assert_eq!(ids.len(), 6);
    let a = ids["a"].as_str().expect("a").to_string();
    assert!(a.starts_with("h-"), "labels become fresh board ids");
    let d = ok(&o, json!({ "cmd": "delete", "board": b, "id": a }));
    let gone: Vec<&str> = d["deleted"].as_array().expect("deleted").iter().map(|x| x.as_str().expect("id")).collect();
    assert_eq!(gone, [a.as_str(), ids["l"].as_str().expect("l"), ids["q"].as_str().expect("q")]);
    let left: Vec<String> = objects(&o, &b).iter().map(|x| x["id"].as_str().expect("id").to_string()).collect();
    assert_eq!(left.len(), 3, "{left:?}");
    assert!(left.contains(&ids["z"].as_str().expect("z").to_string()));
    let d = ok(&o, json!({ "cmd": "delete", "board": b, "id": ids["root"] }));
    assert_eq!(d["deleted"].as_array().expect("deleted").len(), 2, "the mind child goes with its parent");
    assert_eq!(refused(&o, json!({ "cmd": "delete", "board": b, "id": a })), (404, "no_object".into()));
}

#[test]
fn refusals_are_typed_and_write_nothing() {
    let _e = env();
    let o = DataWho::Owner;
    let b = ok(&o, json!({ "cmd": "board-new", "name": "Limits" }))["board"].as_str().expect("id").to_string();
    let (code, v) = call(&o, json!({ "cmd": "add", "board": b, "type": "sticky", "fields": { "text": "x".repeat(5001) } }));
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("too_long")));
    assert_eq!(refused(&o, json!({ "cmd": "add", "board": b, "type": "sticky", "fields": { "colour": "red" } })).1, "unknown_field");
    assert_eq!(refused(&o, json!({ "cmd": "add", "board": b, "type": "nope" })).1, "unknown_type");
    assert_eq!(
        refused(&o, json!({ "cmd": "add", "board": b, "type": "image", "fields": { "src": "data:image/png;base64,AAAA" } })).1,
        "bad_field",
        "images are blob refs"
    );
    // One bad op in an apply: nothing lands.
    assert_eq!(
        refused(&o, json!({ "cmd": "apply", "board": b, "ops": [
            { "op": "add", "obj": { "type": "sticky", "text": "fine" } },
            { "op": "add", "obj": { "type": "sticky", "text": "x".repeat(6000) } },
        ] }))
        .1,
        "too_long"
    );
    assert!(objects(&o, &b).is_empty(), "all or nothing");
    // Locked.
    let id = ok(&o, json!({ "cmd": "add", "board": b, "type": "sticky", "fields": { "locked": true } }))["object"]["id"].as_str().expect("id").to_string();
    assert_eq!(refused(&o, json!({ "cmd": "update", "board": b, "id": id, "fields": { "text": "no" } })), (409, "locked".into()));
    assert_eq!(refused(&o, json!({ "cmd": "delete", "board": b, "id": id })), (409, "locked".into()));
    ok(&o, json!({ "cmd": "react", "board": b, "id": id, "emoji": "👍" }));
    assert_eq!(refused(&o, json!({ "cmd": "vote", "board": b, "id": id })), (409, "voting_off".into()));
    assert_eq!(refused(&o, json!({ "cmd": "template", "board": b, "name": "nope" })), (404, "no_template".into()));
    assert_eq!(refused(&o, json!({ "cmd": "zap" })).1, "bad_request");
}

#[test]
fn templates_people_log_and_export() {
    let _e = env();
    let o = DataWho::Owner;
    let b = ok(&o, json!({ "cmd": "board-new", "name": "Retro" }))["board"].as_str().expect("id").to_string();
    grant("w-pat", "write", &format!("board/{b}/"));
    let pat = agent("w-pat");
    let t = ok(&pat, json!({ "cmd": "template", "board": b, "name": "retro", "x": 100, "y": 50 }));
    let n = t["ids"].as_array().expect("ids").len();
    assert!(n > 10, "{t}");
    assert_eq!(objects(&o, &b).len(), n);
    let first = t["ids"][0].as_str().expect("id").to_string();
    let r = ok(&pat, json!({ "cmd": "react", "board": b, "id": first, "emoji": "🎉" }));
    assert_eq!(r["object"]["reactions"], json!({ "agent:pat": ["🎉"] }));
    ok(&o, json!({ "cmd": "react", "board": b, "id": first, "emoji": "🎉" }));
    ok(&o, json!({ "cmd": "meta", "board": b, "fields": { "voting": { "active": true, "perPerson": 2 } } }));
    let v = ok(&pat, json!({ "cmd": "vote", "board": b, "id": first }));
    assert_eq!(v["object"]["votes"], json!({ "agent:pat": 1 }));
    let c = ok(&pat, json!({ "cmd": "comment", "board": b, "id": first, "text": "let's keep this" }));
    assert_eq!(c["object"]["target"], first.as_str());
    let hits = ok(&o, json!({ "cmd": "search", "board": b, "text": "KEEP THIS" }))["objects"].as_array().expect("hits").len();
    assert_eq!(hits, 1);
    // The log: one row per command, newest last, with who.
    let log = ok(&o, json!({ "cmd": "log", "board": b, "limit": 3 }))["log"].as_array().expect("log").clone();
    assert_eq!(log.len(), 3);
    assert_eq!(log[2]["by"], "agent:pat", "the comment, last");
    // The export: people tokens, no K2 identifiers.
    let env = ok(&pat, json!({ "cmd": "export", "board": b }))["export"].clone();
    assert_eq!((env["format"].clone(), env["version"].clone(), env["title"].clone()), (json!("k2-board"), json!(1), json!("Retro")));
    assert_eq!(env["objects"].as_array().expect("objects").len(), n + 1);
    let kinds: Vec<&str> = env["people"].as_object().expect("people").values().map(|p| p["kind"].as_str().expect("kind")).collect();
    assert!(kinds.contains(&"agent") && kinds.contains(&"person"), "{}", env["people"]);
    let text = env.to_string();
    for banned in ["agent:pat", "person:owner", "k2blob:", b.as_str(), "\"batch\""] {
        assert!(!text.contains(banned), "the export carries {banned}");
    }
    // `fields` and `templates` answer without a board.
    let f = ok(&pat, json!({ "cmd": "fields" }));
    assert!(f["types"]["connector"]["fields"]["from"].is_string());
    assert_eq!(ok(&pat, json!({ "cmd": "templates" }))["templates"].as_array().expect("templates").len(), 7);
}

#[test]
fn the_board_route_is_post_only_and_agents_reach_it_on_the_cell_socket() {
    assert!(crate::zen_data_routes::is_post_route(crate::board_routes::ROUTE));
    assert!(crate::zen_data_routes::is_agent_route(crate::board_routes::ROUTE));
    let row = crate::routes::route_policy::lookup(crate::board_routes::ROUTE).expect("a policy row");
    assert!(row.post.is_some() && row.get.is_none(), "POST only: a GET is 405 before the handler");
}

/// `store.grantees` (catalog v43): the agents with a grant on the Board's
/// data and what each reaches, for the Board's chat list.
#[test]
fn grantees_lists_each_agent_once_with_its_reach() {
    let _e = env();
    grant("w-pat", "write", "board/b1a2b3c/");
    grant("w-pat", "read", "board/b9f9f9f/");
    grant("w-sam", "read", "");
    let space = k2_core::garden_data::Space::data("k2:board").expect("space");
    let v = crate::garden_data::call::grantees(&space, "g-none").expect("grantees");
    assert_eq!(
        v,
        json!({ "items": [
            { "handle": "pat", "level": "write", "prefixes": ["board/b1a2b3c/", "board/b9f9f9f/"] },
            { "handle": "sam", "level": "read", "prefixes": null },
        ] })
    );
}
