#!/bin/bash
# build.sh — build "K2 Release Approve.app" (prd-signing-user-v1 §4.4) as the
# release user, on z3mbpZ:
#
#   bash scripts/k2sign/approve/build.sh [--out DIR] [--developer-id]
#
# Default: ~/Applications/K2 Release Approve.app, ad-hoc signed. The app
# needs no entitlement (its key is a CryptoKit blob in a file, not in the
# keychain), so an ad-hoc signature is enough; --developer-id signs with the
# release identity instead (S0 (a) checks both). The app is untrusted either
# way: the signer trusts only the Secure Enclave key it enrolled.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$HOME/Applications"
IDENTITY="-"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:?--out DIR}"; shift 2 ;;
        --developer-id) IDENTITY="Developer ID Application: LZTEK, LLC (36B8R93HXV)"; shift ;;
        *) echo "build.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
[ "$(uname -s)" = Darwin ] || { echo "build.sh: macOS only" >&2; exit 1; }
APP="$OUT/K2 Release Approve.app"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-approve-build.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/app/Contents/MacOS"
/usr/bin/xcrun swiftc -O -target arm64-apple-macos13.0 \
    -o "$WORK/app/Contents/MacOS/K2 Release Approve" "$HERE/main.swift"
cp "$HERE/Info.plist" "$WORK/app/Contents/Info.plist"
/usr/bin/codesign --force --options runtime --sign "$IDENTITY" "$WORK/app"
/usr/bin/codesign --verify --strict "$WORK/app"
mkdir -p "$OUT"
rm -rf "$APP"
mv "$WORK/app" "$APP"
echo "built: $APP (signed: $([ "$IDENTITY" = - ] && echo ad-hoc || echo "$IDENTITY"))"
echo "enrol once: \"$APP/Contents/MacOS/K2 Release Approve\" --enrol"
