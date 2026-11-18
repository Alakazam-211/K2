//! `permission.approvers`: an agent asks the owner to set who agents' cards
//! go to; the owner allows it once, through the settings route's own entry.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, Proof};
use serde_json::json;

use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, login, seed, view};
use super::*;

#[test]
fn the_card_is_the_owners_and_sets_who_agents_ask() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Owner, Proof::Click, Grant::Standing));

    let ws = seed("pap");
    let ada = login("papa", Role::Admin);
    let asked = ask_ok(&ws, ACTION, json!({ "to": format!("{ada},owner") }));
    let card = card_of(&asked);
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], "Who agents' permission cards ask", "{vw}");
    assert_eq!(
        vw["whatChanges"],
        format!("Send agents' permission cards to {ada}, the owner (this server's own login) when an agent names no one."),
        "{vw}"
    );
    assert!(vw["consequences"].as_str().unwrap_or("").contains("Now: every owner or admin who may answer."), "{vw}");

    // An Admin is below the key's floor; nothing changes.
    below_floor(answer_as(&ada, &ws, &card, json!({ "answer": "Allow" })), "owner");
    assert!(k2_core::app_settings::load().permission_approvers.is_empty());

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(k2_core::app_settings::load().permission_approvers, vec![ada.clone(), "owner".to_string()]);

    // The same list again: already done. `everyone` puts it back.
    let (s, again) = ask(&ws, ACTION, json!({ "to": format!("{ada},owner") }));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
    let asked = ask_ok(&ws, ACTION, json!({ "to": "everyone" }));
    let (s, a) = answer_as("owner", &ws, &card_of(&asked), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert!(k2_core::app_settings::load().permission_approvers.is_empty());

    // A name that isn't an owner or admin: refused at the ask.
    let mem = login("papm", Role::Member);
    for bad in [json!({ "to": mem }), json!({ "to": "nobody-here" }), json!({ "to": "everyone,owner" })] {
        let (s, e) = ask(&ws, ACTION, bad.clone());
        assert!(s.starts_with("400"), "{bad}: {s}: {e}");
    }
}
