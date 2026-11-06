#!/bin/sh
# Encodes a real camera clip at the sizes and rates of a WebRTC simulcast
# ladder (the 60-frame clip played forward and back to 300 frames) and
# prints, for each, the bitrate achieved against the target and
# the quantizer range rate control used. The test suite checks the same on
# synthetic camera-like content (tests/bitrate.rs); this is the real-clip
# cross-check.
#
# The clip is downloaded for the run, never committed.
# Needs: curl, a Rust toolchain. Run from the repository root:
#   crates/vp8-encoder/scripts/rate-check.sh [WORKDIR]
set -eu

work=${1:-/tmp/vp8-rate-check}
mkdir -p "$work"
clip="$work/niklas_1280_720_30.y4m"
if [ ! -f "$clip" ]; then
    curl -sSf --retry 3 -o "$clip" \
        https://storage.googleapis.com/downloads.webmproject.org/test_data/libvpx/niklas_1280_720_30.y4m
fi

cargo build --locked --release -q -p vp8-encoder --example encode_y4m
for layers in 1 3; do
    for size in "640 360" "1280 720"; do
        for rate in 300000 600000 1200000 2500000; do
            # shellcheck disable=SC2086 # size is two words on purpose
            cargo run --locked --release -q -p vp8-encoder --example encode_y4m -- \
                "$clip" "$work/out.ivf" "$rate" $size --layers "$layers" --frames 300 --loop
        done
    done
done
