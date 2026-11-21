//! `vault.accept-from` (0.46.17): an agent asks once for several agents to
//! take one agent's vault sends without a card each; a person allows it on
//! one card; `--assign` sends it to one person; an agent never answers it.
//! Temp HOME, test DB, test vault; no real agent is woken.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Proof, WallCoverage};
use k2_core::permission::request::RequestState;
use serde_json::{json, Value};

use super::super::held::tests::{agent, answer_as, below_floor, card_of, id_of, login, row, seed, view, Ws};
use super::*;

fn env() -> (k2_core::db::TestDbGuard, crate::test_support::TempHome, std::sync::Arc<k2_vault::LocalVault>) {
    let db = k2_core::db::scoped_for_test();
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    (db, home, v)
}

/// The agent's ask, with `--to` / `--assign` when given.
fn ask_to(ws: &Ws, params: Value, to: Option<&str>) -> (String, Value) {
    let mut body = json!({ "action": ACTION, "params": params, "why": "The reports agents need the shared API keys sales sends" });
    if let Some(t) = to {
        body["to"] = json!(t);
    }
    let r = crate::permission::routes::handle_gated(crate::permission::routes::REQUEST, &agent(ws), true, body.to_string().as_bytes(), &HashMap::new());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn accepts(v: &k2_vault::LocalVault, receiver: &Ws, sender: &Ws) -> bool {
    v.accept_from(Some(&receiver.id)).expect("list").iter().any(|a| a.sender == format!("ws:{}", sender.id))
}

#[test]
fn it_is_registered_asked_only_with_a_click_for_an_owner_or_admin() {
    let _db = k2_core::db::scoped_for_test();
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.coverage), (Floor::Admin, Proof::Click, WallCoverage::None));
    assert!(crate::permission::present::find_not_cardable(ACTION).is_none());
}

#[test]
fn one_card_names_every_receiver_and_allow_lets_each_accept() {
    let (_db, _home, v) = env();
    let (sales, ops, ads) = (seed("vafs"), seed("vafo"), seed("vafa"));
    let receivers = format!("{},{}", ops.handle, ads.handle);
    let (s, out) = ask_to(&sales, json!({ "sender": sales.handle, "receivers": receivers }), None);
    assert_eq!((s.as_str(), out["status"].as_str()), ("200 OK", Some("pending")), "{out}");
    let card = card_of(&out);
    let id = id_of(&out);
    let vw = view(&sales, &card);
    assert_eq!(vw["whatChanges"], format!("Let {} and {} accept vault sends from {}.", ops.handle, ads.handle, sales.handle), "{vw}");
    assert_eq!(vw["setting"], format!("Accept vault sends from {}", sales.handle));
    assert_eq!(vw["proof"], json!({ "kind": "click" }));
    assert_eq!(vw["why"]["text"], "The reports agents need the shared API keys sales sends");

    // The agent can't answer its own card, Allow or Deny.
    let passport: HashMap<String, String> = [
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
        ("project_id".to_string(), sales.id.clone()),
        ("from".to_string(), sales.id.clone()),
    ]
    .into();
    for answer in ["Allow", "Deny"] {
        let body = json!({ "addr": sales.handle, "id": card, "answer": answer }).to_string();
        let r = crate::overlay_routes::dispatch_post("/cli/thread/answer", &passport, body.as_bytes());
        assert_eq!(r.status, "403 Forbidden", "{answer}: {}", r.body);
    }
    // A Member is below the floor.
    let mel = login("vafm", Role::Member);
    below_floor(answer_as(&mel, &sales, &card, json!({ "answer": "Allow" })), "admin");
    assert!(!accepts(&v, &ops, &sales) && !accepts(&v, &ads, &sales), "nothing changed yet");

    // An Admin allows it: each receiver accepts from sales, once.
    let ada = login("vafa", Role::Admin);
    let (s, a) = answer_as(&ada, &sales, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert!(accepts(&v, &ops, &sales) && accepts(&v, &ads, &sales), "both receivers accept from sales");
    let added: Vec<String> = v.accept_from(Some(&ops.id)).expect("list").iter().map(|e| e.added_by.clone()).collect();
    assert!(added.iter().any(|b| b.contains(&ada)), "the person who clicked is the one who added it: {added:?}");
    assert_eq!(row(&id).state, RequestState::Approved);

    // The agent sees where it stands.
    let p: HashMap<String, String> = [("id".to_string(), id.clone())].into();
    let r = crate::permission::routes::handle_gated(crate::permission::routes::STATUS, &agent(&sales), false, b"", &p);
    let st: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!((st["state"].as_str(), st["action"].as_str()), (Some("approved"), Some(ACTION)), "{st}");

    // Asked again: already done, nothing stored.
    let (s, again) = ask_to(&sales, json!({ "sender": sales.handle, "receivers": receivers }), None);
    assert_eq!((s.as_str(), again["status"].as_str()), ("200 OK", Some("already_done")), "{again}");
}

#[test]
fn assign_sends_the_card_to_that_person_and_refuses_one_who_cant_answer() {
    let (_db, _home, v) = env();
    let (sales, ops) = (seed("vags"), seed("vago"));
    let ada = login("vaga", Role::Admin);
    let mel = login("vagm", Role::Member);
    let params = json!({ "sender": sales.handle, "receivers": ops.handle });

    let (s, out) = ask_to(&sales, params.clone(), Some(&mel));
    assert_eq!((s.as_str(), out["error"]["code"].as_str()), ("400 Bad Request", Some("approver_not_eligible")), "{out}");
    let (s, out) = ask_to(&sales, params.clone(), Some("nobody-here"));
    assert_eq!((s.as_str(), out["error"]["code"].as_str()), ("400 Bad Request", Some("unknown_approver")), "{out}");
    assert!(crate::permission::store::open(Some(&sales.id)).expect("open").is_empty(), "a refused name asks nothing");

    let (s, out) = ask_to(&sales, params, Some(&ada));
    assert_eq!(s, "200 OK", "{out}");
    let q = row(&id_of(&out));
    assert_eq!(crate::permission::approvers::of(q.id.as_str()), Some(vec![ada.clone()]), "only ada is asked");
    // Deny changes nothing.
    let (s, a) = answer_as(&ada, &sales, &card_of(&out), json!({ "answer": "Deny" }));
    assert_eq!(s, "200 OK", "{a}");
    assert!(!accepts(&v, &ops, &sales));
}

#[test]
fn what_it_takes_is_checked_at_the_ask_and_at_the_click() {
    let (_db, _home, v) = env();
    let (sales, ops) = (seed("vacs"), seed("vaco"));
    let refused = |params: Value| {
        let (s, out) = ask_to(&sales, params, None);
        assert_eq!((s.as_str(), out["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{out}");
        out["error"]["hint"].as_str().expect("hint").to_string()
    };
    assert!(refused(json!({ "sender": sales.handle, "receivers": "nobody-here-xyz" })).contains("no agent named nobody-here-xyz"));
    assert!(refused(json!({ "sender": "nobody-here-xyz", "receivers": ops.handle })).contains("no agent named nobody-here-xyz"));
    assert!(refused(json!({ "sender": sales.handle, "receivers": sales.handle })).contains("accept from itself"));
    let many: Vec<String> = (0..=MAX_RECEIVERS).map(|i| format!("a{i}")).collect();
    assert!(refused(json!({ "sender": sales.handle, "receivers": many.join(",") })).contains("at most"));

    // The receivers swapped behind the card: stale at the click, nothing
    // changes for either.
    let other = seed("vacx");
    let (_, out) = ask_to(&sales, json!({ "sender": sales.handle, "receivers": ops.handle }), None);
    let id = id_of(&out);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n = conn
            .execute(
                "UPDATE permission_requests SET params = ?2 WHERE id = ?1",
                rusqlite::params![row(&id).id.as_str(), json!({ "sender": sales.handle, "receivers": other.handle }).to_string()],
            )
            .expect("swap");
        assert_eq!(n, 1);
    }
    let (s, a) = answer_as("owner", &sales, &card_of(&out), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Stale);
    assert!(!accepts(&v, &ops, &sales) && !accepts(&v, &other, &sales), "nothing changed");
}
