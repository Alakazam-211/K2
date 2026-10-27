#!/bin/bash
# windows-patch-latest-json.sh — minisign the Windows NSIS setup and merge
# `platforms.windows-x86_64` into the GitHub release's latest.json.
#
# Why: release.sh Step 8 publishes latest.json with darwin-aarch64 only
# (Mac build finishes first). Windows "Check for Updates" uses
# @tauri-apps/plugin-updater which requires platforms["windows-x86_64"].
# Without it: "None of the fallback platforms `["windows-x86_64"]` were
# found in the response 'platforms' object" (0.40.95).
#
# Usage (after setup.exe is on the release or local):
#   scripts/windows-patch-latest-json.sh <version> [path-to-setup.exe]
#
# Signs through scripts/lib/sign-updater.sh (key file + keychain password,
# read only for the signature; never exported, never on argv). Optional env:
# K2_RELEASE_REPO.
set -euo pipefail

VERSION="${1:-}"
SETUP="${2:-}"
RELEASE_REPO="${K2_RELEASE_REPO:-Alakazam-211/K2}"
TAG="v${VERSION}"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version> [setup.exe path]" >&2
    exit 1
fi

if [ -z "$SETUP" ]; then
    SETUP="$PROJECT_DIR/dist-windows/K2_${VERSION}_x64-setup.exe"
fi
if [ ! -f "$SETUP" ]; then
    echo "ERROR: setup not found: $SETUP" >&2
    exit 1
fi

# shellcheck source=lib/sign-updater.sh
source "$PROJECT_DIR/scripts/lib/sign-updater.sh"
k2u_env_guard

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/k2-win-latest.XXXXXX")"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

cp "$SETUP" "$STAGE/setup.exe"
echo "Signing $(basename "$SETUP") for updater..."
# Base64-wrapped minisig (b64), as the tauri signer wrote it and the Tauri
# updater reads it; trusted comment file:setup.exe as before.
k2u_sign u1 "$STAGE/setup.exe" setup.exe "$VERSION" "$STAGE/setup.exe.sig" b64
# U2 (SS4/SS5): `k2_signatures`, the only signature 0.46.5+ apps read. The
# asset name is the URL's file name (what the app checks).
WIN_ASSET="K2_${VERSION}_x64-setup.exe"
k2u_sign u2 "$STAGE/setup.exe" "$WIN_ASSET" "$VERSION" "$STAGE/setup.exe.sig2" raw
U2_KEY_ID="$(k2u_slot_key_id u2)"
SIG2_B64="$(k2u_b64 "$STAGE/setup.exe.sig2")"
if [ ! -f "$STAGE/setup.exe.sig" ]; then
    echo "ERROR: signer did not write setup.exe.sig" >&2
    ls -la "$STAGE" >&2
    exit 1
fi
SIG_CONTENT="$(tr -d '\n\r' < "$STAGE/setup.exe.sig")"
SETUP_URL="https://github.com/${RELEASE_REPO}/releases/download/${TAG}/K2_${VERSION}_x64-setup.exe"

# This release's own latest.json, never the `latest` alias: patching some
# other release's manifest and stamping this version on it would publish a
# mismatched manifest (security review). Missing → stop.
echo "Fetching ${TAG}'s latest.json..."
curl -fsSL "https://github.com/${RELEASE_REPO}/releases/download/${TAG}/latest.json" \
    -o "$STAGE/latest.json" \
    || { echo "ERROR: ${TAG} has no latest.json to patch (release.sh Step 9 publishes it); not falling back to another release" >&2; exit 1; }

python3 -I - "$STAGE/latest.json" "$SIG_CONTENT" "$SETUP_URL" "$VERSION" "$U2_KEY_ID" "$SIG2_B64" <<'PY'
import json, sys
path, sig, url, ver, u2_id, sig2 = sys.argv[1:7]
with open(path) as f:
    data = json.load(f)
if data.get("version") != ver:
    sys.exit("ERROR: the fetched latest.json is for %r, not %s; refusing to patch it" % (data.get("version"), ver))
plats = data.setdefault("platforms", {})
plats["windows-x86_64"] = {"signature": sig, "url": url, "k2_signatures": {u2_id: sig2}}
with open(path, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
print("platforms:", ", ".join(sorted(plats.keys())))
PY

echo "Uploading patched latest.json..."
gh release upload "$TAG" "$STAGE/latest.json" \
    --repo "$RELEASE_REPO" --clobber

# Also keep a local copy next to the setup for auditing
mkdir -p "$PROJECT_DIR/dist-windows"
cp "$STAGE/latest.json" "$PROJECT_DIR/dist-windows/latest.json"
cp "$STAGE/setup.exe.sig" "$PROJECT_DIR/dist-windows/K2_${VERSION}_x64-setup.exe.sig"

echo "OK: latest.json now includes windows-x86_64 → $SETUP_URL"
