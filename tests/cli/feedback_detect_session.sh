#!/usr/bin/env bash
# Loud harness for `_feedback_detect_session` (prd-ticket-answer-wakes-canonical §5).
# cargo does not cover this bash helper. Extracts the function from cli/k2
# and asserts the four env matrices. Fail-loud: no skip-if-missing.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -f "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found" >&2; exit 1; }

FN="$(awk '
    /^_feedback_detect_session\(\)/ { grab=1 }
    grab { print }
    grab && /^}/ { exit }
' "$K2_CLI")"

[ -n "$FN" ] || { echo "FAIL: could not extract _feedback_detect_session from $K2_CLI" >&2; exit 1; }
printf '%s\n' "$FN" | grep -q 'FB_SESSION_KIND="sandbox"' && {
    echo "FAIL: legacy sandbox stamp still present in _feedback_detect_session" >&2
    exit 1
}

eval "$FN"

pass=0
fail=0
assert_kind() {
    local label="$1" expected="$2"
    _feedback_detect_session
    if [ "${FB_SESSION_KIND:-}" = "$expected" ]; then
        echo "  PASS: $label → kind=$expected id=${FB_SESSION_ID:-}"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label expected kind='$expected' got kind='${FB_SESSION_KIND:-}' id='${FB_SESSION_ID:-}'" >&2
        fail=$((fail + 1))
    fi
}

echo "== _feedback_detect_session env matrices =="

unset K2_CELL K2_API_CELL K2SO_API_CELL K2_SESSION_ID CLAUDE_SESSION_ID CLAUDE_CODE_SESSION_ID

# 1. K2_CELL=canonical + K2_SESSION_ID → canonical, not sandbox
export K2_CELL=canonical K2_SESSION_ID=sess-canonical
assert_kind "K2_CELL=canonical + K2_SESSION_ID" "canonical"
[ "${FB_SESSION_ID}" = "sess-canonical" ] || {
    echo "  FAIL: expected id=sess-canonical got '${FB_SESSION_ID}'" >&2
    fail=$((fail + 1))
}

# 2. K2_SESSION_ID only (no K2_CELL) → canonical
unset K2_CELL
export K2_SESSION_ID=sess-pinned
assert_kind "K2_SESSION_ID only (no K2_CELL)" "canonical"
[ "${FB_SESSION_ID}" = "sess-pinned" ] || {
    echo "  FAIL: expected id=sess-pinned got '${FB_SESSION_ID}'" >&2
    fail=$((fail + 1))
}

# 3. K2_CELL=sidecar → sidecar (inject still wakes canonical; stamp is sidecar)
unset K2_SESSION_ID
export K2_CELL=sidecar K2_SESSION_ID=sess-sidecar
assert_kind "K2_CELL=sidecar" "sidecar"
[ "${FB_SESSION_ID}" = "sess-sidecar" ] || {
    echo "  FAIL: expected id=sess-sidecar got '${FB_SESSION_ID}'" >&2
    fail=$((fail + 1))
}

# 4. K2_API_CELL=1 → api (wins over K2_CELL / K2_SESSION_ID)
unset K2_CELL
export K2_API_CELL=1 K2_SESSION_ID=sess-api
assert_kind "K2_API_CELL=1" "api"
[ "${FB_SESSION_ID}" = "sess-api" ] || {
    echo "  FAIL: expected id=sess-api got '${FB_SESSION_ID}'" >&2
    fail=$((fail + 1))
}

# Self-check via the K2_TEST_FEEDBACK_DETECT print (no daemon). Conn gate
# still requires PORT/TOKEN, so we only run this when extraction already
# proved the function; a fake token is enough because the test seam exits
# before HTTP.
unset K2_CELL K2_API_CELL K2SO_API_CELL CLAUDE_SESSION_ID CLAUDE_CODE_SESSION_ID
got="$(
    env -u K2_CELL -u K2_API_CELL -u K2SO_API_CELL \
        K2_PORT=9 K2_HOOK_TOKEN=fake K2_TEST_FEEDBACK_DETECT=1 \
        K2_SESSION_ID=sess-print \
        "$K2_CLI" tickets ask "detect-self-check" 2>/dev/null || true
)"
if [ "$got" = "canonical" ]; then
    echo "  PASS: K2_TEST_FEEDBACK_DETECT print (K2_SESSION_ID only) → canonical"
    pass=$((pass + 1))
else
    echo "  FAIL: K2_TEST_FEEDBACK_DETECT print expected 'canonical' got $(printf %q "$got")" >&2
    fail=$((fail + 1))
fi

echo ""
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ] || exit 1
[ "$pass" -ge 5 ] || { echo "FAIL: expected at least 5 assertions" >&2; exit 1; }
