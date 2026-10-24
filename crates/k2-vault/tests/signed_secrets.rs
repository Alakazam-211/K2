//! Item 2 (PRD vault-gaps): signed secrets in the store: value checks on
//! put/rotate/policy, defaults, the loosening rule, MAC round trip, audit.

use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::hosts::Origin;
use k2_vault::inject::{Enc, Inject, Sign};
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::*;

const OVH: &str = r#"{"applicationKey":"k2testApp","applicationSecret":"k2testSecretValue","consumerKey":"k2testConsumer"}"#;

fn open(t: &TestDir, ks: &Arc<FakeKeyStore>) -> LocalVault {
    LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open vault")
}

fn agent(ws: &str) -> Actor {
    Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("sess-1".into()), shell: false, vault_manage: false })
}

fn signed(sign: Sign) -> Inject {
    Inject { sign: Some(sign), ..Inject::bearer() }
}

fn put_ovh(v: &LocalVault, name: &str, value: &str, actor: &Actor) -> Result<PutOutcome> {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), name, Kind::Generic);
    r.hosts = vec!["https://eu.api.ovh.com:443".into()];
    r.inject = Some(signed(Sign::Ovh));
    v.put(r, SecretBytes::from_slice(value.as_bytes()), actor)
}

#[test]
fn an_ovh_secret_needs_its_json_and_defaults_to_fetch() {
    let t = TestDir::new("sign-put");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let e = put_ovh(&v, "OVH_BAD", r#"{"applicationKey":"a","applicationSecret":"b"}"#, &Actor::Owner).expect_err("no consumerKey");
    assert_eq!(e.code, "bad_value");
    assert!(!e.hint.contains("\"b\""), "never the value: {}", e.hint);
    let out = put_ovh(&v, "OVH_EU", OVH, &Actor::Owner).expect("put");
    assert_eq!(out.meta.default_delivery, Delivery::Fetch);
    assert_eq!(out.meta.delivery_ceiling, Delivery::Fetch);
    assert_eq!(out.meta.inject.as_ref().and_then(|i| i.sign.clone()), Some(Sign::Ovh));
    // Rotate checks the new value too.
    let sel = Selector::user(Scope::workspace("ws1"), "OVH_EU");
    assert_eq!(v.rotate(&sel, SecretBytes::from_slice(b"not json"), &Actor::Owner).expect_err("rotate").code, "bad_value");
    v.rotate(&sel, SecretBytes::from_slice(OVH.as_bytes()), &Actor::Owner).expect("good rotate");
}

#[test]
fn a_signed_secret_survives_reopen_and_its_fetch_audit_names_the_scheme() {
    let t = TestDir::new("sign-mac");
    let ks = Arc::new(FakeKeyStore::new());
    {
        let v = open(&t, &ks);
        put_ovh(&v, "OVH_EU", OVH, &Actor::Owner).expect("put");
    }
    let v = open(&t, &ks);
    assert_eq!(v.status().tamper, "ok");
    let sel = Selector::user(Scope::workspace("ws1"), "OVH_EU");
    let (_, origin) = Origin::parse_url("https://eu.api.ovh.com/1.0/me").expect("url");
    let who = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    let r = v.resolve(&sel, Use::Fetch(&origin), &who).expect("resolve");
    assert_eq!(r.inject.sign, Some(Sign::Ovh));
    // Agents can't choose where it goes.
    assert_eq!(r.inject.choose(Some("query:x"), None).expect_err("dst").code, "inject_not_allowed");
    v.flush_rollups();
    let rows = v.audit(&AuditQuery { secret_id: Some(r.meta.id.clone()), since: None, workspace_id: None, limit: None }, &Actor::Owner).expect("audit");
    assert!(rows.iter().any(|a| a.action == "fetch" && a.outcome == "ok:sign=ovh"), "{rows:?}");
}

#[test]
fn changing_the_signer_is_loosening_and_checks_the_stored_value() {
    let t = TestDir::new("sign-policy");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    // A person's plain key: an agent may not add or change a signer.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "HOOK", Kind::HmacKey);
    r.hosts = vec!["hooks.example.com".into()];
    v.put(r, SecretBytes::from_slice(b"k2test-hook-key"), &Actor::Owner).expect("put");
    let sel = Selector::user(Scope::workspace("ws1"), "HOOK");
    let hmac = Sign::HmacSha256 { header: "X-Hub-Signature-256".into(), encoding: Enc::Hex, prefix: "sha256=".into() };
    let patch = |i: Inject| PolicyPatch { inject: Some(i), ..Default::default() };
    let e = v.set_policy(&sel, patch(signed(hmac.clone())), &agent("ws1")).expect_err("agent");
    assert_eq!(e.code, "vault_owner_confirm_required");
    let m = v.set_policy(&sel, patch(signed(hmac.clone())), &Actor::Owner).expect("owner");
    assert_eq!(m.inject.and_then(|i| i.sign), Some(hmac));
    // OVH needs the stored value to be the OVH JSON.
    let e = v.set_policy(&sel, patch(signed(Sign::Ovh)), &Actor::Owner).expect_err("not ovh json");
    assert_eq!(e.code, "bad_value");
    // Removing the signer is also the owner's call for a person's secret.
    let e = v.set_policy(&sel, patch(Inject::bearer()), &agent("ws1")).expect_err("agent removes");
    assert_eq!(e.code, "vault_owner_confirm_required");
}
