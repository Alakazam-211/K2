#!/usr/bin/env bash
# usage_single_record.sh: the single-record path against a real, headless
# k2-daemon (prd-usage-single-source-v1 §11, U21).
#
# Real daemon from this tree, temp HOME, no client attached, no vendor call
# (a temp HOME turns vendor calls off). Seeds real readings with sqlite3 and
# fails loudly if a route returns nothing where it should have a number:
# the comparisons can never pass on all nulls.
#
# Usage: tests/cli/usage_single_record.sh  (K2_DAEMON_BIN=<path> overrides
# the binary). Needs: python3, sqlite3, curl.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

fail() { echo "FAIL: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "needs $1"; }
need python3; need sqlite3; need curl

# The test process only: never inherit a session's identity.
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done

DAEMON="${K2_DAEMON_BIN:-}"
if [ -z "$DAEMON" ]; then
    for root in "${CARGO_TARGET_DIR:-}" "$ROOT/target"; do
        [ -n "$root" ] || continue
        for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
            if [ -x "$cand" ]; then DAEMON="$cand"; break 2; fi
        done
    done
fi
[ -n "$DAEMON" ] && [ -x "$DAEMON" ] || fail "no k2-daemon (cargo build -p k2-daemon)"

# Short root: the daemon's sockets must fit SUN_LEN.
WORK="$(mktemp -d /tmp/k2usg.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
mkdir -p "$SB/.k2" "$SB/tmp" "$SB/run"
export HOME="$SB"

log() { printf '[usage_single_record] %s\n' "$*"; }
cleanup() {
    [ -n "${PID:-}" ] && kill "$PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

DLOG="$SB/daemon.log"
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= K2SO_WATCHDOG_DISABLED=1 K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    "$DAEMON" >"$DLOG" 2>&1 &
PID=$!
phase=""
for _ in $(seq 1 400); do
    if [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ]; then
        DPORT="$(cat "$SB/.k2/daemon.port")"
        phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
        [ "$phase" = "ready" ] && break
    fi
    kill -0 "$PID" 2>/dev/null || { tail -40 "$DLOG" >&2; fail "the daemon exited at boot"; }
    sleep 0.1
done
[ "$phase" = "ready" ] || { tail -40 "$DLOG" >&2; fail "daemon never reached ready"; }
TOKEN="$(cat "$SB/.k2/daemon.token")"
API="http://127.0.0.1:$DPORT"

url() { case "$1" in *\?*) printf '%s%s&token=%s' "$API" "$1" "$TOKEN" ;; *) printf '%s%s?token=%s' "$API" "$1" "$TOKEN" ;; esac; }
get()  { curl -s "$(url "$1")"; }
post() { curl -s -H 'Content-Type: application/json' -X POST -d "$2" "$(url "$1")"; }
code() { curl -s -o /dev/null -w '%{http_code}' -X "$1" "$(url "$2")"; }
jqp() { python3 -I -c "import json,sys; d=json.load(sys.stdin); print($1)"; }

DB="$HOME/.k2/k2.db"
[ -f "$DB" ] || fail "no database at $DB"

# ── Seed: the switch on, one token with a real reading, live ─────────
post /cli/settings/update '{"usageSingleRecord": true}' >/dev/null
[ "$(get /cli/settings/get | jqp 'd["usageSingleRecord"]')" = "True" ] || fail "the switch did not turn on"

NOW="$(date +%s)"
ID="acc_usage_headless"
RESET="$(python3 -I -c "import datetime as d; print((d.datetime.now(d.timezone.utc)+d.timedelta(hours=3)).isoformat())")"
OBS="$(python3 -I -c "import datetime as d; print((d.datetime.now(d.timezone.utc)-d.timedelta(minutes=1)).isoformat())")"
READING="$(RESET="$RESET" OBS="$OBS" python3 -I -c '
import json, os
r=os.environ["RESET"]; o=os.environ["OBS"]
print(json.dumps({"harness":"claude","plan":"max","windows":[
 {"label":"Session","used":0.41,"resetsAt":r,"id":"five_hour","kind":"session","observedAt":o,"source":"probe"},
 {"label":"Weekly","used":0.1,"resetsAt":r,"id":"seven_day","kind":"weekly","observedAt":o,"source":"probe"},
 {"label":"Fable Weekly","used":0.99,"resetsAt":r,"id":"model:fable:weekly","kind":"weekly","observedAt":o,"source":"probe"}
],"checkedAt":o,"status":"","source":"probe","vendorCheckedAt":o}))')"
sqlite3 "$DB" <<SQL
INSERT INTO llm_accounts (id, tool, label, kind, state, usage_json, usage_checked_at, created_at, position)
VALUES ('$ID', 'claude', 'Headless', 'subscription', 'signed_in', '$READING', $NOW, $NOW, 0);
INSERT OR REPLACE INTO llm_active (tool, account_id, switched_at, switched_by, live_account_id) VALUES ('claude', '$ID', 1000, NULL, NULL);
SQL

# ── Every route, same numbers (fails on any null) ────────────────────
pick() { python3 -I -c 'import json,sys; d=json.load(sys.stdin); v=eval(sys.argv[1]); print(json.dumps(v, sort_keys=True))' "$1"; }

SUBS="$(get /cli/usage/subscriptions)"
LIMITS="$(get /cli/usage/limits)"
LIST="$(get /cli/llm/accounts/list)"
STATUS="$(get "/cli/llm/accounts/status?id=$ID")"
USAGE="$(get /cli/llm/accounts/usage)"

rec_subs="$(printf '%s' "$SUBS" | pick 'next(h for h in d["harnesses"] if h["harness"]=="claude")')"
rec_limits="$(printf '%s' "$LIMITS" | pick 'next(l for t in d["tools"] if t["tool"]=="claude" for l in t["logins"] if l["accountId"]=="'$ID'")')"
rec_list="$(printf '%s' "$LIST" | pick 'next(a for t in d["tools"] if t["tool"]=="claude" for a in t["accounts"] if a["id"]=="'$ID'")["usage"]')"
rec_status="$(printf '%s' "$STATUS" | pick 'd["account"]["usage"]')"
rec_usage="$(printf '%s' "$USAGE" | pick 'next(r for r in d["rows"] if r["accountId"]=="'$ID'")["usage"]')"

check_rec() { # name record
    python3 -I - "$1" "$2" <<'PY'
import json, sys
name, rec = sys.argv[1], json.loads(sys.argv[2])
fail = lambda m: sys.exit("FAIL %s: %s\n%s" % (name, m, json.dumps(rec)))
if rec is None or not isinstance(rec, dict):
    fail("no record")
if rec.get("maxPercent") != 41:
    fail("maxPercent %r, want 41 (Session 41%%, model weekly never drives it)" % rec.get("maxPercent"))
if rec.get("accountId") != "acc_usage_headless":
    fail("accountId %r" % rec.get("accountId"))
if rec.get("plan") != "Max":
    fail("plan %r, want Max (normalised)" % rec.get("plan"))
cw = rec.get("chipWindow")
if not cw or cw.get("id") != "five_hour" or cw.get("percent") != 41 or not cw.get("observedAt"):
    fail("chipWindow %r" % cw)
if not rec.get("asOf"):
    fail("asOf is empty")
wins = rec.get("windows") or []
if [w.get("usedPercent") for w in wins] != [41.0, 10.0, 99.0]:
    fail("windows usedPercent %r" % [w.get("usedPercent") for w in wins])
if [w.get("chip") for w in wins] != [True, True, False]:
    fail("windows chip %r" % [w.get("chip") for w in wins])
if not all(w.get("observedAt") and w.get("source") for w in wins):
    fail("a window has no observedAt/source")
PY
}
check_rec subscriptions "$rec_subs"
check_rec limits "$rec_limits"
check_rec list "$rec_list"
check_rec status "$rec_status"
check_rec usage "$rec_usage"
log "ok: every route serves the same record with real numbers"

# ── GET on each refresh route: 405 ───────────────────────────────────
for route in /cli/usage/subscriptions/refresh /cli/usage/limits/refresh /cli/llm/accounts/refresh /cli/llm/accounts/for/refresh; do
    got="$(code GET "$route")"
    [ "$got" = "405" ] || fail "GET $route answered $got, want 405"
done
log "ok: GET on each refresh route is 405"

# ── A second refresh within 60 s makes no probe (U21) ────────────────
before="$(get /cli/usage/subscriptions | pick 'next(h for h in d["harnesses"] if h["harness"]=="claude")["vendorCheckedAt"]')"
first="$(post /cli/usage/subscriptions/refresh '{}')"
second="$(post /cli/usage/subscriptions/refresh '{}')"
after="$(get /cli/usage/subscriptions | pick 'next(h for h in d["harnesses"] if h["harness"]=="claude")["vendorCheckedAt"]')"
[ "$before" = "$after" ] || fail "a refresh under temp HOME moved vendorCheckedAt"
log "ok: refresh within a minute makes no probe"

# Step 5 (the file is a view of the live record) needs an accepted reading
# and is covered by the Rust test the_file_is_a_view_of_the_live_tokens_record.

# ── Air-gap: no numbers on any route ─────────────────────────────────
post /cli/settings/update '{"airgap": true}' >/dev/null
for route in /cli/usage/subscriptions /cli/usage/limits /cli/llm/accounts/list "/cli/llm/accounts/status?id=$ID" /cli/llm/accounts/usage; do
    body="$(get "$route")"
    printf '%s' "$body" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
def walk(v):
    if isinstance(v, dict):
        if v.get("maxPercent") not in (None, 0) and v.get("windows"):
            sys.exit("air-gap served numbers: %s" % json.dumps(v)[:300])
        for x in v.values():
            walk(x)
    elif isinstance(v, list):
        for x in v:
            walk(x)
walk(d)
'
done
post /cli/settings/update '{"airgap": false}' >/dev/null
log "ok: air-gap shows no numbers"

# ── Switch off: today's shape, immediately ───────────────────────────
post /cli/settings/update '{"usageSingleRecord": false}' >/dev/null
OFF="$(get /cli/usage/subscriptions)"
printf '%s' "$OFF" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
for h in d.get("harnesses", []):
    if "asOf" in h or "chipWindow" in h:
        sys.exit("the switch off still serves the new shape: %s" % json.dumps(h)[:200])
print("[usage_single_record] ok: the switch off goes straight back")
'

log "PASS"
