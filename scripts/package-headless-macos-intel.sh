#!/usr/bin/env bash
# Package an already-built native Intel daemon for K2's signed Shape-B updater.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DAEMON=""
OUT=""
BASE_URL=""
SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-}"

usage() {
    echo "Usage: $0 --daemon PATH --out-dir DIR --base-url URL --signing-identity ID" >&2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --daemon) DAEMON="${2:-}"; shift 2 ;;
        --out-dir) OUT="${2:-}"; shift 2 ;;
        --base-url) BASE_URL="${2:-}"; shift 2 ;;
        --signing-identity) SIGNING_IDENTITY="${2:-}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
    esac
done

[ "$(uname -s)" = Darwin ] || { echo "macOS host required" >&2; exit 1; }
[ "$(uname -m)" = x86_64 ] || { echo "native x86_64 host required" >&2; exit 1; }
[ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" != 1 ] || {
    echo "Rosetta-translated process is not a native Intel host" >&2; exit 1;
}
[ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" != 1 ] || {
    echo "Apple Silicon hardware is not a native Intel host" >&2; exit 1;
}
[ -n "$DAEMON" ] && [ -x "$DAEMON" ] || { echo "executable daemon required" >&2; exit 1; }
[ -n "$OUT" ] && [ ! -e "$OUT" ] || { echo "fresh --out-dir required" >&2; exit 1; }
[ -n "$SIGNING_IDENTITY" ] || { echo "Apple signing identity required" >&2; exit 1; }
[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || { echo "TAURI_SIGNING_PRIVATE_KEY required" >&2; exit 1; }
[ -n "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ] || { echo "TAURI_SIGNING_PRIVATE_KEY_PASSWORD required" >&2; exit 1; }

BASE_URL="${BASE_URL%/}"
python3 - "$BASE_URL" <<'PY' || { echo "--base-url must be https or loopback http" >&2; exit 1; }
import sys
import unicodedata
from urllib.parse import unquote, urlsplit

url = sys.argv[1]
u = urlsplit(url)
decoded_path = unquote(u.path)
unsafe = lambda value: any(
    c in {'"', "'", "\\"} or c.isspace() or unicodedata.category(c) == "Cc"
    for c in value
)
if u.username is not None or u.password is not None or u.query or u.fragment:
    raise SystemExit(1)
if unsafe(url) or unsafe(decoded_path):
    raise SystemExit(1)
if any(part in {".", ".."} for part in decoded_path.split("/")):
    raise SystemExit(1)
try:
    u.port
except ValueError:
    raise SystemExit(1)
if u.scheme == "https" and u.hostname:
    raise SystemExit(0)
if u.scheme == "http" and u.hostname in {"127.0.0.1", "localhost", "::1"}:
    raise SystemExit(0)
raise SystemExit(1)
PY

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/crates/k2-daemon/Cargo.toml" | head -1)"
[ -n "$VERSION" ] || { echo "cannot determine daemon version" >&2; exit 1; }
TAG="v$VERSION"
git -C "$ROOT" diff --quiet HEAD -- || { echo "tracked source differs from HEAD" >&2; exit 1; }
git -C "$ROOT" diff --cached --quiet HEAD -- || { echo "index differs from HEAD" >&2; exit 1; }
[ "$(git -C "$ROOT" cat-file -t "$TAG")" = tag ] || { echo "$TAG is not an annotated tag" >&2; exit 1; }
[ "$(git -C "$ROOT" rev-parse HEAD)" = "$(git -C "$ROOT" rev-parse "$TAG^{}")" ] || {
    echo "HEAD is not exact $TAG" >&2; exit 1;
}
grep -q "^K2_CLI_VERSION=\"$VERSION\"$" "$ROOT/cli/k2" || {
    echo "CLI version does not match daemon source" >&2; exit 1;
}
[ "$(git -C "$ROOT" hash-object cli/k2)" = "$(git -C "$ROOT" rev-parse "$TAG:cli/k2")" ] || {
    echo "CLI bytes do not match $TAG" >&2; exit 1;
}
[ "$(lipo -archs "$DAEMON")" = x86_64 ] || { echo "daemon is not x86_64-only" >&2; exit 1; }
file "$DAEMON" | grep -q 'Mach-O.*x86_64' || { echo "daemon is not an x86_64 Mach-O" >&2; exit 1; }
grep -aq "$VERSION" "$DAEMON" || { echo "daemon lacks source version $VERSION" >&2; exit 1; }

PARENT="$(dirname "$OUT")"
mkdir -p "$PARENT"
STAGE="$(mktemp -d "$PARENT/.k2-headless-intel.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
ASSET="k2-daemon-macos-x86_64"
install -m 0755 "$DAEMON" "$STAGE/$ASSET"

codesign --force --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$STAGE/$ASSET"
codesign --verify --strict --verbose=2 "$STAGE/$ASSET"
chmod 0555 "$STAGE/$ASSET"

bunx @tauri-apps/cli@2 signer sign "$STAGE/$ASSET"
if ! head -c 17 "$STAGE/$ASSET.sig" | grep -q 'untrusted comment'; then
    base64 -d < "$STAGE/$ASSET.sig" > "$STAGE/$ASSET.sig.raw" 2>/dev/null \
        || base64 -D < "$STAGE/$ASSET.sig" > "$STAGE/$ASSET.sig.raw"
    head -c 17 "$STAGE/$ASSET.sig.raw" | grep -q 'untrusted comment' || {
        echo "invalid updater signature" >&2; exit 1;
    }
    mv "$STAGE/$ASSET.sig.raw" "$STAGE/$ASSET.sig"
fi

mkdir "$STAGE/cli"
install -m 0555 "$ROOT/cli/k2" "$STAGE/cli/k2"
SHA="$(shasum -a 256 "$STAGE/$ASSET" | awk '{print $1}')"
python3 - "$VERSION" "$BASE_URL" "$ASSET" "$SHA" "$STAGE/daemon-latest.json" <<'PY'
import json
import sys

version, base_url, asset, sha256, output = sys.argv[1:]
document = {
    "version": version,
    "artifacts": {
        "macos-x86_64": {
            "url": f"{base_url}/{asset}",
            "sig": f"{base_url}/{asset}.sig",
            "sha256": sha256,
        }
    },
}
with open(output, "x", encoding="utf-8") as handle:
    json.dump(document, handle, indent=2)
    handle.write("\n")
PY
(
    cd "$STAGE"
    shasum -a 256 "$ASSET" "$ASSET.sig" cli/k2 daemon-latest.json > SHA256SUMS
)
chmod 0444 "$STAGE/$ASSET.sig" "$STAGE/daemon-latest.json" "$STAGE/SHA256SUMS"
mv "$STAGE" "$OUT"
trap - EXIT
echo "packaged K2 $VERSION macos-x86_64 at $OUT"
