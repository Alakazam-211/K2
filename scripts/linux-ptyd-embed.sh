#!/usr/bin/env bash
# linux-ptyd-embed.sh — Linux release daemons carry k2-ptyd inside them
# (PRD prd-k2-ptyd-v1 P84). The headless installer (install-daemon.sh) and
# the daemon self-updater only ever fetch `k2-daemon`, so ptyd rides in the
# daemon binary: crates/k2-daemon/build.rs embeds the file named by
# K2_PTYD_EMBED (and its sha256); K2_REQUIRE_PTYD_EMBED=1 makes a missing
# file a build failure. At run time the daemon writes the embedded bytes to
# ~/.k2/bin/ptyd/<ver>-<sha12>/k2-ptyd (hash-checked) and launches that.
#
# Usage (any Linux daemon build: CI release assets, Arch PKGBUILD, .deb):
#   source scripts/linux-ptyd-embed.sh
#   k2_ptyd_build_for_embed [<rust target triple>]   # builds + exports
#   cargo build --release [--target <triple>] -p k2-daemon --bin k2-daemon
#   assert_daemon_embeds_ptyd path/to/k2-daemon
#
# The export worker rides the same way (prd-k2-core-exports-v1, vs-live
# §3.1): k2_export_build_for_embed exports K2_EXPORT_EMBED (+
# K2_REQUIRE_EXPORT_EMBED=1) and assert_daemon_embeds_export checks the
# daemon carries it; the daemon places it at
# ~/.k2/bin/export/<ver>-<sha12>/k2-export.
#   k2_export_build_for_embed [<rust target triple>]
#   assert_daemon_embeds_export path/to/k2-daemon
#
# Sourced: sets no shell options (the PKGBUILD runs with nounset off).

# Build k2-ptyd (release, optional target) and export K2_PTYD_EMBED (an
# absolute path: build.rs hands it to include_bytes! from OUT_DIR) plus
# K2_REQUIRE_PTYD_EMBED=1 for the daemon build that follows.
k2_ptyd_build_for_embed() {
    local target="${1:-}" dir
    if [ -n "$target" ]; then
        cargo build --release --target "$target" -p k2-ptyd --bin k2-ptyd \
            || { echo "FATAL: k2-ptyd build failed ($target)" >&2; return 1; }
        dir="${CARGO_TARGET_DIR:-target}/$target/release"
    else
        cargo build --release -p k2-ptyd --bin k2-ptyd \
            || { echo "FATAL: k2-ptyd build failed" >&2; return 1; }
        dir="${CARGO_TARGET_DIR:-target}/release"
    fi
    [ -x "$dir/k2-ptyd" ] || { echo "FATAL: k2-ptyd missing at $dir/k2-ptyd after cargo build" >&2; return 1; }
    K2_PTYD_EMBED="$(cd "$dir" && pwd)/k2-ptyd"
    K2_REQUIRE_PTYD_EMBED=1
    export K2_PTYD_EMBED K2_REQUIRE_PTYD_EMBED
    echo "  k2-ptyd built for embedding: $K2_PTYD_EMBED"
}

# Build k2-export (release, optional target; never --features
# test-hostile) and export K2_EXPORT_EMBED + K2_REQUIRE_EXPORT_EMBED=1.
k2_export_build_for_embed() {
    local target="${1:-}" dir
    if [ -n "$target" ]; then
        cargo build --release --target "$target" -p k2-export --bin k2-export \
            || { echo "FATAL: k2-export build failed ($target)" >&2; return 1; }
        dir="${CARGO_TARGET_DIR:-target}/$target/release"
    else
        cargo build --release -p k2-export --bin k2-export \
            || { echo "FATAL: k2-export build failed" >&2; return 1; }
        dir="${CARGO_TARGET_DIR:-target}/release"
    fi
    [ -x "$dir/k2-export" ] || { echo "FATAL: k2-export missing at $dir/k2-export after cargo build" >&2; return 1; }
    if grep -a -q K2_EXPORT_TEST_HOSTILE_ENABLED "$dir/k2-export"; then
        echo "FATAL: $dir/k2-export is a test-hostile build; never embed it" >&2
        return 1
    fi
    K2_EXPORT_EMBED="$(cd "$dir" && pwd)/k2-export"
    K2_REQUIRE_EXPORT_EMBED=1
    export K2_EXPORT_EMBED K2_REQUIRE_EXPORT_EMBED
    echo "  k2-export built for embedding: $K2_EXPORT_EMBED"
}

_k2_sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# Fail unless DAEMON carries the k2-ptyd that K2_PTYD_EMBED names (build.rs
# stores its sha256 hex next to the bytes).
assert_daemon_embeds_ptyd() {
    local daemon="$1" sha
    [ -n "${K2_PTYD_EMBED:-}" ] && [ -f "$K2_PTYD_EMBED" ] \
        || { echo "FATAL: K2_PTYD_EMBED is not set to a file (run k2_ptyd_build_for_embed first)" >&2; return 1; }
    [ -f "$daemon" ] || { echo "FATAL: daemon binary missing at $daemon" >&2; return 1; }
    sha="$(_k2_sha256_of "$K2_PTYD_EMBED")"
    [ "${#sha}" -eq 64 ] || { echo "FATAL: could not hash $K2_PTYD_EMBED" >&2; return 1; }
    if ! grep -a -q "$sha" "$daemon"; then
        echo "FATAL: $daemon does not embed k2-ptyd (sha256 $sha not found; P84)" >&2
        return 1
    fi
    echo "  ✓ $daemon embeds k2-ptyd (sha256 ${sha:0:12}…)."
}

# Fail unless DAEMON carries the k2-export that K2_EXPORT_EMBED names.
assert_daemon_embeds_export() {
    local daemon="$1" sha
    [ -n "${K2_EXPORT_EMBED:-}" ] && [ -f "$K2_EXPORT_EMBED" ] \
        || { echo "FATAL: K2_EXPORT_EMBED is not set to a file (run k2_export_build_for_embed first)" >&2; return 1; }
    [ -f "$daemon" ] || { echo "FATAL: daemon binary missing at $daemon" >&2; return 1; }
    sha="$(_k2_sha256_of "$K2_EXPORT_EMBED")"
    [ "${#sha}" -eq 64 ] || { echo "FATAL: could not hash $K2_EXPORT_EMBED" >&2; return 1; }
    if ! grep -a -q "$sha" "$daemon"; then
        echo "FATAL: $daemon does not embed k2-export (sha256 $sha not found)" >&2
        return 1
    fi
    echo "  ✓ $daemon embeds k2-export (sha256 ${sha:0:12}…)."
}
