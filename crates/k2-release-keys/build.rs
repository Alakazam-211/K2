//! Picks the update keys this build's verifiers trust, from the brand
//! file's `[update]` table (src/brand_keys.rs says the rule).
//!
//! The brand folder is `K2_BRAND_DIR` when it is set (a relative path is
//! read from the workspace root), else `brand/k2`, as for crates/k2-brand.
//! For k2 and k5 it sets `cfg(k2_own_update_keys)` and `lib.rs` compiles
//! the ceremony's `UPDATER_KEYS` as it is. For any other brand it writes
//! `$OUT_DIR/brand_update_keys.rs` from the brand file. A brand file this
//! build can't use, or another brand carrying K2's keys, stops the build
//! with a line that names the file and why.

use std::path::{Path, PathBuf};
use std::{env, fs, process};

#[path = "src/brand_keys.rs"]
mod brand_keys;

fn stop(msg: String) -> ! {
    eprintln!("{msg}");
    process::exit(1);
}

fn main() {
    println!("cargo:rerun-if-env-changed=K2_BRAND_DIR");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/brand_keys.rs");
    println!("cargo::rustc-check-cfg=cfg(k2_own_update_keys)");
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("k2-release-keys lives at crates/k2-release-keys in the workspace");
    let dir = match env::var_os("K2_BRAND_DIR") {
        Some(d) if !d.is_empty() => root.join(d),
        _ => root.join("brand").join("k2"),
    };
    let file = dir.join("brand.toml");
    println!("cargo:rerun-if-changed={}", file.display());
    let shown = file
        .strip_prefix(root)
        .unwrap_or(&file)
        .display()
        .to_string();
    let text = fs::read_to_string(&file).unwrap_or_else(|e| {
        stop(format!("brand file {shown}: can't read it ({e}); K2_BRAND_DIR names the folder that holds brand.toml"))
    });
    let lib_rs = fs::read_to_string(manifest.join("src/lib.rs"))
        .unwrap_or_else(|e| stop(format!("k2-release-keys: can't read src/lib.rs ({e})")));
    let (own, retired) =
        brand_keys::own_keys(&lib_rs).unwrap_or_else(|e| stop(format!("k2-release-keys: {e}")));
    let (id, keys) =
        brand_keys::brand_keys(&text).unwrap_or_else(|e| stop(format!("brand file {shown}: {e}")));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("brand_update_keys.rs");
    match brand_keys::choose(&id, keys, &own, &retired) {
        Ok(brand_keys::Choice::Own) => {
            println!("cargo::rustc-cfg=k2_own_update_keys");
            if let Err(e) = fs::write(
                &out,
                "// Brand k2 or k5: lib.rs compiles the ceremony's UPDATER_KEYS.\n",
            ) {
                stop(format!("can't write {}: {e}", out.display()));
            }
        }
        Ok(brand_keys::Choice::Brand(keys)) => {
            if let Err(e) = fs::write(&out, brand_keys::render(&id, &shown, &keys)) {
                stop(format!("can't write {}: {e}", out.display()));
            }
        }
        Err(e) => stop(format!("brand file {shown}: {e}")),
    }
}
