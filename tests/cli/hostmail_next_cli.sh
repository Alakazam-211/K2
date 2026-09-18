#!/usr/bin/env bash
# Next hostmail CLI parser (prd-hostmail-next-cli-v1 N8/N17).
# No live daemon. Do not steal `k2 mail list`. Redirect spam|queue|acl|autoconfig.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

# k2 mail list stays inbox catalog — do not add list to moved-to-hostmail.
if grep -E 'create\|status\|domain\|doctor\|config\|approvals\|spam\|queue\|acl\|autoconfig\|list\)' "$K2" | grep -q 'moved to'; then
    # The redirect line must include spam|queue|acl|autoconfig and must NOT include list.
    :
fi
# The moved-to-hostmail case for list must not exist as a mail-family redirect.
# `k2 mail list` is cmd_mail_inboxes.
grep -q 'list)        shift 2; cmd_mail_inboxes' "$K2" || fail "k2 mail list must still be cmd_mail_inboxes"

# Redirect spam|queue|acl|autoconfig only.
grep -q 'spam|queue|acl|autoconfig)' "$K2" || fail "k2 mail spam|queue|acl|autoconfig must redirect to hostmail"

# Do not steal k2 mail list via the redirect case.
python3 - <<'PY' || fail "redirect case must not include list"
import pathlib, re, sys
text = pathlib.Path("cli/k2").read_text()
# Find the mail-family moved-to-hostmail cases.
hits = re.findall(r"create\|status\|domain\|doctor\|config\|approvals\)[^\n]+moved to", text)
if not hits:
    # allow split cases
    pass
# The dedicated spam|queue|acl|autoconfig redirect must not mention list.
block = None
for m in re.finditer(r"spam\|queue\|acl\|autoconfig\)", text):
    block = text[m.start(): m.start()+400]
    break
if not block:
    sys.exit("missing spam|queue|acl|autoconfig redirect")
if re.search(r"(^|\|)list(\||\))", block.split(")",1)[0]):
    sys.exit("do not add list to moved-to-hostmail")
print("ok redirect")
PY

help="$("$K2" hostmail --help)"
printf '%s' "$help" | grep -q 'list create' || fail "hostmail --help must list mailing lists: $help"
printf '%s' "$help" | grep -q 'spam ham' || fail "hostmail --help must list spam: $help"
printf '%s' "$help" | grep -q 'queue list' || fail "hostmail --help must list queue: $help"
printf '%s' "$help" | grep -q 'acl grant' || fail "hostmail --help must list acl: $help"
printf '%s' "$help" | grep -q 'autoconfig show' || fail "hostmail --help must list autoconfig: $help"
printf '%s' "$help" | grep -q 'k2 mail list' || fail "hostmail --help must teach k2 mail list stays inboxes: $help"

# Bare k2 hostmail list = usage (exit 2), not address listing.
out="$(mktemp -t k2-hostmail-list-XXXXXX)"
set +e
"$K2" hostmail list >"$out" 2>&1
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "bare k2 hostmail list must exit 2 (usage), got $rc: $(cat "$out")"
grep -q 'list create' "$out" || fail "bare k2 hostmail list must print list usage: $(cat "$out")"

# k2 mail list --help is inboxes, not mailing lists.
mail_list_help="$("$K2" mail list --help)"
printf '%s' "$mail_list_help" | grep -qi 'inbox' || fail "k2 mail list --help must stay inboxes: $mail_list_help"
printf '%s' "$mail_list_help" | grep -q 'x:MailingList' && fail "k2 mail list --help must not be mailing lists: $mail_list_help"

# Redirects exit 2.
for verb in spam queue acl autoconfig; do
    set +e
    "$K2" mail "$verb" >"$out" 2>&1
    rc=$?
    set -e
    [ "$rc" -eq 2 ] || fail "k2 mail $verb must exit 2, got $rc: $(cat "$out")"
    grep -q "k2 hostmail $verb" "$out" || fail "k2 mail $verb must teach hostmail: $(cat "$out")"
done

# k2 hostmail create stays mint (help mentions mint/create address).
create_help="$("$K2" hostmail create --help)"
printf '%s' "$create_help" | grep -qi 'create' || fail "k2 hostmail create --help missing: $create_help"
printf '%s' "$create_help" | grep -q 'MailingList' && fail "k2 hostmail create must stay mint, not mailing list: $create_help"

# ACL is not k2 mail access.
acl_help="$("$K2" hostmail acl --help)"
printf '%s' "$acl_help" | grep -q 'mail access' || fail "acl help must mention k2 mail access is separate: $acl_help"

# Python helper exports new env.
grep -q 'M_MEMBERS="${M_MEMBERS:-}"' "$K2" || fail "_mail_py must export M_MEMBERS"
grep -q 'M_SENDER="${M_SENDER:-}"' "$K2" || fail "_mail_py must export M_SENDER"

echo "OK: hostmail next CLI parser (list/spam/queue/acl/autoconfig); k2 mail list still inboxes"
