#!/bin/sh
# The self-test of scripts/check-targets.sh (the kit's tools/test-check-targets.sh, for a pack:
# the same in every pack). A copy of the tree (the files git tracks and the untracked ones it
# doesn't ignore; no build output) is checked clean first (the control), then one plant at a time,
# each restored before the next: the check must fail (exit 1) with the plant's finding, naming
# it, and no finding of another kind. A plant whose edit didn't apply fails the test. The plants
# go in the first workspace member, and the unselected one reuses its first integration-test row.
# Landing mode (CHECK_TARGETS_LANDING=1): a planted pending row is allowed in the normal mode and
# is a finding in landing mode, where every finding is a pending one.
# POSIX sh, git, tar, sed, grep and cmp; python3 for the check itself and to read the manifests.
#
#   scripts/test-check-targets.sh     exit 0: the control clean, every plant refused; 1: not
set -eu
LC_ALL=C; export LC_ALL
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
CHECK=$HERE/check-targets.sh
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
C=$T/tree
FAILS=0; PLANTS=0
fail() { echo "test-check-targets: FAIL: $*"; FAILS=$((FAILS + 1)); }

mkdir -p "$C"
(cd "$ROOT" && git -c core.quotePath=false ls-files -c -o --exclude-standard \
  | while IFS= read -r f; do [ -e "$f" ] && printf '%s\n' "$f"; done | tar -cf - -T -) | (cd "$C" && tar -xf -)
[ -f "$C/scripts/targets.txt" ] && [ -f "$C/Cargo.toml" ] \
  || { echo "test-check-targets: the tree wasn't copied from $ROOT" >&2; exit 2; }
KINDS='new|stale|unselected|feature|ignore|pending|format|unreadable'

# the first workspace member: its directory and package name
set -- $(python3 -I - "$C" <<'PY'
import glob, os, sys, tomllib
root = sys.argv[1]
with open(os.path.join(root, "Cargo.toml"), "rb") as f:
    members = tomllib.load(f)["workspace"]["members"]
d = sorted(glob.glob(os.path.join(root, members[0])))[0]
with open(os.path.join(d, "Cargo.toml"), "rb") as f:
    print(os.path.relpath(d, root), tomllib.load(f)["package"]["name"])
PY
)
[ $# -eq 2 ] || { echo "test-check-targets: can't read the first workspace member" >&2; exit 2; }
DIR=$1; PKG=$2
# its first integration test with a row of no features, and that row's first part
ROW=$(grep -m1 "^$PKG|test|[A-Za-z0-9_]*||[a-z]" "$C/scripts/targets.txt" || true)
TEST=$(printf '%s\n' "$ROW" | cut -d'|' -f3)
[ -n "$TEST" ] && [ -f "$C/$DIR/tests/$TEST.rs" ] \
  || { echo "test-check-targets: $PKG has no integration-test row with no features in scripts/targets.txt" >&2; exit 2; }
PART=$(printf '%s\n' "$ROW" | cut -d'|' -f5 | cut -d, -f1)
case $PART in *:*|'') echo "test-check-targets: $PKG's $TEST row names no part: $ROW" >&2; exit 2 ;; esac
echo "=== plants in $PKG ($DIR), the unselected one on its $TEST row (part $PART)"

# run [VAR=VALUE]: the check on the copy, in the normal mode unless VAR=VALUE sets the landing
# mode (the caller's own CHECK_TARGETS_LANDING never leaks in); sets RC and OUTPUT
run() {
  set +e
  OUTPUT=$(env CHECK_TARGETS_LANDING=0 "$@" sh "$CHECK" "$C" 2>&1)
  RC=$?
  set -e
}
show() { printf '%s\n' "$OUTPUT" | sed 's|^|  |'; }

# keep FILE / restore FILE: around each plant's edits
keep() { cp "$C/$1" "$T/kept"; KEPT=$1; }
restore() {
  cp "$T/kept" "$C/$KEPT"
  cmp -s "$T/kept" "$C/$KEPT" || { echo "test-check-targets: $KEPT not restored" >&2; exit 2; }
}

# append FILE: stdin's lines at the end of FILE (kept first)
append() { keep "$1"; cat >> "$C/$1"; }

# refused NAME KIND WHAT: exit 1, a KIND finding holding WHAT, and no finding of another kind
refused() {
  PLANTS=$((PLANTS + 1))
  hit=$(printf '%s\n' "$OUTPUT" | grep -E "^check-targets: $2: " | grep -F -- "$3" || true)
  if [ "$RC" -ne 1 ] || [ -z "$hit" ]; then
    fail "$1: not refused with a $2 finding naming \"$3\" (exit $RC)"; show; return 0
  fi
  other=$(printf '%s\n' "$OUTPUT" | grep -E "^check-targets: ($KINDS): " | grep -vE "^check-targets: $2: " || true)
  if [ -n "$other" ]; then
    fail "$1: another kind of finding besides the plant's"; printf '%s\n' "$other" | sed 's|^|  |'; return 0
  fi
  echo "=== plant $1: refused (exit 1): $(printf '%s\n' "$hit" | sed -n 1p)"
}

# the control: the tree as it is
run
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUTPUT" | grep -q '^check-targets: clean'; then
  echo "=== control: clean: $(printf '%s\n' "$OUTPUT" | grep '^check-targets: clean')"
else
  fail "control: not clean (exit $RC)"; show
fi
PENDING=$(printf '%s\n' "$OUTPUT" | sed -n 's/^check-targets: clean .* \([0-9]*\) pending)$/\1/p')
# landing mode on the control: clean when it has no pending row (as a change must land)
if [ "$PENDING" = 0 ]; then
  run CHECK_TARGETS_LANDING=1
  if [ "$RC" -eq 0 ] && printf '%s\n' "$OUTPUT" | grep -q '^check-targets: clean'; then
    echo "=== control, landing mode: clean: $(printf '%s\n' "$OUTPUT" | grep '^check-targets: clean')"
  else
    fail "control, landing mode: not clean with no pending row (exit $RC)"; show
  fi
fi

# (a) new: a test target with no row
printf '#[test]\nfn planted() {}\n' > "$C/$DIR/tests/zz_planted_target.rs"
run; refused new-target new "$PKG|test|zz_planted_target"
rm "$C/$DIR/tests/zz_planted_target.rs"

# (b) stale: a row for no target
append scripts/targets.txt <<EOF
$PKG|test|zz_no_such_target||$PART
EOF
run; refused stale-row stale "$PKG|test|zz_no_such_target"
restore

# (c) unselected: a row naming a part that doesn't run the target with its features
append scripts/targets.txt <<EOF
$PKG|test|$TEST|zz-planted|$PART
EOF
run; refused unselected-part unselected "$PKG|test|$TEST|zz-planted|$PART"
restore

# (d) feature: a test-only feature whose row names a part that doesn't enable it; when that part
# turns on every feature (--all-features), a row that names no part
FPARTS=$PART
PFILE=$(sed -n 's/^PARTS="\(.*\)"$/\1/p' "$C/scripts/check-targets.sh" | tr ' ' '\n' | sed -n "s|^$PART=||p")
[ -n "$PFILE" ] && [ -f "$C/$PFILE" ] || { echo "test-check-targets: no script for part $PART" >&2; exit 2; }
if grep -v '^[[:space:]]*#' "$C/$PFILE" | grep -q -- '--all-features'; then FPARTS=,; fi
keep "$DIR/Cargo.toml"
if grep -q '^\[features\]$' "$T/kept"; then
  sed '/^\[features\]$/a\
planted-zz = []' "$T/kept" > "$C/$DIR/Cargo.toml"
else
  { cat "$T/kept"; printf '\n[features]\nplanted-zz = []\n'; } > "$C/$DIR/Cargo.toml"
fi
if cmp -s "$T/kept" "$C/$DIR/Cargo.toml"; then fail "feature: the plant didn't change $DIR/Cargo.toml"
else
  cp "$C/scripts/targets.txt" "$T/kept-targets"
  echo "$PKG|feature|planted-zz||$FPARTS" >> "$C/scripts/targets.txt"
  run; refused feature-unenabled feature "$PKG|feature|planted-zz||$FPARTS"
  cp "$T/kept-targets" "$C/scripts/targets.txt"
fi
restore

# (e) ignore: an #[ignore] whose reason names no runner
append "$DIR/tests/$TEST.rs" <<'EOF'

#[test]
#[ignore = "a measurement: run in release"]
fn zz_planted_ignore() {}
EOF
run; refused ignore-no-runner ignore 'zz_planted_ignore'
restore

# landing mode: a pending row is allowed, then a finding
append "$DIR/tests/$TEST.rs" <<'EOF'

#[test]
#[ignore = "a measurement: run in release"]
fn zz_planted_pending() {}
EOF
cp "$C/scripts/targets.txt" "$T/kept-targets"
echo "$PKG|ignore|tests/$TEST.rs::zz_planted_pending||pending ruling: planted" >> "$C/scripts/targets.txt"
run
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUTPUT" | grep -q '^check-targets: clean'; then
  echo "=== plant pending-normal: allowed in the normal mode: $(printf '%s\n' "$OUTPUT" | grep '^check-targets: clean')"
else
  fail "pending-normal: a pending row refused in the normal mode (exit $RC)"; show
fi
run CHECK_TARGETS_LANDING=1; refused pending-landing pending "tests/$TEST.rs::zz_planted_pending"
n=$(printf '%s\n' "$OUTPUT" | grep -cE '^check-targets: pending: ' || true)
if [ -n "$PENDING" ] && [ "$n" -eq $((PENDING + 1)) ]; then
  echo "=== landing mode: $n pending findings: the control's $PENDING and the plant's"
else
  fail "landing mode: $n pending findings, want the control's ${PENDING:-?} and the plant's"
fi
cp "$T/kept-targets" "$C/scripts/targets.txt"
restore

if [ "$FAILS" -eq 0 ] && [ "$PLANTS" -eq 6 ]; then
  echo "test-check-targets: ok (the control clean; $PLANTS plants refused, each with its own finding alone)"
  exit 0
fi
echo "test-check-targets: $FAILS failures ($PLANTS plants checked)"
exit 1
