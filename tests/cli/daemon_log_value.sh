#!/usr/bin/env bash
# `k2 daemon log --tail N` (and -n N): a --tail or -n with no word after it
# is a usage error. Stand-in service managers only
# (tests/cli/_daemon_stand_ins.sh): no row can reach a real systemctl,
# launchctl, loginctl, journalctl or sudo, and the guard fails the test if
# one is reachable.
#
# Regression (0.46.17): cmd_daemon_log read the word after --tail or -n as
# "$2" whatever was there, so either one as the last word stopped on
# bash's own "$2: unbound variable" (exit 1).
#
# Now it prints "Error: <flag> takes a number" on stderr, nothing on
# stdout, exits 2, and nothing else runs: no tail of the log, no service
# call, no request. The plain forms (the default 200 lines, --tail N, -n N)
# still print the log's last lines.
# Run with: bash tests/cli/daemon_log_value.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
. "$SCRIPT_DIR/_daemon_stand_ins.sh"
for v in $(env | grep -oE '^K[25](SO)?_[A-Z0-9_]+' || true); do
    [ "$v" = K5_CLI_UNDER_TEST ] || unset "$v"
done
ds_setup

# with_log: a fresh row with a four-line daemon log.
with_log() {
    ds_reset
    printf 'one\ntwo\nthree\nfour\n' >"$DS_K2HOME/daemon.stdout.log"
}

# row <label> <rc> <stdout> <stderr>: no service call, no request.
row() {
    eq "$1: exit" "$RC" "$2"
    eq "$1: stdout" "$OUT" "$3"
    eq "$1: stderr" "$ERR" "$4"
    eq "$1: service calls" "$(ds_calls)" ""
    if [ -s "$DS_REQS" ]; then
        bad "$1: sent a request:"; sed 's/^/      | /' "$DS_REQS" >&2
    else
        ok "$1: sent nothing"
    fi
}

echo "== a value flag as the last word is a usage error =="
with_log; ds_run log --tail
row "log --tail" 2 "" "Error: --tail takes a number"
with_log; ds_run log -n
row "log -n" 2 "" "Error: -n takes a number"
with_log; ds_run log --follow --tail
row "log --follow --tail" 2 "" "Error: --tail takes a number"
with_log; ds_run logs -n 2 -n
row "logs -n 2 -n" 2 "" "Error: -n takes a number"

echo "== the plain forms still print the log =="
with_log; ds_run log
row "log" 0 "$(printf 'one\ntwo\nthree\nfour')" ""
with_log; ds_run log --tail 1
row "log --tail 1" 0 "four" ""
with_log; ds_run logs x -n 2
row "logs x -n 2" 0 "$(printf 'three\nfour')" ""

echo
echo "daemon_log_value: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
