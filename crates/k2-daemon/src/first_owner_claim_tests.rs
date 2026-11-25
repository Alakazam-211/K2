//! The claim door's own check ([`admit`]) for every ingress, and the claim
//! itself ([`handle`]) against the login store. The dispatcher path (real
//! HTTP on loopback and the tunnel) is in
//! `tests/connect_login_edge_integration.rs`.

use super::*;
use k2_core::edge_attest::{random_nonce, EdgeSigner};

const SUB: &str = "rosson";
const BODY: &[u8] = br#"{"username":"alice","password":"claimed-pass-123"}"#;

fn signed(signer: &EdgeSigner, sub: &str, body: &[u8]) -> String {
    signer.header("POST", PATH, sub, k2_core::edge_attest::now_unix(), &random_nonce(), "-", body)
}

fn admit_now(ingress: Ingress, is_post: bool, header: Option<&str>, body: &[u8]) -> Result<String, String> {
    admit(ingress, is_post, header, PATH, body, SUB, k2_core::edge_attest::now_unix()).map(|a| a.kid)
}

/// Only the tunnel: loopback (the CLI, agents, the owner token), LAN and an
/// App's gateway link are refused before the signature is even looked at,
/// a valid dashboard signature included. Every variant is named here, so a
/// new ingress has to be decided on.
#[test]
fn only_the_tunnel_with_a_dashboard_signature_is_admitted() {
    let _home = k2_core::test_env::TempHome::new();
    let dash = EdgeSigner::generate("k2-edge-dash-unit").expect("key");
    dash.install_trust_file().expect("trust");
    for ingress in [Ingress::Loopback, Ingress::Lan, Ingress::App, Ingress::Tunnel] {
        let h = signed(&dash, SUB, BODY);
        let got = admit_now(ingress, true, Some(&h), BODY);
        match ingress {
            Ingress::Tunnel => assert_eq!(got, Ok("k2-edge-dash-unit".to_string()), "the tunnel with the dashboard's signature"),
            Ingress::Loopback | Ingress::Lan | Ingress::App => assert_eq!(got, Err("off_tunnel".to_string()), "{ingress:?}"),
        }
    }
    let h = signed(&dash, SUB, BODY);
    assert_eq!(admit_now(Ingress::Tunnel, false, Some(&h), BODY), Err("not_post".to_string()));
}

#[test]
fn a_missing_unknown_app_or_wrong_server_signature_is_refused() {
    let _home = k2_core::test_env::TempHome::new();
    let dash = EdgeSigner::generate("k2-edge-dash-unit").expect("key");
    let app = EdgeSigner::generate("k2-edge-app-unit").expect("key");
    let unbaked = EdgeSigner::generate("k2-edge-dash-unbaked").expect("key");
    let trust = serde_json::json!({"keys": [
        {"kid": dash.kid(), "pub": dash.public_key_b64()},
        {"kid": app.kid(), "pub": app.public_key_b64()},
    ]});
    std::fs::write(k2_core::edge_attest::override_path(), trust.to_string()).expect("edge-keys.json");

    assert_eq!(admit_now(Ingress::Tunnel, true, None, BODY), Err("bad_attest:missing".to_string()));
    let h = signed(&unbaked, SUB, BODY);
    assert_eq!(admit_now(Ingress::Tunnel, true, Some(&h), BODY), Err("bad_attest:unknown_kid".to_string()), "a dashboard kid nobody trusts");
    let h = signed(&app, SUB, BODY);
    assert_eq!(admit_now(Ingress::Tunnel, true, Some(&h), BODY), Err("bad_attest:unknown_kid".to_string()), "the app Worker is not the dashboard");
    let h = signed(&dash, "someone-else", BODY);
    assert_eq!(admit_now(Ingress::Tunnel, true, Some(&h), BODY), Err("bad_attest:bad_sig".to_string()), "signed for another server");
    let h = signed(&dash, SUB, BODY);
    assert_eq!(admit_now(Ingress::Tunnel, true, Some(&h), br#"{"username":"mallory","password":"x"}"#), Err("bad_attest:bad_sig".to_string()), "another body");
    assert_eq!(
        admit(Ingress::Tunnel, true, Some(&signed(&dash, SUB, BODY)), PATH, BODY, "", k2_core::edge_attest::now_unix()).map(|a| a.kid),
        Err("bad_attest:no_subdomain".to_string()),
        "a server with no tunnel subdomain can't be claimed"
    );
    assert_eq!(admit_now(Ingress::Tunnel, true, Some("v1;garbage"), BODY), Err("bad_attest:malformed".to_string()));
}

fn status(r: &CliResponse) -> &str {
    r.status
}

fn code(r: &CliResponse) -> String {
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON body");
    v["error"]["code"].as_str().expect("error.code").to_string()
}

#[test]
fn the_claim_makes_the_first_owner_once_and_never_with_an_owner_present() {
    let _home = k2_core::test_env::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    let r = handle(br#"{"username":"Alice","password":"claimed-pass-123","k2Account":"acct-7"}"#, "edge:k2-edge-dash-unit");
    assert_eq!(status(&r), "200 OK", "{}", r.body);
    assert_eq!(connect_users::role_for_user("alice"), Some(Role::Owner));
    assert!(!connect_users::must_change_password_usernames().expect("flags").contains("alice"), "the person's own password, not temporary");
    let r = handle(br#"{"username":"bob","password":"claimed-pass-123"}"#, "edge:k2-edge-dash-unit");
    assert_eq!(status(&r), "409 Conflict");
    assert_eq!(code(&r), "owner_exists");
    assert!(connect_users::role_for_user("bob").is_none());
}

#[test]
fn a_disabled_owner_still_counts() {
    let _home = k2_core::test_env::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    connect_users::add_user("old", "password123").expect("add");
    connect_users::set_role("old", Role::Owner).expect("role");
    connect_users::set_disabled("old", true).expect("disable");
    let r = handle(BODY, "edge:k2-edge-dash-unit");
    assert_eq!(status(&r), "409 Conflict", "{}", r.body);
    assert_eq!(code(&r), "owner_exists");
    assert!(connect_users::role_for_user("alice").is_none());
}

#[test]
fn an_existing_login_is_never_promoted_and_a_weak_password_is_refused() {
    let _home = k2_core::test_env::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    connect_users::add_user("alice", "member-pass-123").expect("add");
    let r = handle(BODY, "edge:k2-edge-dash-unit");
    assert_eq!(code(&r), "username_taken");
    assert_eq!(connect_users::role_for_user("alice"), Some(Role::Member));
    let r = handle(br#"{"username":"carol","password":""}"#, "edge:k2-edge-dash-unit");
    assert_eq!(status(&r), "400 Bad Request", "{}", r.body);
    assert!(connect_users::role_for_user("carol").is_none());
    let r = handle(b"not json", "edge:k2-edge-dash-unit");
    assert_eq!(status(&r), "400 Bad Request");
}

#[test]
fn the_ops_login_is_made_with_the_owner_or_nothing_is() {
    let _home = k2_core::test_env::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    // The same name twice: refused, nothing made.
    let r = handle(br#"{"username":"buyer","password":"initial-pass-123","opsUsername":"Buyer"}"#, "edge:k2-edge-dash-unit");
    assert_eq!(status(&r), "400 Bad Request", "{}", r.body);
    assert!(connect_users::role_for_user("buyer").is_none());
    // An ops name that exists: 409 naming it, and the buyer isn't made.
    connect_users::add_user("k2cloud", "old-ops-pass-1").expect("add");
    let r = handle(br#"{"username":"buyer","password":"initial-pass-123","opsUsername":"k2cloud"}"#, "edge:k2-edge-dash-unit");
    assert_eq!(code(&r), "username_taken", "{}", r.body);
    assert!(r.body.contains("k2cloud"), "{}", r.body);
    assert!(connect_users::role_for_user("buyer").is_none(), "nothing made");
    assert_eq!(connect_users::role_for_user("k2cloud"), Some(Role::Member), "never promoted");
    connect_users::remove_user("k2cloud").expect("remove");
    // Both, in one claim: two owners, k2cloud's password returned once.
    let r = handle(br#"{"username":"buyer","password":"initial-pass-123","opsUsername":"k2cloud"}"#, "edge:k2-edge-dash-unit");
    assert_eq!(status(&r), "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON");
    let pw = v["opsPassword"].as_str().expect("opsPassword");
    assert_eq!(connect_users::role_for_user("k2cloud"), Some(Role::Owner));
    assert_eq!(connect_users::role_for_user("buyer"), Some(Role::Owner));
    assert!(connect_users::verify("k2cloud", pw), "the returned password is k2cloud's");
    assert!(connect_users::verify("buyer", "initial-pass-123"));
}

#[test]
fn a_claim_without_an_ops_name_returns_no_ops_fields() {
    let _home = k2_core::test_env::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    let r = handle(br#"{"username":"solo","password":"initial-pass-123"}"#, "edge:k2-edge-dash-unit");
    assert_eq!(status(&r), "200 OK", "{}", r.body);
    assert!(!r.body.contains("ops"), "{}", r.body);
}
