#!/usr/bin/env bash
# Unit test: msg-inbox upload strategy threshold (PRD P1).
# Pure helpers only — no daemon. < 50 MB → single-shot; ≥ 50 MB → chunked.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="${ROOT}/cli/k2"

if [ ! -f "$K2_CLI" ]; then
    echo "FAIL: cli/k2 not found at $K2_CLI" >&2
    exit 1
fi

# Extract pure policy helpers (BEGIN/END markers).
eval "$(sed -n '/^# BEGIN_MSG_INBOX_UPLOAD_POLICY/,/^# END_MSG_INBOX_UPLOAD_POLICY/p' "$K2_CLI")"

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# Constant sanity.
[ "${_MSG_INBOX_SINGLE_SHOT_MAX}" = "50000000" ] \
    || fail "SINGLE_SHOT_MAX expected 50000000 got ${_MSG_INBOX_SINGLE_SHOT_MAX}"
ok "SINGLE_SHOT_MAX=50000000"

# Below threshold → not chunked.
if _msg_inbox_use_chunked 0; then fail "size 0 should be single-shot"; fi
ok "size 0 → single-shot"

if _msg_inbox_use_chunked 1; then fail "size 1 should be single-shot"; fi
ok "size 1 → single-shot"

if _msg_inbox_use_chunked 49999999; then fail "size 49999999 should be single-shot"; fi
ok "size 49999999 → single-shot"

# At / above threshold → chunked.
if ! _msg_inbox_use_chunked 50000000; then fail "size 50000000 should be chunked"; fi
ok "size 50000000 → chunked"

if ! _msg_inbox_use_chunked 50000001; then fail "size 50000001 should be chunked"; fi
ok "size 50000001 → chunked"

if ! _msg_inbox_use_chunked 100000000; then fail "size 100000000 should be chunked"; fi
ok "size 100000000 → chunked"

# Non-numeric → not chunked (safe default: try single-shot / reject later).
if _msg_inbox_use_chunked ""; then fail "empty size should not be chunked"; fi
if _msg_inbox_use_chunked "abc"; then fail "non-numeric size should not be chunked"; fi
ok "non-numeric → not chunked"

echo "OK: msg_inbox_upload_threshold"
