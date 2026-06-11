#!/bin/bash
# K2SO Companion WebSocket Protocol Test Script
# Tests all WS methods against a running K2SO instance.
#
# Prerequisites:
#   - K2SO running with companion enabled
#   - websocat installed (brew install websocat)
#
# Usage:
#   ./scripts/test-companion-ws.sh <url> <username> <password>
#   ./scripts/test-companion-ws.sh https://k2.ngrok.app z3thon mr0ss0nl

set -uo pipefail

URL="${1:-}"
USERNAME="${2:-}"
PASSWORD="${3:-}"

if [ -z "$URL" ] || [ -z "$USERNAME" ] || [ -z "$PASSWORD" ]; then
    echo "Usage: ./scripts/test-companion-ws.sh <url> <username> <password>" >&2
    exit 1
fi

URL="${URL%/}"
WS_URL=$(echo "$URL" | sed 's|^https://|wss://|; s|^http://|ws://|')

PASS=0
FAIL=0

GREEN='\033[0;32m'
RED='\033[0;31m'
CYAN='\033[0;36m'
NC='\033[0m'

passed() { ((PASS++)); echo -e "  ${GREEN}✓ PASS${NC}"; }
failed() { ((FAIL++)); echo -e "  ${RED}✗ FAIL: $1${NC}"; }

TMPDIR="/tmp/k2so_ws_test_$$"
mkdir -p "$TMPDIR"
trap "rm -rf $TMPDIR" EXIT

echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo -e "${CYAN}  K2SO Companion WebSocket Test Suite${NC}"
echo -e "${CYAN}  Target: $WS_URL${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo ""

# ── 0. Get auth token ────────────────────────────────────────────────────
echo -e "${CYAN}0. HTTP Auth (get token for WS)${NC}"
AUTH_RESP=$(curl -s --max-time 10 -X POST \
    -H "Authorization: Basic $(echo -n "$USERNAME:$PASSWORD" | base64)" \
    -H "ngrok-skip-browser-warning: true" \
    "$URL/companion/auth")
TOKEN=$(echo "$AUTH_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['token'])" 2>/dev/null || echo "")

if [ -z "$TOKEN" ]; then
    echo -e "  ${RED}✗ Failed to get auth token${NC}"
    exit 1
fi
echo -e "  Token: ${TOKEN:0:16}..."
echo ""

# Helper: send a WS method and save response to a tmpfile
ws_call() {
    local method="$1"
    local params="$2"
    local outfile="$3"
    local id="$RANDOM"
    local msg="{\"id\":\"$id\",\"method\":\"$method\",\"params\":$params}"
    (echo "$msg"; sleep 2) | websocat -B 4194304 "$WS_URL/companion/ws?token=$TOKEN" > "$outfile" 2>/dev/null || true
}

# Helper: check if file contains "result" key (simple grep)
has_result() {
    grep -q '"result"' "$1" 2>/dev/null
}

has_error() {
    grep -q '"error"' "$1" 2>/dev/null
}

count_data() {
    # Just check if it has data array — actual count not critical for pass/fail
    if grep -q '"data":\[' "$1" 2>/dev/null; then
        echo "1"  # non-zero = has data
    else
        echo "0"
    fi
}

# ── 1. Ping ──────────────────────────────────────────────────────────────
echo -e "${CYAN}1. method: ping${NC}"
ws_call "ping" "{}" "$TMPDIR/1.json"
if grep -q '"pong"' "$TMPDIR/1.json" 2>/dev/null; then
    passed
else
    failed "Expected pong response"
fi
echo ""

# ── 2. Projects List ─────────────────────────────────────────────────────
echo -e "${CYAN}2. method: projects.list${NC}"
ws_call "projects.list" "{}" "$TMPDIR/2.json"
COUNT=$(count_data "$TMPDIR/2.json")
if [ "$COUNT" -gt 0 ] 2>/dev/null; then
    passed
    echo "   Found $COUNT workspace(s)"
else
    failed "Expected projects list"
fi
echo ""

# ── 3. Projects Summary ──────────────────────────────────────────────────
echo -e "${CYAN}3. method: projects.summary${NC}"
ws_call "projects.summary" "{}" "$TMPDIR/3.json"
if has_result "$TMPDIR/3.json"; then
    passed
else
    failed "Expected summary data"
fi
echo ""

# ── 4. Sessions List ─────────────────────────────────────────────────────
echo -e "${CYAN}4. method: sessions.list${NC}"
ws_call "sessions.list" "{}" "$TMPDIR/4.json"
if has_result "$TMPDIR/4.json"; then
    passed
else
    failed "Expected sessions data"
fi
echo ""

# Get first project path for scoped tests
PROJECT_PATH=$(python3 -c "
import json, sys
with open('$TMPDIR/2.json') as f:
    for line in f:
        try:
            d = json.loads(line.strip())
            if 'result' in d and 'data' in d['result']:
                for p in d['result']['data']:
                    if isinstance(p, dict) and 'path' in p:
                        print(p['path']); sys.exit(0)
        except: pass
" 2>/dev/null || echo "")

if [ -z "$PROJECT_PATH" ]; then
    echo -e "${RED}Could not get project path — skipping scoped tests${NC}"
    FAIL=$((FAIL + 4))
else
    PROJ_JSON=$(python3 -c "import json; print(json.dumps('$PROJECT_PATH'))")
    PARAMS="{\"project\":$PROJ_JSON}"

    # ── 5. Agents List ───────────────────────────────────────────────────
    echo -e "${CYAN}5. method: agents.list${NC}"
    ws_call "agents.list" "$PARAMS" "$TMPDIR/5.json"
    if has_result "$TMPDIR/5.json"; then passed; else failed "Expected agents data"; fi
    echo ""

    # ── 6. Agents Running ────────────────────────────────────────────────
    echo -e "${CYAN}6. method: agents.running${NC}"
    ws_call "agents.running" "$PARAMS" "$TMPDIR/6.json"
    if has_result "$TMPDIR/6.json"; then passed; else failed "Expected running data"; fi
    echo ""

    # ── 7. Status ────────────────────────────────────────────────────────
    echo -e "${CYAN}7. method: status${NC}"
    ws_call "status" "$PARAMS" "$TMPDIR/7.json"
    if has_result "$TMPDIR/7.json"; then passed; else failed "Expected status data"; fi
    echo ""

    # ── 8. Reviews List ──────────────────────────────────────────────────
    echo -e "${CYAN}8. method: reviews.list${NC}"
    ws_call "reviews.list" "$PARAMS" "$TMPDIR/8.json"
    if has_result "$TMPDIR/8.json"; then passed; else failed "Expected reviews data"; fi
    echo ""
fi

# ── 9. Presets List (global) ─────────────────────────────────────────────
echo -e "${CYAN}9. method: presets.list${NC}"
ws_call "presets.list" "{}" "$TMPDIR/9.json"
if has_result "$TMPDIR/9.json"; then
    passed
else
    failed "Expected presets data"
fi
echo ""

# ── 10. Auth via WS ──────────────────────────────────────────────────────
echo -e "${CYAN}10. method: auth (over WS)${NC}"
(echo "{\"id\":\"a1\",\"method\":\"auth\",\"params\":{\"token\":\"$TOKEN\"}}"; sleep 2) \
    | websocat -B 65536 "$WS_URL/companion/ws" > "$TMPDIR/10.json" 2>/dev/null || true
if grep -q '"authenticated"' "$TMPDIR/10.json" 2>/dev/null; then
    passed
else
    failed "Expected authenticated:true"
fi
echo ""

# ── 11. Unauthenticated request ─────────────────────────────────────────
echo -e "${CYAN}11. Unauthenticated WS request${NC}"
(echo '{"id":"u1","method":"projects.list","params":{}}'; sleep 2) \
    | websocat -B 65536 "$WS_URL/companion/ws" > "$TMPDIR/11.json" 2>/dev/null || true
if has_error "$TMPDIR/11.json"; then
    passed
else
    failed "Expected error for unauthenticated request"
fi
echo ""

# ── 11. Unknown method ──────────────────────────────────────────────────
echo -e "${CYAN}12. Unknown method${NC}"
ws_call "nonexistent.method" "{}" "$TMPDIR/12.json"
if has_error "$TMPDIR/12.json"; then
    passed
else
    failed "Expected error for unknown method"
fi
echo ""

# ── 12. Terminal subscribe ───────────────────────────────────────────────
echo -e "${CYAN}13. method: terminal.subscribe${NC}"
ws_call "terminal.subscribe" '{"terminalId":"test-terminal"}' "$TMPDIR/13.json"
if grep -q '"subscribed"' "$TMPDIR/13.json" 2>/dev/null; then
    passed
else
    failed "Expected subscribed confirmation"
fi
echo ""

# ── Summary ──────────────────────────────────────────────────────────────
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo -e "  Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"

if [ "$FAIL" -gt 0 ]; then exit 1; fi
