#!/usr/bin/env bash
# Which k2-node tests run on which kind of Mac compute node.
#
# A K2 Compute job on a Mac runs inside a sandbox, and macOS refuses a
# sandbox inside a sandbox. The k2-node test files that apply one
# themselves are listed in crates/k2-node/tests/mac-wall-tests.txt; they
# run on the Mac node without a job wall, and all the others on the walled
# nodes. Nothing is skipped inside a test: the split is in the filter the
# job is started with.
#
#   k2-node-mac-tests.sh filter walled     the nextest filter for a walled Mac node
#   k2-node-mac-tests.sh filter unwalled   the nextest filter for the Mac node with no wall
#   k2-node-mac-tests.sh check             the list matches the test files; exit 1 with the reason if not
#
# Use: cargo nextest run -p k2-node -E "$(scripts/ci/k2-node-mac-tests.sh filter walled)"
# The two filters are each other's complement, so every test is in
# exactly one of them.
#
# For tests: --root DIR works on another tree.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --root) ROOT="$2"; shift ;;
        *) args+=("$1") ;;
    esac
    shift
done
TESTS="$ROOT/crates/k2-node/tests"
LIST="$TESTS/mac-wall-tests.txt"
# What a test file that applies a sandbox itself says somewhere.
MARKS='Wall::Seatbelt|SANDBOX_EXEC|sandbox-exec|seatbelt_argv|seatbelt_profile|seatbelt_tenant_profile'

[ -f "$LIST" ] || { echo "k2-node-mac-tests: no list at $LIST" >&2; exit 2; }
names=()
while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in ''|'#'*) continue ;; esac
    name="${line%%[[:space:]]*}"
    why="${line#"$name"}"
    why="${why#"${why%%[![:space:]]*}"}"
    case "$name" in *[!a-z0-9_]*) echo "k2-node-mac-tests: '$name' is not a test file name (letters, digits, _)" >&2; exit 2 ;; esac
    [ -n "$why" ] || { echo "k2-node-mac-tests: $name has no reason after its name" >&2; exit 2; }
    for seen in ${names[@]+"${names[@]}"}; do
        [ "$seen" != "$name" ] || { echo "k2-node-mac-tests: $name is listed twice" >&2; exit 2; }
    done
    names+=("$name")
done < "$LIST"
[ "${#names[@]}" -gt 0 ] || { echo "k2-node-mac-tests: the list names no test file" >&2; exit 2; }

listed="binary(${names[0]})"
for name in "${names[@]:1}"; do listed="$listed | binary($name)"; done

case "${args[0]-} ${args[1]-}" in
    "filter unwalled") [ "${#args[@]}" = 2 ] || { echo "k2-node-mac-tests: too many arguments" >&2; exit 2; }
        echo "$listed" ;;
    "filter walled") [ "${#args[@]}" = 2 ] || { echo "k2-node-mac-tests: too many arguments" >&2; exit 2; }
        echo "not ($listed)" ;;
    "check ") bad=0
        for name in "${names[@]}"; do
            if [ ! -f "$TESTS/$name.rs" ]; then
                echo "k2-node-mac-tests: $name is listed, but crates/k2-node/tests/$name.rs does not exist: its tests would run nowhere under that name. Fix or remove the line." >&2
                bad=1
            fi
        done
        for f in "$TESTS"/*.rs; do
            [ -e "$f" ] || continue
            name="$(basename "$f" .rs)"
            is=0
            for seen in "${names[@]}"; do [ "$seen" != "$name" ] || is=1; done
            if [ "$is" = 0 ] && grep -Eq "$MARKS" "$f"; then
                echo "k2-node-mac-tests: crates/k2-node/tests/$name.rs applies a macOS sandbox ($(grep -Eo "$MARKS" "$f" | head -1)) but is not in mac-wall-tests.txt: it would fail on a walled Mac node. Add it with its reason." >&2
                bad=1
            fi
        done
        [ "$bad" = 0 ] || exit 1
        echo "k2-node-mac-tests: ok (${#names[@]} test files run on the Mac node with no wall)" ;;
    *) echo "usage: k2-node-mac-tests.sh [--root DIR] filter walled|unwalled | check" >&2; exit 2 ;;
esac
