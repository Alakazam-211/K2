#!/usr/bin/env bash
# Stage 4 — k2 remote-session CLI smoke (no live daemon required).
#
# Covers:
#   1. Tool policy: remote-session is locked id (like api-key / users-admin)
#   2. TTL parse: 30m / 1h / bare seconds
#   3. Help text lists every Stage 4 subcommand
#   4. Error teacher maps REMOTE_SESSIONS_DISABLED / NO_GRANT → exit 3
#   5. bash -n syntax (caller also runs this; re-checked here)
#
# Pure helpers are sourced from cli/k2 markers / functions. Never touches
# the real ~/.k2 (HOME is sandboxed). No daemon build.

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
assert_ok() {
    local label="$1"; shift
    if "$@"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label" >&2
        fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -qF -- "$needle"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    fi
}

# ── 0. Syntax ────────────────────────────────────────────────────────
echo "== bash -n =="
if bash -n "$K2_CLI"; then
    echo "  PASS: bash -n cli/k2"
    pass=$((pass + 1))
else
    echo "  FAIL: bash -n cli/k2" >&2
    fail=$((fail + 1))
fi

# ── 1. Tool policy ───────────────────────────────────────────────────
echo "== tool policy =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "remote-session tool id" "$(_cli_tool_id_for_verb remote-session)" "remote-session"
assert_eq "remote-session mode id" "$(_cli_tool_default_mode remote-session)" "id"
assert_ok "remote-session locked" _cli_tool_is_locked remote-session

# ── 2. TTL parse (source helper from cli/k2) ─────────────────────────
echo "== TTL parse =="
# Extract only the pure function body by evaluating the definition lines.
eval "$(sed -n '/^_rs_parse_ttl()/,/^}/p' "$K2_CLI")"
assert_eq "ttl 30m" "$(_rs_parse_ttl 30m)" "1800"
assert_eq "ttl 45m" "$(_rs_parse_ttl 45m)" "2700"
assert_eq "ttl 1h"  "$(_rs_parse_ttl 1h)"  "3600"
assert_eq "ttl 90s" "$(_rs_parse_ttl 90s)" "90"
assert_eq "ttl bare 1800" "$(_rs_parse_ttl 1800)" "1800"
assert_eq "ttl default empty" "$(_rs_parse_ttl "")" "1800"
if _rs_parse_ttl "nope" >/dev/null 2>&1; then
    echo "  FAIL: invalid ttl should fail" >&2
    fail=$((fail + 1))
else
    echo "  PASS: invalid ttl rejected"
    pass=$((pass + 1))
fi

# ── 3. Help ──────────────────────────────────────────────────────────
echo "== help =="
# Connection gate needs PORT+TOKEN; help exits before any HTTP call.
help_out="$(PORT=1 TOKEN=fake "$K2_CLI" remote-session --help 2>&1)" || true
for needle in \
    "remote-session status" \
    "remote-session enable" \
    "remote-session disable" \
    "remote-session grant" \
    "remote-session grants" \
    "remote-session revoke" \
    "remote-session shell" \
    "remote-session write" \
    "remote-session read" \
    "k2rs_" \
    "--ttl"
do
    assert_contains "help has $needle" "$help_out" "$needle"
done

# ── 4. Error teacher ─────────────────────────────────────────────────
echo "== error teacher =="
eval "$(sed -n '/^_rs_print_err()/,/^}/p' "$K2_CLI")"
teach_rc=0
teach_out="$(printf '%s' '{"ok":false,"error":{"code":"REMOTE_SESSIONS_DISABLED","hint":"off"}}' | _rs_print_err 2>&1)" || teach_rc=$?
assert_eq "REMOTE_SESSIONS_DISABLED exit 3" "$teach_rc" "3"
assert_contains "REMOTE_SESSIONS_DISABLED teach enable" "$teach_out" "enable"

teach_rc=0
teach_out="$(printf '%s' '{"ok":false,"error":{"code":"NO_GRANT","hint":"no grant"}}' | _rs_print_err 2>&1)" || teach_rc=$?
assert_eq "NO_GRANT exit 3" "$teach_rc" "3"
assert_contains "NO_GRANT teach grant" "$teach_out" "grant"

teach_rc=0
teach_out="$(printf '%s' '{"ok":false,"error":{"code":"GRANT_EXPIRED","hint":"expired"}}' | _rs_print_err 2>&1)" || teach_rc=$?
assert_eq "GRANT_EXPIRED exit 3" "$teach_rc" "3"

teach_rc=0
teach_out="$(printf '%s' '{"error":"Invalid or missing auth token"}' | _rs_print_err 2>&1)" || teach_rc=$?
assert_eq "invalid token exit 3" "$teach_rc" "3"

# ── summary ──────────────────────────────────────────────────────────
echo ""
echo "remote_session_cli_smoke: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
