//! `vault.owner-confirm` end to end (0.46.17): the owner's terminal raises
//! the phone card for its own vault confirm, an owner login answers it with
//! the existing confirm sheet, and the terminal gets its step's token. The
//! real routes throughout (`/cli/vault/confirm/*`, `/cli/thread/answer`,
//! `/cli/vault/reveal|grant`); this computer's own prompt is the phone
//! module's test seam, never a real one. Test vault, scoped DB, temp HOME.

use std::sync::Arc;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{ConfirmAction, Floor, Grant, Proof};
use k2_vault::{Actor, LocalVault};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, below_floor, card_status, login, row, seed, view, Ws};
use super::*;
use crate::vault::actor::{Channel, VaultCaller};
use crate::vault_routes::phone::test_seam::{self, Fake};
use crate::vault_routes::{self as vr, presence};

const VALUE: &str = "k2test-owner-confirm-not-real-8841";

fn owner() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Tty, session_id: None }
}

fn as_login(user: &str, role: k2_vault::Role) -> VaultCaller {
    VaultCaller { actor: Actor::Login { username: user.into(), role }, channel: Channel::Login, session_id: None }
}

fn call(v: &Arc<LocalVault>, path: &str, c: &VaultCaller, body: Value) -> (u16, Value) {
    call_p(v, path, c, body, Default::default())
}

fn call_p(v: &Arc<LocalVault>, path: &str, c: &VaultCaller, body: Value, params: std::collections::HashMap<String, String>) -> (u16, Value) {
    let r = vr::handle_on(v, path, Ok(c.clone()), body.to_string().as_bytes(), &params, "test", None);
    let s: u16 = r.status[..3].parse().expect("status");
    (s, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

struct Env {
    v: Arc<LocalVault>,
    ws: Ws,
    dir: String,
    _cards: k2_core::test_env::EnvVar,
}

/// A workspace with a Thread in a real folder, permission cards on.
fn env(tag: &str) -> Env {
    let v = crate::vault::ensure_test_vault();
    let ws = seed(tag);
    let dir = format!("/tmp/pc-e2-{}", ws.handle);
    std::fs::create_dir_all(format!("{dir}/sub")).expect("workspace folder");
    let cards = k2_core::test_env::EnvVar::set("K2_PERMISSION_CARDS", "1");
    Env { v, ws, dir, _cards: cards }
}

/// A server secret; returns (name, id).
fn secret(e: &Env, tag: &str) -> (String, String) {
    let name = format!("{tag}_{}", &uuid::Uuid::new_v4().simple().to_string()[..6].to_uppercase());
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, VALUE.as_bytes());
    let (s, r) = call(&e.v, vr::SET, &owner(), json!({ "name": name, "scope": "server", "hosts": ["api.example.com"], "valueB64": b64 }));
    assert_eq!(s, 200, "{r}");
    let params = [("name".to_string(), name.clone()), ("scope".to_string(), "server".to_string())].into_iter().collect();
    let (s, r) = call_p(&e.v, vr::SHOW, &owner(), Value::Null, params);
    assert_eq!(s, 200, "{r}");
    (name, r["secret"]["id"].as_str().expect("id").to_string())
}

/// The terminal raises the phone card for `body`'s step (from a folder
/// inside the workspace); returns (request, card, the start answer).
fn raise(e: &Env, mut body: Value) -> (String, String, Value) {
    body["phone"] = json!(true);
    body["workspacePath"] = json!(format!("{}/sub", e.dir));
    let (s, r) = call(&e.v, vr::CONFIRM_START, &owner(), body);
    assert_eq!(s, 200, "{r}");
    let p = &r["phone"];
    (p["request"].as_str().expect("request").to_string(), p["card"].as_str().expect("card").to_string(), r)
}

/// The terminal's finish, waiting on its own thread (as the vault socket
/// does), with this computer's own answer `local`.
fn wait(e: &Env, mut body: Value, request: &str, method: &str, local: Option<Fake>) -> std::thread::JoinHandle<((u16, Value), bool)> {
    body["phoneRequest"] = json!(request);
    body["method"] = json!(method);
    let v = e.v.clone();
    let db = k2_core::db::scoped_handle_for_test().expect("scoped db");
    std::thread::spawn(move || {
        k2_core::db::with_scoped_handle(db, || {
            test_seam::set(local);
            let _p = PresenceOffers::local();
            let out = call(&v, vr::CONFIRM_FINISH, &owner(), body);
            (out, test_seam::closed())
        })
    })
}

/// This computer offers the owner its own prompt (for the first-setup
/// steps), on the calling thread only.
struct PresenceOffers;

impl PresenceOffers {
    fn local() -> Self {
        presence::fake::set(Some(presence::fake::Fake { method: Some(presence::LOCAL_AUTH), approve: false }));
        PresenceOffers
    }
}

impl Drop for PresenceOffers {
    fn drop(&mut self) {
        presence::fake::set(None);
    }
}

/// The card's own confirm, from `who` (the phone's sheet).
fn card_token(e: &Env, who: &VaultCaller, request: &str, method: &str, password: &str) -> (u16, Value) {
    call(&e.v, vr::CONFIRM_FINISH, who, json!({ "action": phone::CARD_CONFIRM, "secretId": request, "method": method, "password": password }))
}

fn allow(e: &Env, user: &str, card: &str, token: &str) -> (String, Value) {
    answer_as(user, &e.ws, card, json!({ "answer": "Allow", "confirmToken": token }))
}

/// Wait until the card is up for answers (the terminal's finish is running).
fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(300));
}

#[test]
fn the_card_is_registered_owner_floor_with_its_own_confirm() {
    let _db = k2_core::db::scoped_for_test();
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.spec.floor, Floor::Owner);
    assert_eq!(r.spec.grant, Grant::OneTime);
    assert_eq!(r.spec.proof, Proof::SettingsConfirm { action: ConfirmAction::VaultOwnerConfirm });
    assert_eq!(ConfirmAction::VaultOwnerConfirm.as_str(), phone::CARD_CONFIRM);
    assert!(vr::phone::eligible("reveal") && vr::phone::eligible(vr::GRANT_READ_CONFIRM_ACTION) && vr::phone::eligible(presence::RECOVERY_SETUP));
    assert!(!vr::phone::eligible(presence::OWNER_LOGIN), "making a login an owner stays at the computer");
    assert!(!vr::phone::eligible(phone::CARD_CONFIRM));
    assert_eq!(r.spec.expiry.default_secs, phone::WINDOW_SECS);
}

/// The phone path is never weaker than the computer's: the vault password
/// never stands in where `presence` refuses it, and only an owner login's
/// password or the computer's own check confirm anything.
#[test]
fn the_phone_is_never_weaker_than_the_computer() {
    use vr::phone::strong_enough;
    for a in presence::VAULT_STEPS {
        assert!(strong_enough(a, presence::CONNECT_PASSWORD, false), "{a}");
        assert!(!strong_enough(a, "vault-password", false), "{a}: the vault password isn't this step's check");
        assert!(!strong_enough(a, "vault-password", true), "{a}");
        assert!(strong_enough(a, presence::LOCAL_AUTH, true), "{a}");
        assert!(!strong_enough(a, vr::phone::METHOD, false), "{a}: a phone token never confirms a card");
    }
    for a in ["reveal", vr::GRANT_READ_CONFIRM_ACTION, "rekey"] {
        assert!(strong_enough(a, presence::CONNECT_PASSWORD, false) && strong_enough(a, "vault-password", false), "{a}");
        assert!(!strong_enough(a, "", false) && !strong_enough(a, "click", false) && !strong_enough(a, vr::phone::METHOD, false), "{a}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_phone_answers_a_reveal_and_the_computer_prompt_closes() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let e = env("voc");
    let (name, id) = secret(&e, "STRIPE");
    let step = json!({ "action": "reveal", "secretId": id, "label": "something else entirely" });

    // Offered to the owner's terminal only.
    let (s, r) = call(&e.v, vr::CONFIRM_START, &owner(), json!({ "action": "reveal", "secretId": id }));
    assert_eq!((s, &r["phoneOffered"]), (200, &json!(true)), "{r}");
    let (s, r) = call(&e.v, vr::CONFIRM_START, &as_login("lee", k2_vault::Role::Owner), json!({ "action": "reveal", "secretId": id }));
    assert_eq!((s, &r["phoneOffered"]), (200, &json!(false)), "{r}");

    let (request, card, r) = raise(&e, step.clone());
    // The card names the step from the vault's own record, never the label.
    let text = r["phone"]["text"].as_str().expect("text");
    assert_eq!(text, format!("Show the value of {name} (server) in your terminal"), "{r}");
    let vw = view(&e.ws, &card);
    assert_eq!(vw["action"], ACTION, "{vw}");
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": phone::CARD_CONFIRM, "secretId": request }), "{vw}");
    assert_eq!(vw["approvers"]["floor"], "owner", "{vw}");
    assert!(!serde_json::to_string(&vw).unwrap().contains(VALUE), "never the value");
    let req = row(&request);
    assert_eq!((req.session_id.as_deref(), req.floor.as_str()), (Some(K2_SESSION), "owner"));

    // The terminal waits for the phone.
    let waiter = wait(&e, step.clone(), &request, vr::phone::METHOD, None);
    settle();
    let alice = login("alice", Role::Owner);
    let alice_c = as_login(&alice, k2_vault::Role::Owner);
    let (s, r) = call(&e.v, vr::CONFIRM_START, &alice_c, json!({ "action": phone::CARD_CONFIRM, "secretId": request }));
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["confirmText"], text, "{r}");
    assert!(r["methods"].as_array().expect("methods").contains(&json!("connect-password")), "{r}");
    let (s, r) = card_token(&e, &alice_c, &request, "connect-password", "wrong password!!");
    assert_eq!((s, code(&r)), (403, "confirm_failed"), "{r}");
    let (s, t) = card_token(&e, &alice_c, &request, "connect-password", "password123");
    assert_eq!(s, 200, "{t}");
    let (s, a) = allow(&e, &alice, &card, t["token"].as_str().expect("token"));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");

    let ((s, done), _) = waiter.join().expect("waiter");
    assert_eq!(s, 200, "{done}");
    assert_eq!(done["confirmedOn"], "phone", "{done}");
    let tok = done["token"].as_str().expect("token");
    let (s, r) = call(&e.v, vr::REVEAL, &owner(), json!({ "name": name, "scope": "server", "confirmToken": tok }));
    assert_eq!(s, 200, "{r}");
    let got = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, r["valueB64"].as_str().expect("value")).expect("b64");
    assert_eq!(got, VALUE.as_bytes());

    // One use: the token is spent, and the card releases nothing more.
    let (s, r) = call(&e.v, vr::REVEAL, &owner(), json!({ "name": name, "scope": "server", "confirmToken": tok }));
    assert_eq!((s, code(&r)), (403, "vault_confirm_required"), "{r}");
    let replay = wait(&e, step, &request, vr::phone::METHOD, None);
    let ((s, r), _) = replay.join().expect("replay");
    assert_eq!((s, code(&r)), (409, "phone_confirm_closed"), "{r}");
    assert_eq!(card_status(&e.ws, &card), "answered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finish_for_another_secret_is_refused_and_the_card_withdrawn() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let e = env("voa");
    let (_, a_id) = secret(&e, "ALPHA");
    let (_, b_id) = secret(&e, "BRAVO");
    let (request, card, _) = raise(&e, json!({ "action": "reveal", "secretId": a_id }));
    let w = wait(&e, json!({ "action": "reveal", "secretId": b_id }), &request, vr::phone::METHOD, None);
    let ((s, r), _) = w.join().expect("waiter");
    assert_eq!((s, code(&r)), (403, vr::phone::MISMATCH), "{r}");
    assert_eq!(card_status(&e.ws, &card), "voided", "the card is withdrawn");
    // Nothing waits for it any more: the phone gets nothing to confirm.
    let alice = login("alice", Role::Owner);
    let (s, r) = card_token(&e, &as_login(&alice, k2_vault::Role::Owner), &request, "connect-password", "password123");
    assert_eq!((s, code(&r)), (400, "bad_secret_id"), "{r}");
}

/// A share's card binds the share: the token works for that target and
/// delivery only, and an altered one burns it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_share_confirmed_on_the_phone_is_bound_to_its_target() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let e = env("vog");
    let (name, id) = secret(&e, "GRANT");
    let share = json!({ "name": name, "scope": "server", "to": "ws:*", "as": "env" });
    let step = json!({ "action": vr::GRANT_READ_CONFIRM_ACTION, "secretId": id, "grant": share });
    let (request, card, r) = raise(&e, step.clone());
    let text = r["phone"]["text"].as_str().expect("text");
    assert!(text.starts_with(&format!("Share {name} (server) with ")) && text.contains("as an environment variable named "), "{text}");
    let vw = view(&e.ws, &card);
    let values = serde_json::to_string(&vw["values"]).unwrap();
    assert!(values.contains("\"to\"") && values.contains("\"env_name\""), "{values}");

    let w = wait(&e, step, &request, vr::phone::METHOD, None);
    settle();
    let alice = login("alice", Role::Owner);
    let (_, t) = card_token(&e, &as_login(&alice, k2_vault::Role::Owner), &request, "connect-password", "password123");
    let (s, a) = allow(&e, &alice, &card, t["token"].as_str().expect("token"));
    assert_eq!(s, "200 OK", "{a}");
    let ((s, done), _) = w.join().expect("waiter");
    assert_eq!(s, 200, "{done}");
    let tok = done["token"].as_str().expect("token").to_string();
    assert!(test_seam::issued(&tok).is_some(), "the token carries the share");

    // Another delivery than the card named: refused, and the token is gone.
    let mut other = share.clone();
    other["as"] = json!("file");
    other["confirmToken"] = json!(tok);
    let (s, r) = call(&e.v, vr::GRANT, &owner(), other);
    assert_eq!((s, code(&r)), (403, vr::phone::MISMATCH), "{r}");
    let mut same = share.clone();
    same["confirmToken"] = json!(tok);
    let (s, r) = call(&e.v, vr::GRANT, &owner(), same);
    assert_eq!((s, code(&r)), (403, "vault_confirm_required"), "a burnt token shares nothing: {r}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_owner_answers_and_no_agent_asks() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let e = env("voo");
    let (_, id) = secret(&e, "OWNER");
    let (request, card, _) = raise(&e, json!({ "action": "reveal", "secretId": id }));
    let admin = login("ada", Role::Admin);
    let admin_c = as_login(&admin, k2_vault::Role::Admin);
    let (s, r) = call(&e.v, vr::CONFIRM_START, &admin_c, json!({ "action": phone::CARD_CONFIRM, "secretId": request }));
    assert_eq!((s, code(&r)), (403, "owner_only"), "{r}");
    let (s, r) = card_token(&e, &admin_c, &request, "connect-password", "password123");
    assert_eq!((s, code(&r)), (403, "owner_only"), "{r}");
    // Even with a token minted for them, the card's floor refuses an Admin.
    let forged = e.v.confirm.mint(phone::CARD_CONFIRM, Some(&request), &format!("connect:{admin}"), "connect-password");
    below_floor(allow(&e, &admin, &card, &forged), "owner");
    // A token minted for another login doesn't answer as this one.
    let alice = login("alice", Role::Owner);
    let bob = login("bob", Role::Owner);
    let (_, t) = card_token(&e, &as_login(&bob, k2_vault::Role::Owner), &request, "connect-password", "password123");
    let (s, a) = allow(&e, &alice, &card, t["token"].as_str().expect("token"));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert_eq!(card_status(&e.ws, &card), "pending", "a refused confirm decides nothing");
    // Allow with no confirm at all: refused, never a click.
    let (s, a) = answer_as(&alice, &e.ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    // An agent can't ask for the card.
    let (s, a) = ask(&e.ws, ACTION, json!({ "id": "vpc_0", "step": "Show the value of X" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("not_cardable")), "{a}");
    // An agent can't raise one either.
    let agent = VaultCaller {
        actor: Actor::Agent(k2_vault::AgentActor { workspace_id: e.ws.id.clone(), session_id: Some("s".into()), shell: false, vault_manage: true }),
        channel: Channel::Plain,
        session_id: Some("s".into()),
    };
    let (s, r) = call(&e.v, vr::CONFIRM_START, &agent, json!({ "action": "reveal", "secretId": id, "phone": true }));
    assert_eq!((s, code(&r)), (403, "owner_only"), "{r}");
}

/// A first-setup step takes only an owner login's own password on the
/// phone: the vault password is refused at the sheet and at the Allow.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_setup_step_takes_the_owner_logins_password_only() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let e = env("vop");
    let step = json!({ "action": presence::RECOVERY_SETUP });
    let (request, card, _) = {
        let _p = PresenceOffers::local();
        raise(&e, step.clone())
    };
    // Nobody answers at the computer; its prompt waits.
    let w = wait(&e, step, &request, presence::LOCAL_AUTH, Some(Fake::Wait));
    settle();
    let alice = login("alice", Role::Owner);
    let alice_c = as_login(&alice, k2_vault::Role::Owner);
    let (s, r) = call(&e.v, vr::CONFIRM_START, &alice_c, json!({ "action": phone::CARD_CONFIRM, "secretId": request }));
    assert_eq!((s, &r["methods"]), (200, &json!([presence::CONNECT_PASSWORD])), "{r}");
    let (s, r) = card_token(&e, &alice_c, &request, "vault-password", "correct horse battery");
    assert_eq!((s, code(&r)), (400, "bad_method"), "{r}");
    // A vault-password token for the card (minted any other way) still
    // releases nothing.
    let weak = e.v.confirm.mint(phone::CARD_CONFIRM, Some(&request), &format!("connect:{alice}"), "vault-password");
    let (s, a) = allow(&e, &alice, &card, &weak);
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert_eq!(card_status(&e.ws, &card), "pending");
    let (_, t) = card_token(&e, &alice_c, &request, "connect-password", "password123");
    let (s, a) = allow(&e, &alice, &card, t["token"].as_str().expect("token"));
    assert_eq!(s, "200 OK", "{a}");
    let ((s, done), closed) = w.join().expect("waiter");
    assert_eq!((s, &done["confirmedOn"]), (200, &json!("phone")), "{done}");
    assert!(closed, "the computer's prompt closed when the phone answered");
    // The step takes the released token.
    let tok = done["token"].as_str().expect("token");
    let proof = {
        let _p = PresenceOffers::local();
        presence::take(&e.v, &owner(), presence::RECOVERY_SETUP, None, Some(tok))
    };
    assert!(proof.is_ok(), "{proof:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_answer_wins() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let e = env("vof");
    let step = json!({ "action": presence::RECOVERY_SETUP });
    // The computer answers first: the card is withdrawn, and a later Allow
    // on the phone releases nothing.
    let (request, card, _) = {
        let _p = PresenceOffers::local();
        raise(&e, step.clone())
    };
    let alice = login("alice", Role::Owner);
    let alice_c = as_login(&alice, k2_vault::Role::Owner);
    let (_, t) = card_token(&e, &alice_c, &request, "connect-password", "password123");
    let w = wait(&e, step.clone(), &request, presence::LOCAL_AUTH, Some(Fake::Approve));
    let ((s, done), _) = w.join().expect("waiter");
    assert_eq!((s, &done["confirmedOn"]), (200, &json!("computer")), "{done}");
    assert_eq!(card_status(&e.ws, &card), "voided");
    let (s, a) = allow(&e, &alice, &card, t["token"].as_str().expect("token"));
    assert_ne!(s, "200 OK", "{a}");

    // Declined at the computer first: the terminal fails, the card goes.
    let (request, card, _) = {
        let _p = PresenceOffers::local();
        raise(&e, step.clone())
    };
    let w = wait(&e, step.clone(), &request, presence::LOCAL_AUTH, Some(Fake::Decline));
    let ((s, r), _) = w.join().expect("waiter");
    assert_eq!((s, code(&r)), (403, "confirm_failed"), "{r}");
    assert_eq!(card_status(&e.ws, &card), "voided");

    // Denied on the phone first: the terminal stops at once.
    let (request, card, _) = {
        let _p = PresenceOffers::local();
        raise(&e, step.clone())
    };
    let w = wait(&e, step, &request, presence::LOCAL_AUTH, Some(Fake::Wait));
    settle();
    let (s, a) = answer_as(&alice, &e.ws, &card, json!({ "answer": "Deny" }));
    assert_eq!(s, "200 OK", "{a}");
    let ((s, r), closed) = w.join().expect("waiter");
    assert_eq!((s, code(&r)), (403, "phone_confirm_denied"), "{r}");
    assert!(closed, "the computer's prompt closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_card_is_refused() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let e = env("voe");
    let (_, id) = secret(&e, "LATE");
    let step = json!({ "action": "reveal", "secretId": id });
    let (request, card, _) = raise(&e, step.clone());
    let alice = login("alice", Role::Owner);
    let alice_c = as_login(&alice, k2_vault::Role::Owner);
    let (_, t) = card_token(&e, &alice_c, &request, "connect-password", "password123");
    let w = wait(&e, step, &request, vr::phone::METHOD, None);
    settle();
    test_seam::expire(&request);
    let ((s, r), _) = w.join().expect("waiter");
    assert_eq!((s, code(&r)), (409, "phone_confirm_expired"), "{r}");
    assert_eq!(card_status(&e.ws, &card), "expired");
    let (s, a) = allow(&e, &alice, &card, t["token"].as_str().expect("token"));
    assert_ne!(s, "200 OK", "an expired card releases nothing: {a}");
    let (s, r) = card_token(&e, &alice_c, &request, "connect-password", "password123");
    assert_eq!((s, code(&r)), (400, "bad_secret_id"), "{r}");
}
