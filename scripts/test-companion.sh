#!/bin/bash
# K2SO Companion API Test Script
# Tests all mobile companion endpoints against a running K2SO instance.
#
# Prerequisites:
#   - K2SO running (dev or production) with companion enabled
#   - Companion credentials configured in Settings
#
# Usage:
#   ./scripts/test-companion.sh <ngrok-url> <username> <password>
#   ./scripts/test-companion.sh https://abc123.ngrok-free.app z3thon mr0ss0nl
#
# Or test against localhost (if you know the companion local port):
#   ./scripts/test-companion.sh http://localhost:PORT z3thon mr0ss0nl

set -euo pipefail

URL="${1:-}"
USERNAME="${2:-}"
PASSWORD="${3:-}"

if [ -z "$URL" ] || [ -z "$USERNAME" ] || [ -z "$PASSWORD" ]; then
    echo "Usage: ./scripts/test-companion.sh <url> <username> <password>" >&2
    echo "Example: ./scripts/test-companion.sh https://abc123.ngrok-free.app z3thon mypassword" >&2
    exit 1
fi

# Strip trailing slash
URL="${URL%/}"

PASS=0
FAIL=0
SKIP=0

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

passed() { ((PASS++)); echo -e "  ${GREEN}✓ PASS${NC}"; }
failed() { ((FAIL++)); echo -e "  ${RED}✗ FAIL: $1${NC}"; }
skipped() { ((SKIP++)); echo -e "  ${YELLOW}○ SKIP: $1${NC}"; }

# Helper: curl with ngrok header
api() {
    local method="$1"
    local endpoint="$2"
    shift 2
    curl -s -X "$method" \
        -H "ngrok-skip-browser-warning: true" \
        --max-time 10 \
        "$URL$endpoint" "$@"
}

echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo -e "${CYAN}  K2SO Companion API Test Suite${NC}"
echo -e "${CYAN}  Target: $URL${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo ""

# ── 1. Auth ──────────────────────────────────────────────────────────────
echo -e "${CYAN}1. POST /companion/auth${NC} — Login with Basic Auth"
AUTH_RESP=$(api POST "/companion/auth" \
    -H "Authorization: Basic $(echo -n "$USERNAME:$PASSWORD" | base64)")
echo "   Response: $AUTH_RESP"

TOKEN=$(echo "$AUTH_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('data',{}).get('token',''))" 2>/dev/null || echo "")
OK=$(echo "$AUTH_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('ok',False))" 2>/dev/null || echo "")

if [ "$OK" = "True" ] && [ -n "$TOKEN" ]; then
    passed
    echo -e "   Token: ${TOKEN:0:16}..."
else
    failed "Expected ok:true with token"
    echo "Aborting — cannot test authenticated endpoints without a token."
    exit 1
fi

AUTH_HEADER="Authorization: Bearer $TOKEN"
echo ""

# ── 2. Projects (global — no project param) ─────────────────────────────
echo -e "${CYAN}2. GET /companion/projects${NC} — List all workspaces"
RESP=$(api GET "/companion/projects" -H "$AUTH_HEADER")
echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])")"

PROJECT_COUNT=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('data',[])))")
if [ "$PROJECT_COUNT" -gt 0 ] 2>/dev/null; then
    passed
    echo "   Found $PROJECT_COUNT workspace(s)"
    # Extract first project path for subsequent tests
    PROJECT_PATH=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['data'][0]['path'])")
else
    failed "Expected at least 1 project"
    PROJECT_PATH=""
fi
echo ""

# ── 2b. Projects sentinel filter ───────────────────────────────────────
# Assert audit-bucket sentinels (_orphan, _broadcast) are filtered server-side.
# These rows live in the projects table but aren't real workspaces; if they leak
# into the companion response the mobile drawer renders bogus "Broadcast audit
# bucket" entries the user can't navigate into.
echo -e "${CYAN}2b. /companion/projects sentinel filter${NC} — _orphan/_broadcast must be excluded"
SENTINEL_CHECK=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
ids={p['id'] for p in d.get('data',[])}
leaked=[s for s in ('_orphan','_broadcast') if s in ids]
print('ok' if not leaked else 'leaked:'+','.join(leaked))
")
if [ "$SENTINEL_CHECK" = "ok" ]; then
    passed
else
    failed "Sentinel rows leaked into /companion/projects: $SENTINEL_CHECK"
fi
echo ""

# ── 3. Projects Summary (global) ────────────────────────────────────────
echo -e "${CYAN}3. GET /companion/projects/summary${NC} — Workspaces with counts"
RESP=$(api GET "/companion/projects/summary" -H "$AUTH_HEADER")
echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])")"

SUMMARY_OK=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') and isinstance(d.get('data'),list) else 'fail')")
if [ "$SUMMARY_OK" = "ok" ]; then
    passed
else
    failed "Expected ok:true with data array"
fi
echo ""

# ── 3b. Projects-summary sentinel filter ───────────────────────────────
echo -e "${CYAN}3b. /companion/projects/summary sentinel filter${NC}"
SENTINEL_CHECK=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
ids={p['id'] for p in d.get('data',[])}
leaked=[s for s in ('_orphan','_broadcast') if s in ids]
print('ok' if not leaked else 'leaked:'+','.join(leaked))
")
if [ "$SENTINEL_CHECK" = "ok" ]; then
    passed
else
    failed "Sentinel rows leaked into /companion/projects/summary: $SENTINEL_CHECK"
fi
echo ""

# ── 3c. Projects-summary fields ────────────────────────────────────────
# Verify each record carries the count fields the mobile app depends on.
echo -e "${CYAN}3c. /companion/projects/summary record shape${NC}"
SHAPE_CHECK=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
required={'id','name','path','color','agentMode','agentsRunning','reviewsPending'}
records=d.get('data',[])
if not records:
    print('fail:no records')
    sys.exit(0)
missing=set()
for r in records:
    missing |= required - set(r.keys())
print('ok' if not missing else 'missing:'+','.join(sorted(missing)))
")
if [ "$SHAPE_CHECK" = "ok" ]; then
    passed
else
    failed "Summary record missing required fields: $SHAPE_CHECK"
fi
echo ""

# ── 4. Sessions (global) ────────────────────────────────────────────────
echo -e "${CYAN}4. GET /companion/sessions${NC} — Active sessions across workspaces"
RESP=$(api GET "/companion/sessions" -H "$AUTH_HEADER")
echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])")"

SESSIONS_OK=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') and isinstance(d.get('data'),list) else 'fail')")
if [ "$SESSIONS_OK" = "ok" ]; then
    passed
else
    failed "Expected ok:true with data array"
fi
echo ""

# ── Project-scoped endpoints (require ?project=) ────────────────────────
if [ -z "$PROJECT_PATH" ]; then
    echo -e "${YELLOW}Skipping project-scoped endpoints — no project path available${NC}"
    SKIP=$((SKIP + 5))
else
    PROJ_PARAM="project=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$PROJECT_PATH'))")"

    # ── 5. Agents List ───────────────────────────────────────────────────
    echo -e "${CYAN}5. GET /companion/agents${NC} — List agents in workspace"
    RESP=$(api GET "/companion/agents?$PROJ_PARAM" -H "$AUTH_HEADER")
    echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])" 2>/dev/null || echo "$RESP")"

    AGENTS_OK=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') else 'fail')" 2>/dev/null || echo "fail")
    if [ "$AGENTS_OK" = "ok" ]; then
        passed
    else
        failed "Expected ok:true"
    fi
    echo ""

    # ── 6. Running Agents ────────────────────────────────────────────────
    echo -e "${CYAN}6. GET /companion/agents/running${NC} — Running terminal sessions"
    RESP=$(api GET "/companion/agents/running?$PROJ_PARAM" -H "$AUTH_HEADER")
    echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])" 2>/dev/null || echo "$RESP")"

    RUNNING_OK=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') else 'fail')" 2>/dev/null || echo "fail")
    if [ "$RUNNING_OK" = "ok" ]; then
        passed
    else
        failed "Expected ok:true"
    fi
    echo ""

    # ── 7. Status ────────────────────────────────────────────────────────
    echo -e "${CYAN}7. GET /companion/status${NC} — Workspace mode/name"
    RESP=$(api GET "/companion/status?$PROJ_PARAM" -H "$AUTH_HEADER")
    echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])" 2>/dev/null || echo "$RESP")"

    STATUS_OK=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') else 'fail')" 2>/dev/null || echo "fail")
    if [ "$STATUS_OK" = "ok" ]; then
        passed
    else
        failed "Expected ok:true"
    fi
    echo ""

    # ── 8. Reviews ───────────────────────────────────────────────────────
    echo -e "${CYAN}8. GET /companion/reviews${NC} — Review queue"
    RESP=$(api GET "/companion/reviews?$PROJ_PARAM" -H "$AUTH_HEADER")
    echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])" 2>/dev/null || echo "$RESP")"

    REVIEWS_OK=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') else 'fail')" 2>/dev/null || echo "fail")
    if [ "$REVIEWS_OK" = "ok" ]; then
        passed
    else
        failed "Expected ok:true"
    fi
    echo ""

    # ── 9. Agent Work ────────────────────────────────────────────────────
    echo -e "${CYAN}9. GET /companion/agents/work${NC} — Agent work items"
    # Use first agent name if available
    AGENT_NAME=$(echo "$RESP" | python3 -c "
import sys,json
try:
    # Try to get from agents list (endpoint 5 response may be stale, re-fetch)
    pass
except:
    pass
print('')
" 2>/dev/null || echo "")
    RESP=$(api GET "/companion/agents/work?$PROJ_PARAM" -H "$AUTH_HEADER")
    echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])" 2>/dev/null || echo "$RESP")"

    WORK_OK=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') else 'fail')" 2>/dev/null || echo "fail")
    if [ "$WORK_OK" = "ok" ]; then
        passed
    else
        failed "Expected ok:true"
    fi
    echo ""
fi

# ── 10. Presets ──────────────────────────────────────────────────────────
echo -e "${CYAN}10. GET /companion/presets${NC} — CLI LLM tool presets"
RESP=$(api GET "/companion/presets" -H "$AUTH_HEADER")
echo "   Response: $(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d,indent=2)[:500])" 2>/dev/null || echo "$RESP")"

PRESETS_OK=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') and isinstance(d.get('data'), list) else 'fail')" 2>/dev/null || echo "fail")
if [ "$PRESETS_OK" = "ok" ]; then
    passed
else
    failed "Expected ok:true with data array"
fi
echo ""

# ── 11. Auth failure (bad credentials) ──────────────────────────────────
echo -e "${CYAN}11. POST /companion/auth (bad password)${NC} — Should return 401"
RESP=$(api POST "/companion/auth" \
    -H "Authorization: Basic $(echo -n "$USERNAME:wrongpassword" | base64)")
echo "   Response: $RESP"

AUTH_FAIL=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') == False else 'fail')" 2>/dev/null || echo "fail")
if [ "$AUTH_FAIL" = "ok" ]; then
    passed
else
    failed "Expected ok:false"
fi
echo ""

# ── 12. Missing Bearer token ────────────────────────────────────────────
echo -e "${CYAN}12. GET /companion/projects (no auth)${NC} — Should return 401"
RESP=$(api GET "/companion/projects")
echo "   Response: $RESP"

NO_AUTH=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok' if d.get('ok') == False else 'fail')" 2>/dev/null || echo "fail")
if [ "$NO_AUTH" = "ok" ]; then
    passed
else
    failed "Expected ok:false (unauthorized)"
fi
echo ""

# ── Summary ──────────────────────────────────────────────────────────────
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo -e "  Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
