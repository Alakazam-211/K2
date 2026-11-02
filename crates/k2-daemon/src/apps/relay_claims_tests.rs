//! The claim client's wire: what each HTTP answer means. The state machine
//! on top of it is in `custom_domain_flow_tests.rs` (Path B).

use super::*;

const HOST: &str = "shop.example.com";

/// Every refusal the control plane answers (k2-connect PR #43), as it
/// answers it: which kind it is, and that its words say what to do.
#[test]
fn every_control_plane_refusal_maps_to_plain_words() {
    let v = parse_reply(201, r#"{"hostname":"shop.example.com","status":"pending","httpRedirect":true}"#, HOST).unwrap();
    assert_eq!(answer(&v, HOST).unwrap().status, "pending", "a new claim");
    let v = parse_reply(200, r#"{"hostname":"shop.example.com","status":"verified"}"#, HOST).unwrap();
    assert_eq!(answer(&v, HOST).unwrap(), ClaimAnswer { hostname: HOST.into(), status: "verified".into(), last_error: None });
    for (status, body, code, want) in [
        (401, r#"{"error":"invalid_token","detail":"x"}"#, "invalid_token", "the owner re-pairs"),
        (400, r#"{"error":"capability_missing"}"#, "capability_missing", "the owner runs k2 update"),
        (403, r#"{"error":"nested_label"}"#, "nested_label", "main name"),
        (403, r#"{"error":"pro_required"}"#, "pro_required", UPGRADE_URL),
        (400, r#"{"error":"hostname_invalid","detail":"no dots"}"#, "hostname_invalid", "no dots"),
        (400, r#"{"error":"hostname_not_allowed","detail":"a public suffix"}"#, "hostname_not_allowed", "a public suffix"),
        (400, r#"{"error":"bad_verify_token"}"#, "bad_verify_token", "add it again"),
        (409, r#"{"error":"hostname_claimed_elsewhere","detail":"x is already verified by another K2 account"}"#, "hostname_claimed_elsewhere", "another K2 account"),
        (409, r#"{"error":"hostname_claimed_elsewhere","detail":"x is held by another box on your account ('b2'); release it there first"}"#, "hostname_claimed_elsewhere", "another box on your account"),
        (403, r#"{"error":"cap_reached","scope":"box","count":20,"limit":20}"#, "cap_reached", "this server already holds 20 of 20"),
        (403, r#"{"error":"cap_reached","scope":"account","count":100,"limit":100}"#, "cap_reached", "this K2 account already holds 100 of 100"),
        (404, r#"{"error":"not_claimed"}"#, "not_claimed", "claims it again"),
        (422, r#"{"error":"verification_failed","detail":"TXT does not hold it"}"#, "verification_failed", "TXT does not hold it"),
    ] {
        match parse_reply(status, body, HOST) {
            Err(CpError::Refused { code: c, words }) => {
                assert_eq!(c, code);
                assert!(words.contains(want), "{code}: {words}");
            }
            other => panic!("{code}: {other:?}"),
        }
    }
    // These may go through later: nothing sticks to the name.
    for (status, body, want) in [
        (429, r#"{"error":"rate_limited","count":5,"limit":5}"#, "5 new names an hour"),
        (409, r#"{"error":"conflict"}"#, "same moment"),
        (502, r#"{"error":"supabase_error"}"#, "database"),
        (503, r#"{"error":"dns_unavailable"}"#, "look up"),
        (503, r#"{"error":"custom_hostnames_disabled"}"#, "hasn't turned on"),
    ] {
        match parse_reply(status, body, HOST) {
            Err(CpError::Retry(words)) => assert!(words.contains(want) && words.contains("tries again"), "{words}"),
            other => panic!("{body}: {other:?}"),
        }
    }
}

/// No code, a bare 5xx, a 404 (the service isn't live yet) or a bad body: no
/// answer, never a refusal that would stick to the name.
#[test]
fn anything_else_is_no_answer() {
    assert!(matches!(parse_reply(502, "bad gateway", HOST), Err(CpError::Unreachable(_))));
    assert!(matches!(parse_reply(404, "", HOST), Err(CpError::Unreachable(_))));
    assert!(matches!(parse_reply(401, "", HOST), Err(CpError::Refused { code, .. }) if code == "invalid_token"));
    let v = parse_reply(200, "not json", HOST).unwrap();
    assert!(matches!(answer(&v, HOST), Err(CpError::Unreachable(_))));
}

/// The URL: the override, trimmed; a hostname is normalized for the body.
#[test]
fn the_endpoint_and_hostname_argument() {
    let _e = k2_core::test_env::EnvVar::set(BASE_ENV, " https://cp.example/ ");
    assert_eq!(endpoint(), "https://cp.example/api/custom-hostnames");
    assert_eq!(host_arg("Shop.Example.COM."), "shop.example.com");
}

/// The body the claim sends carries the capability (R1 refuses without it).
#[test]
fn the_capability_is_customsni() {
    assert_eq!(CAPABILITY, "customSni");
}

/// `verifyToken` must be 16-128 of A-Za-z0-9_-: K2's own always is.
#[test]
fn k2s_verify_tokens_fit_the_control_planes_rule() {
    let t = k2_core::domains::new_verify_token();
    assert!((16..=128).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'), "{t}");
}

/// The relay's :80 switch: only when this box holds a name; its word for it
/// reaches the owner.
#[test]
fn the_relay_redirect_switch_goes_to_the_control_plane_only_with_names() {
    struct Cp(std::cell::RefCell<Vec<String>>);
    impl RelayApi for Cp {
        fn claim(&self, _: &str, _: &str, _: bool) -> Result<ClaimAnswer, CpError> {
            panic!("no claim")
        }
        fn verify(&self, _: &str) -> Result<ClaimAnswer, CpError> {
            panic!("no verify")
        }
        fn release(&self, _: &str) -> Result<(), CpError> {
            panic!("no release")
        }
        fn list(&self) -> Result<Vec<ClaimAnswer>, CpError> {
            panic!("no list")
        }
        fn http_redirect(&self, h: Option<&str>, on: bool) -> Result<(), CpError> {
            self.0.borrow_mut().push(format!("{h:?} {on}"));
            Ok(())
        }
    }
    let _db = k2_core::db::scoped_for_test();
    let cp = Cp(Default::default());
    assert_eq!(set_relay_redirect(false, &cp), Ok(false), "no names: nothing to tell it");
    assert!(cp.0.borrow().is_empty());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_binding(&conn, "example.com", None, false).unwrap();
        k2_core::domains::upsert_name(&conn, HOST, "example.com", "app:shop").unwrap();
        k2_core::domains::set_relay_state(&conn, HOST, Some(k2_core::domains::RELAY_VERIFIED), None, 1).unwrap();
    }
    assert_eq!(set_relay_redirect(false, &cp), Ok(true));
    assert_eq!(*cp.0.borrow(), vec!["None false".to_string()], "all of this box's names");
}
