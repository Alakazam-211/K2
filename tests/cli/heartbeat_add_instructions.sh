#!/usr/bin/env bash
# S5 (prd-heartbeat-firing-v1 HB32/HB34): `k2 heartbeat schedule add`
#   - sends --instructions / --instructions-file to the daemon (which
#     writes WAKEUP.md on its own disk) and never writes the returned
#     wakeupAbs path locally;
#   - without instructions, creates the heartbeat and says it needs them;
#   - refuses `--every 0` before any request;
#   - surfaces a daemon 400 (unknown frequency) as a failure;
#   - fails loudly against a daemon too old to write the instructions.
#
# No real daemon: a stub HTTP server records each request. HOME is
# sandboxed. Never touches the real ~/.k2 or launchd.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2; fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2; fail=$((fail + 1))
    fi
}
assert_not_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  FAIL: $label (found $(printf %q "$needle") in $(printf %q "$hay"))" >&2; fail=$((fail + 1))
    else
        echo "  PASS: $label"; pass=$((pass + 1))
    fi
}

WORK="$(mktemp -d -t k2-hb-add-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/ws"
LOG="$WORK/requests.jsonl"
: >"$LOG"
REMOTE_ABS="$WORK/daemon-disk/WAKEUP.md"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

LOG = os.environ["LOG"]
REMOTE_ABS = os.environ["REMOTE_ABS"]
MODE_FILE = os.environ["MODE_FILE"]

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        u = urlsplit(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query, keep_blank_values=True).items()}
        if u.path == "/cli/heartbeat/scheduler-status":
            self._send(200, {"transportInstalled": True, "staleSecs": 5, "enabledCount": 1})
            return
        if u.path != "/cli/heartbeat/add":
            self._send(404, {"error": "not found"})
            return
        with open(LOG, "a") as f:
            f.write(json.dumps(q) + "\n")
        if q.get("name") == "bad":
            self._send(400, {"error": "unknown frequency 'list' — expected one of hourly|daily|weekly|monthly|yearly|scheduled"})
            return
        mode = open(MODE_FILE).read().strip()
        has = bool(q.get("instructions", "").strip())
        body = {"id": "x", "name": q.get("name"), "wakeupPath": ".k2/heartbeats/x/WAKEUP.md",
                "wakeupAbs": REMOTE_ABS}
        if mode != "old":
            body["instructionsWritten"] = has
            body["waitReason"] = None if has else "wakeup_empty"
        self._send(200, body)

    # 0.44.4: heartbeat/add is POST-only (params stay in the query).
    def do_POST(self):
        self.do_GET()

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
MODE_FILE="$WORK/mode"
echo "new" >"$MODE_FILE"
echo "$PORT" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
LOG="$LOG" REMOTE_ABS="$REMOTE_ABS" MODE_FILE="$MODE_FILE" PORT="$PORT" python3 "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 40); do
    if curl -s -o /dev/null --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/heartbeat/scheduler-status"; then
        up=1; break
    fi
    sleep 0.05
done
[ "$up" = "1" ] || { echo "FAIL: stub server did not start" >&2; exit 1; }

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2SO_PROJECT_PATH \
        HOME="$HOME" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="fake-token" \
        K2_PROJECT_PATH="$WORK/ws" \
        "$K2_CLI" heartbeat schedule add "$@"
}
requests() { wc -l <"$LOG" | tr -d ' '; }
last_param() {
    tail -n 1 "$LOG" | python3 -c 'import json,sys; d=json.loads(sys.stdin.read()); k=sys.argv[1]; print(d[k] if k in d else "<absent>")' "$1"
}

echo "== --instructions goes to the daemon, not the local disk =="
set +e
out="$(run_k2 --name with-body --daily --time 07:00 --instructions 'check inbox' 2>&1)"
rc=$?
set -e
assert_eq "add with instructions exit" "$rc" "0"
assert_eq "one add request" "$(requests)" "1"
assert_eq "instructions param sent" "$(last_param instructions)" "check inbox"
assert_eq "frequency param sent" "$(last_param frequency)" "daily"
assert_contains "reports written" "$out" "Wake instructions written"
if [ -e "$REMOTE_ABS" ]; then
    echo "  FAIL: CLI wrote the daemon's wakeupAbs path locally" >&2; fail=$((fail + 1))
else
    echo "  PASS: CLI did not write wakeupAbs locally"; pass=$((pass + 1))
fi

echo "== --instructions-file is sent too =="
printf 'line one\nline two & more\n' >"$WORK/wake.md"
set +e
out="$(run_k2 --name from-file --hourly --every 15 --instructions-file "$WORK/wake.md" 2>&1)"
rc=$?
set -e
assert_eq "add with instructions-file exit" "$rc" "0"
assert_eq "file body sent verbatim" "$(last_param instructions)" "$(printf 'line one\nline two & more')"

echo "== no instructions: created, needs instructions =="
set +e
out="$(run_k2 --name no-body --hourly --every 1 2>&1)"
rc=$?
set -e
assert_eq "add without instructions exit" "$rc" "0"
assert_eq "no instructions param" "$(last_param instructions)" "<absent>"
assert_contains "says it needs instructions" "$out" "Needs instructions: WAKEUP.md is empty"
assert_contains "says it stays enabled" "$out" "stays enabled"
assert_not_contains "no written claim" "$out" "Wake instructions written"

echo "== --every 0 is refused before any request =="
before="$(requests)"
set +e
out="$(run_k2 --name zero --hourly --every 0 2>&1)"
rc=$?
set -e
assert_eq "every 0 exit" "$rc" "2"
assert_contains "every 0 message" "$out" "at least 1 minute"
assert_eq "no request for every 0" "$(requests)" "$before"

echo "== daemon 400 (unknown frequency) fails the command =="
set +e
out="$(run_k2 --name bad --daily 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then
    echo "  PASS: 400 exits non-zero ($rc)"; pass=$((pass + 1))
else
    echo "  FAIL: 400 exited 0" >&2; fail=$((fail + 1))
fi
assert_contains "400 names allowed values" "$out" "hourly|daily|weekly|monthly|yearly|scheduled"
assert_not_contains "no created line on 400" "$out" "Created heartbeat"

echo "== a daemon older than S5 fails loudly with instructions =="
echo "old" >"$MODE_FILE"
set +e
out="$(run_k2 --name old-daemon --daily --instructions 'check inbox' 2>&1)"
rc=$?
set -e
assert_eq "old daemon exit" "$rc" "1"
assert_contains "old daemon message" "$out" "did not write them"
if [ -e "$REMOTE_ABS" ]; then
    echo "  FAIL: CLI fell back to writing wakeupAbs locally" >&2; fail=$((fail + 1))
else
    echo "  PASS: no local fallback write"; pass=$((pass + 1))
fi

echo
echo "heartbeat_add_instructions: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
