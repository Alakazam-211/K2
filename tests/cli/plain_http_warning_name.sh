#!/usr/bin/env bash
# The plain-http warning names the command, however the script is run.
#
# Regression (0.46.17): the warning took the name from $0. The k5 front
# hands a command to the bash CLI by running it from an open file
# (/dev/fd/N), so through the front it said "warning: 4 is sending this
# server's owner token ...". It now uses the CLI's name (_K2_CLI_NAME).
#
# Rows, each with K2_HOST a LAN address over plain http and a command
# that stops on its usage line before any request (`agent update`):
#   - the CLI under test, run as people run it;
#   - the bash CLI run as the front runs it: bash /dev/fd/3, with
#     K5_CLI_VIA=rust as the front sets it.
# Each: the first stderr line is exactly the warning naming k2, and it is
# printed once.
#
# Hermetic: temp HOME, no daemon, no request (the usage stops first).
# Run with: bash tests/cli/plain_http_warning_name.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-plain-http-name-XXXXXX)"
trap 'rm -rf "$WORK"; hermetic_cli_cleanup' EXIT
cd "$WORK"

LAN=192.0.2.10
WANT="warning: k2 is sending this server's owner token to $LAN over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https."

check() {
    local label="$1" err="$2"
    local first count
    first="$(printf '%s\n' "$err" | head -1)"
    count="$(printf '%s\n' "$err" | grep -c '^warning:' || true)"
    [ "$first" = "$WANT" ] && ok "$label: the warning names k2" \
        || { bad "$label: first stderr line"; printf '%s\n' "$err" | sed 's/^/      | /' >&2; }
    [ "$count" = 1 ] && ok "$label: one warning" || bad "$label: $count warnings"
}

echo "== the CLI under test, as people run it =="
set +e
ERR="$(K2_HOST="$LAN" "$K2" agent update </dev/null 2>&1 >/dev/null)"
set -e
check "k2 agent update" "$ERR"

echo "== the bash CLI as the front runs it (/dev/fd/3) =="
set +e
ERR="$(K2_HOST="$LAN" K5_CLI_VIA=rust /bin/bash /dev/fd/3 agent update 3<"$CLI_BASH_SOURCE" </dev/null 2>&1 >/dev/null)"
set -e
check "bash /dev/fd/3 agent update" "$ERR"

echo ""
echo "plain_http_warning_name: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
