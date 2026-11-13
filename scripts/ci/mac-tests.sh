#!/usr/bin/env bash
# The macOS test run, as one K2 Compute job on a Mac node:
#
#   k2 compute run k2n1-m4 --sha <sha> --os macos --cpus 7 --mem 8G \
#       --stall-timeout off --needs rust@<pin>,cmake@3,cargo-nextest \
#       -- bash scripts/ci/mac-tests.sh
#
# What runs, in one nextest run: all of k2-core and k2-ptyd, and k2-node
# without the test files that apply a macOS sandbox themselves (a compute
# job already runs inside one; see scripts/ci/k2-node-mac-tests.sh, whose
# `filter walled` is used here and whose `check` runs first).
#
# It prints the counts, names every failed test, and exits non-zero when
# any test failed, when a step before the tests failed, or when the run
# printed no summary. The whole log stays in ./mac-tests.log.
#
# It refuses to run outside a compute job: cargo never runs on a person's
# Mac (the full k2-core tests ask for Touch ID and the keychain there).
#
# The Mac nodes have 8 GB: debug info is off and cargo builds two crates
# at a time unless CARGO_BUILD_JOBS says otherwise.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
[ $# -eq 0 ] || { echo "mac-tests: takes no arguments" >&2; exit 2; }
[ "$(uname -s)" = Darwin ] || { echo "mac-tests: this is the macOS run; this machine is $(uname -s)" >&2; exit 2; }
[ -n "${K2_JOB_CPUS:-}" ] || { echo "mac-tests: not in a K2 Compute job (no K2_JOB_CPUS). Start it with k2 compute run on a Mac node; cargo never runs on a person's Mac." >&2; exit 2; }

export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
LOG="$ROOT/mac-tests.log"

echo "mac-tests: checking the k2-node Mac wall list"
bash scripts/ci/k2-node-mac-tests.sh check
listed="$(bash scripts/ci/k2-node-mac-tests.sh filter unwalled)"
# Everything in the three crates, but not k2-node's listed test files.
filter="not (package(k2-node) & ($listed))"

echo "mac-tests: cargo nextest run -p k2-core -p k2-ptyd -p k2-node -E '$filter'"
rc=0
cargo nextest run --locked --no-fail-fast -p k2-core -p k2-ptyd -p k2-node -E "$filter" > "$LOG" 2>&1 || rc=$?

summary="$(grep -E '^ +Summary \[' "$LOG" | tail -1 || true)"
if [ -z "$summary" ]; then
    echo "mac-tests: FAILED: no test summary (cargo exit $rc). The end of the log:" >&2
    tail -40 "$LOG" >&2
    exit 1
fi
count() { echo "$summary" | sed -nE "s/.*[^0-9]([0-9]+) $1.*/\1/p"; }
run="$(count 'tests? run')"; passed="$(count passed)"; failed="$(count failed)"; skipped="$(count skipped)"
echo "mac-tests: ${run:-?} run, ${passed:-0} passed, ${failed:-0} failed, ${skipped:-0} skipped"
if [ "$rc" != 0 ] || [ "${failed:-0}" != 0 ]; then
    echo "mac-tests: FAILED (cargo exit $rc). Failed tests:" >&2
    grep -E '^ +(FAIL|SIGABRT|SIGSEGV|TIMEOUT|LEAK-FAIL)' "$LOG" | sed -E 's/^ +//; s/\[ *[0-9.]+s\] \([ 0-9/]+\) //' | sort -u >&2 || true
    exit 1
fi
[ -n "$run" ] && [ "$run" = "$passed" ] || { echo "mac-tests: FAILED: the counts don't add up: $summary" >&2; exit 1; }
echo "mac-tests: ok"
