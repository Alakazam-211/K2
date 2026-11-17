#!/usr/bin/env bash
# 0.46.15: `k2 heartbeat schedule add|edit --tz <IANA zone>`.
#   - add and edit send `tz` to the daemon (edit with --tz alone too);
#   - `--tz local` and `--tz ''` are sent as given (the daemon clears);
#   - no --tz sends no tz (a schedule edit keeps the zone);
#   - the reply's zone and next fire are shown there and here;
#   - the daemon's refusal of a bad zone fails the command, naming --tz;
#   - a daemon older than --tz fails loudly instead of claiming a zone;
#   - list shows a zoned heartbeat's next fire in its zone and here;
#   - `--tz` with no value is a usage error before any request.
#
# No real daemon: a stub HTTP server records each request. HOME is
# sandboxed. The CLI process runs with TZ=America/Denver, so "here" is
# Mountain time. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
python3 -c 'from zoneinfo import ZoneInfo; ZoneInfo("America/New_York")' \
    || { echo "FAIL: python3 has no zoneinfo data; this test needs it" >&2; exit 1; }

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
    if <<<"$hay" grep -Fq -- "$needle"; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2; fail=$((fail + 1))
    fi
}

WORK="$(mktemp -d -t k2-hb-tz-XXXXXX)"
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
MODE_FILE="$WORK/mode"
echo "new" >"$MODE_FILE"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

LOG = os.environ["LOG"]
MODE_FILE = os.environ["MODE_FILE"]
NEXT = "2026-11-17T19:15:00Z"  # 14:15 EST, 12:15 MST
ZONES = {"America/New_York", "Europe/London"}

def zone_reply(q):
    raw = q["tz"].strip()
    if raw in ("", "local"):
        return None, None
    if raw not in ZONES:
        return None, "--tz '%s' is not an IANA time zone (for example America/New_York); --tz local follows this computer's clock" % raw
    return raw, None

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
        if u.path == "/cli/heartbeat/list":
            self._send(200, [
                {"name": "close", "frequency": "daily", "specJson": "{\"time\":\"14:15\"}", "enabled": True,
                 "lastFired": None, "nextFireAt": NEXT, "waitReason": "scheduled", "tz": "America/New_York"},
                {"name": "local-one", "frequency": "daily", "specJson": "{\"time\":\"09:00\"}", "enabled": True,
                 "lastFired": None, "nextFireAt": "2026-11-17T16:00:00Z", "waitReason": "scheduled", "tz": None},
            ])
            return
        if u.path not in ("/cli/heartbeat/add", "/cli/heartbeat/edit"):
            self._send(404, {"error": "not found"})
            return
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "q": q}) + "\n")
        mode = open(MODE_FILE).read().strip()
        zone, err = (None, None)
        if "tz" in q:
            zone, err = zone_reply(q)
        if err:
            self._send(400, {"error": err})
            return
        if u.path == "/cli/heartbeat/add":
            body = {"id": "x", "name": q.get("name"), "wakeupPath": ".k2/heartbeats/x/WAKEUP.md",
                    "wakeupAbs": "/daemon/disk/WAKEUP.md", "instructionsWritten": False,
                    "waitReason": "wakeup_empty", "wakeupBodyBytes": 0, "instructionsKeptExisting": False,
                    "nextFireAt": NEXT, "tz": zone}
            self._send(200, body)
            return
        if mode == "old":
            # A daemon before --tz: ignores the param, plain success.
            self._send(200, {"success": True})
            return
        body = {"success": True}
        if "tz" in q:
            body.update({"tz": zone, "nextFireAt": NEXT})
        self._send(200, body)

    def do_POST(self):
        self.do_GET()

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo "$PORT" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
LOG="$LOG" MODE_FILE="$MODE_FILE" PORT="$PORT" python3 "$WORK/stub.py" &
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
        K2_PROJECT_PATH="$WORK/ws" TZ=America/Denver \
        "$K2_CLI" heartbeat schedule "$@"
}
requests() { wc -l <"$LOG" | tr -d ' '; }
last_param() {
    tail -n 1 "$LOG" | python3 -c 'import json,sys; d=json.loads(sys.stdin.read())["q"]; k=sys.argv[1]; print(d[k] if k in d else "<absent>")' "$1"
}
last_path() { tail -n 1 "$LOG" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["path"])'; }

echo "== add --tz sends the zone and shows the next fire there and here =="
set +e
out="$(run_k2 add --name close --daily --time 14:15 --tz America/New_York 2>&1)"
rc=$?
set -e
assert_eq "add --tz exit" "$rc" "0"
assert_eq "add route" "$(last_path)" "/cli/heartbeat/add"
assert_eq "tz param sent" "$(last_param tz)" "America/New_York"
assert_eq "spec still sent" "$(last_param spec)" '{"frequency": "daily", "time": "14:15"}'
assert_contains "names the zone" "$out" "Time zone: America/New_York"
assert_contains "next fire there and here" "$out" "Next fire: 2026-11-17 14:15 EST (2026-11-17 12:15 MST here)"

echo "== add without --tz sends no tz =="
set +e
out="$(run_k2 add --name plain --daily --time 09:00 2>&1)"
rc=$?
set -e
assert_eq "plain add exit" "$rc" "0"
assert_eq "no tz param" "$(last_param tz)" "<absent>"

echo "== a bad zone on add is refused, naming --tz =="
set +e
out="$(run_k2 add --name bad --daily --time 09:00 --tz Mars/Olympus 2>&1)"
rc=$?
set -e
assert_eq "bad zone add exit" "$rc" "1"
assert_contains "bad zone message" "$out" "--tz 'Mars/Olympus' is not an IANA time zone"
if <<<"$out" grep -Fq "Created heartbeat"; then
    echo "  FAIL: a refused add said Created" >&2; fail=$((fail + 1))
else
    echo "  PASS: no Created line on a refused add"; pass=$((pass + 1))
fi

echo "== edit --tz alone =="
set +e
out="$(run_k2 edit close --tz Europe/London 2>&1)"
rc=$?
set -e
assert_eq "edit --tz exit" "$rc" "0"
assert_eq "edit route" "$(last_path)" "/cli/heartbeat/edit"
assert_eq "edit tz param" "$(last_param tz)" "Europe/London"
assert_eq "edit sends no frequency" "$(last_param frequency)" "<absent>"
assert_contains "edit says updated" "$out" "Updated heartbeat: close"
assert_contains "edit names the zone" "$out" "Time zone: Europe/London"
assert_contains "edit next fire in London and here" "$out" "Next fire: 2026-11-17 19:15 GMT (2026-11-17 12:15 MST here)"

echo "== edit --tz local and --tz '' clear =="
set +e
out="$(run_k2 edit close --tz local 2>&1)"
rc=$?
set -e
assert_eq "tz local exit" "$rc" "0"
assert_eq "tz local sent" "$(last_param tz)" "local"
assert_contains "local: the computer's clock" "$out" "Time zone: this computer's clock"
assert_contains "local: next fire here" "$out" "Next fire: 2026-11-17 12:15 MST"
set +e
out="$(run_k2 edit close --tz '' 2>&1)"
rc=$?
set -e
assert_eq "tz empty exit" "$rc" "0"
assert_eq "tz empty sent as empty" "$(last_param tz)" ""
assert_contains "empty: the computer's clock" "$out" "Time zone: this computer's clock"

echo "== a schedule edit without --tz sends no tz =="
set +e
out="$(run_k2 edit close --daily --time 16:00 2>&1)"
rc=$?
set -e
assert_eq "spec edit exit" "$rc" "0"
assert_eq "spec edit: no tz param" "$(last_param tz)" "<absent>"
assert_eq "spec edit: frequency" "$(last_param frequency)" "daily"

echo "== a bad zone on edit is refused =="
set +e
out="$(run_k2 edit close --tz Nowhere 2>&1)"
rc=$?
set -e
assert_eq "bad zone edit exit" "$rc" "1"
assert_contains "bad zone edit message" "$out" "--tz 'Nowhere' is not an IANA time zone"

echo "== a daemon older than --tz fails loudly =="
echo "old" >"$MODE_FILE"
set +e
out="$(run_k2 edit close --daily --time 14:15 --tz America/New_York 2>&1)"
rc=$?
set -e
assert_eq "old daemon exit" "$rc" "1"
assert_contains "old daemon message" "$out" "older than --tz"
echo "new" >"$MODE_FILE"

echo "== --tz with no value: usage, no request =="
before="$(requests)"
set +e
out="$(run_k2 edit close --tz 2>&1)"
rc=$?
set -e
assert_eq "--tz no value exit" "$rc" "2"
assert_contains "--tz no value message" "$out" "--tz needs a zone"
assert_eq "no request for --tz with no value" "$(requests)" "$before"

echo "== list shows a zoned heartbeat's next fire there and here =="
set +e
out="$(run_k2 list 2>&1)"
rc=$?
set -e
assert_eq "list exit" "$rc" "0"
assert_contains "list zone line" "$out" "time zone America/New_York: next 2026-11-17 14:15 EST (2026-11-17 12:15 MST here)"
assert_eq "one zone line (the machine-clock row has none)" "$(<<<"$out" grep -c 'time zone ')" "1"

echo
echo "heartbeat_schedule_tz: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
