#!/bin/sh
# Build k2-isobmff's cargo-fuzz targets and run each for a while: `read`
# (open, every sample, sample bytes, timecode, configurations, Annex B of
# the first H.264 samples) and `annexb` (the H.264 conversion and the avcC
# record). Seeds come from examples/fuzz_seeds.rs (files made in code from
# the test fixtures and the writer). Needs a nightly toolchain (cargo-fuzz's
# sanitizers); installs cargo-fuzz when it is missing. SECONDS_EACH (default
# 120) sets each target's run.
# Usage, from the repository root, on Linux or macOS:
#   RUSTUP_TOOLCHAIN=nightly sh crates/k2-isobmff/scripts/check-fuzz.sh [target...]
# Crash inputs stay on the machine that found them (fuzz/artifacts/, ignored by git).
set -eu
[ $# -gt 0 ] || set -- read annexb
seconds=${SECONDS_EACH:-120}
command -v cargo-fuzz >/dev/null 2>&1 || cargo install --locked cargo-fuzz
root=$(pwd)
seeds="$root/target/k2-isobmff-fuzz-seeds"
rm -rf "$seeds" && mkdir -p "$seeds"
cargo run -q -p k2-isobmff --example fuzz_seeds -- "$seeds"
cd crates/k2-isobmff/fuzz
for target in "$@"; do
    echo "== build $target"
    cargo fuzz build "$target"
done
status=0
for target in "$@"; do
    echo "== run $target for ${seconds}s"
    corpus="corpus/$target"
    mkdir -p "$corpus"
    cp "$seeds/$target"/* "$corpus"/
    cargo fuzz run "$target" "$corpus" -- -max_total_time="$seconds" -timeout=30 -rss_limit_mb=2048 -malloc_limit_mb=1024 -max_len=262144 -print_final_stats=1 || status=$?
done
[ "$status" -eq 0 ] && echo "== every fuzz target ran clean" || echo "== a fuzz target stopped with a finding (exit $status)"
exit "$status"
