#!/bin/sh
# Encodes a real camera clip at several sizes and settings, then checks that
# libvpx (through ffmpeg; the decoder Chrome uses) decodes every stream to
# exactly the same pictures as vp8-decoder. Prints the encoder's speed,
# bitrate and PSNR for each.
#
# ffmpeg is an outside test tool here, never a dependency of the crates.
# Needs: ffmpeg, curl, a Rust toolchain. Run from the repository root:
#   crates/vp8-encoder/scripts/libvpx-crosscheck.sh [WORKDIR]
set -eu

work=${1:-/tmp/vp8-crosscheck}
mkdir -p "$work"
clip="$work/niklas_1280_720_30.y4m"
if [ ! -f "$clip" ]; then
    curl -sSf --retry 3 -o "$clip" \
        https://storage.googleapis.com/downloads.webmproject.org/test_data/libvpx/niklas_1280_720_30.y4m
fi

cargo build --locked --release -p vp8-encoder --example encode_y4m 2>&1 | grep -E "^(warning|error)" || true
bin=target/release/examples/encode_y4m

fail=0
run() {
    name=$1
    shift
    out="$work/$name.ivf"
    "$bin" "$clip" "$out" "$@"
    # libvpx's decoder, then ffmpeg's own VP8 decoder.
    ffmpeg -hide_banner -loglevel error -y -c:v libvpx -i "$out" -f rawvideo -pix_fmt yuv420p "$work/$name.libvpx.yuv"
    ffmpeg -hide_banner -loglevel error -y -c:v vp8 -i "$out" -f rawvideo -pix_fmt yuv420p "$work/$name.ffvp8.yuv"
    for other in libvpx ffvp8; do
        if cmp -s "$work/$name.$other.yuv" "$out.dec.yuv"; then
            echo "[crosscheck] $name: $other and vp8-decoder decode identically ($(wc -c < "$out.dec.yuv") bytes of I420)"
        else
            echo "[crosscheck] $name: MISMATCH between $other and vp8-decoder" >&2
            fail=1
        fi
    done
}

run 720p-2m 2000000
run 720p-1m-l1t3 1000000 --layers 3
run 360p-600k 600000 640 360
run 180p-150k 150000 320 180
run 1080p-4m 4000000 1920 1080
exit $fail
