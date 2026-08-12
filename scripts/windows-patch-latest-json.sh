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
# Env: TAURI_SIGNING_PRIVATE_KEY (+ PASSWORD), optional K2_RELEASE_REPO
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

if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    if [ -f "$HOME/.tauri/k2-updater.key" ]; then
        export TAURI_SIGNING_PRIVATE_KEY
        TAURI_SIGNING_PRIVATE_KEY="$(cat "$HOME/.tauri/k2-updater.key")"
    elif [ -f "$HOME/.tauri/k2so-updater.key" ]; then
        export TAURI_SIGNING_PRIVATE_KEY
        TAURI_SIGNING_PRIVATE_KEY="$(cat "$HOME/.tauri/k2so-updater.key")"
    fi
fi
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || [ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]; then
    echo "ERROR: TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD required" >&2
    exit 1
fi

STAGE="$(mktemp -d -t k2-win-latest)"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

cp "$SETUP" "$STAGE/setup.exe"
echo "Signing $(basename "$SETUP") for updater..."
(
    cd "$STAGE"
    bunx @tauri-apps/cli@2 signer sign setup.exe \
        --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
        --password "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
)
# tauri signer writes setup.exe.sig as base64 of the minisig (same as Mac bundle)
if [ ! -f "$STAGE/setup.exe.sig" ]; then
    echo "ERROR: signer did not write setup.exe.sig" >&2
    ls -la "$STAGE" >&2
    exit 1
fi
SIG_CONTENT="$(tr -d '\n\r' < "$STAGE/setup.exe.sig")"
SETUP_URL="https://github.com/${RELEASE_REPO}/releases/download/${TAG}/K2_${VERSION}_x64-setup.exe"

echo "Fetching current latest.json..."
curl -fsSL "https://github.com/${RELEASE_REPO}/releases/download/${TAG}/latest.json" \
    -o "$STAGE/latest.json" \
    || curl -fsSL "https://github.com/${RELEASE_REPO}/releases/latest/download/latest.json" \
        -o "$STAGE/latest.json"

python3 - "$STAGE/latest.json" "$SIG_CONTENT" "$SETUP_URL" "$VERSION" <<'PY'
import json, sys
path, sig, url, ver = sys.argv[1:5]
with open(path) as f:
    data = json.load(f)
data["version"] = ver
plats = data.setdefault("platforms", {})
plats["windows-x86_64"] = {"signature": sig, "url": url}
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
