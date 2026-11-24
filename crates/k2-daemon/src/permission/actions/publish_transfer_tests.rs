//! `publish.transfer` (0.46.18): after a server swap a site's name can belong
//! to an old workspace (`label_owned_by_other_workspace`), and `--transfer`
//! is the owner's or an Admin's. The agent asks on a card; a person's Allow
//! runs the claim route's own entry with `transfer=1` as that person, and the
//! move is audited with who clicked.
//!
//! Test DB, temp HOME; no network, no real agent is woken.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::db::schema::SubdomainWorkspace;
use k2_core::permission::catalog::{Direction, Floor, Grant, Proof, Undo, WallCoverage, WallMatch};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, ask_refused, below_floor, card_of, last_turn, login, seed, view};
use super::*;

fn holder(label: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    SubdomainWorkspace::owner_of(&conn, label).expect("owner_of")
}

/// `label` held by `ws` (what a server swap leaves behind).
fn held_by(label: &str, ws: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = SubdomainWorkspace::unclaim(&conn, label);
    SubdomainWorkspace::self_stamp(&conn, label, ws, None).expect("stamp");
}

fn label(tag: &str) -> String {
    format!("pt{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

#[test]
fn the_card_is_an_admins_answer_and_walls_both_refusals() {
    let _db = k2_core::db::scoped_for_test();
    let r = super::super::find(TRANSFER).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant, r.spec.direction), (Floor::Admin, Proof::Click, Grant::OneTime, Direction::Loosen));
    assert!(matches!(r.spec.undo, Undo::None));
    assert_eq!(r.spec.coverage, WallCoverage::Code);
    let walls: Vec<WallMatch> = r.spec.walls.iter().map(|w| w.matches).collect();
    assert_eq!(
        walls,
        vec![
            WallMatch::Code { status: 409, code: "label_owned_by_other_workspace" },
            WallMatch::Code { status: 403, code: "transfer_owner_only" },
        ]
    );
    assert_eq!(r.spec.params.iter().map(|p| p.key).collect::<Vec<_>>(), vec!["label"]);
    let floor = crate::routes::route_policy::lookup(CLAIM_ROUTE).and_then(|x| x.post).expect("the claim route has a POST floor");
    assert!(floor.admits(Role::Admin) && floor.admits(Role::Owner), "{CLAIM_ROUTE}: the card's answerers may use the route");
    assert!(crate::permission::walls::V1_ACTIONS.contains(&TRANSFER));
}

/// The report: the old workspace holds the name; the agent's own
/// `--transfer` is refused; it asks; a Member can't answer; an Admin's Allow
/// moves the name to the asking workspace, audited as that Admin.
#[test]
fn an_agent_asks_and_an_admins_allow_moves_the_name_and_is_audited() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let old = seed("ptold");
    let me = seed("ptnew");
    let l = label("a");
    held_by(&l, &old.id);

    // The agent's own --transfer: the route refuses it (it can't fix it alone).
    let mut p = HashMap::new();
    p.insert("label".to_string(), l.clone());
    p.insert("project".to_string(), me.id.clone());
    p.insert("transfer".to_string(), "1".to_string());
    let r = crate::label_owner::with_caller(crate::label_owner::Caller::Agent, || crate::publish_routes::handle_subdomain_claim(&p));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!((&v["code"], &v["label"]), (&json!("transfer_owner_only"), &json!(l)), "{v}");

    let asked = ask_ok(&me, TRANSFER, json!({ "label": l }));
    let card = card_of(&asked);
    let vw = view(&me, &card);
    let what = vw["whatChanges"].as_str().expect("whatChanges");
    assert!(what.starts_with(&format!("Move the site name {l} to {}. It belongs to ", me.handle)), "{what}");
    assert!(what.contains(&old.handle), "names who holds it now: {what}");
    let text = vw.to_string();
    assert!(!text.contains(&old.id) && !text.contains(&format!("/tmp/pc-e2-{}", old.handle)), "never the holder's id or folder: {text}");

    // A Member is below the card's floor; nothing moves.
    let member = login("ptm", Role::Member);
    below_floor(answer_as(&member, &me, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(holder(&l).as_deref(), Some(old.id.as_str()), "untouched before the Allow");

    let admin = login("pta", Role::Admin);
    let (s, a) = answer_as(&admin, &me, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(holder(&l).as_deref(), Some(me.id.as_str()), "the name is the asking workspace's now");
    let turn = last_turn(&me);
    assert!(turn.contains(&format!("{l} is {}'s now", me.handle)) && turn.contains("without --transfer"), "{turn}");

    let events = k2_core::auth_audit::tail(50).expect("audit tail");
    let hit = events
        .iter()
        .find(|e| e["event"] == "label-transfer" && e["outcome"].as_str().is_some_and(|o| o.contains(&format!("label={l} "))))
        .unwrap_or_else(|| panic!("no label-transfer audit line for {l}: {events:?}"));
    let outcome = hit["outcome"].as_str().expect("outcome");
    assert!(outcome.contains(&format!("from={}", old.id)) && outcome.contains(&format!("to={}", me.id)), "{outcome}");
    assert_eq!(hit["user"], format!("connect:{admin}"), "who clicked: {hit}");

    // Asking again: already done, nothing new.
    let (s, again) = ask(&me, TRANSFER, json!({ "label": l }));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
}

/// A name that moves again between the ask and the click: the card's words
/// named the old holder, so the Allow is stale and nothing moves.
#[test]
fn a_name_that_moves_again_before_the_click_is_stale() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let old = seed("ptso");
    let third = seed("ptst");
    let me = seed("ptsn");
    let l = label("s");
    held_by(&l, &old.id);
    let card = card_of(&ask_ok(&me, TRANSFER, json!({ "label": l })));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(SubdomainWorkspace::transfer(&conn, &l, &old.id, &third.id, None).expect("move"), "moved to a third workspace");
    }
    let admin = login("ptsa", Role::Admin);
    let (s, a) = answer_as(&admin, &me, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "stale", "{a}");
    assert_eq!(holder(&l).as_deref(), Some(third.id.as_str()), "nothing moved");
}

/// What can't be asked: a name nobody holds (claim it), a reserved door, a
/// word that is no site name; the workspace comes from the passport.
#[test]
fn a_free_reserved_or_bad_name_is_refused_at_the_ask() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let me = seed("ptr");
    let free = label("f");
    assert!(ask_refused(&me, TRANSFER, json!({ "label": free })).contains("nothing to move"));
    assert!(ask_refused(&me, TRANSFER, json!({ "label": "meet" })).contains("meet"));
    assert!(ask_refused(&me, TRANSFER, json!({ "label": "Not a name!" })).contains("1-63"));
    let other = seed("ptro");
    let l = label("w");
    held_by(&l, &other.id);
    let (s, _) = ask(&me, TRANSFER, json!({ "label": l, "workspace": other.handle }));
    assert!(s.starts_with("400"), "the workspace comes from the passport");
}
