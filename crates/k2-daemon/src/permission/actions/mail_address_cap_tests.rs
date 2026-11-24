//! `mail.address-cap` end to end: an agent asks a person to set or clear a
//! workspace's hosted mail address cap; a Member can't answer, an Admin taps
//! Allow and the card runs the config route as the Admin; the cap lands and
//! the agent's turn says what the workspace now has. Test DB, temp HOME; no
//! mail server, no real agent is woken. Hosted mail runs on Linux only:
//! elsewhere the card isn't offered.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Direction, Floor, Grant, ParamKind, Proof, Undo};
use k2_core::workspace::settings::{mail_address_cap_info_for_path, set_mail_manage_enabled, MailAddressCapSource};
use serde_json::json;

use super::super::held::tests::{answer_as, ask, ask_ok, ask_refused, below_floor, card_of, last_turn, login, seed, view, Ws};
use super::*;

struct Env {
    _home: crate::test_support::TempHome,
    /// The mail lock first, then the HOME lock (the order every mail test uses).
    _mail: crate::mail::MailServerTestGuard,
}

fn env() -> Env {
    let mail = crate::mail::mail_server_test_lock();
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    Env { _home: home, _mail: mail }
}

fn path_of(ws: &Ws) -> String {
    format!("/tmp/pc-e2-{}", ws.handle)
}

fn cap(ws: &Ws) -> (u32, MailAddressCapSource) {
    let i = mail_address_cap_info_for_path(&path_of(ws));
    (i.cap, i.source)
}

#[test]
fn the_card_is_registered_at_the_routes_admin_floor() {
    let _db = k2_core::db::scoped_for_test();
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant, r.spec.direction), (Floor::Admin, Proof::Click, Grant::OneTime, Direction::Loosen));
    assert!(matches!(r.spec.undo, Undo::None));
    let params: Vec<(&str, ParamKind, bool)> = r.spec.params.iter().map(|p| (p.key, p.kind, p.required)).collect();
    assert_eq!(params, [("target", ParamKind::Workspace, true), ("cap", ParamKind::Text, true)]);
    for route in [ROUTE, CREATE_ROUTE] {
        assert!(crate::routes::route_policy::lookup(route).and_then(|x| x.post).is_some(), "{route}: a classified POST");
    }
    assert_eq!(
        crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post),
        Some(crate::routes::route_policy::Floor::Admin),
        "{ROUTE}: the card's floor is the route's"
    );
}

/// The agent manages email hosting (no cap), the owner's cap is on it, and
/// it asks for more: an Admin allows; then it asks to clear the cap and the
/// owner allows. On a server where hosted mail can't run, it isn't offered.
#[test]
fn an_agent_asks_for_its_own_cap_and_an_admin_allows_it() {
    let _db = k2_core::db::scoped_for_test();
    let _env = env();
    let me = seed("mac");
    set_mail_manage_enabled(&path_of(&me), true).expect("manage on");
    k2_core::workspace::settings::update_project_setting(&path_of(&me), "mail_address_cap", "5").expect("owner cap 5");
    assert_eq!(cap(&me), (5, MailAddressCapSource::Set));

    if !crate::mail::supervisor::mail_supported() {
        let (s, v) = ask(&me, ACTION, json!({ "target": me.handle, "cap": "12" }));
        assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("not_available")), "{v}");
        return;
    }

    let asked = ask_ok(&me, ACTION, json!({ "target": me.handle, "cap": "12" }));
    let card = card_of(&asked);
    let vw = view(&me, &card);
    assert_eq!(vw["whatChanges"], format!("Let {} have up to 12 email addresses.", me.handle), "{vw}");
    let effects = vw["consequences"].as_str().expect("consequences");
    assert!(effects.starts_with("Today: Addresses: up to 5 (set by the owner for this workspace)."), "{effects}");
    assert!(effects.contains(&format!("Afterwards {}'s agents may create up to 12 email addresses", me.handle)), "{effects}");
    assert!(effects.contains("Send limits, mailbox sizes and the server's other limits stay as they are."), "{effects}");

    // A Member is below the floor; nothing changes.
    let member = login("macm", Role::Member);
    below_floor(answer_as(&member, &me, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(cap(&me), (5, MailAddressCapSource::Set));

    // An Admin taps Allow: the config route runs as the Admin, once.
    let admin = login("maca", Role::Admin);
    let (s, a) = answer_as(&admin, &me, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(cap(&me), (12, MailAddressCapSource::Set));
    let turn = last_turn(&me);
    assert!(turn.contains("Addresses: up to 12 (set by the owner for this workspace)"), "{turn}");

    // The same again: already done.
    let (s, v) = ask(&me, ACTION, json!({ "target": me.handle, "cap": "12" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");

    // Clear it: back to no limit, because it manages email hosting.
    let asked = ask_ok(&me, ACTION, json!({ "target": path_of(&me), "cap": "none" }));
    let card = card_of(&asked);
    assert_eq!(
        view(&me, &card)["whatChanges"],
        format!("Clear {}'s email address cap: it manages email hosting, so it has no limit.", me.handle)
    );
    let (s, a) = answer_as("owner", &me, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert_eq!(cap(&me), (0, MailAddressCapSource::ManagesEmailHosting));
    assert!(last_turn(&me).contains("Addresses: unlimited (this agent manages email hosting)"));
    let (s, v) = ask(&me, ACTION, json!({ "target": me.handle, "cap": "none" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
}

/// Another workspace without the switch: clearing goes back to the server
/// default, and the card says so.
#[test]
fn another_workspaces_cap_cleared_goes_back_to_the_default() {
    let _db = k2_core::db::scoped_for_test();
    let _env = env();
    let (me, sales) = (seed("mad"), seed("mae"));
    k2_core::workspace::settings::update_project_setting(&path_of(&sales), "mail_address_cap", "20").expect("cap 20");
    if !crate::mail::supervisor::mail_supported() {
        let (s, v) = ask(&me, ACTION, json!({ "target": sales.handle, "cap": "none" }));
        assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("not_available")), "{v}");
        return;
    }
    let asked = ask_ok(&me, ACTION, json!({ "target": sales.handle, "cap": "none" }));
    let card = card_of(&asked);
    assert_eq!(
        view(&me, &card)["whatChanges"],
        format!("Clear {}'s email address cap: back to the server default, up to 5 email addresses.", sales.handle)
    );
    let (s, a) = answer_as("owner", &me, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert_eq!(cap(&sales), (5, MailAddressCapSource::Default));
}

#[test]
fn bad_values_are_refused_when_asked() {
    let _db = k2_core::db::scoped_for_test();
    let _env = env();
    let me = seed("maf");
    if !crate::mail::supervisor::mail_supported() {
        let (s, v) = ask(&me, ACTION, json!({ "target": me.handle, "cap": "lots" }));
        assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("not_available")), "{v}");
        return;
    }
    for bad in ["lots", "-1", "1.5", "<cap>"] {
        let hint = ask_refused(&me, ACTION, json!({ "target": me.handle, "cap": bad }));
        assert!(hint.contains("whole number (0 = no limit) or none"), "{bad}: {hint}");
    }
    assert!(ask_refused(&me, ACTION, json!({ "target": "no-such-ws-xyz", "cap": "9" })).contains("workspace not found"));
    let (s, v) = ask(&me, ACTION, json!({ "target": me.handle }));
    assert!(s.starts_with("400") && v["error"]["hint"].as_str().expect("hint").contains("cap="), "{s}: {v}");
}
