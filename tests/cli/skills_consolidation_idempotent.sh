#!/usr/bin/env bash
# 0.39.0g Phase 2.5b — idempotent first-boot consolidation. Once the
# marker file exists, a second daemon boot must skip the migration
# entirely (no new errors, no further trashing).

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

# Seed a single instance so the first run has something to do.
mkdir -p "$WS/.k2so/agents/foo"
cat >"$WS/.k2so/agents/foo/SKILL.md" <<'EOF'
foo body
EOF

REG_URL="http://127.0.0.1:${K2SO_PORT}/cli/projects/add-from-path?token=${K2SO_TOKEN}"
REG_BODY="$(python3 -c "import json,sys; print(json.dumps({'path': sys.argv[1]}))" "$WS")"
curl -s -X POST -H "Content-Type: application/json" \
    --connect-timeout 5 --max-time 15 \
    -d "$REG_BODY" "$REG_URL" >/dev/null 2>&1

kill "$SANDBOX_DAEMON_PID" 2>/dev/null || true
wait "$SANDBOX_DAEMON_PID" 2>/dev/null || true
unset SANDBOX_DAEMON_PID

# Boot daemon B — first sweep, real work.
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

MARKER="$WS/.k2so/.skills-consolidation-v1-done"
if [ ! -f "$MARKER" ]; then
    echo "FAIL: marker missing after first migration boot" >&2
    tail -60 "$SANDBOX_HOME/daemon-2.log" >&2 || true
    exit 1
fi
if [ ! -f "$WS/.k2so/skills/foo/SKILL.md" ]; then
    echo "FAIL: foo not consolidated on first migration boot" >&2
    exit 1
fi

MARKER_MTIME_1="$(stat -f '%m' "$MARKER" 2>/dev/null || stat -c '%Y' "$MARKER")"

# Kill daemon B and boot daemon C — sweep must short-circuit on marker.
kill "$SANDBOX_DAEMON_PID" 2>/dev/null || true
wait "$SANDBOX_DAEMON_PID" 2>/dev/null || true
unset SANDBOX_DAEMON_PID

# Sleep enough that an unguarded re-run would bump the mtime.
sleep 1

HOME="$SANDBOX_HOME" "$SANDBOX_DAEMON_BIN" >"$SANDBOX_HOME/daemon-3.log" 2>&1 &
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

MARKER_MTIME_2="$(stat -f '%m' "$MARKER" 2>/dev/null || stat -c '%Y' "$MARKER")"
if [ "$MARKER_MTIME_1" != "$MARKER_MTIME_2" ]; then
    echo "FAIL: marker mtime changed between boots ($MARKER_MTIME_1 → $MARKER_MTIME_2); migration re-ran" >&2
    exit 1
fi

# Daemon log should show "consolidate_skills_v1 ran on 0" (or no
# consolidate-skills log line at all for this workspace).
if grep -q "consolidate_skills_v1(${WS}):" "$SANDBOX_HOME/daemon-3.log"; then
    echo "FAIL: daemon-3 log shows the consolidation re-ran for $WS" >&2
    grep "consolidate_skills_v1" "$SANDBOX_HOME/daemon-3.log" >&2 || true
    exit 1
fi

echo "OK: second boot short-circuited on marker (mtime unchanged, no re-run log)"
