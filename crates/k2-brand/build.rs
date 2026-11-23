//! Writes `$OUT_DIR/brand.rs` from one brand file, for `src/lib.rs` to
//! include.
//!
//! The brand folder is `K2_BRAND_DIR` when it is set (a relative path is
//! read from the workspace root), else `brand/k2` at the workspace root.
//! A field the brand file leaves out takes the default brand's value
//! (`brand/k2`), so this reads that file too. A brand file this build
//! can't use stops the build with a line that names the file and the
//! field.

use std::path::{Path, PathBuf};
use std::{env, fs, process};

#[path = "src/brand_file.rs"]
mod brand_file;

fn main() {
    println!("cargo:rerun-if-env-changed=K2_BRAND_DIR");
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("k2-brand lives at crates/k2-brand in the workspace");
    let default = root.join("brand").join("k2").join("brand.toml");
    let dir = match env::var_os("K2_BRAND_DIR") {
        Some(d) if !d.is_empty() => root.join(d),
        _ => root.join("brand").join("k2"),
    };
    let file = dir.join("brand.toml");
    println!("cargo:rerun-if-changed={}", file.display());
    println!("cargo:rerun-if-changed={}", default.display());
    let brand = match brand_file::load(&file, &default) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("brand.rs");
    if let Err(e) = fs::write(&out, brand_file::render(&brand)) {
        eprintln!("can't write {}: {e}", out.display());
        process::exit(1);
    }
}
