#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd -P)"
SCRIPT="$ROOT/scripts/build-unsigned-x86_64-test.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/k2-unsigned-x86-test.XXXXXX")"
if [ "${K2_KEEP_TEST_TMP:-0}" != 1 ]; then
    trap 'rm -rf "$TMP"' EXIT
else
    echo "preserving fake-tool evidence at $TMP"
fi

FAKE_BIN="$TMP/bin"
SOURCE="$TMP/source"
LOG="$TMP/tools.log"
mkdir -p "$FAKE_BIN" "$SOURCE/src-tauri" "$SOURCE/crates/k2-daemon" \
    "$SOURCE/crates/k2-menubar/src" "$SOURCE/cli"

git -C "$ROOT" show 655fe8bbbc5bbd259cabb77ea714f3bf092db2c3:crates/k2-menubar/src/main.rs \
    > "$SOURCE/crates/k2-menubar/src/main.rs"

cat > "$SOURCE/package.json" <<'EOF'
{"version": "0.41.3"}
EOF
printf 'version = 3\n' > "$SOURCE/Cargo.lock"
cat > "$SOURCE/src-tauri/tauri.conf.json" <<'EOF'
{"version": "0.41.3"}
EOF
cat > "$SOURCE/src-tauri/Cargo.toml" <<'EOF'
[package]
version = "0.41.3"
EOF
cat > "$SOURCE/crates/k2-daemon/Cargo.toml" <<'EOF'
[package]
version = "0.41.3"
EOF
cat > "$SOURCE/crates/k2-menubar/Cargo.toml" <<'EOF'
[package]
version = "0.41.3"
EOF
cat > "$SOURCE/cli/k2" <<'EOF'
#!/bin/sh
K2_CLI_VERSION="0.41.3"
echo "k2 ${FAKE_CLI_VERSION:-$K2_CLI_VERSION}"
EOF
chmod 755 "$SOURCE/cli/k2"

cat > "$FAKE_BIN/git" <<'EOF'
#!/bin/sh
case "$1:$2" in
    cat-file:-t) echo tag ;;
    rev-parse:5a9ad6880d6c09e95eb452c5c218451a724c24cf^\{\})
        echo "${FAKE_COMMIT:-655fe8bbbc5bbd259cabb77ea714f3bf092db2c3}" ;;
    rev-parse:655fe8bbbc5bbd259cabb77ea714f3bf092db2c3^\{tree\})
        echo "${FAKE_TREE:-ff9cf6a5ef9eaffd739e43dd207ea4e9db575f32}" ;;
    rev-parse:617655ee0b015bd9419b35c7d9779cdc24069781^\{tree\})
        echo "${FAKE_OVERLAY_TREE:-243aabb4402e2edeb6b2254cdc8dc8fe340e97cb}" ;;
    show:655fe8bbbc5bbd259cabb77ea714f3bf092db2c3:crates/k2-menubar/src/main.rs)
        if [ "${FAKE_BAD_MENUBAR_PREIMAGE:-0}" = 1 ]; then
            echo wrong
        else
            cat "$FAKE_SOURCE/crates/k2-menubar/src/main.rs"
        fi ;;
    archive:655fe8bbbc5bbd259cabb77ea714f3bf092db2c3)
        /usr/bin/tar -cf - -C "$FAKE_SOURCE" . ;;
    *) echo "unexpected fake git call: $*" >&2; exit 90 ;;
esac
EOF

cat > "$FAKE_BIN/bun" <<'EOF'
#!/bin/sh
echo "bun $*" >> "$FAKE_LOG"
[ "$*" = "run tauri build --target x86_64-apple-darwin --bundles app" ] || exit 92
[ "${CARGO_NET_OFFLINE:-}" = true ] || exit 91
[ "${FAKE_MUTATE_LOCK:-0}" != 1 ] || echo changed >> Cargo.lock
app="$CARGO_TARGET_DIR/x86_64-apple-darwin/release/bundle/macos/K2.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/_up_/cli"
printf 'fake app\n' > "$app/Contents/MacOS/k2"
cp "$FRPC_SRC" "$app/Contents/MacOS/frpc"
cat > "$app/Contents/Info.plist" <<'PLIST'
<plist><dict><key>CFBundleShortVersionString</key><string>0.41.3</string></dict></plist>
PLIST
cat > "$app/Contents/Resources/_up_/cli/k2" <<'CLI'
#!/bin/sh
echo "k2 ${FAKE_CLI_VERSION:-0.41.3}"
CLI
chmod 755 "$app/Contents/MacOS/k2" "$app/Contents/MacOS/frpc" \
    "$app/Contents/Resources/_up_/cli/k2"
EOF

cat > "$FAKE_BIN/cargo" <<'EOF'
#!/bin/sh
echo "cargo $*" >> "$FAKE_LOG"
[ "$*" = "build --locked --offline --release --target x86_64-apple-darwin -p k2-daemon -p k2-menubar" ] || exit 93
grep -Fq 'fn version_output' crates/k2-menubar/src/main.rs || {
    echo "missing version seam in archived build source" >&2
    exit 95
}
bin="$CARGO_TARGET_DIR/x86_64-apple-darwin/release"
mkdir -p "$bin"
cat > "$bin/k2-daemon" <<DAEMON
#!/bin/sh
echo "k2-daemon ${FAKE_DAEMON_VERSION:-0.41.3}"
DAEMON
cat > "$bin/k2-menubar" <<MENUBAR
#!/bin/sh
echo "k2-menubar ${FAKE_MENUBAR_VERSION:-0.41.3}"
MENUBAR
chmod 755 "$bin/k2-daemon" "$bin/k2-menubar"
EOF

cat > "$FAKE_BIN/lipo" <<'EOF'
#!/bin/sh
echo "${FAKE_ARCH:-x86_64}"
EOF
cat > "$FAKE_BIN/plutil" <<'EOF'
#!/bin/sh
echo "${FAKE_APP_VERSION:-0.41.3}"
EOF
cat > "$FAKE_BIN/codesign" <<'EOF'
#!/bin/sh
if [ "${FAKE_AUTHORITY:-0}" = 1 ]; then
    echo "Authority=Developer ID Application: test" >&2
else
    echo "Signature=adhoc" >&2
fi
EOF
cat > "$FAKE_BIN/rustup" <<'EOF'
#!/bin/sh
[ "$*" = "target list --installed" ] || exit 94
echo "${FAKE_RUST_TARGET:-x86_64-apple-darwin}"
EOF
chmod 755 "$FAKE_BIN"/*

FRPC="$TMP/frpc"
printf '#!/bin/sh\nexit 0\n' > "$FRPC"
chmod 755 "$FRPC"
NODE_MODULES="$TMP/node_modules"
mkdir -p "$NODE_MODULES/.bin" "$NODE_MODULES/@tauri-apps/cli"
printf '#!/bin/sh\nexit 0\n' > "$NODE_MODULES/.bin/tauri"
chmod 755 "$NODE_MODULES/.bin/tauri"
printf '{"version": "2.12.0"}\n' > "$NODE_MODULES/@tauri-apps/cli/package.json"

export PATH="$FAKE_BIN:/usr/bin:/bin"
export FAKE_SOURCE="$SOURCE" FAKE_LOG="$LOG"

expect_fail() {
    local label="$1"; shift
    if "$@" >"$TMP/$label.out" 2>"$TMP/$label.err"; then
        echo "FAIL: $label unexpectedly succeeded" >&2
        exit 1
    fi
}

OUT_OK="$TMP/out-ok"
"$SCRIPT" --output "$OUT_OK" --frpc "$FRPC" --node-modules "$NODE_MODULES" >"$TMP/happy.out"
grep -Fq 'VERIFIED TEST BUILD ONLY:' "$TMP/happy.out"
grep -Fq 'source_commit 655fe8bbbc5bbd259cabb77ea714f3bf092db2c3' "$OUT_OK/UNSIGNED_TEST_MANIFEST.txt"
grep -Fq 'source_tree ff9cf6a5ef9eaffd739e43dd207ea4e9db575f32' "$OUT_OK/UNSIGNED_TEST_MANIFEST.txt"
grep -Fq 'menubar_overlay_commit 617655ee0b015bd9419b35c7d9779cdc24069781' "$OUT_OK/UNSIGNED_TEST_MANIFEST.txt"
grep -Fq 'menubar_overlay_tree 243aabb4402e2edeb6b2254cdc8dc8fe340e97cb' "$OUT_OK/UNSIGNED_TEST_MANIFEST.txt"
grep -Fq 'menubar_main_base_sha256 b6a2473f2e3966450228b9c2e28371749a799efedebb2752ced33c8eb717d61d' "$OUT_OK/UNSIGNED_TEST_MANIFEST.txt"
grep -Fq 'menubar_main_overlay_sha256 187079fb19f35c3648e176dd7c219f8f61d9796edff06aad4c3d0b6e7adad631' "$OUT_OK/UNSIGNED_TEST_MANIFEST.txt"
grep -Fq 'target x86_64-apple-darwin' "$OUT_OK/UNSIGNED_TEST_MANIFEST.txt"
grep -Fxq 'bun run tauri build --target x86_64-apple-darwin --bundles app' "$LOG"
grep -Fxq 'cargo build --locked --offline --release --target x86_64-apple-darwin -p k2-daemon -p k2-menubar' "$LOG"

mkdir "$TMP/existing"
expect_fail no-clobber "$SCRIPT" --output "$TMP/existing" --frpc "$FRPC" --node-modules "$NODE_MODULES"

FAKE_TREE=wrong expect_fail wrong-tree "$SCRIPT" --output "$TMP/out-wrong-tree" --frpc "$FRPC" --node-modules "$NODE_MODULES"
FAKE_OVERLAY_TREE=wrong expect_fail wrong-overlay-tree "$SCRIPT" --output "$TMP/out-wrong-overlay-tree" --frpc "$FRPC" --node-modules "$NODE_MODULES"
FAKE_BAD_MENUBAR_PREIMAGE=1 expect_fail wrong-menubar-preimage "$SCRIPT" --output "$TMP/out-wrong-menubar-preimage" --frpc "$FRPC" --node-modules "$NODE_MODULES"
FAKE_RUST_TARGET=aarch64-apple-darwin expect_fail missing-target "$SCRIPT" --output "$TMP/out-missing-target" --frpc "$FRPC" --node-modules "$NODE_MODULES"
FAKE_MUTATE_LOCK=1 expect_fail changed-lock "$SCRIPT" --output "$TMP/out-changed-lock" --frpc "$FRPC" --node-modules "$NODE_MODULES"
: > "$LOG"
FAKE_ARCH=arm64 expect_fail wrong-arch "$SCRIPT" --output "$TMP/out-wrong-arch" --frpc "$FRPC" --node-modules "$NODE_MODULES"
[ ! -s "$LOG" ] || { echo "FAIL: wrong-arch invoked a build tool" >&2; exit 1; }
[ ! -e "$TMP/out-wrong-arch" ] || { echo "FAIL: wrong-arch created output" >&2; exit 1; }
FAKE_DAEMON_VERSION=0.41.2 expect_fail wrong-daemon "$SCRIPT" --output "$TMP/out-wrong-daemon" --frpc "$FRPC" --node-modules "$NODE_MODULES"
sed -i '' 's/version = "0.41.3"/version = "0.41.2"/' "$SOURCE/crates/k2-menubar/Cargo.toml"
expect_fail wrong-menubar-source "$SCRIPT" --output "$TMP/out-wrong-menubar-source" --frpc "$FRPC" --node-modules "$NODE_MODULES"
sed -i '' 's/version = "0.41.2"/version = "0.41.3"/' "$SOURCE/crates/k2-menubar/Cargo.toml"
FAKE_MENUBAR_VERSION=0.41.2 expect_fail wrong-menubar-artifact "$SCRIPT" --output "$TMP/out-wrong-menubar-artifact" --frpc "$FRPC" --node-modules "$NODE_MODULES"
FAKE_CLI_VERSION=0.41.2 expect_fail wrong-cli "$SCRIPT" --output "$TMP/out-wrong-cli" --frpc "$FRPC" --node-modules "$NODE_MODULES"
FAKE_AUTHORITY=1 expect_fail signed "$SCRIPT" --output "$TMP/out-signed" --frpc "$FRPC" --node-modules "$NODE_MODULES"

if grep -Eq 'bun install|codesign .*--sign|notarytool|build-app\.sh|release\.sh|/Applications|launchctl|[[:space:]]open[[:space:]]' "$SCRIPT"; then
    echo "FAIL: unsigned-test script contains a shipping/install/launch action" >&2
    exit 1
fi

echo "PASS: exact-source Intel unsigned-test route and predecessor-discriminating failures"
