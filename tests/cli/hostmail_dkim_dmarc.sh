#!/usr/bin/env bash
# hostmail dkim/dmarc CLI surface (prd-hostmail-dkim-dmarc-v1 + vs-live K6–K16).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'M_SELECTOR="${M_SELECTOR:-}"' "$K2" || fail "_mail_py must export M_SELECTOR"
grep -q 'M_VERB="dkim_show"' "$K2" || fail "dkim show must set M_VERB"
grep -q 'M_VERB="dkim_rotate"' "$K2" || fail "dkim rotate must set M_VERB"
grep -q 'M_VERB="dkim_retire"' "$K2" || fail "dkim retire must set M_VERB"
grep -q 'M_VERB="dmarc_show"' "$K2" || fail "dmarc show must set M_VERB"
grep -q 'M_VERB="dmarc_report_to"' "$K2" || fail "dmarc report-to must set M_VERB"
grep -q '/cli/mail/dkim/retire' "$K2" || fail "retire must POST /cli/mail/dkim/retire"
grep -q '"/cli/mail/dkim"' "$K2" || fail "show/rotate must use /cli/mail/dkim"
grep -q '/cli/mail/dmarc' "$K2" || fail "dmarc must use /cli/mail/dmarc"

# Do not steal domain / rotate-admin / password.
"$K2" hostmail domain --help >/dev/null || fail "hostmail domain still exists"
"$K2" hostmail rotate-admin --help >/dev/null || fail "hostmail rotate-admin still exists"
"$K2" hostmail password --help >/dev/null || fail "hostmail password still exists"

help="$("$K2" hostmail --help)"
printf '%s' "$help" | grep -q 'dkim show|rotate|retire' || fail "hostmail help must list dkim"
printf '%s' "$help" | grep -q 'dmarc show|report-to' || fail "hostmail help must list dmarc"

dkim_help="$("$K2" hostmail dkim --help)"
printf '%s' "$dkim_help" | grep -q 'rotate' || fail "dkim help must mention rotate"

dmarc_help="$("$K2" hostmail dmarc report-to --help)"
printf '%s' "$dmarc_help" | grep -q -- '--address' || fail "dmarc report-to --help must mention --address"

# k2 mail dkim|dmarc moved to hostmail (exit 2).
set +e
out="$("$K2" mail dkim show example.test 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "k2 mail dkim must exit 2, got $rc ($out)"
printf '%s' "$out" | grep -q "hostmail" || fail "k2 mail dkim must point at k2 hostmail ($out)"

set +e
out="$("$K2" mail dmarc show example.test 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "k2 mail dmarc must exit 2, got $rc ($out)"

echo "OK: hostmail dkim/dmarc CLI surface"
