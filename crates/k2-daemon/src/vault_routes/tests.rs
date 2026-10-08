//! Route-level tests for `/cli/vault/*` (V2 must-haves, §19a + §23.7).
//! Temp vault + fake key store; never the real home or keychain.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{Actor, AgentActor, KeyStore, LocalVault, OpenOptions, Role};
use serde_json::{json, Value};

use super::*;
use crate::vault::actor::{Channel, VaultCaller};

const SENTINEL: &str = "k2test_ROUTE_SENTINEL_5e4d3c2b1a0918273645";

struct T {
    _dir: TestDir,
    v: Arc<LocalVault>,
}

fn vault() -> T {
    let dir = TestDir::new("routes");
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let v = Arc::new(LocalVault::open(dir.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
    T { _dir: dir, v }
}

fn owner() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None }
}

fn owner_tty() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Tty, session_id: None }
}

fn login(role: Role) -> VaultCaller {
    VaultCaller { actor: Actor::Login { username: "lee".into(), role }, channel: Channel::Login, session_id: None }
}

fn agent(ws: &str, manage: bool) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("sess-9".into()), shell: false, vault_manage: manage }),
        channel: Channel::Plain,
        session_id: Some("sess-9".into()),
    }
}

fn shell(ws: &str) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("sess-s".into()), shell: true, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some("sess-s".into()),
    }
}

fn call(t: &T, path: &str, c: &VaultCaller, body: Value) -> (u16, Value) {
    call_p(t, path, c, body, HashMap::new())
}

fn call_p(t: &T, path: &str, c: &VaultCaller, body: Value, params: HashMap<String, String>) -> (u16, Value) {
    let b = if body.is_null() { vec![] } else { body.to_string().into_bytes() };
    let r = handle_on(&t.v, path, Ok(c.clone()), &b, &params, "test", None);
    let code: u16 = r.status[..3].parse().expect("status");
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json body for {path}: {}", r.body));
    (code, v)
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn set_server(t: &T, name: &str, value: &str) {
    let (s, v) = call(t, SET, &owner(), json!({ "name": name, "scope": "server", "hosts": ["api.stripe.com"], "valueB64": b64(value) }));
    assert_eq!(s, 200, "{v}");
}

#[test]
fn sentinel_never_leaves_through_management_routes() {
    let t = vault();
    let a = agent("ws1", false);
    let (s, v) = call(&t, SET, &a, json!({ "name": "STRIPE_KEY", "kind": "api_key", "hosts": ["api.stripe.com"], "valueB64": b64(SENTINEL) }));
    assert_eq!(s, 200, "{v}");
    let mut seen = vec![v.to_string()];
    let sel = json!({ "name": "STRIPE_KEY" });
    for (path, body) in [
        (POLICY, json!({ "name": "STRIPE_KEY", "description": "narrowed" })),
        (ROTATE, json!({ "name": "STRIPE_KEY", "valueB64": b64(&format!("{SENTINEL}-2")) })),
        (DISABLE, sel.clone()),
    ] {
        let (_, v) = call(&t, path, &a, body);
        seen.push(v.to_string());
    }
    let mut p = HashMap::new();
    p.insert("name".to_string(), "STRIPE_KEY".to_string());
    for path in [LIST, SHOW, STATUS, DOCTOR, REQUESTS, AUDIT] {
        let (_, v) = call_p(&t, path, &a, Value::Null, p.clone());
        seen.push(v.to_string());
        let (_, v) = call_p(&t, path, &owner(), Value::Null, p.clone());
        seen.push(v.to_string());
    }
    for s in &seen {
        assert!(!s.contains(SENTINEL), "a response leaked the value: {s}");
        assert!(!s.contains(&b64(SENTINEL)), "a response leaked the value (base64): {s}");
    }
    // Nor the audit log.
    let rows = t.v.audit(&Default::default(), &Actor::Owner).expect("audit");
    assert!(!serde_json::to_string(&rows).expect("json").contains(SENTINEL));
}

#[test]
fn values_on_the_url_are_refused() {
    let t = vault();
    let mut p = HashMap::new();
    p.insert("value".to_string(), "x".to_string());
    let (s, v) = call_p(&t, SET, &owner(), json!({ "name": "A", "valueB64": b64("abcdefgh") }), p);
    assert_eq!(s, 400);
    assert_eq!(code(&v), "value_on_argv");
}

#[test]
fn passports_are_refused_on_every_person_route() {
    let t = vault();
    for c in [agent("ws1", true), shell("ws1")] {
        for path in PASSPORT_DENIED {
            let (s, v) = call(&t, path, &c, json!({}));
            assert_eq!(s, 403, "{path} must refuse passports: {v}");
            assert_eq!(code(&v), "owner_only", "{path}");
        }
    }
    // And the agent-verb gate itself never lets them through.
    for path in PASSPORT_DENIED {
        assert!(!crate::session_token::is_agent_verb(path), "{path} must not be an agent verb");
    }
    for path in [STATUS, LIST, SHOW, SET, ROTATE, POLICY, GRANT, REVOKE, DISABLE, ENABLE, DESTROY, GET, AUDIT, REQUESTS, DOCTOR] {
        assert!(crate::session_token::is_agent_verb(path), "{path} should be an agent verb");
    }
}

#[test]
fn reveal_needs_a_single_use_confirm_bound_to_the_secret() {
    let t = vault();
    set_server(&t, "A", SENTINEL);
    set_server(&t, "B", "other-value-123");
    let (s, v) = call(&t, REVEAL, &owner(), json!({ "name": "A", "scope": "server" }));
    assert_eq!(s, 403, "{v}");
    assert_eq!(code(&v), "vault_confirm_required");
    // No confirm method yet → set the vault password from a person channel.
    let (s, v) = call(&t, CONFIRM_START, &owner(), json!({ "action": "reveal" }));
    if cfg!(target_os = "macos") {
        assert_eq!(s, 200, "{v}");
    } else {
        assert_eq!(code(&v), "no_confirm_method");
    }
    let (s, v) = call(&t, CONFIRM_PASSWORD_SET, &owner(), json!({ "password": "correct horse battery" }));
    assert_eq!(s, 403, "owner token over TCP isn't a person: {v}");
    assert_eq!(code(&v), "person_required");
    let (s, v) = call(&t, CONFIRM_PASSWORD_SET, &owner_tty(), json!({ "password": "correct horse battery" }));
    assert_eq!(s, 200, "{v}");
    let a_id = t.v.meta(&k2_vault::Selector::user(k2_vault::Scope::server(), "A"), &Actor::Owner).expect("meta").id;
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "reveal", "secretId": a_id, "method": "vault-password", "password": "wrong password!!" }));
    assert_eq!(s, 403, "{v}");
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "reveal", "secretId": a_id, "method": "vault-password", "password": "correct horse battery" }));
    assert_eq!(s, 200, "{v}");
    let tok = v["token"].as_str().expect("token").to_string();
    // Bound to A: B is refused (and the token burns).
    let (s, _) = call(&t, REVEAL, &owner(), json!({ "name": "B", "scope": "server", "confirmToken": tok }));
    assert_eq!(s, 403);
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "reveal", "secretId": a_id, "method": "vault-password", "password": "correct horse battery" }));
    assert_eq!(s, 200, "{v}");
    let tok = v["token"].as_str().expect("token").to_string();
    let (s, v) = call(&t, REVEAL, &owner(), json!({ "name": "A", "scope": "server", "confirmToken": tok }));
    assert_eq!(s, 200, "{v}");
    let got = base64::engine::general_purpose::STANDARD.decode(v["valueB64"].as_str().expect("b64")).expect("decode");
    assert_eq!(got, SENTINEL.as_bytes());
    let (s, _) = call(&t, REVEAL, &owner(), json!({ "name": "A", "scope": "server", "confirmToken": tok }));
    assert_eq!(s, 403, "single use");
}

#[test]
fn recovery_is_for_people_even_on_a_fresh_vault() {
    let t = vault();
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner(), json!({}));
    assert_eq!(s, 403, "{v}");
    assert_eq!(code(&v), "person_required");
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &agent("ws1", true), json!({}));
    assert_eq!(s, 403, "{v}");
    let r = handle_on(&t.v, RECOVERY_SHOW_ONCE, Ok(owner()), b"{}", &HashMap::new(), "vault.sock", Some("runs inside K2 (a K2 terminal or agent)"));
    assert!(r.status.starts_with("403"), "{}", r.body);
    assert!(r.body.contains("runs inside K2"));
    // A person over the vault socket.
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner_tty(), json!({}));
    assert_eq!(s, 200, "{v}");
    let phrase = v["phrase"].as_str().expect("phrase").to_string();
    let words: Vec<&str> = phrase.split(' ').collect();
    let (s, v) = call(&t, RECOVERY_CONFIRM, &owner_tty(), json!({ "id": v["id"], "words": [words[2], words[10], words[19]] }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(t.v.status().recovery, "confirmed");
    // A second show-once doesn't silently replace it.
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner_tty(), json!({}));
    assert_eq!(s, 409, "{v}");
    // A Connect owner login is a person too.
    let (s, v) = call(&t, RECOVERY_ROTATE, &login(Role::Owner), json!({}));
    assert_eq!(s, 403, "rotate needs a confirm: {v}");
}

#[test]
fn it_agents_share_server_secrets_with_others_only() {
    let t = vault();
    set_server(&t, "OPENAI", "k2test-0123456789abcdef");
    let it = agent("it-ws", true);
    // Workspace names resolve through the DB; use the id form the
    // handler accepts for an agent's own and another workspace.
    let g = |to: &str| json!({ "name": "OPENAI", "scope": "server", "to": to });
    let (s, v) = call(&t, GRANT, &it, g("session:sess-9"));
    assert_eq!(s, 403, "own session: {v}");
    assert_eq!(code(&v), "vault_self_grant");
    let (s, v) = call(&t, GRANT, &it, g("ws:*"));
    assert_eq!(s, 403, "{v}");
    // A plain agent can't even see the server secret.
    let (s, v) = call(&t, GRANT, &agent("plain", false), g("ws:*"));
    assert_eq!(s, 404, "{v}");
}

#[test]
fn agents_cannot_widen_person_given_secrets_over_the_routes() {
    let t = vault();
    let (s, v) = call(&t, SET, &owner(), json!({ "name": "STRIPE", "scope": "server", "hosts": ["api.stripe.com"], "valueB64": b64(SENTINEL) }));
    assert_eq!(s, 200, "{v}");
    let it = agent("it", true);
    let (s, v) = call(&t, POLICY, &it, json!({ "name": "STRIPE", "scope": "server", "addHosts": ["evil.example"] }));
    assert_eq!(s, 403, "{v}");
    assert_eq!(code(&v), "vault_owner_confirm_required");
    let (s, v) = call(&t, POLICY, &it, json!({ "name": "STRIPE", "scope": "server", "addHosts": ["*.stripe.com"] }));
    assert_eq!(s, 400, "{v}");
    assert_eq!(code(&v), "wildcard_not_allowed");
    // The owner token keeps owner rights in 0.45.2 (VS1 deferred): audited.
    let (s, v) = call(&t, POLICY, &owner(), json!({ "name": "STRIPE", "scope": "server", "addHosts": ["files.stripe.com"] }));
    assert_eq!(s, 200, "{v}");
    // A Member can't widen.
    let (s, v) = call(&t, POLICY, &login(Role::Member), json!({ "name": "STRIPE", "scope": "server", "addHosts": ["x.stripe.com"] }));
    assert_eq!(s, 403, "{v}");
}

#[test]
fn get_is_for_agents_with_an_api_grant_only() {
    let t = vault();
    let a = agent("ws1", false);
    let (s, v) = call(&t, SET, &a, json!({ "name": "TOKEN", "hosts": ["api.example.com"], "valueB64": b64("agent-made-token-123") }));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(&t, GET, &a, json!({ "name": "TOKEN" }));
    assert_eq!(s, 403, "fetch-only: {v}");
    // Its own secret: it may raise delivery to api, then read it.
    let (s, v) = call(&t, POLICY, &a, json!({ "name": "TOKEN", "ceiling": "api", "delivery": "api" }));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(&t, GET, &a, json!({ "name": "TOKEN" }));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(&t, GET, &owner(), json!({ "name": "TOKEN", "scope": "server" }));
    assert_eq!(s, 403, "people use reveal: {v}");
    assert_eq!(code(&v), "use_reveal");
}

#[test]
fn key_operations_need_the_owner_and_a_confirm() {
    let t = vault();
    let (s, v) = call(&t, REKEY, &owner(), json!({}));
    assert_eq!(s, 403, "{v}");
    let (s, v) = call(&t, KEK_ROTATE, &login(Role::Admin), json!({}));
    assert_eq!(s, 403, "{v}");
    assert_eq!(code(&v), "owner_only");
}

#[test]
fn status_tells_people_how_to_confirm_recovery_and_is_honest() {
    let t = vault();
    let (s, v) = call(&t, STATUS, &agent("ws1", false), Value::Null);
    assert_eq!(s, 200);
    assert_eq!(v["recovery"], "unconfirmed");
    assert!(v["recoveryHint"].as_str().expect("hint").contains("Agents: don't run it"));
    assert_eq!(v["keyHome"], "fake");
    assert!(v["honest"].as_str().expect("honest").contains("never shown to guests"));
}

#[test]
fn local_auth_confirm_uses_the_fake_in_tests() {
    let t = vault();
    test_hooks::set_local_auth_fake(Some(|_r| Ok(())));
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "rekey", "method": "local-auth" }));
    test_hooks::set_local_auth_fake(None);
    assert_eq!(s, 200, "{v}");
    let tok = v["token"].as_str().expect("token");
    let (s, v) = call(&t, REKEY, &owner(), json!({ "confirmToken": tok }));
    assert_eq!(s, 200, "{v}");
}
