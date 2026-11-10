//! `vault.send` end to end (0.46.11): an agent's `k2 vault send` of a
//! person-given secret to another server is held and says how to ask; the
//! agent asks; an Owner or Admin sends it from the card with the vault
//! confirm (as themselves), or lets the agent send those secrets to that
//! agent itself from now on; Deny sends nothing. Settings lists, adds and
//! removes the standing grants. The other server is a loopback stub with its
//! own temp vault (`federation_vault::tests::spawn_stub`); temp HOME, test
//! DB, fake key store; no real agent is woken (turns go to `inject`'s sink).
//!
//! The refusal's `Ask with:` line with cards on needs the process-wide
//! switch, so it lives in `tests/permission_vault_send.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use k2_core::connect_users::Role;
use k2_core::federation::{self, FederationPeer, PeerStore, PeerTrust};
use k2_core::permission::catalog::{ConfirmAction, Floor, Grant, Proof};
use k2_vault::{AgentActor, Kind, PutRequest, SecretBytes};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, id_of, last_turn, login, row, seed, view, Ws};
use super::*;
use crate::federation_vault::tests::{other_vault, send, spawn_stub, Seen};
use crate::vault::actor::{Channel, VaultCaller};

/// This server with one paired, Trusted peer (`heldbox`, agent `bob` in
/// `ws-b`) that opens what it's sent for real.
struct Fx {
    v: Arc<k2_vault::LocalVault>,
    seen: Arc<std::sync::Mutex<Seen>>,
    fp: String,
    peer_host: String,
    _env: (k2_core::test_env::EnvVar, k2_core::test_env::EnvVar),
    _d: k2_vault::fsutil::TestDir,
    _home: crate::test_support::TempHome,
}

fn setup(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (d, receiver) = other_vault(tag);
    let peer_key = Arc::new(rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap());
    let mut store = PeerStore::load().unwrap_or_default();
    let fp = store.upsert(FederationPeer::pin("heldbox", "heldbox", peer_key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    let (port, seen) = spawn_stub(false, peer_key, receiver, federation::local_fingerprint().unwrap());
    let base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &format!("http://127.0.0.1:{port}"));
    let fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
    let peer = PeerStore::load().unwrap().list().iter().find(|p| p.fingerprint == fp).cloned().expect("pinned");
    let peer_host = crate::federation_routes::peer_host(&peer);
    Fx { v, seen, fp, peer_host, _env: (base, fed), _d: d, _home: home }
}

/// The value every secret here holds; it must never surface.
fn value_of(name: &str) -> String {
    format!("k2test_VAULT_SEND_{name}_not_real")
}

/// A secret the owner gave this workspace (person-given).
fn given(v: &k2_vault::LocalVault, ws: &Ws, name: &str) -> String {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), name, Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    v.put(r, SecretBytes::from_slice(value_of(name).as_bytes()), &Actor::Owner).expect("put").meta.id
}

fn agent_caller(ws: &Ws) -> VaultCaller {
    let sid = format!("sess-{}", &ws.id[..8]);
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.id.clone(), session_id: Some(sid.clone()), shell: false, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some(sid),
    }
}

fn owner() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None }
}

fn copy(ws: &Ws) -> Ws {
    Ws { id: ws.id.clone(), handle: ws.handle.clone(), conv: ws.conv.clone() }
}

/// The agent's own `k2 vault send NAME --to TO`, off the runtime (it dials).
async fn agent_sends(fx: &Fx, ws: &Ws, name: &str, to: &str) -> (u16, Value, String) {
    let (v, c, b) = (fx.v.clone(), agent_caller(ws), json!({ "name": name, "to": to }));
    tokio::task::spawn_blocking(move || send(&v, &c, b)).await.unwrap()
}

/// A person answers the card, off the runtime (Allow sends: it dials).
async fn answer(author: &str, ws: &Ws, card: &str, body: Value) -> (String, Value) {
    let (a, w, c) = (author.to_string(), copy(ws), card.to_string());
    tokio::task::spawn_blocking(move || answer_as(&a, &w, &c, body)).await.unwrap()
}

fn confirm_for(v: &k2_vault::LocalVault, request: &str, stamp: &str) -> String {
    v.confirm.mint(SHARE_CONFIRM, Some(request), stamp, "local-auth")
}

fn sent_count(fx: &Fx) -> usize {
    fx.seen.lock().unwrap().handoff_bodies.len()
}

/// The workspace's connection to `bob` there, bound to its workspace id (the
/// sender's own gate, which a standing grant never lifts).
fn connect(fx: &Fx, ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let addr = format!("bob::{}", fx.peer_host);
    k2_core::db::schema::WorkspaceRemoteConnection::create(&conn, &id, &ws.id, &addr, &fx.peer_host, "bob", Some(&fx.fp)).unwrap();
    k2_core::db::schema::WorkspaceRemoteConnection::set_binding(&conn, &id, Some("ws-b"), Some("bob"), None).unwrap();
}

fn audit_lines() -> Vec<(String, String)> {
    k2_core::auth_audit::tail(2000)
        .unwrap()
        .into_iter()
        .filter(|e| e["event"] == crate::vault_handoff_routes::AUDIT_EVENT)
        .map(|e| (e["user"].as_str().unwrap_or("").to_string(), e["outcome"].as_str().unwrap_or("").to_string()))
        .collect()
}

fn options_of(ws: &Ws, card: &str) -> Vec<String> {
    let doc = k2_core::overlay::get_thread_doc(&ws.conv, card).expect("doc read").expect("card doc");
    let d = crate::permission::view::project(&doc, &k2_core::permission::card::Viewer::OwnerToken, 2);
    d.choice.as_ref().map(|c| c.options.iter().map(|o| o.label.clone()).collect()).unwrap_or_default()
}

fn send_allow_get(fx: &Fx, c: &VaultCaller, ws: &Ws) -> (u16, Value) {
    let params: HashMap<String, String> = [("workspace".to_string(), ws.id.clone())].into();
    let r = crate::vault_handoff_routes::handle(&fx.v, crate::vault_handoff_routes::SEND_ALLOW, c, b"", &params, "test");
    (r.status[..3].parse().unwrap(), serde_json::from_str(&r.body).unwrap_or(Value::Null))
}

fn send_allow_post(fx: &Fx, c: &VaultCaller, body: Value) -> (u16, Value) {
    let r = crate::vault_handoff_routes::handle(&fx.v, crate::vault_handoff_routes::SEND_ALLOW, c, body.to_string().as_bytes(), &HashMap::new(), "test");
    (r.status[..3].parse().unwrap(), serde_json::from_str(&r.body).unwrap_or(Value::Null))
}

#[test]
fn the_card_is_registered_with_the_senders_floor_a_confirm_and_its_wall() {
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (r.spec.floor, r.spec.proof, r.spec.grant),
        (Floor::Admin, Proof::SettingsConfirm { action: ConfirmAction::VaultShareService }, Grant::OneTime)
    );
    assert!(r.spec.params[0].list, "several secrets for one agent go on one card");
    assert_eq!(r.spec.walls.len(), 1);
    assert_eq!(r.spec.walls[0].route_prefix, "/cli/vault/send");
    assert!(crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post).is_some(), "{ROUTE} has a POST row");
}

/// Today's rule, with words that say what to do. Cards off (lib tests never
/// flip the server switch): the hint names Settings and the exact command.
/// A person's own send and a secret the agent gave itself are unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_send_names_the_secret_and_how_to_get_a_yes_and_other_sends_are_unchanged() {
    let fx = setup("vs-held");
    let ws = seed("vsh");
    given(&fx.v, &ws, "GEO_DB");

    let (v, c) = (fx.v.clone(), agent_caller(&ws));
    let (st, j, raw) = tokio::task::spawn_blocking(move || send(&v, &c, json!({ "name": "GEO_DB", "to": "bob::heldbox", "note": "for the import" })))
        .await
        .unwrap();
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "held"), "{raw}");
    assert_eq!((j["error"]["secret"].as_str(), j["error"]["to"].as_str(), j["error"]["note"].as_str()), (Some("GEO_DB"), Some("bob::heldbox"), Some("for the import")), "{j}");
    let hint = j["error"]["hint"].as_str().unwrap();
    assert!(hint.starts_with("a person gave GEO_DB, so it goes to another server only when a person on this server says so."), "{hint}");
    assert!(hint.contains("Ask your person (an Owner or Admin)"), "{hint}");
    assert!(hint.contains(&format!("Settings → Workspaces → {} → Permissions → Secrets it may send to other servers", ws.handle)), "{hint}");
    assert!(hint.contains(&format!("k2 vault send GEO_DB --scope ws:{} --to bob::heldbox", ws.handle)), "{hint}");
    assert!(j.get("permission").is_none(), "cards off: no closing line here");
    assert!(!raw.contains(&value_of("GEO_DB")));
    assert_eq!(sent_count(&fx), 0, "nothing left this server");

    // The walls map teaches exactly this ask from that refusal.
    let params = HashMap::new();
    let req = crate::permission::walls::Refused { status: 403, method: "POST", path: ROUTE, params: &params, body: None, response: &j, workspace_id: "" };
    let p = crate::permission::walls::permission_for(&req).expect("teaches");
    assert_eq!(p["action"], ACTION, "{p}");
    assert_eq!(p["approvers"], "owner or admin", "{p}");
    assert_eq!(p["command"], "k2 permission request vault.send note='for the import' secret=GEO_DB to=bob::heldbox --why \"<what you need it for>\"", "{p}");

    // A person's own send: the confirm, then sent, as before.
    let (v, o, h) = (fx.v.clone(), owner(), ws.handle.clone());
    let (st, j, raw) = tokio::task::spawn_blocking(move || send(&v, &o, json!({ "name": "GEO_DB", "scope": "workspace", "workspace": h, "to": "bob::heldbox" })))
        .await
        .unwrap();
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "confirm_required"), "{raw}");
    let token = fx.v.confirm.mint(crate::vault_routes::HANDOFF_CONFIRM_ACTION, Some(j["secretId"].as_str().unwrap()), "owner", "vault-password");
    let (v, o, h) = (fx.v.clone(), owner(), ws.handle.clone());
    let (st, j, raw) = tokio::task::spawn_blocking(move || {
        send(&v, &o, json!({ "name": "GEO_DB", "scope": "workspace", "workspace": h, "to": "bob::heldbox", "confirmToken": token }))
    })
    .await
    .unwrap();
    assert_eq!((st, j["status"].as_str()), (200, Some("pending")), "{raw}");
    assert_eq!(sent_count(&fx), 1);

    // A secret the agent gave itself is never held: its own send goes as
    // before (here: not connected to bob, so the connection gate refuses).
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), "MINE", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    fx.v.put(r, SecretBytes::from_slice(value_of("MINE").as_bytes()), &agent_caller(&ws).actor).unwrap();
    let (st, j, raw) = agent_sends(&fx, &ws, "MINE", "bob::heldbox").await;
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "not_connected"), "{raw}");
    connect(&fx, &ws);
    let (st, _, raw) = agent_sends(&fx, &ws, "MINE", "bob::heldbox").await;
    assert_eq!(st, 200, "{raw}");
    // And the card is only for a person-given secret.
    let hint = super::super::held::tests::ask_refused(&ws, ACTION, json!({ "secret": "MINE", "to": "bob::heldbox" }));
    assert!(hint.contains("isn't one a person gave"), "{hint}");
}

/// One card for several secrets; a Member can't answer; no confirm (or the
/// wrong one) sends nothing; the owner's confirmed Allow sends each one as
/// the owner, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_sends_them_from_one_card_with_the_confirm_and_is_logged_as_the_sender() {
    let fx = setup("vs-allow");
    let ws = seed("vsa");
    given(&fx.v, &ws, "GEO_A");
    given(&fx.v, &ws, "GEO_B");

    let asked = ask_ok(&ws, ACTION, json!({ "secret": "GEO_A,GEO_B", "to": "bob::heldbox", "note": "for the import" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], "Send GEO_A and GEO_B to bob::heldbox", "{vw}");
    assert_eq!(vw["whatChanges"], format!("Send GEO_A and GEO_B from {}'s vault to bob::heldbox, an agent on another server.", ws.handle), "{vw}");
    let cons = vw["consequences"].as_str().unwrap();
    assert!(cons.starts_with(&format!("A person gave {} GEO_A and GEO_B, so they leave this server only when you say so.", ws.handle)), "{cons}");
    assert!(cons.ends_with("The note that goes with it: \"for the import\""), "{cons}");
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": SHARE_CONFIRM, "secretId": id }), "{vw}");
    assert_eq!(options_of(&ws, &card), vec!["Allow", "Deny", label::ALWAYS_VAULT_SEND], "both choices, both behind the confirm");
    assert_eq!(
        super::super::vault::confirm_reason(SHARE_CONFIRM, Some(&id)),
        Some(Ok(format!("send GEO_A and GEO_B from {} to bob::heldbox on another server", ws.handle)))
    );

    // A Member sees it and can't answer it.
    let member = login("vsm", Role::Member);
    below_floor(answer(&member, &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&fx.v, &id, &format!("connect:{member}")) })).await, "admin");
    // No confirm, another card's, someone else's: nothing is sent, the card stays open.
    let (s, a) = answer("owner", &ws, &card, json!({ "answer": "Allow" })).await;
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let (s, a) = answer("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&fx.v, "pr_another_card", "owner") })).await;
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let admin = login("vsd", Role::Admin);
    let (s, a) = answer("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&fx.v, &id, &format!("connect:{admin}")) })).await;
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    // The route's own `handoff-send` confirm isn't this card's proof.
    let m = fx.v.meta(&Selector::user(Scope::workspace(ws.id.clone()), "GEO_A"), &Actor::Owner).unwrap();
    let hs = fx.v.confirm.mint(crate::vault_routes::HANDOFF_CONFIRM_ACTION, Some(&m.id), "owner", "local-auth");
    let (s, a) = answer("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": hs })).await;
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert!(row(&id).is_answerable(), "a refused confirm leaves the card open");
    assert_eq!(sent_count(&fx), 0, "approving without the confirm sends nothing");

    // The owner, with the confirm bound to this card: both go, as the owner.
    let token = confirm_for(&fx.v, &id, "owner");
    let (s, a) = answer("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": token, "client": "phone" })).await;
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert!(fx.v.confirm.take(&token, SHARE_CONFIRM, Some(&id), "owner").is_err(), "the Allow spent the token");
    let opened = fx.seen.lock().unwrap().opened.clone();
    assert_eq!(opened.iter().map(|o| o.1.as_str()).collect::<Vec<_>>(), vec!["GEO_A", "GEO_B"]);
    assert_eq!(opened[0].2, value_of("GEO_A").as_bytes(), "the other server's vault opens the right value");
    for (delivery, name, _) in &opened {
        let out = fx.v.handoff_get(delivery, "out").expect("out row");
        assert_eq!((out.sender_actor.as_str(), out.name.as_str()), ("owner", name.as_str()), "the person is the sender");
        assert_eq!(out.note, "for the import");
    }
    let lines = audit_lines();
    for name in ["GEO_A", "GEO_B"] {
        assert!(lines.iter().any(|(u, o)| u == "owner" && o.starts_with(&format!("ok send {name} to bob::"))), "{name}: {lines:?}");
    }
    let turn = last_turn(&ws);
    assert!(turn.contains("approved by owner") && turn.contains("K2 sent GEO_A and GEO_B to bob::heldbox as the owner"), "{turn}");
    assert!(turn.contains("Don't send them again"), "{turn}");
    assert!(fx.v.send_allow(Some(&ws.id)).unwrap().is_empty(), "Allow is once: no standing grant");

    // Once: a second answer sends nothing more.
    let (s, _) = answer("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&fx.v, &id, "owner") })).await;
    assert_eq!(s, "409 Conflict");
    assert_eq!(sent_count(&fx), 2);
    for (what, text) in [("turn", last_turn(&ws)), ("request", format!("{:?}", row(&id))), ("view", view(&ws, &card).to_string()), ("answer", a.to_string())] {
        assert!(!text.contains(&value_of("GEO_A")) && !text.contains(&value_of("GEO_B")), "{what} carries a value: {text}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denying_sends_nothing_and_tells_the_agent() {
    let fx = setup("vs-deny");
    let ws = seed("vsn");
    given(&fx.v, &ws, "GEO_DB");
    let asked = ask_ok(&ws, ACTION, json!({ "secret": "GEO_DB", "to": "bob::heldbox" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let (s, a) = answer("owner", &ws, &card, json!({ "answer": "Deny", "reason": "not that database" })).await;
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state.as_str(), "denied");
    assert_eq!(sent_count(&fx), 0, "nothing was sent");
    assert!(fx.v.send_allow(Some(&ws.id)).unwrap().is_empty());
    let turn = last_turn(&ws);
    assert!(turn.contains("denied") && turn.contains("not that database"), "the agent is told: {turn}");
    let (st, j, _) = agent_sends(&fx, &ws, "GEO_DB", "bob::heldbox").await;
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "held"), "still held");
}

/// "From now on": sent now, then the agent's own later sends of those
/// secrets to that agent go through (logged on the person's grant); another
/// agent or another secret still asks; removing the grant stops it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_now_on_lets_later_sends_through_to_that_agent_only_until_it_is_removed() {
    let fx = setup("vs-always");
    let ws = seed("vsw");
    given(&fx.v, &ws, "GEO_DB");
    given(&fx.v, &ws, "OTHER_DB");
    connect(&fx, &ws);

    let asked = ask_ok(&ws, ACTION, json!({ "secret": "GEO_DB", "to": "bob::heldbox" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    // The choice needs the confirm too.
    let (s, a) = answer("owner", &ws, &card, json!({ "answer": label::ALWAYS_VAULT_SEND })).await;
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert!(fx.v.send_allow(Some(&ws.id)).unwrap().is_empty());
    let admin = login("vsx", Role::Admin);
    let (s, a) = answer(&admin, &ws, &card, json!({ "answer": label::ALWAYS_VAULT_SEND, "confirmToken": confirm_for(&fx.v, &id, &format!("connect:{admin}")) })).await;
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(sent_count(&fx), 1, "sent now");
    let grants = fx.v.send_allow(Some(&ws.id)).unwrap();
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert_eq!((grants[0].to.clone(), grants[0].added_by.clone()), (format!("bob::{}", fx.peer_host), format!("connect:{admin}")));
    let turn = last_turn(&ws);
    assert!(turn.contains(&format!("From now on {} may send GEO_DB to bob::{} itself, without a card", ws.handle, fx.peer_host)), "{turn}");

    // A later send of that secret to that agent: straight through, no card.
    let (st, j, raw) = agent_sends(&fx, &ws, "GEO_DB", "bob::heldbox").await;
    assert_eq!((st, j["status"].as_str()), (200, Some("pending")), "{raw}");
    assert_eq!(j["onGrant"]["by"], format!("connect:{admin}"), "{j}");
    assert_eq!(sent_count(&fx), 2);
    let lines = audit_lines();
    assert!(
        lines.iter().any(|(u, o)| u.starts_with(&format!("agent:{}", ws.id)) && o.starts_with("ok send GEO_DB to bob::") && o.contains(&format!("on connect:{admin}'s send grant"))),
        "logged with the agent and the grant: {lines:?}"
    );
    // Another agent there, or another secret: still held.
    let (st, j, _) = agent_sends(&fx, &ws, "GEO_DB", "carol::heldbox").await;
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "held"), "another target still asks");
    let (st, j, _) = agent_sends(&fx, &ws, "OTHER_DB", "bob::heldbox").await;
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "held"), "another secret still asks");
    // Another workspace can't ride it.
    let other = seed("vso");
    given(&fx.v, &other, "GEO_DB");
    let (st, j, _) = agent_sends(&fx, &other, "GEO_DB", "bob::heldbox").await;
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "held"));
    assert_eq!(sent_count(&fx), 2);

    // Settings lists it; the agent sees its own; removing it stops it.
    let (st, l) = send_allow_get(&fx, &agent_caller(&ws), &ws);
    assert_eq!(st, 200, "{l}");
    assert_eq!(l["entries"][0]["name"], "GEO_DB", "{l}");
    assert!(l.get("secrets").is_none(), "the agent gets no picker");
    let to = l["entries"][0]["to"].as_str().unwrap().to_string();
    let (st, e) = send_allow_post(&fx, &agent_caller(&ws), json!({ "workspace": ws.id, "name": "GEO_DB", "to": to, "action": "remove" }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("handoff_person_only")), "an agent never edits it: {e}");
    let (st, l) = send_allow_post(&fx, &owner(), json!({ "workspace": ws.id, "name": "GEO_DB", "to": to, "action": "remove" }));
    assert_eq!(st, 200, "{l}");
    assert_eq!(l["entries"], json!([]), "{l}");
    let (st, j, _) = agent_sends(&fx, &ws, "GEO_DB", "bob::heldbox").await;
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "held"), "removed: it asks again");
    assert_eq!(sent_count(&fx), 2);
    assert!(audit_lines().iter().any(|(u, o)| u == "owner" && o.starts_with("ok send-allow remove GEO_DB")));
}

/// Settings → Permissions: an Owner or Admin adds one with the vault confirm
/// (the same proof as sending it); a Member and an agent can't; a secret
/// the agent gave itself needs none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_adds_one_only_with_a_persons_vault_confirm() {
    let fx = setup("vs-settings");
    let ws = seed("vss");
    let sid = given(&fx.v, &ws, "GEO_DB");
    connect(&fx, &ws);
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), "MINE", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    fx.v.put(r, SecretBytes::from_slice(value_of("MINE").as_bytes()), &agent_caller(&ws).actor).unwrap();

    let (st, l) = send_allow_get(&fx, &owner(), &ws);
    assert_eq!(st, 200, "{l}");
    assert_eq!(l["secrets"], json!([{ "id": sid, "name": "GEO_DB" }]), "the picker lists the person-given secrets only: {l}");
    let add = json!({ "workspace": ws.id, "secretId": sid, "to": "bob::heldbox", "action": "add" });

    let (st, e) = send_allow_post(&fx, &agent_caller(&ws), add.clone());
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("handoff_person_only")), "{e}");
    let member = login("vsq", Role::Member);
    let mc = VaultCaller { actor: Actor::Login { username: member.clone(), role: k2_vault::Role::Member }, channel: Channel::Login, session_id: None };
    let (st, e) = send_allow_post(&fx, &mc, add.clone());
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("handoff_person_only")), "{e}");
    let (st, e) = send_allow_post(&fx, &owner(), add.clone());
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("confirm_required")), "no confirm, no grant: {e}");
    let mut wrong = add.clone();
    wrong["confirmToken"] = json!(fx.v.confirm.mint(crate::vault_routes::HANDOFF_CONFIRM_ACTION, Some("another-secret"), "owner", "local-auth"));
    let (st, _) = send_allow_post(&fx, &owner(), wrong);
    assert_eq!(st, 403, "a confirm for another secret isn't this one's");
    assert!(fx.v.send_allow(Some(&ws.id)).unwrap().is_empty());
    let mut mine = json!({ "workspace": ws.id, "name": "MINE", "to": "bob::heldbox", "action": "add" });
    mine["confirmToken"] = json!("x");
    let (st, e) = send_allow_post(&fx, &owner(), mine);
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("not_needed")), "{e}");

    let mut ok = add.clone();
    ok["confirmToken"] = json!(fx.v.confirm.mint(crate::vault_routes::HANDOFF_CONFIRM_ACTION, Some(&sid), "owner", "local-auth"));
    let (st, l) = send_allow_post(&fx, &owner(), ok);
    assert_eq!(st, 200, "{l}");
    assert_eq!(l["entries"][0]["to"], format!("bob::{}", fx.peer_host), "stored as the paired server names itself: {l}");
    assert_eq!(l["entries"][0]["addedBy"], "owner");
    let (st, j, raw) = agent_sends(&fx, &ws, "GEO_DB", "bob::heldbox").await;
    assert_eq!(st, 200, "{raw}");
    assert_eq!(j["onGrant"]["by"], "owner");
    assert!(audit_lines().iter().any(|(u, o)| u == "owner" && o.starts_with("ok send-allow add GEO_DB")));
    // A card for a secret the agent may already send is refused when asked.
    let (s, v) = ask(&ws, ACTION, json!({ "secret": "GEO_DB", "to": format!("bob::{}", fx.peer_host) }));
    assert!(s.starts_with("400") && v["error"]["hint"].as_str().unwrap_or("").contains("already lets you send"), "{s}: {v}");
}
