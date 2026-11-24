//! K2 Web's names block, as the control plane's shared names vectors say
//! it: a byte-identical copy of the canonical file, run through this
//! daemon's parser. A change to the copy comes from the canonical file only.

use super::*;
use sha2::{Digest, Sha256};

/// The copy (testdata/names-vectors.json).
const VECTORS: &[u8] = include_bytes!("testdata/names-vectors.json");

/// SHA-256 of the copy, pinned so an edit here shows up as a failure.
const VECTORS_SHA256: &str = "18be50c2406070f02cb05e28bcfd16e4859f809baf2c73b20a3c3cfa57962da5";

/// Where the daemon is stricter than the control plane: these cases'
/// names are not hostnames, so the daemon never takes them (the answer is
/// Malformed and the box keeps its last good names).
const DAEMON_IS_STRICTER: &[&str] = &["nested-escaping", "rust-trim-not-js-trim"];

#[test]
fn the_vectors_are_the_pinned_copy() {
    assert_eq!(format!("{:x}", Sha256::digest(VECTORS)), VECTORS_SHA256);
}

fn version_of(expect: &Value, name: &str) -> u64 {
    expect["namesVersion"]
        .as_u64()
        .unwrap_or_else(|| panic!("{name}: namesVersion is not an integer"))
}

/// The entries K2 Web means: the features that are true, sorted.
fn entries_of(expect: &Value, name: &str) -> Vec<NameEntry> {
    let list = expect["names"].as_array().unwrap_or_else(|| panic!("{name}: no names list"));
    list.iter()
        .map(|e| {
            let text = |k: &str| e[k].as_str().unwrap_or_else(|| panic!("{name}: no {k}")).to_string();
            let mut features: Vec<String> = e["features"]
                .as_object()
                .unwrap_or_else(|| panic!("{name}: features is not an object"))
                .iter()
                .filter(|(_, on)| on.as_bool().unwrap_or_else(|| panic!("{name}: a feature that is not a bool")))
                .map(|(w, _)| w.clone())
                .collect();
            features.sort();
            NameEntry {
                zone: text("zone"),
                fqdn: text("fqdn"),
                features,
                turn: e.get("turn").and_then(Value::as_str).map(str::to_string),
            }
        })
        .collect()
}

#[test]
fn every_vector_parses_as_k2_web_means_it() {
    let doc: Value = serde_json::from_slice(VECTORS).expect("the vectors are JSON");
    let cases = doc["cases"].as_array().expect("a cases list");
    assert_eq!(cases.len(), 38, "the 38 shared cases");
    let (mut active, mut revoked, mut absent) = (0, 0, 0);
    let mut stricter = Vec::new();
    for c in cases {
        let name = c["name"].as_str().expect("a case name");
        let expect = &c["expect"];
        let wire: WireNames = serde_json::from_value(expect.clone())
            .unwrap_or_else(|e| panic!("{name}: the names fields: {e}"));
        let got = parse(&wire);
        if DAEMON_IS_STRICTER.contains(&name) {
            assert!(matches!(got, NamesAnswer::Malformed(_)), "{name}: the daemon refuses it, got {got:?}");
            stricter.push(name);
            continue;
        }
        match expect.get("namesStatus").and_then(Value::as_str) {
            None => {
                assert_eq!(got, NamesAnswer::Absent, "{name}");
                absent += 1;
            }
            Some("none") => {
                let want = NamesAnswer::Usable {
                    names: BoxNames::Revoked { version: version_of(expect, name) },
                    dropped: Vec::new(),
                };
                assert_eq!(got, want, "{name}");
                revoked += 1;
            }
            Some("ok") => {
                let NamesAnswer::Usable { names: BoxNames::Active(n), dropped } = got else {
                    panic!("{name}: expected an active list, got {got:?}");
                };
                assert!(dropped.is_empty(), "{name}: dropped {dropped:?}");
                assert_eq!(n.version, version_of(expect, name), "{name}");
                assert_eq!(Some(n.primary_zone.as_str()), expect["primaryZone"].as_str(), "{name}");
                assert_eq!(n.entries, entries_of(expect, name), "{name}");
                active += 1;
            }
            Some(other) => panic!("{name}: unknown namesStatus {other:?}"),
        }
    }
    assert_eq!(stricter, DAEMON_IS_STRICTER, "every stricter case is in the vectors, in order");
    assert_eq!((active, revoked, absent), (24, 7, 5), "the cases by outcome");
}
