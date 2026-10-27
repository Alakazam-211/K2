//! 0.46.5 S0 (Rosson's addition): K2's `zen.sign` card end to end. A
//! manifest change is a new version; an agent that can't sign it for every
//! placing Garden is refused (403 `sign_needs_person`) and K2 puts the card
//! on its Thread with the change in plain words; the person's Allow signs
//! exactly that version (a later edit needs a new card); "Turn off again"
//! takes the signature off. A listed agent still vouches itself, and a
//! widget the upgrade carried with a changed manifest offers the same card.
//! Temp HOME, this thread's DB, temp Zen folder; no real agent wakes. Fail
//! loudly.

use std::collections::HashMap;

use k2_core::db::schema::WorkspaceSession;
use k2_core::garden_data::authors::{self, CallerKind, StampSource};
use k2_core::garden_data::{signer_gate, MadeBy};
use k2_core::overlay;
use k2_core::zen::garden_access::{self as ga, Actor, Change, Via};
use k2_core::zen::{ZenFile, ZenFiles};
use serde_json::{json, Value};

use super::*;
use crate::permission::actions::{self, Answerer, Applied, Check, Ctx};
use crate::sidecar_routes::{AgentCaller, Caller};

struct Switch;

impl Drop for Switch {
    fn drop(&mut self) {
        ga::set_switch(false);
    }
}

struct Env {
    _switch: Switch,
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    f: ZenFiles,
    garden: String,
}

const PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n";

/// Zen set up, `board` (the hello starter) placed in Garden 1, the person's
/// signature on its first version, Garden access on.
fn env() -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    ga::set_switch(true);
    let f = ZenFiles::local();
    f.setup().expect("zen setup");
    f.new_widget("board", Some("hello")).expect("widget");
    let garden = f.gardens()[0].id.clone();
    std::fs::write(f.path_of(&ZenFile::Garden(garden.clone())), PAGE).expect("page");
    crate::zen_routes::refresh_and_emit().expect("refresh");
    let e = Env { _switch: Switch, _db: db, _home: home, f, garden };
    let hash = live(&e);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        authors::vouch(&conn, "board", &hash, &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 1).expect("person signs");
    }
    e
}

struct Ws {
    id: String,
    handle: String,
    conv: String,
}

fn seed(tag: &str) -> Ws {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let id = uuid::Uuid::new_v4().to_string();
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, format!("/tmp/zen-sign/{handle}")],
    )
    .expect("seed project");
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

fn agent(ws: &Ws) -> Caller {
    Caller::Agent(AgentCaller { project_id: ws.id.clone(), map_key: None, args: vec![], sandboxed: false })
}

fn live(e: &Env) -> String {
    let h = e.f.widget_status("board").hash().to_string();
    assert_eq!(h.len(), 64, "board bundles clean");
    h
}

fn passes(e: &Env, hash: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    signer_gate::passes(&conn, &e.garden, "board", hash).expect("gate")
}

fn maker(hash: &str) -> MadeBy {
    let db = k2_core::db::shared();
    let conn = db.lock();
    authors::maker_of(&conn, "board", hash).expect("maker")
}

fn edit_caps(e: &Env, caps: &[&str]) {
    let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
    let m = format!("{{\n  \"schema\": 1,\n  \"name\": \"Board\",\n  \"caps\": [{}]\n}}\n", caps.join(", "));
    std::fs::write(e.f.widget_dir("board").join("manifest.json"), m).expect("disk edit");
    crate::zen_routes::refresh_and_emit().expect("refresh");
}

fn sign(caller: &Caller, owner: bool, body: Value) -> (String, Value) {
    let r = crate::zen_routes::handle("/cli/zen/widget/sign", owner, caller, "test", &HashMap::new(), body.to_string().as_bytes());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|err| panic!("{err}: {}", r.body)))
}

fn view_of(ws: &Ws, card: &str) -> Value {
    overlay::get_thread_doc(&ws.conv, card)
        .expect("read")
        .unwrap_or_else(|| panic!("card {card}"))
        .choice
        .expect("a choice")
        .action
        .expect("an action")
        .view
        .expect("a view")
}

fn answer(ws: &Ws, card: &str, label: &str) -> (String, Value) {
    let b = json!({ "addr": ws.handle, "id": card, "answer": label, "client": "phone" });
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), b.to_string().as_bytes(), "owner");
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|err| panic!("{err}: {}", r.body)))
}

fn ctx(ws: &Ws, widget: &str, version: &str) -> Ctx {
    Ctx {
        workspace_id: ws.id.clone(),
        workspace_handle: ws.handle.clone(),
        params: json!({ "widget": widget, "version": version }).as_object().cloned().expect("object"),
    }
}

/// The mismatch is refused until the card is answered; the card lists the
/// difference; Allow signs exactly that version; a later edit needs a new
/// card; "Turn off again" takes the signature off.
#[test]
fn a_manifest_change_is_refused_until_the_person_signs_that_exact_version() {
    let e = env();
    let ws = seed("ana");
    let v0 = live(&e);
    assert!(passes(&e, &v0), "the person signed the first version");

    // The manifest changes on disk: a new version, unsigned.
    edit_caps(&e, &["agents:read", "thread:post"]);
    let v1 = live(&e);
    assert_ne!(v1, v0);
    assert!(!passes(&e, &v1), "a manifest change runs without the person's data");

    // The agent isn't on the Garden's list: refused, nothing stamped, a card.
    let (s, v) = sign(&agent(&ws), false, json!({"widget": "board"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some(SIGN_NEEDS_PERSON_CODE)), "{v}");
    assert_eq!(v["widget"], "board");
    assert_eq!(v["version"], json!(v1), "the exact version the card is for");
    let want = "Since the last signed version, this widget now asks for thread:post.";
    assert_eq!(v["changes"], json!(want), "{v}");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("zen.sign") && m.contains("k2 zen widget sign board")), "{v}");
    let card = v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string();
    assert_eq!(maker(&v1), MadeBy::Unclaimed, "a refused sign stamps nothing");
    assert!(!passes(&e, &v1));

    // The card: K2's words with the difference, the exact version.
    let view = view_of(&ws, &card);
    assert_eq!(view["action"], json!(SIGN), "{view}");
    assert_eq!(view["setting"], json!("Sign board"));
    assert!(view["consequences"].as_str().is_some_and(|c| c.starts_with(want)), "the card lists the difference: {view}");
    assert_eq!(view["values"], json!([{ "key": "workspace", "value": ws.handle }, { "key": "widget", "value": "board" }, { "key": "version", "value": v1 }]));
    assert_eq!(view["approvers"]["floor"], json!("owner"));

    // Asking again names the waiting card.
    let (s, again) = sign(&agent(&ws), false, json!({"widget": "board"}));
    assert_eq!(s, "403 Forbidden");
    assert_eq!(again["card"], json!(card), "one waiting card per version: {again}");

    // The person allows: exactly v1 is signed as theirs.
    let (s, a) = answer(&ws, &card, "Allow");
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["decided"][0]["state"], "approved", "{a}");
    assert_eq!(maker(&v1), MadeBy::Owner);
    assert!(passes(&e, &v1), "the signed version uses the person's data");
    let reg = actions::find(SIGN).expect("registered");
    assert_eq!((reg.check)(&ctx(&ws, "board", &v1)), Check::AlreadyDone);

    // A later edit is a new version: the old card's values are stale, and
    // the agent's sign asks with a new card.
    edit_caps(&e, &["agents:read", "thread:post", "presence:read"]);
    let v2 = live(&e);
    assert!(!passes(&e, &v2), "a later edit needs a new sign");
    assert!(matches!((reg.check)(&ctx(&ws, "board", &v1)), Check::Moved(_)), "the approved card can't sign v2");
    let (s, v) = sign(&agent(&ws), false, json!({"widget": "board"}));
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_ne!(v["card"], json!(card), "a new version, a new card: {v}");
    assert_eq!(v["changes"], json!("Since the last signed version, this widget now asks for presence:read."), "{v}");

    // A version named by the caller that isn't live is refused, never signed.
    let (s, v) = sign(&Caller::Owner, true, json!({"widget": "board", "version": &v1[..12]}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("409 Conflict", Some("stale_version")), "{v}");
    assert_eq!(maker(&v2), MadeBy::Unclaimed);

    // Turn off again (the twin) on v2 once the person signed it by hand.
    let (s, _) = sign(&Caller::Owner, true, json!({"widget": "board", "version": v2}));
    assert_eq!(s, "200 OK");
    assert!(passes(&e, &v2));
    let off = actions::find(SIGN_OFF).expect("twin registered");
    assert_eq!((off.check)(&ctx(&ws, "board", &v2)), Check::Needed);
    match (off.apply)(&ctx(&ws, "board", &v2), &Answerer::OwnerToken, None) {
        Applied::Done { .. } => {}
        other => panic!("twin refused: {other:?}"),
    }
    assert!(!passes(&e, &v2), "signature off: no data again");
    assert_eq!((off.check)(&ctx(&ws, "board", &v2)), Check::AlreadyDone);
}

/// An agent on every placing Garden's list still vouches itself (and the
/// answer says nothing about cards); `--ask` presents the card on purpose.
#[test]
fn a_listed_agent_vouches_and_ask_presents_the_card_on_purpose() {
    let e = env();
    let ws = seed("ed");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::add(&conn, &e.garden, &ws.id, &Change::new(Via::Settings, &Actor::Owner, 1)).expect("list");
    }
    edit_caps(&e, &["agents:read", "thread:read"]);
    let v1 = live(&e);
    let (s, v) = sign(&agent(&ws), false, json!({"widget": "board"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["stamped"].as_bool(), v["vouched"].as_bool()), (Some(true), Some(true)), "{v}");
    assert!(v["review"].is_null(), "{v}");
    assert!(passes(&e, &v1));
    // --ask: a card for the person even so, with the change in plain words.
    let (s, v) = sign(&agent(&ws), false, json!({"widget": "board", "ask": true}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["asked"], true);
    assert!(v["card"].is_string(), "{v}");
    assert_eq!(v["changes"], json!("Since the last signed version, this widget now asks for thread:read."), "{v}");
}

/// The upgrade carried a signature although the manifest had changed: the
/// widget's answers offer the card (`review`), `--ask` presents it with the
/// logged difference, and the person's Allow marks it reviewed.
#[test]
fn a_carried_difference_offers_the_card_until_the_person_signs() {
    let e = env();
    let ws = seed("cy");
    edit_caps(&e, &["agents:read", "presence:read"]);
    let v1 = live(&e);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // What the carry wrote: the old signer kept, the difference logged.
        authors::vouch(&conn, "board", &v1, &MadeBy::Owner, StampSource::Import, CallerKind::Owner, 2).expect("carried");
        conn.execute(
            "INSERT INTO zen_version_carry (widget, place, old_hash, new_hash, carried, maker, differs, at)
             VALUES ('board', 'live', ?1, ?2, 1, 'owner', 'now asks for presence:read', 2)",
            rusqlite::params!["0".repeat(64), v1],
        )
        .expect("carry row");
    }
    assert!(passes(&e, &v1), "carried: it keeps running with the person's data");
    let reg = actions::find(SIGN).expect("registered");
    assert_eq!((reg.check)(&ctx(&ws, "board", &v1)), Check::Needed, "the person hasn't seen the difference yet");
    // A non-voucher's sign passes (it's signed), and the answer offers the card.
    let (s, v) = sign(&agent(&ws), false, json!({"widget": "board"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["review"]["ask"], json!("k2 zen widget sign board --ask"), "{v}");
    assert_eq!(v["review"]["changes"], json!("Since it was signed, this widget now asks for presence:read."));
    let (s, v) = sign(&agent(&ws), false, json!({"widget": "board", "ask": true}));
    assert_eq!(s, "200 OK", "{v}");
    let card = v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string();
    assert!(
        view_of(&ws, &card)["consequences"].as_str().is_some_and(|c| c.starts_with("Since it was signed, this widget now asks for presence:read.")),
        "the card shows the logged difference"
    );
    let (s, a) = answer(&ws, &card, "Allow");
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["decided"][0]["state"], "approved", "{a}");
    let pending = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::zen::version_carry::unreviewed_difference(&conn, "board", &v1).expect("read")
    };
    assert!(pending.is_none(), "reviewed once the person signed");
    assert_eq!((reg.check)(&ctx(&ws, "board", &v1)), Check::AlreadyDone);
    let (_, v) = sign(&agent(&ws), false, json!({"widget": "board"}));
    assert!(v["review"].is_null(), "{v}");
}

const SIGN_NEEDS_PERSON_CODE: &str = crate::zen_access_routes::SIGN_NEEDS_PERSON;
