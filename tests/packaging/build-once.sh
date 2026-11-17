#!/usr/bin/env bash
# Stubbed test for the macOS sidecar build (scripts/lib/build-binaries.sh,
# release.sh Step 2) and the Step 8.5 standalone daemon
# (k2_build_standalone_macos_daemon): every sidecar comes from ONE cargo
# call, and the standalone rebuild uses the exact same package set, so it
# reuses every compiled crate and only k2-daemon relinks. The k5 command's
# program is the one other call: universal (both --target slices joined by
# lipo) and checked to carry cli/k2's version. Stub `bun`, `cargo`, `lipo`,
# `rustup` and `codesign` on PATH; nothing compiles, signs or touches ~/.k2.
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
mkdir -p "$stub" "$proj/cli"
# The script the k5 program's version comes from (its build.rs reads it).
printf '#!/bin/bash\nK2_CLI_VERSION="9.8.7"\n' > "$proj/cli/k2"
export CALLS="$work/cargo-calls"
: > "$CALLS"

# cargo: record each call (argv + the embed env), then play cargo: every
# --bin NAME lands in <target>/release/NAME; k2-daemon carries the sha256
# of $K2_PTYD_EMBED and of $K2_EXPORT_EMBED when set (what build.rs embeds).
cat > "$stub/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\tembed=%s\texport=%s\n' "$*" "${K2_PTYD_EMBED:-}" "${K2_EXPORT_EMBED:-}" >> "$CALLS"
printf 'target=%s jobs=%s\n' "${CARGO_TARGET_DIR:-}" "${CARGO_BUILD_JOBS:-}" >> "$CALLS.env"
printf 'cargo %s\n' "${CARGO_ENCODED_RUSTFLAGS:-none}" | tr '\037' '|' >> "$CALLS.rf"
# Parallel proof: the tauri build must already be running while we build
# (STUB_MEET=1); in the serial order it never is, so this times out.
if [ "${STUB_MEET:-0}" = 1 ]; then
    : > "$MEET/cargo"
    i=0; while [ ! -e "$MEET/tauri" ]; do i=$((i + 1)); [ "$i" -lt 100 ] || { echo "stub cargo: tauri build never ran beside me" >&2; exit 1; }; sleep 0.1; done
fi
[ "${STUB_CARGO_FAIL:-0}" = 1 ] && { echo "stub cargo: failing on purpose" >&2; exit 101; }
# --target T (any number): each lands in <target>/T/release, as cargo's.
outs=()
prev=""
for a in "$@"; do
    [ "$prev" = "--target" ] && outs+=("${CARGO_TARGET_DIR:-target}/$a/release")
    prev="$a"
done
[ "${#outs[@]}" -gt 0 ] || outs=("${CARGO_TARGET_DIR:-target}/release")
for out in "${outs[@]}"; do
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
            # The k5 program's build.rs writes cli/k2's version mark.
            if [ "$a" = k5-cli ]; then
                v="${STUB_K5_VERSION:-$(sed -n 's/^K2_CLI_VERSION="\(.*\)"$/\1/p' cli/k2)}"
                printf 'K5_CLI_VERSION=%s\0' "$v" >> "$out/$a"
            fi
            # STUB_LEAK=1: a binary that kept the builder's home (no path maps).
            [ "${STUB_LEAK:-0}" = 1 ] && [ "$a" = k2-node ] && printf '\0/home/%s/.cargo/registry/src/x-1.0/src/lib.rs\0' "$(id -un)" >> "$out/$a"
            chmod +x "$out/$a"
        fi
        prev="$a"
    done
done
EOF
# lipo -create -output DEST SLICE...: a real universal Mach-O header over
# the slices (cputype from the slice's target folder), so
# scripts/lib/check-k5-cli.py reads it as it reads the real one.
cat > "$stub/lipo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$1" = -create ] && [ "$2" = -output ] || { echo "stub lipo: unexpected: $*" >&2; exit 1; }
dest="$3"; shift 3
python3 - "$dest" "$@" <<'PY'
import struct, sys
dest, slices = sys.argv[1], sys.argv[2:]
types = {"aarch64-apple-darwin": 0x0100000C, "x86_64-apple-darwin": 0x01000007}
blobs = [(types[next(t for t in types if "/" + t + "/" in p)], b"\xcf\xfa\xed\xfe" + open(p, "rb").read()) for p in slices]
head = b"\xca\xfe\xba\xbe" + struct.pack(">I", len(blobs))
off = 8 + 20 * len(blobs)
body = b""
for t, b in blobs:
    head += struct.pack(">IIIII", t, 0, off + len(body), len(b), 0)
    body += b
open(dest, "wb").write(head + body)
PY
EOF
# rustup target list --installed: both Mac targets (STUB_NO_X86=1: the
# x86_64 one is missing).
cat > "$stub/rustup" <<'EOF'
#!/usr/bin/env bash
[ "$*" = "target list --installed" ] || { echo "stub rustup: unexpected: $*" >&2; exit 1; }
echo aarch64-apple-darwin
[ "${STUB_NO_X86:-0}" = 1 ] || echo x86_64-apple-darwin
echo aarch64-unknown-linux-gnu
EOF
# bun run tauri build: makes the app bundle where tauri does, under
# $CARGO_TARGET_DIR (else target/).
cat > "$stub/bun" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$*" = "run tauri build" ] || { echo "stub bun: unexpected: $*" >&2; exit 1; }
printf 'tauri target=%s jobs=%s\n' "${CARGO_TARGET_DIR:-}" "${CARGO_BUILD_JOBS:-}" >> "$CALLS.env"
printf 'tauri %s\n' "${CARGO_ENCODED_RUSTFLAGS:-none}" | tr '\037' '|' >> "$CALLS.rf"
echo $$ > "$MEET/tauri.pid"
if [ "${STUB_MEET:-0}" = 1 ]; then
    : > "$MEET/tauri"
    i=0; while [ ! -e "$MEET/cargo" ]; do i=$((i + 1)); [ "$i" -lt 100 ] || { echo "stub bun: the sidecar build never ran beside me" >&2; exit 1; }; sleep 0.1; done
fi
[ -z "${STUB_TAURI_SLEEP:-}" ] || sleep "$STUB_TAURI_SLEEP"
[ "${STUB_TAURI_FAIL:-0}" = 1 ] && { echo "stub bun: failing on purpose"; exit 1; }
t="${CARGO_TARGET_DIR:-target}"
mkdir -p "$t/release/bundle/macos/K2.app/Contents/MacOS" "$t/release/bundle/dmg" "$t/release/bundle/macos/K2.app/Contents/Resources/dfn3"
printf 'app' > "$t/release/bundle/macos/K2.app/Contents/MacOS/k2"
# The DeepFilterNet3 files tauri bundles (beforeBuildCommand stages them).
for f in dfn3_ll.nnef.tar NOTICE LICENSE-MIT LICENSE-APACHE; do
    printf 'fake %s' "$f" > "$t/release/bundle/macos/K2.app/Contents/Resources/dfn3/$f"
done
printf '#!/bin/sh\n' > "$t/release/bundle/dmg/bundle_dmg.sh"
echo "tauri: Finished release"
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
chmod +x "$stub/cargo" "$stub/bun" "$stub/codesign" "$stub/lipo" "$stub/rustup"
# Off macOS there is no plutil: lint the generated Info.plist with plistlib.
if ! command -v plutil >/dev/null 2>&1; then
    cat > "$stub/plutil" <<'EOF'
#!/usr/bin/env bash
[ "$1" = -lint ] || { echo "stub plutil: only -lint" >&2; exit 1; }
python3 -c 'import plistlib, sys; plistlib.load(open(sys.argv[1], "rb"))' "$2"
EOF
    chmod +x "$stub/plutil"
fi

# scripts/fetch-dfn3.sh in the project: a stub that fetches nothing and pins
# the fake weights the stub tauri build bundles (k2_check_bundled_dfn3).
mkdir -p "$proj/scripts"
printf '#!/bin/sh\n# stub\nDFN3_WEIGHTS_SHA256="%s"\nexit 0\n' \
    "$(printf 'fake dfn3_ll.nnef.tar' | shasum -a 256 | cut -d' ' -f1)" > "$proj/scripts/fetch-dfn3.sh"
chmod +x "$proj/scripts/fetch-dfn3.sh"
export MEET="$work/meet"
mkdir -p "$MEET"
# run_step2 LOG [VAR=value...]: k2_build_mac_binaries in $proj with stubs.
run_step2() {
    local log="$1"; shift
    rm -f "$MEET"/*
    (
        cd "$proj"
        export PROJECT_DIR="$ROOT" PATH="$stub:$PATH"
        unset CARGO_TARGET_DIR SDKROOT CARGO_BUILD_JOBS
        for kv in "$@"; do export "${kv?}"; done
        # shellcheck source=scripts/lib/build-binaries.sh
        source "$ROOT/scripts/lib/build-binaries.sh"
        # plutil is macOS-only; the helper's layout is checked below instead.
        k2_daemon_helper_verify() { [ -x "$(k2_daemon_helper_exe "$1")" ]; }
        k2_screen_helper_verify() { [ -x "$(k2_screen_helper_exe "$1")" ]; }
        assert_daemon_oauth_not_placeholder() { [ -x "$1" ]; }
        k2_build_mac_binaries "target/release/bundle/macos/K2.app"
    ) > "$log" 2>&1
}

echo "Step 2 (k2_build_mac_binaries):"
# A stale bundle from an older build must not survive.
mkdir -p "$proj/target/release/bundle/macos/K2.app/Contents/MacOS"
printf 'stale' > "$proj/target/release/bundle/macos/K2.app/Contents/MacOS/stale-file"
run_step2 "$work/step2.log" STUB_MEET=1 K2_MAC_TAURI_JOBS=3 K2_MAC_SIDECAR_JOBS=7 \
    && pass "k2_build_mac_binaries succeeds" || fail "k2_build_mac_binaries" "$(cat "$work/step2.log")"
pass_or() { if eval "$1"; then pass "$2"; else fail "$2" "$3"; fi; }
pass_or '[ -e "$MEET/tauri" ] && [ -e "$MEET/cargo" ]' "tauri build and the sidecar build run at the same time" "$(cat "$work/step2.log")"
pass_or 'grep -qx "tauri target=$proj/target/tauri jobs=3" "$CALLS.env"' \
    "tauri build gets its own target dir (target/tauri) and K2_MAC_TAURI_JOBS" "$(cat "$CALLS.env")"
pass_or 'grep -qx "target= jobs=7" "$CALLS.env"' \
    "the sidecars keep target/ (Step 8.5 reuses it) with K2_MAC_SIDECAR_JOBS" "$(cat "$CALLS.env")"
pass_or '[ -e "$proj/target/release/bundle/dmg/bundle_dmg.sh" ] && [ ! -e "$proj/target/tauri/release/bundle" ] && [ ! -e "$proj/target/release/bundle/macos/K2.app/Contents/MacOS/stale-file" ]' \
    "the fresh tauri bundle replaces target/release/bundle (DMG script included, stale bundle gone)" "$(ls -R "$proj/target" | head -30)"
pass_or 'grep -q "^  \[tauri\] tauri: Finished release" "$work/step2.log"' "tauri's output is prefixed [tauri]" "$(cat "$work/step2.log")"
# The release path maps (scripts/lib/build-paths.sh) reach both builds, the
# same flags, $HOME first and the checkout after it.
rf_cargo="$(sed -n 's/^cargo //p' "$CALLS.rf")"
rf_tauri="$(sed -n 's/^tauri //p' "$CALLS.rf")"
pass_or '[ -n "$rf_cargo" ] && [ "$rf_cargo" = "$rf_tauri" ] && [[ "$rf_cargo" == "--remap-path-prefix=$HOME=/home|"* ]] && [[ "$rf_cargo" == *"|--remap-path-prefix=$proj=/k2|"* ]]' \
    "the sidecar and tauri builds both run under the path maps (\$HOME first, the checkout after)" "$(cat "$CALLS.rf")"
pass_or 'grep -q "^  no builder paths in [0-9]* file(s)" "$work/step2.log"' "Step 2 checks the whole app for build paths" "$(cat "$work/step2.log")"

want="build --release -p k2-daemon --bin k2-daemon --bin k2-power-helper -p k2-ptyd --bin k2-ptyd -p k2-export --bin k2-export -p k2-deckrender --bin k2-deckrender -p k2-scenerender --bin k2-scenerender -p k2-node --bin k2-node -p k2-menubar --bin k2-menubar -p k2-deskd --bin k2-deskd"
want_k5="build --release -p k5-cli --bin k5-cli --target aarch64-apple-darwin --target x86_64-apple-darwin"
[ "$(wc -l < "$CALLS" | tr -d ' ')" = 2 ] && pass "one cargo call builds every sidecar, one the k5 program" \
    || fail "cargo calls" "$(cat "$CALLS")"
[ "$(head -1 "$CALLS")" = "$want	embed=	export=" ] && pass "the call is K2_MAC_SIDECAR_CARGO_ARGS, release, no embed" \
    || fail "step 2 argv" "got: $(head -1 "$CALLS")"
[ "$(sed -n 2p "$CALLS")" = "$want_k5	embed=	export=" ] && pass "then k5-cli alone, release, both Mac targets" \
    || fail "k5-cli argv" "got: $(sed -n 2p "$CALLS")"
app="$proj/target/release/bundle/macos/K2.app"
helper="$app/Contents/Helpers/K2 Daemon.app/Contents/MacOS"
ok=1
for f in "$helper/k2-daemon" "$helper/k2-ptyd" "$helper/k2-export" "$helper/k2-deckrender" "$helper/k2-scenerender" "$app/Contents/MacOS/k2-power-helper" \
    "$app/Contents/MacOS/k2-node" "$app/Contents/MacOS/k2-menubar" "$app/Contents/MacOS/k5-cli" \
    "$app/Contents/Helpers/K2 Screen.app/Contents/MacOS/k2-deskd"; do
    [ -x "$f" ] || { ok=0; echo "        missing: $f" >&2; }
done
[ "$ok" = 1 ] && pass "every sidecar is in K2.app (daemon + ptyd + export + deckrender + scenerender nested, K2 Screen and k5-cli beside)" || fail "bundle" "see missing files above"
python3 -I "$ROOT/scripts/lib/check-k5-cli.py" "$app/Contents/MacOS/k5-cli" "$proj/cli/k2" macos >/dev/null \
    && pass "the bundled k5-cli is universal (arm64 + x86_64), each slice cli/k2's version" \
    || fail "bundled k5-cli" "$(python3 -I "$ROOT/scripts/lib/check-k5-cli.py" "$app/Contents/MacOS/k5-cli" "$proj/cli/k2" macos 2>&1)"
[ ! -L "$app/Contents/MacOS/k5" ] && [ ! -e "$app/Contents/MacOS/k5" ] && [ ! -L "$app/Contents/MacOS/k2" ] \
    && pass "the build links nothing to k5-cli (only the owner's switch does)" || fail "links" "$(ls -l "$app/Contents/MacOS")"
grep -q "bin k2-ptyd" "$helper/k2-ptyd" && grep -q "bin k2-daemon" "$helper/k2-daemon" \
    && pass "the nested binaries are the ones that call built" || fail "nested bytes" "$(cat "$helper/k2-daemon")"

echo "Step 8.5 (k2_build_standalone_macos_daemon):"
: > "$CALLS"
: > "$CALLS.rf"
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
[ "$(sed -n 's/^cargo //p' "$CALLS.rf")" = "$rf_cargo" ] \
    && pass "the standalone rebuild runs under Step 2's exact path maps (no full rebuild)" || fail "standalone maps" "step 2: $rf_cargo / 8.5: $(cat "$CALLS.rf")"
grep -q "^  no builder paths in 1 file(s)" "$work/step85.log" && pass "and its daemon is checked for build paths" || fail "standalone check" "$(cat "$work/step85.log")"

echo "Step 2 failures and the serial order:"
: > "$CALLS"; : > "$CALLS.env"
t0=$(date +%s)
if run_step2 "$work/fail1.log" STUB_CARGO_FAIL=1 STUB_TAURI_SLEEP=30; then
    fail "a failed sidecar build fails Step 2" "$(cat "$work/fail1.log")"
else
    pass "a failed sidecar build fails Step 2"
fi
tp="$(cat "$MEET/tauri.pid" 2>/dev/null || echo none)"
pass_or '[ $(( $(date +%s) - t0 )) -lt 20 ] && ! kill -0 "$tp" 2>/dev/null && grep -q "the tauri build was stopped" "$work/fail1.log"' \
    "and stops the tauri build at once (no 30 s wait, no process left)" "pid $tp; $(cat "$work/fail1.log")"
if run_step2 "$work/leak.log" STUB_LEAK=1; then
    fail "a sidecar holding \$HOME fails Step 2" "$(cat "$work/leak.log")"
else
    pass_or 'grep -qF "FATAL: target/release/bundle/macos/K2.app/Contents/MacOS/k2-node holds the builder" "$work/leak.log" && grep -qF "/.cargo/registry/src/x-1.0/src/lib.rs" "$work/leak.log"' \
        "a sidecar holding \$HOME fails Step 2, before signing, naming the file and the string" "$(cat "$work/leak.log")"
fi
: > "$CALLS"
if run_step2 "$work/fail-x86.log" STUB_NO_X86=1; then
    fail "a Mac without the x86_64 target fails Step 2" "$(cat "$work/fail-x86.log")"
else
    pass_or 'grep -q "rustup target add x86_64-apple-darwin" "$work/fail-x86.log" && ! grep -q -- "-p k5-cli" "$CALLS"' \
        "a Mac without the x86_64 target fails Step 2 before building k5-cli, and says how to fix it" "$(cat "$work/fail-x86.log")"
fi
if run_step2 "$work/fail-k5v.log" STUB_K5_VERSION=1.2.3; then
    fail "a k5-cli of another version fails Step 2" "$(cat "$work/fail-k5v.log")"
else
    pass_or 'grep -q "carries version 1.2.3, the script 9.8.7" "$work/fail-k5v.log"' \
        "a k5-cli whose mark isn't cli/k2's version fails Step 2" "$(cat "$work/fail-k5v.log")"
fi
if run_step2 "$work/fail2.log" STUB_TAURI_FAIL=1; then
    fail "a failed tauri build fails Step 2" "$(cat "$work/fail2.log")"
else
    pass_or 'grep -q "FATAL: tauri build failed" "$work/fail2.log"' "a failed tauri build fails Step 2" "$(cat "$work/fail2.log")"
fi
: > "$CALLS"; : > "$CALLS.env"
rm -rf "$proj/target"
run_step2 "$work/serial.log" K2_MAC_BUILD_PARALLEL=0 \
    && pass "K2_MAC_BUILD_PARALLEL=0 still builds" || fail "serial build" "$(cat "$work/serial.log")"
pass_or 'grep -qx "tauri target= jobs=" "$CALLS.env" && [ -x "$proj/target/release/bundle/macos/K2.app/Contents/MacOS/k2-node" ] && [ -x "$proj/target/release/bundle/macos/K2.app/Contents/MacOS/k5-cli" ] && [ ! -e "$proj/target/tauri" ]' \
    "K2_MAC_BUILD_PARALLEL=0 is the old order: one target dir, cargo's own job count" "$(cat "$CALLS.env")"
# k2_mac_build_jobs with a fake core count (sysctl stubbed as a function).
jobs_for() (
    unset K2_MAC_TAURI_JOBS K2_MAC_SIDECAR_JOBS
    ncpu="$1"
    sysctl() { echo "$ncpu"; }
    # shellcheck source=scripts/lib/build-binaries.sh
    source "$ROOT/scripts/lib/build-binaries.sh" >/dev/null 2>&1
    k2_mac_build_jobs
    echo "$K2_MAC_TAURI_JOBS+$K2_MAC_SIDECAR_JOBS"
)
pass_or '[ "$(jobs_for 10)" = "3+7" ] && [ "$(jobs_for 18)" = "6+12" ] && [ "$(jobs_for 4)" = "2+2" ]' \
    "job split: a third of the cores to tauri, the rest to the sidecars (10 cores: 3+7)" "10:$(jobs_for 10) 18:$(jobs_for 18) 4:$(jobs_for 4)"

echo "No second list:"
n="$(grep -c 'cargo build --release' "$ROOT/scripts/lib/build-binaries.sh")"
[ "$n" = 2 ] && grep -q 'cargo build --release "${K2_MAC_SIDECAR_CARGO_ARGS\[@\]}"' "$ROOT/scripts/lib/build-binaries.sh" \
    && grep -q 'cargo build --release -p k5-cli --bin k5-cli "${args\[@\]}"' "$ROOT/scripts/lib/build-binaries.sh" \
    && pass "build-binaries.sh has two cargo builds: the shared array and the k5 program" || fail "build-binaries.sh" "$n cargo build lines"
grep -q 'cargo build --release "${K2_MAC_SIDECAR_CARGO_ARGS\[@\]}"' "$ROOT/scripts/macos-daemon-helper-app.sh" \
    && ! grep -q 'cargo build --release -p k2-daemon' "$ROOT/scripts/macos-daemon-helper-app.sh" \
    && pass "the standalone daemon builds from the shared array" || fail "helper" "own package list in macos-daemon-helper-app.sh"
grep -q '^k2_build_mac_binaries "target/release/bundle/macos/K2.app"$' "$ROOT/scripts/release.sh" \
    && pass "release.sh Step 2 calls k2_build_mac_binaries" || fail "release.sh" "Step 2 no longer calls k2_build_mac_binaries"
for s in build-binaries.sh; do bash -n "$ROOT/scripts/lib/$s" && pass "bash -n $s" || fail "bash -n $s" syntax; done
for s in macos-daemon-helper-app.sh macos-screen-helper-app.sh measure-clean-build.sh; do bash -n "$ROOT/scripts/$s" && pass "bash -n $s" || fail "bash -n $s" syntax; done

echo ""
echo "build-once: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
