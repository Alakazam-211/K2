//! k2-node reports the product version (the daemon's), so a node and a
//! controller from one release agree, self-update compares real release
//! versions, and release.sh bumps one file (prd-k2-node-admin-v1 §8.2).
//!
//! The build commit (vs-live V8): `K2_NODE_BUILD_COMMIT` (40 lowercase hex,
//! set by `scripts/ci/build-linux-bins.sh` and the release workflow) goes
//! into a second marker, `K2_NODE_COMMIT c=<sha>;` (`c=none` without it),
//! which the node's self-statement reports as `k2node_commit`. A separate
//! marker, not a field of `K2_NODE_BUILD`: older nodes and signers parse
//! that one strictly and would refuse an update that grew a field.
//!
//! TEST ONLY (`--features test-keys`): `K2_NODE_TEST_VERSION` overrides the
//! version and `K2_NODE_TEST_BROKEN=1` builds a node whose `run` exits at
//! once, so the Linux end-to-end can make "newer good" and "newer bad"
//! signed builds. Both are ignored without the feature.
use std::fs;

fn main() {
    let manifest = "../k2-daemon/Cargo.toml";
    println!("cargo:rerun-if-changed={manifest}");
    println!("cargo:rerun-if-env-changed=K2_NODE_TEST_VERSION");
    println!("cargo:rerun-if-env-changed=K2_NODE_TEST_BROKEN");
    println!("cargo:rerun-if-env-changed=K2_NODE_BUILD_COMMIT");
    let commit = match std::env::var("K2_NODE_BUILD_COMMIT") {
        Ok(c) if c.is_empty() => "none".to_string(),
        Ok(c) => {
            assert!(
                c.len() == 40 && c.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "K2_NODE_BUILD_COMMIT must be a 40-hex lowercase commit, got {c:?}"
            );
            c
        }
        Err(_) => "none".to_string(),
    };
    println!("cargo:rustc-env=K2_NODE_COMMIT={commit}");
    println!("cargo::rustc-check-cfg=cfg(k2_node_test_broken)");
    let text = fs::read_to_string(manifest).expect("read crates/k2-daemon/Cargo.toml");
    let mut version = text
        .lines()
        .find_map(|l| l.strip_prefix("version = \"").and_then(|r| r.strip_suffix('"')))
        .expect("k2-daemon Cargo.toml has a version line")
        .to_string();
    let test_keys = std::env::var_os("CARGO_FEATURE_TEST_KEYS").is_some();
    if test_keys {
        if let Ok(v) = std::env::var("K2_NODE_TEST_VERSION") {
            version = v;
        }
        if std::env::var("K2_NODE_TEST_BROKEN").as_deref() == Ok("1") {
            println!("cargo:rustc-cfg=k2_node_test_broken");
        }
    }
    let ok = {
        let parts: Vec<&str> = version.split('.').collect();
        parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    };
    assert!(ok, "product version must be plain x.y.z, got {version}");
    println!("cargo:rustc-env=K2_PRODUCT_VERSION={version}");
}
