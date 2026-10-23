#!/usr/bin/env bash
# Clean release build timing for the two big compiles K2.app ships:
# `-p k2` (src-tauri, the thin client) then `-p k2-daemon`, in that order,
# in one fresh target dir (the order k2_build_mac_binaries uses).
#
# Prints wall seconds per build, how many crates each one compiled (the
# second number is how much the daemon build could NOT reuse from the app
# build), and the slowest units from cargo's --timings report.
#
# Meant for compute nodes, from the repo root:
#   k2 compute run <node> --sha <sha> --needs rust@stable,cmake@3 --detach -- \
#     env CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 bash scripts/measure-clean-build.sh
#
# It does not run vite or fetch-frpc: it stubs out/renderer/index.html and
# src-tauri/binaries/frpc-<host> when they are missing (tauri-build and
# generate_context! only check that they exist). Never use the binaries it
# builds; they are for timing. The target dir is deleted at the end.
set -u
cd "$(dirname "$0")/.."

# The same build env as dev-build.sh, build-app.sh and release.sh
# (scripts/lib/build-binaries.sh): on macOS an unset SDKROOT is filled from
# `xcrun --sdk macosx --show-sdk-path` (a bare shell on z3mbpZ left it unset
# and llama-cpp-sys's cmake configure failed); a caller's SDKROOT wins.
# CMAKE_POLICY_VERSION_MINIMUM comes from .cargo/config.toml [env] for every
# cargo call; it is exported here too for any cmake run outside cargo.
# shellcheck source=scripts/lib/build-binaries.sh
source scripts/lib/build-binaries.sh
k2_mac_build_env
export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"
# Half-configured cmake dirs (a cache but no generated build file) in the
# repo's own target/ only, as the other build scripts do; good caches stay.
k2_clear_failed_cmake_dirs

T="$PWD/.bs-clean-target"
rm -rf "$T"; mkdir -p "$T"
export CARGO_TARGET_DIR="$T"
unset RUSTC_WRAPPER

HT=$(rustc -vV | sed -n 's/host: //p')
mkdir -p src-tauri/binaries out/renderer
if [ ! -e "src-tauri/binaries/frpc-$HT" ]; then
    printf '#!/bin/sh\n' > "src-tauri/binaries/frpc-$HT"
    chmod +x "src-tauri/binaries/frpc-$HT"
fi
[ -e out/renderer/index.html ] || echo '<html></html>' > out/renderer/index.html

count_tree() {
    cargo tree -p "$1" -e normal,build --prefix none 2>/dev/null | sed 's/ (\*)//' | sort -u | wc -l | tr -d ' '
}

echo "== $(rustc --version)  jobs=${CARGO_BUILD_JOBS:-default}  host=$HT"
echo "== crates in tree: k2=$(count_tree k2)  k2-daemon=$(count_tree k2-daemon)"

s0=$(date +%s)
cargo build --release -p k2 --timings > .bs-app.log 2>&1; rc1=$?
s1=$(date +%s)
cargo build --release -p k2-daemon --bin k2-daemon --bin k2-power-helper --timings > .bs-daemon.log 2>&1; rc2=$?
s2=$(date +%s)

echo "== app    (-p k2)        rc=$rc1 secs=$((s1-s0)) compiled=$(grep -c '^ *Compiling ' .bs-app.log)"
echo "== daemon (-p k2-daemon) rc=$rc2 secs=$((s2-s1)) compiled=$(grep -c '^ *Compiling ' .bs-daemon.log) (second compile)"
echo "== total secs=$((s2-s0))"
[ "$rc1" -ne 0 ] && grep -E '^error' -A12 .bs-app.log | head -80
[ "$rc2" -ne 0 ] && grep -E '^error' -A12 .bs-daemon.log | head -80

for h in "$T"/cargo-timings/cargo-timing-2*.html; do
    [ -e "$h" ] || continue
    echo "== slowest units: $(basename "$h")"
    python3 - "$h" <<'PY' || true
import json, re, sys
t = open(sys.argv[1]).read()
m = re.search(r'const UNIT_DATA = (\[.*?\]);', t, re.S)
if m:
    for u in sorted(json.loads(m.group(1)), key=lambda u: -u['duration'])[:12]:
        print(f"   {u['duration']:7.1f}s {u['name']} {u.get('version', '')} {u.get('target', '')}")
PY
done

du -sh "$T" 2>/dev/null
rm -rf "$T"
exit $(( rc1 | rc2 ))
