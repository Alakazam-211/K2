//! `sew.owner-handoff`: a Manage-Sew agent asks, only the Owner answers
//! (with the Settings confirm, bound to this request), and the one-time
//! link reaches the person who allowed it, never the agent.

use k2_core::connect_users::Role;
use k2_core::permission::request::RequestState;
use serde_json::json;

use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, last_turn, login, row, seed, view};
use super::test_seams::{APP_URL, CONFIRM};
use super::*;

const URL: &str = "https://chat.box.k2.dev";

/// Sew's app URL for this thread; cleared on drop.
struct Sew;
impl Sew {
    fn set() -> Sew {
        APP_URL.with(|c| *c.borrow_mut() = Some(URL.to_string()));
        Sew
    }
}
impl Drop for Sew {
    fn drop(&mut self) {
        APP_URL.with(|c| *c.borrow_mut() = None);
        CONFIRM.with(|c| *c.borrow_mut() = None);
    }
}

fn confirm(token: &str, request: &str, stamp: &str) {
    CONFIRM.with(|c| *c.borrow_mut() = Some((token.into(), request.into(), stamp.into())));
}

#[test]
fn the_card_is_registered_owner_only_with_the_settings_confirm_and_the_hc2_note() {
    let r = super::super::find("sew.owner-handoff").expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.spec.floor, Floor::Owner);
    assert_eq!(r.spec.proof, Proof::SettingsConfirm { action: ConfirmAction::ChatOwnerHandoff });
    assert_eq!(ConfirmAction::ChatOwnerHandoff.as_str(), crate::chat_pass::handoff::CONFIRM_ACTION, "the same confirm Settings takes");
    assert!(r.spec.words.consequences.contains("promoting someone to install owner or admin lifts their Sew bans"));
    assert!(crate::routes::route_policy::lookup(HANDOFF_ROUTE).and_then(|x| x.post).is_some(), "{HANDOFF_ROUTE} has a POST row");
}

#[test]
fn only_a_manage_sew_agent_asks_and_only_with_an_app_url() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("sewho");
    let (s, v) = ask(&ws, "sew.owner-handoff", json!({}));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("chat_manage_off")), "{v}");
    k2_core::workspace::settings::set_agents_can_manage_chat(&ws.id, true).unwrap();
    let (s, v) = ask(&ws, "sew.owner-handoff", json!({}));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "no Sew app URL yet: {v}");
    let _sew = Sew::set();
    ask_ok(&ws, "sew.owner-handoff", json!({}));
    // The agent itself never gets the handoff: the route refuses its passport.
    let agent = crate::vault::actor::VaultCaller {
        actor: k2_vault::Actor::Agent(k2_vault::AgentActor { workspace_id: ws.id.clone(), session_id: Some("s1".into()), shell: false, vault_manage: true }),
        channel: crate::vault::actor::Channel::Plain,
        session_id: Some("s1".into()),
    };
    let r = crate::chat_pass::mint::handle_owner_handoff_with(
        "loopback",
        Ok(agent),
        br#"{"confirmToken":"k2vc_x"}"#,
        crate::chat_pass::keyring::now(),
        &|_t: &str, _s: &str| Ok("local-auth".to_string()),
        &crate::chat_pass::ratelimit::HANDOFFS,
        &crate::chat_pass::handoff::CODES,
    );
    let body: serde_json::Value = serde_json::from_str(&r.response.body).unwrap();
    assert_eq!((r.response.status, &body["error"]["code"]), ("403 Forbidden", &json!("owner_only")), "{body}");
}

#[test]
fn the_owner_allows_and_the_link_goes_to_them_never_to_the_agent() {
    let _home = crate::test_support::TempHome::new();
    let _sew = Sew::set();
    let ws = seed("sewok");
    k2_core::workspace::settings::set_agents_can_manage_chat(&ws.id, true).unwrap();
    let v = ask_ok(&ws, "sew.owner-handoff", json!({}));
    let id = v["id"].as_str().expect("id").to_string();
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": "chat-owner-handoff", "secretId": id }));
    assert!(vw.to_string().contains("lifts their Sew bans"), "{vw}");

    // An Admin is below the floor; so is a Member.
    let admin = login("sewad", Role::Admin);
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": "k2vc_a" })), "owner");
    // No confirm, or a confirm for something else: nothing is minted.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    confirm("k2vc_owner", "perm_someone_else", "owner");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": "k2vc_owner" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "a confirm for another request: {a}");
    assert!(row(&id).is_answerable(), "nothing decided; the owner can confirm again");

    // Confirmed by the owner for this request: the link is theirs.
    confirm("k2vc_owner2", &id, "owner");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": "k2vc_owner2" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Approved);
    let open = a["forYou"]["openUrl"].as_str().unwrap_or_else(|| panic!("no link for the owner: {a}")).to_string();
    let code = open.strip_prefix(&format!("{URL}/k2/owner-handoff#code=")).unwrap_or_else(|| panic!("{open}")).to_string();
    assert!(code.starts_with("k2sho_"), "{open}");
    assert_eq!(a["forYou"]["expiresInMs"], json!(60_000));

    // The agent's turn, the card and the stored request never carry it.
    let turn = last_turn(&ws);
    assert!(turn.contains("approved") && turn.contains("never gets it"), "{turn}");
    for (what, text) in [("turn", turn), ("card", view(&ws, &card).to_string()), ("row", format!("{:?}", row(&id))), ("result", a["action"].to_string())] {
        assert!(!text.contains(&code) && !text.contains("owner-handoff#code"), "the {what} leaks the link: {text}");
    }
    // One-time: it redeems once.
    let now = crate::chat_pass::keyring::now();
    assert!(crate::chat_pass::handoff::CODES.redeem(&code, now).is_ok());
    assert!(crate::chat_pass::handoff::CODES.redeem(&code, now).is_err(), "single use");
}
