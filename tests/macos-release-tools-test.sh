#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="$ROOT/scripts/macos-native-arch.sh"
BUILD_LOCAL="$ROOT/scripts/build-local.sh"

[ "$($ARCH x86_64)" = x86_64 ]
[ "$($ARCH amd64)" = x86_64 ]
[ "$($ARCH arm64)" = aarch64 ]
[ "$($ARCH aarch64)" = aarch64 ]
if "$ARCH" sparc >/dev/null 2>&1; then
    echo "macos-native-arch accepted an unsupported architecture" >&2
    exit 1
fi

[ "$(grep -c -- '--entitlements "$ENTITLEMENTS"' "$BUILD_LOCAL")" -eq 3 ] || {
    echo "build-local must preserve entitlements on app and daemon signatures" >&2
    exit 1
}

echo "macOS release architecture mapping: PASS"
