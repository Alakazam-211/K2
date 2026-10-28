//! Garden access A4 (prd-garden-access-v1 G16–G18, §8 "Card"): K2's two
//! Garden cards end to end through the real permission core: present →
//! the standard `cards=2` card → a person's answer on the Thread route →
//! the one door (list row + History) or the Settings delete; Not now and
//! Keep it; the twin ("Turn off again"); the switch off (`not_available`)
//! and on (`retired`); the switch-on void; Settings/card parity for both
//! prompts. Temp HOME (its env lock keeps the switch to this test), temp
//! DB (migration 0219), temp Zen folder; no test wakes a real agent. Fail
//! loudly.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay;
use k2_core::permission::request::RequestState;
use k2_core::zen::garden_access::AddedBy;
use serde_json::{json, Value};

use super::*;
use crate::compute_routes::Who;
use k2_core::zen::garden_access as ga;
use crate::permission::actions::{self, Check, Ctx};
use crate::permission::routes::{self as proutes, handle_gated};
use crate::sidecar_routes::{AgentCaller, Caller};

// ── fixtures ────────────────────────────────────────────────────────

/// The switch as `on` until dropped (then off), even on a panic. Taken
/// while the TempHome's env lock is held, so no other test sees it.
struct Switch;

impl Switch {
    fn set(on: bool) -> Self {
        ga::set_switch(on);
        Switch
    }
}

impl Drop for Switch {
    fn drop(&mut self) {
        ga::set_switch(false);
    }
}

struct Env {
    // Drop order: the switch goes off before the HOME (and its lock) goes.
    switch: Option<Switch>,
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    f: k2_core::zen::ZenFiles,
}

/// Temp HOME + this thread's DB + a set-up Zen folder, with the switch as
/// `on`.
fn env(on: bool) -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let f = k2_core::zen::ZenFiles::local();
    f.setup().expect("zen setup");
    Env { switch: Some(Switch::set(on)), _db: db, _home: home, f }
}

struct Ws {
    id: String,
    handle: String,
    conv: String,
}

/// An agent workspace with its pinned Chat (the Thread its cards go on).
fn seed(tag: &str) -> Ws {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let id = uuid::Uuid::new_v4().to_string();
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, format!("/tmp/zen-a4/{handle}")],
    )
    .expect("seed project");
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

fn login(name: &str, role: Role) -> String {
    let user = format!("{name}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&user, "password-for-a4-9").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    user
}

/// The owner person's own login (the phone they paired): an Owner-role
/// login the owner LINKED (`ownerAccounts`). An unlinked Owner-role login
/// is another person and isn't the Garden's owner (GP-B1).
fn owner_login(name: &str) -> String {
    let user = login(name, Role::Owner);
    k2_core::app_settings::update(json!({ "ownerAccounts": [&user] })).expect("link to the owner");
    user
}

/// A new Garden: `(id, name)`.
fn new_garden(e: &Env, name: &str) -> (String, String) {
    let g = e.f.new_garden(name, None, None, None).unwrap_or_else(|err| panic!("new garden {name}: {err:?}"));
    (g.id, g.name)
}

fn made(garden: &str, ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(ga::record_made(&conn, garden, &ws.id, 1).expect("made row"), "first maker");
}

/// The three asks K2 presents: the maker's, another agent's, a delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Made,
    Join,
    Delete,
}

fn present_for(kind: Kind, ws: &Ws, g: &(String, String), explicit: bool, reason: Option<&str>) -> Value {
    let (k, made) = match kind {
        Kind::Made => (GardenAsk::Access, true),
        Kind::Join => (GardenAsk::Access, false),
        Kind::Delete => (GardenAsk::Delete, false),
    };
    present_with_reason(k, &ws.id, &g.0, made, explicit, reason)
}

fn detail(kind: Kind, ws: &Ws, g: &(String, String), explicit: bool) -> Value {
    present_for(kind, ws, g, explicit, None)
}

fn list(garden: &str) -> Vec<(String, AddedBy, Option<String>)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::list(&conn, garden).expect("list").into_iter().map(|r| (r.project_id, r.added_by, r.card_id)).collect()
}

/// History kinds and actors for a Garden, oldest first.
fn events(garden: &str) -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut ev: Vec<(String, String)> = ga::events_for_garden(&conn, garden, &[], 200, None)
        .expect("history")
        .into_iter()
        .map(|e| (e.kind.as_str().to_string(), e.actor))
        .collect();
    ev.reverse();
    ev
}

fn ask_row(garden: &str, ws: &Ws, kind: AskKind) -> ga::AskRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::ask_state(&conn, garden, &ws.id, kind).expect("asks read").unwrap_or_else(|| panic!("no asks row for {garden}"))
}

fn card_of(v: &Value) -> String {
    v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string()
}

fn doc(ws: &Ws, card: &str) -> overlay::OverlayDoc {
    overlay::get_thread_doc(&ws.conv, card).expect("read").unwrap_or_else(|| panic!("card {card}"))
}

fn view_of(ws: &Ws, card: &str) -> Value {
    doc(ws, card).choice.expect("a choice").action.expect("an action").view.expect("a view")
}

/// The person's answer on the Thread route (`POST /cli/thread/answer`).
fn answer(author: &str, ws: &Ws, card: &str, label: &str) -> (String, Value) {
    let b = json!({ "addr": ws.handle, "id": card, "answer": label, "client": "phone" });
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), b.to_string().as_bytes(), author);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn decided(a: &Value) -> String {
    a["decided"][0]["state"].as_str().unwrap_or_else(|| panic!("nothing decided: {a}")).to_string()
}

fn ctx(ws: &Ws, params: Value) -> Ctx {
    Ctx { workspace_id: ws.id.clone(), workspace_handle: ws.handle.clone(), params: params.as_object().cloned().expect("an object") }
}

fn check(action: &str, ws: &Ws, garden: &str) -> Check {
    let r = actions::find(action).unwrap_or_else(|| panic!("{action} is not registered"));
    (r.check)(&ctx(ws, json!({ "garden": garden })))
}

/// An agent's own Beta ask (`k2 permission request …`, gate passed).
fn beta_ask(ws: &Ws, action: &str, params: Value) -> (String, Value) {
    let who = Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) };
    let body = json!({ "action": action, "params": params, "why": "Tidy the board" });
    let r = handle_gated(proutes::REQUEST, &who, true, body.to_string().as_bytes(), &HashMap::new());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

// ── off ─────────────────────────────────────────────────────────────

/// G0a item 4: the three rows are listed in every state; off, their checks
/// answer `not_available`, K2 presents nothing and writes nothing. Keep as
/// mine and proposal approve are as 0.46.3.
#[test]
fn off_the_rows_are_listed_and_answer_not_available() {
    let e = env(false);
    let ws = seed("a4off");
    let g = new_garden(&e, "Sales board");
    for id in [ACCESS, ACCESS_OFF, DELETE] {
        assert!(actions::find(id).is_some(), "{id} is listed while off");
        match check(id, &ws, &g.0) {
            Check::Unavailable { code, why } => assert_eq!((code, why.as_str()), ("not_available", OFF_WORDS), "{id}"),
            other => panic!("{id}: {other:?}"),
        }
    }
    let (s, v) = beta_ask(&ws, ACCESS, json!({ "garden": g.0 }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("409 Conflict", Some("not_available")), "{v}");
    assert_eq!(detail(Kind::Made, &ws, &g, true), json!({ "card": null, "asked": false }));
    assert!(list(&g.0).is_empty() && events(&g.0).is_empty(), "nothing written while off");
    assert!(crate::permission::actions::gardens::retired().is_none(), "Keep as mine isn't retired while off");
    let keep = actions::find("garden.keep-mine").expect("registered");
    let c = (keep.check)(&ctx(&ws, json!({ "widget": "board", "version": "ab".repeat(32) })));
    assert!(!matches!(c, Check::Unavailable { .. }), "{c:?}");
}

// ── the access card ─────────────────────────────────────────────────

/// §1.5 made → Allow: one standard permission card (cards=2 view, Allow /
/// Deny, click), the asks row and an `access_asked` line; an Admin is below
/// the floor; the Owner login allows on the phone; the agent is listed by
/// `card` with the card id, an `access_added` line, and its one turn.
#[test]
fn made_then_allow_lists_the_agent_through_the_card() {
    let e = env(true);
    let owner = owner_login("a4o");
    let admin = login("a4a", Role::Admin);
    let ws = seed("billing");
    let g = new_garden(&e, "Sales board");
    made(&g.0, &ws);

    let v = detail(Kind::Made, &ws, &g, true);
    let card = card_of(&v);
    assert_eq!(v["addr"], json!(ws.handle), "{v}");
    let d = doc(&ws, &card);
    let choice = d.choice.as_ref().expect("a choice card");
    assert_eq!(choice.prompt, format!("{} made Sales board and wants to keep editing it. Allow?", ws.handle));
    let labels: Vec<&str> = choice.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["Allow", "Deny"]);
    let action = choice.action.as_ref().expect("an action");
    assert_eq!(action.id, k2_core::permission::CARD_ACTION_PERMISSION, "a standard permission card (Appendix C)");
    let view = action.view.as_ref().expect("a stored view");
    assert_eq!((view["v"].as_u64(), view["kind"].as_str()), (Some(1), Some("permission")), "{view}");
    assert_eq!(view["action"], json!(ACCESS));
    assert_eq!(view["setting"], json!("Edit Sales board"), "the name, never the id: {view}");
    assert_eq!(view["whatChanges"], json!(format!("Let {} edit Sales board.", ws.handle)));
    assert_eq!(view["values"], json!([{ "key": "workspace", "value": ws.handle }, { "key": "garden", "value": "Sales board" }]));
    assert_eq!((view["grant"].as_str(), view["proof"].clone()), (Some("standing"), json!({ "kind": "click" })));
    assert_eq!(view["approvers"]["floor"], json!("owner"));
    assert_eq!(view["why"]["text"], json!("I made Sales board and want to keep editing it."));
    let row = ask_row(&g.0, &ws, AskKind::Access);
    assert_eq!(row.card_id.as_deref(), Some(card.as_str()));
    assert!(row.last_ask_at.is_some() && row.last_not_now_at.is_none(), "{row:?}");
    assert_eq!(events(&g.0), vec![("access_asked".to_string(), format!("agent:{}", ws.id))]);

    let (s, a) = answer(&admin, &ws, &card, "Allow");
    assert_eq!((s.as_str(), a["error"]["code"].as_str()), ("403 Forbidden", Some("action_card_person_only")), "{a}");
    assert!(list(&g.0).is_empty(), "an Admin moves nothing");

    let (s, a) = answer(&owner, &ws, &card, "Allow");
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(decided(&a), "approved", "{a}");
    assert_eq!(list(&g.0), vec![(ws.id.clone(), AddedBy::Card, Some(card.clone()))]);
    assert_eq!(events(&g.0).last(), Some(&("access_added".to_string(), format!("login:{owner}"))));
    assert_eq!(ask_row(&g.0, &ws, AskKind::Access).card_id, None, "the card isn't waiting any more");
    assert_eq!(check(ACCESS, &ws, &g.0), Check::AlreadyDone);
    let turns = crate::permission::inject::sent_to(&ws.handle);
    assert!(
        turns.iter().any(|t| t.contains("approved by") && t.contains("Edit Sales board is now on for the Sales board Garden")),
        "{turns:?}"
    );
}

/// §1.5 dedupe: one waiting card per (Garden, agent, kind); the first
/// refusal presents, later refusals name the waiting card (or nothing);
/// a delete ask is its own card; a shell tab is "a terminal in X's room".
#[test]
fn one_waiting_card_per_garden_and_agent() {
    let e = env(true);
    let ws = seed("ana");
    let g = new_garden(&e, "Fleet Ops");
    let first = detail(Kind::Join, &ws, &g, false);
    let card = card_of(&first);
    assert_eq!(
        doc(&ws, &card).choice.unwrap().prompt,
        format!("Let {0} edit Fleet Ops? {0} can change its layout and widgets, and use their data.", ws.handle)
    );
    for explicit in [false, true] {
        let again = detail(Kind::Join, &ws, &g, explicit);
        assert_eq!((again["card"].as_str(), again["waiting"].as_bool()), (Some(card.as_str()), Some(true)), "{again}");
        assert_eq!(present(GardenAsk::Access, &ws.id, &g.0, false, explicit)["card"], json!(card));
    }
    // After the card is answered, a refusal posts nothing; an explicit ask does.
    let (s, _) = answer("owner", &ws, &card, "Allow");
    assert_eq!(s, "200 OK");
    let r = access_entry(&Caller::Owner, &g.0, &ws.id, false, "settings");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(detail(Kind::Join, &ws, &g, false), json!({ "card": null, "asked": true }));
    assert!(detail(Kind::Join, &ws, &g, true)["card"].is_string(), "an explicit ask posts again");

    // A delete ask is its own kind; the agent's own words stand in `why`.
    made(&g.0, &ws);
    let dv = present_for(Kind::Delete, &ws, &g, true, Some("It duplicates Garden 1"));
    assert_ne!(card_of(&dv), card);
    assert_eq!(view_of(&ws, &card_of(&dv))["why"]["text"], json!("It duplicates Garden 1"));
    // A reason that looks like a secret never reaches the card.
    let ws2 = seed("sec");
    let sv = present_for(Kind::Join, &ws2, &g, true, Some("sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
    assert_eq!(view_of(&ws2, &card_of(&sv))["why"]["text"], json!("I'd like to edit Fleet Ops."));

    // A gone Garden gets no card.
    let gone = ("g-00000000".to_string(), "Gone".to_string());
    assert_eq!(detail(Kind::Join, &ws, &gone, true), json!({ "card": null, "asked": false }));
}

/// Not now (Deny) is stored with a History line; no new card for 24 h.
#[test]
fn not_now_holds_a_new_card_back_for_a_day() {
    let e = env(true);
    let ws = seed("nn");
    let g = new_garden(&e, "Kanban notes");
    let card = card_of(&detail(Kind::Join, &ws, &g, true));
    let (s, a) = answer("owner", &ws, &card, "Deny");
    assert_eq!((s.as_str(), decided(&a).as_str()), ("200 OK", "denied"), "{a}");
    let row = ask_row(&g.0, &ws, AskKind::Access);
    let at = row.last_not_now_at.expect("Not now stored");
    assert_eq!(row.card_id, None);
    assert_eq!(events(&g.0).last(), Some(&("access_not_now".to_string(), "owner".to_string())));
    let v = detail(Kind::Join, &ws, &g, true);
    assert_eq!(v, json!({ "card": null, "notNowUntil": at + ga::NOT_NOW_QUIET_SECS }));
    assert!(list(&g.0).is_empty());
    // A day later an explicit ask posts again.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE zen_garden_access_asks SET last_not_now_at = last_not_now_at - ?1 WHERE garden_id = ?2",
            rusqlite::params![ga::NOT_NOW_QUIET_SECS + 1, g.0],
        )
        .expect("age the Not now");
    }
    assert!(detail(Kind::Join, &ws, &g, true)["card"].is_string());
}

/// The twin: "Turn off again" on the approved card removes the agent
/// through the same door (`access_removed`); asked directly, the twin is
/// `already_done` for an unlisted agent.
#[test]
fn turn_off_again_runs_the_tighten_twin() {
    let e = env(true);
    let ws = seed("twin");
    let g = new_garden(&e, "Board");
    assert_eq!(check(ACCESS_OFF, &ws, &g.0), Check::AlreadyDone);
    let card = card_of(&detail(Kind::Join, &ws, &g, true));
    let (s, a) = answer("owner", &ws, &card, "Allow");
    assert_eq!((s.as_str(), decided(&a).as_str()), ("200 OK", "approved"), "{a}");
    assert_eq!(view_of(&ws, &card)["undo"]["label"], json!("Turn off again"));
    assert_eq!(check(ACCESS_OFF, &ws, &g.0), Check::Needed);
    let b = json!({ "addr": ws.handle, "id": card });
    let r = crate::overlay_routes::dispatch_post_as(k2_core::permission::UNDO_ROUTE, &HashMap::new(), b.to_string().as_bytes(), "owner");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(list(&g.0).is_empty(), "removed");
    assert_eq!(events(&g.0).last(), Some(&("access_removed".to_string(), "owner".to_string())));
}

/// A rename between the ask and the click isn't stale: the card shows the
/// current name and the Allow applies.
#[test]
fn a_renamed_garden_is_named_by_its_current_name() {
    let e = env(true);
    let ws = seed("ren");
    let g = new_garden(&e, "Old name");
    let card = card_of(&detail(Kind::Join, &ws, &g, true));
    e.f.rename_garden(&g.0, "New name").expect("rename");
    let reg = actions::find(ACCESS).expect("registered");
    let w = crate::permission::present::words(reg.spec, &ctx(&ws, json!({ "garden": g.0 })));
    assert_eq!((w.setting.as_str(), w.what_changes.clone()), ("Edit New name", format!("Let {} edit New name.", ws.handle)));
    let (s, a) = answer("owner", &ws, &card, "Allow");
    assert_eq!((s.as_str(), decided(&a).as_str()), ("200 OK", "approved"), "{a}");
    assert_eq!(list(&g.0).len(), 1);
}

// ── the delete card ─────────────────────────────────────────────────

/// §1.5a: the maker asks; Delete by the Owner login deletes through the
/// Settings route (a `.history` copy); then the check is `already_done`; an
/// agent that's neither listed nor the maker is `stale` at the click; Keep
/// it is stored; the last Garden can't go.
#[test]
fn the_delete_card_deletes_as_the_person() {
    let e = env(true);
    let owner = owner_login("a4do");
    let ws = seed("del");
    let stranger = seed("str");
    let g = new_garden(&e, "Sales board");
    made(&g.0, &ws);
    assert_eq!(check(DELETE, &ws, &g.0), Check::Needed);
    assert!(matches!(check(DELETE, &stranger, &g.0), Check::Moved(_)));
    let card = card_of(&detail(Kind::Delete, &ws, &g, true));
    assert_eq!(
        doc(&ws, &card).choice.unwrap().prompt,
        format!("{} asks to delete Sales board. It leaves your Gardens. K2 keeps a copy in history.", ws.handle)
    );
    let view = view_of(&ws, &card);
    assert_eq!((view["setting"].as_str(), view["grant"].as_str()), (Some("Delete Garden"), Some("one-time")), "{view}");
    assert_eq!(view["undo"], Value::Null);
    assert_eq!(events(&g.0).last().map(|e| e.0.as_str()), Some("delete_asked"));
    let (s, a) = answer(&owner, &ws, &card, "Allow");
    assert_eq!((s.as_str(), decided(&a).as_str()), ("200 OK", "approved"), "{a}");
    assert!(e.f.find_garden(&g.0).is_none(), "deleted");
    assert!(e.f.garden_file(&g.0, true).is_ok(), "K2 keeps a copy in history");
    assert_eq!(view_of(&ws, &card)["whatChanges"], json!("Delete Sales board."), "a deleted Garden keeps its name on the card");
    assert_eq!(check(DELETE, &ws, &g.0), Check::AlreadyDone, "a gone Garden is already done");

    // A stranger (neither listed nor the maker) is stale at the click:
    // nothing deleted.
    let g2 = new_garden(&e, "Notes board");
    made(&g2.0, &ws);
    let sc = card_of(&detail(Kind::Delete, &stranger, &g2, true));
    let (s, a) = answer("owner", &stranger, &sc, "Allow");
    assert_eq!((s.as_str(), decided(&a).as_str()), ("200 OK", "stale"), "{a}");
    assert!(e.f.find_garden(&g2.0).is_some());

    // Keep it.
    let card = card_of(&detail(Kind::Delete, &ws, &g2, true));
    let (s, a) = answer("owner", &ws, &card, "Deny");
    assert_eq!((s.as_str(), decided(&a).as_str()), ("200 OK", "denied"), "{a}");
    assert!(ask_row(&g2.0, &ws, AskKind::Delete).last_not_now_at.is_some());
    assert!(events(&g2.0).iter().any(|e| e.0 == "delete_kept" && e.1 == "owner"), "{:?}", events(&g2.0));
    assert!(e.f.find_garden(&g2.0).is_some(), "kept");
    assert!(detail(Kind::Delete, &ws, &g2, true)["notNowUntil"].is_i64());

    // The last Garden can't go.
    for other in e.f.gardens().into_iter().map(|g| g.id).filter(|id| *id != g2.0) {
        e.f.delete_garden(&other).expect("delete the others");
    }
    assert!(matches!(check(DELETE, &ws, &g2.0), Check::Moved(m) if m.contains("last Garden")));
}

/// One door: the entries refuse an Admin, a Member, an agent and a shell
/// tab; the owner token and an Owner login pass; the card door has the same
/// floor for both prompts and the same words.
#[test]
fn settings_and_the_card_are_one_door_for_both_prompts() {
    let e = env(true);
    let owner = owner_login("a4po");
    let admin = login("a4pa", Role::Admin);
    let member = login("a4pm", Role::Member);
    let ws = seed("par");
    let g = new_garden(&e, "Shared");
    let agent = Caller::Agent(AgentCaller { project_id: ws.id.clone(), map_key: None, args: vec![], sandboxed: false });
    // GP-B1: another person's unlinked Owner-role login is refused like an Admin.
    let other_owner = login("a4px", Role::Owner);
    for c in [
        Caller::Login { username: admin.clone() },
        Caller::Login { username: member },
        Caller::Login { username: other_owner },
        agent,
        Caller::Shell { project_id: ws.id.clone() },
    ] {
        let r = access_entry(&c, &g.0, &ws.id, true, "settings");
        assert_eq!(r.status, "403 Forbidden", "{c:?}: {}", r.body);
        let r = delete_entry(&c, &g.0, "settings");
        assert_eq!(r.status, "403 Forbidden", "{c:?}: {}", r.body);
    }
    assert!(list(&g.0).is_empty());
    let r = access_entry(&Caller::Login { username: owner.clone() }, &g.0, &ws.handle, true, "settings");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(list(&g.0), vec![(ws.id.clone(), AddedBy::Owner, None)]);
    let r = access_entry(&Caller::Owner, &g.0, &ws.id, false, "settings");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(list(&g.0).is_empty());
    for kind in [Kind::Made, Kind::Join] {
        let card = card_of(&detail(kind, &ws, &g, true));
        assert_eq!(view_of(&ws, &card)["whatChanges"], json!(format!("Let {} edit Shared.", ws.handle)), "{kind:?}");
        let (s, _) = answer(&admin, &ws, &card, "Allow");
        assert_eq!(s, "403 Forbidden", "{kind:?}");
        let (s, a) = answer(&owner, &ws, &card, "Allow");
        assert_eq!((s.as_str(), decided(&a).as_str()), ("200 OK", "approved"), "{kind:?}: {a}");
        assert_eq!(list(&g.0).len(), 1);
        let r = access_entry(&Caller::Owner, &g.0, &ws.id, false, "settings");
        assert_eq!(r.status, "200 OK", "{}", r.body);
    }
}

// ── on: retired, and the switch-on void ─────────────────────────────

/// G18: with the switch on, Keep as mine and proposal approve answer
/// `retired` (asked); the switch-on void closes the old path's waiting
/// Keep-as-mine cards and its open requests, once.
#[test]
fn switch_on_retires_keep_mine_and_proposals_and_voids_what_waits() {
    let mut e = env(false);
    let ws = seed("old");
    let g = new_garden(&e, "Legacy");
    // While off: an open garden.keep-mine request and a Keep-as-mine card.
    let live = "ab".repeat(32);
    let mut params = serde_json::Map::new();
    params.insert("widget".into(), json!("board"));
    params.insert("version".into(), json!(live));
    let now = overlay::now_secs();
    let req = k2_core::permission::request::PermissionRequest {
        id: k2_core::permission::request::RequestId::new(),
        workspace_id: ws.id.clone(),
        session_id: Some("sess-old".into()),
        conversation_id: None,
        addr: Some(ws.handle.clone()),
        card_id: None,
        action: "garden.keep-mine".into(),
        params_hash: k2_core::permission::request::params_hash(&params),
        params: Value::Object(params),
        text_hash: "x".into(),
        why: "keep it".into(),
        state: RequestState::Pending,
        floor: "garden-owner".into(),
        held_ref: None,
        batch_id: None,
        ask_number: 1,
        asked_for: None,
        created_at: now,
        expires_at: now + 3600,
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    crate::permission::store::insert(&req).expect("insert");
    let keep = crate::overlay_routes::present_action_card(
        None,
        &ws.id,
        "keep?",
        &[crate::action_cards::KEEP_AS_MINE, "Leave it"],
        overlay::CardAction { id: crate::action_cards::KEEP_WIDGET.into(), params: json!({}), dedupe: "board".into(), result: None, view: None },
        false,
        now + 3600,
        crate::overlay_routes::CardAudience::AgentPost,
    )
    .expect("keep card");
    let crate::overlay_routes::Presented::Card { id: keep_id, .. } = keep else { panic!("{keep:?}") };

    // The person turns Garden access lists on (A1's flip, which runs the
    // void): the old guard's drop turns the switch off, so drop it first.
    e.switch = None;
    assert!(crate::settings_routes::sync_garden_access(true, "owner"), "off -> on");
    e.switch = Some(Switch::set(true));
    assert_eq!(doc(&ws, &keep_id).choice.unwrap().status, "voided", "the flip voided the Keep-as-mine card");
    let after = crate::permission::store::get(req.id.as_str()).expect("read").expect("row");
    assert_eq!((after.state, after.result_message.as_deref()), (RequestState::Cancelled, Some(RETIRED_WORDS)));
    for (id, params) in [("garden.keep-mine", json!({ "widget": "board", "version": live })), ("garden.proposal.approve", json!({ "proposal": "zp_1" }))] {
        let r = actions::find(id).expect("still registered");
        match (r.check)(&ctx(&ws, params.clone())) {
            Check::Unavailable { code, why } => assert_eq!((code, why.as_str()), ("retired", RETIRED_WORDS), "{id}"),
            other => panic!("{id}: {other:?}"),
        }
        let (s, v) = beta_ask(&ws, id, params);
        assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("409 Conflict", Some("retired")), "{id}: {v}");
    }
    assert_eq!(void_on_switch_on(), 0, "a second run closes nothing");
    assert!(list(&g.0).is_empty(), "the void lists nobody");
}

/// The void itself counts what it closed: one card and one request.
#[test]
fn the_void_counts_cards_and_requests() {
    let _e = env(true);
    let ws = seed("cnt");
    let keep = crate::overlay_routes::present_action_card(
        None,
        &ws.id,
        "keep?",
        &[crate::action_cards::KEEP_AS_MINE, "Leave it"],
        overlay::CardAction { id: crate::action_cards::KEEP_WIDGET.into(), params: json!({}), dedupe: "w".into(), result: None, view: None },
        false,
        overlay::now_secs() + 3600,
        crate::overlay_routes::CardAudience::AgentPost,
    )
    .expect("keep card");
    assert!(matches!(keep, crate::overlay_routes::Presented::Card { .. }), "{keep:?}");
    let mut params = serde_json::Map::new();
    params.insert("proposal".into(), json!("zp_9"));
    let now = overlay::now_secs();
    let req = k2_core::permission::request::PermissionRequest {
        id: k2_core::permission::request::RequestId::new(),
        workspace_id: ws.id.clone(),
        session_id: Some("sess-cnt".into()),
        conversation_id: None,
        addr: Some(ws.handle.clone()),
        card_id: None,
        action: "garden.proposal.approve".into(),
        params_hash: k2_core::permission::request::params_hash(&params),
        params: Value::Object(params),
        text_hash: "x".into(),
        why: "approve it".into(),
        state: RequestState::Pending,
        floor: "garden-owner".into(),
        held_ref: None,
        batch_id: None,
        ask_number: 1,
        asked_for: None,
        created_at: now,
        expires_at: now + 3600,
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    crate::permission::store::insert(&req).expect("insert");
    assert_eq!(void_on_switch_on(), 2, "one Keep-as-mine card and one request");
    assert_eq!(void_on_switch_on(), 0, "then nothing");
}

/// Deny on any other action leaves the asks table alone.
#[test]
fn on_denied_ignores_other_actions() {
    let e = env(true);
    let ws = seed("other");
    let g = new_garden(&e, "Any board");
    let req = k2_core::permission::request::PermissionRequest {
        id: k2_core::permission::request::RequestId::new(),
        workspace_id: ws.id.clone(),
        session_id: None,
        conversation_id: None,
        addr: None,
        card_id: None,
        action: "compute.use".into(),
        params: json!({ "garden": g.0 }),
        params_hash: String::new(),
        text_hash: String::new(),
        why: String::new(),
        state: RequestState::Denied,
        floor: "admin".into(),
        held_ref: None,
        batch_id: None,
        ask_number: 1,
        asked_for: None,
        created_at: 1,
        expires_at: 2,
        answered_at: Some(1),
        answered_by: Some("owner".into()),
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    on_denied(&req);
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(ga::ask_state(&conn, &g.0, &ws.id, AskKind::Access).expect("read"), None);
    assert!(ga::events_for_garden(&conn, &g.0, &[], 10, None).expect("history").is_empty());
}
