#!/usr/bin/env bash
# `k2 remind snooze <name> --in|--at|--on <when>` snoozes that reminder.
#
# Regression: cmd_heartbeat_remind looked for --in/--at/--on anywhere in
# the words and, when it found one, skipped the verbs and set a NEW
# reminder whose text was "snooze <name>". Only the positional form
# (`snooze <name> 10m`) reached the snooze route. A time flag still makes a
# first word like `list` into text when setting a reminder; that stays.
#
# Hermetic: temp HOME, a stub daemon on loopback that logs each request's
# path and form, no real ~/.k2. Loud.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORKDIR="$(mktemp -d -t k2-remind-snooze-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORKDIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

python3 - "$WORKDIR" <<'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse

root = sys.argv[1]
log = os.path.join(root, "requests.jsonl")

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def answer(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        form = parse_qs(u.query)
        if n:
            form.update(parse_qs(self.rfile.read(n).decode()))
        form.pop("token", None)
        form.pop("project", None)
        with open(log, "a") as f:
            f.write(json.dumps({"path": u.path, "form": {k: v[0] for k, v in sorted(form.items())}}) + "\n")
        body = json.dumps({"name": (form.get("name") or ["r-abc123"])[0], "fires": "in 10 minutes"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = answer
    do_POST = answer

httpd = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(root, "stub.port"), "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do
    [ -s "$WORKDIR/stub.port" ] && break
    sleep 0.05
done
[ -s "$WORKDIR/stub.port" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }
export K2_PORT="$(cat "$WORKDIR/stub.port")"

# Runs `k2 <args>`, then checks the one request it sent: its path and its
# form (token and project left out), as sorted JSON.
expect() {
    local label="$1" want_path="$2" want_form="$3"; shift 3
    : > "$WORKDIR/requests.jsonl"
    local rc=0
    "$K2_CLI" "$@" >"$WORKDIR/out" 2>"$WORKDIR/err" || rc=$?
    if [ "$rc" != 0 ]; then
        bad "$label: exit $rc: $(cat "$WORKDIR/err")"
        return
    fi
    local got
    got="$(python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
if len(rows) != 1:
    print("requests=%d %s" % (len(rows), rows))
else:
    print(rows[0]["path"], json.dumps(rows[0]["form"], sort_keys=True))
' "$WORKDIR/requests.jsonl")"
    if [ "$got" = "$want_path $want_form" ]; then
        ok "$label"
    else
        bad "$label: got $got; want $want_path $want_form"
    fi
}

SNOOZE=/cli/heartbeat/reminder/snooze
ADD=/cli/heartbeat/reminder/add

echo "== snooze with a time flag snoozes =="
expect "remind snooze call-jane --in 10m" "$SNOOZE" '{"in": "10m", "name": "call-jane"}' \
    remind snooze call-jane --in 10m
expect "remind snooze call-jane --at 13:00 --tz Europe/Paris" "$SNOOZE" '{"at": "13:00", "name": "call-jane", "tz": "Europe/Paris"}' \
    remind snooze call-jane --at 13:00 --tz Europe/Paris
expect "remind snooze call-jane --on 2026-10-30" "$SNOOZE" '{"name": "call-jane", "on": "2026-10-30"}' \
    remind snooze call-jane --on 2026-10-30
expect "heartbeat remind snooze call-jane --in 1h" "$SNOOZE" '{"in": "1h", "name": "call-jane"}' \
    heartbeat remind snooze call-jane --in 1h

echo "== the positional form still snoozes =="
expect "remind snooze call-jane 10m" "$SNOOZE" '{"in": "10m", "name": "call-jane"}' \
    remind snooze call-jane 10m

echo "== setting a reminder still adds =="
expect "remind \"call Jane\" --in 10m" "$ADD" '{"in": "10m", "text": "call Jane"}' \
    remind "call Jane" --in 10m
expect "remind list --in 1m (list is text with a time)" "$ADD" '{"in": "1m", "text": "list"}' \
    remind list --in 1m

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
