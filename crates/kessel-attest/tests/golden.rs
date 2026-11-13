//! The frozen format's golden vectors (tests/release/fixtures/provenance-v1.json,
//! written by make-provenance-v1.py with openssl, not by any Rust crate):
//! this crate, which shares no code with the writer, reads every vector's
//! envelope, verifies it with the vector's node key, and gets exactly the
//! vector's payload, statement, PAE and `.intoto.jsonl` line back. A failure
//! here means a reader and the writer disagree about the format.

mod common;

use common::{fixture, s};
use kessel_attest::dsse::{pae, Envelope, NodeKey, PAYLOAD_TYPE};
use kessel_attest::statement::Owner;
use kessel_attest::util::{b64, unb64};

fn hex_bytes(h: &str) -> Vec<u8> {
    assert!(h.len() % 2 == 0);
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

fn node_key() -> NodeKey {
    let fx = fixture();
    let k = &fx["keys"]["node"];
    let key = NodeKey::from_pem(s(&k["public_key_pem"])).unwrap();
    assert_eq!(key.fingerprint(), s(&k["fp"]));
    assert_eq!(b64(key.spki_der()), s(&k["spki_b64"]));
    key
}

#[test]
fn every_golden_vector_verifies_and_reads_back_byte_for_byte() {
    let fx = fixture();
    let key = node_key();
    let vectors = fx["vectors"].as_array().expect("vectors");
    assert_eq!(vectors.len(), 2, "the fixture's vector count changed: read the new ones here too");
    for v in vectors {
        let name = s(&v["name"]);
        let env: Envelope = serde_json::from_value(v["envelope"].clone()).unwrap();
        let st = env.open(&key).unwrap_or_else(|e| panic!("{name}: {e}"));
        let payload = unb64(&env.payload).unwrap();
        assert_eq!(String::from_utf8(payload.clone()).unwrap(), s(&v["payload"]), "{name}: payload");
        assert_eq!(serde_json::to_value(&st).unwrap(), v["statement"], "{name}: statement");
        assert_eq!(pae(PAYLOAD_TYPE, &payload), hex_bytes(s(&v["pae_hex"])), "{name}: PAE");
        assert_eq!(env.line().unwrap(), s(&v["line"]), "{name}: the .intoto.jsonl line");
        assert_eq!(Envelope::from_json(s(&v["line"])).unwrap(), env, "{name}: the line parses back");
        assert_eq!(st.node_key_fp(), Some(s(&fx["keys"]["node"]["fp"])), "{name}");
        // One byte of the payload changed: refused.
        let mut bad = env.clone();
        let mut p = payload.clone();
        let i = p.iter().position(|b| *b == b'1').unwrap();
        p[i] = b'2';
        bad.payload = b64(&p);
        assert!(bad.open(&key).unwrap_err().contains("does not verify"), "{name}");
    }
}

#[test]
fn the_vectors_name_their_owners_in_both_forms() {
    let fx = fixture();
    let key = node_key();
    let open = |i: usize| serde_json::from_value::<Envelope>(fx["vectors"][i]["envelope"].clone()).unwrap().open(&key).unwrap();
    let a = open(0);
    let b = open(1);
    assert_eq!(a.builder_owner(), Some(Owner::RosterKey(s(&fx["vectors"][0]["facts"]["owner"]["roster_key"]).into())));
    assert_eq!(b.builder_owner(), Some(Owner::Server(s(&fx["vectors"][1]["facts"]["owner"]["server"]).into())));
    assert_eq!(a.subject.len(), 2);
    assert_eq!(a.commit().map(str::len), Some(40));
    assert_eq!(a.started(), kessel_attest::util::parse_rfc3339(s(&fx["vectors"][0]["statement"]["predicate"]["runDetails"]["metadata"]["startedOn"])).unwrap());
}

#[test]
fn another_key_or_payload_type_is_refused() {
    let fx = fixture();
    let env: Envelope = serde_json::from_value(fx["vectors"][0]["envelope"].clone()).unwrap();
    let other = common::Node::other();
    let other_key = NodeKey::from_spki_der(&other.spki_der).unwrap();
    assert!(env.open(&other_key).unwrap_err().contains("keyid"));
    let mut typed = env.clone();
    typed.payload_type = "application/json".into();
    assert!(typed.open(&node_key()).unwrap_err().contains("payloadType"));
}

#[test]
fn the_shape_rules_refuse_what_stock_tools_would() {
    let fx = fixture();
    let node = common::Node::golden();
    let key = node_key();
    let base = fx["vectors"][0]["statement"].clone();
    let cases: Vec<common::EditCase> = vec![
        ("no subject", Box::new(|v| v["subject"] = serde_json::json!([])), "no subject"),
        (
            "toolchain without uri",
            Box::new(|v| {
                v["predicate"]["buildDefinition"]["resolvedDependencies"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name": "toolchain:cc", "annotations": {"version": "14"}}))
            }),
            "neither uri nor digest",
        ),
        ("unknown field", Box::new(|v| v["predicate"]["extra"] = serde_json::json!(1)), "unknown field"),
        ("another build type", Box::new(|v| v["predicate"]["buildDefinition"]["buildType"] = serde_json::json!("https://example.test/other")), "buildType"),
    ];
    for (tag, edit, want) in cases {
        let mut st = base.clone();
        edit(&mut st);
        let payload = kessel_attest::canonical::to_vec(&st).unwrap();
        let env: Envelope = serde_json::from_value(node.envelope_bytes(&payload)).unwrap();
        let err = env.open(&key).unwrap_err();
        assert!(err.contains(want), "{tag}: {err}");
    }
    // A float: no canonical form exists, so the payload is written by hand.
    let text = String::from_utf8(kessel_attest::canonical::to_vec(&base).unwrap()).unwrap().replacen("\"bytes\":812345", "\"bytes\":812345.5", 1);
    assert!(text.contains("812345.5"));
    let env: Envelope = serde_json::from_value(node.envelope_bytes(text.as_bytes())).unwrap();
    let err = env.open(&key).unwrap_err();
    assert!(err.contains("float"), "{err}");
}
