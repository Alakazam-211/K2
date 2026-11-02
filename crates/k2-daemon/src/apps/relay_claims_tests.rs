//! The claim client's wire: what each HTTP answer means. The state machine
//! on top of it is in `custom_domain_flow_tests.rs` (Path B).

use super::*;

const HOST: &str = "shop.example.com";

#[test]
fn a_2xx_is_the_body_and_a_refusal_is_its_code_in_plain_words() {
    let v = parse_reply(200, r#"{"hostname":"shop.example.com","status":"verified"}"#, HOST).unwrap();
    assert_eq!(answer(&v, HOST).unwrap(), ClaimAnswer { hostname: HOST.into(), status: "verified".into(), last_error: None });
    let v = parse_reply(201, r#"{"status":"Pending","lastError":"no TXT"}"#, HOST).unwrap();
    assert_eq!(
        answer(&v, HOST).unwrap(),
        ClaimAnswer { hostname: HOST.into(), status: "pending".into(), last_error: Some("no TXT".into()) }
    );
    for (status, body, code, want) in [
        (403, r#"{"error":"pro_required"}"#, "pro_required", UPGRADE_URL),
        (409, r#"{"error":"hostname_claimed_elsewhere"}"#, "hostname_claimed_elsewhere", "another K2 account"),
        (409, r#"{"error":"cap_reached","count":20,"cap":20}"#, "cap_reached", "20 of 20"),
        (422, r#"{"error":"capability_missing"}"#, "capability_missing", "k2 update"),
        (400, r#"{"error":"public_suffix","detail":"example.com is a public suffix"}"#, "public_suffix", "example.com is a public suffix"),
    ] {
        match parse_reply(status, body, HOST) {
            Err(CpError::Refused { code: c, words }) => {
                assert_eq!(c, code);
                assert!(words.contains(want), "{code}: {words}");
            }
            other => panic!("{code}: {other:?}"),
        }
    }
}

/// No code, a 5xx, a 404 (the service isn't live yet) or a bad body: no
/// answer, never a refusal that would stick to the name.
#[test]
fn anything_else_is_no_answer() {
    assert!(matches!(parse_reply(502, "bad gateway", HOST), Err(CpError::Unreachable(_))));
    assert!(matches!(parse_reply(503, r#"{"error":"down"}"#, HOST), Err(CpError::Unreachable(_))));
    assert!(matches!(parse_reply(404, "", HOST), Err(CpError::Unreachable(_))));
    assert!(matches!(parse_reply(401, "", HOST), Err(CpError::Refused { code, .. }) if code == "unauthorized"));
    let v = parse_reply(200, "not json", HOST).unwrap();
    assert!(matches!(answer(&v, HOST), Err(CpError::Unreachable(_))));
}

/// The URL: the override, trimmed; a hostname can't change the path.
#[test]
fn the_endpoint_and_hostname_segment() {
    let _e = k2_core::test_env::EnvVar::set(BASE_ENV, " https://cp.example/ ");
    assert_eq!(endpoint(), "https://cp.example/api/custom-hostnames");
    assert_eq!(seg("Shop.Example.COM."), "shop.example.com");
    assert_eq!(seg("a/b?c"), "a%2Fb%3Fc");
}

/// The body the claim sends carries the capability (R1 refuses without it).
#[test]
fn the_capability_is_customsni() {
    assert_eq!(CAPABILITY, "customSni");
}
