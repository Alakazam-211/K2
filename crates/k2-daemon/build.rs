//! Build script for `k2-daemon`.
//!
//! Two jobs. (1) k2-ptyd P84: generate `ptyd_embed.rs` — the embedded
//! k2-ptyd binary when `K2_PTYD_EMBED` is set, `None` otherwise (see
//! `embed_ptyd`). (2) Link libkrun for the `k2-vmm-worker` binary — and only
//! on a Linux build with the `sandbox-microvm` feature on. On every other
//! build (the default daemon, any macOS build, feature-off) it emits nothing,
//! so the normal build is completely unaffected and no libkrun is required.
//!
//! libkrun installs into `/usr/local/lib64` on the on-box build host
//! (`k2-sandbox-01`, per `.k2/notes/p2b-onbox-bootstrap.md` §4). We add it to
//! the link search path, link `dylib=krun`, and bake an rpath so the worker
//! finds `libkrun.so` at runtime without `LD_LIBRARY_PATH`.

fn main() {
    // Re-run only when the gating inputs change.
    println!("cargo:rerun-if-changed=build.rs");
    // Mail OAuth defaults are option_env! in mail/oauth/mod.rs — without
    // these, cargo will not rebuild when CI/.env injects the real clients
    // and a prior compile with REPLACE_ME can stick in the incremental cache.
    println!("cargo:rerun-if-env-changed=K2_GMAIL_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=K2_GMAIL_CLIENT_SECRET");
    println!("cargo:rerun-if-env-changed=K2_MICROSOFT_CLIENT_ID");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let feature_on = std::env::var("CARGO_FEATURE_SANDBOX_MICROVM").is_ok();

    embed_ptyd();
    embed_export();

    // Link libkrun ONLY for linux + sandbox-microvm. Anywhere else: do nothing.
    if target_os != "linux" || !feature_on {
        return;
    }

    // libkrun lib location on the on-box build host. Kept in sync with the
    // LIBKRUN_LIBDIR constant in src/bin/k2-vmm-worker.rs.
    const LIBKRUN_LIBDIR: &str = "/usr/local/lib64";

    // Search path so the linker finds libkrun.so. The actual `-lkrun` is
    // emitted by the `#[link(name = "krun")]` attribute on the worker's extern
    // block (which positions it correctly past the default `--as-needed`); a
    // build-script `rustc-link-lib` lands too early and gets silently dropped.
    println!("cargo:rustc-link-search=native={LIBKRUN_LIBDIR}");
    // rpath so the worker resolves libkrun.so at runtime without env wiring.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{LIBKRUN_LIBDIR}");
}

/// k2-ptyd P84: embed the release-built `k2-ptyd` (path in `K2_PTYD_EMBED`)
/// and its sha256 so the Linux self-update and headless installers deliver
/// ptyd with no new asset. Without the env var the daemon embeds nothing
/// (dev builds use `K2_PTYD_BIN` or the binary beside the daemon).
/// `K2_REQUIRE_PTYD_EMBED=1` (release builds) makes a missing or unreadable
/// binary a build failure.
fn embed_ptyd() {
    use sha2::{Digest, Sha256};
    println!("cargo:rerun-if-env-changed=K2_PTYD_EMBED");
    println!("cargo:rerun-if-env-changed=K2_REQUIRE_PTYD_EMBED");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("ptyd_embed.rs");
    let required = std::env::var("K2_REQUIRE_PTYD_EMBED").map(|v| v == "1").unwrap_or(false);
    let code = match std::env::var("K2_PTYD_EMBED").ok().filter(|p| !p.is_empty()) {
        Some(path) => {
            println!("cargo:rerun-if-changed={path}");
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("K2_PTYD_EMBED={path}: {e}"));
            assert!(bytes.len() > 4096, "K2_PTYD_EMBED={path} is too small to be k2-ptyd");
            let sha = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
            format!("pub static PTYD_EMBED: Option<(&[u8], &str)> = Some((include_bytes!({path:?}), {sha:?}));\n")
        }
        None => {
            assert!(!required, "K2_REQUIRE_PTYD_EMBED=1 but K2_PTYD_EMBED is not set (P84)");
            "pub static PTYD_EMBED: Option<(&[u8], &str)> = None;\n".to_string()
        }
    };
    std::fs::write(&out, code).expect("write ptyd_embed.rs");
}

/// Exports (vs-live §3.1): embed the release-built `k2-export` worker (path
/// in `K2_EXPORT_EMBED`) and its sha256, exactly as `embed_ptyd` does, so
/// the standalone daemon downloads (Linux, `k2-daemon-macos-aarch64`) carry
/// it with no new asset. `K2_REQUIRE_EXPORT_EMBED=1` (release builds) makes
/// a missing or unreadable binary a build failure.
fn embed_export() {
    use sha2::{Digest, Sha256};
    println!("cargo:rerun-if-env-changed=K2_EXPORT_EMBED");
    println!("cargo:rerun-if-env-changed=K2_REQUIRE_EXPORT_EMBED");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("export_embed.rs");
    let required = std::env::var("K2_REQUIRE_EXPORT_EMBED").map(|v| v == "1").unwrap_or(false);
    let code = match std::env::var("K2_EXPORT_EMBED").ok().filter(|p| !p.is_empty()) {
        Some(path) => {
            println!("cargo:rerun-if-changed={path}");
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("K2_EXPORT_EMBED={path}: {e}"));
            assert!(bytes.len() > 4096, "K2_EXPORT_EMBED={path} is too small to be k2-export");
            assert!(
                !bytes.windows(30).any(|w| w == b"K2_EXPORT_TEST_HOSTILE_ENABLED"),
                "K2_EXPORT_EMBED={path} is a test-hostile build; never embed it"
            );
            let sha = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
            format!("pub static EXPORT_EMBED: Option<(&[u8], &str)> = Some((include_bytes!({path:?}), {sha:?}));\n")
        }
        None => {
            assert!(!required, "K2_REQUIRE_EXPORT_EMBED=1 but K2_EXPORT_EMBED is not set");
            "pub static EXPORT_EMBED: Option<(&[u8], &str)> = None;\n".to_string()
        }
    };
    std::fs::write(&out, code).expect("write export_embed.rs");
}
