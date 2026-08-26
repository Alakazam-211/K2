#!/bin/bash
# K2 DEV BUILD — GUI + k2-daemon sidecar, no Linux gate, no notarize, no version bump.
#
# `bun run build` / `tauri build` only produces the thin client. k2-daemon is
# not a macOS externalBin (Windows is), so the .app ships without a daemon
# unless a later step copies it in. This script is that later step, without
# the pre-release gates in build-app.sh or the publish path in release.sh.
#
# Usage:
#   ./scripts/dev-build.sh           # write target/release/bundle/macos/K2.app
#   ./scripts/dev-build.sh --install # also replace /Applications/K2.app + kickstart launchd
#   bun run build:dev
#
# Same GUI-session codesign rule as build-app.sh / release.sh (z3mbpZ iTerm,
# not plain SSH) or codesign hits errSecInternalComponent.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"
export PATH="$HOME/.cargo/bin:$PATH"

INSTALL=0
for arg in "$@"; do
    case "$arg" in
        --install) INSTALL=1 ;;
        -h|--help)
            sed -n '2,16p' "$0"
            exit 0
            ;;
        *)
            echo "Usage: ./scripts/dev-build.sh [--install]" >&2
            exit 1
            ;;
    esac
done

if [ -f "$PROJECT_DIR/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env"
    set +a
fi
# shellcheck source=scripts/require-mail-oauth-build-env.sh
source "$PROJECT_DIR/scripts/require-mail-oauth-build-env.sh"
export K2_REQUIRE_MICROSOFT_OAUTH="${K2_REQUIRE_MICROSOFT_OAUTH:-0}"
require_mail_oauth_build_env

SIGNING_IDENTITY="Developer ID Application: LZTEK, LLC (36B8R93HXV)"
ENTITLEMENTS="${PROJECT_DIR}/src-tauri/entitlements.plist"
APP="target/release/bundle/macos/K2.app"
LAUNCH_LABEL="dev.k2.daemon"

[ -f "$ENTITLEMENTS" ] || { echo "FATAL: entitlements not found at $ENTITLEMENTS" >&2; exit 1; }
VER="$(grep -m1 '"version"' src-tauri/tauri.conf.json | sed 's/.*: *"\([^"]*\)".*/\1/')"
echo "Dev-building K2.app v${VER} — GUI + k2-daemon (no Linux gate / notarize / publish)."

echo ""; echo "Step 1: tauri build..."
export APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY"
export APPLE_TEAM_ID="36B8R93HXV"
bun run tauri build || { echo "FATAL: tauri build failed" >&2; exit 1; }

echo ""; echo "Step 2: bundling k2-daemon sidecar..."
cargo build --release -p k2-daemon || { echo "FATAL: k2-daemon build failed" >&2; exit 1; }
[ -x "target/release/k2-daemon" ] || { echo "FATAL: k2-daemon missing after build" >&2; exit 1; }
assert_daemon_oauth_not_placeholder "target/release/k2-daemon"
cp "target/release/k2-daemon" "$APP/Contents/MacOS/k2-daemon"
echo "  k2-daemon copied into the bundle."

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

echo ""; echo "Step 4: launch smoke-test (AMFI exec check)..."
"$APP/Contents/MacOS/k2" --version >/tmp/k2-dev-smoke.out 2>&1 &
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
        head -c 400 /tmp/k2-dev-smoke.out >&2; echo "" >&2
        exit 1
    fi
    echo "  ✓ App exec exited rc=${SMOKE_RC} (not SIGKILL) — launchable past AMFI."
fi

if [ "$INSTALL" -eq 1 ]; then
    echo ""; echo "Step 5: installing to /Applications/K2.app + kickstart ${LAUNCH_LABEL}..."
    if pgrep -x k2 >/dev/null 2>&1; then
        echo "  quitting running K2 GUI..."
        osascript -e 'tell application "K2" to quit' >/dev/null 2>&1 || true
        sleep 1
        killall k2 2>/dev/null || true
    fi
    ditto "$APP" /Applications/K2.app
    codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGNING_IDENTITY" /Applications/K2.app
    UID_NUM="$(id -u)"
    launchctl kickstart -k "gui/${UID_NUM}/${LAUNCH_LABEL}" || {
        echo "  WARNING: launchctl kickstart failed — start K2 once or run:" >&2
        echo "    launchctl kickstart -k gui/${UID_NUM}/${LAUNCH_LABEL}" >&2
    }
    echo "  Installed. Pick This Mac in the server picker."
fi

echo ""
echo "════════════════════════════════════════════════════════════"
echo "  Dev build (NOT a release): ${PROJECT_DIR}/${APP}"
echo "  Launch:   open \"${APP}\""
if [ "$INSTALL" -eq 0 ]; then
    echo "  Install:  ./scripts/dev-build.sh --install"
fi
echo "  Pre-release gate:  ./scripts/build-app.sh"
echo "  Live ship:         ./scripts/release.sh ${VER}"
echo "════════════════════════════════════════════════════════════"
