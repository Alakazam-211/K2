#!/usr/bin/env bash
# `k2 sources` 0.46.1 rules and visibility against a headless sandbox
# daemon (daemon-first: no client attached):
#   - a person adds the Fleet Ops shape (lists, counts, ageGt, versionBehind
#     with a distance, warnAfter/critAfter, expect.status) and it stores and
#     reads back with the same keys;
#   - a bad rule is refused with its spec code;
#   - export feeds propose --from as is (plain query values kept);
#   - history health answers (empty) and history of a count is accepted;
#   - an agent-made proposal shows in the owner's pending list.
# The sandbox is air-gapped: no request leaves it.
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

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2rules-XXXXXX)"
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

HOME="$DHOME" K2_AIRGAP=1 "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"
DB=""
for f in "$DHOME/.k2/k2.db" "$DHOME/.k2/k2so.db"; do [ -s "$f" ] && DB="$f" && break; done
[ -n "$DB" ] || { bad "found the sandbox DB"; exit 1; }
sql() { sqlite3 -cmd ".timeout 5000" "$DB" "$1"; }

cat > "$SANDBOX/ops.json" <<'JSON'
{"name": "k2-ops-boxes", "label": "K2 boxes", "fetch": {"url": "https://k2.dev/api/ops/fleet?part=boxes"},
 "everySecs": 60, "expect": {"status": [200, 401]},
 "extract": {
   "fields": {"target_version": "/boxes/target_version", "ts": "/boxes/ts"},
   "counts": {"past_due_n": "/billing/past_due"},
   "items": {"pointer": "/boxes/items", "itemKey": "/label", "itemFields": {"version": "/version", "online": "/online"}},
   "lists": {"certs": {"pointer": "/certs/items", "itemKey": "/name", "itemFields": {"cert_days": "/days"}}},
   "aggregates": {"behind": {"op": "countWhere", "field": "version", "cond": {"versionBehind": {"field": "target_version"}}},
                  "certs_n": {"op": "count", "list": "certs"}}},
 "rules": [
   {"field": "ts", "crit": {"ageGt": 900}, "warn": {"ageGt": 300}, "ok": {"ageLt": 300}},
   {"field": "version", "warn": {"versionBehind": {"field": "target_version", "minor": 1}}, "ok": {"exists": true}, "warnAfter": 3},
   {"field": "online", "crit": {"eq": false}, "warn": {"eq": false}, "ok": {"eq": true}, "critAfter": 3},
   {"field": "cert_days", "crit": {"lt": 7}, "warn": {"lt": 21}, "ok": {"gte": 21}}]}
JSON

echo "== add the Fleet Ops shape =="
run sources add --from "$SANDBOX/ops.json"
expect_eq "add exit 0" "$rc" "0"
expect_has "add says active" "$(cat "$SANDBOX/out")" "k2-ops-boxes: active"
stored="$(sql "SELECT spec_json FROM sources WHERE name='k2-ops-boxes'")"
expect_has "expect stored" "$stored" '"expect":{"status":[200,401]}'
expect_has "lists stored" "$stored" '"lists":{"certs"'
expect_has "critAfter stored" "$stored" '"critAfter":3'
run sources export k2-ops-boxes
python3 - "$SANDBOX/out" "$SANDBOX/ops.json" <<'PY' && ok "export reads back every 0.46.1 key" || bad "export round trip: $(cat "$SANDBOX/out")"
import json, sys
got, want = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
assert got["expect"] == want["expect"], got["expect"]
assert got["extract"]["lists"] == want["extract"]["lists"]
assert got["extract"]["counts"] == want["extract"]["counts"]
assert got["rules"][1]["warnAfter"] == 3 and got["rules"][2]["critAfter"] == 3
assert got["fetch"]["url"].endswith("?part=boxes")
PY

echo "== refusals =="
python3 - "$SANDBOX/ops.json" "$SANDBOX/bad.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1])); d["name"] = "bad-one"
d["rules"][1]["warn"] = {"versionBehind": {"field": "nope"}}
json.dump(d, open(sys.argv[2], "w"))
PY
run sources add --from "$SANDBOX/bad.json" --json
expect_eq "a reference to a missing field exits 1" "$rc" "1"
expect_has "unknown_field" "$(cat "$SANDBOX/out")" "unknown_field"

echo "== history =="
run sources history k2-ops-boxes health
expect_eq "history health exit 0" "$rc" "0"
expect_has "no changes yet (air-gapped)" "$(cat "$SANDBOX/out")" "No state changes"
run sources history k2-ops-boxes past_due_n --json
expect_eq "history of a count exit 0" "$rc" "0"
run sources history k2-ops-boxes version
expect_eq "history of an item field exits 1" "$rc" "1"

echo "== pending =="
HASH="$(printf '7%.0s' $(seq 1 64))"
sql "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, every_secs, viewers, agent_access, keep, status, made_by, made_by_project, rev, created_at, updated_at) VALUES ('id-p', 'agent-prop', 'Agent prop', '{\"fetch\":{\"url\":\"https://example.com/x\"}}', '$HASH', '[\"https://example.com:443\"]', 60, 'member', 'values', 'values', 'proposed', 'agent:p1', 'p1', 1, 0, 0);"
run sources pending
expect_has "the owner sees the agent's proposal" "$(cat "$SANDBOX/out")" "agent-prop"

echo "== help =="
run sources propose --help
expect_has "propose --help lists critAfter" "$(cat "$SANDBOX/out")" "critAfter:N"

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
