#!/bin/sh
# Encodes a real camera clip at a few sizes and settings and decodes the
# results in headless Chromium (WebCodecs VideoDecoder): every frame must be
# byte-identical to vp8-decoder's output. Run from the repository root.
#
# Needs a Rust toolchain, Node 22 and network access (the clip, playwright
# and its Chromium are downloaded into WORKDIR).
#   crates/vp8-encoder/scripts/chrome-check.sh [WORKDIR]
set -eu

work=${1:-/tmp/vp8-chrome-check}
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$work"
clip="$work/niklas_1280_720_30.y4m"
if [ ! -f "$clip" ]; then
    curl -sSf --retry 3 -o "$clip" \
        https://storage.googleapis.com/downloads.webmproject.org/test_data/libvpx/niklas_1280_720_30.y4m
fi

cargo build --locked --release -p vp8-encoder --example encode_y4m 2>&1 | grep -E "^(warning|error)" || true
bin="$PWD/target/release/examples/encode_y4m"
"$bin" "$clip" "$work/720p.ivf" 2000000 --frames 30
"$bin" "$clip" "$work/720p-l1t3.ivf" 1000000 --layers 3 --frames 30
"$bin" "$clip" "$work/360p.ivf" 600000 640 360 --frames 30
"$bin" "$clip" "$work/odd.ivf" 300000 321 179 --frames 15

cd "$work"
[ -f package.json ] || echo '{"private":true,"type":"module"}' > package.json
npm install --silent --no-audit --no-fund playwright@1.61.1 >/dev/null
npx --yes playwright install chromium >/dev/null
cp "$here/chrome-webcodecs-check.mjs" "$work/"
node chrome-webcodecs-check.mjs 720p.ivf 720p-l1t3.ivf 360p.ivf odd.ivf
