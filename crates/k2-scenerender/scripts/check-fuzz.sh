#!/bin/sh
# Build k2-scenerender's cargo-fuzz targets and run each for a while:
# `decode` (the K2SCENE1 decoder and the listing's digests) and `render`
# (decode, plan and draw two small views). Seeds come from
# examples/fuzz_seeds.rs (scenes generated in code). Needs a nightly
# toolchain (cargo-fuzz's sanitizers); installs cargo-fuzz when it is
# missing. SECONDS_EACH (default 600) sets each target's run.
# Usage, from the repository root, on Linux or macOS:
#   RUSTUP_TOOLCHAIN=nightly sh crates/k2-scenerender/scripts/check-fuzz.sh [target...]
# Crash inputs stay on the machine that found them (fuzz/artifacts/, ignored by git).
set -eu
[ $# -gt 0 ] || set -- decode render
seconds=${SECONDS_EACH:-600}
command -v cargo-fuzz >/dev/null 2>&1 || cargo install --locked cargo-fuzz
root=$(pwd)
seeds="$root/target/k2-scenerender-fuzz-seeds"
rm -rf "$seeds" && mkdir -p "$seeds"
cargo run -q -p k2-scenerender --example fuzz_seeds -- "$seeds"
cd crates/k2-scenerender/fuzz
# The same versions as K2's lock (cargo prunes what the fuzz crate doesn't use).
[ -f Cargo.lock ] || cp "$root/Cargo.lock" Cargo.lock
for target in "$@"; do
    echo "== build $target"
    cargo fuzz build "$target"
done
status=0
for target in "$@"; do
    echo "== run $target for ${seconds}s"
    corpus="corpus/$target"
    mkdir -p "$corpus"
    cp "$seeds"/*.k2scene "$corpus"/
    cargo fuzz run "$target" "$corpus" -- -max_total_time="$seconds" -timeout=30 -rss_limit_mb=4096 -malloc_limit_mb=3072 -max_len=1048576 -print_final_stats=1 || status=$?
done
[ "$status" -eq 0 ] && echo "== every fuzz target ran clean" || echo "== a fuzz target stopped with a finding (exit $status)"
exit "$status"
