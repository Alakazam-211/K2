//! `chat.account` end to end: an agent asks for its own workspace's chat
//! account, the card says what it means, an Admin is below the route's
//! floor, the owner taps Allow and the card runs the grants route's own
//! handler as the owner (the workspace's `chat` grant, exactly what `k2 app
//! grant add --kind chat --subject-kind workspace` makes). Only its own
//! workspace. Test DB, temp HOME; no Sew (the chat target points at a
//! socket nobody serves; nothing here waits on it); no real agent is woken.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, ParamKind, Proof, Undo, WallMatch};
use k2_core::tool_hook::{self as hook, CHAT_KIND};
use serde_json::json;

use super::super::held::tests::{answer_as, ask, ask_ok, ask_refused, below_floor, card_of, id_of, last_turn, login, row, seed, view, Ws};
use super::*;

fn env() -> crate::test_support::TempHome {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    hook::allow_target_off_linux_for_tests(true);
    hook::set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").expect("chat target");
    home
}

/// The workspace's chat grant: `(scope, created_by)`.
fn chat_grant(ws: &Ws) -> Option<(String, String)> {
    k2_core::skin::list_grants()
        .expect("grants")
        .into_iter()
        .find(|g| g.kind == CHAT_KIND && g.subject_kind == "workspace" && g.subject_id.eq_ignore_ascii_case(&ws.id))
        .map(|g| (g.scope.expect("a chat grant has a scope"), g.created_by.expect("a writer")))
}

#[test]
fn the_card_is_registered_with_the_grants_routes_floor_and_its_three_walls() {
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Owner, Proof::Click, Grant::OneTime));
    assert!(matches!(r.spec.undo, Undo::None));
    let params: Vec<(&str, ParamKind, bool)> = r.spec.params.iter().map(|p| (p.key, p.kind, p.required)).collect();
    assert_eq!(params, [("scope", ParamKind::ChatScope, false)], "the workspace is never a value: it comes from the passport");
    let walls: Vec<(&str, &str)> = r
        .spec
        .walls
        .iter()
        .map(|w| match w.matches {
            WallMatch::Code { status: 403, code } => (w.route_prefix, code),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(walls, [(ROUTE, "owner_only"), (SPACE_CREATE, "chat_account_needed"), (CHAT_ROUTES, "not_granted")]);
    let floor = crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post).expect("a POST row");
    assert!(!floor.admits(Role::Admin), "the card's floor is the route's own (the owner)");
}

#[test]
fn the_owner_allows_a_coworker_account_once_and_the_grant_is_the_routes_own() {
    let _home = env();
    let ws = seed("cac");

    let asked = ask_ok(&ws, ACTION, json!({}));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("{}'s chat account", ws.handle), "{vw}");
    assert_eq!(vw["whatChanges"], format!("Give {} its own chat account, as a coworker.", ws.handle), "{vw}");
    let effects = vw["consequences"].as_str().expect("consequences");
    assert!(effects.contains("signs in to chat as itself, an agent"), "{effects}");
    assert!(effects.contains("k2 app grant rm --id"), "{effects}");

    // A Member and an Admin are below the route's floor; nothing changes.
    let member = login("cam", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "owner");
    let admin = login("caa", Role::Admin);
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow" })), "owner");
    let who = Answerer::Login { user: admin, role: "admin".into() };
    let ctx = crate::permission::present::ctx_of(&row(&id));
    assert!(matches!(apply(&ctx, &who, None), Applied::Refused { .. }), "the route's own floor at the click");
    assert_eq!(chat_grant(&ws), None, "nothing changed");

    // The owner: the grants route runs as the owner.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(chat_grant(&ws), Some(("coworker".to_string(), "owner-token".to_string())));
    let turn = last_turn(&ws);
    assert!(turn.contains(&format!("{} has its own chat account now, as a coworker; it stays until someone removes it.", ws.handle)), "{turn}");
    assert!(turn.contains("then run your command again"), "{turn}");

    // Once: a second answer applies nothing; asking again is already done.
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "409 Conflict");
    let (s, again) = ask(&ws, ACTION, json!({ "scope": "coworker" }));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
    // The other scope is the owner's to change.
    let hint = ask_refused(&ws, ACTION, json!({ "scope": "assistant:owner" }));
    assert!(hint.contains("already has a chat account, as a coworker"), "{hint}");
}

#[test]
fn an_assistant_account_names_the_person_it_speaks_for() {
    let _home = env();
    let ws = seed("caa");
    let dana = format!("dana{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let p = k2_core::skin::add_principal(&dana).expect("a person");

    let asked = ask_ok(&ws, ACTION, json!({ "scope": format!("assistant:{dana}") }));
    let card = card_of(&asked);
    let vw = view(&ws, &card);
    assert_eq!(vw["whatChanges"], format!("Give {} its own chat account, as {dana}'s assistant.", ws.handle), "{vw}");
    assert!(vw["consequences"].as_str().expect("consequences").contains("never a Space's manager"), "{vw}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert_eq!(chat_grant(&ws).map(|g| g.0), Some(format!("assistant:{}", p.id)));
    assert!(last_turn(&ws).contains(&format!("as {dana}'s assistant")), "{}", last_turn(&ws));
}

/// Only its own workspace, and only real scopes.
#[test]
fn an_agent_cant_ask_for_another_workspaces_account_or_a_scope_that_isnt_one() {
    let _home = env();
    let (ws, other) = (seed("cao"), seed("cap"));
    for params in [json!({ "workspace": other.handle }), json!({ "subject": other.handle }), json!({ "target": other.id })] {
        let (s, v) = ask(&ws, ACTION, params.clone());
        assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{params}: {v}");
    }
    let (_, v) = ask(&ws, ACTION, json!({ "workspace": other.handle }));
    assert!(v["error"]["hint"].as_str().expect("hint").contains("the workspace is always yours"), "{v}");
    assert!(ask_refused(&ws, ACTION, json!({ "scope": "boss" })).contains("scope=boss: coworker, or assistant:<person>"));
    assert!(ask_refused(&ws, ACTION, json!({ "scope": "assistant:nobody-here" })).contains("no person named nobody-here"));
    assert_eq!((chat_grant(&ws), chat_grant(&other)), (None, None), "nothing was granted");
}

/// With Chat / Voice Hosting off there is no account to give.
#[test]
fn no_card_while_chat_is_off() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = seed("caf");
    let (s, v) = ask(&ws, ACTION, json!({}));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("chat_unavailable")), "{v}");
}
