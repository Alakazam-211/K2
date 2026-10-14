#!/usr/bin/env bash
# `k2 sources` B2 verbs (contract-sources-v1 §7, B2 region) against a
# headless sandbox daemon: pending, one-click approve (one and many), pin,
# simulate, the trigger verbs, the exit codes, and `k2 study data-sources`.
#
# Proposed sources are written straight into the sandbox DB (propose/
# validate are B1's), the way the daemon would store them. No trigger test
# wake here: that would wake a real agent (the wake path is covered by the
# daemon's unit tests through a seam).
#
# Build first:  cargo build -p k2-daemon
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v sqlite3 >/dev/null || { echo "FAIL: sqlite3 is required" >&2; exit 1; }

# Session variables reach the production daemon: drop them all.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2b2-XXXXXX)"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.3
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2
        tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

DHOME="$SANDBOX/home"
mkdir -p "$DHOME"
k2() { HOME="$DHOME" "$K2" "$@"; }
run() {
    set +e
    k2 "$@" >"$SANDBOX/out" 2>"$SANDBOX/err"
    rc=$?
    set -e
}

echo "== no daemon needed =="
run study data-sources
expect_eq "study data-sources exit 0" "$rc" "0"
expect_has "study page teaches the loop" "$(cat "$SANDBOX/out")" "k2 sources trigger propose --source fleet --on item_went_crit"
run study
expect_has "study catalog lists it" "$(cat "$SANDBOX/out")" "data-sources"

echo "== sandbox daemon =="
HOME="$DHOME" K2_AIRGAP=1 "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
OWNER_TOKEN="$(cat "$DHOME/.k2/heartbeat.token")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"
DB=""
for f in "$DHOME/.k2/k2.db" "$DHOME/.k2/k2so.db"; do [ -s "$f" ] && DB="$f" && break; done
[ -n "$DB" ] || { bad "found the sandbox DB"; exit 1; }

echo "== usage =="
run sources trigger help
expect_eq "trigger help exit 0" "$rc" "0"
expect_has "trigger help" "$(cat "$SANDBOX/out")" "stale_for:30m"
run sources simulate fleet purple
expect_eq "bad simulate state exits 2" "$rc" "2"
run sources trigger add --source fleet --on went_purple --agent x
expect_eq "bad --on exits 2" "$rc" "2"
run sources approve
expect_eq "approve without a name exits 2" "$rc" "2"

WS="$SANDBOX/ws"
mkdir -p "$WS"
git -C "$WS" -c init.defaultBranch=main init -q
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "http://127.0.0.1:$PORT/cli/projects/add-from-path?token=$OWNER_TOKEN")"
PID="$(printf '%s' "$reg" | python3 -c 'import json,sys; v=json.load(sys.stdin); print((v.get("project") or v).get("id",""))')"
[ -n "$PID" ] || { bad "workspace registered ($reg)"; exit 1; }
ok "workspace $PID"

HASH="$(printf '7%.0s' $(seq 1 64))"
sql() { sqlite3 -cmd ".timeout 5000" "$DB" "$1"; }
add_source() { # name status secret
    local sec="NULL"
    [ -n "${3:-}" ] && sec="'$3'"
    sql "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, every_secs, secret_name, viewers, agent_access, keep, status, made_by, made_by_project, rev, created_at, updated_at) VALUES ('id-$1', '$1', 'Label $1', '{\"fetch\":{\"url\":\"https://example.com/x\"}}', '$HASH', '[\"https://example.com:443\"]', 60, $sec, 'member', 'values', 'values', '$2', 'agent:$PID', '$PID', 1, 0, 0);"
}
add_source fleet proposed
add_source boxes proposed
add_source relays proposed
add_source deploys proposed VERCEL_RO

echo "== pending =="
run sources pending
expect_eq "pending exit 0" "$rc" "0"
expect_has "pending lists fleet as one click" "$(cat "$SANDBOX/out")" "fleet"
expect_has "pending says a secret is wanted" "$(cat "$SANDBOX/out")" "wants secret VERCEL_RO"
run sources pending --json --names fleet
expect_has "pending --json --names" "$(cat "$SANDBOX/out")" "\"approval\": \"one_click\""
if grep -q '"deploys"' "$SANDBOX/out"; then bad "--names filters"; else ok "--names filters"; fi

echo "== approve =="
run sources approve fleet
expect_eq "one-click approve exit 0" "$rc" "0"
expect_has "approve says active" "$(cat "$SANDBOX/out")" "fleet: active"
expect_eq "fleet is active in the DB" "$(sql "SELECT status || ':' || approved_by FROM sources WHERE name='fleet'")" "active:owner"
run sources approve boxes relays
expect_eq "batch approve exit 0" "$rc" "0"
expect_has "batch approve names both" "$(cat "$SANDBOX/out")" "approved: boxes, relays"
run sources approve nope
expect_eq "approving an unknown name exits 4" "$rc" "4"
# deploys needs a vault confirm (a Touch ID sheet on a Mac): not run here.

echo "== pin and simulate =="
run sources pin fleet
expect_has "pin" "$(cat "$SANDBOX/out")" "fleet: pinned"
expect_eq "pinned in the DB" "$(sql "SELECT pinned || ':' || pinned_by FROM sources WHERE name='fleet'")" "1:owner"
run sources unpin fleet
expect_has "unpin" "$(cat "$SANDBOX/out")" "fleet: not pinned"
run sources simulate fleet crit --for 10m
expect_eq "simulate exit 0" "$rc" "0"
expect_has "simulate says TEST" "$(cat "$SANDBOX/out")" "fleet: TEST crit until"
expect_eq "simulate stored" "$(sql "SELECT simulate_state || ':' || simulate_by FROM sources WHERE name='fleet'")" "crit:owner"
run sources simulate fleet off
expect_has "simulate off" "$(cat "$SANDBOX/out")" "simulate off"
expect_eq "simulate cleared" "$(sql "SELECT IFNULL(simulate_state, '-') FROM sources WHERE name='fleet'")" "-"

echo "== try (air-gapped sandbox: no request leaves) =="
run sources try --url https://example.com/status.json --items /list
expect_eq "try exit 0" "$rc" "0"
expect_has "try reports the air-gap" "$(cat "$SANDBOX/out")" "failed: airgap"
expect_has "try saves nothing" "$(cat "$SANDBOX/out")" "Nothing was saved"
run sources try --url http://example.com/x
expect_eq "try of plain http exits 1 (bad_spec)" "$rc" "1"
expect_has "https_only" "$(cat "$SANDBOX/err")" "bad_spec"
run sources try --name fleet --json
expect_has "try a saved source" "$(cat "$SANDBOX/out")" "\"name\": \"fleet\""

echo "== triggers =="
run sources trigger propose --source fleet --on item_went_crit --agent "$PID" --note "check the relay"
expect_eq "trigger propose exit 0" "$rc" "0"
TID="$(sed -n 's/^trigger \([0-9a-f-]*\): proposed$/\1/p' "$SANDBOX/out")"
[ -n "$TID" ] && ok "trigger id $TID" || bad "trigger id parsed: $(cat "$SANDBOX/out")"
run sources trigger propose --source fleet --on item_went_crit --agent "$PID"
expect_eq "duplicate trigger exits 1 (trigger_exists)" "$rc" "1"
expect_has "trigger_exists" "$(cat "$SANDBOX/err")" "trigger_exists"
run sources trigger list --source fleet
expect_has "list shows the proposal" "$(cat "$SANDBOX/out")" "proposed"
run sources pending
expect_has "pending lists the trigger" "$(cat "$SANDBOX/out")" "item_went_crit"
run sources trigger approve "${TID:0:8}"
expect_eq "approve by short id exit 0" "$rc" "0"
expect_has "approved" "$(cat "$SANDBOX/out")" ": active"
run sources trigger edit "$TID" --cooldown 10m --daily-cap 4 --no-note
expect_eq "edit exit 0" "$rc" "0"
expect_eq "edit stored" "$(sql "SELECT cooldown_secs || ':' || daily_cap || ':' || IFNULL(note,'-') FROM source_triggers WHERE id='$TID'")" "600:4:-"
run sources trigger add --source fleet --on stale_for:30m --agent "$PID"
expect_eq "add stale_for exit 0" "$rc" "0"
expect_eq "stale_for stored in seconds" "$(sql "SELECT stale_for_secs FROM source_triggers WHERE on_kind='stale_for'")" "1800"
run sources trigger pause "$TID"
expect_has "pause" "$(cat "$SANDBOX/out")" ": paused"
run sources trigger show "$TID"
expect_has "show prints the trigger" "$(cat "$SANDBOX/out")" "paused"
expect_has "show prints the fire log" "$(cat "$SANDBOX/out")" "No fires yet."
run sources trigger log "$TID" --json
expect_has "log --json" "$(cat "$SANDBOX/out")" "\"fires\": []"
run sources trigger remove "$TID"
expect_has "remove" "$(cat "$SANDBOX/out")" "removed"
run sources trigger show "$TID"
expect_eq "a removed trigger exits 4" "$rc" "4"
run sources trigger add --source ghost --on went_crit --agent "$PID"
expect_eq "an unknown source exits 4" "$rc" "4"

echo "== audit =="
expect_has "approvals and trigger changes are audited" \
    "$(sql "SELECT group_concat(action, ',') FROM (SELECT action FROM source_audit ORDER BY id)")" \
    "approve,batch_approve,pin,unpin,simulate,simulate_off,trigger_propose,trigger_approve,trigger_edit,trigger_add,trigger_pause,trigger_remove"

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
