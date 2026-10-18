//! Widget-door and transaction tests (contract §7.1, §4.5, §12.2): through
//! `zen_data_routes::handle`, so the gates run too. Temp HOME (a Zen
//! folder with one Garden placing `board`), a scoped k2so.db, and the
//! agent-door helpers from `agent_routes_tests`.

use std::collections::HashMap;

use serde_json::{json, Value};

use k2_core::garden_data::authors::{self, CallerKind, StampSource};
use k2_core::garden_data::MadeBy;
use k2_core::zen::{ZenFile, ZenFiles};

use crate::garden_data::agent_routes::tests::{agent, env, err, ok, person_grant, Env};
use crate::sidecar_routes::{AgentCaller, Caller};
use crate::zen_data_routes::{handle, DataWho};

const APP: &str = "POST /cli/zen/call HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: tauri://localhost\r\n\r\n";

/// The Garden page: one custom placement `b1` of widget `board`.
const PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n";

struct Door {
    _env: Env,
    f: ZenFiles,
    garden: String,
}

fn manifest(caps: &[&str]) -> String {
    let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
    let sources = if caps.iter().any(|c| c == "\"sources:read\"") { ",\n  \"sources\": \"config\"" } else { "" };
    format!(
        "{{\n  \"schema\": 1,\n  \"name\": \"Board\",\n  \"entry\": \"index.html\",\n  \"caps\": [{}]{sources}\n}}\n",
        caps.join(", ")
    )
}

/// A Zen folder whose first Garden places `board` (with `caps`).
fn door(caps: &[&str]) -> Door {
    let e = env();
    let f = ZenFiles::local();
    f.setup().expect("zen setup");
    f.new_widget("board", Some("hello")).expect("widget folder");
    set_caps(&f, caps);
    let garden = f.gardens()[0].id.clone();
    std::fs::write(f.path_of(&ZenFile::Garden(garden.clone())), PAGE).expect("garden page");
    Door { _env: e, f, garden }
}

fn set_caps(f: &ZenFiles, caps: &[&str]) {
    std::fs::write(f.widget_dir("board").join("manifest.json"), manifest(caps)).expect("manifest");
}

impl Door {
    fn hash(&self) -> String {
        let h = self.f.garden_widget_status(&self.garden, "board").hash().to_string();
        assert_eq!(h.len(), 64, "board bundles clean");
        h
    }

    fn stamp(&self, maker: MadeBy) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let kind = if maker == MadeBy::Owner { CallerKind::Owner } else { CallerKind::Agent };
        authors::stamp(&conn, "board", &self.hash(), &maker, StampSource::Sign, kind, 1).expect("stamp");
    }

    fn raw(&self, hash: &str, verb: &str, args: Value) -> (u16, Value) {
        let body = json!({ "garden": self.garden, "placement": "b1", "hash": hash, "verb": verb, "args": args });
        let r = handle("/cli/zen/call", DataWho::Owner, body.to_string().as_bytes(), &HashMap::new(), APP);
        let code: u16 = r.status.split(' ').next().unwrap().parse().unwrap();
        (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{verb}: {e}: {}", r.body)))
    }

    fn call(&self, verb: &str, args: Value) -> Value {
        let (code, v) = self.raw(&self.hash(), verb, args);
        assert_eq!(code, 200, "{verb}: {v}");
        assert_eq!(v["ok"], true, "{v}");
        v["result"].clone()
    }

    fn refuse(&self, verb: &str, args: Value) -> String {
        let (code, v) = self.raw(&self.hash(), verb, args);
        assert_ne!(code, 200, "{verb} should refuse: {v}");
        v["error"]["code"].as_str().unwrap_or("").to_string()
    }
}

fn builder() -> MadeBy {
    MadeBy::Agent { project_id: "w-builder".into() }
}

// ── widget/sign, widget/claim, widget/new (contract §4.3, BG3) ──────

fn zen(path: &str, owner: bool, caller: &Caller, body: Value) -> (u16, Value) {
    let r = crate::zen_routes::handle(path, owner, caller, "local", &HashMap::new(), body.to_string().as_bytes());
    let code: u16 = r.status.split(' ').next().unwrap().parse().unwrap();
    (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

fn passport(project: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: project.into(), map_key: None, args: Vec::new(), sandboxed: false })
}

#[test]
fn the_first_signer_of_a_hash_wins_and_only_the_person_claims() {
    let d = door(&["store:read", "store:write"]);
    let (code, v) = zen("/cli/zen/widget/sign", false, &passport("w-builder"), json!({"widget": "board"}));
    assert_eq!(code, 200, "{v}");
    assert_eq!((v["stamped"].as_bool(), v["madeBy"]["kind"].as_str()), (Some(true), Some("agent")));
    assert_eq!(v["madeBy"]["handle"], "board-builder");
    assert_eq!(v["hash"], d.hash());
    // The person re-signing the same code doesn't move rights; nor does a shell tab.
    let (_, v) = zen("/cli/zen/widget/sign", true, &Caller::Owner, json!({"widget": "board"}));
    assert_eq!((v["stamped"].as_bool(), v["madeBy"]["kind"].as_str()), (Some(false), Some("agent")));
    let shell = Caller::Shell { project_id: "w-keeper".into() };
    let (_, v) = zen("/cli/zen/widget/sign", false, &shell, json!({"widget": "board"}));
    assert_eq!(v["stamped"], false);
    // Claim: an agent or a K2 terminal can't; the person can.
    let (code, v) = zen("/cli/zen/widget/claim", false, &passport("w-builder"), json!({"widget": "board"}));
    assert_eq!((code, v["error"].as_str()), (403, Some("owner_only")));
    let (code, v) = zen("/cli/zen/widget/claim", false, &shell, json!({"widget": "board"}));
    assert_eq!((code, v["error"].as_str()), (403, Some("owner_only")));
    let (code, v) = zen("/cli/zen/widget/claim", true, &Caller::Owner, json!({"widget": "board"}));
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["madeBy"], json!({"kind": "owner"}));
    // An edit is a new hash: a K2 shell tab signs it as its workspace (agent-level).
    std::fs::write(d.f.widget_dir("board").join("index.html"), "<!doctype html><p>edited</p>").expect("edit");
    let (_, v) = zen("/cli/zen/widget/sign", false, &shell, json!({"widget": "board"}));
    assert_eq!((v["stamped"].as_bool(), v["madeBy"]["handle"].as_str()), (Some(true), Some("board-keeper")), "{v}");
    // A sandboxed agent or a Connect login never signs.
    let boxed = Caller::Agent(AgentCaller { project_id: "w-builder".into(), map_key: None, args: Vec::new(), sandboxed: true });
    let (code, _) = zen("/cli/zen/widget/sign", false, &boxed, json!({"widget": "board"}));
    assert_eq!(code, 403);
    let (code, _) = zen("/cli/zen/widget/sign", false, &Caller::Login { username: "x".into() }, json!({"widget": "board"}));
    assert_eq!(code, 403);
}

#[test]
fn widget_rows_carry_their_maker() {
    let _d = door(&["store:read", "store:write"]);
    let made = || -> Value {
        let (code, v) = zen("/cli/zen/widgets", true, &Caller::Owner, Value::Null);
        assert_eq!(code, 200, "{v}");
        let rows = v["widgets"].as_array().expect("rows").clone();
        rows.into_iter().find(|r| r["name"] == "board").expect("board row")["madeBy"].clone()
    };
    assert_eq!(made(), json!({"kind": "unclaimed"}), "nobody signed it yet");
    zen("/cli/zen/widget/sign", false, &passport("w-builder"), json!({"widget": "board"}));
    assert_eq!(made()["handle"], "board-builder");
    zen("/cli/zen/widget/claim", true, &Caller::Owner, json!({"widget": "board"}));
    assert_eq!(made(), json!({"kind": "owner"}));
}

#[test]
fn sign_needs_a_clean_folder_and_new_stamps_its_maker() {
    let d = door(&["store:read", "store:write"]);
    std::fs::write(d.f.widget_dir("board").join("manifest.json"), "{ not json").expect("break");
    let (code, v) = zen("/cli/zen/widget/sign", true, &Caller::Owner, json!({"widget": "board"}));
    assert_eq!((code, v["error"].as_str()), (409, Some("widget_broken")), "{v}");
    let (code, v) = zen("/cli/zen/widget/sign", true, &Caller::Owner, json!({"widget": "nope"}));
    assert_eq!(code, 404, "{v}");
    let (code, v) = zen("/cli/zen/widget/new", false, &passport("w-builder"), json!({"name": "notes"}));
    assert_eq!(code, 200, "an agent may make a widget folder: {v}");
    assert_eq!((v["stamped"].as_bool(), v["madeBy"]["handle"].as_str()), (Some(true), Some("board-builder")), "{v}");
    let (_, v) = zen("/cli/zen/widget/sign", true, &Caller::Owner, json!({"widget": "k2:diary@2"}));
    assert_eq!(v["madeBy"], json!({"kind": "k2"}));
}

#[test]
fn an_owner_made_widget_reads_and_writes_its_whole_keyspace() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    let r = d.call("store.put", json!(["obj/1", {"t": "hi"}, {"ifRev": 0}]));
    let rev = r["rev"].as_i64().expect("rev");
    let e = d.call("store.get", json!(["obj/1"]));
    assert_eq!(e["value"], json!({"t": "hi"}));
    assert_eq!(e["by"], "you via board", "a widget write is the person via the widget");
    assert_eq!(d.refuse("store.put", json!(["obj/1", 2, {"ifRev": rev + 5}])), "conflict");
    let page = d.call("store.list", json!([{"prefix": "obj/"}]));
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1));
    // An agent sees the widget's write through the agent door (owner-made: the
    // person granted nothing, so the agent sees nothing).
    let (_, code) = err(&agent("w-keeper"), "GET", "/cli/zen/data/get", Value::Null, &[("widget", "board"), ("key", "obj/1")]);
    assert_eq!(code, "not_granted");
    let del = d.call("store.delete", json!(["obj/1"]));
    assert_eq!(del["deleted"], true);
    assert_eq!(d.call("store.delete", json!(["obj/1"])), json!({"rev": 0, "deleted": false}));
    assert_eq!(d.call("viewer.get", json!([])), json!({"kind": "owner", "name": "You"}));
}

#[test]
fn an_unsigned_widget_gets_author_unknown_but_keeps_its_saves() {
    let d = door(&["store:read", "store:write"]);
    assert_eq!(d.refuse("store.list", json!([])), "author_unknown");
    assert_eq!(d.refuse("store.put", json!(["k", 1])), "author_unknown");
    assert_eq!(d.call("save.set", json!(["high-score", 42])), Value::Null);
    assert_eq!(d.call("save.get", json!(["high-score"])), json!(42));
    assert_eq!(d.call("save.keys", json!([])), json!(["high-score"]));
    assert_eq!(d.call("save.remove", json!(["high-score"])), Value::Null);
    assert_eq!(d.call("save.get", json!(["high-score"])), Value::Null);
}

#[test]
fn an_agent_made_widget_never_reaches_past_its_makers_grants() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(builder());
    assert_eq!(d.refuse("store.put", json!(["project/7/a", 1])), "not_granted", "a maker with no grant");
    person_grant("board", "w-builder", "write", "project/7/");
    d.call("store.put", json!(["project/7/a", 1]));
    assert_eq!(d.refuse("store.put", json!(["project/8/a", 1])), "not_granted");
    assert_eq!(d.refuse("store.get", json!(["project/8/a"])), "not_granted");
    // The person writes outside the grant; the widget's list never shows it.
    ok(&DataWho::Owner, "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "secret/x", "value": 1}), &[]);
    let page = d.call("store.list", json!([]));
    let keys: Vec<&str> = page["items"].as_array().expect("items").iter().filter_map(|i| i["key"].as_str()).collect();
    assert_eq!(keys, vec!["project/7/a"]);
    let ch = d.call("store.changes", json!([{"since": 0}]));
    let keys: Vec<&str> = ch["items"].as_array().expect("items").iter().filter_map(|i| i["key"].as_str()).collect();
    assert_eq!(keys, vec!["project/7/a"], "changes filtered too");
    // A batch with one key outside the grant writes nothing.
    let before = d.call("store.changes", json!([{"since": 0}]))["head"].clone();
    let ops = json!([{"op": "put", "key": "project/7/b", "value": 1}, {"op": "put", "key": "other/c", "value": 1}]);
    assert_eq!(d.refuse("store.batch", json!([ops])), "not_granted");
    assert_eq!(d.call("store.changes", json!([{"since": 0}]))["head"], before);
    // The person's "It's mine" lifts it to the whole keyspace.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        authors::claim(&conn, "board", &d.hash(), 2).expect("claim");
    }
    d.call("store.put", json!(["other/c", 1]));
}

#[test]
fn a_stale_hash_and_a_missing_cap_are_refused() {
    let d = door(&["store:read"]);
    d.stamp(MadeBy::Owner);
    let (code, v) = d.raw(&"0".repeat(64), "store.get", json!(["k"]));
    assert_eq!((code, v["error"]["code"].as_str()), (409, Some("stale_widget")));
    assert_eq!(d.refuse("store.put", json!(["k", 1])), "cap_not_granted");
    assert_eq!(d.refuse("store.txBegin", json!([])), "cap_not_granted");
    assert_eq!(d.refuse("thread.post", json!(["a", "b"])), "not_exposed");
    assert_eq!(d.refuse("nope.nope", json!([])), "unknown_verb");
    // 0.46.1 (Rosson 2026-10-17): a widget that reads data sources keeps
    // store:write when its manifest asks for it...
    set_caps(&d.f, &["store:read", "store:write", "sources:read"]);
    d.stamp(MadeBy::Owner);
    assert_eq!(d.call("store.get", json!(["k"])), Value::Null);
    d.call("store.put", json!(["k", 1]));
    assert_eq!(d.call("store.get", json!(["k"]))["value"], json!(1));
    d.call("save.set", json!(["k", 1]));
    // ...and is refused when it didn't ask for it.
    set_caps(&d.f, &["store:read", "sources:read"]);
    d.stamp(MadeBy::Owner);
    assert_eq!(d.refuse("store.put", json!(["k", 2])), "cap_not_granted");
    assert_eq!(d.call("store.get", json!(["k"]))["value"], json!(1), "the refused write changed nothing");
}

#[test]
fn saves_are_private_and_never_pushed() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    let gen = crate::garden_data::generation("data:board");
    d.call("save.set", json!(["mine", {"x": 1}]));
    assert_eq!(crate::garden_data::generation("data:board"), gen, "a save is never pushed");
    let v = ok(&DataWho::Owner, "GET", "/cli/zen/data/get", Value::Null, &[("widget", "board"), ("key", "mine")]);
    assert_eq!(v["entry"], Value::Null, "the data door never sees saves: {v}");
    d.call("store.put", json!(["shared", 1]));
    assert!(crate::garden_data::generation("data:board") > gen, "a data write is pushed");
}

#[test]
fn a_paused_placement_refuses_writes_and_keeps_reads() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    crate::zen_routes::pause_for_writes(&d.garden, "b1");
    assert_eq!(d.refuse("store.put", json!(["k", 1])), "paused");
    assert_eq!(d.refuse("save.set", json!(["k", 1])), "paused");
    assert_eq!(d.call("store.get", json!(["k"])), Value::Null);
}

#[test]
fn a_widget_transaction_is_invisible_until_commit_and_one_rev() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    let tx = d.call("store.txBegin", json!([]));
    let id = tx["txId"].as_str().expect("txId").to_string();
    for part in 0..16 {
        let n = if part == 15 { 40 } else { 64 };
        let ops: Vec<Value> =
            (0..n).map(|i| json!({"op": "put", "key": format!("obj/{:04}", part * 64 + i), "value": {"i": i}})).collect();
        d.call("store.txAdd", json!([id, ops]));
    }
    let page = d.call("store.list", json!([{"limit": 10}]));
    assert_eq!(page["items"], json!([]), "nothing staged is readable");
    let r = d.call("store.txCommit", json!([id]));
    assert_eq!(r["ops"], 1000);
    let ch = d.call("store.changes", json!([{"since": 0}]));
    let revs: std::collections::BTreeSet<i64> =
        ch["items"].as_array().expect("items").iter().filter_map(|i| i["rev"].as_i64()).collect();
    assert_eq!(revs.len(), 1, "one rev for the whole transaction");
    assert_eq!(d.refuse("store.txCommit", json!([id])), "bad_tx", "a finished id is gone");
    // Abort leaves nothing.
    let id2 = d.call("store.txBegin", json!([]))["txId"].as_str().expect("txId").to_string();
    d.call("store.txAdd", json!([id2, [{"op": "put", "key": "z/1", "value": 1}]]));
    assert_eq!(d.call("store.txAbort", json!([id2])), json!({"aborted": true}));
    assert_eq!(d.call("store.get", json!(["z/1"])), Value::Null);
}

#[test]
fn a_revoke_between_add_and_commit_fails_the_commit_and_writes_nothing() {
    let _e = env();
    person_grant("board", "w-keeper", "write", "project/7/");
    let keeper = agent("w-keeper");
    let tx = ok(&keeper, "POST", "/cli/zen/data/tx/begin", json!({"widget": "board"}), &[]);
    let id = tx["txId"].as_str().expect("txId").to_string();
    ok(&keeper, "POST", "/cli/zen/data/tx/add", json!({"txId": id, "ops": [{"op": "put", "key": "project/7/a", "value": 1}]}), &[]);
    ok(
        &DataWho::Owner,
        "POST",
        "/cli/zen/data/revoke",
        json!({"widget": "board", "agent": "w-keeper", "prefix": "project/7/"}),
        &[],
    );
    let (_, code) = err(&keeper, "POST", "/cli/zen/data/tx/commit", json!({"txId": id}), &[]);
    assert_eq!(code, "not_granted");
    let v = ok(&DataWho::Owner, "GET", "/cli/zen/data/get", Value::Null, &[("widget", "board"), ("key", "project/7/a")]);
    assert_eq!(v["entry"], Value::Null, "nothing written: {v}");
}

#[test]
fn agent_transactions_are_bound_to_their_caller_and_checked_per_part() {
    let _e = env();
    person_grant("board", "w-keeper", "write", "project/7/");
    let keeper = agent("w-keeper");
    let (_, code) = err(&agent("w-stranger"), "POST", "/cli/zen/data/tx/begin", json!({"widget": "board"}), &[]);
    assert_eq!(code, "not_granted", "no write grant anywhere in the space");
    let id = ok(&keeper, "POST", "/cli/zen/data/tx/begin", json!({"widget": "board"}), &[])["txId"]
        .as_str()
        .expect("txId")
        .to_string();
    let part = json!({"txId": id, "ops": [{"op": "put", "key": "project/7/a", "value": 1}]});
    let (_, code) = err(&agent("w-builder"), "POST", "/cli/zen/data/tx/add", part.clone(), &[]);
    assert_eq!(code, "bad_tx", "another agent's id");
    let st = ok(&keeper, "POST", "/cli/zen/data/tx/add", part, &[]);
    assert_eq!(st["ops"], 1);
    let outside = json!({"txId": id, "ops": [{"op": "put", "key": "project/8/a", "value": 1}]});
    let (_, code) = err(&keeper, "POST", "/cli/zen/data/tx/add", outside, &[]);
    assert_eq!(code, "not_granted");
    let (_, code) = err(&keeper, "POST", "/cli/zen/data/tx/commit", json!({"txId": id}), &[]);
    assert_eq!(code, "bad_tx", "a failing part dropped the transaction");
    let v = ok(&keeper, "POST", "/cli/zen/data/tx/abort", json!({"txId": "0".repeat(32)}), &[]);
    assert_eq!(v["aborted"], false);
    // The person's transactions: full access, one rev.
    let o = DataWho::Owner;
    let id = ok(&o, "POST", "/cli/zen/data/tx/begin", json!({"widget": "board"}), &[])["txId"].as_str().expect("id").to_string();
    ok(&o, "POST", "/cli/zen/data/tx/add", json!({"txId": id, "ops": [{"op": "put", "key": "a", "value": 1}, {"op": "put", "key": "b", "value": 2}]}), &[]);
    let r = ok(&o, "POST", "/cli/zen/data/tx/commit", json!({"txId": id}), &[]);
    assert_eq!(r["ops"], 2);
}

/// Room-scope 0.46.1 through the widget door: `widget.self` (no cap; its
/// id, live hash, maker and 7-day saves), `room.members` behind the
/// manifest's `room:members`, and `app.*` for a local App (the owner's view).
#[test]
fn room_scope_verbs_through_the_widget_door() {
    use k2_core::garden_data::backend::StoreBackend;
    use k2_core::garden_data::{Actor, Space};
    let d = door(&["store:read"]);
    d.stamp(builder());
    let me = d.call("widget.self", json!([]));
    assert_eq!((me["widget"].as_str(), me["placement"].as_str(), me["garden"].as_str()), (Some("board"), Some("b1"), Some(d.garden.as_str())));
    assert_eq!(me["hash"], d.hash());
    assert_eq!(me["author"], json!({ "kind": "agent", "handle": "board-builder" }));
    assert_eq!(me["uses7d"], json!({ "writes": 0, "changes": 0 }));
    let s = crate::garden_data::store();
    let space = Space::data("board").unwrap();
    s.put(&space, "a", &json!(1), None, &Actor::Widget { widget: "board".into() }).unwrap();
    s.put(&space, "b", &json!(2), None, &Actor::Agent { project_id: "w-keeper".into() }).unwrap();
    assert_eq!(d.call("widget.self", json!([]))["uses7d"], json!({ "writes": 1, "changes": 2 }), "its own saves and everyone's");
    // room.members needs the cap in the manifest.
    assert_eq!(d.refuse("room.members", json!([{ "workspace": "board-keeper" }])), "cap_not_granted");
    set_caps(&d.f, &["store:read", "room:members"]);
    let v = d.call("room.members", json!([{ "workspace": "board-keeper" }]));
    assert_eq!((v["workspace"].as_str(), v["count"].as_i64(), v["admins"].as_str()), (Some("board-keeper"), Some(0), Some("admins of this box")));
    assert_eq!(d.refuse("room.members", json!([{ "workspace": "nowhere" }])), "not_bound");
    // app.* need no cap; an unknown App is not_found.
    assert_eq!(d.refuse("app.self", json!([{ "app": "ghost" }])), "not_found");
    assert_eq!(d.refuse("app.usage", json!([{ "app": "ghost" }])), "not_found");
    // A stale frame is refused before any of them.
    let (code, v) = d.raw(&"0".repeat(64), "widget.self", json!([]));
    assert_eq!((code, v["error"]["code"].as_str()), (409, Some("stale_widget")));
}
