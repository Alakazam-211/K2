#!/bin/sh
# Build and package; exits non-zero on the first failure.
set -eu

out="${OUT_DIR:-dist}"
mkdir -p "$out"

for target in linux darwin; do
	echo "building $target"
	GOOS="$target" go build -o "$out/tool-$target" ./cmd/tool
done

tar -czf "$out/tool.tar.gz" -C "$out" .
echo "done: $(ls "$out" | wc -l) files"