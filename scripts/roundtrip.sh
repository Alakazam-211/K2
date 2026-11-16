#!/bin/sh
# The gate's round-trip part: the program's headless round trip (PROVENANCE.md), checked on the
# build node that holds its folder. The program and its driver run outside every repo,
# hash-pinned, and only their outputs come here: BLEND_ROUNDTRIP_DIR names the folder they wrote,
# one subfolder per release (the releases tests/roundtrip.rs names), each with the program's
# report of what it read (values.tsv) and its own saves of our patched files (resaved/*.blend).
# scripts/check.sh runs this part when a change lands (CHECK_TARGETS_LANDING=1), so a landing
# can't skip it; scripts/test-roundtrip.sh is its self-test.
#
# It fails, rather than pass having checked nothing, when: the folder isn't given or isn't
# there; a release's folder, report or saves are missing or empty; the test fails, runs no
# test, or checks fewer releases (with objects seen) than it names.
#
#   BLEND_ROUNDTRIP_DIR=DIR scripts/roundtrip.sh
set -eu
cd "$(dirname "$0")/.."
fail() {
  echo "round trip: FAIL: $*"
  exit 1
}
DIR=${BLEND_ROUNDTRIP_DIR:-}
[ -n "$DIR" ] || fail "BLEND_ROUNDTRIP_DIR is not set: the round trip's folder on the build node (PROVENANCE.md)"
[ -d "$DIR" ] || fail "BLEND_ROUNDTRIP_DIR names no folder: $DIR"
RELEASES=$(sed -n 's/^const RELEASES: &\[&str\] = &\[\(.*\)\];$/\1/p' crates/blend/tests/roundtrip.rs | tr -d '" ' | tr ',' ' ')
[ -n "$RELEASES" ] || fail "tests/roundtrip.rs names no releases"
n=0; rows=0; saves=0
for r in $RELEASES; do
  n=$((n + 1))
  [ -d "$DIR/$r" ] || fail "release $r: no folder in $DIR"
  v=$(grep -c . "$DIR/$r/values.tsv" 2>/dev/null || true)
  [ "${v:-0}" -gt 0 ] || fail "release $r: the program's report (values.tsv) is missing or empty"
  s=$(find "$DIR/$r/resaved" -maxdepth 1 -type f -name '*.blend' 2>/dev/null | wc -l | tr -d ' ')
  [ "$s" -gt 0 ] || fail "release $r: no program save in resaved/"
  rows=$((rows + v))
  saves=$((saves + s))
done
LOG=$(mktemp)
trap 'rm -f "$LOG"' EXIT
rc=0
cargo test -q --locked -p k2-mpc-blend --test roundtrip -- --ignored --exact --nocapture the_program_reads_our_writes_and_keeps_them > "$LOG" 2>&1 || rc=$?
cat "$LOG"
[ "$rc" -eq 0 ] || fail "the round-trip test failed (exit $rc)"
grep -q '^running 1 test' "$LOG" || fail "the round-trip test ran no test"
checked=$(grep -c '^round trip [^ ]*: [1-9][0-9]* objects seen, values held$' "$LOG" || true)
[ "${checked:-0}" -eq "$n" ] || fail "the test checked ${checked:-0} of $n releases with objects seen"
echo "round trip: ok ($checked releases checked, $rows objects in the program's reports, $saves program saves)"
