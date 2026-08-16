#!/usr/bin/env bash
# PRD connections-list-users-v1 — CLI flag/help/schema smoke.
# No daemon: unknown flags + help + --schema fire before HTTP.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}

# Connection gate needs PORT+TOKEN; help/usage never talk to the daemon.
K2_FAKE=(env K2_PORT=9 K2_HOOK_TOKEN=fake)

echo "== help =="
help_out="$("${K2_FAKE[@]}" "$K2_CLI" connections list --help 2>&1 || true)"
assert_contains "list --help mentions --users" "$help_out" "--users"
assert_contains "list --help mentions do not k2 msg" "$help_out" "do not k2 msg"

top="$("${K2_FAKE[@]}" "$K2_CLI" help 2>&1 || true)"
assert_contains "top-level help mentions --users" "$top" "--users"

echo "== unknown flag exit 2 =="
set +e
"${K2_FAKE[@]}" "$K2_CLI" connections list --nope >/tmp/k2-conn-users-out.$$ 2>/tmp/k2-conn-users-err.$$
rc=$?
set -e
assert_eq "k2 connections list --nope → 2" "$rc" "2"
err="$(cat /tmp/k2-conn-users-err.$$)"
assert_contains "unknown flag names --nope" "$err" "--nope"
rm -f /tmp/k2-conn-users-out.$$ /tmp/k2-conn-users-err.$$

echo "== schema =="
schema_file="$(mktemp -t k2-conn-schema-XXXXXX)"
"$K2_CLI" --schema >"$schema_file" 2>/dev/null || true
if grep -Fq -- '"name": "connections list"' "$schema_file"; then
    echo "  PASS: schema has connections list"
    pass=$((pass + 1))
else
    echo "  FAIL: schema has connections list" >&2
    fail=$((fail + 1))
fi
if grep -Fq -- '"name": "--users"' "$schema_file"; then
    echo "  PASS: schema connections list has --users"
    pass=$((pass + 1))
else
    echo "  FAIL: schema connections list has --users" >&2
    fail=$((fail + 1))
fi
rm -f "$schema_file"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
