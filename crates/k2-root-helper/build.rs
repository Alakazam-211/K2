//! The helper reports the product version (the daemon's), so a helper and
//! a daemon from one release always agree and release.sh bumps one file.
//!
//! It also carries the K2 CLI (`cli/k2`, `helper cli-refresh`): the build
//! refuses a CLI whose `K2_CLI_VERSION` is not the product version (a
//! helper must never write an off-version CLI as root), and exports the
//! CLI's sha256 (`helper version` → `cliSha256`, which the daemon compares
//! with its own embedded copy before it asks for a refresh). The CLI goes
//! in deflate-compressed (`$OUT_DIR/k2-cli.deflate`, see core.rs).
use std::fs;

use sha2::Digest;

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

    // The repo's script file, not the command's name: a brand build of the bash CLI is a later step (C7).
    let cli_path = "../../cli/k2";
    println!("cargo:rerun-if-changed={cli_path}");
    let cli = fs::read(cli_path).expect("read cli/k2");
    let cli_version = String::from_utf8_lossy(&cli[..cli.len().min(16 * 1024)])
        .lines()
        .take(30)
        .find_map(|l| l.strip_prefix("K2_CLI_VERSION=").map(|v| v.trim().trim_matches('"').to_string()))
        .expect("cli/k2 has a K2_CLI_VERSION line in its first 30");
    assert_eq!(
        cli_version, version,
        "cli/k2 K2_CLI_VERSION ({cli_version}) must equal the product version ({version}); \
         release.sh bumps both"
    );
    let sha = sha2::Sha256::digest(&cli);
    let hex: String = sha.iter().map(|b| format!("{b:02x}")).collect();
    println!("cargo:rustc-env=K2_CLI_SHA256={hex}");

    // The helper carries the CLI deflate-compressed (0.46.18): raw, the
    // ~6 MB script was most of 0.46.17's 8.8 MB helper, over the 8 MiB
    // self-update cap every helper up to 0.46.17 enforces. Deterministic
    // (miniz_oxide, fixed level), so every builder makes the same bytes.
    let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut z, &cli).expect("deflate cli/k2");
    let packed = z.finish().expect("deflate cli/k2");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("k2-cli.deflate"), &packed).expect("write k2-cli.deflate");
}
