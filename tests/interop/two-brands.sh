#!/usr/bin/env bash
# Two servers of different brands talk to each other (brand step C4, the
# eval's 3.5 and 9.11): the wire stays neutral.
#
#   bash tests/interop/two-brands.sh [OUT]          OUT defaults to out/interop
#
# 1. The harness's own tests: the comparer and the test plane on fixtures,
#    then the door and the plane over sockets on 127.0.0.1.
# 2. The xy brand's names pass the name policy (scripts/brand.sh lint).
# 3. Two k2-daemon builds from this one commit, each in a target folder of
#    its own under $TMPDIR (never the shared cache, never a folder inside
#    $CARGO_TARGET_DIR): the default brand (no K2_BRAND_DIR), and xy
#    (K2_BRAND_DIR=brand/xy). Both start cold, so the free space there is
#    checked first: under INTEROP_MIN_FREE_GB (default 60) nothing is built
#    and the run asks to resubmit on a node with more free disk. Both
#    folders are removed on every way out, with an M line that says so.
# 4. Each daemon carries its own brand's mark (scripts/brand/check-brand-mark.py).
# 5. tests/interop/harness.py: the daemons with throwaway homes on free
#    ports, paired through federation and friends through a Postal test
#    plane, a message each way, a refused call each way, a vault hand-off;
#    every request and answer captured; three sessions (baseline, mixed,
#    swapped); then tests/interop/wire_compare.py compares them by structure
#    and checks that no X-K5 header and no other brand's name is on the
#    federation or Postal wire.
#
# Captures, compare reports and daemon logs land in OUT. Runs cargo, starts
# daemons and opens ports: on K2 Compute (Linux) only. Every step prints an
# M line; the last one names every failure, and the exit code is non-zero
# when anything failed.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
OUT="${1:-$ROOT/out/interop}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

FAILED=""
m() { echo "M $1=$2"; [ "$2" = 0 ] || FAILED="$FAILED [$1]"; }
TARGETS=()
# Remove the build folders (once), with an M line; a folder left is a failure.
cleanup() {
    [ "${#TARGETS[@]}" -gt 0 ] || return 0
    rm -rf "${TARGETS[@]}"
    local left="" t
    for t in "${TARGETS[@]}"; do [ -e "$t" ] && left="$left $t"; done
    TARGETS=()
    if [ -z "$left" ]; then m "interop target folders removed: yes" 0; else m "interop target folders removed: no:$left" 1; fi
}
end() {
    trap - EXIT
    cleanup
    if [ -n "$FAILED" ]; then echo "M interop job=1 failed:$FAILED"; exit 1; fi
    echo "M interop job=0"; exit 0
}
trap cleanup EXIT
trap 'exit 143' TERM INT
step() { echo ""; echo "── interop: $* ──"; }

export RUSTFLAGS="${RUSTFLAGS:--Dwarnings}"
export NO_COLOR=1

step "the harness's own tests"
python3 -B tests/interop/test_wire_compare.py > "$OUT/test_wire_compare.log" 2>&1; rc=$?
m tests/interop/test_wire_compare.py $rc; [ $rc = 0 ] || tail -40 "$OUT/test_wire_compare.log"
python3 -B tests/interop/test_postal_plane.py > "$OUT/test_postal_plane.log" 2>&1; rc=$?
m tests/interop/test_postal_plane.py $rc; [ $rc = 0 ] || tail -40 "$OUT/test_postal_plane.log"
python3 -B tests/interop/test_harness_sockets.py > "$OUT/test_harness_sockets.log" 2>&1; rc=$?
m tests/interop/test_harness_sockets.py $rc; [ $rc = 0 ] || tail -40 "$OUT/test_harness_sockets.log"
[ -z "$FAILED" ] || { echo "the harness's own tests failed: nothing is built"; end; }

step "the xy brand's names pass the name policy"
bash scripts/brand.sh lint brand/xy; rc=$?
m "scripts/brand.sh lint brand/xy" $rc
[ $rc = 0 ] || end

PARENT="${TMPDIR:-/tmp}"
MIN_GB="${INTEROP_MIN_FREE_GB:-60}"
free_gb="$(df -Pk "$PARENT" | awk 'NR == 2 { print int($4 / 1048576) }')"
echo "M interop targets: ${free_gb:-unknown} GB free under $PARENT (want $MIN_GB)"
if [ -z "$free_gb" ] || [ "$free_gb" -lt "$MIN_GB" ]; then
    m "disk: ${free_gb:-unknown} GB free under $PARENT, want $MIN_GB; nothing was built; resubmit on a node with more free disk, never in the shared folder" 1
    end
fi
DEF_T="$(mktemp -d "$PARENT/k2-interop-default.XXXXXX")"
TARGETS+=("$DEF_T")
XY_T="$(mktemp -d "$PARENT/k2-interop-xy.XXXXXX")"
TARGETS+=("$XY_T")

step "cargo build of k2-daemon, the default brand"
env -u K2_BRAND_DIR CARGO_TARGET_DIR="$DEF_T" cargo build --locked -p k2-daemon --bin k2-daemon > "$OUT/build-default.log" 2>&1; rc=$?
m "cargo build -p k2-daemon (default brand)" $rc
[ $rc = 0 ] || grep -E '^(error|warning)' -A16 "$OUT/build-default.log" | head -200
step "cargo build of k2-daemon, K2_BRAND_DIR=brand/xy"
K2_BRAND_DIR=brand/xy CARGO_TARGET_DIR="$XY_T" cargo build --locked -p k2-daemon --bin k2-daemon > "$OUT/build-xy.log" 2>&1; rc=$?
m "cargo build -p k2-daemon (K2_BRAND_DIR=brand/xy)" $rc
[ $rc = 0 ] || grep -E '^(error|warning)' -A16 "$OUT/build-xy.log" | head -200
[ -z "$FAILED" ] || end

DEF_BIN="$DEF_T/debug/k2-daemon"
XY_BIN="$XY_T/debug/k2-daemon"
step "each daemon carries its own brand's mark"
python3 scripts/brand/check-brand-mark.py --brand k2 "$DEF_BIN"; rc=$?
m "scripts/brand/check-brand-mark.py --brand k2 (default daemon)" $rc
python3 scripts/brand/check-brand-mark.py --brand xy "$XY_BIN"; rc=$?
m "scripts/brand/check-brand-mark.py --brand xy (xy daemon)" $rc
[ -z "$FAILED" ] || end

step "two brands, three sessions"
python3 -B tests/interop/harness.py --default-daemon "$DEF_BIN" --brand-daemon "$XY_BIN" \
    --default-brand brand/k2 --brand brand/xy --out "$OUT" 2>&1 | tee "$OUT/harness.log"
rc=${PIPESTATUS[0]}
m tests/interop/harness.py $rc
for f in "$OUT"/compare-*.txt; do
    [ -f "$f" ] || continue
    echo "── $(basename "$f") ──"
    head -80 "$f"
done
end
