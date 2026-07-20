#!/bin/bash
# K2 Local-Only Release Build
#
# Builds, signs, and notarizes a native K2 app + DMG for controlled
# on-machine testing and immutable production staging.
#
# Does NOT upload to GitHub. Does NOT generate latest.json. Does NOT
# tag the commit. Safe to run multiple times against the same
# version string — each run overwrites the previous DMG.
#
# Prerequisites (same as release.sh):
#   - TAURI_SIGNING_PRIVATE_KEY env var (or ~/.tauri/k2-updater.key)
#   - TAURI_SIGNING_PRIVATE_KEY_PASSWORD env var (or will prompt)
#   - Apple signing identity in keychain
#   - ASC_API_KEY_P8 / ASC_API_KEY_ID / ASC_API_ISSUER, or the
#     "K2SO-notarize" notarytool keychain profile
#
# Usage:
#   ./scripts/build-local.sh <version>
#   Example: ./scripts/build-local.sh 0.33.0-rc1
#
# Output:
#   target/release/bundle/dmg/K2_<version>_<native-arch>.dmg
#   target/release/bundle/dmg/K2_<version>_<native-arch>.provenance.json
#
# After the script finishes, follow docs/macos-production-release.md.

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
ENTITLEMENTS="$PROJECT_DIR/src-tauri/entitlements.plist"
MAC_ARCH="$("$PROJECT_DIR/scripts/macos-native-arch.sh")"
DMG_NAME="K2_${VERSION}_${MAC_ARCH}.dmg"
DMG_PATH="target/release/bundle/dmg/$DMG_NAME"
PROVENANCE_PATH="target/release/bundle/dmg/K2_${VERSION}_${MAC_ARCH}.provenance.json"
SOURCE_COMMIT="$(git -C "$PROJECT_DIR" rev-parse HEAD)"
SOURCE_DIRTY=false
[ -f "$ENTITLEMENTS" ] || {
    echo "FATAL: entitlements not found at $ENTITLEMENTS" >&2
    exit 1
}
if [ -n "$(git -C "$PROJECT_DIR" status --porcelain)" ]; then
    SOURCE_DIRTY=true
fi
if [ "$SOURCE_DIRTY" = true ] && [ "${K2_ALLOW_DIRTY:-0}" != 1 ]; then
    echo "FATAL: signed local releases require a clean source tree." >&2
    echo "  Commit/stash the reviewed changes, or set K2_ALLOW_DIRTY=1 for a development-only candidate." >&2
    exit 1
fi
if [ "$SOURCE_DIRTY" = true ]; then
    echo "WARNING: building a development-only artifact from a dirty source tree." >&2
fi

notary_auth_args() {
    if [ -n "${ASC_API_KEY_P8:-}" ] && [ -n "${ASC_API_KEY_ID:-}" ] && [ -n "${ASC_API_ISSUER:-}" ]; then
        printf '%s\n' --key "$ASC_API_KEY_P8" --key-id "$ASC_API_KEY_ID" --issuer "$ASC_API_ISSUER"
    else
        printf '%s\n' --keychain-profile "$KEYCHAIN_PROFILE"
    fi
}

# ── Step 1: Verify checked-in version ──
# Production provenance must describe the exact source commit. Never rewrite
# version-bearing source files inside the build; require the reviewed tree to
# already carry one consistent version.
cd "$PROJECT_DIR"
echo ""
echo "Step 1: Verifying checked-in version ${VERSION}..."
assert_version() {
    local label="$1" actual="$2"
    if [ "$actual" != "$VERSION" ]; then
        echo "  FATAL: $label reports '$actual', expected '$VERSION'." >&2
        exit 1
    fi
}
json_version() {
    sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$1" | head -1
}
cargo_version() {
    sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$1" | head -1
}
assert_version package.json "$(json_version package.json)"
assert_version src-tauri/tauri.conf.json "$(json_version src-tauri/tauri.conf.json)"
assert_version src-tauri/Cargo.toml "$(cargo_version src-tauri/Cargo.toml)"
assert_version crates/k2-daemon/Cargo.toml "$(cargo_version crates/k2-daemon/Cargo.toml)"
assert_version crates/k2-core/Cargo.toml "$(cargo_version crates/k2-core/Cargo.toml)"
assert_version cli/k2 "$(sed -n 's/^K2_CLI_VERSION="\([^"]*\)".*/\1/p' cli/k2 | head -1)"
echo "  Version sources agree."
if [ "${K2_PREFLIGHT_ONLY:-0}" = 1 ]; then
    echo "  Preflight only: no credentials loaded and no build started."
    exit 0
fi

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
echo "  Native architecture: ${MAC_ARCH}"
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

# ── Step 2: Build ──
#
# Nuke `target/release/` entirely before building. `cargo clean -p k2so`
# targeted cleaning has repeatedly left stale env!("CARGO_PKG_VERSION")
# strings baked into the Tauri binary after a checked-in version change —
# cargo's incremental fingerprint surface does not include the package
# version, so the crate's object files get reused. The nuclear option
# trades ~15 min of cold-compile time for a guarantee that the shipped
# binary's self-reported version matches the sources verified in Step 1.
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
"$PROJECT_DIR/scripts/write-daemon-probe-marker.sh" \
    "target/release/bundle/macos/K2.app"
echo "  k2-daemon copied into K2.app/Contents/MacOS/"

# The app, daemon, and frpc must be one native-architecture release pair.
# This also executes the early `k2 --version` path, which exits before Tauri
# can install or reload the shared launchd label.
APP="target/release/bundle/macos/K2.app"
"$PROJECT_DIR/scripts/verify-macos-bundle.sh" "$APP" "$MAC_ARCH"
BUNDLE_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
if [ "$BUNDLE_VERSION" != "$VERSION" ]; then
    echo "  FATAL: bundle version '$BUNDLE_VERSION' does not match requested '$VERSION'." >&2
    exit 1
fi
echo "  Build + architecture + version checks passed."

# ── Step 3: Sign with hardened runtime ──
echo ""
echo "Step 3: Signing with hardened runtime..."
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2"
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-daemon"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/frpc"
"$PROJECT_DIR/scripts/write-daemon-probe-marker.sh" "$APP"
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app"
codesign --verify --deep --strict --verbose=2 "$APP"
"$PROJECT_DIR/scripts/verify-macos-bundle.sh" "$APP" "$MAC_ARCH"
echo "  Signed and verified (main + daemon + frpc + bundle)."

# ── Step 4: Notarize app via ZIP ──
echo ""
echo "Step 4: Notarizing app..."
NOTARY_AUTH=()
while IFS= read -r arg; do NOTARY_AUTH+=("$arg"); done < <(notary_auth_args)
cd target/release/bundle/macos
ditto -c -k --keepParent "K2.app" "/tmp/K2_${VERSION}.zip"
xcrun notarytool submit "/tmp/K2_${VERSION}.zip" \
    "${NOTARY_AUTH[@]}" --wait
xcrun stapler staple "K2.app"
xcrun stapler validate "K2.app"
spctl --assess --type execute --verbose=2 "K2.app"
echo "  App notarized and stapled."

# ── Step 5: Create DMG from notarized app ──
echo ""
echo "Step 5: Creating DMG..."
cd "$PROJECT_DIR"
rm -f "$DMG_PATH"
hdiutil create -volname "K2" \
    -srcfolder "target/release/bundle/macos/K2.app" \
    -ov -format UDZO \
    "$DMG_PATH"
codesign --force --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "$DMG_PATH"

# ── Step 6: Notarize DMG ──
echo ""
echo "Step 6: Notarizing DMG..."
xcrun notarytool submit "$DMG_PATH" \
    "${NOTARY_AUTH[@]}" --wait
xcrun stapler staple "$DMG_PATH"
xcrun stapler validate "$DMG_PATH"
echo "  DMG notarized and stapled."

# ── Step 7: Write artifact provenance ──
echo ""
echo "Step 7: Writing provenance..."
APP_BIN="$APP/Contents/MacOS/k2"
DAEMON_BIN="$APP/Contents/MacOS/k2-daemon"
FRPC_BIN="$APP/Contents/MacOS/frpc"
BUILT_AT="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
cat > "$PROVENANCE_PATH" <<MANIFEST
{
  "schema": 1,
  "version": "${VERSION}",
  "architecture": "${MAC_ARCH}",
  "source_commit": "${SOURCE_COMMIT}",
  "source_dirty_before_build": ${SOURCE_DIRTY},
  "built_at": "${BUILT_AT}",
  "signed": true,
  "notarized": true,
  "artifacts": {
    "dmg": { "name": "${DMG_NAME}", "sha256": "$(shasum -a 256 "$DMG_PATH" | awk '{print $1}')" },
    "k2": { "sha256": "$(shasum -a 256 "$APP_BIN" | awk '{print $1}')" },
    "k2-daemon": { "sha256": "$(shasum -a 256 "$DAEMON_BIN" | awk '{print $1}')" },
    "frpc": { "sha256": "$(shasum -a 256 "$FRPC_BIN" | awk '{print $1}')" }
  }
}
MANIFEST
if [ "$SOURCE_DIRTY" = true ]; then
    K2_ALLOW_DIRTY_PROVENANCE=1 \
        "$PROJECT_DIR/scripts/verify-macos-bundle.sh" \
        "$APP" "$MAC_ARCH" "$PROVENANCE_PATH" "$SOURCE_COMMIT"
else
    "$PROJECT_DIR/scripts/verify-macos-bundle.sh" \
        "$APP" "$MAC_ARCH" "$PROVENANCE_PATH" "$SOURCE_COMMIT"
fi
echo "  Provenance: $PROJECT_DIR/$PROVENANCE_PATH"

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Local build complete — v${VERSION}"
echo "═══════════════════════════════════════════════════"
echo ""
echo "DMG: $PROJECT_DIR/$DMG_PATH"
echo "Provenance: $PROJECT_DIR/$PROVENANCE_PATH"
echo ""
echo "Next steps:"
echo "  1. Review the provenance and verify the three bundled architectures."
echo "  2. Follow docs/macos-production-release.md for immutable paired staging."
echo "  3. Do not replace a live app or launchd registration without approval."
echo ""
echo "If you decide to cut a real release from this version:"
echo "  ./scripts/release.sh ${VERSION} [notes-file]"
echo "  (it rebuilds from scratch and adds the GitHub upload steps)"
