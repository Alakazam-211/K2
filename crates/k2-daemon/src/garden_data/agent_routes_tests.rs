//! Agent-door tests (contract §4.5, §5, §12.1): through
//! `zen_data_routes::handle`, so the gates run too. Temp HOME + a scoped
//! k2so.db per test; the Garden data store lives under that HOME.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::zen_data_routes::{handle, DataWho};

pub(crate) struct Env {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
}

/// Temp HOME, a scoped DB, and two agents (`w-builder` / `w-keeper`) plus a
/// third (`w-stranger`), all with handles.
pub(crate) fn env() -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (id, handle) in [("w-builder", "board-builder"), ("w-keeper", "board-keeper"), ("w-stranger", "stranger")] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                rusqlite::params![id, handle, format!("/tmp/gd-{handle}")],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO project_groups (id, name, created_at, updated_at) VALUES ('pg-q4', 'Q4 launch', 1, 1)",
            [],
        )
        .unwrap();
    }
    Env { _home: home, _db: db }
}

pub(crate) fn agent(id: &str) -> DataWho {
    DataWho::Agent { project_id: id.into() }
}

/// (status code, body) for one call.
pub(crate) fn call(who: &DataWho, method: &str, path: &str, body: Value, params: &[(&str, &str)]) -> (u16, Value) {
    let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let raw = if method == "POST" { body.to_string().into_bytes() } else { Vec::new() };
    let r = handle(path, who.clone(), &raw, &p, "");
    let code: u16 = r.status.split(' ').next().unwrap().parse().unwrap();
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body));
    (code, v)
}

pub(crate) fn ok(who: &DataWho, method: &str, path: &str, body: Value, params: &[(&str, &str)]) -> Value {
    let (code, v) = call(who, method, path, body, params);
    assert_eq!(code, 200, "{path}: {v}");
    v
}

pub(crate) fn err(who: &DataWho, method: &str, path: &str, body: Value, params: &[(&str, &str)]) -> (u16, String) {
    let (code, v) = call(who, method, path, body, params);
    assert_ne!(code, 200, "{path} should refuse: {v}");
    (code, v["error"]["code"].as_str().unwrap_or("").to_string())
}

pub(crate) fn person_grant(widget: &str, agent_id: &str, level: &str, prefix: &str) {
    ok(
        &DataWho::Owner,
        "POST",
        "/cli/zen/data/grant",
        json!({ "widget": widget, "agent": agent_id, "level": level, "prefix": prefix }),
        &[],
    );
}

#[test]
fn owner_reads_and_writes_everything_and_by_is_wire_shaped() {
    let _e = env();
    let o = DataWho::Owner;
    let v = ok(&o, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "obj/1", "value": {"t": "hi"}, "ifRev": 0}), &[]);
    assert_eq!(v["rev"], 1);
    let v = ok(&o, "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", "obj/1")]);
    assert_eq!(v["entry"]["value"], json!({"t": "hi"}));
    assert_eq!(v["entry"]["by"], "you");
    let (code, c) = err(&o, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "obj/1", "value": 2, "ifRev": 0}), &[]);
    assert_eq!((code, c.as_str()), (409, "conflict"));
    let v = ok(&o, "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", "nope")]);
    assert_eq!(v["entry"], Value::Null);
    // A missing space reads empty, never 404.
    let v = ok(&o, "GET", "/cli/zen/data/list", json!({}), &[("widget", "ghost")]);
    assert_eq!(v["items"], json!([]));
    let v = ok(&o, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "ghost")]);
    assert_eq!(v["head"], 0);
    // Body shape is strict.
    let (code, c) = err(&o, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "k", "value": 1, "extra": 1}), &[]);
    assert_eq!((code, c.as_str()), (400, "bad_request"));
}

#[test]
fn agents_see_and_change_only_granted_prefixes() {
    let _e = env();
    let keeper = agent("w-keeper");
    // No grant: nothing.
    let (code, c) = err(&keeper, "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", "obj/1")]);
    assert_eq!((code, c.as_str()), (403, "not_granted"));
    let (_, c) = err(&keeper, "GET", "/cli/zen/data/list", json!({}), &[("widget", "board")]);
    assert_eq!(c, "not_granted");
    let o = DataWho::Owner;
    ok(&o, "POST", "/cli/zen/data/batch", json!({"widget": "board", "ops": [
        {"op": "put", "key": "project/7/obj/1", "value": 1},
        {"op": "put", "key": "project/8/obj/1", "value": 2},
        {"op": "put", "key": "shared/readme", "value": 3}
    ]}), &[]);
    person_grant("board", "w-keeper", "write", "project/7/");
    person_grant("board", "w-keeper", "read", "shared/");
    let v = ok(&keeper, "GET", "/cli/zen/data/list", json!({}), &[("widget", "board")]);
    let keys: Vec<&str> = v["items"].as_array().unwrap().iter().map(|e| e["key"].as_str().unwrap()).collect();
    assert_eq!(keys, vec!["project/7/obj/1", "shared/readme"], "never project/8");
    let v = ok(&keeper, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "project/7/obj/2", "value": {"x": 1}}), &[]);
    assert_eq!(v["rev"], 2);
    let v = ok(&o, "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", "project/7/obj/2")]);
    assert_eq!(v["entry"]["by"], "agent:board-keeper");
    let (_, c) = err(&keeper, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "shared/readme", "value": 9}), &[]);
    assert_eq!(c, "not_granted", "read-only prefix");
    let (_, c) = err(&keeper, "POST", "/cli/zen/data/delete", json!({"widget": "board", "key": "project/8/obj/1"}), &[]);
    assert_eq!(c, "not_granted");
    let (code, c) = err(&keeper, "POST", "/cli/zen/data/batch", json!({"widget": "board", "ops": [
        {"op": "put", "key": "project/7/obj/3", "value": 1},
        {"op": "delete", "key": "project/8/obj/1"}
    ]}), &[]);
    assert_eq!((code, c.as_str()), (403, "not_granted"));
    let v = ok(&o, "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", "project/7/obj/3")]);
    assert_eq!(v["entry"], Value::Null, "a refused batch writes nothing");
    let v = ok(&keeper, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board"), ("since", "0")]);
    assert!(v["items"].as_array().unwrap().iter().all(|c| !c["key"].as_str().unwrap().starts_with("project/8/")));
    assert_eq!(v["head"], 2);
    // Other widgets' spaces are separate.
    let (_, c) = err(&keeper, "GET", "/cli/zen/data/list", json!({}), &[("widget", "diary")]);
    assert_eq!(c, "not_granted");
}

#[test]
fn project_grants_follow_live_membership() {
    let _e = env();
    let o = DataWho::Owner;
    ok(&o, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "project/7/a", "value": 1}), &[]);
    let v = ok(&o, "POST", "/cli/zen/data/grant", json!({"widget": "board", "project": "Q4 launch", "level": "read", "prefix": "project/7/"}), &[]);
    assert_eq!(v["warning"], "project_grant_convenience");
    let s = agent("w-stranger");
    let (_, c) = err(&s, "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", "project/7/a")]);
    assert_eq!(c, "not_granted");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO project_group_members (group_id, workspace_id, created_at) VALUES ('pg-q4', 'w-stranger', 1)", [])
            .unwrap();
    }
    let v = ok(&s, "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", "project/7/a")]);
    assert_eq!(v["entry"]["value"], 1);
    let v = ok(&s, "GET", "/cli/zen/data/grants", json!({}), &[("widget", "board")]);
    assert_eq!(v["grants"][0]["from"], json!({"kind": "project", "id": "pg-q4", "name": "Q4 launch"}));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("DELETE FROM project_group_members WHERE workspace_id = 'w-stranger'", []).unwrap();
    }
    let (_, c) = err(&s, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board")]);
    assert_eq!(c, "not_granted", "left the project: refused on the next call");
}

#[test]
fn batch_limits_and_changes_cursor() {
    let _e = env();
    let o = DataWho::Owner;
    let ops: Vec<Value> = (0..65).map(|i| json!({"op": "put", "key": format!("k{i}"), "value": i})).collect();
    let (code, c) = err(&o, "POST", "/cli/zen/data/batch", json!({"widget": "board", "ops": ops}), &[]);
    assert_eq!((code, c.as_str()), (413, "too_large"));
    let (_, c) = err(&o, "POST", "/cli/zen/data/batch", json!({"widget": "board", "ops": []}), &[]);
    assert_eq!(c, "too_large");
    let ops: Vec<Value> = (0..3).map(|i| json!({"op": "put", "key": format!("k{i}"), "value": i})).collect();
    let v = ok(&o, "POST", "/cli/zen/data/batch", json!({"widget": "board", "ops": ops}), &[]);
    assert_eq!((v["rev"].as_i64(), v["ops"].as_i64()), (Some(1), Some(3)));
    let v = ok(&o, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board"), ("since", "1"), ("after", "")]);
    assert_eq!(v["items"], json!([]));
    let v = ok(&o, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board"), ("since", "0")]);
    assert_eq!(v["items"].as_array().unwrap().len(), 3);
    assert_eq!(v["partial"], false);
    let (_, c) = err(&o, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board"), ("wait", "31")]);
    assert_eq!(c, "bad_request");
    let (_, c) = err(&o, "GET", "/cli/zen/data/list", json!({}), &[("widget", "board"), ("limit", "501")]);
    assert_eq!(c, "bad_request");
}

#[test]
fn changes_wait_answers_on_a_write_and_times_out_quietly() {
    let _e = env();
    let o = DataWho::Owner;
    ok(&o, "POST", "/cli/zen/data/put", json!({"widget": "board-wait", "key": "a", "value": 1}), &[]);
    ok(&o, "POST", "/cli/zen/data/put", json!({"widget": "board-wait", "key": "b", "value": 2}), &[]);
    person_grant("board-wait", "w-keeper", "read", "");
    let keeper = agent("w-keeper");
    // Something already after the cursor: answered at once.
    let v = ok(&keeper, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board-wait"), ("since", "1"), ("wait", "5")]);
    assert_eq!(v["items"][0]["key"], "b");
    // Nothing after rev 2 for a second: an empty answer after the wait.
    let started = std::time::Instant::now();
    let v = ok(&keeper, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board-wait"), ("since", "2"), ("wait", "1")]);
    assert_eq!(v["items"], json!([]));
    assert!(started.elapsed() >= std::time::Duration::from_millis(900), "it waited");
    // A write during the wait wakes it early with the new row.
    let writer = std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let r = handle(
            "/cli/zen/data/put",
            DataWho::Owner,
            json!({"widget": "board-wait", "key": "c", "value": 3}).to_string().as_bytes(),
            &HashMap::new(),
            "",
        );
        assert!(r.status.starts_with("200"), "{}", r.body);
    });
    let started = std::time::Instant::now();
    let v = ok(&keeper, "GET", "/cli/zen/data/changes", json!({}), &[("widget", "board-wait"), ("since", "2"), ("wait", "10")]);
    writer.join().unwrap();
    assert_eq!(v["items"][0]["key"], "c");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "woken by the write");
}

#[test]
fn shell_tabs_follow_agent_rules_and_person_routes_refuse_passports() {
    let _e = env();
    let shell = DataWho::Shell { project_id: "w-keeper".into() };
    let (_, c) = err(&shell, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "a", "value": 1}), &[]);
    assert_eq!(c, "not_granted");
    let (code, c) = err(&agent("w-keeper"), "POST", "/cli/zen/data/admin", json!({"widget": "board", "agent": "w-keeper", "level": "write"}), &[]);
    assert_eq!((code, c.as_str()), (403, "person_only"));
}

#[test]
fn agent_writes_are_rate_limited() {
    let _e = env();
    person_grant("board", "w-builder", "write", "");
    let b = agent("w-builder");
    let mut limited = false;
    for i in 0..200 {
        let (code, v) = call(&b, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": format!("k{i}"), "value": i}), &[]);
        if code == 429 {
            assert_eq!(v["error"]["code"], "rate_limited");
            assert!(v["error"]["retryAfterSecs"].as_u64().unwrap() >= 1);
            limited = true;
            break;
        }
    }
    assert!(limited, "a burst over 120 writes is refused");
}
