//! The helper reports the product version (the daemon's), so a helper and
//! a daemon from one release always agree and release.sh bumps one file.
use std::fs;

fn main() {
    let manifest = "../k2-daemon/Cargo.toml";
    println!("cargo:rerun-if-changed={manifest}");
    let text = fs::read_to_string(manifest).expect("read crates/k2-daemon/Cargo.toml");
    let version = text
        .lines()
        .find_map(|l| {
            l.strip_prefix("version = \"")
                .and_then(|r| r.strip_suffix('"'))
        })
        .expect("k2-daemon Cargo.toml has a version line");
    let ok = {
        let parts: Vec<&str> = version.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    };
    assert!(ok, "k2-daemon version must be plain x.y.z, got {version}");
    println!("cargo:rustc-env=K2_PRODUCT_VERSION={version}");
}
