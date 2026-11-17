#!/usr/bin/env bash
# hostmail import: Maildir++ / --skip-inbox / 2h wait. No live daemon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$CLI_BASH_SOURCE" || fail "bash -n cli/k2"

help="$("$K2" hostmail import --help)"
<<<"$help" grep -q -- '--skip-inbox' || fail "import --help must mention --skip-inbox: $help"
<<<"$help" grep -Eq '7200|2 hour' || fail "import --help must mention the 2h / 7200s wait: $help"

# Import POST waits 7200s; global cli_post_json stays 30s.
if ! grep -A30 'verb == "import"' "$CLI_BASH_SOURCE" | grep -q 'timeout=7200'; then
    fail "k2 hostmail import must call POST /cli/mail/import with timeout=7200"
fi
if grep -A30 'verb == "import"' "$CLI_BASH_SOURCE" | grep -q 'cli_post_json'; then
    fail "import must not go through global cli_post_json (30s)"
fi
# cli_post_json's curl says `--max-time 30`, or `--max-time
# "${_K2_POST_MAX_TIME:-30}"` (one send may ask for more); the default must
# be 30 either way.
mt_arg="$(grep -A16 '^cli_post_json()' "$CLI_BASH_SOURCE" | grep -oE -- '--max-time ([0-9]+|"\$\{_K2_POST_MAX_TIME:-[0-9]+\}")' | head -1 || true)"
case "$mt_arg" in
    '--max-time "${_K2_POST_MAX_TIME:-'*) mt_default="${mt_arg#*:-}"; mt_default="${mt_default%%[!0-9]*}" ;;
    '--max-time '[0-9]*) mt_default="${mt_arg#--max-time }" ;;
    *) mt_default="" ;;
esac
[ "$mt_default" = 30 ] || fail "cli_post_json must stay --max-time 30 by default (found: ${mt_arg:-no --max-time})"
if ! grep -q 'skipInbox' "$CLI_BASH_SOURCE"; then
    fail "CLI import must send skipInbox JSON"
fi
if ! grep -q -- '--skip-inbox)' "$CLI_BASH_SOURCE"; then
    fail "cmd_hostmail_import must parse --skip-inbox"
fi

echo "OK: hostmail import help, --skip-inbox, 7200s wait"
