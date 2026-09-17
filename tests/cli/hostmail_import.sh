#!/usr/bin/env bash
# hostmail import: Maildir++ / --skip-inbox / 2h wait. No live daemon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

help="$("$K2" hostmail import --help)"
printf '%s' "$help" | grep -q -- '--skip-inbox' || fail "import --help must mention --skip-inbox: $help"
printf '%s' "$help" | grep -Eq '7200|2 hour' || fail "import --help must mention the 2h / 7200s wait: $help"

# Import POST waits 7200s; global cli_post_json stays 30s.
if ! grep -A30 'verb == "import"' "$K2" | grep -q 'timeout=7200'; then
    fail "k2 hostmail import must call POST /cli/mail/import with timeout=7200"
fi
if grep -A30 'verb == "import"' "$K2" | grep -q 'cli_post_json'; then
    fail "import must not go through global cli_post_json (30s)"
fi
if ! grep -A12 '^cli_post_json()' "$K2" | grep -q -- '--max-time 30'; then
    fail "cli_post_json must stay --max-time 30"
fi
if ! grep -q 'skipInbox' "$K2"; then
    fail "CLI import must send skipInbox JSON"
fi
if ! grep -q -- '--skip-inbox)' "$K2"; then
    fail "cmd_hostmail_import must parse --skip-inbox"
fi

echo "OK: hostmail import help, --skip-inbox, 7200s wait"
