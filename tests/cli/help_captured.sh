#!/usr/bin/env bash
# The captured help forms (crates/k5-cli/src/help_captured.txt: what bash
# printed for every help form of the router that tests/cli/help_native.sh
# proves bash only prints, the same everywhere) print the same from the
# CLI under test as from this checkout's bash: stdout, stderr and the exit.
#
# When the CLI under test is the k5 binary (K5_CLI_UNDER_TEST set), its
# hand-over to bash is broken (K5_HOME_OVERRIDE set to a relative path,
# which bash never reads and the binary refuses before it runs bash), so a
# pass is the binary's own answer:
#   - every form, in the owner's terminal (a port and an owner token on
#     disk);
#   - every fifth form in an agent's terminal (a passport, K2_PORT and
#     K2_HOOK_SOCK a live socket);
#   - every tenth form with K2_HOOK_SOCK set and no passport: an id tool's
#     help is refused (exit 3), an open one's printed, by both.
# (crates/k5-cli/tests/help_captured_parity.rs runs the binary on every
# form in both terminals against the list; help_native.sh proves the list
# is bash's.) With no port and no token the binary hands a form to bash
# (the connection gate): a few such forms must fail with the hand-over
# broken. On bash alone there is nothing to compare: it says so and passes.
#
# Each run gets a scratch HOME and no K2_/K5_ settings from the caller
# (env -i); curl, launchctl, systemctl, sudo, security, open, osascript,
# ssh and k2 itself are stubs that log and fail: a run that calls one
# fails the test. Never touches ~/.k2.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
NEW="$CLI_UNDER_TEST"
LIST="$ROOT/crates/k5-cli/src/help_captured.txt"
[ -f "$LIST" ] || { echo "FAIL: no $LIST" >&2; exit 1; }

if [ "$NEW" = "$CLI_BASH_SOURCE" ]; then
    echo "help_captured: the CLI under test is bash: nothing to compare (run it with K5_CLI_UNDER_TEST)"
    exit 0
fi

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

SANDBOX="$(mktemp -d -t k2-help-captured-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT
mkdir -p "$SANDBOX/home/.k2" "$SANDBOX/empty-home" "$SANDBOX/cwd" "$SANDBOX/out" "$SANDBOX/stub"
printf '1\n' > "$SANDBOX/home/.k2/heartbeat.port"
printf 'test-token\n' > "$SANDBOX/home/.k2/heartbeat.token"
for t in curl wget nc launchctl systemctl sudo security open osascript pkill killall ssh k2 k5; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/stub.log"\nexit 97\n' "$t" "$SANDBOX" > "$SANDBOX/stub/$t"
    chmod +x "$SANDBOX/stub/$t"
done
python3 -c 'import socket, sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$SANDBOX/live.sock"
[ -S "$SANDBOX/live.sock" ] || { echo "FAIL: no socket at $SANDBOX/live.sock" >&2; exit 1; }
BROKEN="K5_HOME_OVERRIDE=not/absolute"

# run <prog> <out file> <settings words> -- <args...>: stdout to the file,
# stderr to <out>.err, the exit code to <out>.code. A stub call fails.
run() {
    local prog="$1" out="$2" code=0; shift 2
    local -a set=()
    while [ "$1" != "--" ]; do set+=("$1"); shift; done
    shift
    : > "$SANDBOX/stub.log"
    (cd "$SANDBOX/cwd" && env -i HOME="$SANDBOX/home" PATH="$SANDBOX/stub:$PATH" LC_ALL=C \
        ${set[@]+"${set[@]}"} "$prog" "$@") > "$out" 2> "$out.err" < /dev/null || code=$?
    printf '%s\n' "$code" > "$out.code"
    if [ -s "$SANDBOX/stub.log" ]; then
        bad "k2 $* ran $(tr '\n' ';' < "$SANDBOX/stub.log")"
    fi
}

# native <settings words> -- <args...>: the binary with its hand-over
# broken prints what bash prints, both with the settings.
native() {
    local -a set=()
    while [ "$1" != "--" ]; do set+=("$1"); shift; done
    shift
    run "$CLI_BASH_SOURCE" "$SANDBOX/out/bash" ${set[@]+"${set[@]}"} -- "$@"
    run "$NEW" "$SANDBOX/out/bin" ${set[@]+"${set[@]}"} "$BROKEN" -- "$@"
    if cmp -s "$SANDBOX/out/bash" "$SANDBOX/out/bin" && cmp -s "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" \
        && cmp -s "$SANDBOX/out/bash.code" "$SANDBOX/out/bin.code"; then
        ok
    else
        bad "k2 $* (${set[*]:-}): the binary's own answer differs from bash"
        diff "$SANDBOX/out/bash" "$SANDBOX/out/bin" | head -6 >&2 || true
        diff "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" | head -6 >&2 || true
    fi
}

# handed <settings words> -- <args...>: the binary hands it to bash, so
# with the hand-over broken it fails, saying why, with stdout empty.
handed() {
    local -a set=()
    while [ "$1" != "--" ]; do set+=("$1"); shift; done
    shift
    run "$NEW" "$SANDBOX/out/bin" ${set[@]+"${set[@]}"} "$BROKEN" -- "$@"
    if [ "$(cat "$SANDBOX/out/bin.code")" != 0 ] && [ ! -s "$SANDBOX/out/bin" ] \
        && grep -q K5_HOME_OVERRIDE "$SANDBOX/out/bin.err"; then
        ok
    else
        bad "k2 $* (${set[*]:-}) did not go to bash (exit $(cat "$SANDBOX/out/bin.code"))"
    fi
}

AGENT=("K2_PORT=1" "K2_HOOK_TOKEN=sessT.notreal1" "K2_HOOK_SOCK=$SANDBOX/live.sock")
SOCK="K2_HOOK_SOCK=$SANDBOX/no.sock"
EMPTY="HOME=$SANDBOX/empty-home"
n=0
while IFS=$'\t' read -r kind argv _; do
    [ "$kind" = F ] || continue
    n=$((n + 1))
    # shellcheck disable=SC2086  # argv is words
    native -- $argv
    if [ $((n % 5)) = 0 ]; then
        # shellcheck disable=SC2086
        native "${AGENT[@]}" -- $argv
    fi
    if [ $((n % 10)) = 0 ]; then
        # shellcheck disable=SC2086
        native "$SOCK" -- $argv
    fi
    if [ $((n % 200)) = 0 ]; then
        # shellcheck disable=SC2086
        handed "$EMPTY" -- $argv
    fi
done < "$LIST"
[ "$n" -gt 1000 ] || bad "only $n forms in $LIST"
for cmd in "agent team help x" "app nope --help" "zen nope --help" "agent team --help x"; do
    # shellcheck disable=SC2086
    handed -- $cmd
done

echo "help_captured: $pass passed, $fail failed ($n forms)"
[ "$fail" = 0 ]
