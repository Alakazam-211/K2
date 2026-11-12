#!/usr/bin/env bash
# k2 desktop on a Linux server with no screen (prd-linux-virtual-desktop-v1
# L4): start / stop / setup / list, --desktop and --size, and what status
# prints for a virtual desktop. A stub daemon answers with the wire
# contract's shapes and logs each request (path, query, body).
# Hermetic: HOME is a temp dir; no real daemon, no ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

WORK="$(mktemp -d -t k2-vd-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -p "$WORK/home"

python3 -I -c '
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
STATUS = {
    "desktop": "virtual:ws1", "kind": "agent", "name": "scientist", "workspaceId": "ws1",
    "machine": "your-server", "enabled": True, "backend": "linux", "lane": "main",
    "state": "ready", "message": "Ready.", "reason": None,
    "you": {"role": "owner", "mayView": True, "mayControl": True, "mayManage": True, "sameMachine": False, "mayStart": True},
    "virtual": {
        "size": {"width": 1600, "height": 900}, "display": ":100", "memoryBytes": 73400320,
        "present": [{"viewer": "dv_1", "user": "rosson"}],
        "hold": {"by": "k2/scientist", "until": 1731301800},
        "nextSleepAt": None, "nextCloseAt": None, "canSleep": True, "screen": "ready", "waiting": None,
        "label": "Separate, not private",
        "limits": {"desktops": {"max": 4, "used": 2}, "awake": {"max": 2, "used": 1},
                   "memoryBytes": {"max": 2147483648, "each": 2147483648}, "cpuCores": 2},
        "missing": [], "manual": None, "suggestRecycle": False,
    },
}
LIST = {"default": "virtual", "desktops": [
    {"id": "virtual", "kind": "server", "name": "This server", "state": "asleep"},
    {"id": "virtual:ws1", "kind": "agent", "name": "scientist", "workspaceId": "ws1", "state": "ready", "label": "Separate, not private"}]}
class H(BaseHTTPRequestHandler):
    def _reply(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def _log(self, body):
        u = urllib.parse.urlparse(self.path)
        q = "&".join(p for p in u.query.split("&") if p and not p.startswith("token="))
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write("%s %s?%s %s\n" % (self.command, u.path, q, body))
        return u.path
    def do_GET(self):
        path = self._log("")
        if path == "/cli/desktop/list":
            return self._reply(200, LIST)
        polls = os.path.join(work, "setup_polls")
        if os.path.exists(polls):
            n = int(open(polls).read())
            if n > 0:
                open(polls, "w").write(str(n - 1))
                return self._reply(200, dict(STATUS, state="setting_up", message="Installing the desktop packages\u2026"))
            if os.path.exists(os.path.join(work, "setup_fail")):
                return self._reply(200, dict(STATUS, state="not_set_up", message="The desktop packages didn\u0027t install: E: Unable to locate package xvfb. Set up again."))
            return self._reply(200, dict(STATUS, state="closed", message="This server has no screen. K2 can start a desktop on it."))
        return self._reply(200, STATUS)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        path = self._log(body)
        if path == "/cli/desktop/start":
            return self._reply(200, STATUS)
        if path == "/cli/desktop/stop":
            return self._reply(200, {"ok": True, "desktop": "virtual:ws1", "closed": True, "state": "closed", "message": "The desktop was closed by rosson."})
        if path == "/cli/desktop/setup" and os.path.exists(os.path.join(work, "setup_async")):
            open(os.path.join(work, "setup_polls"), "w").write("2")
            return self._reply(200, {"ok": True, "state": "setting_up", "message": "Installing the desktop packages\u2026"})
        if path == "/cli/desktop/setup":
            return self._reply(409, {"ok": False, "error": {"code": "setup_manual", "hint": "Run this, then try again: sudo apt-get update && sudo apt-get install --no-install-recommends xvfb icewm fonts-dejavu-core"}, "manual": "sudo apt-get update"})
        return self._reply(404, {"ok": False, "error": {"code": "not_found", "hint": path}})
    def log_message(self, *a):
        pass
s = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(work, "port.tmp"), "w").write(str(s.server_port))
os.rename(os.path.join(work, "port.tmp"), os.path.join(work, "port"))
s.serve_forever()
' "$WORK" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -f "$WORK/port" ] && break; sleep 0.05; done
[ -f "$WORK/port" ] || { echo "FAIL: the stub daemon didn't start" >&2; exit 1; }

k2() {
    env -i PATH="$PATH" HOME="$WORK/home" K2_PORT="$(cat "$WORK/port")" K2_HOOK_TOKEN=test-token "$K2_CLI" desktop "$@"
}

fail=0
pass() { echo "  PASS: $1"; }
bad() { echo "  FAIL: $1" >&2; fail=1; }
last_req() { tail -1 "$WORK/requests.log"; }

out="$(k2 status --desktop virtual:ws1)"
# Percent-escapes differ in case between encoders: compare lower case.
case "$(last_req | tr 'A-Z' 'a-z')" in "get /cli/desktop/status?desktop=virtual%3aws1 ") pass "status sends ?desktop=" ;; *) bad "status request: $(last_req)" ;; esac
for want in "desktop        : virtual:ws1 (scientist, an agent)" "size           : 1600x900" "viewing now    : rosson" \
    "agent hold     : k2/scientist until" "limits         : 2/4 desktops, 1/2 awake, 2048 MB for all (2048 MB each), 2 cores" \
    "note           : Separate, not private" "you may start and close it"; do
    case "$out" in *"$want"*) pass "status: $want" ;; *) bad "status lacks '$want': $out" ;; esac
done

k2 start --desktop virtual:ws1 --size 1600x900 >/dev/null
case "$(last_req)" in *"POST /cli/desktop/start?"*'"width": 1600'*'"height": 900'*'"desktop": "virtual:ws1"'*) pass "start sends the size and the desktop" ;; *) bad "start request: $(last_req)" ;; esac

set +e
k2 start --size big >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "a bad --size is a usage error" || bad "--size big exit $rc"
set +e
k2 status --size 1600x900 >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "--size only goes with start" || bad "status --size exit $rc"

out="$(k2 list)"
case "$out" in *"virtual:ws1"*"scientist"*"Separate, not private"*) pass "list" ;; *) bad "list: $out" ;; esac

out="$(k2 stop --desktop virtual:ws1)"
case "$out" in *"Closed. The desktop was closed by rosson."*) pass "stop" ;; *) bad "stop: $out" ;; esac
case "$(last_req)" in *'"desktop": "virtual:ws1"'*) pass "stop sends the desktop" ;; *) bad "stop request: $(last_req)" ;; esac

set +e
err="$(k2 setup 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" != "0" ] && pass "setup_manual exits non-zero" || bad "setup exit 0"
case "$err" in *"setup_manual"*"sudo apt-get update"*) pass "setup prints the command" ;; *) bad "setup: $err" ;; esac

# Setup answers setting_up at once (contract note 11-11); the CLI waits on
# status until closed (installed) or not_set_up (why), printing progress.
touch "$WORK/setup_async"
set +e
out="$(k2 setup 2>"$WORK/err")"; rc=$?
set -e
[ "$rc" = "0" ] && pass "setup waits and exits 0 when installed" || bad "setup exit $rc: $(cat "$WORK/err")"
case "$out" in *"Installed. k2 desktop start starts it."*) pass "setup says installed" ;; *) bad "setup: $out" ;; esac
case "$(cat "$WORK/err")" in *"Installing the desktop packages"*) pass "setup prints progress" ;; *) bad "setup progress: $(cat "$WORK/err")" ;; esac
case "$(grep -c 'GET /cli/desktop/status?desktop=virtual' "$WORK/requests.log")" in 0) bad "setup never polled status" ;; *) pass "setup polls status for virtual" ;; esac

touch "$WORK/setup_fail"
set +e
err="$(k2 setup 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" = "1" ] && pass "a failed install exits 1" || bad "failed setup exit $rc"
case "$err" in *"setup_failed"*"Unable to locate package xvfb"*) pass "a failed install says why" ;; *) bad "failed setup: $err" ;; esac
rm -f "$WORK/setup_fail"

out="$(k2 setup --no-wait)"
case "$out" in *"k2 desktop status says setting_up"*) pass "--no-wait returns at once" ;; *) bad "--no-wait: $out" ;; esac
set +e
k2 status --no-wait >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "--no-wait only goes with setup" || bad "status --no-wait exit $rc"

[ "$fail" = "0" ] || exit 1
echo "desktop_virtual_cli: ok"
