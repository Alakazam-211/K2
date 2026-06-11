#!/bin/bash
# K2SO Behavioral Tests — Tier 1: Filesystem-Based
# Tests real system behavior without needing DB registration.
# Requires a running K2SO instance.
#
# Usage: ./tests/behavior-test-tier1.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TEST_WORKSPACE="${TEST_WORKSPACE:-/Users/z3thon/DevProjects/k2so-cli-test}"
K2SO_CLI="$PROJECT_ROOT/cli/k2so"
PASS=0; FAIL=0; SKIP=0

GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'; CYAN='\033[0;36m'; NC='\033[0m'
pass() { PASS=$((PASS + 1)); echo -e "  ${GREEN}PASS${NC} $1"; }
fail() { FAIL=$((FAIL + 1)); echo -e "  ${RED}FAIL${NC} $1"; echo -e "       ${RED}$2${NC}"; }
skip() { SKIP=$((SKIP + 1)); echo -e "  ${YELLOW}SKIP${NC} $1"; }
section() { echo ""; echo -e "${CYAN}── $1 ──${NC}"; }

run() { K2SO_PROJECT_PATH="$TEST_WORKSPACE" "$K2SO_CLI" "$@" 2>&1 || true; }

# Direct HTTP request helper (for endpoints not exposed via CLI)
http_get() {
    local PORT TOKEN endpoint params
    PORT=$(cat "$HOME/.k2so/heartbeat.port" 2>/dev/null)
    TOKEN=$(cat "$HOME/.k2so/heartbeat.token" 2>/dev/null)
    endpoint="$1"; shift
    params="token=${TOKEN}&project=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$TEST_WORKSPACE'))")"
    for p in "$@"; do params="${params}&${p}"; done
    curl -sG "http://127.0.0.1:${PORT}${endpoint}" -d "$params" --connect-timeout 3 --max-time 10 2>/dev/null
}

# Check K2SO is running
PORT=$(cat "$HOME/.k2so/heartbeat.port" 2>/dev/null || echo "")
if [ -z "$PORT" ]; then echo -e "${RED}K2SO is not running.${NC}"; exit 1; fi
HEALTH=$(curl -s --connect-timeout 2 "http://127.0.0.1:$PORT/health" 2>/dev/null || true)
if ! echo "$HEALTH" | grep -q '"ok"'; then echo -e "${RED}K2SO health check failed.${NC}"; exit 1; fi

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║     K2SO Behavioral Tests — Tier 1 (Filesystem-Based)      ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo -e "${GREEN}K2SO running on port $PORT${NC}"

# ═══════════════════════════════════════════════════════════════════════
section "1.1: Auto-Backoff Math"
# ═══════════════════════════════════════════════════════════════════════

# Setup: create agent with known heartbeat config
run agent create backoff-test --role "Backoff test agent" > /dev/null
run heartbeat set --agent backoff-test --interval 100 --phase monitoring > /dev/null

# Verify initial state
CONFIG=$(run heartbeat get --agent backoff-test)
if echo "$CONFIG" | grep -q "100"; then
    pass "backoff: initial interval is 100s"
else
    fail "backoff: initial interval" "Expected 100 in: $CONFIG"
fi

# Call noop 3 times
run heartbeat noop --agent backoff-test > /dev/null
run heartbeat noop --agent backoff-test > /dev/null
run heartbeat noop --agent backoff-test > /dev/null

# Read config — should have 3 no-ops and increased interval
CONFIG=$(run heartbeat get --agent backoff-test)
if echo "$CONFIG" | grep -q "No-ops:.*3"; then
    pass "backoff: 3 consecutive no-ops recorded"
else
    fail "backoff: no-op count" "Expected 3 no-ops in: $CONFIG"
fi

if echo "$CONFIG" | grep -q "150"; then
    pass "backoff: interval increased to 150s (100 * 1.5)"
else
    fail "backoff: interval increase" "Expected 150 in: $CONFIG"
fi

# 4th noop — should increase again
run heartbeat noop --agent backoff-test > /dev/null
CONFIG=$(run heartbeat get --agent backoff-test)
if echo "$CONFIG" | grep -q "225"; then
    pass "backoff: interval increased to 225s (150 * 1.5)"
else
    fail "backoff: second increase" "Expected 225 in: $CONFIG"
fi

# Action resets counter but NOT interval
run heartbeat action --agent backoff-test > /dev/null
CONFIG=$(run heartbeat get --agent backoff-test)
if echo "$CONFIG" | grep -q "No-ops:.*0"; then
    pass "backoff: action resets no-op counter to 0"
else
    fail "backoff: action reset" "Expected 0 no-ops in: $CONFIG"
fi

if echo "$CONFIG" | grep -q "225"; then
    pass "backoff: interval stays at 225s after action (only counter resets)"
else
    fail "backoff: interval preserved" "Expected 225 still in: $CONFIG"
fi

# ═══════════════════════════════════════════════════════════════════════
section "1.2: Lock Prevention in Triage"
# ═══════════════════════════════════════════════════════════════════════

run agent create lock-agent-a --role "Lock test A" > /dev/null
run agent create lock-agent-b --role "Lock test B" > /dev/null
run work create --title "Task for A" --body "test" --agent lock-agent-a --priority normal > /dev/null
run work create --title "Task for B" --body "test" --agent lock-agent-b --priority normal > /dev/null

# Both should appear in triage
TRIAGE=$(run agents triage)
if echo "$TRIAGE" | grep -q "lock-agent-a"; then
    pass "lock: agent-a appears in triage (unlocked)"
else
    fail "lock: agent-a triage" "Expected lock-agent-a in: $TRIAGE"
fi

# Lock agent-a
run agents lock lock-agent-a > /dev/null

# Triage should show LOCKED status
TRIAGE=$(run agents triage)
if echo "$TRIAGE" | grep -q "LOCKED"; then
    pass "lock: triage shows LOCKED status for locked agent"
else
    fail "lock: LOCKED status" "Expected LOCKED in: $TRIAGE"
fi

# Verify lock file on disk
if [ -f "$TEST_WORKSPACE/.k2so/agents/lock-agent-a/work/.lock" ]; then
    pass "lock: .lock file exists on disk"
else
    fail "lock: .lock file" "File not found"
fi

# Unlock
run agents unlock lock-agent-a > /dev/null
if [ ! -f "$TEST_WORKSPACE/.k2so/agents/lock-agent-a/work/.lock" ]; then
    pass "lock: .lock file removed after unlock"
else
    fail "lock: unlock" ".lock file still exists"
fi

# ═══════════════════════════════════════════════════════════════════════
section "1.3: Priority Ordering"
# ═══════════════════════════════════════════════════════════════════════

run agent create prio-low --role "Low priority test" > /dev/null
run agent create prio-crit --role "Critical priority test" > /dev/null
run agent create prio-norm --role "Normal priority test" > /dev/null

run work create --title "Low task" --body "test" --agent prio-low --priority low > /dev/null
run work create --title "Critical task" --body "test" --agent prio-crit --priority critical > /dev/null
run work create --title "Normal task" --body "test" --agent prio-norm --priority normal > /dev/null

TRIAGE=$(run agents triage)
# Check that all three appear
if echo "$TRIAGE" | grep -q "prio-crit" && echo "$TRIAGE" | grep -q "prio-norm" && echo "$TRIAGE" | grep -q "prio-low"; then
    pass "priority: all three agents appear in triage"
else
    fail "priority: agents missing" "Triage: $TRIAGE"
fi

# Check critical item is marked as such
if echo "$TRIAGE" | grep -q "critical"; then
    pass "priority: critical priority visible in triage"
else
    fail "priority: critical label" "Expected 'critical' in triage"
fi

# ═══════════════════════════════════════════════════════════════════════
section "1.4: Session Resume (DB-Backed)"
# ═══════════════════════════════════════════════════════════════════════
# Session IDs moved from `.last_session` files to the agent_sessions DB
# table in the v0.26 refactor. These tests seed a row, call heartbeat
# noop, and verify the `session_id` column was cleared.

DB_PATH="$HOME/.k2so/k2so.db"
PROJECT_ID=$(sqlite3 "$DB_PATH" "SELECT id FROM projects WHERE path='$TEST_WORKSPACE' LIMIT 1" 2>/dev/null || echo "")

if [ -z "$PROJECT_ID" ]; then
    skip "resume: project row not found in DB (test workspace not registered)"
    skip "resume: noop clears session_id"
    skip "resume: no stale session after pruning"
else
    run agent create resume-test --role "Session resume test" > /dev/null

    # Seed an agent_sessions row with a fake session_id.
    sqlite3 "$DB_PATH" \
        "INSERT OR REPLACE INTO agent_sessions (project_id, agent_name, session_id, created_at, updated_at) VALUES ('$PROJECT_ID', 'resume-test', 'test-session-abc123', strftime('%s','now'), strftime('%s','now'))" \
        2>/dev/null

    SEEDED=$(sqlite3 "$DB_PATH" "SELECT session_id FROM agent_sessions WHERE project_id='$PROJECT_ID' AND agent_name='resume-test'" 2>/dev/null)
    if [ "$SEEDED" = "test-session-abc123" ]; then
        pass "resume: session_id seeded in agent_sessions table"
    else
        fail "resume: seed" "Expected seeded session_id, got '$SEEDED'"
    fi

    # heartbeat noop should clear session_id (set to NULL).
    run heartbeat noop --agent resume-test > /dev/null

    POST_NOOP=$(sqlite3 "$DB_PATH" "SELECT IFNULL(session_id,'NULL') FROM agent_sessions WHERE project_id='$PROJECT_ID' AND agent_name='resume-test'" 2>/dev/null)
    if [ "$POST_NOOP" = "NULL" ] || [ -z "$POST_NOOP" ]; then
        pass "resume: noop clears session_id in agent_sessions table"
    else
        fail "resume: transcript pruning" "session_id still present: '$POST_NOOP'"
    fi

    # No-ops should make the next wake start a fresh session — verify by
    # querying the DB one more time and confirming it's still NULL.
    RECHECK=$(sqlite3 "$DB_PATH" "SELECT IFNULL(session_id,'NULL') FROM agent_sessions WHERE project_id='$PROJECT_ID' AND agent_name='resume-test'" 2>/dev/null)
    if [ "$RECHECK" = "NULL" ] || [ -z "$RECHECK" ]; then
        pass "resume: no stale session after pruning"
    else
        fail "resume: stale session" "session_id reappeared: '$RECHECK'"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
section "1.5: CLAUDE.md Content Validation"
# ═══════════════════════════════════════════════════════════════════════

run agent create claude-leader --role "Coordinator for CLAUDE.md test" > /dev/null
run agent create claude-member --role "Backend engineer for CLAUDE.md test" > /dev/null

# Generate CLAUDE.md for leader
run agents generate-md claude-leader > /dev/null

CLAUDE_FILE="$TEST_WORKSPACE/.k2so/agents/claude-leader/CLAUDE.md"
if [ -f "$CLAUDE_FILE" ]; then
    CONTENT=$(cat "$CLAUDE_FILE")

    if echo "$CONTENT" | grep -q "claude-member"; then
        pass "claude.md: lists other agents (claude-member)"
    else
        fail "claude.md: other agents" "Expected claude-member in CLAUDE.md"
    fi

    if echo "$CONTENT" | grep -q "CLI Tools\|k2so"; then
        pass "claude.md: includes CLI tools documentation"
    else
        fail "claude.md: CLI tools" "Expected CLI tools in CLAUDE.md"
    fi

    if echo "$CONTENT" | grep -q "Work Queue\|inbox\|active\|done"; then
        pass "claude.md: includes work queue info"
    else
        fail "claude.md: work queue" "Expected work queue section"
    fi
else
    fail "claude.md: file exists" "CLAUDE.md not found at $CLAUDE_FILE"
fi

# ═══════════════════════════════════════════════════════════════════════
section "1.6: Event Queue Flow"
# ═══════════════════════════════════════════════════════════════════════

run agent create event-test --role "Event queue test" > /dev/null

# Create a work item (this pushes an event internally)
run work create --title "Event trigger item" --body "test event flow" --agent event-test --priority high --source issue > /dev/null

# Drain events
EVENTS=$(http_get "/cli/events" "agent=event-test")
if echo "$EVENTS" | grep -q "work-item\|Event trigger"; then
    pass "events: work creation pushed event to queue"
else
    fail "events: push" "Expected work-item event in: $EVENTS"
fi

# Drain again — should be empty
EVENTS2=$(http_get "/cli/events" "agent=event-test")
if [ "$EVENTS2" = "[]" ]; then
    pass "events: queue drained (empty on second read)"
else
    fail "events: drain" "Expected empty array, got: $EVENTS2"
fi

# Create multiple items
run work create --title "Event A" --body "a" --agent event-test > /dev/null
run work create --title "Event B" --body "b" --agent event-test > /dev/null
run work create --title "Event C" --body "c" --agent event-test > /dev/null

EVENTS3=$(http_get "/cli/events" "agent=event-test")
# Count events (count "type" occurrences)
EVENT_COUNT=$(echo "$EVENTS3" | grep -o '"type"' | wc -l | xargs)
if [ "${EVENT_COUNT:-0}" -ge 3 ]; then
    pass "events: 3 events queued from 3 work items"
else
    fail "events: batch" "Expected 3 events, got $EVENT_COUNT"
fi

# ═══════════════════════════════════════════════════════════════════════
section "1.7: No-Op Transcript Pruning (DB-Backed)"
# ═══════════════════════════════════════════════════════════════════════
# Clean standalone test mirroring 1.4 but with a different agent name so
# the prior test's cleanup can't mask regressions.

if [ -z "$PROJECT_ID" ]; then
    skip "pruning: session row clear after noop"
else
    run agent create prune-test --role "Pruning test" > /dev/null

    sqlite3 "$DB_PATH" \
        "INSERT OR REPLACE INTO agent_sessions (project_id, agent_name, session_id, created_at, updated_at) VALUES ('$PROJECT_ID', 'prune-test', 'session-to-prune', strftime('%s','now'), strftime('%s','now'))" \
        2>/dev/null

    SEEDED=$(sqlite3 "$DB_PATH" "SELECT session_id FROM agent_sessions WHERE project_id='$PROJECT_ID' AND agent_name='prune-test'" 2>/dev/null)
    if [ "$SEEDED" = "session-to-prune" ]; then
        pass "pruning: session_id seeded before noop"
    else
        fail "pruning: setup" "Expected seeded session_id, got '$SEEDED'"
    fi

    run heartbeat noop --agent prune-test > /dev/null

    POST=$(sqlite3 "$DB_PATH" "SELECT IFNULL(session_id,'NULL') FROM agent_sessions WHERE project_id='$PROJECT_ID' AND agent_name='prune-test'" 2>/dev/null)
    if [ "$POST" = "NULL" ] || [ -z "$POST" ]; then
        pass "pruning: session_id cleared after noop"
    else
        fail "pruning: delete" "session_id still present: '$POST'"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
section "CLEANUP"
# ═══════════════════════════════════════════════════════════════════════

echo "  Cleaning up test data..."
rm -rf "$TEST_WORKSPACE/.k2so" 2>/dev/null || true
pass "cleanup complete"

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo -e "║  Tier 1 Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC}     ║"
echo "╚══════════════════════════════════════════════════════════════╝"

if [ "$FAIL" -gt 0 ]; then exit 1; fi
