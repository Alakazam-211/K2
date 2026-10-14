#!/usr/bin/env bash
# k2 sources — CLI smoke against a sandbox daemon (temp HOME) and a fake
# upstream on loopback (an Owner-typed exact IP source, `lan = 1`).
#
# Covers: add a fan-out source (one box up, one answering 500) → refresh
# --wait → the down box is a red item and the source stays live [GV55];
# list / read / show / status / export / history (human and --json);
# propose a public source (proposed, one-click approval); mute / unmute;
# pause / resume; edit; remove → 410 removed → name held (409); exit
# codes 2 (usage), 4 (not found), 1 (refused).
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

# Session variables reach the production daemon: drop them all.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2src-XXXXXX)"
DAEMON_PID=""
UP_PID=""
cleanup() {
    [ -n "$UP_PID" ] && kill "$UP_PID" 2>/dev/null || true
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -40 "$SANDBOX/daemon.log" >&2 || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

echo "== fake upstream =="
cat > "$SANDBOX/up.py" <<'PY'
import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/down"):
            self.send_response(500); self.send_header("Content-Length", "2"); self.end_headers(); self.wfile.write(b"{}")
            return
        body = json.dumps({"phase": "ready", "version": "0.45.6"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
python3 "$SANDBOX/up.py" > "$SANDBOX/up.port" &
UP_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/up.port" ] && break; sleep 0.1; done
UP="http://127.0.0.1:$(head -1 "$SANDBOX/up.port")"
ok "upstream on $UP"

echo "== sandbox daemon =="
DHOME="$SANDBOX/home"
mkdir -p "$DHOME"
HOME="$DHOME" K2_AIRGAP=0 "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
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
k2() { HOME="$DHOME" "$K2" "$@"; }
run() {
    set +e
    out="$(k2 "$@" 2>&1)"
    rc=$?
    set -e
}

echo "== add a fan-out source and check now =="
run sources add --name lan --label "LAN boxes" --target "a=$UP/ok" --target "b=$UP/down" \
    --item-field phase=/phase --item-field version=/version --rule 'phase:ok:eq:ready,warn:ne:ready' \
    --agg 'n=count' --trend n --every 30s --fail-after 1
expect_eq "add exit 0" "$rc" "0"
expect_has "add says active" "$out" "lan: active"
run sources refresh lan --wait
expect_eq "refresh --wait exit 0" "$rc" "0"
expect_has "read shows the up box" "$out" "a"
run sources read lan --json
expect_eq "read --json exit 0" "$rc" "0"
items="$(printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(" ".join("%s=%s/%s" % (i["key"], i["health"], i["reason"]) for i in d["items"]))')"
expect_eq "per-box states [GV55]" "$items" "a=ok/None b=crit/http_5xx"
phase="$(printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["phase"], d["health"], d["fields"]["n"], d["serverNow"] >= d["updatedAt"])')"
expect_eq "source live, crit, count 2, serverNow" "$phase" "live crit 2 True"

run sources list
expect_has "list line" "$out" "lan"
expect_has "list counts" "$out" "1 ok, 1 crit"
run sources show lan
expect_has "show: targets" "$out" "fetch: 2 targets"
run sources status
expect_has "status: poller running" "$out" "Poller: running"
run sources export lan
expect_has "export is JSON" "$out" "\"targets\""
run sources history lan n --json
expect_has "history has a point" "$out" "\"points\""
run sources history lan
expect_eq "history needs a field (usage)" "$rc" "2"

echo "== propose, mute, pause, edit =="
run sources propose --name status --label Status --url https://status.example.com/api/v2/summary.json --every 5m
expect_has "propose: proposed" "$out" "status: proposed"
expect_has "propose: one click" "$out" "one click"
run sources mute lan --for 30m --reason smoke
expect_has "mute" "$out" "muted for 30m"
run sources read lan --json
expect_has "muted shows who and why" "$out" "\"reason\": \"smoke\""
run sources unmute lan
expect_has "unmute" "$out" "unmuted"
run sources pause lan
expect_has "pause" "$out" "paused"
run sources read lan --json
expect_has "paused display" "$out" "\"display\": \"paused\""
run sources resume lan
expect_has "resume" "$out" "active"
run sources edit lan --label "LAN boxes (edited)"
expect_has "edit" "$out" "lan: active"
run sources mute lan --for 3d
expect_eq "too long mute is refused (exit 1)" "$rc" "1"
expect_has "too_long" "$out" "too_long"

echo "== remove and the name hold =="
run sources remove lan --yes
expect_has "removed" "$out" "Removed lan"
run sources read lan
expect_eq "removed → exit 4" "$rc" "4"
run sources add --name lan --label again --url "$UP/ok" --every 30s
expect_eq "held name → exit 1" "$rc" "1"
expect_has "name_held" "$out" "name_held"
run sources read nope
expect_eq "not found → exit 4" "$rc" "4"
run sources frobnicate
expect_eq "unknown verb → exit 2" "$rc" "2"
run sources add --label x --url https://a.example.com/
expect_eq "missing --name → exit 2" "$rc" "2"

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
