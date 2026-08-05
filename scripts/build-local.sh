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

# Load .env file if present (contains TAURI_SIGNING_PRIVATE_KEY_PASSWORD
# + K2_GMAIL_CLIENT_* for option_env! into k2-daemon).
if [ -f "$PROJECT_DIR/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env"
    set +a
    echo "Loaded .env"
fi
# shellcheck source=scripts/require-mail-oauth-build-env.sh
source "$PROJECT_DIR/scripts/require-mail-oauth-build-env.sh"
# Local/dev may lack Microsoft client id; release/GHA require it (default 1).
export K2_REQUIRE_MICROSOFT_OAUTH="${K2_REQUIRE_MICROSOFT_OAUTH:-0}"
require_mail_oauth_build_env

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
assert_daemon_oauth_not_placeholder "$DAEMON_SRC"
cp "$DAEMON_SRC" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-daemon"
echo "  k2-daemon copied into K2.app/Contents/MacOS/"

# Staple with retry: after notarytool --wait, the ticket can lag CloudKit
# (same helper as release.sh — premature staple → Error 65).
staple_with_retry() {
    local target="$1"
    local attempt
    for attempt in 1 2 3 4 5; do
        if xcrun stapler staple "$target"; then
            return 0
        fi
        echo "  staple attempt ${attempt} failed (ticket not propagated yet?) — retrying in 30s..."
        sleep 30
    done
    echo "FATAL: stapling $target failed after 5 attempts" >&2
    return 1
}

# ── Step 3: Sign with hardened runtime (mirrors release.sh) ──
echo ""
echo "Step 3: Signing with hardened runtime..."
ENTITLEMENTS="${PROJECT_DIR}/src-tauri/entitlements.plist"
if [ ! -f "$ENTITLEMENTS" ]; then
    echo "  FATAL: entitlements file not found at $ENTITLEMENTS" >&2
    exit 1
fi
# Inner binaries first (Apple requires sub-binaries signed before the outer bundle).
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2"
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-daemon"
# frpc tunnel sidecar — re-sign so notarization covers it.
FRPC_BIN="target/release/bundle/macos/K2.app/Contents/MacOS/frpc"
if [ -x "$FRPC_BIN" ]; then
    codesign --force --options runtime --timestamp \
        --sign "$SIGNING_IDENTITY" \
        "$FRPC_BIN"
    echo "  Signed frpc sidecar."
else
    echo "  WARNING: frpc sidecar not found at $FRPC_BIN" >&2
fi
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app"
echo "  Signed (main + daemon + frpc + bundle) with entitlements."

# ── Step 3.5: Launch smoke-test (AMFI exec check) ──
echo ""
echo "Step 3.5: Launch smoke-test (AMFI exec check)..."
SMOKE_BIN="target/release/bundle/macos/K2.app/Contents/MacOS/k2"
"$SMOKE_BIN" --version >/tmp/k2-smoke.out 2>&1 &
SMOKE_PID=$!
sleep 2
if kill -0 "$SMOKE_PID" 2>/dev/null; then
    pkill -9 -P "$SMOKE_PID" 2>/dev/null || true
    kill -9 "$SMOKE_PID" 2>/dev/null || true
    echo "  ✓ App survived exec (not AMFI-killed) — launchable."
else
    SMOKE_RC=0; wait "$SMOKE_PID" 2>/dev/null || SMOKE_RC=$?
    if [ "$SMOKE_RC" -eq 137 ]; then
        echo "  FATAL: signed app was SIGKILL'd at launch (137 = AMFI)." >&2
        head -c 400 /tmp/k2-smoke.out >&2; echo "" >&2
        exit 1
    fi
    echo "  ✓ App exec exited rc=$SMOKE_RC (not SIGKILL) — launchable past AMFI."
fi

# ── Step 4: Notarize app via ZIP ──
echo ""
echo "Step 4: Notarizing app..."
cd target/release/bundle/macos
ditto -c -k --keepParent "K2.app" "/tmp/K2_${VERSION}.zip"
xcrun notarytool submit "/tmp/K2_${VERSION}.zip" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
staple_with_retry "K2.app"
echo "  App notarized and stapled."

# ── Step 5: Create DMG from notarized app ──
echo ""
echo "Step 5: Creating DMG..."
cd "$PROJECT_DIR"
rm -f "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
# ditto preserves signature + stapled ticket (same as release.sh stage).
DMG_STAGE="$(mktemp -d)"
ditto "target/release/bundle/macos/K2.app" "$DMG_STAGE/K2.app"
ln -s /Applications "$DMG_STAGE/Applications"
hdiutil create -volname "K2" \
    -srcfolder "$DMG_STAGE" \
    -ov -format UDZO \
    "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
rm -rf "$DMG_STAGE"
codesign --force --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"

# ── Step 6: Notarize DMG ──
echo ""
echo "Step 6: Notarizing DMG..."
xcrun notarytool submit "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
staple_with_retry "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
echo "  DMG notarized and stapled."

# ── Step 7: Gatekeeper verify (same checks a clean Mac should pass) ──
echo ""
echo "Step 7: Verifying DMG + app Gatekeeper status..."
DMG_PATH="target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
codesign --verify --verbose=2 "$DMG_PATH"
xcrun stapler validate "$DMG_PATH"
spctl -a -t open --context context:primary-signature -vv "$DMG_PATH"
VERIFY_MNT="$(mktemp -d)"
hdiutil attach -nobrowse -readonly "$DMG_PATH" -mountpoint "$VERIFY_MNT"
codesign --verify --deep --strict --verbose=2 "$VERIFY_MNT/K2.app"
xcrun stapler validate "$VERIFY_MNT/K2.app"
spctl -a -vv -t exec "$VERIFY_MNT/K2.app"
hdiutil detach "$VERIFY_MNT" -quiet
rmdir "$VERIFY_MNT" 2>/dev/null || true
echo "  ✓ codesign + staple + spctl all green."

# ── Step 8: Clean-VM new-user pairing smoke (0.40.33/34 regression class) ──
# Fresh Sequoia guest, empty ~/.k2*, install this DMG, start bundled daemon,
# assert ~/.k2so → ~/.k2 symlink + /boot-status via the thin-client path.
# Fails the build on FAIL. Escape: K2_SKIP_VM_PAIRING_SMOKE=1 (loud).
echo ""
echo "Step 8: Clean-VM new-user pairing smoke..."
"$PROJECT_DIR/scripts/vm-newuser-pair-smoke.sh" \
  "$PROJECT_DIR/$DMG_PATH" \
  "$VERSION"

# Copy a stable path for handoff (Desktop) so you don't dig under target/
DESKTOP_DMG="$HOME/Desktop/K2_${VERSION}_aarch64.dmg"
cp -f "$PROJECT_DIR/$DMG_PATH" "$DESKTOP_DMG"
DMG_SHA="$(shasum -a 256 "$DESKTOP_DMG" | awk '{print $1}')"

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Local build complete — v${VERSION}"
echo "═══════════════════════════════════════════════════"
echo ""
echo "DMG (repo):     $PROJECT_DIR/$DMG_PATH"
echo "DMG (Desktop):  $DESKTOP_DMG"
echo "SHA-256:        $DMG_SHA"
echo ""
echo "On the other Mac:"
echo "  1. Copy the Desktop DMG over (AirDrop / USB)."
echo "  2. Double-click → drag K2.app to Applications."
echo "  3. Launch from /Applications (first launch may need right-click → Open)."
echo "  4. If 'damaged':  xattr -cr /Applications/K2.app"
echo "     then: spctl -a -vv /Applications/K2.app"
echo ""
echo "This did NOT publish to GitHub. Official release still uses:"
echo "  ./scripts/release.sh <version>"
