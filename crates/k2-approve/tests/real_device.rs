//! Real-device fixtures from S0 (fixtures/sa1/real-device): an iPhone's
//! Secure Enclave signatures and its App Attest attestation.
#![cfg(feature = "verify")]

use base64::Engine;
use k2_approve::attest::{self, K2_PHONE_APP_ID};
use serde_json::Value;
use k2_approve::{keys, payload, time, Environment};
use sha2::{Digest, Sha256};
use std::path::Path;

fn read(rel: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sa1").join(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn obj(rel: &str) -> Value {
    serde_json::from_slice(&read(rel)).expect("JSON")
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or_else(|| panic!("no {k}"))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    get(v, k).as_str().unwrap_or_else(|| panic!("{k} is not a string"))
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).expect("base64")
}

fn at(t: &str) -> i64 {
    time::parse_utc(t).expect("time")
}

#[test]
fn phone_signatures_verify_over_the_exact_bytes() {
    let sigs = obj("real-device/s0-sigs.json");
    for name in ["sign_biometric", "sign_passcode"] {
        let e = get(&sigs, name);
        let bytes = b64(s(e, "payload_b64"));
        let sig = b64(s(e, "sig_der_b64"));
        let key = keys::parse_spki(&keys::pem_to_der(s(e, "pub_pem")).expect("pem")).expect("P-256");
        assert!(k2_approve::verify::signature_ok(&key, &bytes, &sig), "{name}: the phone's signature verifies");
        let mut changed = bytes.clone();
        let last = changed.len() - 2;
        changed[last] ^= 1;
        assert!(!k2_approve::verify::signature_ok(&key, &changed, &sig), "{name}: a one-byte change does not");
        // The spike body predates k2.release v1's shape.
        assert_eq!(payload::parse(&bytes).expect_err(name).code.as_str(), "shape", "{name}");
        // The payload lists this key's id: lowercase hex SHA-256 of its SPKI.
        let id = keys::key_id(&key.spki);
        assert!(String::from_utf8_lossy(&bytes).contains(&id), "{name}: approver_keys names {id}");
    }
}

#[test]
fn the_pinned_apple_root_is_apples() {
    let der = keys::pem_block_to_der(attest::APPLE_APP_ATTEST_ROOT_PEM, "CERTIFICATE").expect("root PEM");
    let h: String = Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(h, attest::APPLE_APP_ATTEST_ROOT_SHA256);
}

#[test]
fn the_phone_attestation_checks_out_once_and_is_stored() {
    let exp = obj("real-device/expect.json");
    let ea = get(&exp, "attest");
    let a = obj("real-device/s0-attest.json");
    let att = b64(s(&a, "attestation_b64"));
    let cd = b64(s(&a, "client_data_b64"));
    let kid = s(&a, "attest_key_id");
    assert_eq!(s(ea, "app_id"), K2_PHONE_APP_ID);
    let got = attest::verify_app_attest(&att, &cd, kid, K2_PHONE_APP_ID, at(s(ea, "check_at"))).expect("the S0 attestation verifies");
    assert_eq!(got.environment, Environment::parse(s(ea, "environment")).expect("env"));
    assert_eq!(got.leaf_not_after, at(s(ea, "leaf_not_after")));
    assert_eq!(got.verified_at, at(s(ea, "check_at")));
    // The spike clientData is the two keys; their ids are the phone's.
    assert_eq!(keys::key_id(&cd[..91]), s(&a, "biometric_key_id"));
    assert_eq!(keys::key_id(&cd[91..]), s(&a, "passcode_key_id"));

    let now = at(s(ea, "check_at"));
    let refuse = |att: &[u8], cd: &[u8], kid: &str, app: &str, now: i64, want: &str| {
        let e = attest::verify_app_attest(att, cd, kid, app, now).expect_err(want);
        assert!(e.contains(want), "{e} (wanted {want})");
    };
    refuse(&att, &cd, kid, K2_PHONE_APP_ID, at(s(ea, "refused_at")), "not valid now");
    refuse(&att, &cd, kid, "36B8R93HXV.com.example.other", now, "rpIdHash");
    let mut other_cd = cd.clone();
    other_cd[0] ^= 1;
    refuse(&att, &other_cd, kid, K2_PHONE_APP_ID, now, "nonce");
    refuse(&att, &cd, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", K2_PHONE_APP_ID, now, "not the App Attest key named");
    let mut cut = att.clone();
    cut.truncate(att.len() - 10);
    assert!(attest::verify_app_attest(&cut, &cd, kid, K2_PHONE_APP_ID, now).is_err());
    // The enrolment check wants the K2-APPROVER-ENROL-1 form.
    let e = attest::verify_enrolment(&att, &cd, kid, &[0u8; 32], K2_PHONE_APP_ID, now).expect_err("spike form");
    assert!(e.contains("K2-APPROVER-ENROL-1"), "{e}");
}

#[test]
fn the_enrolment_client_data_form() {
    let a = obj("real-device/s0-attest.json");
    let keys2 = b64(s(&a, "client_data_b64"));
    let challenge = [7u8; 32];
    let mut cd = attest::ENROL_PREFIX.to_vec();
    cd.extend_from_slice(&challenge);
    cd.extend_from_slice(&keys2);
    let (c, bio, pass) = attest::parse_enrol_client_data(&cd).expect("the enrolment form");
    assert_eq!((c, bio.as_slice(), pass.as_slice()), (challenge, &keys2[..91], &keys2[91..]));
    assert!(attest::parse_enrol_client_data(&cd[1..]).is_err(), "no prefix");
    assert!(attest::parse_enrol_client_data(&cd[..cd.len() - 1]).is_err(), "cut short");
    let mut twice = attest::ENROL_PREFIX.to_vec();
    twice.extend_from_slice(&challenge);
    twice.extend_from_slice(&keys2[..91]);
    twice.extend_from_slice(&keys2[..91]);
    assert!(attest::parse_enrol_client_data(&twice).expect_err("same key").contains("same key"));
    // A real attestation over other data never passes the enrolment check.
    let e = attest::verify_enrolment(&b64(s(&a, "attestation_b64")), &cd, s(&a, "attest_key_id"), &challenge, K2_PHONE_APP_ID, at("2026-11-11T12:00:00Z")).expect_err("nonce");
    assert!(e.contains("nonce"), "{e}");
    let e = attest::verify_enrolment(&b64(s(&a, "attestation_b64")), &cd, s(&a, "attest_key_id"), &[8u8; 32], K2_PHONE_APP_ID, at("2026-11-11T12:00:00Z")).expect_err("challenge");
    assert!(e.contains("challenge"), "{e}");
}
