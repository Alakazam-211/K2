//! The owner's decide token against a stub plane, kept in the test vault's
//! real `system` scope: the owner's switch-on mints and stores it, tells
//! the owner (a Thread line and an audit row, never the token); a 409 with
//! the token here keeps it; a 409 with it lost revokes, then mints; a 429
//! is answered, not retried; a 5xx is tried again; switch-off revokes it on
//! the plane and removes it; a person's Accept with no token asks once; an
//! agent can't read it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use k2_core::postal::sdk::plane::Request;
use k2_vault::{ListFilter, Namespace, Requester, Scope, SecretStore, Selector, Use};
use serde_json::{json, Value};

use super::*;
use crate::compute_routes::Who;
use crate::postal_net::plane::{Answer, Plane, Transport};

const TOKEN_A: &str = "k2c_pd_0123456789abcdef0123456789abcdef";
const TOKEN_B: &str = "k2c_pd_fedcba9876543210fedcba9876543210";
const LABEL: &str = "initech";

/// The plane's two routes, answered from a script per path (the last
/// answer repeats); every call is recorded.
#[derive(Default)]
struct Stub {
    answers: Mutex<HashMap<&'static str, Vec<(u16, Value)>>>,
    calls: Mutex<Vec<String>>,
}

impl Stub {
    fn script(&self, path: &'static str, answers: Vec<(u16, Value)>) {
        self.answers.lock().unwrap().insert(path, answers);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn minted(token: &str) -> (u16, Value) {
    (200, json!({ "token": token, "scope": "postal-decide", "label": LABEL, "minted_at": "2026-11-20T00:00:00Z" }))
}

fn plane_error(status: u16, code: &str) -> (u16, Value) {
    (status, json!({ "error": { "code": code, "message": "", "retry_after_s": 3600 } }))
}

impl Transport for Arc<Stub> {
    fn call(&self, bearer: &str, req: &Request) -> Result<Answer, String> {
        assert_eq!(bearer, "k2c_connect_test", "the box's Connect credential, nothing else");
        assert_eq!(req.body.as_deref(), Some("{}"));
        let path = req.path.strip_prefix("/api/postal/v1").expect("a v1 path").to_string();
        self.calls.lock().unwrap().push(path.clone());
        let mut a = self.answers.lock().unwrap();
        let list = a.get_mut(path.as_str()).unwrap_or_else(|| panic!("no answer scripted for {path}"));
        let (s, v) = if list.len() > 1 { list.remove(0) } else { list[0].clone() };
        Ok((s, v.to_string().into_bytes()))
    }
    fn post_open(&self, _path: &str, _body: &Value) -> Result<Answer, String> {
        Err("not in this test".into())
    }
}

struct Box1 {
    _home: crate::test_support::TempHome,
    stub: Arc<Stub>,
    vault: Arc<k2_vault::LocalVault>,
}

fn setup(on: bool) -> (Box1, crate::postal_routes::SwitchGuard) {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let vault = crate::vault::ensure_test_vault();
    crate::vault::postal::install();
    k2_core::postal::keys::clear_decide_token(LABEL).expect("a clean slate");
    let guard = crate::postal_routes::switch_for_test(on);
    reset_for_test();
    let stub = Arc::new(Stub::default());
    super::super::install(Arc::new(Plane::new(Box::new(stub.clone()), Box::new(|| Ok(("k2c_connect_test".to_string(), LABEL.to_string()))))));
    (Box1 { _home: home, stub, vault }, guard)
}

fn switch(on: bool) -> Value {
    let r = crate::postal_routes::handle(crate::postal_routes::SETTINGS, Who::Owner, true, json!({ "enabled": on }).to_string().as_bytes(), &HashMap::new());
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    assert!(!r.body.contains("k2c_pd_"), "the token is never in an answer: {}", r.body);
    serde_json::from_str(&r.body).expect("json")
}

fn stored() -> Option<String> {
    k2_core::postal::keys::decide_token(LABEL).expect("the vault answers")
}

fn notices_for(label: &str) -> Vec<String> {
    OWNER_NOTICES.lock().unwrap().iter().filter(|n| n.contains(label)).cloned().collect()
}

fn audits() -> Vec<(String, String, String)> {
    crate::hire_routes::AUDIT_SINK.lock().unwrap().iter().filter(|a| a.0 == "postal-decide-token").cloned().collect()
}

#[test]
fn the_owners_switch_on_mints_it_into_the_system_scope_and_tells_the_owner() {
    let (b, _g) = setup(false);
    b.stub.script("/decide-token", vec![minted(TOKEN_A)]);
    let v = switch(true);
    assert_eq!((v["enabled"].as_bool(), v["decideToken"]["state"].as_str()), (Some(true), Some("minted")), "{v}");
    assert_eq!(b.stub.calls(), ["/decide-token"]);
    assert_eq!(stored().as_deref(), Some(TOKEN_A));
    // Where owner-link keeps it: the vault's system scope, engine:postal.
    let sel = Selector::system(Scope::engine("postal"), &k2_core::postal::keys::decide_name(LABEL));
    let daemon = b.vault.resolve(&sel, Use::System, &Requester::System("postal".into())).expect("the daemon reads it");
    assert_eq!(daemon.value.expose(), TOKEN_A.as_bytes());
    // An agent can't: not resolved, not even listed.
    let agent = Requester::Session { workspace_id: "ws-any".into(), session_id: Some("sess".into()) };
    assert!(b.vault.resolve(&sel, Use::Api, &agent).is_err(), "an agent never resolves a system secret");
    let agent_actor = k2_vault::Actor::Agent(k2_vault::AgentActor { workspace_id: "ws-any".into(), session_id: Some("sess".into()), shell: false, vault_manage: false });
    let listed = b.vault.list(&ListFilter { include_system: true, ..Default::default() }, &agent_actor).expect("list");
    assert!(listed.iter().all(|m| m.namespace == Namespace::User), "an agent doesn't see it listed");
    // The owner is told: one Thread line and one audit row, never the token.
    let n = notices_for(LABEL);
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].contains("Postal approvals linked for initech") && !n[0].contains("k2c_pd_"), "{}", n[0]);
    assert!(n[0].contains("Agents on this server aren't isolated from each other yet."), "{}", n[0]);
    let a = audits();
    assert_eq!(a.len(), 1, "{a:?}");
    assert_eq!((a[0].1.as_str(), a[0].2.as_str()), ("owner", "label=initech minted (Postal turned on)"));
}

#[test]
fn a_409_with_the_token_here_keeps_it() {
    let (b, _g) = setup(false);
    k2_core::postal::keys::set_decide_token(LABEL, TOKEN_A).expect("stored");
    b.stub.script("/decide-token", vec![plane_error(409, "already_minted")]);
    let v = switch(true);
    assert_eq!(v["decideToken"]["state"], "kept", "{v}");
    assert_eq!(b.stub.calls(), ["/decide-token"], "no revoke, no second mint");
    assert_eq!(stored().as_deref(), Some(TOKEN_A));
    assert!(notices_for(LABEL).is_empty() && audits().is_empty(), "nothing was minted");
}

#[test]
fn a_409_with_the_token_lost_revokes_then_mints() {
    let (b, _g) = setup(false);
    b.stub.script("/decide-token", vec![plane_error(409, "already_minted"), minted(TOKEN_B)]);
    b.stub.script("/decide-token/revoke", vec![(200, json!({ "revoked": 1 }))]);
    let v = switch(true);
    assert_eq!(v["decideToken"]["state"], "replaced", "{v}");
    assert_eq!(b.stub.calls(), ["/decide-token", "/decide-token/revoke", "/decide-token"]);
    assert_eq!(stored().as_deref(), Some(TOKEN_B));
    assert_eq!(notices_for(LABEL).len(), 1);
    assert_eq!(audits()[0].2, "label=initech replaced (Postal turned on)");
}

#[test]
fn a_429_is_answered_and_not_retried() {
    let (b, _g) = setup(false);
    b.stub.script("/decide-token", vec![plane_error(429, "rate_limited")]);
    let v = switch(true);
    assert_eq!(v["enabled"], true, "Postal is on all the same: {v}");
    assert_eq!((v["decideToken"]["state"].as_str(), v["decideToken"]["code"].as_str()), (Some("failed"), Some("rate_limited")), "{v}");
    assert!(v["decideToken"]["hint"].as_str().expect("hint").contains("postal owner-link"), "the fallback is named: {v}");
    assert_eq!(b.stub.calls(), ["/decide-token"], "one call, no loop");
    assert_eq!(stored(), None);
}

#[test]
fn a_5xx_is_tried_again_on_the_switch() {
    let (b, _g) = setup(false);
    b.stub.script("/decide-token", vec![plane_error(500, "internal"), minted(TOKEN_A)]);
    let v = switch(true);
    assert_eq!(v["decideToken"]["state"], "minted", "{v}");
    assert_eq!(b.stub.calls(), ["/decide-token", "/decide-token"]);
    assert_eq!(stored().as_deref(), Some(TOKEN_A));
}

#[test]
fn the_owners_switch_off_revokes_it_and_removes_it() {
    let (b, _g) = setup(true);
    k2_core::postal::keys::set_decide_token(LABEL, TOKEN_A).expect("stored");
    b.stub.script("/decide-token/revoke", vec![(200, json!({ "revoked": 1 }))]);
    let v = switch(false);
    assert_eq!((v["enabled"].as_bool(), v["decideToken"]["state"].as_str()), (Some(false), Some("revoked")), "{v}");
    assert_eq!(b.stub.calls(), ["/decide-token/revoke"]);
    assert_eq!(stored(), None, "gone from the vault");
    assert_eq!(audits()[0].2, "label=initech revoked (1 on the network) and removed");
}

#[test]
fn on_and_on_again_mints_nothing() {
    let (b, _g) = setup(true);
    let v = switch(true);
    assert!(v.get("decideToken").is_none(), "only off to on mints: {v}");
    assert!(b.stub.calls().is_empty());
}

#[test]
fn a_persons_accept_with_no_token_asks_once() {
    let (b, _g) = setup(true);
    let addr = format!("scout::{LABEL}.postal.bot");
    let ws_id = uuid::Uuid::new_v4().to_string();
    let friend_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'scout', '/tmp/k2-postal-dt', 'scout')", rusqlite::params![ws_id]).expect("ws");
        let h = k2_core::postal::store::claim(&conn, &ws_id, &k2_core::postal::parse_address(&addr).expect("addr"), "owner").expect("claim");
        match k2_core::postal::store::record_incoming(&conn, &h, &k2_core::postal::parse_address("ops::globex.postal.bot").expect("peer"), None, None, None, None)
            .expect("request")
        {
            k2_core::postal::store::Incoming::New(f) | k2_core::postal::store::Incoming::Known(f) => f.id,
        }
    };
    let ws = crate::postal_routes::ws_of_id(&ws_id).expect("ws");
    let accept = || {
        let r = crate::postal_routes::confirm_with_code(&Who::Owner, &ws, &friend_id, Some("123456"));
        assert!(!r.body.contains("k2c_pd_"), "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        v
    };
    // The plane fails: one try (no retries on an Accept), and it says why.
    b.stub.script("/decide-token", vec![plane_error(500, "internal")]);
    let v = accept();
    assert_eq!(v["error"]["code"], "internal", "{v}");
    assert_eq!(b.stub.calls(), ["/decide-token"], "one try");
    // It answers: the token is stored, and the Accept goes on to the
    // pairing (which has no handshake in this test).
    b.stub.script("/decide-token", vec![minted(TOKEN_A)]);
    let v = accept();
    assert_eq!(stored().as_deref(), Some(TOKEN_A));
    assert_ne!(v["error"]["code"], "postal_owner_link_missing", "the token was there for the pairing: {v}");
    assert_eq!(b.stub.calls().len(), 2);
    assert_eq!(audits().last().expect("audit").2, "label=initech minted (a friend request accepted)");
    // With the token here, an Accept asks nothing.
    accept();
    assert_eq!(b.stub.calls().len(), 2, "no mint with a token here");
}

// ── If an invite can be sent, it can be accepted ──────────────

fn net() -> Arc<Plane> {
    super::super::net().expect("the test's plane")
}

/// The owner's Link now through the settings route: its status and answer.
fn link_now_route() -> (String, Value) {
    let r = crate::postal_routes::handle(crate::postal_routes::SETTINGS, Who::Owner, true, json!({ "link": true }).to_string().as_bytes(), &HashMap::new());
    assert!(!r.body.contains("k2c_pd_"), "the token is never in an answer: {}", r.body);
    (r.status.to_string(), serde_json::from_str(&r.body).expect("json"))
}

/// No page on the Postal website is ever offered.
fn no_web_page(text: &str) {
    assert!(!text.contains("http://") && !text.contains("https://") && !text.contains("postal.bot/"), "no web page: {text}");
}

fn e500() -> crate::postal_net::plane::CallError {
    crate::postal_net::plane::CallError::Plane(k2_core::postal::sdk::plane::PlaneError::Api {
        status: 500,
        code: k2_core::postal::sdk::plane::Code::Internal,
        message: String::new(),
        retry_after_s: None,
    })
}

#[test]
fn a_server_upgraded_with_postal_on_mints_once_at_start() {
    // Postal was on before the upgrade: no off-to-on switch ever runs.
    let (b, _g) = setup(true);
    b.stub.script("/decide-token", vec![minted(TOKEN_A)]);
    on_start(&net());
    on_start(&net()); // the loop's next good pass
    assert_eq!(b.stub.calls(), ["/decide-token"], "once");
    assert_eq!(stored().as_deref(), Some(TOKEN_A));
    assert_eq!(audits().last().expect("audit").2, "label=initech minted (Postal was on when this server started)");
    let n = notices_for(LABEL);
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(!n[0].contains("k2c_pd_"), "{}", n[0]);
    assert!(audits().iter().all(|a| !a.2.contains("k2c_pd_")));
}

#[test]
fn the_start_check_with_a_token_here_asks_nothing() {
    let (b, _g) = setup(true);
    k2_core::postal::keys::set_decide_token(LABEL, TOKEN_A).expect("stored");
    on_start(&net());
    assert!(b.stub.calls().is_empty(), "{:?}", b.stub.calls());
}

#[test]
fn a_failed_start_is_not_a_retry_storm() {
    let (b, _g) = setup(true);
    b.stub.script("/decide-token", vec![plane_error(500, "internal")]);
    on_start(&net());
    assert_eq!(b.stub.calls(), ["/decide-token"], "the start check tries once (no 5xx retries)");
    // Within the wait every unattended ask answers the same failure
    // without calling the plane: the loop again, an agent's invite and
    // claim, the send loop, a request arriving.
    on_start(&net());
    for why in [Why::Start, Why::Invite { person: false }, Why::Claim { person: false }, Why::Request] {
        let e = ready(&net(), why, "agent:scout").expect_err("no token, and it waits");
        assert_eq!(e.code(), "internal", "{why:?}");
    }
    assert_eq!(b.stub.calls().len(), 1, "no plane call while it waits: {:?}", b.stub.calls());
    assert!(unattended_wait(&e500()) >= std::time::Duration::from_secs(300));
    // A person's Link now asks at once, and links.
    b.stub.script("/decide-token", vec![minted(TOKEN_A)]);
    let (s, v) = link_now_route();
    assert_eq!((s.as_str(), v["decideToken"]["state"].as_str(), v["approvalsLinked"].as_bool()), ("200 OK", Some("minted"), Some(true)), "{v}");
    assert_eq!(b.stub.calls().len(), 2);
    assert_eq!(stored().as_deref(), Some(TOKEN_A));
    assert_eq!(audits().last().expect("audit").2, "label=initech minted (the owner's Link now)");
}

#[test]
fn a_429_holds_a_persons_ask_too_until_its_retry_after() {
    let (b, _g) = setup(true);
    b.stub.script("/decide-token", vec![plane_error(429, "rate_limited")]);
    let e = ready(&net(), Why::Accept, "owner").expect_err("rate limited");
    assert_eq!(e.code(), "rate_limited");
    let again = ready(&net(), Why::Accept, "owner").expect_err("still waiting");
    assert_eq!(again.code(), "rate_limited");
    assert_eq!(b.stub.calls(), ["/decide-token"], "the plane's retry_after is kept by every ask");
    let hint = failed_hint(&again);
    assert!(hint.contains("asked too often") && hint.contains("Link now"), "{hint}");
    no_web_page(&hint);
}

#[test]
fn an_unattended_ask_never_replaces_a_lost_token() {
    let (b, _g) = setup(true);
    b.stub.script("/decide-token", vec![plane_error(409, "already_minted")]);
    let e = ready(&net(), Why::Request, "k2").expect_err("lost there, not here");
    assert_eq!(e.code(), "postal_link_lost");
    assert_eq!(b.stub.calls(), ["/decide-token"], "no revoke without a person");
    assert!(e.hint().contains("Link now"), "{}", e.hint());
    no_web_page(&e.hint());
    // The owner's Link now replaces it.
    b.stub.script("/decide-token", vec![plane_error(409, "already_minted"), minted(TOKEN_B)]);
    b.stub.script("/decide-token/revoke", vec![(200, json!({ "revoked": 1 }))]);
    let (s, v) = link_now_route();
    assert_eq!((s.as_str(), v["decideToken"]["state"].as_str()), ("200 OK", Some("replaced")), "{v}");
    assert_eq!(stored().as_deref(), Some(TOKEN_B));
}

#[test]
fn a_link_now_that_fails_says_why_and_names_owner_link_only_as_the_fallback() {
    let (b, _g) = setup(true);
    b.stub.script("/decide-token", vec![plane_error(401, "unauthorized")]);
    let (s, v) = link_now_route();
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("409 Conflict", Some("unauthorized")), "{v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.contains(&format!("didn't accept this server's {} Connect account", k2_core::product_name!())), "why it failed: {hint}");
    assert!(hint.contains("Link now") && hint.ends_with(&format!("by hand with {} postal owner-link", k2_core::cli_name!())), "{hint}");
    no_web_page(hint);
    assert_eq!(b.stub.calls(), ["/decide-token"], "a 401 is answered, not retried");
    assert_eq!(stored(), None);
    // The settings read says approvals aren't linked.
    let r = crate::postal_routes::handle(crate::postal_routes::SETTINGS, Who::Owner, false, b"", &HashMap::new());
    let v: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["approvalsLinked"], json!(false), "{v}");
}

#[test]
fn a_switch_on_that_fails_says_why_without_a_web_page() {
    let (b, _g) = setup(false);
    b.stub.script("/decide-token", vec![plane_error(401, "unauthorized")]);
    let v = switch(true);
    assert_eq!((v["decideToken"]["state"].as_str(), v["approvalsLinked"].as_bool()), (Some("failed"), Some(false)), "{v}");
    let hint = v["decideToken"]["hint"].as_str().expect("hint");
    assert!(hint.contains("Link now"), "{hint}");
    no_web_page(hint);
    assert_eq!(b.stub.calls(), ["/decide-token"]);
}
