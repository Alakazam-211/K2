#!/usr/bin/env bash
# 0.39.0f Phase 2.1b — first-boot migration hook: an existing
# workspace with `.k2so/work/{inbox,active,done}/*.md` must have its
# items relocated to `.k2so/inbox/...` automatically the first time
# the daemon boots after the 2.1b upgrade. The work root goes to the
# Trash and a marker file is left so the next boot is a no-op.
#
# We seed the sandbox HOME with:
#   - A fake workspace at $SANDBOX_HOME/test-workspace/
#   - Three .md items: inbox, active, done
#   - A registered Project row in the daemon DB (via /cli/projects/add-from-path)
#     so the legacy-migrations sweep walks this path on boot.
#
# Two-phase: register the workspace under daemon A, kill A, then
# start daemon B which runs the sweep on a populated DB.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

# Boot the first daemon to register the workspace.
sandbox_daemon_start

WS="$SANDBOX_HOME/test-workspace"
mkdir -p "$WS"
# Initialise a tiny git repo — some workspace machinery requires it.
(cd "$WS" && git init -q && git commit --allow-empty -m "init" -q 2>/dev/null || true)

# Seed legacy work layout BEFORE registration so the sweep on a
# subsequent boot has work to do.
mkdir -p "$WS/.k2so/work/inbox" "$WS/.k2so/work/active" "$WS/.k2so/work/done"
cat >"$WS/.k2so/work/inbox/test-inbox.md" <<'EOF'
---
title: Test Inbox Item
priority: normal
---
This should land in .k2so/inbox/test-inbox.md after first-boot migration.
EOF
cat >"$WS/.k2so/work/active/test-active.md" <<'EOF'
---
title: Test Active Item
---
This should land in .k2so/inbox/active/test-active.md.
EOF
cat >"$WS/.k2so/work/done/test-done.md" <<'EOF'
---
title: Test Done Item
---
This should land in .k2so/inbox/done/test-done.md.
EOF

# Register the workspace via the daemon so it shows up in the DB
# that run_workspace_legacy_migrations_sweep walks on the next boot.
# /cli/projects/add-from-path is POST + JSON-bodied (PathBody { path }).
REG_URL="http://127.0.0.1:${K2SO_PORT}/cli/projects/add-from-path?token=${K2SO_TOKEN}"
REG_BODY="$(python3 -c "import json,sys; print(json.dumps({'path': sys.argv[1]}))" "$WS")"
REG_RESP="$(curl -s -X POST -H "Content-Type: application/json" \
    --connect-timeout 5 --max-time 15 \
    -d "$REG_BODY" "$REG_URL" 2>&1 || true)"
echo "  register response: $REG_RESP"

# Confirm the project landed. The POST returns the new Project row
# directly — easier than inventing a workspaces/list query (which is
# keyed by project_id, not a flat list).
if ! echo "$REG_RESP" | grep -qF "\"path\":\"$WS\""; then
    echo "FAIL: add-from-path response did not echo our workspace path" >&2
    echo "response: $REG_RESP" >&2
    exit 1
fi

# Sanity: marker shouldn't exist yet (we haven't booted with
# migration logic against this populated DB).
MARKER="$WS/.k2so/.work-to-inbox-migration-v1-done"
if [ -f "$MARKER" ]; then
    echo "FAIL: migration marker exists before second boot (the first boot ran the sweep too?)" >&2
    echo "  marker: $MARKER" >&2
    exit 1
fi

# Kill the first daemon so we can boot a fresh one against the
# now-populated DB.
kill "$SANDBOX_DAEMON_PID" 2>/dev/null || true
wait "$SANDBOX_DAEMON_PID" 2>/dev/null || true
unset SANDBOX_DAEMON_PID

# Boot a second daemon under the SAME sandbox HOME. The sweep on
# this boot is the one that should migrate.
HOME="$SANDBOX_HOME" "$SANDBOX_DAEMON_BIN" >"$SANDBOX_HOME/daemon-2.log" 2>&1 &
SANDBOX_DAEMON_PID=$!
export SANDBOX_DAEMON_PID
# Wait for the new port file to be (re)written.
for i in $(seq 1 50); do
    if [ -f "$SANDBOX_HOME/.k2so/daemon.port" ]; then
        # The sweep runs synchronously at startup before the listener
        # comes up, so by the time /health returns 200 the marker
        # should be present. Double-check with /health.
        NEW_PORT="$(cat "$SANDBOX_HOME/.k2so/daemon.port")"
        if curl -sf --connect-timeout 1 "http://127.0.0.1:${NEW_PORT}/health" >/dev/null 2>&1; then
            break
        fi
    fi
    sleep 0.2
done

# Verify the migration ran.
if [ ! -f "$MARKER" ]; then
    echo "FAIL: migration marker not created after second boot" >&2
    echo "  expected: $MARKER" >&2
    echo "log tail:" >&2
    tail -60 "$SANDBOX_HOME/daemon-2.log" >&2 || true
    exit 1
fi

# Verify the inbox now contains the migrated items.
if [ ! -f "$WS/.k2so/inbox/test-inbox.md" ]; then
    echo "FAIL: top-level inbox item not migrated to .k2so/inbox/" >&2
    ls -la "$WS/.k2so/" >&2 || true
    exit 1
fi
if [ ! -f "$WS/.k2so/inbox/active/test-active.md" ]; then
    echo "FAIL: active item not migrated" >&2
    ls -la "$WS/.k2so/inbox/" >&2 || true
    exit 1
fi
if [ ! -f "$WS/.k2so/inbox/done/test-done.md" ]; then
    echo "FAIL: done item not migrated" >&2
    ls -la "$WS/.k2so/inbox/" >&2 || true
    exit 1
fi

# Verify .k2so/work/ is gone (sent to Trash by safe_delete).
if [ -d "$WS/.k2so/work" ]; then
    echo "FAIL: .k2so/work/ still exists; should have been trashed" >&2
    ls -la "$WS/.k2so/" >&2 || true
    exit 1
fi

echo "OK: first-boot migration moved work → inbox and trashed work root"
