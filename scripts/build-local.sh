#!/bin/bash
# K2 Local-Only Release Build
#
# Builds, signs, and notarizes K2 into a DMG you can drag into
# /Applications for on-machine testing (especially the P4 acceptance
# checklist: close the lid, wake on schedule, reconnect from mobile).
#
# Does NOT upload to GitHub. Does NOT generate latest.json. Does NOT
# tag the commit. Safe to run multiple times against the same
# version string — each run overwrites the previous DMG.
#
# Prerequisites (same as release.sh):
#   - TAURI_SIGNING_PRIVATE_KEY env var (or ~/.tauri/k2-updater.key)
#   - TAURI_SIGNING_PRIVATE_KEY_PASSWORD env var (or will prompt)
#   - Apple signing identity in keychain ("K2SO-notarize" profile)
#
# Usage:
#   ./scripts/build-local.sh <version>
#   Example: ./scripts/build-local.sh 0.33.0-rc1
#
# Output:
#   target/release/bundle/dmg/K2_<version>_aarch64.dmg
#
# After the script finishes:
#   open target/release/bundle/dmg/
#   → drag K2.app to Applications → run the P4 acceptance checklist.

set -euo pipefail

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
    echo "Usage: ./scripts/build-local.sh <version>" >&2
    echo "Example: ./scripts/build-local.sh 0.33.0-rc1" >&2
    exit 1
fi

SIGNING_IDENTITY="Developer ID Application: LZTEK, LLC (36B8R93HXV)"
KEYCHAIN_PROFILE="K2SO-notarize"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# rustup installs cargo at ~/.cargo/bin, which interactive shells source
# via .zshrc / .bashrc. `bun run tauri build` spawns a non-interactive
# subshell that does NOT source those, so cargo appears missing. Prepend
# explicitly to survive that spawn path.
if [ -d "$HOME/.cargo/bin" ] && ! command -v cargo >/dev/null 2>&1; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not found on PATH. Install rustup or export PATH manually." >&2
    exit 1
fi

echo "═══════════════════════════════════════════════════"
echo "  K2 Local Build: v${VERSION}"
echo "  (no GitHub upload, no updater manifest)"
echo "═══════════════════════════════════════════════════"

# Load .env file if present (contains TAURI_SIGNING_PRIVATE_KEY_PASSWORD)
if [ -f "$PROJECT_DIR/.env" ]; then
    set -a
    source "$PROJECT_DIR/.env"
    set +a
    echo "Loaded .env"
fi

# Load signing key from file if env var not set. SAME key under either
# name — k2-updater.key is the post-rebrand name, k2so-updater.key the
# original; never rotate the key itself (updates would stop verifying).
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    KEY_FILE="$HOME/.tauri/k2-updater.key"
    [ -f "$KEY_FILE" ] || KEY_FILE="$HOME/.tauri/k2so-updater.key"
    if [ -f "$KEY_FILE" ]; then
        export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY_FILE")"
        echo "Loaded signing key from $KEY_FILE"
    else
        echo "ERROR: TAURI_SIGNING_PRIVATE_KEY not set and $KEY_FILE not found" >&2
        exit 1
    fi
fi

if [ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]; then
    echo "Enter signing key password:"
    read -s TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    export TAURI_SIGNING_PRIVATE_KEY_PASSWORD
fi

cd "$PROJECT_DIR"

# ── Step 1: Bump version ──
#
# Bumps all THREE Cargo packages so they report a consistent version.
# - src-tauri/Cargo.toml:       the main Tauri `k2so` bin
# - crates/k2-daemon/Cargo.toml: the daemon bin (otherwise /status
#                                  reports the crate's literal version
#                                  e.g. "0.33.0-dev", not the release)
# - crates/k2-core/Cargo.toml: the shared library both binaries link
echo ""
echo "Step 1: Bumping version to ${VERSION}..."
sed -i '' "s/\"version\": \"[^\"]*\"/\"version\": \"${VERSION}\"/" package.json src-tauri/tauri.conf.json
sed -i '' "s/^version = \"[^\"]*\"/version = \"${VERSION}\"/" src-tauri/Cargo.toml
sed -i '' "s/^version = \"[^\"]*\"/version = \"${VERSION}\"/" crates/k2-daemon/Cargo.toml
sed -i '' "s/^version = \"[^\"]*\"/version = \"${VERSION}\"/" crates/k2-core/Cargo.toml
sed -i '' "s/K2_CLI_VERSION=\"[^\"]*\"/K2_CLI_VERSION=\"${VERSION}\"/" cli/k2
echo "  Done."

# ── Step 2: Build ──
#
# Nuke `target/release/` entirely before building. `cargo clean -p k2so`
# targeted cleaning has repeatedly left stale env!("CARGO_PKG_VERSION")
# strings baked into the Tauri binary even after a version bump —
# cargo's incremental fingerprint surface does not include the package
# version, so the crate's object files get reused. The nuclear option
# trades ~15 min of cold-compile time for a guarantee that the shipped
# binary's self-reported version matches what we bumped in Step 1.
# Dependency crates also get rebuilt, which is the cost.
echo ""
echo "Step 2: Building release..."
export APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY"
export APPLE_TEAM_ID="36B8R93HXV"
rm -rf target/release/bundle target/release/deps/libk2so_lib* \
       target/release/deps/k2so-* target/release/deps/k2so_core-* \
       target/release/deps/k2so_daemon-* target/release/incremental \
       src-tauri/target/release 2>/dev/null || true
cargo clean -p k2so -p k2-daemon -p k2-core 2>&1 | tail -2 || true
bun run tauri build

# ── Step 2.1: Verify the bundled Tauri binary actually has the new version ──
#
# Cargo has surprised us twice by shipping a binary whose compiled-in
# CARGO_PKG_VERSION doesn't match the Cargo.toml. Fail the build loudly
# here if it happens again — better to stop now than 15 minutes into
# notarization with a wrong DMG.
APP_BIN="target/release/bundle/macos/K2.app/Contents/MacOS/k2"
if ! grep -aq "${VERSION}" "$APP_BIN" 2>/dev/null; then
    echo "  FATAL: built binary $APP_BIN does not contain the expected version string '${VERSION}'." >&2
    echo "  Cargo cache likely leaked a stale CARGO_PKG_VERSION. Check target/release/ pollution and retry." >&2
    exit 1
fi
echo "  Version check: built binary contains '${VERSION}' ✓"
echo "  Build complete."

# ── Step 2.5: Build + bundle k2-daemon sidecar ──
echo ""
echo "Step 2.5: Bundling k2-daemon sidecar..."
cargo build --release -p k2-daemon
DAEMON_SRC="target/release/k2-daemon"
if [ ! -x "$DAEMON_SRC" ]; then
    echo "  FATAL: k2-daemon not at $DAEMON_SRC after cargo build" >&2
    exit 1
fi
cp "$DAEMON_SRC" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-daemon"
echo "  k2-daemon copied into K2.app/Contents/MacOS/"

# ── Step 3: Sign with hardened runtime ──
echo ""
echo "Step 3: Signing with hardened runtime..."
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-daemon"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app"
echo "  Signed (main + daemon + bundle)."

# ── Step 4: Notarize app via ZIP ──
echo ""
echo "Step 4: Notarizing app..."
cd target/release/bundle/macos
ditto -c -k --keepParent "K2.app" "/tmp/K2_${VERSION}.zip"
xcrun notarytool submit "/tmp/K2_${VERSION}.zip" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
xcrun stapler staple "K2.app"
echo "  App notarized and stapled."

# ── Step 5: Create DMG from notarized app ──
echo ""
echo "Step 5: Creating DMG..."
cd "$PROJECT_DIR"
rm -f "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
hdiutil create -volname "K2" \
    -srcfolder "target/release/bundle/macos/K2.app" \
    -ov -format UDZO \
    "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
codesign --force --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"

# ── Step 6: Notarize DMG ──
echo ""
echo "Step 6: Notarizing DMG..."
xcrun notarytool submit "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
xcrun stapler staple "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
echo "  DMG notarized and stapled."

DMG_PATH="target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Local build complete — v${VERSION}"
echo "═══════════════════════════════════════════════════"
echo ""
echo "DMG: $PROJECT_DIR/$DMG_PATH"
echo ""
echo "Next steps:"
echo "  1. open $(dirname "$DMG_PATH")"
echo "  2. Double-click the DMG and drag K2.app into /Applications"
echo "  3. Launch K2 from /Applications (not the dev tree)"
echo "  4. Run the P4 acceptance checklist against the installed app"
echo ""
echo "If you decide to cut a real release from this version:"
echo "  ./scripts/release.sh ${VERSION} [notes-file]"
echo "  (it rebuilds from scratch and adds the GitHub upload steps)"
