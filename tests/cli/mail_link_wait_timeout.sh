#!/usr/bin/env bash
# bash-only: it fakes the shell CLI's Python clock (a sitecustomize on PYTHONPATH), which a binary does not run.
#
# When an OAuth link, relink or upgrade runs out of time waiting for the
# person's approval, the hint names the command to run again with its own
# verb: `k2 mail link relink <address>` for a relink, `k2 mail link upgrade
# <address> --features <f>` for an upgrade, and `k2 mail link add
# --provider <p>` for a new link. The wait is 15 minutes of real time, so
# the CLI's Python gets a clock whose sleep jumps past the deadline: one
# status poll, then the timeout. A fake daemon answers; no live daemon.
# Fail-loud: no skip-if-missing.
# Run with: bash tests/cli/mail_link_wait_timeout.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_BASH_SOURCE"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_exit() { if [ "$3" = "$2" ]; then ok "$1 (exit $3)"; else bad "$1: expected exit $2, got $3"; fi; }
expect_in() { if printf '%s' "$2" | grep -qF -- "$3"; then ok "$1"; else bad "$1: '$3' not in: $2"; fi; }
expect_not_in() { if printf '%s' "$2" | grep -qF -- "$3"; then bad "$1: '$3' in: $2"; else ok "$1"; fi; }

WORKDIR="$(mktemp -d -t k2-mail-link-wait-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

# The fake daemon: start answers a device flow, status answers pending.
# Each request's path is appended to requests.log.
python3 - "$WORKDIR" <<'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
root = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def answer(self):
        path = self.path.split("?", 1)[0]
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        with open(os.path.join(root, "requests.log"), "a") as f:
            f.write(path + "\n")
        if path == "/cli/mail/link/oauth/start":
            body = {"ok": True, "flow": "device", "linkId": "l_1", "features": "mail",
                    "verificationUrl": "https://login.example.com/device", "userCode": "ABCD-EFGH"}
        elif path == "/cli/mail/link/oauth/status":
            body = {"ok": True, "state": "pending"}
        else:
            body = {"ok": False, "error": {"code": "not_found", "hint": "no route " + path}}
        data = json.dumps(body).encode()
        self.send_response(200 if body["ok"] else 404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_POST(self):
        self.answer()
    def do_GET(self):
        self.answer()
httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
FAKE_PID=$!
trap 'kill $FAKE_PID 2>/dev/null || true; wait $FAKE_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT
for _ in $(seq 1 100); do [ -f "$WORKDIR/port" ] && break; sleep 0.05; done
[ -f "$WORKDIR/port" ] || { echo "FAIL: fake daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"

# The clock: time() stands still and sleep() jumps 1000 seconds, past the
# 900-second wait, so the loop polls once and gives up.
mkdir -p "$WORKDIR/clock"
cat >"$WORKDIR/clock/sitecustomize.py" <<'PY'
import time
_now = [time.time()]
def _time():
    return _now[0]
def _sleep(seconds):
    _now[0] += 1000
time.time = _time
time.sleep = _sleep
PY

K2=(env K2_PORT="$PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR"
    PYTHONPATH="$WORKDIR/clock")
polls() { grep -c "^/cli/mail/link/oauth/status$" "$WORKDIR/requests.log" 2>/dev/null || true; }
run() { rm -f "$WORKDIR/requests.log"; set +e; out="$("${K2[@]}" "$K2_CLI" "$@" 2>&1)"; rc=$?; set -e; }

echo "== a relink that runs out of time says to relink again =="
run mail link relink me@example.com
expect_exit "relink timeout" 2 "$rc"
[ "$(polls)" = "1" ] && ok "relink polled once, then gave up" || bad "relink polled $(polls) times"
expect_in "relink: the timeout code" "$out" '"code":"timeout"'
expect_in "relink: run relink again" "$out" "the link did not complete in time — run 'k2 mail link relink me@example.com' again"
expect_not_in "relink: not the add verb" "$out" "k2 mail link add"

echo "== an upgrade that runs out of time says to upgrade again =="
run mail link upgrade me@example.com --features calendar-read
expect_exit "upgrade timeout" 2 "$rc"
[ "$(polls)" = "1" ] && ok "upgrade polled once, then gave up" || bad "upgrade polled $(polls) times"
expect_in "upgrade: run upgrade again" "$out" "the link did not complete in time — run 'k2 mail link upgrade me@example.com --features calendar-read' again"
expect_not_in "upgrade: not the add verb" "$out" "k2 mail link add"

echo "== a new link that runs out of time says to add it again =="
run mail link add me@example.com --provider microsoft --workspace ws1
expect_exit "add timeout" 2 "$rc"
[ "$(polls)" = "1" ] && ok "add polled once, then gave up" || bad "add polled $(polls) times"
expect_in "add: run add again" "$out" "the link did not complete in time — run 'k2 mail link add --provider microsoft' again"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
