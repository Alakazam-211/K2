#!/usr/bin/env bash
# 0.39.0g Phase 2.5b — first-boot consolidation: an existing workspace
# with `.k2so/agents/<x>/`, `.k2so/agent-templates/<y>/`, and bare-md
# `.k2so/skills/<z>.md` must have all three sources collapsed into
# `.k2so/skills/<name>/SKILL.md` automatically the first time the
# daemon boots after the 2.5b upgrade. The two source roots go to the
# Recycle Bin and a marker file is left so the next boot is a no-op.
#
# Two-phase: register the workspace under daemon A, kill A, then start
# daemon B which runs the sweep on a populated DB.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

sandbox_daemon_start

WS="$SANDBOX_HOME/test-workspace"
mkdir -p "$WS"
(cd "$WS" && git init -q && git commit --allow-empty -m "init" -q 2>/dev/null || true)

# Pre-mark the 0.37.0 unification + 0.39.0f work-to-inbox migrations
# as already-done. Those sweeps would otherwise grab our seeded
# `.k2so/agents/foo/` (treating it as a stray template) and trash
# the SKILL.md before our 2.5b consolidation gets a chance to see it.
# Phase 2.5b is a pure consolidation of an already-unified workspace.
mkdir -p "$WS/.k2so"
touch "$WS/.k2so/.unification-0.37.0-done" "$WS/.k2so/.work-to-inbox-migration-v1-done"

# Seed all three legacy sources BEFORE registration.
mkdir -p "$WS/.k2so/agents/foo" "$WS/.k2so/agent-templates/bar" "$WS/.k2so/skills"
cat >"$WS/.k2so/agents/foo/SKILL.md" <<'EOF'
---
name: foo
role: instance skill
---
foo body
EOF
cat >"$WS/.k2so/agent-templates/bar/AGENT.md" <<'EOF'
---
name: bar
role: template skill
---
bar body
EOF
cat >"$WS/.k2so/skills/baz.md" <<'EOF'
# Baz bare-md layer

body
EOF

# Register workspace via the live daemon so it lands in the DB the
# next boot will sweep.
REG_URL="http://127.0.0.1:${K2SO_PORT}/cli/projects/add-from-path?token=${K2SO_TOKEN}"
REG_BODY="$(python3 -c "import json,sys; print(json.dumps({'path': sys.argv[1]}))" "$WS")"
REG_RESP="$(curl -s -X POST -H "Content-Type: application/json" \
    --connect-timeout 5 --max-time 15 \
    -d "$REG_BODY" "$REG_URL" 2>&1 || true)"
if ! echo "$REG_RESP" | grep -qF "\"path\":\"$WS\""; then
    echo "FAIL: add-from-path response did not echo our workspace path" >&2
    echo "response: $REG_RESP" >&2
    exit 1
fi

# Sanity: marker must NOT exist yet.
MARKER="$WS/.k2so/.skills-consolidation-v1-done"
if [ -f "$MARKER" ]; then
    echo "FAIL: consolidation marker exists before second boot" >&2
    exit 1
fi

# Kill daemon A.
kill "$SANDBOX_DAEMON_PID" 2>/dev/null || true
wait "$SANDBOX_DAEMON_PID" 2>/dev/null || true
unset SANDBOX_DAEMON_PID

# Boot daemon B under the same sandbox HOME — its sweep should
# migrate.
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

# Verify the migration ran.
if [ ! -f "$MARKER" ]; then
    echo "FAIL: consolidation marker not created after second boot" >&2
    tail -60 "$SANDBOX_HOME/daemon-2.log" >&2 || true
    exit 1
fi

# Verify the three sources landed in `.k2so/skills/<name>/SKILL.md`.
for name in foo bar baz; do
    if [ ! -f "$WS/.k2so/skills/$name/SKILL.md" ]; then
        echo "FAIL: $name not consolidated into .k2so/skills/$name/SKILL.md" >&2
        ls -la "$WS/.k2so/skills/" >&2 || true
        exit 1
    fi
done

# Verify the template's AGENT.md was renamed to SKILL.md.
if [ -f "$WS/.k2so/skills/bar/AGENT.md" ]; then
    echo "FAIL: bar's AGENT.md was not renamed to SKILL.md" >&2
    ls -la "$WS/.k2so/skills/bar/" >&2 || true
    exit 1
fi

# Verify the source roots are gone (trash).
if [ -d "$WS/.k2so/agents" ]; then
    echo "FAIL: .k2so/agents/ still exists; should have been trashed" >&2
    exit 1
fi
if [ -d "$WS/.k2so/agent-templates" ]; then
    echo "FAIL: .k2so/agent-templates/ still exists; should have been trashed" >&2
    exit 1
fi

echo "OK: first-boot consolidation moved 3 sources → .k2so/skills/ and trashed source roots"
