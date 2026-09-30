#!/bin/bash
# Build an exact v0.41.3 Intel test app without release credentials or shared output.
# This is not a release path: no Developer ID signing, notarization, updater bundle,
# install, launch, service action, upload, or publish occurs here.
set -euo pipefail

TAG_OBJECT="5a9ad6880d6c09e95eb452c5c218451a724c24cf"
SOURCE_COMMIT="655fe8bbbc5bbd259cabb77ea714f3bf092db2c3"
SOURCE_TREE="ff9cf6a5ef9eaffd739e43dd207ea4e9db575f32"
MENUBAR_OVERLAY_COMMIT="617655ee0b015bd9419b35c7d9779cdc24069781"
MENUBAR_OVERLAY_TREE="243aabb4402e2edeb6b2254cdc8dc8fe340e97cb"
MENUBAR_MAIN_BASE_SHA256="b6a2473f2e3966450228b9c2e28371749a799efedebb2752ced33c8eb717d61d"
MENUBAR_MAIN_OVERLAY_SHA256="187079fb19f35c3648e176dd7c219f8f61d9796edff06aad4c3d0b6e7adad631"
VERSION="0.41.3"
TARGET="x86_64-apple-darwin"

usage() {
    echo "Usage: $0 --output NEW-DIRECTORY --frpc /path/to/x86_64/frpc --node-modules /path/to/node_modules" >&2
}

OUTPUT=""
FRPC=""
NODE_MODULES=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) OUTPUT="${2:-}"; shift 2 ;;
        --frpc) FRPC="${2:-}"; shift 2 ;;
        --node-modules) NODE_MODULES="${2:-}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage; exit 2 ;;
    esac
done

[ -n "$OUTPUT" ] && [ -n "$FRPC" ] && [ -n "$NODE_MODULES" ] || { usage; exit 2; }
[ -d "$(dirname "$OUTPUT")" ] || { echo "FATAL: output parent does not exist" >&2; exit 1; }
OUTPUT="$(cd "$(dirname "$OUTPUT")" && pwd -P)/$(basename "$OUTPUT")"
[ -d "$(dirname "$FRPC")" ] || { echo "FATAL: --frpc parent does not exist" >&2; exit 1; }
FRPC="$(cd "$(dirname "$FRPC")" && pwd -P)/$(basename "$FRPC")"
[ -d "$NODE_MODULES" ] || { echo "FATAL: --node-modules directory does not exist" >&2; exit 1; }
NODE_MODULES="$(cd "$NODE_MODULES" && pwd -P)"
[ ! -e "$OUTPUT" ] || { echo "FATAL: output already exists (no clobber): $OUTPUT" >&2; exit 1; }
[ -x "$FRPC" ] || { echo "FATAL: --frpc must name an executable x86_64 frpc" >&2; exit 1; }
[ -x "$NODE_MODULES/.bin/tauri" ] || { echo "FATAL: --node-modules lacks an installed Tauri CLI" >&2; exit 1; }
grep -Fq '"version": "2.12.0"' "$NODE_MODULES/@tauri-apps/cli/package.json" || {
    echo "FATAL: --node-modules does not contain Tauri CLI 2.12.0" >&2; exit 1;
}

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd -P)"
case "$OUTPUT/" in
    "$PROJECT_DIR/"*) echo "FATAL: output must be outside the source checkout" >&2; exit 1 ;;
esac
cd "$PROJECT_DIR"

[ "$(git cat-file -t "$TAG_OBJECT")" = tag ] || { echo "FATAL: expected annotated tag object is absent" >&2; exit 1; }
[ "$(git rev-parse "$TAG_OBJECT^{}")" = "$SOURCE_COMMIT" ] || { echo "FATAL: tag does not peel to the pinned source commit" >&2; exit 1; }
[ "$(git rev-parse "$SOURCE_COMMIT^{tree}")" = "$SOURCE_TREE" ] || { echo "FATAL: pinned commit tree does not match" >&2; exit 1; }
[ "$(git rev-parse "$MENUBAR_OVERLAY_COMMIT^{tree}")" = "$MENUBAR_OVERLAY_TREE" ] || { echo "FATAL: menubar overlay commit tree does not match" >&2; exit 1; }

for tool in bun cargo rustup lipo plutil codesign shasum tar cp; do
    command -v "$tool" >/dev/null 2>&1 || { echo "FATAL: required local tool missing: $tool" >&2; exit 1; }
done
rustup target list --installed | grep -Fxq "$TARGET" || {
    echo "FATAL: Rust target $TARGET is not installed; refusing to fetch it" >&2; exit 1;
}
[ "$(lipo -archs "$FRPC")" = x86_64 ] || {
    echo "FATAL: --frpc is not thin x86_64" >&2; exit 1;
}
MENUBAR_MAIN="crates/k2-menubar/src/main.rs"
[ "$(git show "$SOURCE_COMMIT:$MENUBAR_MAIN" | shasum -a 256 | awk '{print $1}')" = "$MENUBAR_MAIN_BASE_SHA256" ] || {
    echo "FATAL: tagged menubar preimage does not match" >&2; exit 1;
}
[ "$(shasum -a 256 "$MENUBAR_MAIN" | awk '{print $1}')" = "$MENUBAR_MAIN_OVERLAY_SHA256" ] || {
    echo "FATAL: reviewed menubar overlay does not match" >&2; exit 1;
}

mkdir "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd -P)"
SOURCE="$OUTPUT/source"
mkdir "$SOURCE"
git archive "$SOURCE_COMMIT" | tar -x -C "$SOURCE"
[ "$(shasum -a 256 "$SOURCE/$MENUBAR_MAIN" | awk '{print $1}')" = "$MENUBAR_MAIN_BASE_SHA256" ] || {
    echo "FATAL: archived menubar preimage does not match" >&2; exit 1;
}
cp "$PROJECT_DIR/$MENUBAR_MAIN" "$SOURCE/$MENUBAR_MAIN"
[ "$(shasum -a 256 "$SOURCE/$MENUBAR_MAIN" | awk '{print $1}')" = "$MENUBAR_MAIN_OVERLAY_SHA256" ] || {
    echo "FATAL: archived menubar overlay does not match" >&2; exit 1;
}
ln -s "$NODE_MODULES" "$SOURCE/node_modules"
cd "$SOURCE"

grep -Fq '"version": "0.41.3"' package.json || { echo "FATAL: package version is not $VERSION" >&2; exit 1; }
grep -Fq '"version": "0.41.3"' src-tauri/tauri.conf.json || { echo "FATAL: Tauri version is not $VERSION" >&2; exit 1; }
grep -Eq '^version = "0\.41\.3"$' src-tauri/Cargo.toml || { echo "FATAL: app crate version is not $VERSION" >&2; exit 1; }
grep -Eq '^version = "0\.41\.3"$' crates/k2-daemon/Cargo.toml || { echo "FATAL: daemon crate version is not $VERSION" >&2; exit 1; }
grep -Eq '^version = "0\.41\.3"$' crates/k2-menubar/Cargo.toml || { echo "FATAL: menubar crate version is not $VERSION" >&2; exit 1; }
grep -Fq 'K2_CLI_VERSION="0.41.3"' cli/k2 || { echo "FATAL: CLI version is not $VERSION" >&2; exit 1; }
LOCK_SHA256="$(shasum -a 256 Cargo.lock | awk '{print $1}')"

export CARGO_TARGET_DIR="$OUTPUT/target"
export CARGO_NET_OFFLINE=true
export FRPC_SRC="$FRPC"
export FRPC_TARGET_TRIPLE="$TARGET"
unset APPLE_SIGNING_IDENTITY APPLE_TEAM_ID
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD
unset K2_GMAIL_CLIENT_ID K2_GMAIL_CLIENT_SECRET
unset K2_MICROSOFT_CLIENT_ID K2_MICROSOFT_CLIENT_SECRET

echo "Building thin Tauri app for $TARGET..."
bun run tauri build --target "$TARGET" --bundles app
[ "$(shasum -a 256 Cargo.lock | awk '{print $1}')" = "$LOCK_SHA256" ] || {
    echo "FATAL: Tauri build changed the pinned Cargo.lock" >&2; exit 1;
}

echo "Building embedded helpers for $TARGET..."
cargo build --locked --offline --release --target "$TARGET" -p k2-daemon -p k2-menubar

APP="$CARGO_TARGET_DIR/$TARGET/release/bundle/macos/K2.app"
BIN_DIR="$CARGO_TARGET_DIR/$TARGET/release"
[ -d "$APP" ] || { echo "FATAL: Tauri app not found at $APP" >&2; exit 1; }
[ -x "$BIN_DIR/k2-daemon" ] || { echo "FATAL: built daemon missing" >&2; exit 1; }
[ -x "$BIN_DIR/k2-menubar" ] || { echo "FATAL: built menubar helper missing" >&2; exit 1; }

cp "$BIN_DIR/k2-daemon" "$APP/Contents/MacOS/k2-daemon"
cp "$BIN_DIR/k2-menubar" "$APP/Contents/MacOS/k2-menubar"

APP_BIN="$APP/Contents/MacOS/k2"
DAEMON="$APP/Contents/MacOS/k2-daemon"
MENUBAR="$APP/Contents/MacOS/k2-menubar"
FRPC_BIN="$APP/Contents/MacOS/frpc"
CLI="$APP/Contents/Resources/_up_/cli/k2"
for binary in "$APP_BIN" "$DAEMON" "$MENUBAR" "$FRPC_BIN"; do
    [ -x "$binary" ] || { echo "FATAL: required embedded binary missing: $binary" >&2; exit 1; }
    [ "$(lipo -archs "$binary")" = x86_64 ] || { echo "FATAL: embedded binary is not thin x86_64: $binary" >&2; exit 1; }
done

[ "$(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist")" = "$VERSION" ] || {
    echo "FATAL: app bundle version is not $VERSION" >&2; exit 1;
}
[ "$("$DAEMON" --version)" = "k2-daemon $VERSION" ] || { echo "FATAL: embedded daemon identity mismatch" >&2; exit 1; }
[ "$("$MENUBAR" --version)" = "k2-menubar $VERSION" ] || { echo "FATAL: embedded menubar identity mismatch" >&2; exit 1; }
[ -x "$CLI" ] || { echo "FATAL: embedded CLI missing" >&2; exit 1; }
[ "$("$CLI" --version)" = "k2 $VERSION" ] || { echo "FATAL: embedded CLI identity mismatch" >&2; exit 1; }

SIGNATURE_INFO="$(codesign -dvv "$APP" 2>&1 || true)"
if printf '%s\n' "$SIGNATURE_INFO" | grep -q '^Authority='; then
    echo "FATAL: test bundle unexpectedly has an authority-backed signature" >&2
    exit 1
fi

MANIFEST="$OUTPUT/UNSIGNED_TEST_MANIFEST.txt"
{
    echo "TEST ONLY — not signed for distribution, notarized, installed, launched, or released"
    echo "tag_object $TAG_OBJECT"
    echo "source_commit $SOURCE_COMMIT"
    echo "source_tree $SOURCE_TREE"
    echo "menubar_overlay_commit $MENUBAR_OVERLAY_COMMIT"
    echo "menubar_overlay_tree $MENUBAR_OVERLAY_TREE"
    echo "menubar_main_base_sha256 $MENUBAR_MAIN_BASE_SHA256"
    echo "menubar_main_overlay_sha256 $MENUBAR_MAIN_OVERLAY_SHA256"
    echo "version $VERSION"
    echo "target $TARGET"
    echo "app $APP"
    shasum -a 256 "$APP_BIN" "$DAEMON" "$MENUBAR" "$FRPC_BIN" "$CLI"
} > "$MANIFEST"

echo "VERIFIED TEST BUILD ONLY: $APP"
echo "Manifest: $MANIFEST"
