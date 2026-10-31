//! `apps.chat-key` and `sew.space-governance` end to end: an App's own agent
//! asks, the card says what changes, the wrong people can't answer, the
//! owner (or an Admin within the key's rules) allows it with a confirm bound
//! to the request, and the card runs the route's own entry as that person.
//! Test vault (fake key store), test DB, temp HOME; no Sew (the chat target
//! points at a socket nobody serves; nothing here waits on it); no real agent
//! is woken (turns go to `inject`'s sink).

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{ConfirmAction, Floor, Grant, Proof, Undo};
use k2_core::skin::app_chat_key as ck;
use k2_core::tool_hook::{self as hook, CHAT_KIND};
use serde_json::json;

use super::super::held::tests::{answer_as, ask, ask_ok, ask_refused, below_floor, card_of, id_of, last_turn, login, row, seed, view, Ws};
use super::*;

struct Env {
    _home: crate::test_support::TempHome,
    vault: std::sync::Arc<k2_vault::LocalVault>,
}

fn env() -> Env {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let vault = crate::vault::ensure_test_vault();
    hook::allow_target_off_linux_for_tests(true);
    hook::set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").expect("chat target");
    Env { _home: home, vault }
}

fn flag() {
    hook::note_features_for_tests(&[hook::features::APP_GOVERNED_SPACES]).expect("flag");
}

fn uniq() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

/// A `--cmd` App of `ws` (no approved manifest chat caps: only the owner
/// names its key's caps).
fn app_of(ws: &Ws) -> (String, String) {
    let name = format!("jobs{}", uniq());
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = k2_core::published_services::insert(&conn, &ws.id, &name, "node s.js", "/tmp", 47_601, "local", "stopped", "cmd", "").expect("App").id;
    (name, id)
}

fn confirm_for(e: &Env, action: &str, id: &str, stamp: &str) -> String {
    e.vault.confirm.mint(action, Some(id), stamp, "local-auth")
}

#[test]
fn both_cards_are_registered_with_the_routes_floor_a_confirm_and_their_walls() {
    let k = super::super::find(KEY_ACTION).expect("apps.chat-key");
    let g = super::super::find(GOV_ACTION).expect("sew.space-governance");
    for (r, confirm, route) in [(&k, ConfirmAction::AppsChatKey, KEY_ROUTE), (&g, ConfirmAction::SewSpaceGovernance, GOV_ROUTE)] {
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::SettingsConfirm { action: confirm }, Grant::OneTime), "{}", r.spec.id);
        assert!(matches!(r.spec.undo, Undo::None));
        assert_eq!(r.spec.walls.len(), 1);
        assert_eq!(r.spec.walls[0].route_prefix, route);
        assert_eq!(
            crate::routes::route_policy::lookup(route).and_then(|x| x.post),
            Some(crate::routes::route_policy::Floor::Admin),
            "{route}: the card's floor is the route's"
        );
    }
    assert_eq!((ConfirmAction::AppsChatKey.as_str(), ConfirmAction::SewSpaceGovernance.as_str()), (KEY_CONFIRM, GOV_CONFIRM));
    assert_eq!(k.spec.params.iter().map(|p| (p.key, p.kind)).collect::<Vec<_>>()[2], ("caps", k2_core::permission::catalog::ParamKind::ChatCaps));
    assert_eq!(g.spec.params.iter().map(|p| (p.key, p.kind)).collect::<Vec<_>>()[1], ("space", k2_core::permission::catalog::ParamKind::SewSpace));
}

/// Only its own App, real values, and only something that turns the key on
/// or widens it; a Sew without the flag can't have one.
#[test]
fn a_chat_key_ask_is_its_own_apps_and_checked_against_the_routes_rules() {
    let _e = env();
    let ws = seed("ckq");
    let other = seed("cko");
    let (name, _) = app_of(&ws);
    let (theirs, _) = app_of(&other);
    let (s, v) = ask(&ws, KEY_ACTION, json!({ "app": name, "caps": "chat:read" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("sew_feature_missing")), "{v}");
    flag();
    let (s, v) = ask(&ws, KEY_ACTION, json!({ "app": theirs, "caps": "chat:read" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "another workspace's App: {v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains(&format!("has no App named {theirs}")), "{v}");
    for (bad, why) in [
        (json!({ "app": name, "caps": "chat:read,files:read" }), "a cap that isn't a chat cap"),
        (json!({ "app": name, "caps": "chat:read", "level": "root" }), "a level that doesn't exist"),
        (json!({ "app": name, "caps": "chat:read", "uploads": "maybe" }), "uploads on or off"),
        (json!({ "app": name }), "a --cmd service's manifest names no caps: the ask names them"),
    ] {
        let (s, v) = ask(&ws, KEY_ACTION, bad);
        assert_eq!(s, "400 Bad Request", "{why}: {v}");
    }
    // A key that already allows it: nothing to widen.
    let (_, svc) = app_of(&ws);
    hook::put_chat_key_for_tests(&svc, "moderator", 500).expect("key");
    let (s, v) = ask(&ws, KEY_ACTION, json!({ "app": svc, "level": "member" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "narrowing is the agent's own: {v}");
}

/// The whole chat-key card: words, who answers, the confirm, the Admin
/// ceiling, once only.
#[test]
fn the_owner_turns_the_key_on_from_the_card_and_an_admin_past_the_ceiling_leaves_it_open() {
    let e = env();
    flag();
    let ws = seed("ckc");
    let (name, svc) = app_of(&ws);
    let asked = ask_ok(&ws, KEY_ACTION, json!({ "app": name, "caps": "chat:read,chat:post,voice:join", "level": "admin" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("{name}'s chat key"), "{vw}");
    assert_eq!(
        vw["whatChanges"],
        format!(
            "Give {}'s App {name} a chat key, so K2 signs chat passes for its users: people may read chat, post and join calls, up to level admin, up to 500 people at once, uploads on.",
            ws.handle
        ),
        "{vw}"
    );
    let cons = vw["consequences"].as_str().unwrap();
    assert!(cons.contains("Only the server's owner can allow this one"), "level admin is the owner's: {cons}");
    assert!(cons.contains(&format!("k2 app grant {name} --kind chat-key --revoke")), "{cons}");
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": KEY_CONFIRM, "secretId": id }), "{vw}");
    assert_eq!(vw["grant"], "one-time");
    // The sheet's words come from the stored values.
    assert!(confirm_reason(KEY_CONFIRM, Some(&id)).expect("ours").expect("open").starts_with(&format!("Give {}'s App {name} a chat key", ws.handle)));
    assert!(matches!(confirm_reason(KEY_CONFIRM, Some("pr_nope")), Some(Err(_))));
    assert_eq!(confirm_reason("chat-key", Some(&id)), None, "the route's own confirm is not the card's");

    // A Member is below the floor.
    let member = login("ckm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&e, KEY_CONFIRM, &id, &format!("connect:{member}")) })), "admin");
    // An Admin, confirmed: past the Admin ceiling, so the card stays open
    // for the owner and the confirm isn't spent.
    let admin = login("cka", Role::Admin);
    let tok = confirm_for(&e, KEY_CONFIRM, &id, &format!("connect:{admin}"));
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": tok }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!(OWNER_IN_PERSON)), "{a}");
    assert!(row(&id).is_answerable(), "still open for the owner");
    assert_eq!(ck::get(&svc).unwrap(), None, "no key yet");
    // The owner without a confirm, or with another request's: nothing.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&e, KEY_CONFIRM, "pr_other", "owner") }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    // The route's own confirm isn't this card's proof either.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&e, "chat-key", &svc, "owner") }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert_eq!(ck::get(&svc).unwrap(), None);

    // The owner, confirmed for this request.
    let token = confirm_for(&e, KEY_CONFIRM, &id, "owner");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": token }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert!(e.vault.confirm.take(&token, KEY_CONFIRM, Some(&id), "owner").is_err(), "the Allow spent the token");
    let k = ck::get(&svc).unwrap().expect("a key");
    assert!(k.enabled, "{k:?}");
    assert_eq!(k.caps, vec!["chat:read", "chat:post", "voice:join"], "{k:?}");
    assert_eq!((k.max_level.as_str(), k.max_members, k.granted_by.as_str()), ("admin", 500, "owner"), "the route ran as the owner");
    let turn = last_turn(&ws);
    assert!(turn.contains("approved by owner") && turn.contains(&format!("{name} holds a chat key now")), "{turn}");
    // Once: a second answer applies nothing; asking again is already done.
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&e, KEY_CONFIRM, &id, "owner") }));
    assert_eq!(s, "409 Conflict");
    let (s, again) = ask(&ws, KEY_ACTION, json!({ "app": name, "caps": "chat:read,chat:post,voice:join", "level": "admin" }));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
}

/// A Space bound to the asking workspace's App: the owner switches it to
/// App-governed from the card, through the route, as itself.
#[test]
fn the_owner_switches_its_apps_space_to_app_governed_from_the_card() {
    let e = env();
    flag();
    let ws = seed("sgv");
    let (name, svc) = app_of(&ws);
    let space = format!("70{}", &uuid::Uuid::new_v4().as_u128().to_string()[..6]);
    hook::app_space_bind(&svc, &space, "owner-token").expect("bind");
    // Unbound, or another App's Space: refused when asked.
    let msg = ask_refused(&ws, GOV_ACTION, json!({ "app": name, "space": "9999", "to": "app" }));
    assert!(msg.contains("isn't a Space bound to"), "{msg}");
    // No chat key yet: an App governs only with one; the hint says how to ask.
    let msg = ask_refused(&ws, GOV_ACTION, json!({ "app": name, "space": space, "to": "app" }));
    assert!(msg.contains(&format!("k2 permission request apps.chat-key app={name}")), "{msg}");
    // Back to Sew when it already is: nothing to do.
    let (s, v) = ask(&ws, GOV_ACTION, json!({ "app": name, "space": space, "to": "sew" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");

    hook::put_chat_key_for_tests(&svc, "moderator", 500).expect("key");
    let asked = ask_ok(&ws, GOV_ACTION, json!({ "app": name, "space": space, "to": "app" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("who governs chat Space {space}"), "{vw}");
    assert!(vw["whatChanges"].as_str().unwrap().starts_with(&format!("Let {}'s App {name} govern the chat Space {space}: the App decides", ws.handle)), "{vw}");
    assert!(vw["consequences"].as_str().unwrap().contains("Box users in it (the owner, admins) leave the Space first"), "{vw}");
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": GOV_CONFIRM, "secretId": id }), "{vw}");

    let member = login("sgm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&e, GOV_CONFIRM, &id, &format!("connect:{member}")) })), "admin");
    // The route's own confirm is not the card's.
    let route_tok = confirm_for(&e, crate::sew::governed::CONFIRM_GOVERNANCE, &crate::sew::governed::governance_secret_id(&svc, &space, "app"), "owner");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": route_tok }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");

    // An Admin allows it, as itself (the route admits Admins).
    let admin = login("sga", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&e, GOV_CONFIRM, &id, &format!("connect:{admin}")) }));
    assert_eq!(s, "200 OK", "{a}");
    let view_now = hook::app_spaces().unwrap().into_iter().find(|v| v.binding.space_id == space).expect("bound");
    assert_eq!(view_now.governance.as_deref(), Some("app"), "the route ran");
    let turn = last_turn(&ws);
    assert!(turn.contains(&format!("Chat Space {space} is App-governed now")), "{turn}");
    let (s, v) = ask(&ws, GOV_ACTION, json!({ "app": name, "space": space, "to": "app" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
    // Back again: its own card, its own confirm.
    let back = ask_ok(&ws, GOV_ACTION, json!({ "app": name, "space": space, "to": "sew" }));
    let (bid, bcard) = (id_of(&back), card_of(&back));
    assert!(view(&ws, &bcard)["whatChanges"].as_str().unwrap().starts_with(&format!("Give the chat Space {space} back to Sew's own rules")));
    let (s, a) = answer_as("owner", &ws, &bcard, json!({ "answer": "Allow", "confirmToken": confirm_for(&e, GOV_CONFIRM, &bid, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    let view_now = hook::app_spaces().unwrap().into_iter().find(|v| v.binding.space_id == space).expect("bound");
    assert_eq!(view_now.governance.as_deref(), Some("sew"));
}
