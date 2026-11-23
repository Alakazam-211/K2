//! The build statement golden vectors (docs/provenance-v1.md):
//! tests/release/fixtures/provenance-v1.json, written by
//! make-provenance-v1.py from the spec (not from this crate). For every
//! vector, the statement this crate builds from the vector's plan, signed
//! receipt and node facts must be exactly the vector's payload, byte for
//! byte; the vector's envelope (signed by /usr/bin/openssl over the DSSE
//! PAE) must verify here and agree with its receipt; and an envelope this
//! crate signs must carry the same payload and verify. A failure means the
//! Rust drifted from the frozen format: change the format on purpose (a new
//! `buildType`, the spec, the generator), never the vector alone.

use std::path::PathBuf;

use k2_node_proto::ci::{Isolation, Wall};
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{Plan, SignedReceipt};
use k2_node_proto::provenance::{self, Dsse, NodeFacts, Owner, Statement, PAYLOAD_TYPE};
use k2_node_proto::share::Network;
use serde_json::Value;

fn fixture() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/release/fixtures/provenance-v1.json");
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).expect("provenance-v1.json is JSON")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn hex_bytes(h: &str) -> Vec<u8> {
    assert!(h.len().is_multiple_of(2));
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

fn facts(v: &Value) -> NodeFacts {
    let owner = match (v["owner"].get("roster_key"), v["owner"].get("server")) {
        (Some(fp), None) => Owner::RosterKey(s(fp).into()),
        (None, Some(fp)) => Owner::Server(s(fp).into()),
        _ => panic!("owner: {}", v["owner"]),
    };
    let opt = |k: &str| v[k].as_str().map(str::to_string);
    NodeFacts {
        owner,
        network: serde_json::from_value::<Network>(v["network"].clone()).unwrap(),
        isolation: serde_json::from_value::<Isolation>(v["isolation"].clone()).unwrap(),
        wall: serde_json::from_value::<Wall>(v["wall"].clone()).unwrap(),
        tenant_slot: v["tenant_slot"].as_u64().unwrap() as u32,
        lane: opt("lane"),
        k2node_version: s(&v["k2node_version"]).into(),
        k2node_commit: opt("k2node_commit"),
        protocol: v["protocol"].as_u64().unwrap() as u32,
        key_epoch: v["key_epoch"].as_u64().unwrap() as u32,
        statement_digest: opt("statement_digest"),
        node_name: opt("node_name"),
    }
}

#[test]
fn the_key_is_what_it_says() {
    let fx = fixture();
    let k = &fx["keys"]["node"];
    let key = SigningKey::from_pkcs8_pem(s(&k["private_key_pem"])).unwrap();
    assert_eq!(key.fingerprint(), s(&k["fp"]));
    assert_eq!(crypto::b64(key.spki_der()), s(&k["spki_b64"]));
    assert_eq!(crypto::fingerprint_of_spki_pem(s(&k["public_key_pem"])).unwrap(), s(&k["fp"]));
}

#[test]
fn every_vector_is_built_byte_for_byte_and_verifies() {
    let fx = fixture();
    let key = SigningKey::from_pkcs8_pem(s(&fx["keys"]["node"]["private_key_pem"])).unwrap();
    let vectors = fx["vectors"].as_array().expect("vectors");
    assert_eq!(vectors.len(), 2);
    for v in vectors {
        let name = s(&v["name"]);
        // The plan: the vector's JSON is exactly this crate's form.
        let plan: Plan = serde_json::from_value(v["plan"].clone()).unwrap();
        assert_eq!(k2_node_proto::canonical::to_string(&plan).unwrap(), k2_node_proto::canonical::to_string(&v["plan"]).unwrap(), "{name}: plan");
        assert_eq!(plan.digest().unwrap(), s(&v["plan_digest"]), "{name}");
        // The receipt: signed by the node key (openssl).
        let sr = SignedReceipt { body: s(&v["receipt"]["body"]).into(), sig: s(&v["receipt"]["sig"]).into() };
        let receipt = sr.verify(key.spki_der()).unwrap_or_else(|| panic!("{name}: the receipt verifies"));
        receipt.check_version().unwrap();
        // Built here: exactly the spec's statement.
        let st = Statement::for_receipt(&sr, &plan, &facts(&v["facts"])).unwrap().unwrap_or_else(|| panic!("{name}: a statement"));
        let payload = provenance::payload(&st).unwrap();
        assert_eq!(String::from_utf8(payload.clone()).unwrap(), s(&v["payload"]), "{name}: payload");
        assert_eq!(serde_json::to_value(&st).unwrap(), v["statement"], "{name}: statement");
        assert_eq!(provenance::pae(PAYLOAD_TYPE, &payload), hex_bytes(s(&v["pae_hex"])), "{name}: PAE");
        // The vector's envelope (openssl) verifies and agrees.
        let env: Dsse = serde_json::from_value(v["envelope"].clone()).unwrap();
        assert_eq!(env.open_for_receipt(key.spki_der(), &sr).unwrap(), st, "{name}");
        assert_eq!(env.line().unwrap(), s(&v["line"]), "{name}: the .intoto.jsonl line");
        // One signed here: same payload, verifies with the same key.
        let mine = key.sign_statement(&st).unwrap();
        assert_eq!(mine.payload, env.payload, "{name}");
        assert_eq!(mine.signatures[0].keyid, env.signatures[0].keyid, "{name}");
        assert_eq!(mine.open_for_receipt(key.spki_der(), &sr).unwrap(), st, "{name}");
        // A one-byte change to the openssl envelope's payload: refused.
        let mut bad = env.clone();
        let mut p = crypto::unb64(&bad.payload).unwrap();
        let i = p.iter().position(|b| *b == b'1').unwrap();
        p[i] = b'2';
        bad.payload = crypto::b64(&p);
        assert!(bad.open(key.spki_der()).is_err(), "{name}");
    }
}

/// The receipt-v2 vector's facts disagree with its run facts on purpose
/// (epoch, statement, isolation, lane): the receipt's own facts are in the
/// statement, so a node can't say something its receipt doesn't.
#[test]
fn a_v2_receipts_run_facts_win() {
    let fx = fixture();
    let v = fx["vectors"].as_array().unwrap().iter().find(|v| v["name"] == "b_bundle_v2_receipt").unwrap();
    let ip = &v["statement"]["predicate"]["buildDefinition"]["internalParameters"];
    assert_eq!((s(&ip["isolation"]), ip.get("lane")), ("job_uid", None));
    assert_ne!(v["facts"]["isolation"], ip["isolation"]);
    let key = &v["statement"]["predicate"]["runDetails"]["builder"]["builderDependencies"][1];
    assert_eq!(key["annotations"]["keyEpoch"], 0);
    assert_eq!(v["facts"]["key_epoch"], 9);
}
