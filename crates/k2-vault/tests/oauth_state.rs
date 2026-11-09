//! A refreshing sign-in on the vault side (prd-vault-for-agents-v1 §6): the
//! delivery ceiling, sealed token storage, what `rotate`, destroy and rekey
//! do to it. The token POST is the daemon's (`vault::oauth_refresh`).

use std::sync::Arc;

use k2_vault::carry::{CarryItem, CarryPayload};
use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::hosts::Origin;
use k2_vault::inject::{ClientAuth, Inject, Sign};
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::*;

const OLD_ACCESS: &str = "k2test_oldaccess_0123456789abcdef";
const OLD_REFRESH: &str = "k2test_oldrefresh_0123456789abcdef";
const NEW_ACCESS: &str = "k2test_newaccess_fedcba9876543210";
const NEW_REFRESH: &str = "k2test_newrefresh_fedcba9876543210";
const CLIENT_SECRET: &str = "k2test_clientsecret_55aa55aa55aa";

fn open(t: &TestDir, ks: &Arc<FakeKeyStore>) -> LocalVault {
    LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open vault")
}

fn agent() -> Requester {
    Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) }
}

fn bundle(access: &str, refresh: &str) -> SecretBytes {
    SecretBytes::from_string(
        serde_json::json!({
            "access_token": access,
            "refresh_token": refresh,
            "expires_at": 4_000_000_000i64,
            "client_id": "client-1",
            "client_secret": CLIENT_SECRET,
        })
        .to_string(),
    )
}

fn signer() -> Inject {
    Inject { sign: Some(Sign::OauthRefresh { token_url: "https://login.example.com/token".into(), client_auth: ClientAuth::Post }), ..Inject::bearer() }
}

fn request() -> PutRequest {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "MAIL_API", Kind::OauthToken);
    r.hosts = vec!["api.example.com".into()];
    r.inject = Some(signer());
    r
}

fn put(v: &LocalVault) -> SecretMeta {
    v.put(request(), bundle(OLD_ACCESS, OLD_REFRESH), &Actor::Owner).expect("put").meta
}

fn sel() -> Selector {
    Selector::user(Scope::workspace("ws1"), "MAIL_API")
}

fn origin() -> Origin {
    Origin::parse_url("https://api.example.com/v1/x").expect("origin").1
}

fn access_of(v: &LocalVault, id: &str) -> Vec<u8> {
    v.oauth_current(id).expect("current").access.expect("access").expose().to_vec()
}

fn refresh_of(v: &LocalVault, id: &str) -> Vec<u8> {
    v.oauth_current(id).expect("current").refresh.expect("refresh").expose().to_vec()
}

fn secret(s: &str) -> SecretBytes {
    SecretBytes::from_slice(s.as_bytes())
}

#[test]
fn its_default_and_ceiling_are_fetch_and_nothing_looser_is_accepted() {
    let t = TestDir::new("oauth-set");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    for d in [Delivery::Env, Delivery::File, Delivery::Api, Delivery::Agent] {
        let mut r = request();
        r.default_delivery = Some(d);
        let e = v.put(r, bundle(OLD_ACCESS, OLD_REFRESH), &Actor::Owner).err().expect("default delivery refused");
        assert_eq!(e.code, "oauth_fetch_only", "{d}");
        let mut r = request();
        r.delivery_ceiling = Some(d);
        let e = v.put(r, bundle(OLD_ACCESS, OLD_REFRESH), &Actor::Owner).err().expect("ceiling refused");
        assert_eq!(e.code, "oauth_fetch_only", "{d}");
    }
    let m = put(&v);
    assert_eq!(m.default_delivery, Delivery::Fetch);
    assert_eq!(m.delivery_ceiling, Delivery::Fetch);
}

#[test]
fn neither_a_policy_change_nor_a_grant_widens_it() {
    let t = TestDir::new("oauth-policy");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    for d in [Delivery::Env, Delivery::File, Delivery::Api] {
        let e = v
            .set_policy(&sel(), PolicyPatch { delivery_ceiling: Some(d), ..Default::default() }, &Actor::Owner)
            .err()
            .expect("ceiling refused");
        assert_eq!(e.code, "oauth_fetch_only");
        let e = v
            .set_policy(&sel(), PolicyPatch { default_delivery: Some(d), delivery_ceiling: Some(d), ..Default::default() }, &Actor::Owner)
            .err()
            .expect("delivery refused");
        assert_eq!(e.code, "oauth_fetch_only");
        let e = v
            .grant(
                GrantRequest {
                    selector: Selector::Id(m.id.clone()),
                    consumer: Consumer::workspace("ws2"),
                    delivery: d,
                    env_name: None,
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: None,
                    version: None,
                },
                &Actor::Owner,
            )
            .err()
            .expect("grant refused");
        assert_eq!(e.code, "delivery_above_ceiling");
    }
    // The signer can't come off: the value would go out as a bearer token.
    let e = v
        .set_policy(&sel(), PolicyPatch { inject: Some(Inject::bearer()), ..Default::default() }, &Actor::Owner)
        .err()
        .expect("signer stays");
    assert_eq!(e.code, "oauth_signer_fixed");
}

/// A signer added to a secret that may already be read can't leave it
/// readable: the delivery must come down first.
#[test]
fn a_readable_secret_cant_become_a_sign_in() {
    let t = TestDir::new("oauth-add-signer");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "MAIL_API", Kind::OauthToken);
    r.hosts = vec!["api.example.com".into()];
    r.default_delivery = Some(Delivery::Env);
    v.put(r, bundle(OLD_ACCESS, OLD_REFRESH), &Actor::Owner).expect("put");
    let e = v.set_policy(&sel(), PolicyPatch { inject: Some(signer()), ..Default::default() }, &Actor::Owner).err().expect("refused");
    assert_eq!(e.code, "oauth_fetch_only");
}

#[test]
fn only_fetch_or_proxy_can_resolve_it_and_never_the_bundle() {
    let t = TestDir::new("oauth-resolve");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put(&v);
    for u in [Use::Env, Use::File, Use::Api, Use::Agent(None)] {
        let e = v.resolve(&sel(), u, &agent()).err().expect("refused");
        assert_eq!(e.code, "oauth_fetch_only");
    }
    let e = v.resolve(&sel(), Use::System, &Requester::System("t".into())).err().expect("system refused");
    assert_eq!(e.code, "oauth_fetch_only");

    let o = origin();
    let r = v.resolve(&sel(), Use::Fetch(&o), &agent()).expect("fetch resolves");
    assert!(r.value.is_empty(), "the bundle holds the refresh token; resolve must not return it");
    assert!(matches!(r.inject.sign, Some(Sign::OauthRefresh { .. })));
    let r = v.resolve(&sel(), Use::Proxy(&o), &agent()).expect("proxy resolves");
    assert!(r.value.is_empty());
}

#[test]
fn a_refresh_is_stored_sealed_and_is_not_a_new_version() {
    let t = TestDir::new("oauth-store");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    assert!(v.oauth_status(&m.id).expect("status").is_none(), "no row until the first refresh");
    assert_eq!(access_of(&v, &m.id), OLD_ACCESS.as_bytes());

    assert!(v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"));
    assert_eq!(access_of(&v, &m.id), NEW_ACCESS.as_bytes());
    assert_eq!(refresh_of(&v, &m.id), NEW_REFRESH.as_bytes());
    assert_eq!(v.meta(&sel(), &Actor::Owner).expect("meta").latest_version, 1, "a refresh is not a version");
    assert_eq!(v.oauth_status(&m.id).expect("status").expect("row").state, "active");

    drop(v);
    for f in ["vault.db", "vault.db-wal"] {
        let p = t.path().join("vault").join(f);
        if p.exists() {
            let bytes = std::fs::read(&p).expect("read db");
            for token in [NEW_ACCESS, NEW_REFRESH, OLD_ACCESS, OLD_REFRESH, CLIENT_SECRET] {
                assert!(!bytes.windows(token.len()).any(|w| w == token.as_bytes()), "{token} is in {f} in the clear");
            }
        }
    }
}

#[test]
fn a_person_rotating_resets_the_tokens_and_a_late_refresh_is_dropped() {
    let t = TestDir::new("oauth-rotate");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    assert!(v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"));

    let mut r = request();
    r.mode = PutMode::Rotate;
    v.put(r, bundle("k2test_person_access_11112222", "k2test_person_refresh_33334444"), &Actor::Owner).expect("rotate");
    assert!(v.oauth_status(&m.id).expect("status").is_none(), "rotate clears the state row");
    assert_eq!(access_of(&v, &m.id), b"k2test_person_access_11112222");

    // A refresh that started on v1 lands after the rotate.
    assert!(!v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"), "dropped");
    assert_eq!(access_of(&v, &m.id), b"k2test_person_access_11112222");
    assert!(!v.oauth_mark_expired(&m.id, 1, "invalid_grant").expect("mark"), "dropped too");
    assert!(!v.oauth_current(&m.id).expect("current").expired);
}

#[test]
fn expired_holds_no_tokens_until_a_new_value() {
    let t = TestDir::new("oauth-expired");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    assert!(v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"));
    assert!(v.oauth_mark_expired(&m.id, 1, "invalid_grant").expect("mark"));
    let c = v.oauth_current(&m.id).expect("current");
    assert!(c.expired);
    assert!(c.access.is_none() && c.refresh.is_none());
    let s = v.oauth_status(&m.id).expect("status").expect("row");
    assert!(s.is_expired());
    assert_eq!(s.last_error.as_deref(), Some("invalid_grant"));

    let mut r = request();
    r.mode = PutMode::Rotate;
    v.put(r, bundle(OLD_ACCESS, OLD_REFRESH), &Actor::Owner).expect("rotate");
    assert!(!v.oauth_current(&m.id).expect("current").expired, "a new sign-in restarts it");
}

#[test]
fn destroying_it_removes_the_tokens() {
    let t = TestDir::new("oauth-destroy");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    assert!(v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"));
    v.destroy(&sel(), &Actor::Owner).expect("destroy");
    drop(v);
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    let n: i64 = c.query_row("SELECT COUNT(*) FROM oauth_state", [], |r| r.get(0)).expect("count");
    assert_eq!(n, 0);
}

#[test]
fn rekey_keeps_the_tokens_and_a_tampered_row_stops_it() {
    let t = TestDir::new("oauth-rekey");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    assert!(v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"));
    let proof = HumanProof::Confirmed { method: "t".into() };
    v.rekey(&Actor::Owner, &proof).expect("rekey");
    assert_eq!(access_of(&v, &m.id), NEW_ACCESS.as_bytes());
    assert_eq!(refresh_of(&v, &m.id), NEW_REFRESH.as_bytes());
    drop(v);

    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    c.execute("UPDATE oauth_state SET expires_at = '2099-01-01T00:00:00Z'", []).expect("tamper");
    drop(c);
    let v = open(&t, &ks);
    let e = v.oauth_current(&m.id).err().expect("tampered row refused");
    assert_eq!(e.code, "vault_tamper");
    assert_eq!(v.rekey(&Actor::Owner, &proof).expect_err("rekey stops").code, "vault_tamper");
}

/// Two rows can't be swapped: the MAC names the secret.
#[test]
fn a_row_copied_to_another_secret_fails_closed() {
    let t = TestDir::new("oauth-swap");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    let mut other = request();
    other.name = "OTHER_API".into();
    let o = v.put(other, bundle(OLD_ACCESS, OLD_REFRESH), &Actor::Owner).expect("put").meta;
    assert!(v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"));
    drop(v);
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    c.execute("UPDATE oauth_state SET secret_id = ?1", [&o.id]).expect("swap");
    drop(c);
    let v = open(&t, &ks);
    assert_eq!(v.oauth_current(&o.id).err().expect("refused").code, "vault_tamper");
}

#[test]
fn the_redactor_masks_each_token_on_its_own() {
    let t = TestDir::new("oauth-redact");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    let before = v.redactor();
    for token in [OLD_ACCESS, OLD_REFRESH, CLIENT_SECRET] {
        let out = before.redact(format!("x {token} y").as_bytes());
        assert_eq!(String::from_utf8_lossy(&out), "x [k2v:MAIL_API] y");
    }
    assert!(v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"));
    let after = v.redactor();
    for token in [NEW_ACCESS, NEW_REFRESH] {
        let out = after.redact(format!("x {token} y").as_bytes());
        assert_eq!(String::from_utf8_lossy(&out), "x [k2v:MAIL_API] y", "a refresh must refresh the redactor too");
    }
}

#[test]
fn bad_values_are_refused_without_quoting_them() {
    let t = TestDir::new("oauth-bad");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let cases: Vec<String> = vec![
        format!("not json {OLD_REFRESH}"),
        format!(r#"{{"access_token":"{OLD_ACCESS}","client_id":"c"}}"#),
        format!(r#"{{"refresh_token":"{OLD_REFRESH}","client_id":"c"}}"#),
        format!(r#"{{"access_token":"{OLD_ACCESS}","refresh_token":"{OLD_REFRESH}"}}"#),
        format!(r#"{{"access_token":"a\nb{OLD_ACCESS}","refresh_token":"{OLD_REFRESH}","client_id":"c"}}"#),
        format!(r#"{{"access_token":"{OLD_ACCESS}","refresh_token":"{OLD_REFRESH}","client_id":"c","expires_at":"soon"}}"#),
    ];
    for c in cases {
        let e = v.put(request(), SecretBytes::from_string(c), &Actor::Owner).err().expect("refused");
        assert_eq!(e.code, "bad_value");
        for token in [OLD_ACCESS, OLD_REFRESH] {
            assert!(!e.hint.contains(token), "{}", e.code);
        }
    }
    // Basic needs the client secret.
    let basic = Sign::OauthRefresh { token_url: "https://login.example.com/token".into(), client_auth: ClientAuth::Basic };
    let no_secret = format!(r#"{{"access_token":"{OLD_ACCESS}","refresh_token":"{OLD_REFRESH}","client_id":"c"}}"#);
    assert_eq!(basic.check_value(no_secret.as_bytes()).expect_err("basic").code, "bad_value");
}

#[test]
fn the_token_url_must_be_public_https() {
    let build = |u: &str| Sign::OauthRefresh { token_url: u.into(), client_auth: ClientAuth::Post }.normalised();
    for bad in [
        "http://login.example.com/token",
        "https://user:pw@login.example.com/token",
        "https://login.example.com/token#frag",
        "https://10.0.0.5/token",
        "https://127.0.0.1/token",
        "https://[::1]/token",
        "https://169.254.169.254/token",
        "not a url",
        "ftp://login.example.com/token",
    ] {
        assert_eq!(build(bad).expect_err(bad).code, "bad_token_url", "{bad}");
    }
    build("https://login.example.com/oauth2/token").expect("good url");
}

#[test]
fn handoffs_and_carries_leave_it_behind() {
    let t = TestDir::new("oauth-leave");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    assert_eq!(k2_vault::handoff::check_sendable(&m).expect_err("no handoff").code, "never_sendable");

    let mk = |inject: Option<Inject>, name: &str| CarryItem {
        scope_kind: "workspace".into(),
        scope_label: "ws1".into(),
        scope_id: Some("ws1".into()),
        name: name.into(),
        kind: "oauth_token".into(),
        hosts: vec![],
        inject,
        default_delivery: "fetch".into(),
        delivery_ceiling: "fetch".into(),
        description: String::new(),
        human_given: true,
        value_b64: String::new(),
        source_scope: None,
        grants: Default::default(),
    };
    let p = CarryPayload::in_process("v", false, "c", "ws1", "ws1", vec![mk(Some(signer()), "SIGN_IN"), mk(Some(Inject::bearer()), "PLAIN")], vec![]);
    let names: Vec<&str> = p.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["PLAIN"]);
}

#[test]
fn the_audit_says_what_happened_and_holds_no_token() {
    let t = TestDir::new("oauth-audit");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v);
    assert!(v.oauth_store_refresh(&m.id, 1, &secret(NEW_ACCESS), &secret(NEW_REFRESH), 4_000_000_000).expect("store"));
    v.oauth_audit(&m.id, "ok");
    v.oauth_audit(&m.id, "ok");
    v.oauth_audit(&m.id, "error:transport");
    v.flush_rollups();
    let rows = v.audit(&AuditQuery { secret_id: Some(m.id.clone()), ..Default::default() }, &Actor::Owner).expect("audit");
    let text = serde_json::to_string(&rows).expect("json");
    let ok = rows.iter().find(|r| r.action == "refresh" && r.outcome == "ok").expect("ok row");
    assert_eq!(ok.count, 2, "rolled up");
    assert!(rows.iter().any(|r| r.action == "refresh" && r.outcome == "error:transport"));
    for token in [NEW_ACCESS, NEW_REFRESH, OLD_ACCESS, OLD_REFRESH, CLIENT_SECRET] {
        assert!(!text.contains(token));
    }
}
