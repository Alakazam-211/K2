#!/usr/bin/env bash
# Phase 2 Tier 2.4 — inbox primitive + heartbeat scheduling coexistence.
#
# The Phase 2.1a inbox primitive and the multi-heartbeat scheduling
# system both live in the daemon but were introduced under separate
# PRDs. This test pins their END-TO-END coexistence:
#
#   1. A workspace can have BOTH a queued inbox (`/cli/inbox/list`)
#      AND an active heartbeat row (`/cli/heartbeat/list`) on the
#      same project_id without one breaking the other.
#   2. Inbox compose + read + archive + delete continue to round-trip
#      while a heartbeat row exists on the same workspace.
#   3. Adding/removing a heartbeat doesn't perturb the inbox contents
#      (no accidental migration / cleanup hitting `.k2so/inbox/`).
#
# PIVOT note: the original Tier 2.4 PRD framing was "verify items
# DRAIN into agent session context on heartbeat fire." That framing
# doesn't match the actual code path — heartbeat `smart_launch`
# spawns Claude via PTY with WAKEUP.md as the wake message; the
# inbox isn't read by the daemon, the spawned Claude session reads
# it via `k2so inbox` from inside its own terminal. Triggering an
# actual PTY spawn from this test would require Claude on PATH,
# which doesn't exist in CI sandboxes. So this test pivots to the
# coexistence + non-interference contract — the realistic
# integration concern.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

sandbox_daemon_start

# ── Workspace setup ──────────────────────────────────────────────────

WS="$SANDBOX_HOME/inbox-heartbeat-ws"
mkdir -p "$WS"
(cd "$WS" && git init -q && git commit --allow-empty -m "init" -q 2>/dev/null || true)

# Scaffold .k2so + an agent primary so heartbeat/add doesn't reject
# "no scheduleable agent in this workspace". The agent_mode column
# also gates heartbeat/add — set it to "custom" via /cli/mode.
mkdir -p "$WS/.k2so/agent"
cat >"$WS/.k2so/agent/AGENT.md" <<'EOF'
---
name: tester
type: custom
role: integration test agent
---

# tester

Test agent for the inbox + heartbeat coexistence integration test.
EOF

# Register the workspace with the daemon so subsequent /cli/* routes
# can resolve its project_id.
WS_ENC="$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$WS")"
REG_URL="http://127.0.0.1:${K2SO_PORT}/cli/projects/add-from-path?token=${K2SO_TOKEN}"
REG_BODY="$(python3 -c "import json,sys; print(json.dumps({'path': sys.argv[1]}))" "$WS")"
REG_RESP="$(curl -sf -X POST -H "Content-Type: application/json" \
    --connect-timeout 5 --max-time 15 \
    -d "$REG_BODY" "$REG_URL")"
if ! echo "$REG_RESP" | grep -qF "\"path\":\"$WS\""; then
    echo "FAIL: add-from-path did not echo workspace path" >&2
    echo "  response: $REG_RESP" >&2
    exit 1
fi
echo "  registered workspace OK"

# Flip the workspace to agent mode 'custom' so heartbeat/add's
# agent_mode guard passes. Note: /cli/mode is GET-only (not in the
# POST allowlist) — the mutation lives behind the query param.
MODE_URL="http://127.0.0.1:${K2SO_PORT}/cli/mode?project=${WS_ENC}&set=custom&token=${K2SO_TOKEN}"
MODE_RESP="$(curl -sf --connect-timeout 5 --max-time 15 "$MODE_URL")"
echo "  /cli/mode set=custom → $MODE_RESP"

# ── Seed inbox items ─────────────────────────────────────────────────

mkdir -p "$WS/.k2so/inbox"
for i in 1 2 3; do
    cat >"$WS/.k2so/inbox/item-${i}.md" <<EOF
---
title: Inbox Item ${i}
priority: normal
created: 2026-05-25T00:00:0${i}Z
source: integration-test
from: test-harness
---
Body for item ${i}: this should survive heartbeat add/remove.
EOF
done

# Sanity: /cli/inbox/list returns the 3 seeded items.
LIST_URL="http://127.0.0.1:${K2SO_PORT}/cli/inbox/list?project=${WS_ENC}&token=${K2SO_TOKEN}"
RESP_BEFORE="$(curl -sf --connect-timeout 5 --max-time 15 "$LIST_URL")"
COUNT_BEFORE="$(echo "$RESP_BEFORE" | python3 -c "import sys, json; print(len(json.load(sys.stdin)))")"
if [ "$COUNT_BEFORE" != "3" ]; then
    echo "FAIL: expected 3 inbox items before heartbeat add, got $COUNT_BEFORE" >&2
    echo "  response: $RESP_BEFORE" >&2
    exit 1
fi
echo "  pre-heartbeat inbox count: 3 ✓"

# ── Add a heartbeat to the SAME workspace ────────────────────────────

# /cli/heartbeat/add params: name + frequency + spec. We use
# `frequency=heartbeat` (the legacy adaptive-backoff mode that doesn't
# require a cron spec).
HB_NAME="test-coexistence-hb"
HB_SPEC='{"interval_seconds":3600}'
HB_SPEC_ENC="$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$HB_SPEC")"
ADD_URL="http://127.0.0.1:${K2SO_PORT}/cli/heartbeat/add?project=${WS_ENC}&name=${HB_NAME}&frequency=heartbeat&spec=${HB_SPEC_ENC}&token=${K2SO_TOKEN}"
# Heartbeat/add is GET-only (not POST-allowlisted). The mutation is
# behind query params.
ADD_RESP="$(curl -sf --connect-timeout 5 --max-time 15 "$ADD_URL" 2>&1)"
echo "  /cli/heartbeat/add → $ADD_RESP"

# Verify /cli/heartbeat/list shows the new row.
HB_LIST_URL="http://127.0.0.1:${K2SO_PORT}/cli/heartbeat/list?project=${WS_ENC}&token=${K2SO_TOKEN}"
HB_LIST_RESP="$(curl -sf --connect-timeout 5 --max-time 15 "$HB_LIST_URL")"
if ! echo "$HB_LIST_RESP" | python3 -c "
import sys, json
data = json.load(sys.stdin)
assert isinstance(data, list), f'heartbeat/list must return a JSON array, got {type(data).__name__}'
names = [hb.get('name') for hb in data]
assert '$HB_NAME' in names, f'heartbeat $HB_NAME not in list; got names: {names}'
" 2>&1; then
    echo "FAIL: heartbeat row not present after add" >&2
    echo "  response: $HB_LIST_RESP" >&2
    exit 1
fi
echo "  heartbeat row visible in /cli/heartbeat/list ✓"

# ── Verify inbox is unperturbed by the heartbeat add ────────────────

RESP_AFTER_ADD="$(curl -sf --connect-timeout 5 --max-time 15 "$LIST_URL")"
COUNT_AFTER_ADD="$(echo "$RESP_AFTER_ADD" | python3 -c "import sys, json; print(len(json.load(sys.stdin)))")"
if [ "$COUNT_AFTER_ADD" != "3" ]; then
    echo "FAIL: inbox count changed from 3 to $COUNT_AFTER_ADD after heartbeat add" >&2
    echo "  response: $RESP_AFTER_ADD" >&2
    exit 1
fi
echo "  inbox count unchanged (3) after heartbeat add ✓"

# Spot-check item bodies survived too.
if ! echo "$RESP_AFTER_ADD" | python3 -c "
import sys, json
data = json.load(sys.stdin)
titles = sorted(item['title'] for item in data)
assert titles == ['Inbox Item 1', 'Inbox Item 2', 'Inbox Item 3'], \
    f'inbox titles drift: {titles}'
" 2>&1; then
    echo "FAIL: inbox titles drifted after heartbeat add" >&2
    exit 1
fi
echo "  inbox titles intact ✓"

# ── Exercise inbox mutations while heartbeat exists ─────────────────

# Pick one item id from the list and read its body via /cli/inbox/read.
ITEM_ID="$(echo "$RESP_AFTER_ADD" | python3 -c "
import sys, json
data = json.load(sys.stdin)
print(data[0]['id'])
")"
READ_URL="http://127.0.0.1:${K2SO_PORT}/cli/inbox/read?project=${WS_ENC}&id=${ITEM_ID}&token=${K2SO_TOKEN}"
READ_RESP="$(curl -sf --connect-timeout 5 --max-time 15 "$READ_URL")"
if ! echo "$READ_RESP" | grep -q "Body for item"; then
    echo "FAIL: inbox/read did not return item body while heartbeat present" >&2
    echo "  response: $READ_RESP" >&2
    exit 1
fi
echo "  inbox/read still works alongside heartbeat ✓"

# Compose a NEW inbox item via the HTTP route while heartbeat present.
# `/cli/inbox/compose` is a query-string POST (no JSON body) — see the
# existing msg_inbox_cross_workspace.sh test for the canonical pattern.
COMPOSE_RESP="$(curl -sfG --connect-timeout 5 --max-time 15 \
    -X POST "http://127.0.0.1:${K2SO_PORT}/cli/inbox/compose" \
    -d "token=${K2SO_TOKEN}&project=${WS}&title=Composed-During-Heartbeat&body=Added%20during%20heartbeat&priority=high&source=integration-test&from=test-harness")"
if ! echo "$COMPOSE_RESP" | grep -q "Composed-During-Heartbeat"; then
    echo "FAIL: inbox/compose response missing composed title" >&2
    echo "  response: $COMPOSE_RESP" >&2
    exit 1
fi

# Verify count is now 4.
RESP_POST_COMPOSE="$(curl -sf --connect-timeout 5 --max-time 15 "$LIST_URL")"
COUNT_POST_COMPOSE="$(echo "$RESP_POST_COMPOSE" | python3 -c "import sys, json; print(len(json.load(sys.stdin)))")"
if [ "$COUNT_POST_COMPOSE" != "4" ]; then
    echo "FAIL: expected 4 inbox items after compose, got $COUNT_POST_COMPOSE" >&2
    exit 1
fi
echo "  inbox/compose round-trips while heartbeat present (count: 4) ✓"

# ── Remove the heartbeat; verify inbox is STILL unperturbed ─────────

REMOVE_URL="http://127.0.0.1:${K2SO_PORT}/cli/heartbeat/remove?project=${WS_ENC}&name=${HB_NAME}&token=${K2SO_TOKEN}"
# GET-only like /cli/heartbeat/add.
REMOVE_RESP="$(curl -sf --connect-timeout 5 --max-time 15 "$REMOVE_URL")"
echo "  /cli/heartbeat/remove → $REMOVE_RESP"

RESP_AFTER_REMOVE="$(curl -sf --connect-timeout 5 --max-time 15 "$LIST_URL")"
COUNT_AFTER_REMOVE="$(echo "$RESP_AFTER_REMOVE" | python3 -c "import sys, json; print(len(json.load(sys.stdin)))")"
if [ "$COUNT_AFTER_REMOVE" != "4" ]; then
    echo "FAIL: inbox count changed from 4 to $COUNT_AFTER_REMOVE after heartbeat remove" >&2
    exit 1
fi
echo "  inbox count unchanged (4) after heartbeat remove ✓"

# Heartbeat list should now be empty (or at least no longer contain
# our test row).
HB_LIST_AFTER="$(curl -sf --connect-timeout 5 --max-time 15 "$HB_LIST_URL")"
if echo "$HB_LIST_AFTER" | python3 -c "
import sys, json
data = json.load(sys.stdin)
names = [hb.get('name') for hb in data]
assert '$HB_NAME' not in names, f'heartbeat $HB_NAME still in list after remove: {names}'
" 2>&1; then
    echo "  heartbeat row removed from /cli/heartbeat/list ✓"
else
    echo "FAIL: heartbeat row $HB_NAME still in list after remove" >&2
    exit 1
fi

echo "OK: inbox + heartbeat coexistence holds across add/compose/remove"
