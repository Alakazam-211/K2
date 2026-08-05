//! Build script for `k2-daemon`.
//!
//! Its ONLY job is to link libkrun for the `k2-vmm-worker` binary — and only
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
