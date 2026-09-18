#!/usr/bin/env bash
# hostmail app-password: hyphenated sibling of quota, not nested under password.
# k2 mail app-password redirects. k2 mail link is unchanged.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'M_LABEL="${M_LABEL:-}"' "$K2" || fail "_mail_py must export M_LABEL"

if ! grep -A20 'verb == "app_password_add"' "$K2" | grep -q '/cli/mail/app-password'; then
    fail "app_password_add must POST /cli/mail/app-password"
fi
if ! grep -A20 'verb == "app_password_list"' "$K2" | grep -q '/cli/mail/app-password'; then
    fail "app_password_list must GET /cli/mail/app-password"
fi
if ! grep -A20 'verb == "app_password_revoke"' "$K2" | grep -q '/cli/mail/app-password/revoke'; then
    fail "app_password_revoke must POST /cli/mail/app-password/revoke"
fi
if grep -A40 'verb == "app_password_add"' "$K2" | grep -q 'address/password'; then
    fail "app-password must not call password rotate"
fi
if grep -A25 'cmd_hostmail_password()' "$K2" | grep -q 'app-password'; then
    fail "app-password must not nest under password"
fi

# k2 mail link (Gmail/Fastmail linked app-password) is a different verb.
if ! grep -q 'cmd_mail_link' "$K2"; then
    fail "k2 mail link must still exist"
fi

help="$("$K2" hostmail app-password add --help)"
printf '%s' "$help" | grep -q 'k2 hostmail app-password add' || fail "add --help must be leaf help: $help"
if printf '%s' "$help" | grep -q 'k2 hostmail <command>'; then
    fail "add --help must not be group help: $help"
fi

list_help="$("$K2" hostmail app-password list --help)"
printf '%s' "$list_help" | grep -q 'k2 hostmail app-password list' || fail "list --help: $list_help"

rev_help="$("$K2" hostmail app-password revoke --help)"
printf '%s' "$rev_help" | grep -q 'k2 hostmail app-password revoke' || fail "revoke --help: $rev_help"

group="$("$K2" hostmail --help)"
printf '%s' "$group" | grep -q 'app-password' || fail "hostmail --help must list app-password: $group"
printf '%s' "$group" | grep -q 'password rotate' || fail "hostmail --help must still list password rotate: $group"

pw_help="$("$K2" hostmail password --help)"
if printf '%s' "$pw_help" | grep -q 'app-password add'; then
    fail "password --help must not nest app-password: $pw_help"
fi

out="$(mktemp -t k2-hostmail-ap-XXXXXX)"
trap 'rm -f "$out"' EXIT

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail app-password add --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "add without addr must exit 2, got $code: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail app-password revoke a@b.test --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "revoke without id must exit 2, got $code: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" mail app-password add a@b.test --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "k2 mail app-password must exit 2 moved-to-hostmail, got $code: $(cat "$out")"
fi
if ! grep -q "k2 hostmail app-password" "$out"; then
    fail "k2 mail app-password must redirect to hostmail: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" mail link --help >"$out" 2>&1
code=$?
set -e
if grep -q "moved to 'k2 hostmail" "$out"; then
    fail "k2 mail link must not be stolen by app-password redirect: $(cat "$out")"
fi

echo "OK: hostmail app-password help/parse"
