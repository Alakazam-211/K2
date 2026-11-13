#!/usr/bin/env bash
# Tests for scripts/ci/mac-tests.sh with stand-ins for cargo and uname: no
# cargo runs here. Run: bash tests/garden/mac-tests.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
T="$(mktemp -d)"
trap 'rm -rf "$T"; rm -f "$HERE/mac-tests.log"' EXIT
pass=0
mkdir -p "$T/bin"
# cargo: records its arguments, prints what $FAKE_OUT holds, exits $FAKE_RC.
cat > "$T/bin/cargo" <<'S'
#!/bin/sh
printf '%s\n' "$@" > "$FAKE_ARGS"
env | grep -E '^CARGO_(PROFILE|BUILD)' | sort > "$FAKE_ENV"
cat "$FAKE_OUT"
exit "$FAKE_RC"
S
printf '#!/bin/sh\necho "${FAKE_UNAME:-Darwin}"\n' > "$T/bin/uname"
chmod +x "$T/bin/cargo" "$T/bin/uname"
export FAKE_ARGS="$T/args" FAKE_ENV="$T/env" FAKE_OUT="$T/out" FAKE_RC=0
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(env PATH="$T/bin:$PATH" "$@" bash "$HERE/scripts/ci/mac-tests.sh" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL ($*): exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL ($*): output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
ok() { pass=$((pass + 1)); }
no() { echo "FAIL: $1" >&2; exit 1; }

# Outside a compute job, and on another system: refused before any cargo.
run 2 "not in a K2 Compute job" env -u K2_JOB_CPUS
[ ! -e "$FAKE_ARGS" ] || no "cargo ran outside a compute job"; ok
run 2 "this machine is Linux" K2_JOB_CPUS=7 FAKE_UNAME=Linux
[ ! -e "$FAKE_ARGS" ] || no "cargo ran on Linux"; ok

# A clean run: counts printed, exit 0; cargo got --locked, the three
# crates and a filter that leaves out exactly the listed k2-node files.
printf '    Starting 5473 tests across 12 binaries\n        PASS [   0.010s] (1/5473) k2-core a::b\n------------\n     Summary [ 365.974s] 5473 tests run: 5473 passed (1 slow), 3 skipped\n' > "$FAKE_OUT"
run 0 "mac-tests: 5473 run, 5473 passed, 0 failed, 3 skipped" K2_JOB_CPUS=7
args="$(tr '\n' ' ' < "$FAKE_ARGS")"
case "$args" in "nextest run --locked --no-fail-fast -p k2-core -p k2-ptyd -p k2-node -E not (package(k2-node) & (binary("*")) ") ;; *) no "cargo arguments: $args" ;; esac; ok
want="not (package(k2-node) & ($(bash "$HERE/scripts/ci/k2-node-mac-tests.sh" filter unwalled)))"
[ "$(tail -1 "$FAKE_ARGS")" = "$want" ] || no "the filter is not built from the list: $(tail -1 "$FAKE_ARGS")"; ok
grep -qx 'CARGO_BUILD_JOBS=2' "$FAKE_ENV" && grep -qx 'CARGO_PROFILE_TEST_DEBUG=0' "$FAKE_ENV" && grep -qx 'CARGO_PROFILE_DEV_DEBUG=0' "$FAKE_ENV" || no "the build settings for an 8 GB node: $(cat "$FAKE_ENV")"; ok
run 0 "mac-tests: ok" K2_JOB_CPUS=7 CARGO_BUILD_JOBS=1
grep -qx 'CARGO_BUILD_JOBS=1' "$FAKE_ENV" || no "CARGO_BUILD_JOBS from the caller was not kept"; ok

# A failed test: named, counts printed, exit 1.
printf '        FAIL [   5.127s] (3729/5473) k2-core terminal::cli_status::tests::missing_installed_and_broken\n------------\n     Summary [ 385.416s] 5473 tests run: 5470 passed (1 slow), 3 failed, 3 skipped\n        FAIL [   5.127s] (3729/5473) k2-core terminal::cli_status::tests::missing_installed_and_broken\nerror: test run failed\n' > "$FAKE_OUT"
FAKE_RC=100 run 1 "mac-tests: 5473 run, 5470 passed, 3 failed, 3 skipped" K2_JOB_CPUS=7
FAKE_RC=100 run 1 "FAIL k2-core terminal::cli_status::tests::missing_installed_and_broken" K2_JOB_CPUS=7
# The summary says failed although cargo exited 0: still a failure.
FAKE_RC=0 run 1 "FAILED (cargo exit 0)" K2_JOB_CPUS=7
# A build error: no summary at all.
printf 'error[E0425]: cannot find value `x` in this scope\nerror: could not compile `k2-core`\n' > "$FAKE_OUT"
FAKE_RC=101 run 1 "FAILED: no test summary (cargo exit 101)" K2_JOB_CPUS=7
FAKE_RC=101 run 1 "could not compile" K2_JOB_CPUS=7
# cargo failed after a clean summary (a leak, a setup script): a failure.
printf '     Summary [ 1.0s] 10 tests run: 10 passed, 0 skipped\n' > "$FAKE_OUT"
FAKE_RC=1 run 1 "FAILED (cargo exit 1)" K2_JOB_CPUS=7
# Counts that don't add up are not passed off as green.
printf '     Summary [ 1.0s] 10 tests run: 9 passed, 0 skipped\n' > "$FAKE_OUT"
FAKE_RC=0 run 1 "the counts don't add up" K2_JOB_CPUS=7
# One test: nextest says "1 test run".
printf '     Summary [ 0.1s] 1 test run: 1 passed, 0 skipped\n' > "$FAKE_OUT"
FAKE_RC=0 run 0 "mac-tests: 1 run, 1 passed, 0 failed, 0 skipped" K2_JOB_CPUS=7
set +e; out="$(env PATH="$T/bin:$PATH" K2_JOB_CPUS=7 bash "$HERE/scripts/ci/mac-tests.sh" extra 2>&1)"; code=$?; set -e
[ "$code" = 2 ] && case "$out" in *"takes no arguments"*) true ;; *) false ;; esac || no "an argument: exit $code: $out"; ok
echo "mac-tests tests: $pass passed"
