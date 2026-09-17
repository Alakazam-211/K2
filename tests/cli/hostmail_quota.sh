#!/usr/bin/env bash
# quota flags must ride _mail_py env (lztek: --gb dropped → usage nothing to set).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

# The python helper only sees env vars listed on the _mail_py prefix.
# quota set/config --quota-gb were parsed in bash then discarded.
grep -q 'M_QUOTA_BYTES="${M_QUOTA_BYTES:-}"' "$K2" || fail "_mail_py must export M_QUOTA_BYTES"
grep -q 'M_QUOTA_GB="${M_QUOTA_GB:-}"' "$K2" || fail "_mail_py must export M_QUOTA_GB"
grep -q 'M_QUOTA_MESSAGES="${M_QUOTA_MESSAGES:-}"' "$K2" || fail "_mail_py must export M_QUOTA_MESSAGES"

help="$("$K2" hostmail quota set --help)"
printf '%s' "$help" | grep -q -- '--gb' || fail "quota set --help must mention --gb"

echo "OK: hostmail quota env exported to _mail_py"
