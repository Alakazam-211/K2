//! K6 unit tests: keyring in the vault, JWKS shape, sign → verify, the
//! rotation schedule, restart persistence, the maintenance tick, owner
//! route gates, and no key material in logs or responses.
//!
//! Every test opens its own temp vault (fake key store): no real key
//! store, no `~/.k2`, no production daemon.

use std::sync::Arc;

use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{
    Actor, AgentActor, Kind, KeyStore, LocalVault, Namespace, PutMode, PutRequest, Requester, Role, Scope, SecretBytes,
    SecretStatus, SecretStore, Selector, Use, VaultError,
};
use serde_json::{json, Value};

use super::keyring::{
    self, check_invariants, plan, KeyAction, KeyError, KeyRole, Keyring, KEY_PREFIX, PASS_ENGINE,
    PUBLISH_LEAD_MIN_SECS, RETIRE_AFTER_LAST_USE_SECS, ROTATE_AFTER_SECS, STATE_NAME,
};
use super::maint::{tick_once, TickOutcome};
use super::routes::{self, DROP_PREVIOUS, EMERGENCY_CONFIRM_ACTION, KEYS, ROTATE};
use super::{verify_with_jwks, ChatPassClaims, CHAT_PASS_ISSUER, CHAT_PASS_TYP, PASS_TTL_SECS};
use crate::vault::actor::{Channel, VaultCaller};

const T0: i64 = 1_790_500_000;
const INSTALL: &str = "4242";
const AUD: &str = "sew:4242";

struct TV {
    _dir: k2_vault::fsutil::TestDir,
    path: std::path::PathBuf,
    ks: Arc<FakeKeyStore>,
    vault: Arc<LocalVault>,
}

fn open(path: &std::path::Path, ks: &Arc<FakeKeyStore>) -> Arc<LocalVault> {
    Arc::new(
        LocalVault::open(path, ks.clone() as Arc<dyn KeyStore>, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
            .expect("open temp vault"),
    )
}

fn temp_vault(tag: &str) -> TV {
    let dir = k2_vault::fsutil::TestDir::new(tag);
    let path = dir.path().join("vault");
    let ks = Arc::new(FakeKeyStore::new());
    let vault = open(&path, &ks);
    TV { _dir: dir, path, ks, vault }
}

fn ring(tv: &TV) -> Keyring {
    Keyring::new(tv.vault.clone(), PASS_ENGINE)
}

fn claims(now: i64) -> ChatPassClaims {
    ChatPassClaims::new(INSTALL, "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e", "skn_3c4f", now).expect("claims")
}

fn kids(jwks: &Value) -> Vec<String> {
    jwks["keys"].as_array().expect("keys array").iter().map(|k| k["kid"].as_str().expect("kid").to_string()).collect()
}

fn kid_of(state: &keyring::KeyState, role: KeyRole) -> String {
    state.keys.iter().find(|k| k.state == role).unwrap_or_else(|| panic!("no {role:?} key in {state:?}")).kid.clone()
}

/// Every PEM stored for this keyring (read straight from the vault).
fn stored_pems(v: &LocalVault) -> Vec<String> {
    let f = k2_vault::ListFilter { include_system: true, scope: Some(Scope::engine(PASS_ENGINE)), include_destroyed: false };
    v.list(&f, &Actor::System("test".into()))
        .expect("list")
        .into_iter()
        .filter(|m| m.name.starts_with(KEY_PREFIX))
        .map(|m| {
            let r = v
                .resolve(&Selector::system(Scope::engine(PASS_ENGINE), m.name.clone()), Use::System, &Requester::System("test".into()))
                .expect("resolve key");
            String::from_utf8(r.value.expose().to_vec()).expect("pem text")
        })
        .collect()
}

fn entry_status(v: &LocalVault, name: &str) -> Option<SecretStatus> {
    match v.meta(&Selector::system(Scope::engine(PASS_ENGINE), name), &Actor::System("test".into())) {
        Ok(m) => Some(m.status),
        Err(e) if e.status == 404 => None,
        Err(e) => panic!("meta {name}: {e}"),
    }
}

/// The base64 body lines of a PEM (what a leak would show).
fn pem_body_lines(pem: &str) -> Vec<String> {
    pem.lines().filter(|l| !l.starts_with("-----") && l.len() > 16).map(str::to_string).collect()
}

fn assert_no_key_material(text: &str, pems: &[String]) {
    assert!(!text.contains("PRIVATE KEY"), "PEM header leaked: {text}");
    for pem in pems {
        for line in pem_body_lines(pem) {
            assert!(!text.contains(&line), "PEM body leaked into: {text}");
        }
    }
}

// ── claims ──────────────────────────────────────────────────────────

#[test]
fn claims_are_the_seven_with_a_60s_life_and_a_128_bit_jti() {
    let c = claims(T0);
    assert_eq!(c.iss, CHAT_PASS_ISSUER);
    assert_eq!(c.aud, AUD);
    assert_eq!(c.exp - c.iat, PASS_TTL_SECS);
    assert_eq!(c.jti.len(), 32);
    let v = serde_json::to_value(&c).expect("json");
    let mut names: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["aud", "exp", "iat", "iss", "jti", "sid", "sub"]);
    assert_ne!(claims(T0).jti, c.jti, "jti is random");
}

#[test]
fn claims_with_a_bad_shape_are_refused_before_signing() {
    for (install, sid) in [("0", "skn_1"), ("01", "skn_1"), ("12a", "skn_1"), ("", "skn_1"), ("5", "bad sid"), ("5", "")] {
        assert!(ChatPassClaims::new(install, "k2:owner", sid, T0).is_err(), "{install:?}/{sid:?}");
    }
    let tv = temp_vault("cp-badclaims");
    let r = ring(&tv);
    let mut c = claims(T0);
    c.exp = c.iat + 121;
    assert!(matches!(r.sign(&c, T0), Err(KeyError::BadClaims(_))));
    let mut c = claims(T0);
    c.iss = "k2-host-sessions".into();
    assert!(matches!(r.sign(&c, T0), Err(KeyError::BadClaims(_))));
    let mut c = claims(T0);
    c.jti = "ABC".into();
    assert!(matches!(r.sign(&c, T0), Err(KeyError::BadClaims(_))));
}

/// Sew 0.6.0 refuses every pass whose `sub` is an `app:` id, and a box
/// user's pass through a non-home App. K2 never builds either: the claims
/// refuse them before signing, and every identity the mint classifies keeps
/// the App claim to App sessions (`k2:principal:`).
#[test]
fn no_app_sub_and_no_app_claim_on_a_box_users_pass() {
    let app = "0f8fad5b-d9cb-469f-a165-70867728950e";
    assert!(ChatPassClaims::new(INSTALL, &format!("app:{app}"), "skn_1", T0).is_err(), "an app: sub");
    for sub in ["k2:owner", "k2:connect:6f1e2d3c-4b5a-4968-8776-655443322110"] {
        let c = ChatPassClaims::new(INSTALL, sub, "own_1", T0).expect("a box user's pass").with_caps(vec!["chat:read".into()]).expect("caps");
        assert!(c.clone().with_app(None).is_ok(), "{sub} without an App");
        assert!(c.with_app(Some(app)).is_err(), "{sub} through an App");
    }
    let p = ChatPassClaims::new(INSTALL, &format!("k2:principal:{app}"), "skn_1", T0)
        .and_then(|c| c.with_caps(vec!["chat:read".into()]))
        .and_then(|c| c.with_app(Some(app)))
        .expect("an App session's pass");
    assert_eq!(p.app.as_deref(), Some(app));

    // What the mint classifies: only an App session gets an `app` claim,
    // and its sub is a principal.
    use super::identity::{connect_identity, owner_linked_identity, Provenance, SewRole};
    use super::mint::app_claim;
    let owner_linked = owner_linked_identity("boss", "k2cs_session");
    let connect = connect_identity("ana", k2_core::connect_users::Role::Owner, "6f1e2d3c-4b5a-4968-8776-655443322110", "k2cs_s2", false);
    let mut with_app = owner_linked.clone();
    with_app.app = Some(app.into()); // even if a box user's identity named an App
    for ident in [&owner_linked, &connect, &with_app] {
        assert_ne!(ident.provenance, Provenance::AppSession);
        assert_eq!(app_claim(ident), None, "{ident:?}");
        assert!(!ident.sub.starts_with("app:"));
    }
    assert_eq!(connect.sew_role, SewRole::Member, "an Owner-role Connect login is never Sew's install owner");
}

// ── JWKS + sign/verify ──────────────────────────────────────────────

#[test]
fn jwks_lists_current_then_next_with_public_fields_only() {
    let tv = temp_vault("cp-jwks");
    let r = ring(&tv);
    assert_eq!(r.state().expect("state"), None, "no keyring until first use");
    let jwks = r.public_jwks(T0).expect("jwks");
    let st = r.state().expect("state").expect("created");
    check_invariants(&st).expect("invariants");
    assert_eq!(kids(&jwks), vec![kid_of(&st, KeyRole::Current), kid_of(&st, KeyRole::Next)]);
    for k in jwks["keys"].as_array().expect("keys") {
        let mut names: Vec<&str> = k.as_object().expect("jwk").keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["alg", "crv", "kid", "kty", "use", "x", "y"], "public members only (no d)");
        assert_eq!(k["kty"], "EC");
        assert_eq!(k["crv"], "P-256");
        assert_eq!(k["alg"], "ES256");
        assert_eq!(k["use"], "sig");
    }
    assert_no_key_material(&jwks.to_string(), &stored_pems(&tv.vault));
    // Second call: same keys, nothing regenerated.
    assert_eq!(kids(&r.public_jwks(T0 + 5).expect("jwks")), kids(&jwks));
}

#[test]
fn keys_live_in_the_vault_system_namespace_with_no_delivery() {
    let tv = temp_vault("cp-store");
    let r = ring(&tv);
    r.public_jwks(T0).expect("jwks");
    let st = r.state().expect("state").expect("keyring");
    for k in &st.keys {
        let m = tv
            .vault
            .meta(&Selector::system(Scope::engine(PASS_ENGINE), format!("{KEY_PREFIX}{}", k.kid)), &Actor::System("t".into()))
            .expect("key entry");
        assert_eq!(m.namespace, Namespace::System);
        assert_eq!(m.kind, Kind::SigningKey);
        assert_eq!(m.default_delivery, k2_vault::Delivery::None);
    }
    // No person or agent can read a system value back.
    let sel = Selector::system(Scope::engine(PASS_ENGINE), format!("{KEY_PREFIX}{}", st.keys[0].kid));
    match tv.vault.reveal(&sel, &Actor::Owner, &k2_vault::HumanProof::Confirmed { method: "test".into() }) {
        Err(e) => assert_eq!(e.code, "vault_system_namespace"),
        Ok(_) => panic!("a system value was revealed"),
    }
    match tv.vault.resolve(&sel, Use::Api, &Requester::Session { workspace_id: "w".into(), session_id: None }) {
        Err(e) => assert_eq!(e.status, 403, "{e:?}"),
        Ok(_) => panic!("a session resolved a system value"),
    }
    // Nothing on disk outside the vault.
    let stray: Vec<_> = std::fs::read_dir(tv.path.parent().expect("parent"))
        .expect("read dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".pem"))
        .collect();
    assert!(stray.is_empty(), "no .pem files: {stray:?}");
}

#[test]
fn sign_then_verify_round_trips_and_the_header_is_exact() {
    let tv = temp_vault("cp-roundtrip");
    let r = ring(&tv);
    let c = claims(T0);
    let token = r.sign(&c, T0).expect("sign");
    let jwks = r.public_jwks(T0).expect("jwks");
    let got = verify_with_jwks(&token, &jwks, AUD, T0 + 1).expect("verifies");
    assert_eq!(got, c);

    use base64::Engine as _;
    let head: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.split('.').next().expect("head")).expect("b64"),
    )
    .expect("json");
    let st = r.state().expect("state").expect("keyring");
    assert_eq!(head, json!({ "alg": "ES256", "typ": CHAT_PASS_TYP, "kid": kid_of(&st, KeyRole::Current) }));

    // What Sew rejects, the local verifier rejects.
    assert!(verify_with_jwks(&token, &jwks, "sew:9999", T0).is_err(), "other install");
    assert!(verify_with_jwks(&token, &jwks, AUD, c.exp + 31).is_err(), "expired past skew");
    assert!(verify_with_jwks(&token, &jwks, AUD, c.exp + 30).is_ok(), "inside skew");
    assert!(verify_with_jwks(&token, &jwks, AUD, c.iat - 31).is_err(), "issued in the future");
    let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
    let sig = parts[2].clone();
    parts[2] = format!("{}{}", if sig.starts_with('A') { "B" } else { "A" }, &sig[1..]);
    assert!(verify_with_jwks(&parts.join("."), &jwks, AUD, T0).is_err(), "tampered signature");
    let only_next = json!({ "keys": [jwks["keys"][1].clone()] });
    assert!(verify_with_jwks(&token, &only_next, AUD, T0).is_err(), "kid not in JWKS");
    // A different keyring's JWKS doesn't verify it.
    let other = temp_vault("cp-roundtrip-other");
    let other_jwks = ring(&other).public_jwks(T0).expect("jwks");
    assert!(verify_with_jwks(&token, &other_jwks, AUD, T0).is_err(), "foreign keys");
}

#[test]
fn the_chat_key_is_not_the_capability_key() {
    let tv = temp_vault("cp-notcap");
    let r = ring(&tv);
    let jwks = r.public_jwks(T0).expect("jwks");
    let _home = crate::test_support::TempHome::new();
    let cap = crate::v1_capabilities::public_jwks().expect("capability jwks");
    for k in kids(&cap) {
        assert!(!kids(&jwks).contains(&k), "chat kid {k} equals the capability kid");
    }
}

// ── schedule (pure) ─────────────────────────────────────────────────

fn st(keys: Vec<keyring::KeyEntry>) -> keyring::KeyState {
    keyring::KeyState { format: 1, keys, history: vec![] }
}

fn entry(kid: &str, role: KeyRole, published: i64, activated: Option<i64>, last_used: Option<i64>) -> keyring::KeyEntry {
    keyring::KeyEntry { kid: kid.into(), state: role, created_at: published, published_at: published, activated_at: activated, last_used_at: last_used }
}

#[test]
fn plan_follows_the_schedule() {
    assert_eq!(plan(None, T0), vec![KeyAction::Bootstrap]);
    let fresh = st(vec![entry("aaaaaaaaaaaaaaaa", KeyRole::Current, T0, Some(T0), None), entry("bbbbbbbbbbbbbbbb", KeyRole::Next, T0, None, None)]);
    assert_eq!(plan(Some(&fresh), T0 + ROTATE_AFTER_SECS - 1), vec![]);
    assert_eq!(plan(Some(&fresh), T0 + ROTATE_AFTER_SECS), vec![KeyAction::Promote]);
    // Due, but next was published < 10 min ago.
    let short = st(vec![
        entry("aaaaaaaaaaaaaaaa", KeyRole::Current, T0, Some(T0), None),
        entry("bbbbbbbbbbbbbbbb", KeyRole::Next, T0 + ROTATE_AFTER_SECS - 60, None, None),
    ]);
    assert_eq!(plan(Some(&short), T0 + ROTATE_AFTER_SECS), vec![KeyAction::LeadTooShort]);
    // No next: make one (and the rotation waits for its lead).
    let lone = st(vec![entry("aaaaaaaaaaaaaaaa", KeyRole::Current, T0, Some(T0), None)]);
    assert_eq!(plan(Some(&lone), T0 + 5), vec![KeyAction::GenerateNext]);
    assert_eq!(plan(Some(&lone), T0 + ROTATE_AFTER_SECS), vec![KeyAction::GenerateNext, KeyAction::LeadTooShort]);
    // Retire 10 min after last use, not before.
    let ret = st(vec![
        entry("aaaaaaaaaaaaaaaa", KeyRole::Current, T0, Some(T0), None),
        entry("bbbbbbbbbbbbbbbb", KeyRole::Next, T0, None, None),
        entry("cccccccccccccccc", KeyRole::Retiring, T0 - 100, Some(T0 - 100), Some(T0)),
    ]);
    assert_eq!(plan(Some(&ret), T0 + RETIRE_AFTER_LAST_USE_SECS - 1), vec![]);
    assert_eq!(plan(Some(&ret), T0 + RETIRE_AFTER_LAST_USE_SECS), vec![KeyAction::Retire("cccccccccccccccc".into())]);
}

#[test]
fn invariants_catch_a_broken_state() {
    assert!(check_invariants(&st(vec![])).is_err(), "no current");
    assert!(check_invariants(&st(vec![
        entry("aaaaaaaaaaaaaaaa", KeyRole::Current, T0, Some(T0), None),
        entry("bbbbbbbbbbbbbbbb", KeyRole::Current, T0, Some(T0), None)
    ]))
    .is_err());
    assert!(check_invariants(&st(vec![
        entry("aaaaaaaaaaaaaaaa", KeyRole::Current, T0, Some(T0), None),
        entry("bbbbbbbbbbbbbbbb", KeyRole::Next, T0, None, None),
        entry("cccccccccccccccc", KeyRole::Next, T0, None, None)
    ]))
    .is_err());
    assert!(check_invariants(&st(vec![entry("AAAA", KeyRole::Current, T0, Some(T0), None)])).is_err(), "kid shape");
    let mut bad = st(vec![entry("aaaaaaaaaaaaaaaa", KeyRole::Current, T0, Some(T0), None)]);
    bad.format = 2;
    assert!(check_invariants(&bad).is_err(), "format");
}

// ── rotation against a real (temp) vault ────────────────────────────

#[test]
fn rotation_promotes_next_keeps_previous_for_the_overlap_then_drops_it() {
    let tv = temp_vault("cp-rotate");
    let r = ring(&tv);
    let before = r.public_jwks(T0).expect("bootstrap");
    let s0 = r.state().expect("state").expect("keyring");
    let (cur0, next0) = (kid_of(&s0, KeyRole::Current), kid_of(&s0, KeyRole::Next));
    let old_pass = r.sign(&claims(T0 + ROTATE_AFTER_SECS - 30), T0 + ROTATE_AFTER_SECS - 30).expect("sign before");
    assert_eq!(kids(&before), vec![cur0.clone(), next0.clone()]);

    // 30 days on: the tick promotes.
    let t1 = T0 + ROTATE_AFTER_SECS;
    assert!(r.maintain(t1, false).expect("maintain"), "promoted");
    let s1 = r.state().expect("state").expect("keyring");
    assert_eq!(kid_of(&s1, KeyRole::Current), next0, "next became current");
    assert_eq!(kid_of(&s1, KeyRole::Retiring), cur0, "old current is retiring");
    let next1 = kid_of(&s1, KeyRole::Next);
    assert!(next1 != cur0 && next1 != next0, "a new next");
    let during = r.public_jwks(t1).expect("jwks");
    assert_eq!(kids(&during), vec![next0.clone(), next1.clone(), cur0.clone()], "current, next, retiring");
    // A pass from before the promotion still verifies during the overlap.
    verify_with_jwks(&old_pass, &during, AUD, t1).expect("old pass during overlap");
    // New passes use the promoted key.
    let new_pass = r.sign(&claims(t1), t1).expect("sign after");
    let head = jsonwebtoken::decode_header(&new_pass).expect("header");
    assert_eq!(head.kid.as_deref(), Some(next0.as_str()));
    verify_with_jwks(&new_pass, &during, AUD, t1).expect("new pass");

    // Not yet: 10 min after last use.
    assert!(!r.maintain(t1 + RETIRE_AFTER_LAST_USE_SECS - 1, false).expect("maintain"));
    assert_eq!(kids(&r.public_jwks(t1 + RETIRE_AFTER_LAST_USE_SECS - 1).expect("jwks")).len(), 3);
    // Now: dropped from the JWKS, destroyed in the vault, kept in history.
    assert!(r.maintain(t1 + RETIRE_AFTER_LAST_USE_SECS, false).expect("maintain"));
    let after = r.public_jwks(t1 + RETIRE_AFTER_LAST_USE_SECS).expect("jwks");
    assert_eq!(kids(&after), vec![next0.clone(), next1.clone()]);
    assert!(verify_with_jwks(&old_pass, &after, AUD, t1).is_err(), "retired kid no longer verifies");
    assert_eq!(entry_status(&tv.vault, &format!("{KEY_PREFIX}{cur0}")), Some(SecretStatus::Destroyed));
    let s2 = r.state().expect("state").expect("keyring");
    assert_eq!(s2.history.iter().map(|h| h.kid.as_str()).collect::<Vec<_>>(), vec![cur0.as_str()]);
    assert_eq!(stored_pems(&tv.vault).len(), 2, "only current + next remain");
}

#[test]
fn rotation_waits_for_the_publish_lead() {
    let tv = temp_vault("cp-lead");
    let r = ring(&tv);
    r.public_jwks(T0).expect("bootstrap");
    match r.rotate_now(T0 + 100) {
        Err(KeyError::LeadTooShort { retry_after_secs }) => assert_eq!(retry_after_secs, PUBLISH_LEAD_MIN_SECS - 100),
        other => panic!("want lead_too_short, got {other:?}"),
    }
    let s = r.state().expect("state").expect("keyring");
    let next0 = kid_of(&s, KeyRole::Next);
    let s = r.rotate_now(T0 + PUBLISH_LEAD_MIN_SECS).expect("rotate");
    assert_eq!(kid_of(&s, KeyRole::Current), next0, "owner rotate promotes before 30 days");
    // The brand-new next has no lead yet.
    assert!(matches!(r.rotate_now(T0 + PUBLISH_LEAD_MIN_SECS + 1), Err(KeyError::LeadTooShort { .. })));
}

#[test]
fn drop_previous_retires_the_retiring_key_now() {
    let tv = temp_vault("cp-dropprev");
    let r = ring(&tv);
    r.public_jwks(T0).expect("bootstrap");
    let (none, dropped) = (r.state().expect("s").expect("k"), vec![] as Vec<String>);
    assert!(none.keys.iter().all(|k| k.state != KeyRole::Retiring));
    assert_eq!(r.drop_previous(T0).expect("noop").1, dropped);
    let s = r.rotate_now(T0 + PUBLISH_LEAD_MIN_SECS).expect("rotate");
    let old = kid_of(&s, KeyRole::Retiring);
    let (s, dropped) = r.drop_previous(T0 + PUBLISH_LEAD_MIN_SECS + 1).expect("drop");
    assert_eq!(dropped, vec![old.clone()]);
    let s = s.expect("keyring");
    assert!(s.keys.iter().all(|k| k.kid != old));
    assert_eq!(entry_status(&tv.vault, &format!("{KEY_PREFIX}{old}")), Some(SecretStatus::Destroyed));
}

#[test]
fn keys_survive_a_daemon_restart() {
    let tv = temp_vault("cp-restart");
    let (jwks, pass, state) = {
        let r = ring(&tv);
        let jwks = r.public_jwks(T0).expect("jwks");
        let pass = r.sign(&claims(T0), T0).expect("sign");
        (jwks, pass, r.state().expect("s").expect("k"))
    };
    // "Restart": drop every handle, reopen the vault file with the same
    // key store, and build a new keyring.
    let TV { _dir, path, ks, vault } = tv;
    drop(vault);
    let vault2 = open(&path, &ks);
    let r2 = Keyring::new(vault2.clone(), PASS_ENGINE);
    assert_eq!(r2.state().expect("state").expect("keyring"), state, "same lifecycle");
    let jwks2 = r2.public_jwks(T0 + 10).expect("jwks after restart");
    assert_eq!(jwks2, jwks, "same public keys, nothing regenerated");
    verify_with_jwks(&pass, &jwks2, AUD, T0 + 10).expect("old pass verifies after restart");
    let pass2 = r2.sign(&claims(T0 + 10), T0 + 10).expect("sign after restart");
    verify_with_jwks(&pass2, &jwks, AUD, T0 + 10).expect("new pass verifies against the old JWKS");
    // Same object, cache dropped: still the same.
    r2.forget_cache();
    assert_eq!(r2.public_jwks(T0 + 11).expect("jwks"), jwks);
}

#[test]
fn an_orphan_key_from_a_crash_is_destroyed_on_load() {
    let tv = temp_vault("cp-orphan");
    let r = ring(&tv);
    r.public_jwks(T0).expect("bootstrap");
    // A key written but never listed (crash between the key and state writes).
    let sk = crate::es256::random_secret().expect("sk");
    let pem = crate::es256::private_pem(&sk).expect("pem");
    let (kid, _) = crate::es256::jwk_from_private_pem(&pem).expect("kid");
    let mut req = PutRequest::new(Namespace::System, Scope::engine(PASS_ENGINE), format!("{KEY_PREFIX}{kid}"), Kind::SigningKey);
    req.default_delivery = Some(k2_vault::Delivery::None);
    req.mode = PutMode::Create;
    tv.vault.put(req, SecretBytes::from_slice(pem.as_bytes()), &Actor::System(PASS_ENGINE.into())).expect("put orphan");
    r.forget_cache();
    let jwks = r.public_jwks(T0 + 1).expect("jwks");
    assert!(!kids(&jwks).contains(&kid), "orphan never published");
    assert_eq!(entry_status(&tv.vault, &format!("{KEY_PREFIX}{kid}")), Some(SecretStatus::Destroyed));
}

#[test]
fn a_corrupt_keyring_fails_closed_until_an_emergency_rotation() {
    let tv = temp_vault("cp-corrupt");
    let r = ring(&tv);
    let jwks0 = r.public_jwks(T0).expect("bootstrap");
    let mut req = PutRequest::new(Namespace::System, Scope::engine(PASS_ENGINE), STATE_NAME, Kind::Generic);
    req.default_delivery = Some(k2_vault::Delivery::None);
    req.mode = PutMode::Upsert;
    tv.vault.put(req, SecretBytes::from_slice(b"{not json"), &Actor::System(PASS_ENGINE.into())).expect("corrupt state");
    r.forget_cache();
    assert!(matches!(r.public_jwks(T0 + 1), Err(KeyError::Corrupt(_))), "jwks fails closed");
    assert!(matches!(r.sign(&claims(T0 + 1), T0 + 1), Err(KeyError::Corrupt(_))), "mint fails closed");
    assert!(matches!(r.maintain(T0 + 1, true), Err(KeyError::Corrupt(_))), "never regenerated silently");
    assert!(matches!(tick_once(&r, T0 + 1), TickOutcome::Failed(_)));
    let s = r.emergency(T0 + 2).expect("emergency recovers");
    check_invariants(&s).expect("fresh invariants");
    let jwks1 = r.public_jwks(T0 + 2).expect("jwks after recovery");
    for k in kids(&jwks0) {
        assert!(!kids(&jwks1).contains(&k), "old kid {k} gone");
        assert_eq!(entry_status(&tv.vault, &format!("{KEY_PREFIX}{k}")), Some(SecretStatus::Destroyed));
    }
    assert_eq!(entry_status(&tv.vault, &format!("{STATE_NAME}.corrupt.{}", T0 + 2)), Some(SecretStatus::Active), "corrupt state kept");
}

#[test]
fn emergency_rotation_replaces_every_key_with_no_grace() {
    let tv = temp_vault("cp-emergency");
    let r = ring(&tv);
    r.public_jwks(T0).expect("bootstrap");
    r.rotate_now(T0 + PUBLISH_LEAD_MIN_SECS).expect("rotate (three keys now)");
    let old = r.state().expect("s").expect("k");
    assert_eq!(old.keys.len(), 3);
    let old_pass = r.sign(&claims(T0 + 700), T0 + 700).expect("sign");
    let s = r.emergency(T0 + 701).expect("emergency");
    assert_eq!(s.keys.len(), 2, "new current + next only");
    for k in &old.keys {
        assert!(s.keys.iter().all(|n| n.kid != k.kid));
        assert!(s.history.iter().any(|h| h.kid == k.kid), "{} in history", k.kid);
    }
    let jwks = r.public_jwks(T0 + 701).expect("jwks");
    assert!(verify_with_jwks(&old_pass, &jwks, AUD, T0 + 701).is_err(), "old passes die at once");
    assert_eq!(stored_pems(&tv.vault).len(), 2);
}

// ── the tick ────────────────────────────────────────────────────────

#[test]
fn the_tick_never_creates_a_keyring_and_rotates_an_existing_one() {
    let tv = temp_vault("cp-tick");
    let r = ring(&tv);
    assert_eq!(tick_once(&r, T0), TickOutcome::NoKeyring);
    assert_eq!(r.state().expect("state"), None);
    assert!(stored_pems(&tv.vault).is_empty(), "the tick made no keys");
    assert_eq!(entry_status(&tv.vault, STATE_NAME), None);
    r.public_jwks(T0).expect("first use creates it");
    assert_eq!(tick_once(&r, T0 + 60), TickOutcome::Unchanged);
    assert_eq!(tick_once(&r, T0 + ROTATE_AFTER_SECS), TickOutcome::Changed, "promotion on schedule");
    assert_eq!(tick_once(&r, T0 + ROTATE_AFTER_SECS + RETIRE_AFTER_LAST_USE_SECS), TickOutcome::Changed, "retire");
    assert_eq!(r.state().expect("s").expect("k").keys.len(), 2);
}

#[test]
fn tick_interval_env_override() {
    let _unset = k2_core::test_env::EnvVar::remove("K2_CHAT_KEYS_TICK_SECS");
    assert_eq!(super::maint::tick_interval(), std::time::Duration::from_secs(super::maint::MAINT_TICK_SECS));
    let _seven = k2_core::test_env::EnvVar::set("K2_CHAT_KEYS_TICK_SECS", "7");
    assert_eq!(super::maint::tick_interval(), std::time::Duration::from_secs(7));
}

// ── owner routes ────────────────────────────────────────────────────

fn owner() -> Result<VaultCaller, VaultError> {
    Ok(VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None })
}

fn login(role: Role) -> Result<VaultCaller, VaultError> {
    Ok(VaultCaller { actor: Actor::Login { username: "pat".into(), role }, channel: Channel::Login, session_id: None })
}

fn passport() -> Result<VaultCaller, VaultError> {
    Ok(VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: "ws1".into(), session_id: Some("s1".into()), shell: false, vault_manage: true }),
        channel: Channel::Plain,
        session_id: Some("s1".into()),
    })
}

fn body(r: &crate::cli_response::CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json body {}: {e}", r.body))
}

fn call(r: &Keyring, path: &str, is_post: bool, ingress: &str, caller: Result<VaultCaller, VaultError>, b: &str, now: i64) -> crate::cli_response::CliResponse {
    routes::handle_with(path, is_post, ingress, caller, b.as_bytes(), Ok(r), now)
}

#[test]
fn owner_routes_refuse_the_wrong_method() {
    let tv = temp_vault("cp-r405");
    let r = ring(&tv);
    for p in [ROTATE, DROP_PREVIOUS] {
        let resp = call(&r, p, false, "loopback", owner(), "", T0);
        assert_eq!(resp.status, "405 Method Not Allowed", "{p}");
        assert!(crate::routes::route_policy::get_refused(p, ""), "{p}: the dispatcher's top-level GET guard refuses it too");
        assert!(crate::routes::route_policy::post_allowed(p), "{p} on the POST allowlist");
    }
    assert_eq!(call(&r, KEYS, true, "loopback", owner(), "{}", T0).status, "405 Method Not Allowed");
    assert!(!crate::routes::route_policy::get_refused(KEYS, ""));
    assert!(!crate::routes::route_policy::post_allowed(KEYS));
    assert!(!crate::routes::route_policy::post_allowed(super::JWKS_PATH), "the JWKS is GET only");
    for p in [KEYS, ROTATE, DROP_PREVIOUS] {
        let row = crate::routes::route_policy::lookup(p).expect("classified");
        assert_eq!(row.get.or(row.post), Some(crate::routes::route_policy::Floor::Admin), "{p}");
    }
    assert_eq!(r.state().expect("state"), None, "no refused call created a keyring");
}

#[test]
fn owner_routes_admit_only_owner_and_admin_people_off_the_tunnel() {
    let tv = temp_vault("cp-r403");
    let r = ring(&tv);
    let cases: Vec<(&str, Result<VaultCaller, VaultError>, &str, &str)> = vec![
        ("tunnel owner", owner(), "tunnel", "loopback_only"),
        ("agent passport", passport(), "loopback", "owner_only"),
        ("member login", login(Role::Member), "lan", "owner_only"),
        ("app pass", Err(VaultError::forbidden("wrong_credential", "x")), "loopback", "credential_not_allowed"),
        ("no token", Err(VaultError::new("unauthorized", 401, "x")), "loopback", "unauthenticated"),
    ];
    for (what, caller, ingress, code) in cases {
        for (p, post, b) in [(KEYS, false, ""), (ROTATE, true, "{}"), (DROP_PREVIOUS, true, "{}")] {
            let caller = match &caller {
                Ok(c) => Ok(c.clone()),
                Err(e) => Err(e.clone()),
            };
            let resp = call(&r, p, post, ingress, caller, b, T0);
            let v = body(&resp);
            assert_eq!(v["error"]["code"], code, "{what} {p}: {v}");
            assert!(resp.status.starts_with(if code == "unauthenticated" { "401" } else { "403" }), "{what} {p}: {}", resp.status);
        }
    }
    assert_eq!(r.state().expect("state"), None, "no refused call touched the keyring");
    // Owner and Admin people pass.
    for caller in [owner(), login(Role::Admin), login(Role::Owner)] {
        let resp = call(&r, KEYS, false, "lan", caller, "", T0);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_eq!(body(&resp)["exists"], false, "GET never creates a keyring");
    }
}

#[test]
fn emergency_rotation_needs_a_person_confirm_not_the_owner_token_alone() {
    let tv = temp_vault("cp-remergency");
    let r = ring(&tv);
    r.public_jwks(T0).expect("bootstrap");
    let before = r.state().expect("s").expect("k");

    let resp = call(&r, ROTATE, true, "loopback", owner(), r#"{"emergency":true}"#, T0 + 1);
    assert_eq!(resp.status, "403 Forbidden");
    assert_eq!(body(&resp)["error"]["code"], "vault_confirm_required");
    // A confirm for another action, or a made-up token, doesn't do.
    let wrong = tv.vault.confirm.mint("reveal", None, "owner", "vault-password");
    for tok in [wrong.as_str(), "k2vc_madeup"] {
        let resp = call(&r, ROTATE, true, "loopback", owner(), &json!({ "emergency": true, "confirmToken": tok }).to_string(), T0 + 1);
        assert_eq!(resp.status, "403 Forbidden", "{tok}");
        assert_eq!(body(&resp)["error"]["code"], "vault_confirm_required");
    }
    // A confirm minted for someone else doesn't do either.
    let theirs = tv.vault.confirm.mint(EMERGENCY_CONFIRM_ACTION, None, "connect:pat", "connect-password");
    let resp = call(&r, ROTATE, true, "loopback", owner(), &json!({ "emergency": true, "confirmToken": theirs }).to_string(), T0 + 1);
    assert_eq!(resp.status, "403 Forbidden");
    assert_eq!(r.state().expect("s").expect("k"), before, "nothing changed");

    // The person's confirm (Touch ID / vault password) does, once.
    let tok = tv.vault.confirm.mint(EMERGENCY_CONFIRM_ACTION, None, "owner", "local-auth");
    let b = json!({ "emergency": true, "confirmToken": tok }).to_string();
    let resp = call(&r, ROTATE, true, "loopback", owner(), &b, T0 + 2);
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body(&resp);
    assert!(v["hint"].as_str().expect("hint").contains("not revoked"), "{v}");
    let after = r.state().expect("s").expect("k");
    for k in &before.keys {
        assert!(after.keys.iter().all(|n| n.kid != k.kid));
    }
    let replay = call(&r, ROTATE, true, "loopback", owner(), &b, T0 + 3);
    assert_eq!(replay.status, "403 Forbidden", "a confirm works once");
    assert!(crate::vault_routes::POST_ROUTES.contains(&crate::vault_routes::CONFIRM_FINISH));
}

#[test]
fn normal_rotate_and_drop_previous_through_the_route() {
    let tv = temp_vault("cp-rnormal");
    let r = ring(&tv);
    let resp = call(&r, ROTATE, true, "loopback", login(Role::Admin), "{}", T0);
    assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
    let v = body(&resp);
    assert_eq!(v["error"]["code"], "lead_too_short");
    assert_eq!(v["error"]["details"]["retry_after_ms"], PUBLISH_LEAD_MIN_SECS * 1000);
    let resp = call(&r, ROTATE, true, "lan", owner(), "", T0 + PUBLISH_LEAD_MIN_SECS);
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body(&resp);
    let states: Vec<&str> = v["keys"].as_array().expect("keys").iter().map(|k| k["state"].as_str().expect("state")).collect();
    assert_eq!(states, ["current", "next", "retiring"]);
    assert!(v["keys"][2]["retireAfter"].as_i64().is_some());
    let resp = call(&r, DROP_PREVIOUS, true, "loopback", owner(), "{}", T0 + PUBLISH_LEAD_MIN_SECS + 1);
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body(&resp);
    assert_eq!(v["dropped"].as_array().expect("dropped").len(), 1);
    assert_eq!(v["keys"].as_array().expect("keys").len(), 2);
    // Unknown fields are refused, named, never echoed with values.
    let resp = call(&r, ROTATE, true, "loopback", owner(), r#"{"force":"yes-secret-value"}"#, T0);
    assert_eq!(resp.status, "400 Bad Request");
    let v = body(&resp);
    assert_eq!(v["error"]["code"], "validation_failed");
    assert_eq!(v["error"]["hint"], "unknown field force");
    assert!(!resp.body.contains("yes-secret-value"));
    let resp = call(&r, DROP_PREVIOUS, true, "loopback", owner(), r#"{"x":1}"#, T0);
    assert_eq!(resp.status, "400 Bad Request");
    let resp = call(&r, ROTATE, true, "loopback", owner(), r#"{"confirmToken":"k2vc_x"}"#, T0);
    assert_eq!(resp.status, "400 Bad Request", "a confirm without emergency is a mistake");
}

#[test]
fn a_corrupt_keyring_shows_as_chat_keys_unavailable() {
    let tv = temp_vault("cp-rcorrupt");
    let r = ring(&tv);
    r.public_jwks(T0).expect("bootstrap");
    let mut req = PutRequest::new(Namespace::System, Scope::engine(PASS_ENGINE), STATE_NAME, Kind::Generic);
    req.default_delivery = Some(k2_vault::Delivery::None);
    tv.vault.put(req, SecretBytes::from_slice(br#"{"format":1,"keys":[],"history":[]}"#), &Actor::System(PASS_ENGINE.into())).expect("put");
    r.forget_cache();
    for (p, post, b) in [(KEYS, false, ""), (ROTATE, true, "{}"), (DROP_PREVIOUS, true, "{}")] {
        let resp = call(&r, p, post, "loopback", owner(), b, T0 + 1);
        assert_eq!(resp.status, "500 Internal Server Error", "{p}: {}", resp.body);
        assert_eq!(body(&resp)["error"]["code"], "chat_keys_unavailable", "{p}");
    }
    let tok = tv.vault.confirm.mint(EMERGENCY_CONFIRM_ACTION, None, "owner", "vault-password");
    let resp = call(&r, ROTATE, true, "loopback", owner(), &json!({ "emergency": true, "confirmToken": tok }).to_string(), T0 + 2);
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
}

#[test]
fn no_key_material_in_logs_responses_or_errors() {
    let tv = temp_vault("cp-noleak");
    let r = ring(&tv);
    let mut texts: Vec<String> = vec![];
    texts.push(r.public_jwks(T0).expect("jwks").to_string());
    texts.push(r.sign(&claims(T0), T0).expect("sign"));
    texts.push(call(&r, KEYS, false, "loopback", owner(), "", T0).body);
    texts.push(call(&r, ROTATE, true, "loopback", owner(), "{}", T0 + 1).body);
    texts.push(call(&r, ROTATE, true, "loopback", owner(), "{}", T0 + PUBLISH_LEAD_MIN_SECS).body);
    texts.push(call(&r, DROP_PREVIOUS, true, "loopback", owner(), "{}", T0 + PUBLISH_LEAD_MIN_SECS + 1).body);
    let tok = tv.vault.confirm.mint(EMERGENCY_CONFIRM_ACTION, None, "owner", "vault-password");
    let pems_before_wipe = stored_pems(&tv.vault);
    texts.push(call(&r, ROTATE, true, "loopback", owner(), &json!({ "emergency": true, "confirmToken": tok }).to_string(), T0 + 700).body);
    r.maintain(T0 + 701 + ROTATE_AFTER_SECS, false).expect("maintain");
    texts.push(format!("{:?}", r.state().expect("s")));
    let mut pems = stored_pems(&tv.vault);
    pems.extend(pems_before_wipe);
    assert!(pems.len() >= 4, "saw the keys this test made");
    for t in &texts {
        assert_no_key_material(t, &pems);
    }
    let logs = keyring::LOG_CAPTURE.lock().clone();
    assert!(!logs.is_empty(), "the keyring logged its work");
    for l in &logs {
        assert_no_key_material(l, &pems);
    }
    for e in [
        KeyError::Corrupt("x".into()),
        KeyError::Store("y".into()),
        KeyError::VaultClosed("vault_not_open".into()),
        KeyError::LeadTooShort { retry_after_secs: 5 },
    ] {
        assert_no_key_material(&e.to_string(), &pems);
    }
}

#[test]
fn jwks_url_points_at_loopback() {
    assert_eq!(super::jwks_url(38123), "http://127.0.0.1:38123/v1/chat/jwks");
    assert_ne!(super::APPROVAL_JWKS_PATH_RESERVED, super::JWKS_PATH, "approval keys get their own URL");
}
