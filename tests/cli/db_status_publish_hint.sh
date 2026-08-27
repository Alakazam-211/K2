#!/usr/bin/env bash
# D30: k2 db help/status mention publish subdomain + port, never static IP / db expose.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

export K2_PORT=1 K2SO_PORT=1 K2_HOOK_TOKEN=x
help_out="$("$K2" db --help)"
echo "$help_out" | grep -q 'publish subdomain' || fail "k2 db --help must mention publish subdomain"
echo "$help_out" | grep -qi 'static ip' && fail "k2 db --help must not mention static IP"
# No expose subcommand (help may say there isn't one).
echo "$help_out" | grep -E '^  expose' && fail "k2 db --help must not list an expose verb"

schema="$("$K2" --schema 2>/dev/null || true)"
if [ -n "$schema" ]; then
    echo "$schema" | grep -q 'publish subdomain' || fail "k2 --schema db status must mention publish subdomain"
fi

echo "PASS: db status publish hint"
