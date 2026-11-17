//! Packs the shell CLI (`cli/k2`) into the binary: gzipped, with its
//! sha256 (hex) and the version on its `K2_CLI_VERSION="x.y.z"` line. A verb
//! the front doesn't handle runs this copy, so the binary and the script
//! it falls through to are always the same release.
//!
//! Writes `$OUT_DIR/k2.gz` and `$OUT_DIR/embedded.rs` (read by
//! `src/embedded.rs`). Fails the build when the script or its version line
//! is missing.

use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;

/// How far into the script the version line may be (prelude.sh puts it
/// on line 12; k2-client's script check reads 40 lines too).
const VERSION_LINES: usize = 40;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let script = manifest.join("../../cli/k2");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-changed=build.rs");

    let bytes = std::fs::read(&script)
        .unwrap_or_else(|e| panic!("the shell CLI is not at {}: {e} (build it with scripts/build-cli.sh)", script.display()));
    let version = cli_version(&bytes).unwrap_or_else(|| {
        panic!("{}: no K2_CLI_VERSION=\"x.y.z\" line in its first {VERSION_LINES} lines", script.display())
    });

    let mut sha = String::with_capacity(64);
    for b in Sha256::digest(&bytes) {
        write!(sha, "{b:02x}").expect("write to a String");
    }

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&bytes).expect("gzip the shell CLI");
    let gz = gz.finish().expect("gzip the shell CLI");
    let gz_path = out.join("k2.gz");
    std::fs::write(&gz_path, &gz).unwrap_or_else(|e| panic!("write {}: {e}", gz_path.display()));

    let gz_lit = gz_path.to_str().unwrap_or_else(|| panic!("OUT_DIR is not UTF-8: {}", gz_path.display()));
    let rs = format!(
        "/// `cli/k2`, gzipped.\n\
         pub static SCRIPT_GZ: &[u8] = include_bytes!({gz_lit:?});\n\
         /// The sha256 of `cli/k2` (not of the gzip), in lowercase hex.\n\
         pub const SCRIPT_SHA256: &str = {sha:?};\n\
         /// The version on `cli/k2`'s `K2_CLI_VERSION=` line.\n\
         pub const SCRIPT_VERSION: &str = {version:?};\n\
         /// The version as a marker other code finds in the binary's bytes\n\
         /// without running it (k2-client's `cli_install::check_binary`).\n\
         /// The NUL ends it, so `0.46.1` is never read inside `0.46.15`.\n\
         pub static VERSION_MARKER: &str = \"K5_CLI_VERSION={version}\\0\";\n"
    );
    let rs_path = out.join("embedded.rs");
    std::fs::write(&rs_path, rs).unwrap_or_else(|e| panic!("write {}: {e}", rs_path.display()));
}

/// The value of the script's `K2_CLI_VERSION="x.y.z"` line: digits, letters,
/// dots, `-` and `+` only, never empty.
fn cli_version(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(16 * 1024)]);
    let line = text.lines().take(VERSION_LINES).find_map(|l| l.strip_prefix("K2_CLI_VERSION=\""))?;
    let value = line.strip_suffix('"')?;
    let ok = !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b));
    ok.then(|| value.to_string())
}
