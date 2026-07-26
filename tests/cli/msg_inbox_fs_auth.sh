#!/usr/bin/env bash
# Unit test: msg-inbox fs/upload auth-error detector (passport staging teach).
# Pure helper only — no daemon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="${ROOT}/cli/k2"

if [ ! -f "$K2_CLI" ]; then
    echo "FAIL: cli/k2 not found at $K2_CLI" >&2
    exit 1
fi

# Extract pure auth helpers (BEGIN/END markers).
eval "$(sed -n '/^# BEGIN_MSG_INBOX_FS_AUTH/,/^# END_MSG_INBOX_FS_AUTH/p' "$K2_CLI")"

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# Positive: daemon 403 body for missing/garbage token.
if ! _msg_inbox_is_fs_auth_error '{"error":"invalid or missing token"}'; then
    fail "invalid or missing token should be auth error"
fi
ok "invalid or missing token → auth"

if ! _msg_inbox_is_fs_auth_error '{"error":{"code":"forbidden","hint":"not allowed"}}'; then
    fail "error.code=forbidden should be auth error"
fi
ok "error.code=forbidden → auth"

if ! _msg_inbox_is_fs_auth_error '{"error":{"code":"owner_only","hint":"owner surface"}}'; then
    fail "owner_only should be auth error"
fi
ok "owner_only → auth"

if ! _msg_inbox_is_fs_auth_error '{"error":"Forbidden"}'; then
    fail "Forbidden string should be auth error"
fi
ok "Forbidden → auth"

# Negative: ordinary deliver / path errors must NOT look like passport gates.
if _msg_inbox_is_fs_auth_error '{"error":{"code":"not_a_readable_file","hint":"source path is not a readable file"}}'; then
    fail "not_a_readable_file must not be fs-auth"
fi
ok "not_a_readable_file → not auth"

if _msg_inbox_is_fs_auth_error '{"path":"/tmp/staged.bin"}'; then
    fail "success path body must not be fs-auth"
fi
ok "success path → not auth"

if _msg_inbox_is_fs_auth_error ''; then
    fail "empty body must not be fs-auth"
fi
ok "empty → not auth"

if _msg_inbox_is_fs_auth_error '{"ok":true,"id":"abc"}'; then
    fail "ok package body must not be fs-auth"
fi
ok "ok package → not auth"

echo "OK: msg_inbox_fs_auth"
