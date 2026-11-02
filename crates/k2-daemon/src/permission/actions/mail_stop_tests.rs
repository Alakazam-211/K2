//! `mail.stop-sending` end to end: an agent asks to turn off another
//! workspace's agent email sending (or its own), a Member can't answer, an
//! Admin taps Allow and the card runs Settings' own writer as the Admin;
//! the sending mode is off. Test DB, temp HOME; no real agent is woken.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Direction, Floor, Grant, ParamKind, Proof, Undo};
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

fn sending(ws: &Ws) -> String {
    k2_core::workspace::settings::mail_agent_send_for_path(&path_of(ws))
}

/// What Settings writes, as the owner.
fn set_sending(ws: &Ws, mode: &str) {
    let r = crate::mail_routes::mail_send_mode_entry(&Answerer::OwnerToken, &path_of(ws), mode);
    assert!(r.status.starts_with("200"), "{}: {}", r.status, r.body);
    assert_eq!(sending(ws), mode);
}

#[test]
fn the_card_is_registered_at_the_routes_admin_floor_and_only_tightens() {
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant, r.spec.direction), (Floor::Admin, Proof::Click, Grant::OneTime, Direction::Tighten));
    assert!(matches!(r.spec.undo, Undo::None));
    let params: Vec<(&str, ParamKind, bool)> = r.spec.params.iter().map(|p| (p.key, p.kind, p.required)).collect();
    assert_eq!(params, [("target", ParamKind::Workspace, true)]);
    assert_eq!(
        crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post),
        Some(crate::routes::route_policy::Floor::Admin),
        "{ROUTE}: the card's floor is the route's"
    );
    assert!(!ACTION.ends_with("-off"), "a walled action is never a twin");
}

#[test]
fn an_agent_asks_to_stop_another_workspaces_sending_and_an_admin_allows_it() {
    let _env = env();
    let (me, sales) = (seed("msa"), seed("msb"));
    set_sending(&sales, "on");

    let asked = ask_ok(&me, ACTION, json!({ "target": sales.handle }));
    let card = card_of(&asked);
    let vw = view(&me, &card);
    assert_eq!(vw["whatChanges"], format!("Stop {} sending email.", sales.handle), "{vw}");
    let effects = vw["consequences"].as_str().expect("consequences");
    assert!(effects.contains(&format!("{}'s agents can't send email", sales.handle)), "{effects}");
    assert!(effects.contains(&format!("{} gets no other access", me.handle)), "{effects}");

    // A Member is below the floor; nothing changes.
    let member = login("msm", Role::Member);
    below_floor(answer_as(&member, &me, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(sending(&sales), "on");

    // An Admin taps Allow: Settings' own writer runs as the Admin, once.
    let admin = login("msd", Role::Admin);
    let (s, a) = answer_as(&admin, &me, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(sending(&sales), "off");
    let turn = last_turn(&me);
    assert!(turn.contains(&format!("{}'s agents can't send email now", sales.handle)), "{turn}");

    // Asking again: already done.
    let (s, v) = ask(&me, ACTION, json!({ "target": sales.handle }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
}

#[test]
fn its_own_sending_too_by_any_of_its_names_and_only_real_workspaces() {
    let _env = env();
    let me = seed("msc");
    set_sending(&me, "approval");
    // By path: the card still names the workspace by its handle.
    let asked = ask_ok(&me, ACTION, json!({ "target": path_of(&me) }));
    let card = card_of(&asked);
    assert_eq!(view(&me, &card)["whatChanges"], format!("Stop {} sending email.", me.handle));
    let (s, a) = answer_as("owner", &me, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert_eq!(sending(&me), "off");

    assert!(ask_refused(&me, ACTION, json!({ "target": "no-such-ws-xyz" })).contains("workspace not found"));
    let (s, v) = ask(&me, ACTION, json!({}));
    assert!(s.starts_with("400") && v["error"]["hint"].as_str().expect("hint").contains("target="), "{s}: {v}");
}
