//! Third-party tokens (§12 "Third-party token tests (S1b)"): every refusal
//! has its own code; a good token maps to the App's user under both `sub`
//! settings. Keys come from a fetch seam, never the network. One test
//! function: the seam and the key cache are process-wide.

use std::sync::atomic::{AtomicBool, Ordering};

use jsonwebtoken::{EncodingKey, Header};
use serde_json::json;

use super::super::door_config::{self, DoorConfig, Limits, Stored};
use super::*;

const KEY_A: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgGyVxU83C6HE3EjUm
TXjVN4yUpFKKa1em76DnHRN5W/GhRANCAATkQNPibhjrf+JGkcc2k+XhEGqcjOCJ
Dj1AwfbRmTwo4u/b/ySUsmhxOAuzKvuRsvxE7u5SMeF3H38raCsVcxDR
-----END PRIVATE KEY-----
";
const KEY_B: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgP1YHSOriOBd0RZV/
GKZerZN/cOQKyJ54ce26a1YYHMihRANCAATip5hLmfm9qQp8xwpy+yV4+pJzRkTc
9GZ9gbT2EJ1mYvzGr4Ln8FkR3DRyHzHwJ09ymntYIz0hXmo4tYp96qSR
-----END PRIVATE KEY-----
";
/// Key A's public half, the only key the issuer publishes.
const JWKS: &str = r#"{"keys":[{"kty":"EC","crv":"P-256","kid":"ka","alg":"ES256","use":"sig",
  "x":"5EDT4m4Y63_iRpHHNpPl4RBqnIzgiQ49QMH20Zk8KOI","y":"79v_JJSyaHE4C7Mq-5Gy_ETu7lIx4XcffytoKxVzENE"}]}"#;

const ISS: &str = "https://securetoken.example/proj";
const AUD: &str = "proj";
const APP: &str = "fieldapp";
const SVC: &str = "svc-fieldapp";

static DOWN: AtomicBool = AtomicBool::new(false);
static FETCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn fake_fetch(url: &str) -> Fetched {
    FETCHES.fetch_add(1, Ordering::SeqCst);
    assert_eq!(url, "https://keys.example/jwks.json", "only the pinned key-set URL is fetched");
    if DOWN.load(Ordering::SeqCst) {
        return Err("connection refused".into());
    }
    Ok((JWKS.to_string(), Some(3600)))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn token(key: &str, kid: &str, claims: serde_json::Value) -> String {
    let mut h = Header::new(jsonwebtoken::Algorithm::ES256);
    h.kid = Some(kid.into());
    jsonwebtoken::encode(&h, &claims, &EncodingKey::from_ec_pem(key.as_bytes()).expect("test key")).expect("sign")
}

fn good(email: &str, sub: &str) -> serde_json::Value {
    let t = now();
    json!({ "iss": ISS, "aud": AUD, "sub": sub, "email": email, "email_verified": true, "iat": t, "exp": t + 3600 })
}

fn config(sub_mode: SubMode, unknown: UnknownEmail) {
    door_config::upsert(
        &Stored {
            config: DoorConfig {
                app_id: APP.into(),
                database: "ws_f_main".into(),
                database_id: "db-f".into(),
                authenticator: "ws_f_main_authenticator".into(),
                authenticator_secret_ref: "ref".into(),
                anon: false,
                service_key_public: false,
                origins: vec![],
                schemas: vec!["public".into()],
                limits: Limits::default(),
                sub_mode,
                unknown_email: unknown,
            },
            claims_key_ref: "ref-key".into(),
            users_synced_at: None,
            created_by: "owner".into(),
            created_at: 1,
        },
        1,
    )
    .unwrap();
}

fn code(r: Result<Verified, TokenRefusal>) -> String {
    match r {
        Ok(v) => format!("ok {}", v.claims),
        Err(e) => format!("{} {}", e.code, e.status),
    }
}

#[test]
fn third_party_tokens_are_checked_mapped_and_each_refusal_has_its_code() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    set_fetcher(Some(fake_fetch));
    DOWN.store(false, Ordering::SeqCst);
    config(SubMode::Token, UnknownEmail::Refuse);
    add(&Issuer { app_id: APP.into(), key: "firebase".into(), issuer: ISS.into(), audience: AUD.into(), jwks_url: "https://keys.example/jwks.json".into() }, "owner", 1).unwrap();
    assert!(add(&Issuer { app_id: APP.into(), key: "x".into(), issuer: "i".into(), audience: "a".into(), jwks_url: "http://keys.example/j".into() }, "owner", 1).unwrap_err().contains("https"));
    let ann = k2_core::skin::add_principal_for_app("tp-ann", Some("Ann@Example.com"), SVC, None, "owner").expect("ann");
    let bob = k2_core::skin::add_principal_for_app("tp-bob", Some("bob@example.com"), SVC, None, "owner").expect("bob");
    k2_core::skin::compat::set_metadata(&ann.id, Some(&json!({"tier": "gold"})), None).unwrap();
    let t = now();

    // A good token: sub stays the issuer's id (setting a), the user's
    // metadata and the App ride along, the pair is recorded.
    let v = verify(APP, SVC, &token(KEY_A, "ka", good("ann@example.com", "fb-ann")), t).expect("good token");
    assert_eq!(v.role, DoorRole::Authenticated);
    assert_eq!(v.user_id.as_deref(), Some(ann.id.as_str()));
    assert_eq!(v.claims["sub"], "fb-ann");
    assert_eq!(v.claims["email"], "ann@example.com");
    assert_eq!(v.claims["iss"], ISS);
    assert_eq!(v.claims["app_metadata"], json!({"tier": "gold", "k2_app": APP}));
    assert_eq!(k2_core::skin::compat::linked_sub(SVC, ISS, &ann.id).unwrap().as_deref(), Some("fb-ann"));

    // Setting (b): sub is the App user's id; the issuer's id moves.
    config(SubMode::User, UnknownEmail::Refuse);
    let v = verify(APP, SVC, &token(KEY_A, "ka", good("ann@example.com", "fb-ann")), t).expect("good token, sub user");
    assert_eq!(v.claims["sub"], json!(ann.id));
    assert_eq!(v.claims["firebase_uid"], "fb-ann");
    config(SubMode::Token, UnknownEmail::Refuse);

    let cases: Vec<(&str, String, &str)> = vec![
        ("forged signature (key B under key A's id)", token(KEY_B, "ka", good("ann@example.com", "fb-ann")), "token_signature 401"),
        ("a key the issuer doesn't publish", token(KEY_B, "kb", good("ann@example.com", "fb-ann")), "token_signature 401"),
        ("wrong issuer", token(KEY_A, "ka", { let mut c = good("ann@example.com", "fb-ann"); c["iss"] = json!("https://evil.example"); c }), "issuer_unknown 401"),
        ("wrong audience", token(KEY_A, "ka", { let mut c = good("ann@example.com", "fb-ann"); c["aud"] = json!("other"); c }), "token_audience 401"),
        ("expired", token(KEY_A, "ka", { let mut c = good("ann@example.com", "fb-ann"); c["iat"] = json!(t - 7200); c["exp"] = json!(t - 120); c }), "token_expired 401"),
        ("issued in the future", token(KEY_A, "ka", { let mut c = good("ann@example.com", "fb-ann"); c["iat"] = json!(t + 600); c }), "token_expired 401"),
        ("email not verified", token(KEY_A, "ka", { let mut c = good("ann@example.com", "fb-ann"); c["email_verified"] = json!(false); c }), "email_unverified 401"),
        ("no email_verified at all", token(KEY_A, "ka", { let mut c = good("ann@example.com", "fb-ann"); c.as_object_mut().unwrap().remove("email_verified"); c }), "email_unverified 401"),
        ("an email that is no user of the App", token(KEY_A, "ka", good("stranger@example.com", "fb-x")), "not_a_user 403"),
        ("a known sub with another user's email", token(KEY_A, "ka", good("bob@example.com", "fb-ann")), "identity_changed 403"),
        ("a new sub for a user who has one", token(KEY_A, "ka", good("ann@example.com", "fb-other")), "identity_changed 403"),
        ("HS256", jsonwebtoken::encode(&Header::new(jsonwebtoken::Algorithm::HS256), &good("ann@example.com", "fb-ann"), &EncodingKey::from_secret(b"s")).unwrap(), "token_algorithm 401"),
        ("not a token", "abc.def.ghi".to_string(), "token_malformed 401"),
    ];
    for (name, tok, want) in &cases {
        assert_eq!(code(verify(APP, SVC, tok, t)), *want, "{name}");
    }
    let audits: i64 = {
        let h = k2_core::db::shared();
        let c = h.lock();
        c.query_row("SELECT count(*) FROM deploy_audit WHERE event = 'identity.changed' AND subject_id = ?1", [APP], |r| r.get(0))
            .expect("the audit table")
    };
    assert_eq!(audits, 2, "each identity_changed refusal writes an audit row");
    assert!(k2_core::skin::compat::linked_sub(SVC, ISS, &bob.id).unwrap().is_none(), "a refused token records nothing");

    // Supabase's behaviour, when the App asks for it.
    config(SubMode::Token, UnknownEmail::Authenticated);
    let v = verify(APP, SVC, &token(KEY_A, "ka", good("stranger@example.com", "fb-x")), t).expect("unknown email as authenticated");
    assert_eq!((v.user_id, v.claims["sub"].clone()), (None, json!("fb-x")));
    config(SubMode::Token, UnknownEmail::Refuse);

    // Unlink, then the new pair is accepted.
    assert_eq!(k2_core::skin::compat::unlink(SVC, &ann.id).unwrap(), 1);
    assert!(verify(APP, SVC, &token(KEY_A, "ka", good("ann@example.com", "fb-other")), t).is_ok());

    // A disabled user is refused whatever the token says.
    let grant = k2_core::skin::list_grants().unwrap().into_iter().find(|g| g.subject_id == bob.id && g.target_id == SVC).expect("bob's grant");
    k2_core::skin::set_grant_enabled(&grant.id, false).unwrap();
    assert_eq!(code(verify(APP, SVC, &token(KEY_A, "ka", good("bob@example.com", "fb-bob")), t)), "user_disabled 403");

    // Keys: an unknown key id fetches again at most once a minute.
    let before = FETCHES.load(Ordering::SeqCst);
    let _ = verify(APP, SVC, &token(KEY_B, "kz", good("ann@example.com", "fb-other")), t + 1);
    let _ = verify(APP, SVC, &token(KEY_B, "kz", good("ann@example.com", "fb-other")), t + 2);
    assert_eq!(FETCHES.load(Ordering::SeqCst) - before, 0, "within the minute after the last fetch: no fetch");
    let _ = verify(APP, SVC, &token(KEY_B, "kz", good("ann@example.com", "fb-other")), t + 61);
    assert_eq!(FETCHES.load(Ordering::SeqCst) - before, 1, "an unknown kid a minute later: one fetch");

    // The key URL goes down: still valid keys keep working (a warning from
    // the first failed refresh); once they expire, every token is 401.
    DOWN.store(true, Ordering::SeqCst);
    let half = t + 61 + 1801;
    assert!(verify(APP, SVC, &token(KEY_A, "ka", good("ann@example.com", "fb-other")), half).is_ok(), "past half-life, refresh fails, keys still valid");
    let st = key_status("https://keys.example/jwks.json");
    assert!(st["warning"].as_str().is_some_and(|w| w.contains("connection refused")), "{st}");
    assert_eq!(code(verify(APP, SVC, &token(KEY_A, "ka", good("ann@example.com", "fb-other")), t + 61 + 3601)), "issuer_keys_unavailable 401");
    DOWN.store(false, Ordering::SeqCst);
    assert_eq!(code(verify(APP, SVC, &token(KEY_A, "ka", good("ann@example.com", "fb-other")), t + 61 + 3601 + 60)).split(' ').next(), Some("ok"), "back once the keys can be fetched");
    set_fetcher(None);
}

/// The auth door hands supabase-js a K2 pass in a JWT's shape
/// (`header.claims.<pass>`): it must go to the pass check, never to the
/// third-party verifier. A real ES256 ID token does.
#[test]
fn a_k2_pass_in_a_jwt_shape_is_not_a_third_party_token() {
    use base64::Engine as _;
    let b64 = |v: serde_json::Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    let real = token(KEY_A, "ka", good("ann@example.com", "fb-ann"));
    assert!(looks_like_jwt(&real), "an ES256 ID token is a third-party token");
    let pass = format!("{}{}", k2_core::skin::SKIN_KEY_PREFIX, "abcDEF123_-xyz");
    for (name, h) in [
        ("an HS256 header", json!({"alg": "HS256", "typ": "JWT"})),
        ("an ES256 header", json!({"alg": "ES256", "typ": "JWT"})),
        ("an RS256 header", json!({"alg": "RS256", "typ": "JWT", "kid": "ka"})),
    ] {
        let wrapped = format!("{}.{}.{pass}", b64(h.clone()), b64(json!({"sub": "u", "role": "authenticated", "iss": "k2:app:crm"})));
        assert!(!looks_like_jwt(&wrapped), "{name}: a wrapped K2 pass goes to the pass check");
        let k2_iss = format!("{}.{}.sig", b64(h), b64(json!({"iss": "k2:app:crm"})));
        assert!(!looks_like_jwt(&k2_iss), "{name}: K2-issued claims are never a third-party token");
    }
    assert!(!looks_like_jwt(&pass), "a bare pass");
    assert!(!looks_like_jwt("a.b"), "two parts");
    assert!(!looks_like_jwt(".."), "empty parts");
}
