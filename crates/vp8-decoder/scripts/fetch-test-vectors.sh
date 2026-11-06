#!/bin/sh
# Downloads the WebM project's VP8 test vectors (and the per-frame MD5 files
# libvpx publishes beside them) into DIR, and checks each file against the
# SHA-256 in tests/vectors.sha256. The vectors are test data only: they are
# not part of this repository or the published crate.
#
#   scripts/fetch-test-vectors.sh DIR
#   VP8_TEST_VECTORS=DIR cargo test -p vp8-decoder --features conformance --release
set -eu

dir=${1:?usage: fetch-test-vectors.sh DIR}
here=$(cd "$(dirname "$0")/.." && pwd)
manifest="$here/tests/vectors.sha256"
base=https://storage.googleapis.com/downloads.webmproject.org/test_data/libvpx

if command -v sha256sum >/dev/null 2>&1; then
    sha() { sha256sum "$1" | cut -d' ' -f1; }
else
    sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

mkdir -p "$dir"
while read -r want name; do
    [ -n "$name" ] || continue
    f="$dir/$name"
    if [ ! -f "$f" ] || [ "$(sha "$f")" != "$want" ]; then
        curl -sSf --retry 3 -o "$f.part" "$base/$name"
        mv "$f.part" "$f"
    fi
    got=$(sha "$f")
    if [ "$got" != "$want" ]; then
        echo "SHA-256 mismatch for $name: got $got, want $want" >&2
        exit 1
    fi
done < "$manifest"
echo "fetched $(grep -c . "$manifest") files into $dir"
