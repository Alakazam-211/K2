#!/usr/bin/env bash
# hostmail password rotate: nested under password, not rotate-admin.
# POST {address}. _mail_py must export M_ADDRESS (no M_QUOTA-style leak).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'M_ADDRESS="${M_ADDRESS:-}"' "$K2" || fail "_mail_py must export M_ADDRESS"

if ! grep -A20 'verb == "password_rotate"' "$K2" | grep -q '/cli/mail/address/password'; then
    fail "password_rotate must POST /cli/mail/address/password"
fi
if ! grep -A20 'verb == "password_rotate"' "$K2" | grep -q 'body={"address": env("M_ADDRESS")}'; then
    fail "password_rotate must POST {\"address\": ...} (exact body)"
fi
if grep -A25 'verb == "password_rotate"' "$K2" | grep -q 'rotate-admin'; then
    fail "password rotate must not call rotate-admin"
fi
if grep -A30 'verb == "password_rotate"' "$K2" | grep -q '993'; then
    fail "password rotate must not mention 993"
fi

help="$("$K2" hostmail password rotate --help)"
printf '%s' "$help" | grep -q 'k2 hostmail password rotate' || fail "rotate --help must be leaf help: $help"
printf '%s' "$help" | grep -q 'app passwords do not' || fail "rotate --help must say app passwords survive: $help"
printf '%s' "$help" | grep -q '443' || fail "rotate --help must mention 443: $help"
printf '%s' "$help" | grep -q '465' || fail "rotate --help must mention 465: $help"
if printf '%s' "$help" | grep -q 'k2 hostmail <command>'; then
    fail "rotate --help must not be group help: $help"
fi
if printf '%s' "$help" | grep -q 'rotate-admin'; then
    fail "password rotate --help must not be rotate-admin: $help"
fi

pw_help="$("$K2" hostmail password --help)"
printf '%s' "$pw_help" | grep -q 'password rotate' || fail "password --help must list rotate: $pw_help"

group="$("$K2" hostmail --help)"
printf '%s' "$group" | grep -q 'password rotate' || fail "hostmail --help must list password rotate: $group"

out="$(mktemp -t k2-hostmail-pw-XXXXXX)"
trap 'rm -f "$out"' EXIT
set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail password rotate --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "password rotate without addr must exit 2, got $code: $(cat "$out")"
fi
if ! grep -q address "$out"; then
    fail "usage error must mention address: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail rotate --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -eq 0 ]; then
    fail "k2 hostmail rotate (not password rotate / not rotate-admin) must not succeed: $(cat "$out")"
fi
if grep -q 'rotate leftover' "$out"; then
    fail "bare rotate must not be rotate-admin: $(cat "$out")"
fi

echo "OK: hostmail password rotate help/parse"
