#!/usr/bin/env bash
# GH#64: tray file send must keep sender prose, and must not exit 1
# from an unbound `_tray_remote_applied` on RETURN.
# Pure helpers — no daemon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="${ROOT}/cli/k2"

if [ ! -f "$K2_CLI" ]; then
    echo "FAIL: cli/k2 not found at $K2_CLI" >&2
    exit 1
fi

eval "$(sed -n '/^# BEGIN_MSG_INBOX_TRAY_RESTORE/,/^# END_MSG_INBOX_TRAY_RESTORE/p' "$K2_CLI")"
eval "$(sed -n '/^# BEGIN_MSG_INBOX_MERGE_BRIEF/,/^# END_MSG_INBOX_MERGE_BRIEF/p' "$K2_CLI")"

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# ── merge brief (#64 payload drop) ──────────────────────────────────
got="$(_msg_inbox_merge_brief "" ./brief.md -- ./brief.md "please review")"
[ "$got" = "please review" ] || fail "positional brief discarded; got: $got"
ok "positional brief kept when a file is present"

got="$(_msg_inbox_merge_brief "from-flag" ./a.md -- ./a.md extra words)"
[ "$got" = "from-flag extra words" ] || fail "flag body + extras; got: $got"
ok "--body plus leftover positionals merge"

got="$(_msg_inbox_merge_brief "" ./a.md ./b.pdf -- ./a.md ./b.pdf)"
[ -z "$got" ] || fail "files-only must not invent a brief; got: $got"
ok "files only → empty brief"

got="$(_msg_inbox_merge_brief "only-flag" ./a.md -- ./a.md)"
[ "$got" = "only-flag" ] || fail "--body alone; got: $got"
ok "--body without extra positionals"

# ── restore must not unbound under set -u (#64 trap) ────────────────
BASE_URL="http://active"
TOKEN="live-token"
_K2_TRAY_SAVED_BASE_URL="http://saved"
_K2_TRAY_SAVED_TOKEN="saved-token"
_K2_TRAY_REMOTE_APPLIED=1
_tray_restore_env
[ "$BASE_URL" = "http://saved" ] || fail "restore BASE_URL; got $BASE_URL"
[ "$TOKEN" = "saved-token" ] || fail "restore TOKEN; got $TOKEN"
[ "${_K2_TRAY_REMOTE_APPLIED}" = "0" ] || fail "applied flag not cleared"
ok "restore applied=1"

# Second call with applied=0 must not unbound-crash under set -u.
_tray_restore_env
ok "restore applied=0 is a no-op (no unbound)"

# Simulate the old RETURN-trap pattern: local gone, trap still fires.
(
    set -euo pipefail
    _K2_TRAY_REMOTE_APPLIED=0
    _tray_restore_env
) || fail "restore with applied=0 must not exit 1"
ok "set -u restore after function-local teardown"

echo "OK: msg_inbox_payload_and_trap"
