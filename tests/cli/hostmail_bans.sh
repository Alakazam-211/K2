#!/usr/bin/env bash
# hostmail bans/allowlist CLI wiring — no daemon / no Stalwart.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'cmd_hostmail_bans' "$K2" || fail "cmd_hostmail_bans missing"
grep -q 'cmd_hostmail_allowlist' "$K2" || fail "cmd_hostmail_allowlist missing"

grep -A20 'verb == "bans_list"' "$K2" | grep -q '/cli/mail/bans' \
  || fail "bans_list must GET /cli/mail/bans"
grep -A20 'verb == "bans_clear"' "$K2" | grep -q '/cli/mail/bans/clear' \
  || fail "bans_clear must POST /cli/mail/bans/clear"
grep -A20 'verb == "allowlist_add"' "$K2" | grep -q '/cli/mail/allowlist/add' \
  || fail "allowlist_add must POST /cli/mail/allowlist/add"
grep -A20 'verb == "bans_migrate_restore"' "$K2" | grep -q '/cli/mail/bans/migrate/restore' \
  || fail "migrate restore must POST restore"

help="$("$K2" hostmail bans --help)"
printf '%s' "$help" | grep -q 'bans list' || fail "bans --help must mention list"
printf '%s' "$help" | grep -q 'bans clear' || fail "bans --help must mention clear"
printf '%s' "$help" | grep -qi 'fail counter' || fail "bans --help must mention fail counters"

clear_help="$("$K2" hostmail bans clear --help)"
printf '%s' "$clear_help" | grep -q '65.130.10.89' || fail "clear --help should mention standing IPs warn path"
printf '%s' "$clear_help" | grep -q 'allowlist' || fail "clear --help must pair with allowlist"

add_help="$("$K2" hostmail allowlist add --help)"
printf '%s' "$add_help" | grep -q '65.130.10.89' || fail "allowlist add --help must print standing 65.130.10.89"
printf '%s' "$add_help" | grep -q '65.130.229.9' || fail "allowlist add --help must print standing 65.130.229.9"
printf '%s' "$add_help" | grep -q '172.56.0.0/16' || fail "allowlist add --help must refuse documenting 172.56"

"$K2" --schema 2>/dev/null | grep -q '"name": "hostmail bans list"' \
  || fail "schema missing hostmail bans list"
"$K2" --schema 2>/dev/null | grep -q '"name": "hostmail allowlist add"' \
  || fail "schema missing hostmail allowlist add"

set +e
out="$("$K2" hostmail bans clear 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "bans clear without ip must exit 2, got $rc: $out"

set +e
out="$("$K2" hostmail bans migrate 2>&1)"
rc=$?
set -e
# bare migrate prints help (exit 0) or usage; --hours required for start
help_m="$("$K2" hostmail bans migrate --help)"
printf '%s' "$help_m" | grep -q -- '--hours' || fail "migrate --help must mention --hours"
printf '%s' "$help_m" | grep -qi 'Never authBanPeriod\|never authBanPeriod' \
  || fail "migrate --help must say never authBanPeriod"
# Must not present authBanPeriod as a usable flag.
if printf '%s' "$help_m" | grep -qi -- '--auth-ban-period'; then
  fail "migrate --help must not expose authBanPeriod as a flag"
fi

set +e
out="$("$K2" mail bans 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "k2 mail bans must exit 2, got $rc ($out)"
printf '%s' "$out" | grep -q "hostmail" || fail "k2 mail bans must point at k2 hostmail ($out)"

set +e
out="$("$K2" mail allowlist 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "k2 mail allowlist must exit 2, got $rc ($out)"

# k2 mail list still inboxes (do not steal).
set +e
out="$("$K2" mail list --help 2>&1)"
rc=$?
set -e
printf '%s' "$out" | grep -qi 'inbox\|inboxes\|addresses' \
  || fail "k2 mail list --help must still be inbox catalog ($out)"
printf '%s' "$out" | grep -qi 'moved to.*hostmail list' \
  && fail "k2 mail list must NOT redirect to hostmail list"

# k2 hostmail list remains mailing lists.
list_help="$("$K2" hostmail list --help)"
printf '%s' "$list_help" | grep -qi 'MailingList\|mailing list\|list create' \
  || fail "k2 hostmail list --help must stay mailing lists ($list_help)"

if grep -A40 '_uds_eligible()' "$K2" | grep -q '/cli/mail'; then
  fail "_uds_eligible must not glob /cli/mail (bans stays TCP like quota)"
fi

echo "OK: hostmail bans/allowlist CLI wiring"
