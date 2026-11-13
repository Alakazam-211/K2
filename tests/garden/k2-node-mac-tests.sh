#!/usr/bin/env bash
# Tests for scripts/ci/k2-node-mac-tests.sh on a throwaway tree, then the
# real list against the real test files.
# Run: bash tests/garden/k2-node-mac-tests.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
B="$HERE/scripts/ci/k2-node-mac-tests.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(bash "$B" --root "$T" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL ($*): exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL ($*): output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
D="$T/crates/k2-node/tests"
L="$D/mac-wall-tests.txt"
mkdir -p "$D/common"
printf 'fn a() { let w = Wall::Seatbelt; }\n' > "$D/wall_a.rs"
printf 'const X: &str = k2_node::wall::SANDBOX_EXEC;\n' > "$D/wall_b.rs"
printf 'fn plain() {}\n' > "$D/plain.rs"
# A helper folder is not a test file: what it mentions does not count.
printf 'fn h() { Wall::Seatbelt; }\n' > "$D/common/mod.rs"
printf '# why these\n\nwall_a   starts a walled node\nwall_b\tcalls sandbox-exec\n' > "$L"

run 0 "ok (2 test files run on the Mac node with no wall)" check
# The two filters are complements: one names the files, the other is its `not`.
run 0 "binary(wall_a) | binary(wall_b)" filter unwalled
run 0 "not (binary(wall_a) | binary(wall_b))" filter walled
[ "$(bash "$B" --root "$T" filter walled)" = "not ($(bash "$B" --root "$T" filter unwalled))" ] || { echo "FAIL: the filters are not complements" >&2; exit 1; }
pass=$((pass + 1))

# A test file that applies a sandbox and is not listed.
printf 'fn c() { run("sandbox-exec"); }\n' > "$D/wall_c.rs"
run 1 "crates/k2-node/tests/wall_c.rs applies a macOS sandbox (sandbox-exec) but is not in mac-wall-tests.txt" check
rm "$D/wall_c.rs"
# A listed name with no file (renamed or mistyped): its tests would run nowhere.
printf 'wall_a  x\nwall_b  y\nwall_gone  z\n' > "$L"
run 1 "wall_gone is listed, but crates/k2-node/tests/wall_gone.rs does not exist" check
# A file may be listed for a reason the marks don't show.
printf 'wall_a  x\nwall_b  y\nplain  needs a real launchd\n' > "$L"
run 0 "ok (3 test files" check
# Bad lists.
printf 'wall_a\n' > "$L"
run 2 "wall_a has no reason after its name" check
printf 'wall_a  x\nwall_a  y\n' > "$L"
run 2 "wall_a is listed twice" check
printf 'wall-a.rs  x\n' > "$L"
run 2 "'wall-a.rs' is not a test file name" check
printf '# nothing\n' > "$L"
run 2 "the list names no test file" filter walled
rm "$L"
run 2 "no list at" check
printf 'wall_a  x\nwall_b  y\n' > "$L"
run 2 "usage:" filter
run 2 "usage:" filter both
run 2 "too many arguments" filter walled extra
run 2 "usage:"

# The real list and the real test files.
out="$(bash "$B" check)" || { echo "FAIL: the real list: $out" >&2; exit 1; }
pass=$((pass + 1))
echo "k2-node-mac-tests tests: $pass passed"
