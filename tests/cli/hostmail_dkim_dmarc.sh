#!/usr/bin/env bash
# hostmail dkim/dmarc CLI surface (prd-hostmail-dkim-dmarc-v1 + vs-live K6–K16).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$CLI_BASH_SOURCE" || fail "bash -n cli/k2"

grep -q 'M_SELECTOR="${M_SELECTOR:-}"' "$CLI_BASH_SOURCE" || fail "_mail_py must export M_SELECTOR"
grep -q 'M_VERB="dkim_show"' "$CLI_BASH_SOURCE" || fail "dkim show must set M_VERB"
grep -q 'M_VERB="dkim_rotate"' "$CLI_BASH_SOURCE" || fail "dkim rotate must set M_VERB"
grep -q 'M_VERB="dkim_retire"' "$CLI_BASH_SOURCE" || fail "dkim retire must set M_VERB"
grep -q 'M_VERB="dmarc_show"' "$CLI_BASH_SOURCE" || fail "dmarc show must set M_VERB"
grep -q 'M_VERB="dmarc_report_to"' "$CLI_BASH_SOURCE" || fail "dmarc report-to must set M_VERB"
grep -q '/cli/mail/dkim/retire' "$CLI_BASH_SOURCE" || fail "retire must POST /cli/mail/dkim/retire"
grep -q '"/cli/mail/dkim"' "$CLI_BASH_SOURCE" || fail "show/rotate must use /cli/mail/dkim"
grep -q '/cli/mail/dmarc' "$CLI_BASH_SOURCE" || fail "dmarc must use /cli/mail/dmarc"

# Do not steal domain / rotate-admin / password.
"$K2" hostmail domain --help >/dev/null || fail "hostmail domain still exists"
"$K2" hostmail rotate-admin --help >/dev/null || fail "hostmail rotate-admin still exists"
"$K2" hostmail password --help >/dev/null || fail "hostmail password still exists"

help="$("$K2" hostmail --help)"
<<<"$help" grep -q 'dkim show|rotate|retire' || fail "hostmail help must list dkim"
<<<"$help" grep -q 'dmarc show|report-to' || fail "hostmail help must list dmarc"

dkim_help="$("$K2" hostmail dkim --help)"
<<<"$dkim_help" grep -q 'rotate' || fail "dkim help must mention rotate"

dmarc_help="$("$K2" hostmail dmarc report-to --help)"
<<<"$dmarc_help" grep -q -- '--address' || fail "dmarc report-to --help must mention --address"

# k2 mail dkim|dmarc moved to hostmail (exit 2).
set +e
out="$("$K2" mail dkim show example.test 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "k2 mail dkim must exit 2, got $rc ($out)"
<<<"$out" grep -q "hostmail" || fail "k2 mail dkim must point at k2 hostmail ($out)"

set +e
out="$("$K2" mail dmarc show example.test 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "k2 mail dmarc must exit 2, got $rc ($out)"

echo "OK: hostmail dkim/dmarc CLI surface"
