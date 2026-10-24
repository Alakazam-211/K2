#!/usr/bin/env bash
# Clean release build timings, two modes:
#
#   app-daemon (default)  `-p k2` (src-tauri, the thin client) then the
#       sidecar call, in that order, in one fresh target dir (the order
#       k2_build_mac_binaries uses). The second "compiled" number is how
#       much the sidecar build could NOT reuse from the app build.
#   sidecars  Step 2's sidecar build two ways, each in its own fresh target
#       dir: BEFORE = the four cargo calls 0.46.3 made (k2-daemon +
#       k2-power-helper, k2-ptyd, k2-node, k2-menubar), AFTER = the one call
#       K2_MAC_SIDECAR_CARGO_ARGS makes (scripts/macos-daemon-helper-app.sh).
#       On Linux k2-menubar is left out of both (it is a macOS binary).
#
# Prints wall seconds per build, how many crates each compiled, peak RSS
# when /usr/bin/time can say, and the slowest units from cargo's --timings
# report (the reports and logs are kept under .bs-timings/).
#
# Meant for compute nodes, from the repo root:
#   k2 compute run <node> --sha <sha> --needs rust@stable,cmake@3 --detach \
#     --artifact '.bs-timings/**' -- \
#     env CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 bash scripts/measure-clean-build.sh [sidecars]
#
# It does not run vite or fetch-frpc: it stubs out/renderer/index.html and
# src-tauri/binaries/frpc-<host> when they are missing (tauri-build and
# generate_context! only check that they exist). Never use the binaries it
# builds; they are for timing. The target dir is deleted at the end.
set -u
cd "$(dirname "$0")/.."
MODE="${1:-app-daemon}"
case "$MODE" in
    app-daemon|sidecars) ;;
    *) echo "usage: measure-clean-build.sh [app-daemon|sidecars]" >&2; exit 2 ;;
esac

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
OUT="$PWD/.bs-timings"
rm -rf "$T" "$OUT"
mkdir -p "$T" "$OUT"
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

# Peak RSS of each cargo call: BSD time -l (bytes) or GNU time -v (KB).
TIMECMD=()
if /usr/bin/time -l true >/dev/null 2>&1; then
    TIMECMD=(/usr/bin/time -l)
elif /usr/bin/time -v true >/dev/null 2>&1; then
    TIMECMD=(/usr/bin/time -v)
fi

# timed <label> <log> cargo args… : runs the cargo call with --timings and
# prints one summary line.
timed() {
    local label="$1" log="$2" s0 s1 rc rss
    shift 2
    s0=$(date +%s)
    "${TIMECMD[@]}" "$@" --timings > "$log" 2>&1
    rc=$?
    s1=$(date +%s)
    rss="$(sed -n -e 's/^ *\([0-9][0-9]*\)  *maximum resident set size.*/\1 bytes/p' \
        -e 's/^.*Maximum resident set size (kbytes): *\([0-9][0-9]*\).*/\1 KB/p' "$log" | tail -1)"
    echo "== $label rc=$rc secs=$((s1 - s0)) compiled=$(grep -c '^ *Compiling ' "$log") peak_rss=${rss:-?}"
    if [ "$rc" -ne 0 ]; then
        grep -E '^error' -A12 "$log" | head -80
    fi
    return "$rc"
}

# slowest <name> : keep the --timings report, print its 12 slowest units.
slowest() {
    local h
    for h in "$T"/cargo-timings/cargo-timing-2*.html; do
        [ -e "$h" ] || continue
        cp "$h" "$OUT/$1-$(basename "$h")"
        echo "== slowest units: $1"
        python3 - "$h" <<'PY'
import json, re, sys
t = open(sys.argv[1]).read()
m = re.search(r'const UNIT_DATA = (\[.*?\]);', t, re.S)
if not m:
    print("   (no UNIT_DATA in the report)")
    sys.exit(0)
for u in sorted(json.loads(m.group(1)), key=lambda u: -u['duration'])[:12]:
    print(f"   {u['duration']:7.1f}s {u['name']} {u.get('version', '')} {u.get('target', '')}")
PY
    done
    rm -rf "$T"/cargo-timings
}

echo "== $(rustc --version)  jobs=${CARGO_BUILD_JOBS:-default}  host=$HT  mode=$MODE"
rc=0
if [ "$MODE" = app-daemon ]; then
    echo "== crates in tree: k2=$(count_tree k2)  k2-daemon=$(count_tree k2-daemon)"
    timed "app (-p k2)" .bs-app.log cargo build --release -p k2 || rc=1
    slowest app
    timed "sidecars (second compile)" .bs-daemon.log cargo build --release "${K2_MAC_SIDECAR_CARGO_ARGS[@]}" || rc=1
    slowest sidecars
else
    before=(
        "-p k2-daemon --bin k2-daemon --bin k2-power-helper"
        "-p k2-ptyd --bin k2-ptyd"
        "-p k2-node --bin k2-node"
    )
    after=("${K2_MAC_SIDECAR_CARGO_ARGS[@]}")
    if [ "$(uname -s)" = Darwin ]; then
        before+=("-p k2-menubar --bin k2-menubar")
    else
        after=(-p k2-daemon --bin k2-daemon --bin k2-power-helper -p k2-ptyd --bin k2-ptyd -p k2-node --bin k2-node)
    fi
    b0=$(date +%s)
    i=0
    for call in "${before[@]}"; do
        i=$((i + 1))
        # shellcheck disable=SC2086 # the call is a word list on purpose
        timed "before $i/${#before[@]} (cargo build --release $call)" ".bs-before-$i.log" cargo build --release $call || rc=1
        slowest "before-$i"
    done
    b1=$(date +%s)
    echo "== BEFORE total secs=$((b1 - b0)) compiled=$(cat .bs-before-*.log | grep -c '^ *Compiling ')"
    rm -rf "$T"
    mkdir -p "$T"
    a0=$(date +%s)
    timed "after (cargo build --release ${after[*]})" .bs-after.log cargo build --release "${after[@]}" || rc=1
    a1=$(date +%s)
    slowest after
    echo "== AFTER total secs=$((a1 - a0)) compiled=$(grep -c '^ *Compiling ' .bs-after.log)"
fi
cp .bs-*.log "$OUT/" 2>/dev/null
du -sh "$T" 2>/dev/null
rm -rf "$T"
exit "$rc"
