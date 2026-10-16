//! Route-level tests for `/cli/vault/*` (V2 must-haves, §19a + §23.7).
//! Temp vault + fake key store; never the real home or keychain.

use std::collections::HashMap;
use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{Actor, AgentActor, KeyStore, LocalVault, OpenOptions, Role};
use serde_json::{json, Value};

// Also brings the parent's `base64::Engine as _` (encode/decode below); a
// second import of it here is unused to rustc 1.99 (Linux gate).
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

/// 0.45.4 blocker: Settings → Vault (the K2 app, owner token over TCP)
/// couldn't set up the recovery key at all. A person's confirm (the Mac's
/// sheet or the vault password) now stands in for the person channel.
#[test]
fn the_app_sets_up_the_recovery_key_after_a_confirm() {
    let t = vault();
    // Without a confirm the app is still refused (VS2), and says where to go.
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner(), json!({}));
    assert_eq!(s, 403, "{v}");
    assert_eq!(code(&v), "person_required");
    assert!(v["error"]["hint"].as_str().expect("hint").contains("Settings → Vault"), "{v}");
    // A vault password set by a person (no global LA fake: tests run in parallel).
    let (s, v) = call(&t, CONFIRM_PASSWORD_SET, &owner_tty(), json!({ "password": "correct horse battery" }));
    assert_eq!(s, 200, "{v}");
    // A confirm for another action doesn't count.
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "rekey", "method": "vault-password", "password": "correct horse battery" }));
    assert_eq!(s, 200, "{v}");
    let wrong = v["token"].as_str().expect("token").to_string();
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner(), json!({ "confirmToken": wrong }));
    assert_eq!(s, 403, "{v}");
    assert_eq!(code(&v), "vault_confirm_required");
    // The app's confirm for recovery setup.
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "recovery-setup", "method": "vault-password", "password": "correct horse battery" }));
    assert_eq!(s, 200, "recovery-setup is a confirm action: {v}");
    let tok = v["token"].as_str().expect("token").to_string();
    // Agents never get here, token or not.
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &agent("ws1", true), json!({ "confirmToken": tok }));
    assert_eq!(s, 403, "{v}");
    assert_eq!(code(&v), "owner_only");
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner(), json!({ "confirmToken": tok }));
    assert_eq!(s, 200, "{v}");
    let phrase = v["phrase"].as_str().expect("phrase").to_string();
    let words: Vec<&str> = phrase.split(' ').collect();
    assert_eq!(words.len(), 24);
    assert_eq!(v["checkWords"], json!([3, 11, 20]));
    // Single use: the same token can't show a second phrase.
    let (s, _) = call(&t, RECOVERY_SHOW_ONCE, &owner(), json!({ "confirmToken": tok }));
    assert_eq!(s, 403, "single use");
    // Wrong words: refused, still unconfirmed.
    let (s, v2) = call(&t, RECOVERY_CONFIRM, &owner(), json!({ "id": v["id"], "words": ["a", "b", "c"] }));
    assert_eq!(s, 400, "{v2}");
    assert_eq!(t.v.status().recovery, "unconfirmed");
    // Start again and type the right words back, from the app.
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "recovery-setup", "method": "vault-password", "password": "correct horse battery" }));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner(), json!({ "confirmToken": v["token"] }));
    assert_eq!(s, 200, "{v}");
    let phrase = v["phrase"].as_str().expect("phrase").to_string();
    let words: Vec<&str> = phrase.split(' ').collect();
    let (s, v) = call(&t, RECOVERY_CONFIRM, &owner(), json!({ "id": v["id"], "words": [words[2], words[10], words[19]] }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["recovery"], "confirmed");
    assert_eq!(t.v.status().recovery, "confirmed");
    // Once confirmed, show-once doesn't replace it; rotate needs its own confirm.
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "recovery-setup", "method": "vault-password", "password": "correct horse battery" }));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner(), json!({ "confirmToken": v["token"] }));
    assert_eq!(s, 409, "{v}");
    assert_eq!(code(&v), "recovery_exists");
    let (s, v) = call(&t, CONFIRM_FINISH, &owner(), json!({ "action": "recovery-rotate", "method": "vault-password", "password": "correct horse battery" }));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(&t, RECOVERY_ROTATE, &owner(), json!({ "confirmToken": v["token"] }));
    assert_eq!(s, 200, "rotate from the app after a confirm: {v}");
}

#[test]
fn an_agent_with_the_owner_token_cannot_finish_someone_elses_recovery_without_the_words() {
    let t = vault();
    // A person starts on the vault socket; the raw owner token can't
    // confirm without the words only that person saw.
    let (s, v) = call(&t, RECOVERY_SHOW_ONCE, &owner_tty(), json!({}));
    assert_eq!(s, 200, "{v}");
    let (s, v2) = call(&t, RECOVERY_CONFIRM, &owner(), json!({ "id": v["id"], "words": ["abandon", "abandon", "abandon"] }));
    assert_eq!(s, 400, "{v2}");
    assert_eq!(t.v.status().recovery, "unconfirmed");
    // An unknown id is refused.
    let (s, v2) = call(&t, RECOVERY_CONFIRM, &owner(), json!({ "id": "nope", "words": ["a", "b", "c"] }));
    assert_eq!(s, 409, "{v2}");
    // Passports never reach confirm.
    let (s, v2) = call(&t, RECOVERY_CONFIRM, &agent("ws1", true), json!({ "id": v["id"], "words": ["a", "b", "c"] }));
    assert_eq!(s, 403, "{v2}");
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

#[test]
fn moving_secrets_needs_an_admin_or_an_agent_allowed_to_manage() {
    let t = vault();
    use crate::vault_use_routes::RECIPIENT;
    let (s, v) = call(&t, RECIPIENT, &agent("ws1", false), Value::Null);
    assert_eq!(s, 403, "{v}");
    let (s, v) = call(&t, RECIPIENT, &shell("ws1"), Value::Null);
    assert_eq!(s, 403, "{v}");
    let (s, v) = call(&t, RECIPIENT, &login(Role::Member), Value::Null);
    assert_eq!(s, 403, "{v}");
    let (s, v) = call(&t, RECIPIENT, &agent("ws1", true), Value::Null);
    assert_eq!(s, 200, "{v}");
    assert!(v["recipient"].as_str().expect("recipient").starts_with("age1"));
    let (s, v) = call(&t, RECIPIENT, &login(Role::Admin), Value::Null);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["vaultId"].as_str().expect("vaultId"), t.v.vault_id());
}

#[test]
fn a_carry_opens_only_in_the_target_vault_and_lists_before_applying() {
    use crate::vault_use_routes::{CARRY_EXPORT, CARRY_IMPORT, RECIPIENT};
    let src = vault();
    let dst = vault();
    let other = vault();
    set_server(&src, "STRIPE_KEY", SENTINEL);
    let (s, r) = call(&dst, RECIPIENT, &owner(), Value::Null);
    assert_eq!(s, 200, "{r}");
    let body = json!({ "targetRecipient": r["recipient"], "targetVaultId": r["vaultId"], "server": ["STRIPE_KEY"] });
    let (s, ex) = call(&src, CARRY_EXPORT, &owner(), body);
    assert_eq!(s, 200, "{ex}");
    assert_eq!(ex["count"], 1);
    assert!(!ex.to_string().contains(SENTINEL), "export response carries ciphertext only");
    let carry = ex["carryB64"].clone();
    // Another vault can't open it.
    let (s, v) = call(&other, CARRY_IMPORT, &owner(), json!({ "carryB64": carry }));
    assert_ne!(s, 200, "{v}");
    // Dry run first: names only, nothing written.
    let (s, v) = call(&dst, CARRY_IMPORT, &owner(), json!({ "carryB64": carry }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["dryRun"], true);
    assert_eq!(v["secrets"][0]["name"], "STRIPE_KEY");
    assert!(!v.to_string().contains(SENTINEL));
    assert_eq!(dst.v.user_secret_count(), 0);
    let (s, v) = call(&dst, CARRY_IMPORT, &owner(), json!({ "carryB64": carry, "apply": true }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["imported"][0], "STRIPE_KEY");
    assert_eq!(dst.v.user_secret_count(), 1);
    // A second apply keeps what's there.
    let (s, v) = call(&dst, CARRY_IMPORT, &owner(), json!({ "carryB64": carry, "apply": true }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["imported"].as_array().expect("imported").len(), 0);
    // An agent without the switch can't import.
    let (s, v) = call(&dst, CARRY_IMPORT, &agent("ws1", false), json!({ "carryB64": carry }));
    assert_eq!(s, 403, "{v}");
}

/// CL12 regression: `k2 vault carry import` picks the target workspace by
/// id. A different workspace on the target with the same name never gets
/// the secrets: not by default, and not when a person names it.
#[test]
fn a_carry_import_matches_the_workspace_by_id_never_by_name() {
    use crate::vault_use_routes::{CARRY_EXPORT, CARRY_IMPORT, RECIPIENT};
    use k2_vault::{Kind, ListFilter, Namespace, PutRequest, Scope, ScopeKind, SecretBytes, SecretStore};
    crate::test_support::with_temp_home(|| {
        let root = TestDir::new("carry-by-id");
        let folder = format!("ca-sales-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let register = |parent: &str| {
            let p = root.path().join(parent).join(&folder);
            std::fs::create_dir_all(&p).expect("mkdir");
            let canon = std::fs::canonicalize(&p).expect("canon").to_string_lossy().to_string();
            k2_core::projects_ops::projects_add_without_git(&canon).expect("register")
        };
        // Same folder name three times: an unrelated workspace that will be
        // on the target box, the agent on the old box, and its clone.
        let other = register("other");
        let source = register("source");
        let clone = register("clone");
        let other_name = crate::vault::workspace_label(&other.id);
        let clone_name = crate::vault::workspace_label(&clone.id);
        assert_eq!(crate::vault::workspace_id(&other_name).as_deref(), Some(other.id.as_str()), "the name token names the unrelated workspace");

        let src = vault();
        let mut r = PutRequest::new(Namespace::User, Scope::workspace(source.id.clone()), "STRIPE_KEY", Kind::ApiKey);
        r.hosts = vec!["https://api.stripe.com".into()];
        src.v.put(r, SecretBytes::from_slice(SENTINEL.as_bytes()), &Actor::Owner).expect("put");

        let in_scope = |t: &T, id: &str| -> usize {
            t.v.list(&ListFilter::default(), &Actor::System("test".into()))
                .expect("list")
                .into_iter()
                .filter(|m| m.namespace == Namespace::User && m.scope.kind == ScopeKind::Workspace && m.scope.id == id)
                .count()
        };
        let export = |dst: &T, target_ws: Option<&str>| -> Value {
            let (s, r) = call(dst, RECIPIENT, &owner(), Value::Null);
            assert_eq!(s, 200, "{r}");
            let mut body = json!({ "targetRecipient": r["recipient"], "targetVaultId": r["vaultId"], "workspaces": [source.path.clone()] });
            if let Some(t) = target_ws {
                body["targetWorkspaceId"] = json!(t);
            }
            let (s, ex) = call(&src, CARRY_EXPORT, &owner(), body);
            assert_eq!(s, 200, "{ex}");
            assert_eq!(ex["count"], 1, "{ex}");
            assert!(!ex.to_string().contains(SENTINEL), "export response carries ciphertext only");
            ex["carryB64"].clone()
        };
        let dst = vault();
        let dst_bound = vault();
        let carry = export(&dst, None);
        let bound = export(&dst_bound, Some(&clone.id));
        // The target box: the source workspace isn't there.
        k2_core::projects_ops::projects_delete(&source.id).expect("drop the source row");

        // 1. Nothing named: the source id isn't here, so nothing lands (the
        //    old label match would have put it into `other`).
        let (s, v) = call(&dst, CARRY_IMPORT, &owner(), json!({ "carryB64": carry, "apply": true }));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["imported"].as_array().expect("imported").len(), 0, "{v}");
        assert!(v["skipped"][0].as_str().expect("skip line").contains(&source.id), "{v}");
        assert_eq!(in_scope(&dst, &other.id), 0, "never the same-named workspace");

        // 2. A name alone is refused: the carry wasn't made for that id.
        for apply in [false, true] {
            let (s, v) = call(&dst, CARRY_IMPORT, &owner(), json!({ "carryB64": carry, "apply": apply, "workspace": other_name }));
            assert_eq!(s, 403, "{v}");
            assert_eq!(code(&v), "carry_workspace_by_name", "{v}");
            assert!(v["error"]["hint"].as_str().unwrap_or("").contains("--workspace"), "the refusal teaches the id flag: {v}");
        }
        assert_eq!(in_scope(&dst, &other.id), 0, "never the same-named workspace");

        // 3. By id: the dry run shows the resolved target id, then it lands.
        let (s, v) = call(&dst, CARRY_IMPORT, &owner(), json!({ "carryB64": carry, "workspace": clone.id }));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["dryRun"], true);
        assert_eq!(v["secrets"][0]["targetWorkspaceId"], json!(clone.id), "{v}");
        assert_eq!(v["secrets"][0]["sourceWorkspaceId"], json!(source.id), "{v}");
        assert_eq!(in_scope(&dst, &clone.id), 0, "a dry run writes nothing");
        let (s, v) = call(&dst, CARRY_IMPORT, &owner(), json!({ "carryB64": carry, "apply": true, "workspace": clone.id }));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["imported"], json!(["STRIPE_KEY"]), "{v}");
        assert!(!v.to_string().contains(SENTINEL));
        assert_eq!(in_scope(&dst, &clone.id), 1);
        assert_eq!(in_scope(&dst, &other.id), 0, "never the same-named workspace");

        // 4. A carry made for the clone's id: refused for another id or
        //    another workspace's name; the clone's own name is fine.
        let (s, v) = call(&dst_bound, CARRY_IMPORT, &owner(), json!({ "carryB64": bound, "apply": true, "workspace": other.id }));
        assert_eq!(s, 403, "{v}");
        assert_eq!(code(&v), "carry_for_another_workspace", "{v}");
        let (s, v) = call(&dst_bound, CARRY_IMPORT, &owner(), json!({ "carryB64": bound, "apply": true, "workspace": other_name }));
        assert_eq!(s, 403, "{v}");
        assert_eq!(code(&v), "carry_workspace_by_name", "{v}");
        assert_eq!(in_scope(&dst_bound, &other.id), 0, "never the same-named workspace");
        let (s, v) = call(&dst_bound, CARRY_IMPORT, &owner(), json!({ "carryB64": bound, "apply": true, "workspace": clone_name }));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["imported"], json!(["STRIPE_KEY"]), "{v}");
        assert_eq!(in_scope(&dst_bound, &clone.id), 1);
        assert_eq!(in_scope(&dst_bound, &other.id), 0, "never the same-named workspace");

        k2_core::projects_ops::projects_delete(&other.id).expect("cleanup");
        k2_core::projects_ops::projects_delete(&clone.id).expect("cleanup");
    });
}

/// H21 follow-up (Rosson 2026-10-16): the switch is the gate. Without
/// "Allow managing server-wide secrets" no agent or shell tab reaches any
/// carry route (403 → `k2` exit 3), and nothing is sealed or written.
#[test]
fn every_carry_route_refuses_an_agent_without_the_switch() {
    use crate::vault_use_routes::{CARRY_EXPORT, CARRY_IMPORT, RECIPIENT};
    crate::test_support::with_temp_home(|| {
        let src = vault();
        let dst = vault();
        set_server(&src, "STRIPE_KEY", SENTINEL);
        let (s, r) = call(&dst, RECIPIENT, &owner(), Value::Null);
        assert_eq!(s, 200, "{r}");
        let export = json!({ "targetRecipient": r["recipient"], "targetVaultId": r["vaultId"], "server": ["STRIPE_KEY"] });
        let (s, ex) = call(&src, CARRY_EXPORT, &owner(), export.clone());
        assert_eq!(s, 200, "{ex}");
        let carry = ex["carryB64"].clone();
        for c in [agent("ws1", false), shell("ws1"), login(Role::Member)] {
            for (t, path, body) in [
                (&src, RECIPIENT, Value::Null),
                (&src, CARRY_EXPORT, export.clone()),
                (&dst, CARRY_IMPORT, json!({ "carryB64": carry })),
                (&dst, CARRY_IMPORT, json!({ "carryB64": carry, "apply": true })),
            ] {
                let (s, v) = call(t, path, &c, body);
                assert_eq!(s, 403, "{path} must refuse {:?}: {v}", c.actor);
                assert!(v.get("carryB64").is_none(), "{path} sealed something for {:?}: {v}", c.actor);
                assert!(!v.to_string().contains(SENTINEL), "{path} leaked the value: {v}");
            }
        }
        assert_eq!(dst.v.user_secret_count(), 0, "a refused import wrote secrets");
        let rows = src.v.audit(&Default::default(), &Actor::Owner).expect("audit");
        let stray: Vec<_> = rows.iter().filter(|r| r.action == "carry-export" && r.actor != "owner").collect();
        assert!(stray.is_empty(), "a refused export wrote carry rows: {stray:?}");
    });
}

/// H21 follow-up: an agent with the switch carries with no person and no
/// confirm, and the owner sees who, from which workspace and session, which
/// secrets (names only), to which key, and when: in the vault audit and in
/// the auth audit (Settings → Access).
#[test]
fn an_agent_with_the_switch_carries_and_the_owner_sees_it() {
    use crate::vault_use_routes::{CARRY_EXPORT, CARRY_IMPORT, RECIPIENT};
    crate::test_support::with_temp_home(|| {
        let src = vault();
        let dst = vault();
        set_server(&src, "STRIPE_KEY", SENTINEL);
        set_server(&src, "OPENAI_KEY", "k2test_CARRY_OPENAI_77aa66bb55cc");
        set_server(&src, "LEFT_HOME", "k2test_CARRY_LEFT_88dd99ee00ff");
        let it = agent("ws1", true);
        let (s, r) = call(&dst, RECIPIENT, &it, Value::Null);
        assert_eq!(s, 200, "{r}");
        let fp = r["fingerprint"].as_str().expect("fingerprint").to_string();
        let vid = r["vaultId"].as_str().expect("vaultId").to_string();
        let (s, ex) = call(&src, CARRY_EXPORT, &it, json!({ "targetRecipient": r["recipient"], "targetVaultId": vid, "server": ["STRIPE_KEY", "OPENAI_KEY"] }));
        assert_eq!(s, 200, "an agent with the switch carries without a confirm: {ex}");
        assert_eq!(ex["count"], 2, "{ex}");

        // Vault audit: one row per secret plus a summary, all the agent's.
        let ids: HashMap<String, String> = src
            .v
            .list(&k2_vault::ListFilter::default(), &Actor::Owner)
            .expect("list")
            .into_iter()
            .map(|m| (m.name.clone(), m.id.clone()))
            .collect();
        let rows = src.v.audit(&Default::default(), &Actor::Owner).expect("audit");
        let carry: Vec<_> = rows.iter().filter(|r| r.action == "carry-export").collect();
        assert_eq!(carry.len(), 3, "two secrets + a summary: {carry:?}");
        for row in &carry {
            assert_eq!(row.actor, "agent:ws1/sess-9", "{row:?}");
            assert_eq!(row.consumer.as_deref(), Some(format!("carry:{fp}@{vid}").as_str()), "{row:?}");
            assert_eq!(row.outcome, "ok", "{row:?}");
            assert!(!row.ts.is_empty(), "{row:?}");
        }
        let mut per: Vec<&str> = carry.iter().filter_map(|r| r.secret_id.as_deref()).collect();
        per.sort_unstable();
        let mut want = vec![ids["STRIPE_KEY"].as_str(), ids["OPENAI_KEY"].as_str()];
        want.sort_unstable();
        assert_eq!(per, want, "per-secret rows name exactly what was carried");
        let summary: Vec<_> = carry.iter().filter(|r| r.secret_id.is_none()).collect();
        assert_eq!(summary.len(), 1, "{carry:?}");
        assert_eq!(summary[0].count, 2, "{:?}", summary[0]);
        assert!(!serde_json::to_string(&rows).expect("json").contains(SENTINEL), "the audit holds a value");

        // Auth audit (Settings → Access): one owner-readable line.
        let log = k2_core::auth_audit::tail(50).expect("auth audit");
        let line = log.iter().rev().find(|e| e["event"] == "vault-carry").unwrap_or_else(|| panic!("no vault-carry event in {log:?}"));
        let out = line["outcome"].as_str().expect("outcome");
        for want in ["ok export 2", "STRIPE_KEY", "OPENAI_KEY", "by agent:ws1/sess-9", "(workspace ws1)", fp.as_str(), vid.as_str()] {
            assert!(out.contains(want), "auth audit outcome lacks {want:?}: {out}");
        }
        assert!(!out.contains("LEFT_HOME"), "named a secret that wasn't carried: {out}");
        assert!(!out.contains(SENTINEL), "auth audit holds a value: {out}");
        assert!(line["ts"].as_str().is_some_and(|t| !t.is_empty()), "{line}");

        // And the target box takes it from the agent too.
        let (s, v) = call(&dst, CARRY_IMPORT, &agent("ws2", true), json!({ "carryB64": ex["carryB64"], "apply": true }));
        assert_eq!(s, 200, "{v}");
        assert_eq!(dst.v.user_secret_count(), 2);
    });
}

/// People keep what they had: the owner and an Admin login carry with no
/// confirm (the H21 audit row names them).
#[test]
fn people_still_carry_and_are_named_in_the_audit() {
    use crate::vault_use_routes::{CARRY_EXPORT, RECIPIENT};
    crate::test_support::with_temp_home(|| {
        let src = vault();
        let dst = vault();
        set_server(&src, "STRIPE_KEY", SENTINEL);
        let (s, r) = call(&dst, RECIPIENT, &owner(), Value::Null);
        assert_eq!(s, 200, "{r}");
        let body = json!({ "targetRecipient": r["recipient"], "targetVaultId": r["vaultId"], "server": ["STRIPE_KEY"] });
        for (c, who) in [(owner(), "owner"), (login(Role::Admin), "connect:lee")] {
            let (s, ex) = call(&src, CARRY_EXPORT, &c, body.clone());
            assert_eq!(s, 200, "{who}: {ex}");
            assert_eq!(ex["count"], 1, "{who}: {ex}");
            let rows = src.v.audit(&Default::default(), &Actor::Owner).expect("audit");
            let summary = rows
                .iter()
                .find(|r| r.action == "carry-export" && r.secret_id.is_none() && r.actor == who)
                .unwrap_or_else(|| panic!("no carry-export summary for {who}: {rows:?}"));
            assert_eq!(summary.count, 1);
        }
    });
}

#[test]
fn restoring_a_backup_is_for_people_and_checks_before_replacing() {
    use crate::vault_use_routes::BACKUP;
    let src = vault();
    set_server(&src, "STRIPE_KEY", SENTINEL);
    let (s, v) = call(&src, RECOVERY_SHOW_ONCE, &owner_tty(), json!({}));
    assert_eq!(s, 200, "{v}");
    let phrase = v["phrase"].as_str().expect("phrase").to_string();
    let words: Vec<&str> = phrase.split(' ').collect();
    let (s, v) = call(&src, RECOVERY_CONFIRM, &owner_tty(), json!({ "id": v["id"], "words": [words[2], words[10], words[19]] }));
    assert_eq!(s, 200, "{v}");
    let out = TestDir::new("backup-out");
    let (s, v) = call(&src, BACKUP, &agent("ws1", true), json!({ "destDir": out.path() }));
    assert_eq!(s, 403, "agents don't take backups over the routes: {v}");
    let (s, v) = call(&src, BACKUP, &owner(), json!({ "destDir": out.path() }));
    assert_eq!(s, 200, "{v}");
    let snap = std::fs::read(out.path().join("vault.db")).expect("snapshot");
    assert!(!snap.windows(SENTINEL.len()).any(|w| w == SENTINEL.as_bytes()));

    let dst = vault();
    let restore = |c: &VaultCaller, body: Value| call(&dst, RESTORE, c, body);
    let base = json!({ "fromDir": out.path(), "holder": "recovery", "phrase": phrase, "dryRun": true });
    let (s, v) = restore(&agent("ws1", true), base.clone());
    assert_eq!(s, 403, "{v}");
    let (s, v) = restore(&owner(), base.clone());
    assert_eq!(s, 403, "a person at a terminal only: {v}");
    let (s, v) = restore(&owner_tty(), base.clone());
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["vaultId"].as_str().expect("vaultId"), src.v.vault_id());
    assert_eq!(v["secrets"], 1);
    let mut wrong = base.clone();
    let mut w2: Vec<&str> = words.clone();
    w2.swap(0, 1);
    wrong["phrase"] = json!(w2.join(" "));
    let (s, v) = restore(&owner_tty(), wrong);
    assert_ne!(s, 200, "{v}");
    // This box already holds a secret: no silent replace.
    set_server(&dst, "OTHER", "x");
    let mut apply = base.clone();
    apply["dryRun"] = json!(false);
    let (s, v) = restore(&owner_tty(), apply);
    assert_eq!(s, 409, "{v}");
    assert_eq!(code(&v), "vault_not_empty");
}

/// The loopback proxy (V5) is held back from this release: a request that
/// names `proxy` as a delivery is refused instead of silently delivering
/// nothing; every other delivery still parses.
#[test]
fn proxy_delivery_is_refused_while_the_proxy_is_held() {
    let e = request_delivery("proxy").expect_err("proxy must be refused");
    assert_eq!(e.code, "delivery_not_available");
    assert_eq!(e.status, 400);
    for d in ["fetch", "file", "env", "api"] {
        assert_eq!(request_delivery(d).expect(d).as_str(), d);
    }
    assert!(request_delivery("nonsense").is_err());
}
