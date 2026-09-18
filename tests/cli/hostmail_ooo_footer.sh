#!/usr/bin/env bash
# hostmail ooo/footer: siblings of quota; --text literal|@path|-;
# k2 mail ooo|footer exit 2 moved-to-hostmail. No daemon required.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'M_TEXT="${M_TEXT:-}"' "$K2" || fail "_mail_py must export M_TEXT"
grep -q 'M_DAYS="${M_DAYS:-}"' "$K2" || fail "_mail_py must export M_DAYS"
grep -q 'M_UNTIL="${M_UNTIL:-}"' "$K2" || fail "_mail_py must export M_UNTIL"

if ! grep -A30 'verb == "ooo_set"' "$K2" | grep -q '/cli/mail/ooo'; then
    fail "ooo_set must POST /cli/mail/ooo"
fi
if ! grep -A20 'verb == "ooo_unset"' "$K2" | grep -q '/cli/mail/ooo/unset'; then
    fail "ooo_unset must POST /cli/mail/ooo/unset"
fi
if ! grep -A20 'verb == "ooo_get"' "$K2" | grep -q '/cli/mail/ooo'; then
    fail "ooo_get must GET /cli/mail/ooo"
fi
if ! grep -A20 'verb == "footer_set"' "$K2" | grep -q '/cli/mail/footer'; then
    fail "footer_set must POST /cli/mail/footer"
fi
if ! grep -A20 'verb == "footer_unset"' "$K2" | grep -q '/cli/mail/footer/unset'; then
    fail "footer_unset must POST /cli/mail/footer/unset"
fi

if grep -A40 'verb == "ooo_set"' "$K2" | grep -q 'VacationResponse'; then
    fail "ooo must not call VacationResponse"
fi
if grep -A40 'verb == "ooo_set"' "$K2" | grep -q 'x:SieveScript'; then
    fail "ooo must not call x:SieveScript"
fi

help="$("$K2" hostmail ooo set --help)"
printf '%s' "$help" | grep -q 'k2 hostmail ooo set' || fail "ooo set --help must be leaf: $help"
printf '%s' "$help" | grep -q -- '--text' || fail "ooo set --help must mention --text: $help"
if printf '%s' "$help" | grep -q 'k2 hostmail <command>'; then
    fail "ooo set --help must not be group help: $help"
fi

footer_help="$("$K2" hostmail footer set --help)"
printf '%s' "$footer_help" | grep -q 'k2 hostmail footer set' || fail "footer set --help must be leaf: $footer_help"
if printf '%s' "$footer_help" | grep -q 'signature'; then
    fail "footer help must not invent k2 hostmail signature: $footer_help"
fi

group="$("$K2" hostmail --help)"
printf '%s' "$group" | grep -q 'ooo set|unset|show' || fail "hostmail --help must list ooo: $group"
printf '%s' "$group" | grep -q 'footer set|unset|show' || fail "hostmail --help must list footer: $group"

out="$(mktemp -t k2-hostmail-ooo-XXXXXX)"
trap 'rm -f "$out"' EXIT

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail ooo set --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "ooo set without addr must exit 2, got $code: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail ooo set a@b.test --days 99 --text x >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "ooo set --days 99 must exit 2, got $code: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail footer set --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "footer set without --text must exit 2, got $code: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" mail ooo --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "k2 mail ooo must exit 2 moved-to-hostmail, got $code: $(cat "$out")"
fi
if ! grep -q "k2 hostmail ooo" "$out"; then
    fail "k2 mail ooo must teach k2 hostmail ooo: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" mail footer --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "k2 mail footer must exit 2 moved-to-hostmail, got $code: $(cat "$out")"
fi
if ! grep -q "k2 hostmail footer" "$out"; then
    fail "k2 mail footer must teach k2 hostmail footer: $(cat "$out")"
fi

echo "OK: hostmail ooo/footer help/parse"
