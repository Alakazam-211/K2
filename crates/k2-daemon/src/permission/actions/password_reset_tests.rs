//! `apps.user-password-reset` end to end (0.46.6): an App's agent asks
//! through `/cli/permission/request`, a person answers through `POST
//! /cli/thread/answer`, and the routes' own entries reset the person as
//! that person. The secret reaches the answer's `forYou` only: never the
//! card, the Thread, the stored request, the agent's turn, the audit or
//! the daemon log (stderr, read back here). Temp HOME, test DB, fake mail
//! engine (`email_notify::tests::fx`); no real mail, agent, vault or
//! keychain.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Grant, Proof, Undo};
use k2_core::permission::request::RequestState;
use k2_core::skin;
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, id_of, last_turn, login, row, seed, view, Ws};
use super::*;
use crate::compute_routes::Who;
use crate::permission::routes;

const OLD_PW: &str = "old-pass-for-the-person-1";
const DOOR: &str = "https://box.brand.test";

struct Env {
    f: crate::email_notify::tests::Fx,
    ws: Ws,
    app: String,
    app_name: String,
}

fn uniq() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

/// A workspace with a pinned Thread and an App of its own; K2 has a front
/// door page and a system sender (so `email` works for a trusted email).
fn env(tag: &str) -> Env {
    let f = crate::email_notify::tests::fx(tag);
    let ws = seed(tag);
    let app_name = format!("shop{}", uniq());
    let app = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &ws.id, &app_name, "node s.js", "/tmp", 4377, "local", "running", "cmd", "").expect("App").id
    };
    skin::set_front_door("direct", Some(DOOR), None, None).expect("front door");
    crate::email_notify::tests::system_sender_on();
    Env { f, ws, app, app_name }
}

/// An App user of `app` the owner made, with a password and `email`.
fn person(app: &str, name: &str, email: Option<&str>) -> (String, String) {
    let u = format!("{name}{}", uniq());
    let p = skin::add_principal_for_app(&u, email, app, None, "owner-token").expect("add");
    skin::set_principal_password(&u, Some(OLD_PW)).expect("password");
    (u, p.id)
}

fn allow(body: Value) -> Value {
    let mut b = json!({ "answer": "Allow" });
    for (k, v) in body.as_object().expect("object") {
        b[k] = v.clone();
    }
    b
}

/// The daemon log (stderr) while `f` runs, read back from a file.
#[cfg(unix)]
fn with_log<T>(f: impl FnOnce() -> T) -> (T, String) {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    let path = std::env::temp_dir().join(format!("k2-prc-log-{}", uuid::Uuid::new_v4().simple()));
    let file = std::fs::File::create(&path).expect("log file");
    let _ = std::io::stderr().flush();
    // SAFETY: plain fd juggling on this process's own stderr; restored below.
    let saved = unsafe { libc::dup(2) };
    assert!(saved >= 0, "dup stderr");
    assert!(unsafe { libc::dup2(file.as_raw_fd(), 2) } >= 0, "redirect stderr");
    let out = f();
    let _ = std::io::stderr().flush();
    unsafe {
        libc::dup2(saved, 2);
        libc::close(saved);
    }
    let log = std::fs::read_to_string(&path).expect("read log");
    let _ = std::fs::remove_file(&path);
    (out, log)
}

/// Everything the agent and the stored side can see of a request.
fn stored_text(ws: &Ws, card: &str, id: &str) -> Vec<(&'static str, String)> {
    let turns = crate::permission::inject::sent_to(&ws.handle).join("\n");
    let doc = serde_json::to_string(&k2_core::overlay::get_thread_doc(&ws.conv, card).expect("read").expect("doc")).expect("doc json");
    let audit = k2_core::auth_audit::read_all().expect("audit").iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    vec![("turns", turns), ("card doc", doc), ("row", format!("{:?}", row(id))), ("audit", audit)]
}

fn assert_nowhere(secret: &str, places: &[(&'static str, String)]) {
    for (what, text) in places {
        assert!(!text.contains(secret), "the {what} holds the secret: {text}");
    }
}

#[test]
fn the_card_is_registered_with_the_routes_floor_a_click_and_one_time() {
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (FLOOR, Proof::Click, Grant::OneTime));
    assert!(matches!(r.spec.undo, Undo::None));
    assert_eq!((r.spec.expiry.default_secs, r.spec.expiry.max_secs), (24 * 3600, 3 * 24 * 3600));
    assert_eq!(r.spec.params.iter().find(|p| p.key == "mode").expect("mode").choices, MODES);
    assert_eq!(DEFAULT_MODE, "password", "Rosson 2026-10-29");
    // Parity (PR-A2): the card's floor is every route's floor it calls.
    for route in [ROUTE_PASSWORD, ROUTE_UNLOCK, ROUTE_RESET_LINK] {
        let floor = crate::routes::route_policy::lookup(route).and_then(|x| x.post).unwrap_or_else(|| panic!("{route} has a POST row"));
        assert_eq!(floor.as_wire(), FLOOR.as_str(), "{route}");
    }
    // Never in a batch's "Allow all", and never batched.
    assert!(!crate::permission::present::batchable(&r));
    assert!(is_reveal(ACTION) && !is_reveal("compute.use"));
}

#[test]
fn only_an_agent_of_the_apps_workspace_asks_for_its_own_apps_users() {
    let e = env("prask");
    let (u, _) = person(&e.app, "ann", Some("ann@x.test"));
    // No passport (the owner, a login): refused, no card.
    let body = json!({ "action": ACTION, "params": { "user": u }, "why": "locked out" }).to_string();
    for who in [Who::Owner, Who::Login { username: "rosson".into(), role: Role::Owner }] {
        let r = routes::handle_gated(routes::REQUEST, &who, true, body.as_bytes(), &HashMap::new());
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    }
    // Another workspace's agent: not its App.
    let other = seed("prother");
    let (s, v) = ask(&other, ACTION, json!({ "user": u }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("holds no App of"), "{v}");
    let (s, v) = ask(&other, ACTION, json!({ "user": u, "app": e.app }));
    assert_eq!(s, "400 Bad Request", "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("isn't an App of"), "{v}");
    // Someone without a grant on the App, and nobody at all.
    let (stranger, _) = {
        let n = format!("zed{}", uniq());
        let p = skin::add_principal_with_email(&n, Some("zed@x.test")).expect("add");
        (n, p.id)
    };
    let (s, v) = ask(&e.ws, ACTION, json!({ "user": stranger, "app": e.app }));
    assert_eq!(s, "400 Bad Request", "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("has no access to"), "{v}");
    let (s, _) = ask(&e.ws, ACTION, json!({ "user": "nobody-here" }));
    assert_eq!(s, "400 Bad Request");
    // The App's own agent: a card, K2 picking the App.
    let v = ask_ok(&e.ws, ACTION, json!({ "user": u }));
    let vw = view(&e.ws, &card_of(&v));
    assert!(vw["setting"].as_str().unwrap().contains(&e.app_name), "{vw}");
    assert_eq!(vw["approvers"]["floor"], FLOOR.as_str(), "{vw}");
    let _ = &e.f;
}

#[cfg(unix)]
#[test]
fn password_mode_shows_the_approver_a_password_once_and_nothing_else_ever_holds_it() {
    let e = env("prpw");
    let (u, _pid) = person(&e.app, "ben", Some("ben@x.test"));
    // Locked out (three wrong tries) and signed in somewhere.
    let session = skin::create_app_session_token(&u, &e.app).expect("session").raw;
    for _ in 0..3 {
        let _ = skin::check_and_record_login(&u, "wrong-guess-123");
    }
    assert!(matches!(skin::check_and_record_login(&u, OLD_PW), skin::SkinLoginOutcome::LockedOut { .. }), "locked");

    let (secret, log) = with_log(|| {
        let v = ask_ok(&e.ws, ACTION, json!({ "user": u }));
        let (id, card) = (id_of(&v), card_of(&v));
        let vw = view(&e.ws, &card);
        assert_eq!(vw["reveal"], json!({ "mode": "password", "emailOffered": true }), "{vw}");
        assert!(vw["whatChanges"].as_str().unwrap().contains("never sees it"), "{vw}");

        // An Admin can't approve while the floor is Owner.
        if FLOOR == k2_core::permission::catalog::Floor::Owner {
            let admin = login("pradm", Role::Admin);
            below_floor(answer_as(&admin, &e.ws, &card, allow(json!({ "reveal": 1, "client": "desktop" }))), "owner");
        }
        // An old client (no `reveal`), the phone and a terminal: refused,
        // nothing decided, nothing changed.
        for body in [json!({}), json!({ "reveal": 1, "client": "phone" }), json!({ "reveal": "1", "client": "terminal" }), json!({ "client": "desktop" })] {
            let (s, a) = answer_as("owner", &e.ws, &card, allow(body.clone()));
            assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!(REVEAL_NEEDS_SCREEN)), "{body}: {a}");
            assert!(a.get("forYou").is_none(), "{a}");
            assert!(row(&id).is_answerable(), "{body}: still open");
            assert!(skin::verify_principal_password(&u, OLD_PW), "{body}: the old password stands");
        }
        // A mode the card doesn't have.
        let (s, a) = answer_as("owner", &e.ws, &card, allow(json!({ "reveal": 1, "mode": "sms" })));
        assert_eq!(s, "400 Bad Request", "{a}");

        // The owner, on a client that shows it.
        let (s, a) = answer_as("owner", &e.ws, &card, allow(json!({ "reveal": 1, "client": "desktop" })));
        assert_eq!(s, "200 OK", "{a}");
        let f = &a["forYou"];
        assert_eq!((&f["kind"], &f["mode"], &f["username"], &f["app"]), (&json!(ACTION), &json!("password"), &json!(u), &json!(e.app_name)), "{a}");
        assert_eq!(f["revealSecs"], json!(REVEAL_SECS));
        let secret = f["secret"].as_str().expect("secret").to_string();
        // The shared generator's shape (k2-core `generate_password`).
        assert_eq!(secret.chars().filter(|c| *c != '-').count(), skin::GENERATED_PASSWORD_CHARS, "{secret}");
        assert!(secret.chars().all(|c| c == '-' || c.is_ascii_alphanumeric()), "{secret}");
        assert!(!secret.contains(['0', 'O', '1', 'l', 'I']), "no look-alikes: {secret}");

        // Approved; the person signs in with it (the lock is cleared), the
        // old password and the old session are gone.
        let r = row(&id);
        assert_eq!(r.state, RequestState::Approved);
        let msg = r.result_message.clone().unwrap_or_default();
        assert!(msg.starts_with("Password reset by the owner at ") && msg.contains("(shown to the owner once)"), "{msg}");
        assert!(matches!(skin::check_and_record_login(&u, &secret), skin::SkinLoginOutcome::Ok(_)), "signs in: the lock is cleared");
        assert!(!skin::verify_principal_password(&u, OLD_PW));
        assert!(skin::resolve_skin_token(&session).is_none(), "every session ended");
        let turn = last_turn(&e.ws);
        assert!(turn.contains("approved") && turn.contains("Password reset by the owner"), "{turn}");

        // A replayed answer reveals nothing and changes nothing.
        let (s, again) = answer_as("owner", &e.ws, &card, allow(json!({ "reveal": 1, "client": "desktop" })));
        assert_ne!(s, "200 OK", "{again}");
        assert!(again.get("forYou").is_none() && !again.to_string().contains(&secret), "{again}");
        assert!(matches!(skin::check_and_record_login(&u, &secret), skin::SkinLoginOutcome::Ok(_)), "the password stands");

        // Nothing stored holds it; the audit names who, never what.
        let places = stored_text(&e.ws, &card, &id);
        assert_nowhere(&secret, &places);
        let audit = &places.iter().find(|(w, _)| *w == "audit").expect("audit").1;
        assert!(audit.contains(AUDIT_EVENT) && audit.contains(&format!("user={u}")) && audit.contains("mode=password"), "{audit}");
        assert!(audit.contains(&format!("asked_by=agent:{}", e.ws.handle)), "{audit}");
        secret
    });
    assert!(!log.contains(&secret), "the daemon log holds the secret: {log}");
    assert!(log.contains(&format!("[permission] {ACTION}: mode=password user={u}")), "the log names the reset: {log}");
}

#[cfg(unix)]
#[test]
fn link_mode_hands_the_approver_an_owner_reset_link_once() {
    let e = env("prlink");
    let (u, pid) = person(&e.app, "cat", None);
    let v = ask_ok(&e.ws, ACTION, json!({ "user": u, "app": e.app_name, "mode": "link" }));
    let (id, card) = (id_of(&v), card_of(&v));
    // No email on file: the card offers no email.
    assert_eq!(view(&e.ws, &card)["reveal"], json!({ "mode": "link", "emailOffered": false }));
    let (a, log) = with_log(|| answer_as("owner", &e.ws, &card, allow(json!({ "reveal": 1, "client": "web" }))));
    assert_eq!(a.0, "200 OK", "{}", a.1);
    let link = a.1["forYou"]["link"].as_str().expect("link").to_string();
    assert!(link.starts_with(&format!("{DOOR}/cli/skin/password/set#k2inv_")), "{link}");
    let token = link.split('#').nth(1).expect("token").to_string();
    assert!(a.1["forYou"]["expiresAt"].as_i64().is_some(), "{}", a.1);
    // The link is the owner's reset (once, 24 h; PR-A6, not an agent invite).
    let rows = skin::invites::rows_for(&pid).expect("rows");
    assert_eq!(rows.first().map(|r| r.purpose.as_str()), Some(skin::invites::PURPOSE_OWNER_RESET), "{rows:?}");
    assert!(skin::verify_principal_password(&u, OLD_PW), "nothing changes until they use it");
    let msg = row(&id).result_message.unwrap_or_default();
    assert!(msg.starts_with(&format!("Set-password link for {u} made by the owner at ")), "{msg}");
    let places = stored_text(&e.ws, &card, &id);
    assert_nowhere(&token, &places);
    assert!(!log.contains(&token), "the daemon log holds the link: {log}");
}

#[test]
fn email_them_a_link_instead_works_from_a_phone_and_never_names_the_address() {
    let e = env("premail");
    let addr = format!("dee{}@x.test", uniq());
    let (u, _) = person(&e.app, "dee", Some(&addr));
    let v = ask_ok(&e.ws, ACTION, json!({ "user": u }));
    let (id, card) = (id_of(&v), card_of(&v));
    // "Email them a link instead" from the phone: no screen needed.
    let (s, a) = answer_as("owner", &e.ws, &card, allow(json!({ "mode": "email", "client": "phone" })));
    assert_eq!(s, "200 OK", "{a}");
    assert!(a.get("forYou").is_none(), "nothing to reveal: {a}");
    let sent = e.f.engine.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "one mail");
    assert_eq!(sent[0].rcpt, vec![addr.clone()], "to the stored address");
    let sys = sent[0].from.clone();
    assert!(sys.starts_with("system@"), "from the system sender no agent reads: {sys}");
    let msg = row(&id).result_message.unwrap_or_default();
    assert!(msg.starts_with(&format!("Reset link emailed to {u}'s address on file by the owner at ")), "{msg}");
    for (what, text) in stored_text(&e.ws, &card, &id).into_iter().filter(|(w, _)| *w != "audit") {
        assert!(!text.contains(&addr), "the {what} names the address: {text}");
    }
}

#[test]
fn email_is_refused_where_the_stored_email_isnt_trusted() {
    let e = env("prnoem");
    let (u, pid) = person(&e.app, "eve", Some("eve@agent-chosen.test"));
    // An agent touched them: their stored email may be the agent's pick.
    skin::ties::set(&pid, skin::ties::TieWrite { workspace_id: &e.ws.id, set_by: "agent:x", reason: skin::ties::REASON_PASSWORD }).expect("tie");
    let (s, v) = ask(&e.ws, ACTION, json!({ "user": u, "mode": "email" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("email_unconfirmed")), "{v}");
    assert!(!v.to_string().contains("agent-chosen.test"), "never the address: {v}");
    // A password card doesn't offer it, and an email answer is refused
    // without deciding.
    let v = ask_ok(&e.ws, ACTION, json!({ "user": u }));
    let (id, card) = (id_of(&v), card_of(&v));
    assert_eq!(view(&e.ws, &card)["reveal"]["emailOffered"], false);
    let (s, a) = answer_as("owner", &e.ws, &card, allow(json!({ "mode": "email" })));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("409 Conflict", &json!("email_unconfirmed")), "{a}");
    assert!(row(&id).is_answerable());
    assert!(e.f.engine.sent.lock().unwrap().is_empty(), "nothing mailed");
    // No email at all.
    let (u2, _) = person(&e.app, "fay", None);
    let (s, v) = ask(&e.ws, ACTION, json!({ "user": u2, "mode": "email" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("no_email")), "{v}");
}

#[test]
fn a_reset_card_is_never_batched() {
    let e = env("prbatch");
    let (u1, _) = person(&e.app, "gus", Some("gus@x.test"));
    let (u2, _) = person(&e.app, "hal", Some("hal@x.test"));
    let c1 = card_of(&ask_ok(&e.ws, ACTION, json!({ "user": u1 })));
    let c2 = card_of(&ask_ok(&e.ws, ACTION, json!({ "user": u2 })));
    assert_ne!(c1, c2, "each its own card");
    for c in [&c1, &c2] {
        assert_eq!(view(&e.ws, c)["kind"], "permission", "never a batch");
    }
}

#[test]
fn the_list_finds_the_card_by_password() {
    let e = env("prlist");
    let body = json!({ "search": "password" }).to_string();
    let r = routes::handle_gated(routes::LIST, &super::super::held::tests::agent(&e.ws), false, body.as_bytes(), &HashMap::new());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    let found = v["actions"].as_array().expect("actions").iter().find(|a| a["id"] == ACTION).cloned().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(found["setting"], "Reset the person's password (their App)", "{found}");
    assert!(!found["whatChanges"].as_str().unwrap().contains('{'), "no raw placeholders: {found}");
}
