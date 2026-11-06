#!/bin/sh
# Build kessel-frame-ring's cargo-fuzz targets and run each for a while:
# `consumer` (a ring corrupted under the consumer), `producer` (a ring
# corrupted under the producer) and `attach` (a producer opening any file).
# Needs a nightly toolchain (cargo-fuzz's sanitizers); installs cargo-fuzz
# when it is missing. SECONDS_EACH (default 120) sets each target's run.
# Usage, from the repository root, on Linux or macOS:
#   RUSTUP_TOOLCHAIN=nightly sh crates/kessel-frame-ring/scripts/check-fuzz.sh [target...]
set -eu
[ $# -gt 0 ] || set -- producer consumer attach
seconds=${SECONDS_EACH:-120}
command -v cargo-fuzz >/dev/null 2>&1 || cargo install --locked cargo-fuzz
cd crates/kessel-frame-ring/fuzz
for target in "$@"; do
    echo "== build $target"
    cargo fuzz build "$target"
done
for target in "$@"; do
    echo "== run $target for ${seconds}s"
    # A corpus folder of its own per run: fuzz/corpus is gitignored.
    cargo fuzz run "$target" -- -max_total_time="$seconds" -rss_limit_mb=4096
done
echo "== every fuzz target ran clean"
