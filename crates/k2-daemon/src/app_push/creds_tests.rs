//! Key checks (no network), the JWT shapes, and the vault round trip in
//! the system namespace (never readable through a route).

use super::super::test_keys::*;
use super::*;
use serde_json::json;

fn sa(over: serde_json::Value) -> String {
    let mut v = json!({
        "type": "service_account",
        "project_id": "k2-test-proj",
        "private_key_id": "abc123",
        "private_key": RSA_P8,
        "client_email": "push@k2-test-proj.iam.gserviceaccount.com",
        "token_uri": GOOGLE_TOKEN_URI,
    });
    for (k, x) in over.as_object().expect("object") {
        if x.is_null() {
            v.as_object_mut().expect("object").remove(k);
        } else {
            v[k] = x.clone();
        }
    }
    v.to_string()
}

#[test]
fn an_apns_key_is_checked_and_signs_an_es256_provider_token() {
    let k = validate_apns(&format!("  {}\r\n", EC_P256_P8.replace('\n', "\r\n")), "ABC123DEFG", "TEAM123456", "com.example.app")
        .expect("a P-256 .p8 with CRLF and edges");
    assert_eq!((k.key_id.as_str(), k.team_id.as_str(), k.bundle_id.as_str()), ("ABC123DEFG", "TEAM123456", "com.example.app"));
    assert_eq!(k.fingerprint().len(), 16);
    assert!(!format!("{k:?}").contains("PRIVATE"), "Debug is redacted");
    let jwt = apns_jwt(&k, 1_800_000_000).expect("mint");
    let header = jsonwebtoken::decode_header(&jwt).expect("header");
    assert_eq!(header.alg, jsonwebtoken::Algorithm::ES256);
    assert_eq!(header.kid.as_deref(), Some("ABC123DEFG"));
    let mut val = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
    val.required_spec_claims.clear();
    val.validate_exp = false;
    let dec = jsonwebtoken::DecodingKey::from_ec_pem(EC_P256_PUB.as_bytes()).expect("pub");
    let claims = jsonwebtoken::decode::<serde_json::Value>(&jwt, &dec, &val).expect("verifies with the public key").claims;
    assert_eq!(claims, json!({"iss": "TEAM123456", "iat": 1_800_000_000}));
}

#[test]
fn bad_apns_inputs_name_the_field_and_never_echo_it() {
    let cases = [
        (EC_P256_P8, "abc123defg", "TEAM123456", "com.example.app", "keyId"),
        (EC_P256_P8, "ABC123DEF", "TEAM123456", "com.example.app", "keyId"),
        (EC_P256_P8, "ABC123DEFG", "TEAM12345!", "com.example.app", "teamId"),
        (EC_P256_P8, "ABC123DEFG", "TEAM123456", "nodots", "bundleId"),
        (EC_P256_P8, "ABC123DEFG", "TEAM123456", "com..app", "bundleId"),
        (EC_P384_P8, "ABC123DEFG", "TEAM123456", "com.example.app", "P-256"),
        (RSA_P8, "ABC123DEFG", "TEAM123456", "com.example.app", "P-256"),
        ("not a key at all", "ABC123DEFG", "TEAM123456", "com.example.app", "p8"),
    ];
    for (p8, kid, team, bundle, want) in cases {
        let e = validate_apns(p8, kid, team, bundle).expect_err(want);
        assert!(e.contains(want), "{e:?} should name {want}");
        assert!(!e.contains("BEGIN") && !e.contains("not a key"), "never echoes the key: {e}");
        for given in [kid, team, bundle] {
            if !["ABC123DEFG", "TEAM123456", "com.example.app"].contains(&given) {
                assert!(!e.contains(given), "never echoes {given:?}: {e}");
            }
        }
    }
    let huge = "x".repeat(MAX_P8_BYTES + 1);
    assert!(validate_apns(&huge, "ABC123DEFG", "TEAM123456", "com.example.app").expect_err("huge").contains("too large"));
}

#[test]
fn a_service_account_is_checked_and_its_token_uri_is_pinned() {
    let k = validate_fcm(&sa(json!({}))).expect("valid");
    assert_eq!((k.project_id.as_str(), k.client_email.as_str()), ("k2-test-proj", "push@k2-test-proj.iam.gserviceaccount.com"));
    assert!(!format!("{k:?}").contains("PRIVATE"));
    validate_fcm(&sa(json!({"token_uri": null}))).expect("absent token_uri = Google's");
    for (over, want) in [
        (json!({"token_uri": "https://evil.example/token"}), "token_uri must be exactly"),
        (json!({"token_uri": "https://oauth2.googleapis.com/token/../x"}), "token_uri must be exactly"),
        (json!({"token_uri": "http://oauth2.googleapis.com/token"}), "token_uri must be exactly"),
        (json!({"type": "authorized_user"}), "type must be service_account"),
        (json!({"project_id": "Bad Project"}), "project_id"),
        (json!({"project_id": null}), "project_id"),
        (json!({"client_email": "no-at-sign"}), "client_email"),
        (json!({"private_key": EC_P256_P8}), "RSA"),
        (json!({"private_key": "garbage"}), "private_key"),
    ] {
        let e = validate_fcm(&sa(over.clone())).expect_err(&over.to_string());
        assert!(e.contains(want), "{e:?} should say {want:?} for {over}");
        assert!(!e.contains("evil.example") && !e.contains("BEGIN"), "never echoes: {e}");
    }
    assert!(validate_fcm("{not json").expect_err("json").contains("JSON"));
    let jwt = fcm_assertion(&k, 1_800_000_000).expect("mint");
    let header = jsonwebtoken::decode_header(&jwt).expect("header");
    assert_eq!((header.alg, header.kid.as_deref()), (jsonwebtoken::Algorithm::RS256, Some("abc123")));
    let mut val = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    val.validate_exp = false;
    val.set_audience(&[GOOGLE_TOKEN_URI]);
    let dec = jsonwebtoken::DecodingKey::from_rsa_pem(RSA_PUB.as_bytes()).expect("pub");
    let c = jsonwebtoken::decode::<serde_json::Value>(&jwt, &dec, &val).expect("verifies").claims;
    assert_eq!(
        c,
        json!({"iss": "push@k2-test-proj.iam.gserviceaccount.com", "scope": FCM_SCOPE, "aud": GOOGLE_TOKEN_URI, "iat": 1_800_000_000, "exp": 1_800_003_600})
    );
}

fn app_id(tag: &str) -> String {
    format!("svc-creds-{tag}-{}", uuid::Uuid::new_v4().simple())
}

#[test]
fn keys_round_trip_in_the_system_namespace_and_are_destroyed_on_purge() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let app = app_id("rt");
    let k = validate_apns(EC_P256_P8, "ABC123DEFG", "TEAM123456", "com.example.app").expect("apns");
    let f = validate_fcm(&sa(json!({}))).expect("fcm");
    assert_eq!(load_apns(&app), Err(KeyError::NotConfigured));
    let cfg = store_apns(&app, &k, "owner").expect("store apns");
    assert_eq!(cfg.apns.as_ref().map(|m| m.key_fingerprint.clone()), Some(k.fingerprint()));
    store_fcm(&app, &f, "owner").expect("store fcm");
    assert_eq!(load_apns(&app).expect("load"), k);
    assert_eq!(load_fcm(&app).expect("load"), f);
    // The user namespace never sees them (no k2v:// reference, no route).
    let user = vault.resolve(
        &Selector::user(Scope::app(app.clone()), APNS_SECRET),
        Use::System,
        &Requester::System(SUBSYSTEM.into()),
    );
    assert!(user.is_err(), "not in the user namespace");
    // Another App's scope is empty.
    assert_eq!(load_apns(&app_id("other")), Err(KeyError::NotConfigured));
    assert!(remove(&app, "apns", "owner").expect("remove"));
    assert_eq!(load_apns(&app), Err(KeyError::NotConfigured));
    assert!(!remove(&app, "apns", "owner").expect("again"), "nothing left");
    forget_app(&app);
    assert_eq!(get(&app, FCM_SECRET).expect("read"), None, "purge destroys the vault secret");
}
