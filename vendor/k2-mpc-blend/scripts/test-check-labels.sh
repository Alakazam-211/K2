#!/bin/sh
# The self-test of scripts/check-labels.sh's commit-ref rule (its header says what a commit ref
# is). A copy of the files git tracks (what the check reads; never build output or a job's
# untracked files), made a git checkout of its own, is checked clean first (the control). Then one
# plant at a time, each undone before the next: a refused plant must fail the check (exit 1)
# with a commit-ref finding on the planted line and no other finding; an allowed one must leave
# the check clean. The hashes are made when the test runs, so this file names none.
# POSIX sh, git, tar, sed and grep.
#
#   scripts/test-check-labels.sh     exit 0: the control clean, every plant as it should be; 1: not
set -eu
LC_ALL=C; export LC_ALL
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
C=$T/tree
FAILS=0; PLANTS=0
fail() { echo "test-check-labels: FAIL: $*"; FAILS=$((FAILS + 1)); }

mkdir -p "$C"
(cd "$ROOT" && git -c core.quotePath=false ls-files -c \
  | while IFS= read -r f; do [ -e "$f" ] && printf '%s\n' "$f"; done | tar -cf - -T -) | (cd "$C" && tar -xf -)
[ -f "$C/scripts/check-labels.sh" ] || { echo "test-check-labels: the tree wasn't copied from $ROOT" >&2; exit 2; }
(cd "$C" && git init -q && git add -A) || { echo "test-check-labels: no git checkout of the copy" >&2; exit 2; }

# hashes made here: an abbreviated one with letters, one of digits only, a full one
H1=$(printf '%s%s' e3015 a9d)
H2=$(printf '%s%s' 5800 9604)
H3=$(printf 'a1%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20)
SHA256=$(printf '0f%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32)

run() {
  set +e
  OUTPUT=$(sh "$C/scripts/check-labels.sh" "$C" 2>&1)
  RC=$?
  set -e
}
show() { printf '%s\n' "$OUTPUT" | sed 's|^|  |'; }

# plant FILE TEXT: TEXT as a new last line of FILE (kept first; tracked in the copy)
plant() { cp "$C/$1" "$T/kept"; KEPT=$1; printf '%s\n' "$2" >> "$C/$1"; LINE=$(wc -l < "$C/$1" | tr -d ' '); }
undo() { cp "$T/kept" "$C/$KEPT"; }

# refused NAME WORD: exit 1, a commit-ref finding for WORD on the planted line, nothing else
refused() {
  PLANTS=$((PLANTS + 1))
  want="check-labels: $KEPT:$LINE: commit ref ($2)"
  others=$(printf '%s\n' "$OUTPUT" | grep '^check-labels: ' | grep -vF -- "$want" | grep -v 'finding(s)$' || true)
  if [ "$RC" -ne 1 ] || ! printf '%s\n' "$OUTPUT" | grep -qxF -- "$want"; then
    fail "$1: not refused with \"$want\" (exit $RC)"; show
  elif [ -n "$others" ]; then
    fail "$1: another finding besides the plant's"; printf '%s\n' "$others" | sed 's|^|  |'
  else
    echo "=== plant $1: refused (exit 1): $want"
  fi
}

# allowed NAME: the check stays clean
allowed() {
  PLANTS=$((PLANTS + 1))
  if [ "$RC" -eq 0 ] && printf '%s\n' "$OUTPUT" | grep -q '^check-labels: clean'; then
    echo "=== plant $1: allowed (clean)"
  else
    fail "$1: refused, and it should pass (exit $RC)"; show
  fi
}

# the control: the tree as it is
run
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUTPUT" | grep -q '^check-labels: clean'; then
  echo "=== control: $(printf '%s\n' "$OUTPUT" | grep '^check-labels: clean')"
else
  fail "control: not clean (exit $RC)"; show
fi

# refused: a bare hash in Markdown prose, in a code span, digits only, full length; in a Rust
# comment; in a shell comment after code
plant README.md "Built on the kit at $H1."; run; refused md-prose "$H1"; undo
plant README.md "Needs kit \`$H1\` or later."; run; refused md-code-span "$H1"; undo
plant CHANGELOG.md "- Measured at $H2 on a build node."; run; refused md-digits-only "$H2"; undo
plant README.md "The tree at $H3 holds it."; run; refused md-full-hash "$H3"; undo
plant crates/blend/src/lib.rs "// before it ($H1) the save was settled late"; run; refused rs-comment "$H1"; undo
plant scripts/check.sh "echo done # as of $H2"; run; refused sh-comment "$H2"; undo

# allowed: a sha256 checksum, a decimal and a 0x constant, a hash in code (not a comment), and
# a released tag's hash the allowlist names
plant PROVENANCE.md "- sample.blend: sha256 $SHA256"; run; allowed md-sha256; undo
plant crates/blend/src/lib.rs "// 0.70710677 and 0xFD2F_B528 are numbers"; run; allowed rs-numbers; undo
plant crates/blend/src/lib.rs "const _PLANTED: &str = \"$H1\";"; run; allowed rs-code; undo
cp "$C/scripts/commit-refs-allow.txt" "$T/kept-allow"
printf '%s  # v0.0.0, planted\n' "$H1" >> "$C/scripts/commit-refs-allow.txt"
plant README.md "Released at $H1."; run; allowed md-allowlisted; undo
cp "$T/kept-allow" "$C/scripts/commit-refs-allow.txt"

# outside a git checkout (a node job's copy): the files under ROOT are read, so a plant there is
# still found; a folder with no file fails closed (exit 2), never "clean"
N=$T/nogit
mkdir -p "$N"
(cd "$C" && tar -cf - --exclude=.git .) | (cd "$N" && tar -xf -)
printf '%s\n' "Built on the kit at $H1." >> "$N/README.md"
set +e; OUT=$(sh "$N/scripts/check-labels.sh" "$N" 2>&1); RC=$?; set -e
PLANTS=$((PLANTS + 1))
if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -q '^check-labels: README.md:'; then
  echo "test-check-labels: ok: plant no-git: refused, the copy's files read"
else
  fail "plant no-git: not refused outside a git checkout (exit $RC)"; printf '%s\n' "$OUT" | sed 's/^/  /'
fi
E=$T/empty
mkdir -p "$E"
set +e; OUT=$(sh "$C/scripts/check-labels.sh" "$E" 2>&1); RC=$?; set -e
PLANTS=$((PLANTS + 1))
if [ "$RC" -eq 2 ] && printf '%s\n' "$OUT" | grep -q 'no files to read .*(fail closed)'; then
  echo "test-check-labels: ok: plant no-files: fails closed"
else
  fail "plant no-files: not failed closed (exit $RC)"; printf '%s\n' "$OUT" | sed 's/^/  /'
fi

if [ "$FAILS" -eq 0 ] && [ "$PLANTS" -eq 12 ]; then
  echo "test-check-labels: ok (the control clean; $PLANTS plants: 7 refused, each with its own finding alone, 4 allowed, 1 failed closed)"
  exit 0
fi
echo "test-check-labels: $FAILS failures ($PLANTS plants checked)"
exit 1
