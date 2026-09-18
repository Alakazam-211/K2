#!/usr/bin/env bash
# Custom domains Slice A — CLI verb smoke: catalog, usage exit 2, schema.
#
# No daemon build. HOME sandboxed. Never touches the real ~/.k2.

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
    if printf '%s' "$hay" | grep -Fq "$needle"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}

echo "== catalog =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "domain tool id" "$(_cli_tool_id_for_verb domain)" "dns"
assert_eq "cert tool id" "$(_cli_tool_id_for_verb cert)" "dns"
if _cli_tool_is_locked dns; then
    echo "  PASS: dns locked (covers domain/cert)"
    pass=$((pass + 1))
else
    echo "  FAIL: dns must be locked" >&2
    fail=$((fail + 1))
fi

# shellcheck disable=SC1090
eval "$(sed -n '/^_uds_eligible()/,/^}/p' "$K2_CLI")"
if _uds_eligible "/cli/domains"; then
    echo "  PASS: /cli/domains UDS-eligible"
    pass=$((pass + 1))
else
    echo "  FAIL: /cli/domains should be UDS-eligible" >&2
    fail=$((fail + 1))
fi
if _uds_eligible "/cli/certs"; then
    echo "  PASS: /cli/certs UDS-eligible"
    pass=$((pass + 1))
else
    echo "  FAIL: /cli/certs should be UDS-eligible" >&2
    fail=$((fail + 1))
fi

echo "== usage =="
set +e
out="$("$K2_CLI" domain add 2>&1)"
rc=$?
set -e
assert_eq "domain add missing apex exit" "$rc" "2"
assert_contains "domain add usage json" "$out" "usage"

set +e
out="$("$K2_CLI" domain name add 2>&1)"
rc=$?
set -e
assert_eq "domain name add missing hostname exit" "$rc" "2"
assert_contains "domain name add usage" "$out" "usage"

set +e
out="$("$K2_CLI" domain totally-bogus 2>&1)"
rc=$?
set -e
assert_eq "domain unknown subcommand exit" "$rc" "2"
assert_contains "domain unknown teaches hostmail distinction" "$out" "hostmail"

echo "== help =="
help="$("$K2_CLI" domain --help)"
assert_contains "domain help mentions attach" "$help" "attach"
assert_contains "domain help not hostmail" "$help" "hostmail"
help="$("$K2_CLI" cert --help)"
assert_contains "cert help not hostmail cert" "$help" "hostmail"

echo "== schema =="
schema="$("$K2_CLI" --schema 2>/dev/null || true)"
assert_contains "schema has domain add" "$schema" '"name": "domain add"'
assert_contains "schema has cert list" "$schema" '"name": "cert list"'

echo "== $pass passed, $fail failed =="
[ "$fail" -eq 0 ]
