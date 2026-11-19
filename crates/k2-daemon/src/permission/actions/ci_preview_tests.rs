//! `ci.preview` / `ci.preview-off` (0.46.17): an agent asks the owner to
//! turn on CI on K2 Compute; the owner allows it once, through the settings
//! route's own entry, and exactly `ciPreview` changes.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Direction, Floor, Grant, Proof, Undo, WallCoverage};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, apply_as, ask, ask_ok, below_floor, card_of, id_of, login, refused_code, seed, view};
use super::super::{Answerer, Applied};
use super::*;

/// The whole stored settings, as the route echoes them (camelCase).
fn settings_json() -> Value {
    serde_json::to_value(k2_core::app_settings::load()).expect("settings serialize")
}

/// Every key but `ciPreview` is as it was.
fn only_ci_preview_moved(before: &Value, after: &Value) {
    let strip = |v: &Value| {
        let mut m = v.as_object().expect("settings object").clone();
        m.remove(KEY);
        m
    };
    assert_eq!(strip(before), strip(after), "the card changes ciPreview and nothing else");
}

#[test]
fn the_specs_are_the_owners_both_ways_with_a_twin() {
    for (id, dir) in [(ACTION, Direction::Loosen), (ACTION_OFF, Direction::Tighten)] {
        let r = super::super::find(id).unwrap_or_else(|| panic!("{id} is registered"));
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        // `ciPreview` is in settings_routes::OWNER_ONLY_KEYS: the Owner, never wider.
        assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant, r.spec.direction), (Floor::Owner, Proof::Click, Grant::Standing, dir), "{id}");
        assert_eq!(r.spec.words.setting, "CI on K2 Compute (preview)", "{id}");
        assert_eq!(r.spec.settings.page, "compute", "{id}");
    }
    assert_eq!(CI_PREVIEW.undo, Undo::Twin { action: ACTION_OFF });
    assert_eq!((CI_PREVIEW_OFF.undo, CI_PREVIEW_OFF.coverage, CI_PREVIEW_OFF.walls.len()), (Undo::None, WallCoverage::None, 0));
    assert_eq!(
        CI_PREVIEW.words.what_changes,
        "Turn on CI on K2 Compute: K2 checks every pushed commit on your compute machines"
    );
    assert!(CI_PREVIEW.words.consequences.contains("other jobs may wait longer"), "{}", CI_PREVIEW.words.consequences);
    // The other door: the settings route itself refuses Admins and Members.
    let _home = crate::test_support::TempHome::new();
    for actor in [crate::settings_routes::SettingsActor::Admin, crate::settings_routes::SettingsActor::Member] {
        let r = crate::settings_routes::handle_settings_update_as(br#"{"ciPreview":true}"#, actor, "x");
        assert!(r.status.starts_with("403"), "{actor:?} can't write ciPreview: {} {}", r.status, r.body);
    }
    assert!(!k2_core::app_settings::load().ci_preview);
}

#[test]
fn the_owner_allows_it_and_exactly_ci_preview_turns_on() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    assert!(!k2_core::app_settings::load().ci_preview, "off by default");

    let ws = seed("cip");
    let member = login("cipm", Role::Member);
    let admin = login("cipa", Role::Admin);
    let asked = ask_ok(&ws, ACTION, json!({}));
    let card = card_of(&asked);
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], "CI on K2 Compute (preview)", "{vw}");
    assert_eq!(vw["whatChanges"], "Turn on CI on K2 Compute: K2 checks every pushed commit on your compute machines.", "{vw}");
    assert!(vw["consequences"].as_str().unwrap_or("").contains(&format!("not only {}", ws.handle)), "{vw}");

    // A Member can't approve it; an Admin can't either (the key's floor).
    let before = settings_json();
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "owner");
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow" })), "owner");
    assert!(!k2_core::app_settings::load().ci_preview, "nothing changed below the floor");
    assert_eq!(settings_json(), before);

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let after = settings_json();
    assert_eq!(after[KEY], json!(true), "ciPreview is on: {after}");
    only_ci_preview_moved(&before, &after);

    // Asked again: already on, nothing to ask.
    let (s, again) = ask(&ws, ACTION, json!({}));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
}

#[test]
fn the_off_twin_turns_it_off_for_the_owner_only() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = seed("cipo");
    // Off already: the twin has nothing to do.
    let (s, v) = ask(&ws, ACTION_OFF, json!({}));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");

    let r = crate::settings_routes::handle_settings_update_as(br#"{"ciPreview":true}"#, crate::settings_routes::SettingsActor::Owner, "owner-token");
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    let before = settings_json();
    let asked = ask_ok(&ws, ACTION_OFF, json!({}));
    let card = card_of(&asked);
    assert_eq!(view(&ws, &card)["whatChanges"], "Turn off CI on K2 Compute on this server.");
    let admin = login("cipoa", Role::Admin);
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow" })), "owner");
    assert!(k2_core::app_settings::load().ci_preview, "an Admin turned nothing off");

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let after = settings_json();
    assert_eq!(after[KEY], json!(false), "{after}");
    only_ci_preview_moved(&before, &after);
}

/// At the click the route's own floor runs again with the login's
/// **current** role: a login whose stored role string says owner but who
/// is an Admin now is refused, and nothing is written.
#[test]
fn apply_runs_as_the_clickers_current_role() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = seed("cipr");
    let asked = ask_ok(&ws, ACTION, json!({}));
    let id = id_of(&asked);
    let admin = login("cipra", Role::Admin);
    let code = refused_code(apply_as(&id, &Answerer::Login { user: admin, role: "owner".into() }, None));
    assert!(!code.is_empty(), "refused");
    assert!(!k2_core::app_settings::load().ci_preview, "nothing written");
    let member = login("ciprm", Role::Member);
    refused_code(apply_as(&id, &Answerer::Login { user: member, role: "owner".into() }, None));
    assert!(!k2_core::app_settings::load().ci_preview, "nothing written");

    let owner = login("cipro", Role::Owner);
    match apply_as(&id, &Answerer::Login { user: owner, role: "owner".into() }, None) {
        Applied::Done { message } => assert!(message.starts_with("CI on K2 Compute is on for this server"), "{message}"),
        other => panic!("an Owner login applies it: {other:?}"),
    }
    assert!(k2_core::app_settings::load().ci_preview);
}
