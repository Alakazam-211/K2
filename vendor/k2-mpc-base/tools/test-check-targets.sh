#!/bin/sh
# The self-test of tools/check-targets.sh. A copy of the tree (the manifests, crates, tools and
# docs the check reads; no vendor/, no build output) is checked clean first (the control), then
# one plant at a time, each restored before the next: the check must fail (exit 1) with the
# plant's finding, naming it, and no finding of another kind. A plant whose edit didn't apply
# fails the test. Landing mode (CHECK_TARGETS_LANDING=1): a planted pending row is allowed in the
# normal mode and is a finding in landing mode, where every finding is a pending one.
# POSIX sh, tar, sed, grep and cmp; python3 for the check itself.
#
#   tools/test-check-targets.sh       exit 0: the control clean, every plant refused; 1: not
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
(cd "$ROOT" && tar -cf - --exclude ./vendor --exclude ./target --exclude ./.git --exclude ./.claude \
  --exclude ./docs/evidence/runs --exclude ./client --exclude ./spike --exclude '*/target' . ) | (cd "$C" && tar -xf -)
[ -f "$C/tools/targets.txt" ] && [ -f "$C/crates/base/Cargo.toml" ] \
  || { echo "test-check-targets: the tree wasn't copied from $ROOT" >&2; exit 2; }
KINDS='new|stale|unselected|feature|ignore|pending|format|unreadable'

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
printf '#[test]\nfn planted() {}\n' > "$C/crates/base/tests/zz_planted_target.rs"
run; refused new-target new 'k2-mpc-base|test|zz_planted_target'
rm "$C/crates/base/tests/zz_planted_target.rs"

# (b) stale: a row for no target
append tools/targets.txt <<'EOF'
k2-mpc-base|test|zz_no_such_target||base-a2
EOF
run; refused stale-row stale 'k2-mpc-base|test|zz_no_such_target'
restore

# (c) unselected: a row naming a part that doesn't run the target with its features
grep -q '^k2-mpc-base|test|roster||' "$C/tools/targets.txt" || fail "unselected: no roster row (a stale pattern)"
append tools/targets.txt <<'EOF'
k2-mpc-base|test|roster|differential|base-c
EOF
run; refused unselected-part unselected 'k2-mpc-base|test|roster|differential|base-c'
restore

# (d) feature: a test-only feature whose row names a part that doesn't enable it
keep crates/base/Cargo.toml
sed '/^\[features\]$/a\
planted-zz = []' "$T/kept" > "$C/crates/base/Cargo.toml"
if cmp -s "$T/kept" "$C/crates/base/Cargo.toml"; then fail "feature: the plant didn't change crates/base/Cargo.toml (a stale pattern)"
else
  cp "$C/tools/targets.txt" "$T/kept-targets"
  echo 'k2-mpc-base|feature|planted-zz||base-a2' >> "$C/tools/targets.txt"
  run; refused feature-unenabled feature 'k2-mpc-base|feature|planted-zz||base-a2'
  cp "$T/kept-targets" "$C/tools/targets.txt"
fi
restore

# (e) ignore: an #[ignore] whose reason names no runner
append crates/base/tests/roster.rs <<'EOF'

#[test]
#[ignore = "a measurement: run in release"]
fn zz_planted_ignore() {}
EOF
run; refused ignore-no-runner ignore 'zz_planted_ignore'
restore

# landing mode: a pending row is allowed, then a finding
append crates/base/tests/roster.rs <<'EOF'

#[test]
#[ignore = "a measurement: run in release"]
fn zz_planted_pending() {}
EOF
cp "$C/tools/targets.txt" "$T/kept-targets"
echo 'k2-mpc-base|ignore|tests/roster.rs::zz_planted_pending||pending ruling: planted' >> "$C/tools/targets.txt"
run
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUTPUT" | grep -q '^check-targets: clean'; then
  echo "=== plant pending-normal: allowed in the normal mode: $(printf '%s\n' "$OUTPUT" | grep '^check-targets: clean')"
else
  fail "pending-normal: a pending row refused in the normal mode (exit $RC)"; show
fi
run CHECK_TARGETS_LANDING=1; refused pending-landing pending 'tests/roster.rs::zz_planted_pending'
n=$(printf '%s\n' "$OUTPUT" | grep -cE '^check-targets: pending: ' || true)
if [ -n "$PENDING" ] && [ "$n" -eq $((PENDING + 1)) ]; then
  echo "=== landing mode: $n pending findings: the control's $PENDING and the plant's"
else
  fail "landing mode: $n pending findings, want the control's ${PENDING:-?} and the plant's"
fi
cp "$T/kept-targets" "$C/tools/targets.txt"
restore

if [ "$FAILS" -eq 0 ] && [ "$PLANTS" -eq 6 ]; then
  echo "test-check-targets: ok (the control clean; $PLANTS plants refused, each with its own finding alone)"
  exit 0
fi
echo "test-check-targets: $FAILS failures ($PLANTS plants checked)"
exit 1
