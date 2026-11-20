#!/usr/bin/env bash
# Tests for scripts/ci/mac-tests.sh with stand-ins for cargo and uname: no
# cargo runs here. Run: bash tests/garden/mac-tests.sh
set -euo pipefail
# The script reads CARGO_BUILD_JOBS from its caller; a host that sets it
# (some compute nodes do) would change what these tests expect. Start clean.
unset CARGO_BUILD_JOBS
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
T="$(mktemp -d)"
trap 'rm -rf "$T"; rm -f "$HERE"/mac-tests-core.log "$HERE"/mac-tests-daemon.log "$HERE"/mac-tests-daemon.part.log' EXIT
pass=0
mkdir -p "$T/bin" "$T/calls"
# cargo: `metadata` prints $FAKE_META; `build` is noted in calls/builds
# and exits $FAKE_BUILD_RC. Any other call is numbered: its
# arguments go to calls/<n>.args, it prints calls/<n>.out (or $FAKE_OUT)
# and exits with calls/<n>.rc (or $FAKE_RC).
cat > "$T/bin/cargo" <<'S'
#!/bin/sh
if [ "$1" = metadata ]; then cat "$FAKE_META"; exit 0; fi
if [ "$1" = build ]; then echo "$*" >> "$FAKE_CALLS/builds"; echo "building $4"; exit "${FAKE_BUILD_RC:-0}"; fi
n=$(( $(ls "$FAKE_CALLS" | grep -c '\.args$') + 1 ))
printf '%s\n' "$@" > "$FAKE_CALLS/$n.args"
env | grep -E '^CARGO_(PROFILE|BUILD)' | sort > "$FAKE_CALLS/$n.env"
if [ -f "$FAKE_CALLS/$n.out" ]; then cat "$FAKE_CALLS/$n.out"; else cat "$FAKE_OUT"; fi
if [ -f "$FAKE_CALLS/$n.rc" ]; then exit "$(cat "$FAKE_CALLS/$n.rc")"; fi
exit "$FAKE_RC"
S
printf '#!/bin/sh\necho "${FAKE_UNAME:-Darwin}"\n' > "$T/bin/uname"
chmod +x "$T/bin/cargo" "$T/bin/uname"
export FAKE_CALLS="$T/calls" FAKE_OUT="$T/out" FAKE_META="$T/meta" FAKE_RC=0
cat > "$FAKE_META" <<'J'
{"packages":[
 {"name":"k2-ptyd","targets":[]},
 {"name":"k2-deckrender","targets":[]},
 {"name":"k2-scenerender","targets":[]},
 {"name":"k2-core","targets":[{"name":"k2_core","kind":["lib"],"test":true}]},
 {"name":"k2-daemon","targets":[
   {"name":"k2_daemon","kind":["lib"],"test":true},
   {"name":"k2-daemon","kind":["bin"],"test":true},
   {"name":"it_mail","kind":["test"],"test":true},
   {"name":"it_auth","kind":["test"],"test":true},
   {"name":"helper","kind":["bin"],"test":false},
   {"name":"k2-vmm-worker","kind":["bin"],"test":true,"required-features":["sandbox-microvm"]},
   {"name":"speed","kind":["bench"],"test":false}]}]}
J
fresh() { rm -f "$T/calls"/*; }
builds() { cat "$T/calls/builds" 2>/dev/null || true; }
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(env PATH="$T/bin:$PATH" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL ($*): exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL ($*): output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
M="$HERE/scripts/ci/mac-tests.sh"
ok() { pass=$((pass + 1)); }
no() { echo "FAIL: $1" >&2; exit 1; }
calls() { ls "$T/calls" | grep -c '\.args$' || true; }

# Outside a compute job, on another system, a bad argument: no cargo runs.
fresh
run 2 "not in a K2 Compute job" env -u K2_JOB_CPUS bash "$M"
run 2 "this machine is Linux" env K2_JOB_CPUS=7 FAKE_UNAME=Linux bash "$M"
run 2 "usage: mac-tests.sh [core|daemon]" env K2_JOB_CPUS=7 bash "$M" all
run 2 "usage: mac-tests.sh [core|daemon]" env K2_JOB_CPUS=7 bash "$M" core daemon
[ "$(calls)" = 0 ] || no "cargo ran when the script refused"; ok

# core, clean: counts printed, exit 0; cargo got --locked, the three
# crates and a filter that leaves out exactly the listed k2-node files.
printf '        PASS [   0.010s] (1/5473) k2-core a::b\n------------\n     Summary [ 365.974s] 5473 tests run: 5473 passed (1 slow), 3 skipped\n' > "$FAKE_OUT"
fresh
run 0 "mac-tests: core: 5473 run, 5473 passed, 0 failed, 3 skipped" env K2_JOB_CPUS=7 bash "$M" core
[ "$(calls)" = 1 ] || no "core made $(calls) cargo calls"; ok
[ "$(sed -n 1,11p "$T/calls/1.args" | tr '\n' ' ')" = "nextest run --locked --no-fail-fast -p k2-core -p k2-ptyd -p k2-node -E " ] || no "cargo arguments: $(tr '\n' ' ' < "$T/calls/1.args")"; ok
[ "$(tail -1 "$T/calls/1.args")" = "not (package(k2-node) & ($(bash "$HERE/scripts/ci/k2-node-mac-tests.sh" filter unwalled)))" ] || no "the filter is not built from the list: $(tail -1 "$T/calls/1.args")"; ok
grep -qx 'CARGO_BUILD_JOBS=2' "$T/calls/1.env" && grep -qx 'CARGO_PROFILE_TEST_DEBUG=0' "$T/calls/1.env" && grep -qx 'CARGO_PROFILE_DEV_DEBUG=0' "$T/calls/1.env" || no "core's build settings: $(cat "$T/calls/1.env")"; ok

# core, a failed test: named, counts printed, exit 1.
printf '        FAIL [   5.127s] (3729/5473) k2-core terminal::cli_status::tests::missing_installed_and_broken\n------------\n     Summary [ 385.416s] 5473 tests run: 5470 passed (1 slow), 3 failed, 3 skipped\nerror: test run failed\n' > "$FAKE_OUT"
fresh; FAKE_RC=100 run 1 "mac-tests: core: 5473 run, 5470 passed, 3 failed, 3 skipped" env K2_JOB_CPUS=7 bash "$M" core
fresh; FAKE_RC=100 run 1 "FAIL k2-core terminal::cli_status::tests::missing_installed_and_broken" env K2_JOB_CPUS=7 bash "$M" core
# The summary says failed although cargo exited 0: still a failure.
fresh; FAKE_RC=0 run 1 "FAILED: core (cargo exit 0)" env K2_JOB_CPUS=7 bash "$M" core
# A build error: no summary at all.
printf 'error[E0425]: cannot find value `x` in this scope\nerror: could not compile `k2-core`\n' > "$FAKE_OUT"
fresh; FAKE_RC=101 run 1 "FAILED: core: no test summary (cargo exit 101)" env K2_JOB_CPUS=7 bash "$M" core
fresh; FAKE_RC=101 run 1 "could not compile" env K2_JOB_CPUS=7 bash "$M" core
# cargo failed after a clean summary (a leak, a setup script): a failure.
printf '     Summary [ 1.0s] 10 tests run: 10 passed, 0 skipped\n' > "$FAKE_OUT"
fresh; FAKE_RC=1 run 1 "FAILED: core (cargo exit 1)" env K2_JOB_CPUS=7 bash "$M" core
# Counts that don't add up are not passed off as green.
printf '     Summary [ 1.0s] 10 tests run: 9 passed, 0 skipped\n' > "$FAKE_OUT"
fresh; FAKE_RC=0 run 1 "core: the counts don't add up" env K2_JOB_CPUS=7 bash "$M" core

# daemon: the helper programs are built first; then one cargo call per
# test binary (lib, bin, each test; not the target with test off, not the
# bench), one crate built at a time, the counts summed into one line. A
# target behind a feature that is off is named and not run.
printf '     Summary [ 9.0s] 100 tests run: 100 passed, 2 skipped\n' > "$FAKE_OUT"
fresh
printf '     Summary [ 0.1s] 1 test run: 1 passed, 0 skipped\n' > "$T/calls/2.out"
run 0 "mac-tests: daemon: 301 run, 301 passed, 0 failed, 6 skipped" env K2_JOB_CPUS=7 bash "$M" daemon
[ "$(calls)" = 4 ] || no "daemon made $(calls) cargo calls, wanted 4"; ok
[ "$(builds)" = "$(printf 'build --locked -p k2-ptyd\nbuild --locked -p k2-deckrender\nbuild --locked -p k2-scenerender')" ] || no "the helper builds: $(builds)"; ok
fresh
run 0 "mac-tests: daemon: not run, its feature is off in this build: k2-vmm-worker (needs sandbox-microvm)" env K2_JOB_CPUS=7 bash "$M" daemon
fresh
run 0 "mac-tests: daemon: 4 test binaries of k2-daemon, one cargo call each" env K2_JOB_CPUS=7 bash "$M" daemon
got="$(for n in 1 2 3 4; do sed -n '5,$p' "$T/calls/$n.args" | sed '/^-E$/,$d' | tr '\n' ' '; echo; done)"
want="$(printf -- '--test it_auth \n--test it_mail \n--bin k2-daemon \n--lib \n')"
[ "$(echo "$got" | sed 's/-p k2-daemon //' | sort)" = "$(echo "$want" | sort)" ] || no "daemon's cargo calls: $got"; ok
grep -qx 'CARGO_BUILD_JOBS=1' "$T/calls/1.env" || no "daemon builds more than one crate at a time: $(cat "$T/calls/1.env")"; ok
# Every daemon call leaves out exactly the listed login-shell tests.
want="$(bash "$HERE/scripts/ci/k2-daemon-mac-tests.sh" filter walled)"
for n in 1 2 3 4; do
    [ "$(tail -2 "$T/calls/$n.args" | head -1)" = "-E" ] && [ "$(tail -1 "$T/calls/$n.args")" = "$want" ] || no "daemon call $n has no walled filter: $(tail -2 "$T/calls/$n.args" | cut -c1-80)"
done; ok
# One binary red: the stage is red, the others still ran, the red one is named.
fresh
printf '        FAIL [   1.0s] (2/100) k2-daemon::it_mail watch::fires_once\n     Summary [ 9.0s] 100 tests run: 99 passed, 1 failed, 2 skipped\n' > "$T/calls/3.out"; echo 100 > "$T/calls/3.rc"
run 1 "mac-tests: daemon: 400 run, 399 passed, 1 failed, 8 skipped" env K2_JOB_CPUS=7 bash "$M" daemon
[ "$(calls)" = 4 ] || no "a red binary stopped the stage after $(calls) calls"; ok
fresh
printf '        FAIL [   1.0s] (2/100) k2-daemon::it_mail watch::fires_once\n     Summary [ 9.0s] 100 tests run: 99 passed, 1 failed, 2 skipped\n' > "$T/calls/3.out"; echo 100 > "$T/calls/3.rc"
run 1 "FAIL k2-daemon::it_mail watch::fires_once" env K2_JOB_CPUS=7 bash "$M" daemon
# A binary with no tests is nextest's exit 4, and not a failure, with or
# without a summary line.
fresh
printf 'error: no tests to run\n' > "$T/calls/1.out"; echo 4 > "$T/calls/1.rc"
run 0 "mac-tests: daemon: 300 run, 300 passed, 0 failed, 6 skipped" env K2_JOB_CPUS=7 bash "$M" daemon
fresh
printf '     Summary [ 0.0s] 0 tests run: 0 passed, 0 skipped\nerror: no tests to run\n' > "$T/calls/1.out"; echo 4 > "$T/calls/1.rc"
run 0 "mac-tests: daemon: ok" env K2_JOB_CPUS=7 bash "$M" daemon
# Exit 4 for any other reason is still a failure.
fresh
printf 'error: something else\n' > "$T/calls/1.out"; echo 4 > "$T/calls/1.rc"
run 1 "no test summary (cargo exit 4)" env K2_JOB_CPUS=7 bash "$M" daemon
# A tree without one of the helper packages (an older line): that one is
# named and not built, the others are.
sed 's/ {"name":"k2-deckrender","targets":\[\]},//' "$FAKE_META" > "$T/meta.older"
fresh
run 0 "mac-tests: daemon: this tree has no k2-deckrender package; not built" env K2_JOB_CPUS=7 FAKE_META="$T/meta.older" bash "$M" daemon
[ "$(builds)" = "$(printf 'build --locked -p k2-ptyd\nbuild --locked -p k2-scenerender')" ] || no "the helper builds on an older tree: $(builds)"; ok
# A helper that does not build: the stage is red before any test runs.
fresh
run 1 "FAILED: daemon: cargo build -p k2-ptyd" env K2_JOB_CPUS=7 FAKE_BUILD_RC=101 bash "$M" daemon
[ "$(calls)" = 0 ] || no "tests ran although a helper did not build"; ok
# A binary that does not build: named, and the stage is red.
fresh
printf 'error: could not compile `k2-daemon` (test "it_auth")\n' > "$T/calls/2.out"; echo 101 > "$T/calls/2.rc"
run 1 "no test summary (cargo exit 101)" env K2_JOB_CPUS=7 bash "$M" daemon

# Other ways a test ends badly are named and counted too: a timeout, a
# signal, a failed try. nextest counts a timeout apart from "failed".
fresh
printf '     TIMEOUT [  60.004s] (5/100) k2-daemon::it_mail watch::waits_forever\n     SIGKILL [   2.100s] (6/100) k2-daemon::it_mail big::eats_memory\n TRY 2 FAIL [   0.200s] (7/100) k2-daemon::it_mail retry::still_red\n        PASS [   0.010s] (8/100) k2-daemon::it_mail fine::one\n        LEAK [   0.010s] (9/100) k2-daemon::it_mail fine::leaky_but_green\n     Summary [ 70.0s] 100 tests run: 97 passed (1 leaky), 2 failed, 1 timed out, 2 skipped\n' > "$T/calls/3.out"; echo 100 > "$T/calls/3.rc"
run 1 "mac-tests: daemon: 400 run, 397 passed, 3 failed, 8 skipped" env K2_JOB_CPUS=7 bash "$M" daemon
for name in "TIMEOUT k2-daemon::it_mail watch::waits_forever" "SIGKILL k2-daemon::it_mail big::eats_memory" "TRY 2 FAIL k2-daemon::it_mail retry::still_red"; do
    fresh
    cp "$T/keep.3.out" "$T/calls/3.out" 2>/dev/null || printf '     TIMEOUT [  60.004s] (5/100) k2-daemon::it_mail watch::waits_forever\n     SIGKILL [   2.100s] (6/100) k2-daemon::it_mail big::eats_memory\n TRY 2 FAIL [   0.200s] (7/100) k2-daemon::it_mail retry::still_red\n        PASS [   0.010s] (8/100) k2-daemon::it_mail fine::one\n        LEAK [   0.010s] (9/100) k2-daemon::it_mail fine::leaky_but_green\n     Summary [ 70.0s] 100 tests run: 97 passed (1 leaky), 2 failed, 1 timed out, 2 skipped\n' > "$T/calls/3.out"
    echo 100 > "$T/calls/3.rc"
    run 1 "$name" env K2_JOB_CPUS=7 bash "$M" daemon
done
# A pass or a leak that passed is never listed as a failure.
fresh
printf '     TIMEOUT [  60.004s] (5/100) k2-daemon::it_mail watch::waits_forever\n        PASS [   0.010s] (8/100) k2-daemon::it_mail fine::one\n        LEAK [   0.010s] (9/100) k2-daemon::it_mail fine::leaky_but_green\n     Summary [ 70.0s] 100 tests run: 99 passed (1 leaky), 1 timed out, 2 skipped\n' > "$T/calls/3.out"; echo 100 > "$T/calls/3.rc"
set +e; out="$(env PATH="$T/bin:$PATH" K2_JOB_CPUS=7 bash "$M" daemon 2>&1)"; code=$?; set -e
[ "$code" = 1 ] || no "a timeout alone: exit $code"
case "$out" in *"fine::one"*|*"leaky_but_green"*) no "a passing test was listed as failed: $out" ;; esac; ok
case "$out" in *"mac-tests: daemon: 400 run, 399 passed, 1 failed, 8 skipped"*) ok ;; *) no "a timeout is not counted as failed: $out" ;; esac
# A call that failed and named no test still says what it has: the
# summary and the end of its log.
fresh
printf 'some build chatter\nthe last thing the binary printed before it died\n     Summary [ 3.0s] 100 tests run: 98 passed, 2 failed, 0 skipped\nerror: test run failed\n' > "$T/calls/3.out"; echo 100 > "$T/calls/3.rc"
run 1 "(nextest named no test) Summary [ 3.0s] 100 tests run: 98 passed, 2 failed, 0 skipped. The end of this call's log:" env K2_JOB_CPUS=7 bash "$M" daemon
fresh
printf 'some build chatter\nthe last thing the binary printed before it died\n     Summary [ 3.0s] 100 tests run: 98 passed, 2 failed, 0 skipped\nerror: test run failed\n' > "$T/calls/3.out"; echo 100 > "$T/calls/3.rc"
run 1 "the last thing the binary printed before it died" env K2_JOB_CPUS=7 bash "$M" daemon

# Both stages: the second runs although the first failed, each has its
# line, and the exit is 1.
fresh
printf '     Summary [ 1.0s] 10 tests run: 9 passed, 1 failed, 0 skipped\n' > "$T/calls/1.out"; echo 100 > "$T/calls/1.rc"
run 1 "mac-tests: daemon: 400 run, 400 passed, 0 failed, 8 skipped" env K2_JOB_CPUS=7 bash "$M"
[ "$(calls)" = 5 ] || no "both stages made $(calls) cargo calls, wanted 5"; ok
fresh
run 0 "mac-tests: ok" env K2_JOB_CPUS=7 CARGO_BUILD_JOBS=3 bash "$M"
grep -qx 'CARGO_BUILD_JOBS=3' "$T/calls/1.env" && grep -qx 'CARGO_BUILD_JOBS=3' "$T/calls/5.env" || no "CARGO_BUILD_JOBS from the caller was not kept"; ok
echo "mac-tests tests: $pass passed"
