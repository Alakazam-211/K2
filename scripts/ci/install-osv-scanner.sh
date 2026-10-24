#!/usr/bin/env bash
# install-osv-scanner.sh <dir> — put a pinned osv-scanner release binary at
# <dir>/osv-scanner after checking its sha256 against the value pinned
# here (prd-security-sweep-0464-v1 SS14). Used by `scripts/ci/checks.sh
# audit` and the `audit` job in .github/workflows/checks.yml.
#
# Bumping: take the new release's osv-scanner_SHA256SUMS from
# https://github.com/google/osv-scanner/releases, pick a release at least
# two weeks old, and replace VERSION and the four sums below in one commit.
#
# Needs the internet (github.com release download).
set -euo pipefail

DIR="${1:?usage: install-osv-scanner.sh <dir>}"
VERSION="v2.6.0"

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64)  ASSET=osv-scanner_linux_amd64;  SUM=ca69b3d3cd08f889a49dc0a383122f71cc528b83803671df5fd874d97485b108 ;;
    Linux-aarch64) ASSET=osv-scanner_linux_arm64;  SUM=2c71403eb443d05891c4f268c3ad771cf4f16e5443463fd7851ef8f454d3c7e4 ;;
    Darwin-arm64)  ASSET=osv-scanner_darwin_arm64; SUM=98c460dcd37de25819babd757d04542045b6243113e209edcd4d89fedb0256b4 ;;
    Darwin-x86_64) ASSET=osv-scanner_darwin_amd64; SUM=60c5296637e977b28eeda5c7f13573e447659a632922737f94d11fa7e30ad6ca ;;
    *) echo "install-osv-scanner: no pinned build for $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

mkdir -p "$DIR"
if [ -x "$DIR/osv-scanner" ] && [ "$(sha256 "$DIR/osv-scanner")" = "$SUM" ]; then
    echo "install-osv-scanner: $VERSION already at $DIR/osv-scanner"
    exit 0
fi
tmp="$(mktemp "$DIR/.osv-scanner.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
curl -fsSL --retry 3 -o "$tmp" "https://github.com/google/osv-scanner/releases/download/$VERSION/$ASSET"
got="$(sha256 "$tmp")"
if [ "$got" != "$SUM" ]; then
    echo "install-osv-scanner: sha256 mismatch for $ASSET $VERSION: got $got, pinned $SUM" >&2
    exit 1
fi
chmod 0755 "$tmp"
mv "$tmp" "$DIR/osv-scanner"
trap - EXIT
echo "install-osv-scanner: $VERSION ($ASSET, sha256 ok) at $DIR/osv-scanner"
