# shellcheck shell=bash
# One build step for every Rust binary K2.app ships on macOS.
#
# scripts/dev-build.sh, scripts/build-app.sh and scripts/release.sh all call
# k2_build_mac_binaries, so the three build the same binaries the same way:
#
#   1. `bun run tauri build`: the frontend, src-tauri (`k2`) and the bundle.
#   2. k2-daemon + k2-power-helper, k2-ptyd, k2-node, k2-menubar.
#   3. Checks + copies the sidecars into K2.app (nested K2 Daemon.app for
#      k2-daemon + k2-ptyd; Contents/MacOS for the rest).
#
# Signing, the AMFI launch check and notarizing stay in the callers.
#
# Why step 2 is not one `cargo build -p k2 -p k2-daemon -p …`: with
# resolver 2 one cargo call unifies dependency features across every package
# it builds. src-tauri and k2-daemon enable different features on 47 of the
# 443 crates under k2-core (the app has objc2 `exception`, ahash
# `runtime-rng`, hyper-util `client-proxy-system`; the daemon has the
# aws-lc-rs rustls provider, hyper `http2`, …). Building them together would
# change what the shipped app and daemon contain, so k2-core and the crates
# under it compile once per binary, by design. The same holds for the four
# sidecars: grouped, k2-node's rustls gains `ring` next to `aws-lc-rs` and
# the daemon's tungstenite gains rustls. Each cargo call below keeps the
# exact package set it had before, so every binary is byte-for-byte what the
# separate calls produced (check: `cargo tree -e features -p <pkg>`).
#
# What this step does save: `-p k2-daemon` used to build every k2-daemon bin,
# including k2-mail-helper, which K2.app does not ship (it is a Linux hostmail
# helper; .github/workflows/daemon-binaries.yml builds it there). Each bin is
# a full fat-LTO link of the daemon library, so `--bin` skips one of those.
#
# Needs (sourced by the caller): scripts/require-mail-oauth-build-env.sh
# (assert_daemon_oauth_not_placeholder). Run from the repo root.

# shellcheck source=scripts/macos-daemon-helper-app.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/macos-daemon-helper-app.sh"

# One build env for dev-build.sh, build-app.sh and release.sh, so a dev
# build leaves target/release warm for the release build (and the other way
# round). Everything else that reaches rustc / cc / cmake already comes from
# the same places: .cargo/config.toml [env] (CMAKE_POLICY_VERSION_MINIMUM,
# MACOSX_DEPLOYMENT_TARGET), the release profile in Cargo.toml, and .env.
# SDKROOT was the one that differed: some launchers exported it and the
# scripts did not, and an unset SDKROOT let a vendored cmake configure pick
# the Command Line Tools' newer SDK (`libSystem.tbd: unknown architecture`).
# An explicit SDKROOT from the caller still wins. CMAKE_POLICY_VERSION_MINIMUM
# is exported too (same 3.5 as .cargo/config.toml [env]) so a cmake run
# outside cargo, or a cargo that skips [env], gets the same floor; a value
# from the caller still wins.
k2_mac_build_env() {
    if [ -z "${SDKROOT:-}" ] && command -v xcrun >/dev/null 2>&1; then
        SDKROOT="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
        [ -n "$SDKROOT" ] && export SDKROOT
    fi
    if [ -n "${SDKROOT:-}" ]; then
        echo "  build env: SDKROOT=$SDKROOT"
    fi
    export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"
}

# Why `target/release/build/llama-cpp-sys-2-*` (and prost-build's) had to be
# deleted by hand: the `cmake` crate (0.1.58) runs `cmake` configure ONLY
# when `<OUT_DIR>/build/CMakeCache.txt` does not exist yet. A configure that
# fails (wrong SDK above, a CMake 4 policy error, Ctrl-C) still writes
# CMakeCache.txt but no Makefile / build.ninja, so every later run skips
# configure and dies with "No rule to make target Makefile". Remove exactly
# those half-configured build dirs (cache present, no generated build file);
# a good cache is never touched, so nothing rebuilds that didn't need to.
k2_clear_failed_cmake_dirs() {
    local profile_dir cache dir
    for profile_dir in target/release/build target/debug/build; do
        [ -d "$profile_dir" ] || continue
        for cache in "$profile_dir"/*/out/build/CMakeCache.txt; do
            [ -f "$cache" ] || continue
            dir="$(dirname "$cache")"
            if [ ! -f "$dir/Makefile" ] && [ ! -f "$dir/build.ninja" ]; then
                echo "  removing half-configured cmake dir (failed configure): $dir"
                rm -rf "$dir"
            fi
        done
    done
}

# k2_build_mac_binaries <app-path>   e.g. target/release/bundle/macos/K2.app
# Returns non-zero (after a FATAL line) on any failure.
k2_build_mac_binaries() {
    local app="$1"
    local out="target/release"

    k2_mac_build_env
    k2_clear_failed_cmake_dirs

    echo ""; echo "  Build 1/2: tauri build (frontend + k2 + bundle)..."
    bun run tauri build || { echo "FATAL: tauri build failed" >&2; return 1; }
    [ -d "$app" ] || { echo "FATAL: $app missing after tauri build" >&2; return 1; }

    echo ""; echo "  Build 2/2: k2-daemon, k2-power-helper, k2-ptyd, k2-node, k2-menubar..."
    # One cargo call per package (see the header): the package set of each
    # call decides the features, so these are never merged.
    cargo build --release -p k2-daemon --bin k2-daemon --bin k2-power-helper \
        || { echo "FATAL: k2-daemon build failed" >&2; return 1; }
    # k2-ptyd rides beside the daemon in K2 Daemon.app (prd-k2-ptyd-v1 §7.1).
    cargo build --release -p k2-ptyd --bin k2-ptyd \
        || { echo "FATAL: k2-ptyd build failed" >&2; return 1; }
    # Compute nodes: separate crate, never links k2-core.
    cargo build --release -p k2-node --bin k2-node \
        || { echo "FATAL: k2-node build failed" >&2; return 1; }
    cargo build --release -p k2-menubar --bin k2-menubar \
        || { echo "FATAL: k2-menubar build failed" >&2; return 1; }

    local bin
    for bin in k2-daemon k2-power-helper k2-ptyd k2-node k2-menubar; do
        [ -x "$out/$bin" ] || { echo "FATAL: $bin missing at $out/$bin after cargo build" >&2; return 1; }
    done
    assert_daemon_oauth_not_placeholder "$out/k2-daemon" || return 1

    echo ""; echo "  Bundling sidecars into $app..."
    # k2-daemon lives in a nested background-only helper app so Activity
    # Monitor shows the K2 icon + "K2 Daemon"; k2-ptyd sits beside it.
    k2_daemon_helper_assemble "$app" "$out/k2-daemon" "$out/k2-ptyd" || return 1
    k2_daemon_helper_verify "$app" || return 1
    # Heartbeat S2 (D11): the wake helper. The daemon copies it to
    # /Library/PrivilegedHelperTools only after the user approves the one
    # admin dialog.
    cp "$out/k2-power-helper" "$app/Contents/MacOS/k2-power-helper" || return 1
    echo "  k2-power-helper copied into the bundle."
    # Settings → Compute → "Make this computer a compute node" installs it
    # (k2-node setup copies it to /Library/Application Support/K2/node).
    cp "$out/k2-node" "$app/Contents/MacOS/k2-node" || return 1
    echo "  k2-node copied into the bundle."
    # Menu-bar helper. Staged out of the bundle to ~/.k2/bin on first GUI
    # open (same shape as frpc). Not executed from Contents/MacOS.
    cp "$out/k2-menubar" "$app/Contents/MacOS/k2-menubar" || return 1
    echo "  k2-menubar copied into the bundle."
}
