#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="$ROOT/scripts/macos-native-arch.sh"

[ "$($ARCH x86_64)" = x86_64 ]
[ "$($ARCH amd64)" = x86_64 ]
[ "$($ARCH arm64)" = aarch64 ]
[ "$($ARCH aarch64)" = aarch64 ]
if "$ARCH" sparc >/dev/null 2>&1; then
    echo "macos-native-arch accepted an unsupported architecture" >&2
    exit 1
fi

echo "macOS release architecture mapping: PASS"
