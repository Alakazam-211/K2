//! The shared fixtures (fixtures/sa1): every verdict and every display text
//! here must match what k2sign (Python) and the phone app (Swift) reach.

use k2_approve::json::{self, Value};
use k2_approve::keys;
use k2_approve::verify::{verify, verify_once, MemNonceStore, Require};
use k2_approve::{describe, payload, time, Check, View};
use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sa1")
}

fn read(rel: &str) -> Vec<u8> {
    std::fs::read(dir().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn manifest(rel: &str) -> Value {
    json::parse(std::str::from_utf8(&read(rel)).expect("utf8")).expect("manifest is strict JSON")
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    let Value::Obj(o) = v else { panic!("not an object: {v:?}") };
    &o.iter().find(|(n, _)| n == k).unwrap_or_else(|| panic!("no {k}")).1
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    match get(v, k) {
        Value::Str(s) => s,
        other => panic!("{k} is not a string: {other:?}"),
    }
}

fn opt_s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    match get(v, k) {
        Value::Str(s) => Some(s),
        Value::Null => None,
        other => panic!("{k}: {other:?}"),
    }
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match get(v, k) {
        Value::Arr(a) => a,
        other => panic!("{k} is not a list: {other:?}"),
    }
}

#[test]
fn valid_payloads_parse_and_read_as_written() {
    let m = manifest("payloads.json");
    let valid = arr(&m, "valid");
    assert!(valid.len() >= 5);
    for c in valid {
        let name = s(c, "name");
        let bytes = read(s(c, "payload"));
        let p = payload::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(p.bytes, bytes, "{name}: the bytes are kept as received");
        let want = String::from_utf8(read(s(c, "display"))).expect("utf8");
        assert_eq!(describe::text(&p), want, "{name}: display text");
    }
}

#[test]
fn invalid_payloads_are_refused_with_their_code() {
    let m = manifest("payloads.json");
    let invalid = arr(&m, "invalid");
    assert!(invalid.len() >= 50);
    for c in invalid {
        let name = s(c, "name");
        let got = payload::parse(&read(s(c, "payload"))).map(|_| ()).expect_err(name);
        assert_eq!(got.code.as_str(), s(c, "code"), "{name}: {}", got.detail);
    }
}

#[test]
fn key_ids_and_words_match_the_fixture_keys() {
    let m = manifest("keys.json");
    let Value::Obj(entries) = &m else { panic!("keys.json") };
    let trusted = keys::load_dir(&dir().join("keys")).expect("key set loads");
    assert_eq!(trusted.len(), 5);
    for (name, k) in entries {
        assert_eq!(keys::words(s(k, "key_id")).as_deref(), Some(s(k, "words")), "{name}");
        if matches!(get(k, "trusted"), Value::Bool(true)) {
            let t = trusted.iter().find(|t| &t.name == name).expect(name);
            assert_eq!(t.key_id, s(k, "key_id"), "{name}");
            assert_eq!(t.key.alg.as_str(), s(k, "alg"), "{name}");
        }
    }
    let bio = trusted.iter().find(|t| t.name == "alex-phone-bio").expect("bio");
    assert_eq!((bio.person.as_str(), bio.check, bio.view), ("alex", Check::Biometric, View::Device));
    let legacy = trusted.iter().find(|t| t.name == "alex-mac-legacy").expect("legacy");
    assert_eq!((legacy.check, legacy.view), (Check::Credential, View::Host));
}

#[test]
fn record_cases_reach_their_verdicts() {
    let m = manifest("cases.json");
    let trusted = keys::load_dir(&dir().join(s(&m, "keys"))).expect("key set");
    let cases = arr(&m, "cases");
    assert!(cases.len() >= 30);
    let (mut oks, mut refusals) = (0, 0);
    for c in cases {
        let name = s(c, "name");
        let rec = read(s(c, "record"));
        let stored = opt_s(c, "stored_payload").map(read);
        let r = get(c, "require");
        let req = Require {
            rp: opt_s(r, "rp"),
            kind: opt_s(r, "kind"),
            check: Check::parse(s(r, "check")).expect("check"),
            view: View::parse(s(r, "view")).expect("view"),
            now: time::parse_utc(s(r, "now")).expect("now"),
            stored_payload: stored.as_deref(),
            nonce: opt_s(r, "nonce"),
        };
        let mut store = MemNonceStore::default();
        for n in arr(c, "used_nonces") {
            let Value::Str(n) = n else { panic!("nonce") };
            store.0.insert(n.clone());
        }
        let got = verify_once(&rec, &trusted, &req, &mut store);
        let e = get(c, "expect");
        if matches!(get(e, "ok"), Value::Bool(true)) {
            let v = got.unwrap_or_else(|err| panic!("{name}: wanted ok, got {err}"));
            assert_eq!(v.key_name, s(e, "key_name"), "{name}");
            assert_eq!(v.person, s(e, "person"), "{name}");
            assert_eq!(v.key_id, s(e, "key_id"), "{name}");
            assert_eq!(v.check.as_str(), s(e, "check"), "{name}");
            assert_eq!(v.view.as_str(), s(e, "view"), "{name}");
            // Single use: the same record a second time is refused.
            let again = verify_once(&rec, &trusted, &req, &mut store).expect_err("second use");
            assert_eq!(again.code.as_str(), "nonce_used", "{name}");
            // The pure check marks nothing.
            verify(&rec, &trusted, &req).unwrap_or_else(|err| panic!("{name}: {err}"));
            oks += 1;
        } else {
            let err = got.map(|_| ()).expect_err(name);
            assert_eq!(err.code.as_str(), s(e, "code"), "{name}: {}", err.detail);
            refusals += 1;
        }
    }
    assert!(oks >= 8 && refusals >= 20, "{oks} ok, {refusals} refused");
}

/// The openssl recipe in docs/signed-approvals.md, by hand: the payload
/// bytes from the record, the signature, and the PEM verify with nothing
/// but SHA-256 + ECDSA. Here aws-lc-rs stands in for openssl; the shell
/// test tests/approve/openssl-recipe.sh runs the real command.
#[test]
fn a_record_carries_the_exact_payload_bytes() {
    let rec = k2_approve::record::parse(&read("records/ok-release-phone-bio.json")).expect("record");
    assert_eq!(rec.payload, read("payloads/valid/release.bin"));
    assert!(rec.payload.starts_with(k2_approve::PREFIX));
}
