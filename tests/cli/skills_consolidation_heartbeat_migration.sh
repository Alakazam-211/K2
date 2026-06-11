#!/usr/bin/env bash
# 0.39.0g Phase 2.5b — per-skill heartbeats migration. A workspace
# with `.k2so/agents/<skill>/heartbeats/<sched>/WAKEUP.md` must have
# the schedule moved to the workspace-level `.k2so/heartbeats/` with
# the skill name prefixed (`<skill>-<sched>`) so the workspace's
# scheduler treats it as a normal first-class schedule post-migration.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

sandbox_daemon_start

WS="$SANDBOX_HOME/test-workspace"
mkdir -p "$WS"
(cd "$WS" && git init -q && git commit --allow-empty -m "init" -q 2>/dev/null || true)

# Skip the 0.37.0 unification sweep so the seeded `.k2so/agents/`
# entry isn't mistaken for stray template residue.
mkdir -p "$WS/.k2so"
touch "$WS/.k2so/.unification-0.37.0-done" "$WS/.k2so/.work-to-inbox-migration-v1-done"

mkdir -p "$WS/.k2so/agents/scout/heartbeats/daily"
cat >"$WS/.k2so/agents/scout/SKILL.md" <<'EOF'
scout body
EOF
cat >"$WS/.k2so/agents/scout/heartbeats/daily/WAKEUP.md" <<'EOF'
---
description: scout's daily wakeup
---

daily scout body
EOF

REG_URL="http://127.0.0.1:${K2SO_PORT}/cli/projects/add-from-path?token=${K2SO_TOKEN}"
REG_BODY="$(python3 -c "import json,sys; print(json.dumps({'path': sys.argv[1]}))" "$WS")"
curl -s -X POST -H "Content-Type: application/json" \
    --connect-timeout 5 --max-time 15 \
    -d "$REG_BODY" "$REG_URL" >/dev/null 2>&1

kill "$SANDBOX_DAEMON_PID" 2>/dev/null || true
wait "$SANDBOX_DAEMON_PID" 2>/dev/null || true
unset SANDBOX_DAEMON_PID

HOME="$SANDBOX_HOME" "$SANDBOX_DAEMON_BIN" >"$SANDBOX_HOME/daemon-2.log" 2>&1 &
SANDBOX_DAEMON_PID=$!
export SANDBOX_DAEMON_PID
for i in $(seq 1 50); do
    if [ -f "$SANDBOX_HOME/.k2so/daemon.port" ]; then
        NEW_PORT="$(cat "$SANDBOX_HOME/.k2so/daemon.port")"
        if curl -sf --connect-timeout 1 "http://127.0.0.1:${NEW_PORT}/health" >/dev/null 2>&1; then
            break
        fi
    fi
    sleep 0.2
done

if [ ! -f "$WS/.k2so/.skills-consolidation-v1-done" ]; then
    echo "FAIL: marker missing after sweep" >&2
    tail -60 "$SANDBOX_HOME/daemon-2.log" >&2 || true
    exit 1
fi

if [ ! -f "$WS/.k2so/heartbeats/scout-daily/WAKEUP.md" ]; then
    echo "FAIL: per-skill heartbeat not migrated to .k2so/heartbeats/scout-daily/WAKEUP.md" >&2
    ls -la "$WS/.k2so/heartbeats/" >&2 || true
    exit 1
fi

# Skill itself should land in `.k2so/skills/scout/SKILL.md`.
if [ ! -f "$WS/.k2so/skills/scout/SKILL.md" ]; then
    echo "FAIL: scout skill not consolidated" >&2
    exit 1
fi

echo "OK: per-skill heartbeat migrated to workspace-level with skill-name prefix"
