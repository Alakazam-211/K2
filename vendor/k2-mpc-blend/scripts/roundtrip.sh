#!/bin/sh
# The gate's round-trip part: the program's headless round trip (PROVENANCE.md), checked on the
# build node that holds its folder. The program and its driver run outside every repo,
# hash-pinned, and only their outputs come here: BLEND_ROUNDTRIP_DIR names the folder they wrote,
# one subfolder per release (the releases tests/roundtrip.rs names), each with the program's
# report of what it read (values.tsv) and its own saves of our patched files (resaved/*.blend);
# beside it, OUT-build/<release>/ holds the program's own builds of the scene spec (built/*.blend)
# and its report of them (values.tsv). The own saves (BLEND_OWN_SAVES_DIR, one subfolder per release, each
# with values.tsv and resaved/*.blend): the writer's patches of the program's own saves, which
# the program reopened, reported and saved again; the reports before the patches are the round
# trip's. The reopened edits (BLEND_REOPENED_DIR, one subfolder per release, each with
# values.tsv, a <file>.log per file and resaved/*.blend): the writer's and the live tier's edits
# of the program's builds of a scene with an animated visibility bit and of one with a
# constrained object, which the program reopened, reported, logged and saved again; our edited
# files and their record (BLEND_REOPENED_IN: PATCHES.tsv, one subfolder per release); the
# program's reports of the two builds before the edits (BLEND_REOPENED_ANIMATED_BEFORE,
# BLEND_REOPENED_CONSTRAINED_BEFORE, one subfolder per release, each with values.tsv).
# scripts/check.sh runs this part when a change lands (CHECK_TARGETS_LANDING=1), so a landing
# can't skip it; scripts/test-roundtrip.sh is its self-test.
#
# It fails, rather than pass having checked nothing, when: the folder isn't given or isn't
# there; a release's folder, report or saves are missing or empty; a release's builds or their
# report are missing or empty; the own saves' folder isn't given or isn't there, or a release's
# folder, report or saves in it are missing or empty; a reopened edits' folder isn't given or
# isn't there, the record of the edits is missing or empty, or a release's folder, report, saves,
# logs, edited inputs or report before is missing or empty; any of the three tests fails, runs no
# test, or checks fewer releases (with objects seen, builds read, the own saves' files and the
# reopened edits checked) than it names.
#
#   BLEND_ROUNDTRIP_DIR=DIR BLEND_OWN_SAVES_DIR=DIR BLEND_REOPENED_DIR=DIR BLEND_REOPENED_IN=DIR \
#   BLEND_REOPENED_ANIMATED_BEFORE=DIR BLEND_REOPENED_CONSTRAINED_BEFORE=DIR scripts/roundtrip.sh
set -eu
cd "$(dirname "$0")/.."
fail() {
  echo "round trip: FAIL: $*"
  exit 1
}
DIR=${BLEND_ROUNDTRIP_DIR:-}
[ -n "$DIR" ] || fail "BLEND_ROUNDTRIP_DIR is not set: the round trip's folder on the build node (PROVENANCE.md)"
[ -d "$DIR" ] || fail "BLEND_ROUNDTRIP_DIR names no folder: $DIR"
# absolute, so the test (run from its crate's folder) finds the same folder
DIR=$(cd "$DIR" && pwd)
export BLEND_ROUNDTRIP_DIR="$DIR"
SB=${BLEND_OWN_SAVES_DIR:-}
[ -n "$SB" ] || fail "BLEND_OWN_SAVES_DIR is not set: the own saves' folder on the build node (PROVENANCE.md)"
[ -d "$SB" ] || fail "BLEND_OWN_SAVES_DIR names no folder: $SB"
SB=$(cd "$SB" && pwd)
export BLEND_OWN_SAVES_DIR="$SB"
# folder VAR WHAT: the folder VAR names, made absolute and exported, or the part fails
folder() {
  eval "f=\${$1:-}"
  [ -n "$f" ] || fail "$1 is not set: $2 on the build node (PROVENANCE.md)"
  [ -d "$f" ] || fail "$1 names no folder: $f"
  f=$(cd "$f" && pwd)
  eval "export $1=\"\$f\""
}
folder BLEND_REOPENED_DIR "the folder of the program's reopening of our animated and constrained edits"
folder BLEND_REOPENED_IN "the folder of our edited files and their record"
folder BLEND_REOPENED_ANIMATED_BEFORE "the folder of the program's report of its build with an animated visibility bit"
folder BLEND_REOPENED_CONSTRAINED_BEFORE "the folder of the program's report of its build with a constrained object"
RD=$BLEND_REOPENED_DIR; RI=$BLEND_REOPENED_IN
rec=$(grep -c . "$RI/PATCHES.tsv" 2>/dev/null || true)
[ "${rec:-0}" -gt 0 ] || fail "the record of the edits (PATCHES.tsv) is missing or empty in $RI"
RELEASES=$(sed -n 's/^const RELEASES: &\[&str\] = &\[\(.*\)\];$/\1/p' crates/blend/tests/roundtrip.rs | tr -d '" ' | tr ',' ' ')
[ -n "$RELEASES" ] || fail "tests/roundtrip.rs names no releases"
n=0; rows=0; saves=0; brows=0; builds=0; sbrows=0; sbsaves=0; rrows=0; rsaves=0; rlogs=0; rinputs=0
for r in $RELEASES; do
  n=$((n + 1))
  [ -d "$DIR/$r" ] || fail "release $r: no folder in $DIR"
  v=$(grep -c . "$DIR/$r/values.tsv" 2>/dev/null || true)
  [ "${v:-0}" -gt 0 ] || fail "release $r: the program's report (values.tsv) is missing or empty"
  s=$(find "$DIR/$r/resaved" -maxdepth 1 -type f -name '*.blend' 2>/dev/null | wc -l | tr -d ' ')
  [ "$s" -gt 0 ] || fail "release $r: no program save in resaved/"
  B="$DIR/../OUT-build/$r"
  bv=$(grep -c . "$B/values.tsv" 2>/dev/null || true)
  [ "${bv:-0}" -gt 0 ] || fail "release $r: the program's report of its builds (OUT-build/$r/values.tsv) is missing or empty"
  b=$(find "$B/built" -maxdepth 1 -type f -name '*.blend' 2>/dev/null | wc -l | tr -d ' ')
  [ "$b" -gt 0 ] || fail "release $r: no program build in OUT-build/$r/built/"
  [ -d "$SB/$r" ] || fail "own saves release $r: no folder in $SB"
  sv=$(grep -c . "$SB/$r/values.tsv" 2>/dev/null || true)
  [ "${sv:-0}" -gt 0 ] || fail "own saves release $r: the program's report (values.tsv) is missing or empty"
  ss=$(find "$SB/$r/resaved" -maxdepth 1 -type f -name '*.blend' 2>/dev/null | wc -l | tr -d ' ')
  [ "$ss" -gt 0 ] || fail "own saves release $r: no program save in resaved/"
  [ -d "$RD/$r" ] || fail "reopened edits release $r: no folder in $RD"
  rv=$(grep -c . "$RD/$r/values.tsv" 2>/dev/null || true)
  [ "${rv:-0}" -gt 0 ] || fail "reopened edits release $r: the program's report (values.tsv) is missing or empty"
  rs=$(find "$RD/$r/resaved" -maxdepth 1 -type f -name '*.blend' 2>/dev/null | wc -l | tr -d ' ')
  [ "$rs" -gt 0 ] || fail "reopened edits release $r: no program save in resaved/"
  rl=$(find "$RD/$r" -maxdepth 1 -type f -name '*.blend.log' 2>/dev/null | wc -l | tr -d ' ')
  [ "$rl" -gt 0 ] || fail "reopened edits release $r: no driver log (<file>.log)"
  ri=$(find "$RI/$r" -maxdepth 1 -type f -name '*.blend' 2>/dev/null | wc -l | tr -d ' ')
  [ "$ri" -gt 0 ] || fail "reopened edits release $r: no edited input in $RI/$r"
  for bvar in BLEND_REOPENED_ANIMATED_BEFORE BLEND_REOPENED_CONSTRAINED_BEFORE; do
    eval "bdir=\$$bvar"
    bl=$(grep -c . "$bdir/$r/values.tsv" 2>/dev/null || true)
    [ "${bl:-0}" -gt 0 ] || fail "reopened edits release $r: the report before ($bvar) is missing or empty"
  done
  rrows=$((rrows + rv)); rsaves=$((rsaves + rs)); rlogs=$((rlogs + rl)); rinputs=$((rinputs + ri))
  sbrows=$((sbrows + sv))
  sbsaves=$((sbsaves + ss))
  rows=$((rows + v))
  saves=$((saves + s))
  brows=$((brows + bv))
  builds=$((builds + b))
done
LOG=$(mktemp)
SLOG=$(mktemp)
RLOG=$(mktemp)
trap 'rm -f "$LOG" "$SLOG" "$RLOG"' EXIT
rc=0
cargo test -q --locked -p k2-mpc-blend --test roundtrip -- --ignored --exact --nocapture the_program_reads_our_writes_and_keeps_them > "$LOG" 2>&1 || rc=$?
cat "$LOG"
[ "$rc" -eq 0 ] || fail "the round-trip test failed (exit $rc)"
grep -q '^running 1 test' "$LOG" || fail "the round-trip test ran no test"
checked=$(grep -c '^round trip [^ ]*: [1-9][0-9]* objects seen, values held$' "$LOG" || true)
[ "${checked:-0}" -eq "$n" ] || fail "the test checked ${checked:-0} of $n releases with objects seen"
bchecked=$(grep -c '^round trip [^ ]*: [1-9][0-9]* objects of the program.s builds seen, read as it reads them$' "$LOG" || true)
[ "${bchecked:-0}" -eq "$n" ] || fail "the test read the builds of ${bchecked:-0} of $n releases"
rc=0
cargo test -q --locked -p k2-mpc-blend --test roundtrip -- --ignored --exact --nocapture the_program_reopens_our_writes_of_its_own_saves > "$SLOG" 2>&1 || rc=$?
cat "$SLOG"
[ "$rc" -eq 0 ] || fail "the own saves test failed (exit $rc)"
grep -q '^running 1 test' "$SLOG" || fail "the own saves test ran no test"
schecked=$(grep -c '^own saves [^ ]*: [1-9][0-9]* files checked, values held$' "$SLOG" || true)
[ "${schecked:-0}" -eq "$n" ] || fail "the own saves test checked ${schecked:-0} of $n releases"
rc=0
cargo test -q --locked -p k2-mpc-blend --test roundtrip -- --ignored --exact --nocapture the_program_reopens_our_animated_and_constrained_edits > "$RLOG" 2>&1 || rc=$?
cat "$RLOG"
[ "$rc" -eq 0 ] || fail "the reopened edits test failed (exit $rc)"
grep -q '^running 1 test' "$RLOG" || fail "the reopened edits test ran no test"
rchecked=$(grep -c '^reopened edits [^ ]*: [1-9][0-9]* files checked, values held$' "$RLOG" || true)
[ "${rchecked:-0}" -eq "$n" ] || fail "the reopened edits test checked ${rchecked:-0} of $n releases"
echo "round trip: ok ($checked releases checked, $rows objects in the program's reports, $saves program saves; builds: $brows lines, $builds files; own saves: $schecked releases, $sbrows lines, $sbsaves saves; reopened edits: $rchecked releases, $rrows lines, $rsaves saves, $rlogs logs, $rinputs inputs)"
