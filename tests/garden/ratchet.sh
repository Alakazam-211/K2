#!/usr/bin/env bash
# Tests for scripts/garden/ratchet.sh, against a small throwaway tree.
# Run: bash tests/garden/ratchet.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
RATCHET="$HERE/scripts/garden/ratchet.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0

lines() { awk -v n="$1" 'BEGIN { for (i = 0; i < n; i++) print "x" }' > "$2"; }
# run <want exit> <want text> <args...>: the exit code and the text must both match.
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(bash "$RATCHET" --root "$T" --ceilings "$T/c.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}

mkdir -p "$T/crates/a/src" "$T/crates/a/tests" "$T/src"
git -C "$T" init -q
lines 5000 "$T/crates/a/src/big.rs"
lines 100 "$T/crates/a/src/small.rs"
lines 9000 "$T/crates/a/src/big_tests.rs"
lines 9000 "$T/crates/a/tests/it.rs"
lines 9000 "$T/src/x.test.ts"
git -C "$T" add -A
printf '# a comment\n5000 crates/a/src/big.rs\n' > "$T/c.txt"

# At the ceiling passes; test files of any size are not counted.
run 0 "ratchet: ok."

# One line over fails and names the file, both counts and the way out.
lines 5001 "$T/crates/a/src/big.rs"
run 1 "crates/a/src/big.rs has 5001 lines, its ceiling is 5000 (+1). Put new code in a new file or the area file."
# --tighten never raises a ceiling: it fails too and leaves the file alone.
run 1 "its ceiling is 5000" --tighten
[ "$(cat "$T/c.txt")" = "$(printf '# a comment\n5000 crates/a/src/big.rs')" ] || { echo "FAIL: --tighten changed the ceilings on a failure" >&2; exit 1; }

# Below the ceiling passes with a note; --tighten then lowers it and keeps the comment.
lines 4500 "$T/crates/a/src/big.rs"
run 0 "1 file(s) are below their ceiling"
run 0 "crates/a/src/big.rs 5000 -> 4500" --tighten
[ "$(cat "$T/c.txt")" = "$(printf '# a comment\n4500 crates/a/src/big.rs')" ] || { echo "FAIL: ceilings after --tighten: $(cat "$T/c.txt")" >&2; exit 1; }
# Growing back to the old size now fails.
lines 5000 "$T/crates/a/src/big.rs"
run 1 "has 5000 lines, its ceiling is 4500"
lines 4500 "$T/crates/a/src/big.rs"

# An unlisted source file may not reach the limit.
lines 3999 "$T/crates/a/src/small.rs"
run 0 "ratchet: ok."
lines 4000 "$T/crates/a/src/small.rs"
run 1 "crates/a/src/small.rs has 4000 lines and is not listed"
lines 100 "$T/crates/a/src/small.rs"

# The CLI is counted by its parts: a part may not reach the limit, and the
# joined cli/k2 is not counted at all.
mkdir -p "$T/cli/src"
lines 9000 "$T/cli/k2"
lines 3999 "$T/cli/src/mail.sh"
git -C "$T" add -A
run 0 "ratchet: ok."
lines 4000 "$T/cli/src/mail.sh"
run 1 "cli/src/mail.sh has 4000 lines and is not listed"
rm -rf "$T/cli"
git -C "$T" add -A

# A listed file that is gone fails until its row is removed; --tighten removes it.
printf '4500 crates/a/src/big.rs\n4100 crates/a/src/gone.rs\n' > "$T/c.txt"
run 1 "crates/a/src/gone.rs is listed but does not exist"
run 0 "dropped crates/a/src/gone.rs (gone)" --tighten
# A listed file that fell under the limit leaves the list.
lines 3000 "$T/crates/a/src/big.rs"
run 0 "dropped crates/a/src/big.rs (3000 lines, under 4000)" --tighten
[ ! -s "$T/c.txt" ] || { echo "FAIL: ceilings should be empty: $(cat "$T/c.txt")" >&2; exit 1; }

# A held file skips both rules and is named with its lane on one line.
lines 6000 "$T/crates/a/src/big.rs"
lines 4500 "$T/crates/a/src/small.rs"
printf '5000 crates/a/src/small.rs\n\n# held\nheld crates/a/src/big.rs Some lane B2\n' > "$T/c.txt"
run 0 "held out until their lanes land, not checked: crates/a/src/big.rs (Some lane B2)"
# --tighten keeps the held row as it is.
run 0 "crates/a/src/small.rs 5000 -> 4500" --tighten
[ "$(cat "$T/c.txt")" = "$(printf '4500 crates/a/src/small.rs\n\n# held\nheld crates/a/src/big.rs Some lane B2')" ] || { echo "FAIL: held row after --tighten: $(cat "$T/c.txt")" >&2; exit 1; }
# --release moves it into the list at today's count, biggest first; then it is checked.
run 0 "released crates/a/src/big.rs into the list at 6000 lines." --release crates/a/src/big.rs
[ "$(cat "$T/c.txt")" = "$(printf '6000 crates/a/src/big.rs\n4500 crates/a/src/small.rs\n\n# held')" ] || { echo "FAIL: ceilings after --release: $(cat "$T/c.txt")" >&2; exit 1; }
lines 6001 "$T/crates/a/src/big.rs"
run 1 "crates/a/src/big.rs has 6001 lines, its ceiling is 6000"
# Releasing a file that is not held, and a held row with no lane, are errors.
run 2 "is not held" --release crates/a/src/small.rs
printf 'held crates/a/src/big.rs\n' > "$T/c.txt"
run 2 "want: held <path> <lane>"
lines 100 "$T/crates/a/src/small.rs"

# A bad row and a missing ceilings file are errors, not passes.
printf 'lots crates/a/src/big.rs\n' > "$T/c.txt"
run 2 "bad row"
rm "$T/c.txt"
run 2 "no ceilings file"

# The real tree is within its ceilings.
bash "$RATCHET" >/dev/null || { echo "FAIL: the repo is over a ceiling" >&2; bash "$RATCHET" || true; exit 1; }
pass=$((pass + 1))
echo "ratchet tests: $pass passed"
