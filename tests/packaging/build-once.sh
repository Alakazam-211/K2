#!/usr/bin/env bash
# Stubbed test for the macOS sidecar build (scripts/lib/build-binaries.sh,
# release.sh Step 2) and the Step 8.5 standalone daemon
# (k2_build_standalone_macos_daemon): every sidecar comes from ONE cargo
# call, and the standalone rebuild uses the exact same package set, so it
# reuses every compiled crate and only k2-daemon relinks. Stub `bun`,
# `cargo` and `codesign` on PATH; nothing compiles, signs or touches ~/.k2.
# Runs on macOS and Linux. No skips.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stub="$work/stub"
proj="$work/proj"
mkdir -p "$stub" "$proj"
export CALLS="$work/cargo-calls"
: > "$CALLS"

# cargo: record each call (argv + the embed env), then play cargo: every
# --bin NAME lands in <target>/release/NAME; k2-daemon carries the sha256
# of $K2_PTYD_EMBED and of $K2_EXPORT_EMBED when set (what build.rs embeds).
cat > "$stub/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\tembed=%s\texport=%s\n' "$*" "${K2_PTYD_EMBED:-}" "${K2_EXPORT_EMBED:-}" >> "$CALLS"
out="${CARGO_TARGET_DIR:-target}/release"
mkdir -p "$out"
prev=""
for a in "$@"; do
    if [ "$prev" = "--bin" ]; then
        printf 'bin %s ' "$a" > "$out/$a"
        if [ "$a" = k2-daemon ] && [ -n "${K2_PTYD_EMBED:-}" ]; then
            shasum -a 256 "$K2_PTYD_EMBED" | cut -d' ' -f1 >> "$out/$a"
        fi
        if [ "$a" = k2-daemon ] && [ -n "${K2_EXPORT_EMBED:-}" ]; then
            shasum -a 256 "$K2_EXPORT_EMBED" | cut -d' ' -f1 >> "$out/$a"
        fi
        chmod +x "$out/$a"
    fi
    prev="$a"
done
EOF
# bun run tauri build: makes the app bundle.
cat > "$stub/bun" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$*" = "run tauri build" ] || { echo "stub bun: unexpected: $*" >&2; exit 1; }
mkdir -p target/release/bundle/macos/K2.app/Contents/MacOS
printf 'app' > target/release/bundle/macos/K2.app/Contents/MacOS/k2
EOF
# codesign: the nested k2-ptyd and k2-export are validly signed
# (dev.k2.app.ptyd, dev.k2.app.export).
cat > "$stub/codesign" <<'EOF'
#!/usr/bin/env bash
case "$1" in
    --verify) exit 0 ;;
    -dv) case "$2" in *k2-export) echo "Identifier=dev.k2.app.export" >&2 ;; *) echo "Identifier=dev.k2.app.ptyd" >&2 ;; esac; exit 0 ;;
esac
exit 1
EOF
chmod +x "$stub/cargo" "$stub/bun" "$stub/codesign"
# Off macOS there is no plutil: lint the generated Info.plist with plistlib.
if ! command -v plutil >/dev/null 2>&1; then
    cat > "$stub/plutil" <<'EOF'
#!/usr/bin/env bash
[ "$1" = -lint ] || { echo "stub plutil: only -lint" >&2; exit 1; }
python3 -c 'import plistlib, sys; plistlib.load(open(sys.argv[1], "rb"))' "$2"
EOF
    chmod +x "$stub/plutil"
fi

echo "Step 2 (k2_build_mac_binaries):"
(
    cd "$proj"
    export PROJECT_DIR="$ROOT" PATH="$stub:$PATH"
    unset CARGO_TARGET_DIR SDKROOT
    # shellcheck source=scripts/lib/build-binaries.sh
    source "$ROOT/scripts/lib/build-binaries.sh"
    # plutil is macOS-only; the helper's layout is checked below instead.
    k2_daemon_helper_verify() { [ -x "$(k2_daemon_helper_exe "$1")" ]; }
    assert_daemon_oauth_not_placeholder() { [ -x "$1" ]; }
    k2_build_mac_binaries "target/release/bundle/macos/K2.app" > "$work/step2.log" 2>&1
) && pass "k2_build_mac_binaries succeeds" || fail "k2_build_mac_binaries" "$(cat "$work/step2.log")"

want="build --release -p k2-daemon --bin k2-daemon --bin k2-power-helper -p k2-ptyd --bin k2-ptyd -p k2-export --bin k2-export -p k2-node --bin k2-node -p k2-menubar --bin k2-menubar"
[ "$(wc -l < "$CALLS" | tr -d ' ')" = 1 ] && pass "one cargo call builds every sidecar" \
    || fail "cargo calls" "$(cat "$CALLS")"
[ "$(head -1 "$CALLS")" = "$want	embed=	export=" ] && pass "the call is K2_MAC_SIDECAR_CARGO_ARGS, release, no embed" \
    || fail "step 2 argv" "got: $(head -1 "$CALLS")"
app="$proj/target/release/bundle/macos/K2.app"
helper="$app/Contents/Helpers/K2 Daemon.app/Contents/MacOS"
ok=1
for f in "$helper/k2-daemon" "$helper/k2-ptyd" "$helper/k2-export" "$app/Contents/MacOS/k2-power-helper" \
    "$app/Contents/MacOS/k2-node" "$app/Contents/MacOS/k2-menubar"; do
    [ -x "$f" ] || { ok=0; echo "        missing: $f" >&2; }
done
[ "$ok" = 1 ] && pass "every sidecar is in K2.app (daemon + ptyd + export nested)" || fail "bundle" "see missing files above"
grep -q "bin k2-ptyd" "$helper/k2-ptyd" && grep -q "bin k2-daemon" "$helper/k2-daemon" \
    && pass "the nested binaries are the ones that call built" || fail "nested bytes" "$(cat "$helper/k2-daemon")"

echo "Step 8.5 (k2_build_standalone_macos_daemon):"
: > "$CALLS"
mkdir -p "$proj/scripts"
cp "$ROOT/scripts/linux-ptyd-embed.sh" "$proj/scripts/"
head -c 4096 /dev/urandom > "$helper/k2-ptyd"
sha="$(shasum -a 256 "$helper/k2-ptyd" | cut -d' ' -f1)"
head -c 4096 /dev/urandom > "$helper/k2-export"
esha="$(shasum -a 256 "$helper/k2-export" | cut -d' ' -f1)"
(
    export PATH="$stub:$PATH"
    unset CARGO_TARGET_DIR SDKROOT
    # shellcheck source=scripts/macos-daemon-helper-app.sh
    source "$ROOT/scripts/macos-daemon-helper-app.sh"
    k2_build_standalone_macos_daemon "$app" "$work/standalone" "$proj" > "$work/step85.log" 2>&1
) && pass "the standalone build succeeds" || fail "standalone" "$(cat "$work/step85.log")"
[ "$(wc -l < "$CALLS" | tr -d ' ')" = 1 ] && [ "$(cut -f1 "$CALLS")" = "$want" ] \
    && pass "the standalone rebuild uses Step 2's exact package set (k2-core is reused)" \
    || fail "standalone argv" "want: $want / got: $(cat "$CALLS")"
[ "$(cut -f2 "$CALLS")" = "embed=$helper/k2-ptyd" ] && grep -q "$sha" "$work/standalone" \
    && pass "only the embed differs: the signed nested k2-ptyd" || fail "embed" "$(cat "$CALLS")"
[ "$(cut -f3 "$CALLS")" = "export=$helper/k2-export" ] && grep -q "$esha" "$work/standalone" \
    && pass "and the signed nested k2-export" || fail "export embed" "$(cat "$CALLS")"

echo "No second list:"
n="$(grep -c 'cargo build --release' "$ROOT/scripts/lib/build-binaries.sh")"
[ "$n" = 1 ] && grep -q 'cargo build --release "${K2_MAC_SIDECAR_CARGO_ARGS\[@\]}"' "$ROOT/scripts/lib/build-binaries.sh" \
    && pass "build-binaries.sh has one cargo build, on the shared array" || fail "build-binaries.sh" "$n cargo build lines"
grep -q 'cargo build --release "${K2_MAC_SIDECAR_CARGO_ARGS\[@\]}"' "$ROOT/scripts/macos-daemon-helper-app.sh" \
    && ! grep -q 'cargo build --release -p k2-daemon' "$ROOT/scripts/macos-daemon-helper-app.sh" \
    && pass "the standalone daemon builds from the shared array" || fail "helper" "own package list in macos-daemon-helper-app.sh"
grep -q '^k2_build_mac_binaries "target/release/bundle/macos/K2.app"$' "$ROOT/scripts/release.sh" \
    && pass "release.sh Step 2 calls k2_build_mac_binaries" || fail "release.sh" "Step 2 no longer calls k2_build_mac_binaries"
for s in build-binaries.sh; do bash -n "$ROOT/scripts/lib/$s" && pass "bash -n $s" || fail "bash -n $s" syntax; done
for s in macos-daemon-helper-app.sh measure-clean-build.sh; do bash -n "$ROOT/scripts/$s" && pass "bash -n $s" || fail "bash -n $s" syntax; done

echo ""
echo "build-once: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
