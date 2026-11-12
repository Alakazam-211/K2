#!/bin/sh
# Build k2-deckrender's cargo-fuzz targets and run each for a while:
# `parse` (sniffing, the vendored deckcraft-pptx import, the slide list) and
# `render` (the first slide, small). Seeds come from examples/fuzz_seeds.rs.
# Needs a nightly toolchain (cargo-fuzz's sanitizers); installs cargo-fuzz
# when it is missing. SECONDS_EACH (default 600) sets each target's run.
# Usage, from the repository root, on Linux or macOS:
#   RUSTUP_TOOLCHAIN=nightly sh crates/k2-deckrender/scripts/check-fuzz.sh [target...]
# Crash inputs stay on the machine that found them (fuzz/artifacts/, ignored
# by git): they may be pieces of real files.
set -eu
[ $# -gt 0 ] || set -- parse render
seconds=${SECONDS_EACH:-600}
command -v cargo-fuzz >/dev/null 2>&1 || cargo install --locked cargo-fuzz
root=$(pwd)
seeds="$root/target/k2-deckrender-fuzz-seeds"
rm -rf "$seeds" && mkdir -p "$seeds"
cargo run -q -p k2-deckrender --example fuzz_seeds -- "$seeds"
cd crates/k2-deckrender/fuzz
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
    cp "$seeds"/*.pptx "$corpus"/
    cargo fuzz run "$target" "$corpus" -- -max_total_time="$seconds" -timeout=30 -rss_limit_mb=4096 -malloc_limit_mb=3072 -max_len=1048576 -print_final_stats=1 || status=$?
done
[ "$status" -eq 0 ] && echo "== every fuzz target ran clean" || echo "== a fuzz target stopped with a finding (exit $status)"
exit "$status"
