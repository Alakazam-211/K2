#!/bin/sh
# The self-test of the gate's round-trip part (scripts/roundtrip.sh): a change can't land without
# the program's headless round trip (PROVENANCE.md), and the part can't pass having checked
# nothing. Text and shell only: a stand-in `cargo` prints what the round-trip test prints, so
# nothing is built and the program never runs here.
#
# 1. The gate names the part: scripts/check-targets.sh's PARTS holds
#    roundtrip=scripts/roundtrip.sh, the roundtrip test's row runs in part roundtrip, its
#    #[ignore] names scripts/roundtrip.sh, and scripts/check.sh runs the part in landing mode
#    (CHECK_TARGETS_LANDING=1).
# 2. The control: a round-trip folder with each release's report and saves, and the stand-in
#    printing each release's line: the part passes, naming its counts.
# 3. Plants, one at a time, each refused (exit 1, a "round trip: FAIL" line): no folder; a
#    release's folder missing; a report empty; no save; the test ran nothing; a release with 0
#    objects; a release not checked; the test failed; the builds' folder, report or saves
#    missing, or the builds unread; the own saves' folder not given or not there, a release's folder,
#    report or saves missing in it, its test failed, ran nothing, or left a release unchecked;
#    the reopened edits' folders (the program's reports, logs and re-saves; our edited inputs and
#    their record; the reports of the two builds before the edits) each not given or not there,
#    a release's folder, report, saves, logs or inputs missing in them, the record empty, and
#    that test failed, ran nothing, or left a release unchecked.
# 4. A copy of the tree without the part, and one whose PARTS drops it: scripts/check-targets.sh
#    refuses each (exit 1), naming the part.
# POSIX sh, git, tar, sed, grep and find; python3 for scripts/check-targets.sh.
#
#   scripts/test-roundtrip.sh     exit 0: every check holds; 1: not
set -eu
LC_ALL=C; export LC_ALL
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
FAILS=0; CHECKS=0
fail() { echo "test-roundtrip: FAIL: $*"; FAILS=$((FAILS + 1)); }
ok() { CHECKS=$((CHECKS + 1)); echo "test-roundtrip: ok: $*"; }

# ---- 1. the gate names the part ----
grep -q '^PARTS="[^"]*roundtrip=scripts/roundtrip.sh' "$ROOT/scripts/check-targets.sh" \
  && ok "check-targets' PARTS names roundtrip=scripts/roundtrip.sh" \
  || fail "the gate names no round-trip part (scripts/check-targets.sh PARTS)"
grep -q '^k2-mpc-blend|test|roundtrip||\(.*,\)\{0,1\}roundtrip\(,.*\)\{0,1\}$' "$ROOT/scripts/targets.txt" \
  && ok "the roundtrip test's row runs in part roundtrip" \
  || fail "the roundtrip test's row (scripts/targets.txt) doesn't run in part roundtrip"
grep -q '#\[ignore = "[^"]*scripts/roundtrip.sh' "$ROOT/crates/blend/tests/roundtrip.rs" \
  && ok "the round-trip test's #[ignore] names scripts/roundtrip.sh" \
  || fail "the round-trip test's #[ignore] doesn't name scripts/roundtrip.sh"
landing=$(sed -n '/CHECK_TARGETS_LANDING:-0}" = 1/,/^fi/p' "$ROOT/scripts/check.sh" | grep -c '^ *sh scripts/roundtrip.sh$' || true)
[ "$landing" -eq 1 ] \
  && ok "scripts/check.sh runs the part when a change lands" \
  || fail "scripts/check.sh doesn't run scripts/roundtrip.sh when a change lands"
[ -f "$ROOT/scripts/roundtrip.sh" ] || { fail "no scripts/roundtrip.sh"; echo "test-roundtrip: $FAILS failures"; exit 1; }

# ---- 2 and 3. the part, with a stand-in cargo ----
RELEASES=$(sed -n 's/^const RELEASES: &\[&str\] = &\[\(.*\)\];$/\1/p' "$ROOT/crates/blend/tests/roundtrip.rs" | tr -d '" ' | tr ',' ' ')
NREL=$(echo $RELEASES | wc -w | tr -d ' ')
[ "$NREL" -gt 0 ] || { fail "tests/roundtrip.rs names no releases"; exit 1; }
mkdir -p "$T/bin"
cat > "$T/bin/cargo" <<'EOF'
#!/bin/sh
# the reopened edits test's output, as STANDIN says
case "$*" in
  *the_program_reopens_our_animated_and_constrained_edits*)
    case $STANDIN in
      refails) echo "running 1 test"; echo "test result: FAILED. 0 passed; 1 failed"; exit 101 ;;
      renothing) echo "running 0 tests"; echo "test result: ok. 0 passed; 0 failed"; exit 0 ;;
    esac
    echo "running 1 test"
    first=1
    for r in $RELEASES; do
      if [ "$STANDIN" = reskip ] && [ $first -eq 1 ]; then first=0; continue; fi
      first=0
      echo "reopened edits $r: 4 files checked, values held"
    done
    echo "test result: ok. 1 passed; 0 failed"
    exit 0 ;;
esac
# the own saves test's output, as STANDIN says
case "$*" in
  *the_program_reopens_our_writes_of_its_own_saves*)
    case $STANDIN in
      osfails) echo "running 1 test"; echo "test result: FAILED. 0 passed; 1 failed"; exit 101 ;;
      osnothing) echo "running 0 tests"; echo "test result: ok. 0 passed; 0 failed"; exit 0 ;;
    esac
    echo "running 1 test"
    first=1
    for r in $RELEASES; do
      if [ "$STANDIN" = osskip ] && [ $first -eq 1 ]; then first=0; continue; fi
      first=0
      echo "own saves $r: 13 files checked, values held"
    done
    echo "test result: ok. 1 passed; 0 failed"
    exit 0 ;;
esac
# the round-trip test's output, as STANDIN says
case $STANDIN in
  fails) echo "running 1 test"; echo "test result: FAILED. 0 passed; 1 failed"; exit 101 ;;
  nothing) echo "running 0 tests"; echo "test result: ok. 0 passed; 0 failed"; exit 0 ;;
esac
echo "running 1 test"
first=1
for r in $RELEASES; do
  if [ "$STANDIN" = skip ] && [ $first -eq 1 ]; then first=0; continue; fi
  n=5; [ "$STANDIN" = zero ] && [ $first -eq 1 ] && n=0
  first=0
  echo "round trip $r: $n objects seen, values held"
  [ "$STANDIN" = nobuilds ] || echo "round trip $r: 3 objects of the program's builds seen, read as it reads them"
done
echo "test result: ok. 1 passed; 0 failed"
EOF
chmod +x "$T/bin/cargo"
export RELEASES

# fill DIR: each release's folder, report and saves; beside it the builds' (OUT-build), the
# own saves' (OUT-B), and the reopened edits': the program's (OUT-R), our inputs (IN-R) and the
# reports of the two builds before (BA, BC)
fill() {
  rm -rf "$1" "$1/../OUT-build" "$1/../OUT-B" "$1/../OUT-R" "$1/../IN-R" "$1/../BA" "$1/../BC"
  mkdir -p "$1/../IN-R"
  printf 'release\tfile\n' > "$1/../IN-R/PATCHES.tsv"
  for r in $RELEASES; do
    mkdir -p "$1/../OUT-R/$r/resaved" "$1/../IN-R/$r" "$1/../BA/$r" "$1/../BC/$r"
    printf 'constrained.blend\tFollower\tMESH\n' > "$1/../OUT-R/$r/values.tsv"
    : > "$1/../OUT-R/$r/resaved/constrained.blend"
    printf 'referee: scene\n' > "$1/../OUT-R/$r/constrained.blend.log"
    : > "$1/../IN-R/$r/constrained.blend"
    printf 'visibility_animated.blend\tPlain\tMESH\n' > "$1/../BA/$r/values.tsv"
    printf 'constrained.blend\tFollower\tMESH\n' > "$1/../BC/$r/values.tsv"
    mkdir -p "$1/../OUT-B/$r/resaved"
    printf 'living_room.blend\tTable\tMESH\n' > "$1/../OUT-B/$r/values.tsv"
    : > "$1/../OUT-B/$r/resaved/living_room.blend"
    mkdir -p "$1/$r/resaved"
    printf 'patched_all.blend\tProbe Renamed\tMESH\n' > "$1/$r/values.tsv"
    : > "$1/$r/resaved/patched_all.blend"
    mkdir -p "$1/../OUT-build/$r/built"
    printf 'living_room.blend\tFloor\tMESH\n' > "$1/../OUT-build/$r/values.tsv"
    : > "$1/../OUT-build/$r/built/living_room.blend"
  done
}
first_release() { set -- $RELEASES; echo "$1"; }
R1=$(first_release)

# the reopened edits' folders the part is given ("-" for none): set before a plant, put back by
# reo after it
reo() { REO_DIR=$T/OUT-R; REO_IN=$T/IN-R; REO_ANIM=$T/BA; REO_CONS=$T/BC; }
reo
# setenv NAME VALUE: exports NAME, or unsets it for "-"
setenv() { if [ "$2" = - ]; then unset "$1"; else eval "export $1=\"\$2\""; fi; }
# part STANDIN DIR [SBDIR] (DIR or SBDIR "-" for none; SBDIR defaults to the filled OUT-B), with
# the reopened edits' folders as REO_* say: runs the part; sets RC and OUT
part() {
  set +e
  OUT=$(
    cd "$ROOT" || exit 9
    export PATH="$T/bin:$PATH" STANDIN="$1"
    setenv BLEND_ROUNDTRIP_DIR "$2"
    setenv BLEND_OWN_SAVES_DIR "${3:-$T/OUT-B}"
    setenv BLEND_REOPENED_DIR "$REO_DIR"
    setenv BLEND_REOPENED_IN "$REO_IN"
    setenv BLEND_REOPENED_ANIMATED_BEFORE "$REO_ANIM"
    setenv BLEND_REOPENED_CONSTRAINED_BEFORE "$REO_CONS"
    sh scripts/roundtrip.sh 2>&1
  )
  RC=$?
  set -e
}
PLANTS=0
# refused NAME WHAT: exit 1 and a FAIL line holding WHAT
refused() {
  PLANTS=$((PLANTS + 1))
  if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -q "^round trip: FAIL: .*$2"; then
    ok "plant $1: refused"
  else
    fail "plant $1: not refused naming \"$2\" (exit $RC)"; printf '%s\n' "$OUT" | sed 's/^/  /'
  fi
}

D=$T/rt
fill "$D"
part ok "$D"
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUT" | grep -q "^round trip: ok ($NREL releases checked, $NREL objects in the program's reports, $NREL program saves; builds: $NREL lines, $NREL files; own saves: $NREL releases, $NREL lines, $NREL saves; reopened edits: $NREL releases, $NREL lines, $NREL saves, $NREL logs, $NREL inputs)$"; then
  ok "control: the part passes on a full folder ($NREL releases)"
else
  fail "control: the part doesn't pass on a full folder (exit $RC)"; printf '%s\n' "$OUT" | sed 's/^/  /'
fi
part ok -;                                         refused no-folder "BLEND_ROUNDTRIP_DIR is not set"
part ok "$T/none";                                 refused missing-folder "names no folder"
fill "$D"; rm -rf "${D:?}/$R1"; part ok "$D";      refused release-missing "release $R1: no folder"
fill "$D"; : > "$D/$R1/values.tsv"; part ok "$D"; refused report-empty "release $R1: the program's report"
fill "$D"; rm -f "$D/$R1/resaved/"*; part ok "$D"; refused no-save "release $R1: no program save"
fill "$D"; part nothing "$D";                      refused ran-nothing "the round-trip test ran no test"
part zero "$D";                                    refused zero-objects "the test checked $((NREL - 1)) of $NREL releases with objects seen"
part skip "$D";                                    refused release-unchecked "the test checked $((NREL - 1)) of $NREL releases with objects seen"
part fails "$D";                                   refused test-failed "the round-trip test failed"
fill "$D"; rm -rf "$T/OUT-build/$R1"; part ok "$D"; refused builds-missing "release $R1: the program's report of its builds"
fill "$D"; : > "$T/OUT-build/$R1/values.tsv"; part ok "$D"; refused builds-report-empty "release $R1: the program's report of its builds"
fill "$D"; rm -f "$T/OUT-build/$R1/built/"*; part ok "$D"; refused no-build "release $R1: no program build"
fill "$D"; part nobuilds "$D";                     refused builds-unread "the test read the builds of 0 of $NREL releases"
fill "$D"; part ok "$D" -;                         refused own-saves-unset "BLEND_OWN_SAVES_DIR is not set"
part ok "$D" "$T/none-b";                          refused own-saves-missing "BLEND_OWN_SAVES_DIR names no folder"
fill "$D"; rm -rf "$T/OUT-B/$R1"; part ok "$D";    refused own-saves-release-missing "own saves release $R1: no folder"
fill "$D"; : > "$T/OUT-B/$R1/values.tsv"; part ok "$D"; refused own-saves-report-empty "own saves release $R1: the program's report"
fill "$D"; rm -f "$T/OUT-B/$R1/resaved/"*; part ok "$D"; refused own-saves-no-save "own saves release $R1: no program save"
fill "$D"; part osfails "$D";                      refused own-saves-failed "the own saves test failed"
part osnothing "$D";                               refused own-saves-ran-nothing "the own saves test ran no test"
part osskip "$D";                                  refused own-saves-unchecked "the own saves test checked $((NREL - 1)) of $NREL releases"
fill "$D"; REO_DIR=-; part ok "$D"; reo;           refused reopened-unset "BLEND_REOPENED_DIR is not set"
REO_DIR=$T/none-r; part ok "$D"; reo;              refused reopened-missing "BLEND_REOPENED_DIR names no folder"
fill "$D"; rm -rf "$T/OUT-R/$R1"; part ok "$D";    refused reopened-release-missing "reopened edits release $R1: no folder"
fill "$D"; : > "$T/OUT-R/$R1/values.tsv"; part ok "$D"; refused reopened-report-empty "reopened edits release $R1: the program's report"
fill "$D"; rm -f "$T/OUT-R/$R1/resaved/"*; part ok "$D"; refused reopened-no-save "reopened edits release $R1: no program save"
fill "$D"; rm -f "$T/OUT-R/$R1/"*.log; part ok "$D"; refused reopened-no-log "reopened edits release $R1: no driver log"
fill "$D"; REO_IN=-; part ok "$D"; reo;            refused reopened-in-unset "BLEND_REOPENED_IN is not set"
REO_IN=$T/none-i; part ok "$D"; reo;               refused reopened-in-missing "BLEND_REOPENED_IN names no folder"
fill "$D"; : > "$T/IN-R/PATCHES.tsv"; part ok "$D"; refused reopened-record-empty "the record of the edits (PATCHES.tsv) is missing or empty"
fill "$D"; rm -rf "$T/IN-R/$R1"; part ok "$D";     refused reopened-no-input "reopened edits release $R1: no edited input"
fill "$D"; REO_ANIM=-; part ok "$D"; reo;          refused animated-before-unset "BLEND_REOPENED_ANIMATED_BEFORE is not set"
REO_ANIM=$T/none-a; part ok "$D"; reo;             refused animated-before-missing "BLEND_REOPENED_ANIMATED_BEFORE names no folder"
fill "$D"; : > "$T/BA/$R1/values.tsv"; part ok "$D"; refused animated-before-empty "reopened edits release $R1: the report before (BLEND_REOPENED_ANIMATED_BEFORE)"
fill "$D"; REO_CONS=-; part ok "$D"; reo;          refused constrained-before-unset "BLEND_REOPENED_CONSTRAINED_BEFORE is not set"
REO_CONS=$T/none-c; part ok "$D"; reo;             refused constrained-before-missing "BLEND_REOPENED_CONSTRAINED_BEFORE names no folder"
fill "$D"; rm -rf "$T/BC/$R1"; part ok "$D";       refused constrained-before-empty "reopened edits release $R1: the report before (BLEND_REOPENED_CONSTRAINED_BEFORE)"
fill "$D"; part refails "$D";                      refused reopened-failed "the reopened edits test failed"
part renothing "$D";                               refused reopened-ran-nothing "the reopened edits test ran no test"
part reskip "$D";                                  refused reopened-unchecked "the reopened edits test checked $((NREL - 1)) of $NREL releases"

# ---- 4. check-targets refuses a tree without the part ----
C=$T/tree
mkdir -p "$C"
(cd "$ROOT" && git -c core.quotePath=false ls-files -c -o --exclude-standard \
  | while IFS= read -r f; do [ -e "$f" ] && printf '%s\n' "$f"; done | tar -cf - -T -) | (cd "$C" && tar -xf -)
targets() {
  set +e
  OUT=$(env CHECK_TARGETS_LANDING=0 sh "$C/scripts/check-targets.sh" "$C" 2>&1)
  RC=$?
  set -e
}
mv "$C/scripts/roundtrip.sh" "$T/kept.sh"
targets
if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -q "^check-targets: .*scripts/roundtrip.sh"; then
  ok "plant part-missing: check-targets refuses it"
else
  fail "plant part-missing: check-targets doesn't refuse it (exit $RC)"; printf '%s\n' "$OUT" | sed 's/^/  /'
fi
mv "$T/kept.sh" "$C/scripts/roundtrip.sh"
sed 's| roundtrip=scripts/roundtrip.sh||' "$C/scripts/check-targets.sh" > "$T/ct.sh"
cp "$T/ct.sh" "$C/scripts/check-targets.sh"
targets
if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -q "^check-targets: unselected: .*no part roundtrip in PARTS"; then
  ok "plant part-dropped: check-targets refuses it"
else
  fail "plant part-dropped: check-targets doesn't refuse it (exit $RC)"; printf '%s\n' "$OUT" | sed 's/^/  /'
fi

if [ "$FAILS" -ne 0 ]; then
  echo "test-roundtrip: $FAILS failures ($CHECKS checks held)"
  exit 1
fi
echo "test-roundtrip: ok ($CHECKS checks: the gate names the part, the control passes, $PLANTS plants refused by the part, 2 by check-targets)"
