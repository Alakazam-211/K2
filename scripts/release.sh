#!/bin/bash
# K2 Release Script
# Builds, signs, notarizes, and releases K2 with both DMG and update bundle.
# Also publishes the hosted-web SPA bundle to Cloudflare R2 (phase 3).
#
# RELEASE_REPO selects the GitHub repo for `gh release create` + all
# manifest/download URLs. Defaults to the new K2 home; override with
# K2_RELEASE_REPO=Alakazam-211/K2SO for an old-repo (bridge) release.
#
# Prerequisites:
#   - TAURI_SIGNING_PRIVATE_KEY env var (or ~/.tauri/k2-updater.key)
#   - TAURI_SIGNING_PRIVATE_KEY_PASSWORD env var
#   - Apple signing identity in keychain
#   - gh CLI authenticated
#
# Web-bundle publish (R2) — best-effort by default so desktop release is not
# blocked by R2 TLS warmup / missing laptop creds (Ops caveat on brand-new R2):
#   - Creds: ~/.config/cloudflare/r2.env or env R2_ACCESS_KEY_ID /
#     R2_SECRET_ACCESS_KEY / R2_ACCOUNT_ID (see scripts/publish-web-bundles.sh)
#   - Without creds: WARN + continue (publish uses --skip-if-no-creds)
#   - With creds but upload fails (TLS etc.): WARN + continue unless
#     K2_REQUIRE_WEB_PUBLISH=1, in which case the release FAILS
#   - Optional prune after publish: K2_WEB_BUNDLE_PRUNE=1
#   - GitHub Actions secrets (same names): R2_ACCOUNT_ID, R2_ACCESS_KEY_ID,
#     R2_SECRET_ACCESS_KEY (optional R2_ENDPOINT, R2_BUCKET)
#
# Usage:
#   ./scripts/release.sh <version>
#   Example: ./scripts/release.sh 0.25.0

set -euo pipefail

# Staple with retry: after `notarytool --wait` reports Accepted, the ticket
# can take a minute to propagate to CloudKit — a premature staple fails with
# "could not find ticket" (Error 65). Retry instead of dying (killed the
# 0.40.23 release on first attempt).
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

VERSION="${1:-}"
NOTES_FILE="${2:-}"
if [ -z "$VERSION" ]; then
    echo "Usage: ./scripts/release.sh <version> [notes-file]" >&2
    echo "Example: ./scripts/release.sh 0.25.0 release-notes.md" >&2
    echo "" >&2
    echo "If notes-file is provided, its contents are used as GitHub release notes." >&2
    echo "Otherwise, a placeholder is used (edit on GitHub after release)." >&2
    exit 1
fi

TAG="v${VERSION}"
SIGNING_IDENTITY="Developer ID Application: LZTEK, LLC (36B8R93HXV)"
KEYCHAIN_PROFILE="K2SO-notarize"   # machine-local notarytool profile name — NOT renamed
RELEASE_REPO="${K2_RELEASE_REPO:-Alakazam-211/K2}"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Notarization auth (0.39.45): prefer DIRECT App Store Connect API-key
# auth when the env provides it — the keychain profile lives in the
# data-protection keychain, which headless/agent sessions can't read
# (notarytool reports "No Keychain password item found" even though the
# profile exists/validates). Set in .env:
#   ASC_API_KEY_P8=/path/to/AuthKey_XXXX.p8
#   ASC_API_KEY_ID=XXXXXXXXXX
#   ASC_API_ISSUER=<issuer-uuid>
# Falls back to --keychain-profile K2SO-notarize when unset.
notary_auth_args() {
    if [ -n "${ASC_API_KEY_P8:-}" ] && [ -n "${ASC_API_KEY_ID:-}" ] && [ -n "${ASC_API_ISSUER:-}" ]; then
        printf '%s\n' --key "$ASC_API_KEY_P8" --key-id "$ASC_API_KEY_ID" --issuer "$ASC_API_ISSUER"
    else
        printf '%s\n' --keychain-profile "$KEYCHAIN_PROFILE"
    fi
}

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
echo "  K2 Release: ${TAG}  →  ${RELEASE_REPO}"
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

# ── Step 0.5: Pre-flight quality gates ──
# Added at 0.40.31 when the tree first reached warning-zero / typecheck-zero.
# Fail the release BEFORE any version bump or build if regressions slipped in.
# NOTE: bare `tsc --noEmit` is vacuous here (root tsconfig has files:[] +
# references) — typecheck:web (tsconfig.web.json) is the real renderer check.
# vite:build:web is the hosted-SPA production build (vite.config.web.ts) —
# typecheck alone does not prove the Vite graph / CSS / assets still bundle.
# R2 publish is Step 8.6 and stays best-effort; this gate is build-only.
echo ""
echo "Step 0.5: Pre-flight gates (cargo -D warnings, typecheck:web, vitest, vite:build:web)..."
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets
bun run typecheck:web
bunx vitest run --reporter=dot
bun run vite:build:web
echo "  Pre-flight gates passed."

# ── Step 0.6: Linux build gate (k2-sandbox-01) ──
# Shared with scripts/build-app.sh — full rationale + designated-box details
# live in scripts/linux-build-gate.sh. Escape: K2_SKIP_LINUX_GATE=1 (loud).
echo ""
echo "Step 0.6: Linux build gate..."
"$PROJECT_DIR/scripts/linux-build-gate.sh"

# ── Step 1: Bump version ──
echo ""
echo "Step 1: Bumping version to ${VERSION}..."
sed -i '' "s/\"version\": \"[^\"]*\"/\"version\": \"${VERSION}\"/" package.json src-tauri/tauri.conf.json
sed -i '' "s/^version = \"[^\"]*\"/version = \"${VERSION}\"/" \
    src-tauri/Cargo.toml \
    crates/k2-core/Cargo.toml \
    crates/k2-daemon/Cargo.toml
sed -i '' "s/K2_CLI_VERSION=\"[^\"]*\"/K2_CLI_VERSION=\"${VERSION}\"/" cli/k2
echo "  Done."

# ── Step 1.5: Verify WHATS_NEW.md has an entry for this version ──
#
# 0.38.7 release-script contract: every released version MUST have a
# matching `## <version>` header in WHATS_NEW.md (the user-facing
# changelog the Tauri popup displays on first launch after update).
#
# This is intentionally a hard gate — if the entry is missing, the
# release halts BEFORE any build/sign/notarize work so the developer
# has to write the user-facing notes first. WHATS_NEW.md is curated
# user-friendly language (separate audience from release-notes-X.Y.Z.md
# which goes to the GitHub release body).
echo ""
echo "Step 1.5: Verifying WHATS_NEW.md has an entry for ${VERSION}..."
if [ ! -f "WHATS_NEW.md" ]; then
    echo "  ERROR: WHATS_NEW.md not found at repo root." >&2
    echo "  Create it before releasing — see existing entries for format." >&2
    exit 1
fi
if ! grep -qE "^## ${VERSION//./\\.} " WHATS_NEW.md; then
    echo "  ERROR: WHATS_NEW.md has no '## ${VERSION} — ...' section." >&2
    echo "" >&2
    echo "  Every release needs a user-facing changelog entry. Add a" >&2
    echo "  section like:" >&2
    echo "" >&2
    echo "    ## ${VERSION} — Short headline" >&2
    echo "" >&2
    echo "    Friendly 1–3 paragraph description of what changed for" >&2
    echo "    end users. Lead with what they'll notice, not implementation" >&2
    echo "    detail. The Tauri app shows this in a popup on first launch" >&2
    echo "    after this version installs." >&2
    echo "" >&2
    echo "  Then re-run: ./scripts/release.sh ${VERSION} ${NOTES_FILE:-<notes-file>}" >&2
    exit 1
fi
echo "  Found '## ${VERSION}' entry. OK to proceed."

# ── Step 2: Build ──
echo ""
echo "Step 2: Building release..."
export APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY"
export APPLE_TEAM_ID="36B8R93HXV"
bun run tauri build
echo "  Build complete."

# ── Step 2.5: Build + bundle k2-daemon sidecar ──
#
# k2-daemon is a peer binary to the main Tauri app that owns the
# persistent-agent runtime (launched by launchd, outlives the Tauri
# process). It needs to sit next to `k2` inside `Contents/MacOS/`
# so `std::env::current_exe()?.parent()?.join("k2-daemon")` — used
# by the `install_daemon_plist_v1` code migration — can find it on
# first launch of a release build.
#
# We build it explicitly in release mode (cargo workspace builds it
# alongside the Tauri crate, but `tauri build` copies only its own
# primary bin into the bundle) then `cp` it in. Hardened-runtime
# signing in Step 3 covers this binary too.
echo ""
echo "Step 2.5: Bundling k2-daemon sidecar..."
# cargo workspace root is the repo root — both `k2` (Tauri) and
# `k2-daemon` build into `target/release/`. Tauri's bundler writes
# only its own primary bin into the .app, so we copy k2-daemon in
# explicitly.
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
# 0.37.9: pass --entitlements so codesign attaches our requested
# capabilities (audio-input for Apple Dictation, JIT/library
# validation relaxations for wry/WKWebView). Without this, the
# hardened runtime denies audio access and Fn-Fn silently fails.
ENTITLEMENTS="${PROJECT_DIR}/src-tauri/entitlements.plist"
if [ ! -f "$ENTITLEMENTS" ]; then
    echo "  FATAL: entitlements file not found at $ENTITLEMENTS" >&2
    exit 1
fi
# Inner binaries first (Apple requires sub-binaries signed before the
# outer bundle, otherwise codesign rejects with 'resource fork … not
# allowed').
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2"
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app/Contents/MacOS/k2-daemon"
# frpc tunnel sidecar (Tauri externalBin → Contents/MacOS/frpc). Re-sign
# with hardened runtime so the binary the app stages to ~/.k2/bin/frpc
# is notarization-covered and runs without a Gatekeeper quarantine block.
FRPC_BIN="target/release/bundle/macos/K2.app/Contents/MacOS/frpc"
if [ -x "$FRPC_BIN" ]; then
    codesign --force --options runtime --timestamp \
        --sign "$SIGNING_IDENTITY" \
        "$FRPC_BIN"
    echo "  Signed frpc sidecar."
else
    echo "  WARNING: frpc sidecar not found at $FRPC_BIN — K2 Connect host" >&2
    echo "  setup will require a manual frpc install. Did fetch-frpc.sh run?" >&2
fi
codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2.app"
echo "  Signed (main + daemon + frpc + bundle) with entitlements."

# ── Step 3.5: Launch smoke-test — catch AMFI exec rejections (0.40.6 regression) ──
# A signed Developer-ID app carrying a RESTRICTED entitlement (e.g.
# keychain-access-groups) with NO embedded provisioning profile is SIGKILL'd by
# AMFI at exec — it notarizes fine but users get the Finder "K2 can't be opened"
# dialog. `tauri dev` never enforces this, so it slips past dev testing. Exec the
# freshly-signed binary here and FAIL the release if AMFI kills it (rc 137),
# BEFORE wasting a notarization round-trip or publishing an un-launchable build.
echo ""
echo "Step 3.5: Launch smoke-test (AMFI exec check)..."
SMOKE_BIN="target/release/bundle/macos/K2.app/Contents/MacOS/k2"
"$SMOKE_BIN" --version >/tmp/k2-smoke.out 2>&1 &
SMOKE_PID=$!
sleep 2
if kill -0 "$SMOKE_PID" 2>/dev/null; then
    # Alive past the AMFI exec check = launchable. A GUI app ignores SIGTERM and
    # never exits on its own, so SIGKILL it + its children and do NOT `wait` —
    # waiting on a SIGTERM-ignoring GUI hangs the release until the harness
    # timeout (the 0.40.6 re-release failure). kill -9 is uncatchable.
    pkill -9 -P "$SMOKE_PID" 2>/dev/null || true
    kill -9 "$SMOKE_PID" 2>/dev/null || true
    echo "  ✓ App survived exec (not AMFI-killed) — launchable."
else
    SMOKE_RC=0; wait "$SMOKE_PID" 2>/dev/null || SMOKE_RC=$?
    if [ "$SMOKE_RC" -eq 137 ]; then
        echo "  FATAL: signed app was SIGKILL'd at launch (137 = AMFI) — almost certainly a" >&2
        echo "  restricted entitlement without a provisioning profile. NOT releasing." >&2
        echo "  Check src-tauri/entitlements.plist. smoke output:" >&2
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
NOTARY_AUTH=()
while IFS= read -r arg; do NOTARY_AUTH+=("$arg"); done < <(notary_auth_args)
xcrun notarytool submit "/tmp/K2_${VERSION}.zip" \
    "${NOTARY_AUTH[@]}" --wait
staple_with_retry "K2.app"
echo "  App notarized and stapled."

# ── Step 5: Create update bundle (tar.gz) from notarized app + sign it ──
echo ""
echo "Step 5: Creating and signing update bundle..."
cd "$PROJECT_DIR"
COPYFILE_DISABLE=1 tar -czf "target/release/bundle/macos/K2.app.tar.gz" \
    -C "target/release/bundle/macos" "K2.app"

# Sign the update bundle with Tauri updater key
bunx @tauri-apps/cli@2 signer sign \
    "target/release/bundle/macos/K2.app.tar.gz" \
    --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
    --password "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
echo "  Update bundle signed."

# ── Step 6: Create DMG from notarized app ──
echo ""
echo "Step 6: Creating DMG..."
rm -f "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
# Stage the notarized app NEXT TO an /Applications symlink so the mounted
# DMG shows the standard "drag K2 → Applications" drop target. A bare
# `-srcfolder K2.app` packs only the app, so users see no Applications
# folder to drag into (the symlink tauri's own DMG ships gets dropped when
# we recreate the DMG here from the stapled app). `ditto` preserves the
# app's signature + stapled notarization ticket.
DMG_STAGE="$(mktemp -d)"
ditto "target/release/bundle/macos/K2.app" "$DMG_STAGE/K2.app"
# Styled DMG (background art + icon layout from tauri.conf.json) via tauri's
# own vendored create-dmg script, which `tauri build` leaves behind. Plain
# hdiutil ships a bare white window — the styling lives in the volume's
# .DS_Store, which only this script writes. Values mirror tauri.conf.json
# bundle.macOS.dmg (window 660x400, app 180,170, drop-link 480,170).
# Fallback: bare hdiutil + symlink (script missing / headless session where
# the Finder AppleScript pass can't run).
BUNDLE_DMG="target/release/bundle/dmg/bundle_dmg.sh"
DMG_OUT="target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
if [ -x "$BUNDLE_DMG" ] && bash "$BUNDLE_DMG" \
    --volname "K2" \
    --background "src-tauri/icons/dmg-background.png" \
    --window-size 660 400 \
    --icon-size 100 \
    --icon "K2.app" 180 170 \
    --app-drop-link 480 170 \
    --hide-extension "K2.app" \
    "$DMG_OUT" "$DMG_STAGE"; then
    echo "  Styled DMG created (background + layout)."
else
    echo "  WARN: styled DMG failed or bundle_dmg.sh missing — falling back to plain hdiutil."
    ln -s /Applications "$DMG_STAGE/Applications"
    hdiutil create -volname "K2" \
        -srcfolder "$DMG_STAGE" \
        -ov -format UDZO \
        "$DMG_OUT"
fi
rm -rf "$DMG_STAGE"
codesign --force --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"

# ── Step 7: Notarize DMG ──
echo ""
echo "Step 7: Notarizing DMG..."
xcrun notarytool submit "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg" \
    "${NOTARY_AUTH[@]}" --wait
staple_with_retry "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
echo "  DMG notarized and stapled."

# ── Step 8: Generate latest.json ──
echo ""
echo "Step 8: Generating latest.json..."
SIG_CONTENT=""
SIG_FILE="target/release/bundle/macos/K2.app.tar.gz.sig"
if [ -f "$SIG_FILE" ]; then
    SIG_CONTENT=$(cat "$SIG_FILE")
fi

PUB_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
cat > "/tmp/latest.json" <<MANIFEST
{
  "version": "${VERSION}",
  "notes": "K2 ${TAG}",
  "pub_date": "${PUB_DATE}",
  "platforms": {
    "darwin-aarch64": {
      "signature": "${SIG_CONTENT}",
      "url": "https://github.com/${RELEASE_REPO}/releases/download/${TAG}/K2.app.tar.gz"
    }
  }
}
MANIFEST
echo "  latest.json generated."

# ── Step 8.5: Standalone per-OS daemon binary + signature + manifest ──
#
# Remote-update P1: publish a STANDALONE k2-daemon binary (no .app
# wrapper) so headless servers and the daemon self-update path (P3) can
# fetch + verify + install the daemon on its own.
#
# This step produces the NATIVE macos-aarch64 standalone daemon, signs it
# with the SAME minisign key the Tauri updater uses (so P3 verifies it
# against `plugins.updater.pubkey` in tauri.conf.json), computes sha256,
# and emits `daemon-latest.json` (schema below).
#
# Linux binaries are NOT cross-built here — clang/ggml cross-compilation on
# macOS is the wrong tool. The `.github/workflows/daemon-binaries.yml`
# workflow builds + signs the linux-x86_64 / linux-aarch64 artifacts on
# native ubuntu runners and uploads them to this same release. If those CI
# artifacts have already been fetched into target/release/daemon-dist/ (by
# name: k2-daemon-linux-x86_64{,.sig}, k2-daemon-linux-aarch64{,.sig}),
# this step merges them into the manifest; otherwise it emits a macos-only
# manifest (documented — P2/P3 treat a missing artifact key as "no build
# for that platform yet").
#
# daemon-latest.json schema (P2/P3 consume this):
#   {
#     "version":  "<x.y.z>",
#     "pub_date": "<iso8601 UTC>",
#     "artifacts": {
#       "<platform-key>": {
#         "url":    "https://github.com/.../releases/download/v<ver>/<asset>",
#         "sig":    "<URL to the .sig asset>",
#         "sha256": "<hex>"
#       }, ...
#     }
#   }
# platform-key ∈ { macos-aarch64, linux-x86_64, linux-aarch64 }.
echo ""
echo "Step 8.5: Building + signing standalone daemon + daemon-latest.json..."

DIST_DIR="$PROJECT_DIR/target/release/daemon-dist"
mkdir -p "$DIST_DIR"

# The native standalone daemon is the SAME binary already built in Step 2.5
# (target/release/k2-daemon). Publish it under its platform-stamped name.
MAC_ASSET="k2-daemon-macos-aarch64"
cp "$PROJECT_DIR/target/release/k2-daemon" "$DIST_DIR/$MAC_ASSET"

# Sign with the Tauri updater key (minisign-format .sig, identical
# mechanism to Step 5's K2.app.tar.gz.sig).
bunx @tauri-apps/cli@2 signer sign \
    "$DIST_DIR/$MAC_ASSET" \
    --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
    --password "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
# tauri signer writes the .sig as BASE64 of the minisig text; the daemon
# self-updater and plain minisign need the RAW text — publish that.
# (Shape B e2e finding, 2026-07-06. NOTE: K2.app.tar.gz.sig stays wrapped —
# the Tauri APP updater expects that format; only the DAEMON sig is raw.)
if ! head -c 17 "$DIST_DIR/$MAC_ASSET.sig" | grep -q "untrusted comment"; then
    base64 -d < "$DIST_DIR/$MAC_ASSET.sig" > "$DIST_DIR/$MAC_ASSET.sig.raw" \
        || base64 -D < "$DIST_DIR/$MAC_ASSET.sig" > "$DIST_DIR/$MAC_ASSET.sig.raw"
    head -c 17 "$DIST_DIR/$MAC_ASSET.sig.raw" | grep -q "untrusted comment" \
        || { echo "ERROR: daemon .sig decode produced non-minisig content" >&2; exit 1; }
    mv "$DIST_DIR/$MAC_ASSET.sig.raw" "$DIST_DIR/$MAC_ASSET.sig"
fi
MAC_SHA256=$(shasum -a 256 "$DIST_DIR/$MAC_ASSET" | awk '{print $1}')
echo "  macos-aarch64 daemon built, signed, hashed."

DL_BASE="https://github.com/${RELEASE_REPO}/releases/download/${TAG}"

# Build the artifacts object incrementally. macos-aarch64 is always present.
ARTIFACTS_JSON=$(cat <<JSON
    "macos-aarch64": {
      "url": "${DL_BASE}/${MAC_ASSET}",
      "sig": "${DL_BASE}/${MAC_ASSET}.sig",
      "sha256": "${MAC_SHA256}"
    }
JSON
)

# Merge any CI-produced Linux artifacts present in $DIST_DIR.
for LX in "linux-x86_64" "linux-aarch64"; do
    LX_ASSET="k2-daemon-${LX}"
    if [ -f "$DIST_DIR/$LX_ASSET" ] && [ -f "$DIST_DIR/${LX_ASSET}.sig" ]; then
        LX_SHA256=$(shasum -a 256 "$DIST_DIR/$LX_ASSET" | awk '{print $1}')
        ARTIFACTS_JSON="${ARTIFACTS_JSON},
    \"${LX}\": {
      \"url\": \"${DL_BASE}/${LX_ASSET}\",
      \"sig\": \"${DL_BASE}/${LX_ASSET}.sig\",
      \"sha256\": \"${LX_SHA256}\"
    }"
        echo "  Merged CI artifact: ${LX_ASSET}."
    else
        echo "  (no CI artifact for ${LX} — left out; CI workflow uploads it separately)"
    fi
done

cat > "$DIST_DIR/daemon-latest.json" <<MANIFEST
{
  "version": "${VERSION}",
  "pub_date": "${PUB_DATE}",
  "artifacts": {
${ARTIFACTS_JSON}
  }
}
MANIFEST
echo "  daemon-latest.json generated at $DIST_DIR/daemon-latest.json"

# ── Step 8.6: Publish hosted-web SPA to Cloudflare R2 ──
#
# Additive, versioned layout: app/<ver>/index.html + content-hashed assets.
# Separate artifact from the DMG — desktop release must not die on R2 TLS
# warmup (brand-new endpoint) or missing laptop creds. Fail only when
# K2_REQUIRE_WEB_PUBLISH=1. See scripts/publish-web-bundles.sh header.
echo ""
echo "Step 8.6: Publishing web SPA bundle to R2 (app/${VERSION}/)..."
WEB_PUBLISH_ARGS=("$VERSION" --skip-if-no-creds)
if [ "${K2_WEB_BUNDLE_PRUNE:-0}" = "1" ]; then
    WEB_PUBLISH_ARGS+=(--prune)
fi
set +e
"$PROJECT_DIR/scripts/publish-web-bundles.sh" "${WEB_PUBLISH_ARGS[@]}"
WEB_PUBLISH_RC=$?
set -e
if [ "$WEB_PUBLISH_RC" -eq 0 ]; then
    echo "  Web-bundle publish step finished (see messages above)."
else
    if [ "${K2_REQUIRE_WEB_PUBLISH:-0}" = "1" ]; then
        echo "  FATAL: web-bundle publish failed (rc=$WEB_PUBLISH_RC) and K2_REQUIRE_WEB_PUBLISH=1." >&2
        exit 1
    fi
    echo "  WARNING: web-bundle publish failed (rc=$WEB_PUBLISH_RC) — continuing desktop release." >&2
    echo "  WARNING: re-run: bash scripts/publish-web-bundles.sh ${VERSION}" >&2
    echo "  WARNING: set K2_REQUIRE_WEB_PUBLISH=1 to make this a hard gate." >&2
fi

# ── Step 9: Create GitHub Release ──
echo ""
echo "Step 9: Creating GitHub release ${TAG}..."
ASSETS=(
    "target/release/bundle/dmg/K2_${VERSION}_aarch64.dmg"
    "target/release/bundle/macos/K2.app.tar.gz"
)
[ -f "$SIG_FILE" ] && ASSETS+=("$SIG_FILE")
ASSETS+=("/tmp/latest.json")

# Standalone daemon assets (remote-update P1): the native macos-aarch64
# binary + its .sig, plus the daemon manifest. Any CI-fetched Linux
# binaries in $DIST_DIR are uploaded too (the daemon-binaries.yml workflow
# normally uploads those directly to the release on its own, but if they
# were staged here first we attach them in the same `gh release create`).
ASSETS+=(
    "$DIST_DIR/${MAC_ASSET}"
    "$DIST_DIR/${MAC_ASSET}.sig"
    "$DIST_DIR/daemon-latest.json"
)
for LX in "linux-x86_64" "linux-aarch64"; do
    LX_ASSET="k2-daemon-${LX}"
    if [ -f "$DIST_DIR/$LX_ASSET" ] && [ -f "$DIST_DIR/${LX_ASSET}.sig" ]; then
        ASSETS+=("$DIST_DIR/$LX_ASSET" "$DIST_DIR/${LX_ASSET}.sig")
    fi
done

if [ -n "$NOTES_FILE" ] && [ -f "$NOTES_FILE" ]; then
    NOTES_SRC="$NOTES_FILE"
else
    # No explicit notes file → auto-extract this version's WHATS_NEW.md
    # section (the same `## <version>` block Step 1.5 already verified
    # exists) and use it as the GitHub release body. The public
    # "What's New" site mirrors the GH release body, so leaving a
    # placeholder here silently empties the site — this makes the notes
    # impossible to forget.
    NOTES_SRC="$(mktemp -t k2-relnotes)"
    awk -v ver="$VERSION" '
        $0 ~ "^## " ver " " { inblock = 1; print; next }
        inblock && /^## / { exit }
        inblock { print }
    ' WHATS_NEW.md > "$NOTES_SRC"
    echo "  Using WHATS_NEW.md '## ${VERSION}' section as the release body."
fi
# ── Step 8.9: Commit + push the version bump, then create + push the tag ──
# Historically release.sh bumped the version files only to BUILD the installers
# and never committed them, so `main` lagged at the prior version after every
# release and `gh release create` below tagged origin's stale HEAD (the tag
# pointed at the wrong commit). Commit the bump, push main, and create + push
# the annotated tag HERE — so the GitHub release attaches to the REAL release
# commit and the repo's committed version always matches the published release.
# Runs only after a fully built + notarized release, so a failed build never
# leaves a stray commit/tag. A push failure (e.g. diverged origin) aborts
# BEFORE publishing — safer than the old after-the-fact state.
echo ""
echo "Step 8.9: Committing version bump + pushing tag ${TAG}..."
git add package.json src-tauri/tauri.conf.json cli/k2 Cargo.lock \
    src-tauri/Cargo.toml crates/k2-core/Cargo.toml crates/k2-daemon/Cargo.toml \
    WHATS_NEW.md
if git diff --cached --quiet; then
    echo "  (version files already committed — nothing new to commit)"
else
    git commit -m "chore(release): ${TAG}"
    echo "  Committed version bump."
fi
# HEAD:main, not bare HEAD — release worktrees run DETACHED (worktrees
# can't check out a branch already held by the main checkout), and a
# bare `git push origin HEAD` fails there ("not a full refname") AFTER
# the build/sign/notarize already succeeded (v0.40.39 hit this; the
# tail had to be run by hand).
git push origin HEAD:main
git tag -fa "$TAG" -m "K2 ${TAG}"
git push origin "refs/tags/${TAG}" --force
echo "  Pushed main + tag ${TAG} at $(git rev-parse --short HEAD)."

# ── Step 9: Create the GitHub release (attaches to the tag pushed above) ──
gh release create "$TAG" "${ASSETS[@]}" \
    --repo "$RELEASE_REPO" \
    --title "$TAG" \
    --notes-file "$NOTES_SRC"

# ── Step 10: Verify the updater can actually FETCH valid JSON ──
# We GENERATE latest.json + daemon-latest.json above, but a release is only
# good if the updater's real endpoints serve valid JSON. The app updater +
# the daemon self-update both fetch via GitHub's
# `/releases/latest/download/<name>` alias — which can lag for a few seconds
# after publish, and the asset CDN occasionally 504s. So retry, then FAIL
# LOUDLY (the release is already live) rather than let a broken updater pass
# silently. Validates HTTP body actually contains `"version": "<VERSION>"`
# (a 504 HTML page or a stale manifest both fail this).
echo ""
echo "Step 10: Verifying updater endpoints serve valid v${VERSION} JSON..."
VERIFY_BASE="https://github.com/${RELEASE_REPO}/releases/latest/download"
VERIFY_FAILED=""
verify_manifest() {
    local label="$1" url="$2" ok="" body=""
    for attempt in $(seq 1 8); do
        body="$(curl -sL --max-time 20 "$url" 2>/dev/null)"
        if printf '%s' "$body" | grep -q "\"version\"[[:space:]]*:[[:space:]]*\"${VERSION}\""; then
            ok=1; break
        fi
        echo "    ${label}: not ready (attempt ${attempt}/8), retrying in 15s..."
        sleep 15
    done
    if [ -n "$ok" ]; then
        echo "  ✓ ${label} → valid JSON, version ${VERSION}"
    else
        echo "  ✗ ${label} did NOT serve valid v${VERSION} JSON: ${url}" >&2
        echo "     (release IS published, but the in-app/daemon updater will fail until this clears —" >&2
        echo "      usually a transient GitHub 504 / latest-alias lag; re-run the check or wait a minute)" >&2
        VERIFY_FAILED=1
    fi
}
verify_manifest "app latest.json"    "${VERIFY_BASE}/latest.json"
verify_manifest "daemon-latest.json" "${VERIFY_BASE}/daemon-latest.json"

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Release ${TAG} complete!"
echo "  https://github.com/${RELEASE_REPO}/releases/tag/${TAG}"
[ -n "$VERIFY_FAILED" ] && echo "  ⚠  UPDATER MANIFEST CHECK FAILED — see Step 10 above (updater may 'Could not fetch a valid release JSON' until it clears)"
echo "═══════════════════════════════════════════════════"
