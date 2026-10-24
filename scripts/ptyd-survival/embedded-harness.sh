#!/bin/bash
# Run the T1 survival harness against a daemon built the way the release
# Linux and standalone macOS daemons are (prd-k2-ptyd-v1 P84): k2-ptyd
# embedded in k2-daemon, no K2_PTYD_BIN. The harness sees the build-time
# K2_PTYD_EMBED and checks the daemon wrote that exact k2-ptyd to its
# versioned path and runs it.
#
#   scripts/ptyd-survival/embedded-harness.sh
#
# Honours CARGO_TARGET_DIR (compute nodes set one). Uses a copy of k2-ptyd
# outside the target dir, so the embed path stays valid while cargo works.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build -p k2-ptyd --bins
target="${CARGO_TARGET_DIR:-$PWD/target}"
embed_dir="$(mktemp -d "${TMPDIR:-/tmp}/k2-ptyd-embed-XXXXXX")"
trap 'rm -rf "$embed_dir"' EXIT
cp "$target/debug/k2-ptyd" "$embed_dir/k2-ptyd"
K2_PTYD_EMBED="$embed_dir/k2-ptyd" K2_REQUIRE_PTYD_EMBED=1 \
    cargo test -p k2-daemon --test ptyd_survival_integration
