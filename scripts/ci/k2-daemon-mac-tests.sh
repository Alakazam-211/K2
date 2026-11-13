#!/usr/bin/env bash
# Which k2-daemon tests run on which kind of Mac compute node.
#
# A K2 Compute job on a Mac runs inside a sandbox that refuses
# /usr/bin/login, so a test that starts a login shell can only fail there.
# Those tests are listed by name in
# crates/k2-daemon/tests/mac-unwalled-tests.txt; they run on the Mac node
# without a job wall, and every other k2-daemon test on the walled nodes.
# Nothing is skipped inside a test: the split is in the filter the run is
# started with. (k2-node's wall tests have their own list, by file:
# scripts/ci/k2-node-mac-tests.sh.)
#
#   k2-daemon-mac-tests.sh filter walled     the nextest filter for a walled Mac node
#   k2-daemon-mac-tests.sh filter unwalled   the nextest filter for the Mac node with no wall
#   k2-daemon-mac-tests.sh check             the list matches the test sources; exit 1 with the reason if not
#
# The two filters are each other's complement, so every test is in
# exactly one of them. `check` fails when a listed test no longer exists
# under that name (it would run nowhere), or is listed twice. It runs
# without cargo: it looks for the test's function in the file its name
# points at.
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
CRATE="$ROOT/crates/k2-daemon"
LIST="$CRATE/tests/mac-unwalled-tests.txt"

[ -f "$LIST" ] || { echo "k2-daemon-mac-tests: no list at $LIST" >&2; exit 2; }
bins=(); names=()
while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in ''|'#'*) continue ;; esac
    # shellcheck disable=SC2086 # split the line into its three parts on purpose
    set -- $line
    [ $# -ge 3 ] || { echo "k2-daemon-mac-tests: a line is '<binary> <test name> <why>', not: $line" >&2; exit 2; }
    bin="$1"; name="$2"
    case "$bin" in *[!a-z0-9_]*) echo "k2-daemon-mac-tests: '$bin' is not a test binary name (letters, digits, _), in: $line" >&2; exit 2 ;; esac
    case "$name" in *[!A-Za-z0-9_:]*|:*|*:|*:::*) echo "k2-daemon-mac-tests: '$name' is not a test name, in: $line" >&2; exit 2 ;; esac
    for i in ${names[@]+"${!names[@]}"}; do
        if [ "${bins[$i]}" = "$bin" ] && [ "${names[$i]}" = "$name" ]; then
            echo "k2-daemon-mac-tests: $bin $name is listed twice" >&2; exit 2
        fi
    done
    bins+=("$bin"); names+=("$name")
done < "$LIST"
[ "${#names[@]}" -gt 0 ] || { echo "k2-daemon-mac-tests: the list names no test" >&2; exit 2; }

# One term per test: the crate's own tests are `kind(lib)`, the others a
# test binary by name.
term() {
    if [ "$1" = lib ]; then echo "(package(k2-daemon) & kind(lib) & test(=$2))"; else echo "(package(k2-daemon) & binary($1) & test(=$2))"; fi
}
listed="$(term "${bins[0]}" "${names[0]}")"
for i in "${!names[@]}"; do
    [ "$i" = 0 ] || listed="$listed | $(term "${bins[$i]}" "${names[$i]}")"
done

# The files a test's name points at. `a::b::tests::f` in the crate's own
# tests: src/a/b.rs, src/a/b_tests.rs, src/a/b/tests.rs, src/a/b/mod.rs.
# `m::f` in a grouped binary: tests/<bin>/m.rs. `f` in a binary of one
# file: tests/<bin>.rs.
files_of() {
    local bin="$1" name="$2" path
    path="${name%::*}"; [ "$path" != "$name" ] || path=""
    if [ "$bin" = lib ]; then
        path="${path%::tests}"; path="${path//:://}"
        echo "$CRATE/src/$path.rs"; echo "$CRATE/src/${path}_tests.rs"; echo "$CRATE/src/$path/tests.rs"; echo "$CRATE/src/$path/mod.rs"
    elif [ -n "$path" ]; then
        path="${path//:://}"
        echo "$CRATE/tests/$bin/$path.rs"; echo "$CRATE/tests/$bin/$path/mod.rs"
    else
        echo "$CRATE/tests/$bin.rs"; echo "$CRATE/tests/$bin/main.rs"
    fi
}

case "${args[0]-} ${args[1]-}" in
    "filter unwalled") [ "${#args[@]}" = 2 ] || { echo "k2-daemon-mac-tests: too many arguments" >&2; exit 2; }
        echo "$listed" ;;
    "filter walled") [ "${#args[@]}" = 2 ] || { echo "k2-daemon-mac-tests: too many arguments" >&2; exit 2; }
        echo "not ($listed)" ;;
    "check ") bad=0
        for i in "${!names[@]}"; do
            fn="${names[$i]##*::}"; found=0
            while IFS= read -r f; do
                if [ -f "$f" ] && grep -Eq "^[[:space:]]*(pub(\([a-z]+\))? )?(async )?fn $fn\(" "$f"; then found=1; fi
            done < <(files_of "${bins[$i]}" "${names[$i]}")
            if [ "$found" = 0 ]; then
                echo "k2-daemon-mac-tests: ${bins[$i]} ${names[$i]} is listed, but no such test function is in the file its name points at: it would run nowhere under that name. Fix or remove the line." >&2
                bad=1
            fi
        done
        [ "$bad" = 0 ] || exit 1
        echo "k2-daemon-mac-tests: ok (${#names[@]} tests run on the Mac node with no wall)" ;;
    *) echo "usage: k2-daemon-mac-tests.sh [--root DIR] filter walled|unwalled | check" >&2; exit 2 ;;
esac
