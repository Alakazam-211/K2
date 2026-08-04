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

# Same OAuth bake-in as release.sh — signed local apps should exercise
# real Gmail email-link, not REPLACE_ME.
if [ -f "$PROJECT_DIR/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env"
    set +a
fi
# shellcheck source=scripts/require-mail-oauth-build-env.sh
source "$PROJECT_DIR/scripts/require-mail-oauth-build-env.sh"
require_mail_oauth_build_env

SIGNING_IDENTITY="Developer ID Application: LZTEK, LLC (36B8R93HXV)"
ENTITLEMENTS="${PROJECT_DIR}/src-tauri/entitlements.plist"
APP="target/release/bundle/macos/K2.app"

[ -f "$ENTITLEMENTS" ] || { echo "FATAL: entitlements not found at $ENTITLEMENTS" >&2; exit 1; }
VER="$(grep -m1 '"version"' src-tauri/tauri.conf.json | sed 's/.*: *"\([^"]*\)".*/\1/')"
echo "Building K2.app v${VER} — NO notarize / DMG / publish (local test build)."

# ── Step 0: Linux build gate ──
# This script is the pre-release "does the full build work" check, so it
# proves BOTH platforms: Linux first (fast — warm remote check is seconds),
# then the long signed mac build below. Shared logic + designated-box
# details in scripts/linux-build-gate.sh. Escape: K2_SKIP_LINUX_GATE=1.
echo ""; echo "Step 0a: k2-home gate..."
bash "$PROJECT_DIR/scripts/k2-home-gate.sh" || { echo "FATAL: k2-home gate failed" >&2; exit 1; }

echo ""; echo "Step 0: Linux build gate..."
"$PROJECT_DIR/scripts/linux-build-gate.sh" || { echo "FATAL: Linux build gate failed" >&2; exit 1; }

# ── Step 1: Build the Tauri app ──
echo ""; echo "Step 1: tauri build..."
export APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY"
export APPLE_TEAM_ID="36B8R93HXV"
bun run tauri build || { echo "FATAL: tauri build failed" >&2; exit 1; }
echo "  Build complete."

# ── Step 2: Build + bundle the k2-daemon sidecar ──
echo ""; echo "Step 2: bundling k2-daemon sidecar..."
cargo build --release -p k2-daemon || { echo "FATAL: k2-daemon build failed" >&2; exit 1; }
[ -x "target/release/k2-daemon" ] || { echo "FATAL: k2-daemon missing after build" >&2; exit 1; }
assert_daemon_oauth_not_placeholder "target/release/k2-daemon"
cp "target/release/k2-daemon" "$APP/Contents/MacOS/k2-daemon"
echo "  k2-daemon copied into the bundle."

# ── Step 3: Sign (inner binaries first, then the bundle) ──
echo ""; echo "Step 3: signing with hardened runtime + entitlements..."
codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGNING_IDENTITY" "$APP/Contents/MacOS/k2"
codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGNING_IDENTITY" "$APP/Contents/MacOS/k2-daemon"
FRPC_BIN="$APP/Contents/MacOS/frpc"
if [ -x "$FRPC_BIN" ]; then
    codesign --force --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$FRPC_BIN"
    echo "  Signed frpc sidecar."
fi
codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGNING_IDENTITY" "$APP"
echo "  Signed (main + daemon + frpc + bundle)."

# ── Step 4: Launch smoke-test (AMFI exec check) ──
# A GUI app ignores SIGTERM and runs an event loop, so SIGKILL it + children and
# do NOT `wait` (waiting on a SIGTERM-ignoring GUI hangs the script).
echo ""; echo "Step 4: launch smoke-test (AMFI exec check)..."
"$APP/Contents/MacOS/k2" --version >/tmp/k2-smoke.out 2>&1 &
SMOKE_PID=$!
sleep 2
if kill -0 "$SMOKE_PID" 2>/dev/null; then
    pkill -9 -P "$SMOKE_PID" 2>/dev/null || true
    kill -9 "$SMOKE_PID" 2>/dev/null || true
    echo "  ✓ App survived exec (not AMFI-killed) — launchable."
else
    SMOKE_RC=0; wait "$SMOKE_PID" 2>/dev/null || SMOKE_RC=$?
    if [ "$SMOKE_RC" -eq 137 ]; then
        echo "  FATAL: signed app SIGKILL'd at launch (137 = AMFI restricted entitlement)." >&2
        echo "  Check src-tauri/entitlements.plist. smoke output:" >&2
        head -c 400 /tmp/k2-smoke.out >&2; echo "" >&2
        exit 1
    fi
    echo "  ✓ App exec exited rc=${SMOKE_RC} (not SIGKILL) — launchable past AMFI."
fi

echo ""
echo "════════════════════════════════════════════════════════════"
echo "  Built + signed (NOT notarized): ${PROJECT_DIR}/${APP}"
echo "  Launch + watch it run:   open \"${APP}\""
echo "  When it runs cleanly:    ./scripts/release.sh ${VER}"
echo "════════════════════════════════════════════════════════════"
