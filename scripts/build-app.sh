#!/bin/bash
# K2 BUILD-ONLY script — builds + signs the .app bundle for LOCAL testing.
# Does NOT notarize, make a DMG, bump the version, or publish anything.
#
# Use this to verify a full, signed build actually LAUNCHES before running
# scripts/release.sh. Why it exists: a signed Developer-ID bundle can fail to
# launch (AMFI SIGKILL on a restricted entitlement) even when `tauri dev` runs
# fine and notarization passes — the 0.40.6 "K2 can't be opened" regression.
# Build here → `open` the result → confirm it runs → THEN release.
#
# Mirrors scripts/release.sh Steps 2/2.5/3 + the launch smoke-test, then stops.
set -uo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"
# rustup installs cargo at ~/.cargo/bin; non-interactive shells don't source it.
export PATH="$HOME/.cargo/bin:$PATH"

SIGNING_IDENTITY="Developer ID Application: LZTEK, LLC (36B8R93HXV)"
ENTITLEMENTS="${PROJECT_DIR}/src-tauri/entitlements.plist"
APP="target/release/bundle/macos/K2.app"
SKIP_SIGNING="${K2_SKIP_SIGNING:-0}"
TAURI_BUILD_ARGS=()
BUILD_STATUS="Built + signed"

if [ "$SKIP_SIGNING" = "1" ]; then
    echo "K2_SKIP_SIGNING=1: building an unsigned local artifact."
    TAURI_BUILD_ARGS+=(--no-sign --bundles app)
    BUILD_STATUS="Built unsigned"
else
    export APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY"
    export APPLE_TEAM_ID="36B8R93HXV"
fi

[ -f "$ENTITLEMENTS" ] || { echo "FATAL: entitlements not found at $ENTITLEMENTS" >&2; exit 1; }
VER="$(grep -m1 '"version"' src-tauri/tauri.conf.json | sed 's/.*: *"\([^"]*\)".*/\1/')"
echo "Building K2.app v${VER} — NO notarize / DMG / publish (local test build)."

# ── Step 0: Linux build gate ──
# This script is the pre-release "does the full build work" check, so it
# proves BOTH platforms: Linux first (fast — warm remote check is seconds),
# then the long signed mac build below. Shared logic + designated-box
# details in scripts/linux-build-gate.sh. Escape: K2_SKIP_LINUX_GATE=1.
echo ""; echo "Step 0a: k2so gate..."
bash "$PROJECT_DIR/scripts/k2so-gate.sh" || { echo "FATAL: k2so gate failed" >&2; exit 1; }

echo ""; echo "Step 0: Linux build gate..."
"$PROJECT_DIR/scripts/linux-build-gate.sh" || { echo "FATAL: Linux build gate failed" >&2; exit 1; }

# ── Step 1: Build the Tauri app ──
echo ""; echo "Step 1: tauri build..."
bun run tauri build "${TAURI_BUILD_ARGS[@]}" || { echo "FATAL: tauri build failed" >&2; exit 1; }
echo "  Build complete."

# ── Step 2: Build + bundle the k2-daemon sidecar ──
echo ""; echo "Step 2: bundling k2-daemon sidecar..."
cargo build --release -p k2-daemon || { echo "FATAL: k2-daemon build failed" >&2; exit 1; }
[ -x "target/release/k2-daemon" ] || { echo "FATAL: k2-daemon missing after build" >&2; exit 1; }
cp "target/release/k2-daemon" "$APP/Contents/MacOS/k2-daemon"
"$PROJECT_DIR/scripts/write-daemon-probe-marker.sh" "$APP"
echo "  k2-daemon copied into the bundle."

# ── Step 3: Sign (inner binaries first, then the bundle) ──
if [ "$SKIP_SIGNING" = "1" ]; then
    echo ""; echo "Step 3: signing skipped for unsigned local artifact."
else
    echo ""; echo "Step 3: signing with hardened runtime + entitlements..."
    codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGNING_IDENTITY" "$APP/Contents/MacOS/k2"
    codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGNING_IDENTITY" "$APP/Contents/MacOS/k2-daemon"
    FRPC_BIN="$APP/Contents/MacOS/frpc"
    if [ -x "$FRPC_BIN" ]; then
        codesign --force --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$FRPC_BIN"
        echo "  Signed frpc sidecar."
    fi
    "$PROJECT_DIR/scripts/write-daemon-probe-marker.sh" "$APP"
    codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGNING_IDENTITY" "$APP"
    echo "  Signed (main + daemon + frpc + bundle)."
fi

# ── Step 4: Launch smoke-test (AMFI exec check) ──
if [ "$SKIP_SIGNING" = "1" ]; then
    # An unsigned verification build must not initialize a daemon in the
    # user's real K2 home. Artifact architecture checks cover this mode.
    echo ""; echo "Step 4: launch smoke-test skipped for unsigned local artifact."
else
    echo ""; echo "Step 4: launch smoke-test (AMFI exec check)..."
    "$PROJECT_DIR/scripts/verify-macos-bundle.sh" "$APP"
    echo "  ✓ App + daemon executed version probes without starting either runtime."
fi

echo ""
echo "════════════════════════════════════════════════════════════"
echo "  ${BUILD_STATUS} (NOT notarized): ${PROJECT_DIR}/${APP}"
if [ "$SKIP_SIGNING" = "1" ]; then
    echo "  Verification-only artifact — DO NOT LAUNCH OR RELEASE."
else
    echo "  Launch + watch it run:   open \"${APP}\""
    echo "  When it runs cleanly:    ./scripts/release.sh ${VER}"
fi
echo "════════════════════════════════════════════════════════════"
