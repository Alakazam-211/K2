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
#    objects; a release not checked; the test failed.
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
done
echo "test result: ok. 1 passed; 0 failed"
EOF
chmod +x "$T/bin/cargo"
export RELEASES

# fill DIR: each release's folder, report and saves
fill() {
  rm -rf "$1"
  for r in $RELEASES; do
    mkdir -p "$1/$r/resaved"
    printf 'patched_all.blend\tProbe Renamed\tMESH\n' > "$1/$r/values.tsv"
    : > "$1/$r/resaved/patched_all.blend"
  done
}
first_release() { set -- $RELEASES; echo "$1"; }
R1=$(first_release)

# part STANDIN DIR (DIR "-" for none): runs the part; sets RC and OUT
part() {
  set +e
  if [ "$2" = - ]; then
    OUT=$(cd "$ROOT" && env -u BLEND_ROUNDTRIP_DIR PATH="$T/bin:$PATH" STANDIN="$1" sh scripts/roundtrip.sh 2>&1)
  else
    OUT=$(cd "$ROOT" && env PATH="$T/bin:$PATH" STANDIN="$1" BLEND_ROUNDTRIP_DIR="$2" sh scripts/roundtrip.sh 2>&1)
  fi
  RC=$?
  set -e
}
# refused NAME WHAT: exit 1 and a FAIL line holding WHAT
refused() {
  if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -q "^round trip: FAIL: .*$2"; then
    ok "plant $1: refused"
  else
    fail "plant $1: not refused naming \"$2\" (exit $RC)"; printf '%s\n' "$OUT" | sed 's/^/  /'
  fi
}

D=$T/rt
fill "$D"
part ok "$D"
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUT" | grep -q "^round trip: ok ($NREL releases checked, $NREL objects in the program's reports, $NREL program saves)$"; then
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
echo "test-roundtrip: ok ($CHECKS checks: the gate names the part, the control passes, 8 plants refused by the part, 2 by check-targets)"
