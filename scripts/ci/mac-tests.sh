#!/usr/bin/env bash
# The macOS test run, as one K2 Compute job on a Mac node:
#
#   k2 compute run rig1-mac --sha <sha> --os macos --cpus 7 --mem 8G \
#       --stall-timeout off --needs rust@<pin>,cmake@3,cargo-nextest \
#       -- bash scripts/ci/mac-tests.sh
#
# Two stages, each with its own `mac-tests:` summary line:
#
#   core     all of k2-core and k2-ptyd, and k2-node without the test
#            files that apply a macOS sandbox themselves (a compute job
#            already runs inside one; see scripts/ci/k2-node-mac-tests.sh,
#            whose `filter walled` is used and whose `check` runs first).
#            One nextest run.
#   daemon   k2-daemon, one test binary per cargo call and one crate built
#            at a time: its test binaries are too big to link side by side
#            on an 8 GB node. Slow. The helper programs its tests start
#            (k2-ptyd and the renderers) are built first. A test target
#            behind a feature that is off by default is named, not run.
#            The tests that start a login shell (the job's sandbox refuses
#            /usr/bin/login) are left to the node with no wall, by name:
#            scripts/ci/k2-daemon-mac-tests.sh, whose `filter walled` is
#            used and whose `check` runs first.
#
#   mac-tests.sh           both stages; the second runs even if the first failed
#   mac-tests.sh core      one stage
#   mac-tests.sh daemon
#
# Each stage prints its counts, names every failed test and prints the
# panics. The script exits non-zero when any test failed, when a step
# before the tests failed, when a run printed no summary, or when counts
# don't add up. The whole logs are ./mac-tests-core.log and
# ./mac-tests-daemon.log in the job's checkout, which goes with the job.
#
# It refuses to run outside a compute job: cargo never runs on a person's
# Mac (the full k2-core tests ask for Touch ID and the keychain there).
#
# The Mac nodes have 8 GB: debug info is off, and cargo builds two crates
# at a time in `core` and one in `daemon`, unless CARGO_BUILD_JOBS says
# otherwise.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
case "$#:${1-}" in
    0:) stages="core daemon" ;;
    1:core|1:daemon) stages="$1" ;;
    *) echo "usage: mac-tests.sh [core|daemon]" >&2; exit 2 ;;
esac
[ "$(uname -s)" = Darwin ] || { echo "mac-tests: this is the macOS run; this machine is $(uname -s)" >&2; exit 2; }
[ -n "${K2_JOB_CPUS:-}" ] || { echo "mac-tests: not in a K2 Compute job (no K2_JOB_CPUS). Start it with k2 compute run on a Mac node; cargo never runs on a person's Mac." >&2; exit 2; }

export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
# Packages whose binaries k2-daemon's tests look for beside the test
# binary (each test names its own with "cargo build -p …" when missing).
DAEMON_HELPERS="k2-ptyd k2-deckrender k2-scenerender"
JOBS_SET="${CARGO_BUILD_JOBS:-}"

# One number out of a nextest summary line: `count "<line>" passed`.
count() { echo "$1" | sed -nE "s/.*[^0-9]([0-9]+) $2.*/\1/p"; }
# Every nextest result line that is not a pass: FAIL, TIMEOUT, a signal
# (SIGABRT, SIGKILL, …), ABORT, LEAK-FAIL, a failed try. Named by what it
# is not, so a status this script has never seen is still printed.
failed_lines() {
    grep -E '^ +[A-Z][A-Z0-9-]*( [0-9]+ [A-Z-]+)? +\[ *[0-9.]+s\]' "$1" \
        | grep -vE '^ +(PASS|SLOW|LEAK|SKIP|START|FLAKY|TRY [0-9]+ PASS) +\[' \
        | sed -E 's/^ +//; s/\[ *[0-9.]+s\] \([ 0-9/]+\) //' | sort -u || true
}

# Runs one nextest call into $2 and adds its counts to the stage's
# totals. Returns 1 when that call is not clean.
t_run=0; t_passed=0; t_failed=0; t_skipped=0
one_run() {
    local what="$1" log="$2" rc=0 summary run passed failed skipped timed execf named; shift 2
    cargo nextest run --locked --no-fail-fast "$@" >> "$log" 2>&1 || rc=$?
    # nextest's own word for a binary with no tests (exit 4) is not a failure.
    if [ "$rc" = 4 ] && grep -q 'no tests to run' "$log"; then return 0; fi
    summary="$(grep -E '^ +Summary \[' "$log" | tail -1 || true)"
    if [ -z "$summary" ]; then
        echo "mac-tests: FAILED: $what: no test summary (cargo exit $rc). The end of the log:" >&2
        tail -40 "$log" >&2
        return 1
    fi
    run="$(count "$summary" 'tests? run')"; passed="$(count "$summary" passed)"
    failed="$(count "$summary" failed)"; skipped="$(count "$summary" skipped)"
    # nextest counts a test that timed out, or that could not be started,
    # apart from "failed": here they are failures like any other.
    timed="$(count "$summary" 'timed out')"; execf="$(count "$summary" 'exec failed')"
    failed=$((${failed:-0} + ${timed:-0} + ${execf:-0}))
    t_run=$((t_run + ${run:-0})); t_passed=$((t_passed + ${passed:-0}))
    t_failed=$((t_failed + failed)); t_skipped=$((t_skipped + ${skipped:-0}))
    if [ "$rc" != 0 ] || [ "$failed" != 0 ]; then
        echo "mac-tests: FAILED: $what (cargo exit $rc). Failed tests:" >&2
        named="$(failed_lines "$log")"
        if [ -n "$named" ]; then
            echo "$named" >&2
            # Why: each panic with the lines after it. The job's checkout
            # (and the whole log in it) is gone once the job ends.
            if grep -q 'panicked at' "$log"; then
                echo "mac-tests: $what: the panics (each with up to 12 lines after it):" >&2
                grep -A12 'panicked at' "$log" | head -200 >&2 || true
            fi
        else
            # Never a failure with nothing to go on.
            echo "(nextest named no test) $(echo "$summary" | sed -E 's/^ +//'). The end of this call's log:" >&2
            tail -30 "$log" >&2
        fi
        return 1
    fi
    [ -n "$run" ] && [ "$run" = "$passed" ] || { echo "mac-tests: FAILED: $what: the counts don't add up: $summary" >&2; return 1; }
}

stage_core() {
    local log="$ROOT/mac-tests-core.log" listed filter
    : > "$log"
    export CARGO_BUILD_JOBS="${JOBS_SET:-2}"
    echo "mac-tests: checking the k2-node Mac wall list"
    bash scripts/ci/k2-node-mac-tests.sh check || return 1
    listed="$(bash scripts/ci/k2-node-mac-tests.sh filter unwalled)" || return 1
    # Everything in the three crates, but not k2-node's listed test files.
    filter="not (package(k2-node) & ($listed))"
    echo "mac-tests: core: cargo nextest run -p k2-core -p k2-ptyd -p k2-node -E '$filter'"
    one_run "core" "$log" -p k2-core -p k2-ptyd -p k2-node -E "$filter"
}

stage_daemon() {
    local log bad=0 targets has helper kind name need filter
    export CARGO_BUILD_JOBS="${JOBS_SET:-1}"
    : > "$ROOT/mac-tests-daemon.log"
    # The tests that start a login shell wait for the node with no wall.
    echo "mac-tests: checking the k2-daemon Mac list"
    bash scripts/ci/k2-daemon-mac-tests.sh check || return 1
    filter="$(bash scripts/ci/k2-daemon-mac-tests.sh filter walled)" || return 1
    # Every test target of k2-daemon: its library, its binaries, and each
    # integration test, as cargo itself lists them.
    targets="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c '
import json, sys
packages = json.load(sys.stdin)["packages"]
# The workspace has these packages (a helper named below may be newer
# than the tree this runs on).
print("has", " ".join(sorted(p["name"] for p in packages)), "-")
for p in packages:
    if p["name"] != "k2-daemon":
        continue
    for t in sorted(p["targets"], key=lambda t: t["name"]):
        if not t.get("test", True):
            continue
        for kind in ("lib", "bin", "test"):
            if kind in t["kind"]:
                # A target behind a feature that is off by default is not
                # in a plain build; cargo refuses to test it. It is named.
                need = ",".join(t.get("required-features") or [])
                print("off" if need else kind, t["name"], need)
')" || { echo "mac-tests: FAILED: daemon: could not list k2-daemon's test targets" >&2; return 1; }
    has=" $(echo "$targets" | sed -n 's/^has \(.*\) -$/\1/p') "
    targets="$(echo "$targets" | grep -v '^has ' || true)"
    [ -n "$targets" ] || { echo "mac-tests: FAILED: daemon: k2-daemon has no test targets" >&2; return 1; }
    echo "mac-tests: daemon: $(echo "$targets" | grep -vc '^off ') test binaries of k2-daemon, one cargo call each"
    # The helpers k2-daemon's tests start as programs of their own.
    log="$ROOT/mac-tests-daemon.log"
    for helper in $DAEMON_HELPERS; do
        case "$has" in
            *" $helper "*) ;;
            *) echo "mac-tests: daemon: this tree has no $helper package; not built"; continue ;;
        esac
        echo "mac-tests: daemon: building $helper, which the tests start as a program"
        cargo build --locked -p "$helper" >> "$log" 2>&1 || { echo "mac-tests: FAILED: daemon: cargo build -p $helper. The end of the log:" >&2; tail -40 "$log" >&2; return 1; }
    done
    while read -r kind name need; do
        log="$ROOT/mac-tests-daemon.part.log"
        : > "$log"
        case "$kind" in
            lib) one_run "daemon: the library" "$log" -p k2-daemon --lib -E "$filter" || bad=1 ;;
            bin) one_run "daemon: binary $name" "$log" -p k2-daemon --bin "$name" -E "$filter" || bad=1 ;;
            test) one_run "daemon: $name" "$log" -p k2-daemon --test "$name" -E "$filter" || bad=1 ;;
            off) echo "mac-tests: daemon: not run, its feature is off in this build: $name (needs $need)" ;;
        esac
        cat "$log" >> "$ROOT/mac-tests-daemon.log"
    done <<EOF
$targets
EOF
    rm -f "$ROOT/mac-tests-daemon.part.log"
    return "$bad"
}

red=0
for stage in $stages; do
    t_run=0; t_passed=0; t_failed=0; t_skipped=0
    ok=1
    "stage_$stage" || ok=0
    echo "mac-tests: $stage: $t_run run, $t_passed passed, $t_failed failed, $t_skipped skipped"
    if [ "$ok" = 1 ]; then echo "mac-tests: $stage: ok"; else echo "mac-tests: $stage: FAILED" >&2; red=1; fi
done
[ "$red" = 0 ] || exit 1
echo "mac-tests: ok"
