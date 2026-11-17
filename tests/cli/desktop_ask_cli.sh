#!/usr/bin/env bash
# k2 desktop ask, and the Mac permission steps in status and enable
# (Rosson 2026-11-15: Remote desktop on a Mac in one step). A stub daemon
# answers with the route's shapes and logs each request (path, body).
# Hermetic: HOME is a temp dir; no real daemon, no ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

WORK="$(mktemp -d -t k2-desk-ask-XXXXXX)"
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
PERM = {"step": "waiting_screen", "screen": "waiting", "accessibility": "waiting", "opened": "screen", "checking": True,
        "line": "Allow K2 Screen in the window that just opened."}
STATUS = {"desktop": "console", "kind": "server", "name": "This server", "machine": "office-mini", "enabled": True,
          "backend": "macos", "lane": "main", "state": "needs_permission", "message": "Someone at this Mac has to allow K2 Screen once.",
          "reason": None, "you": {"role": "owner-token", "mayView": False, "mayControl": False, "mayManage": True, "sameMachine": True, "mayStart": False},
          "macPermission": PERM}
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
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write("%s %s %s\n" % (self.command, u.path, body))
        return u.path
    def do_GET(self):
        self._log("")
        return self._reply(200, STATUS)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        path = self._log(body)
        if path == "/cli/desktop/ask":
            if os.path.exists(os.path.join(work, "off")):
                return self._reply(409, {"ok": False, "error": {"code": "desktop_off", "hint": "Remote desktop is off on this computer, so there is nothing to ask."}})
            b = json.loads(body or "{}")
            asked = "check" if b.get("check") else ("open_" + b["open"]) if b.get("open") else "prompt"
            return self._reply(200, {"ok": True, "desktop": "console", "asked": asked, "state": "needs_permission", "message": "", "macPermission": PERM})
        if path == "/cli/desktop/enable":
            return self._reply(200, {"ok": True, "desktop": "console", "enabled": True, "changed": True, "state": "needs_permission",
                                     "message": "", "card": "Anyone you allow can see and do anything you can.", "macPermission": dict(PERM, step="asking", opened=None, line="Starting K2 Screen to ask macOS…")})
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
# has <text> <needle>: the text contains the needle (fixed string).
has() { printf '%s' "$1" | grep -qF -- "$2"; }

out="$(k2 ask)"
if [ "$(last_req)" = "POST /cli/desktop/ask {}" ]; then pass "ask posts {}"; else bad "ask request: $(last_req)"; fi
if has "$out" "Asked K2 Screen to show" && has "$out" "Allow K2 Screen in the window that just opened."; then pass "ask says what happened and the line"; else bad "ask output: $out"; fi

k2 ask --open screen >/dev/null
if [ "$(last_req)" = 'POST /cli/desktop/ask {"open":"screen"}' ]; then pass "--open screen"; else bad "--open request: $(last_req)"; fi
k2 ask --open=accessibility >/dev/null
if [ "$(last_req)" = 'POST /cli/desktop/ask {"open":"accessibility"}' ]; then pass "--open=accessibility"; else bad "--open= request: $(last_req)"; fi
out="$(k2 ask --check)"
if [ "$(last_req)" = 'POST /cli/desktop/ask {"check":true}' ]; then pass "--check"; else bad "--check request: $(last_req)"; fi
if has "$out" "Checking again."; then pass "--check output"; else bad "--check output: $out"; fi

before="$(wc -l < "$WORK/requests.log")"
if k2 ask --open camera >/dev/null 2>&1; then bad "--open camera was accepted"; else pass "--open camera is a usage error"; fi
if k2 ask --open screen --check >/dev/null 2>&1; then bad "--open with --check was accepted"; else pass "--open with --check is a usage error"; fi
if k2 status --check >/dev/null 2>&1; then bad "--check outside ask was accepted"; else pass "--check goes with ask only"; fi
if [ "$(wc -l < "$WORK/requests.log")" = "$before" ]; then pass "usage errors send nothing"; else bad "a usage error sent a request"; fi

out="$(k2 status)"
if has "$out" "permission     : Screen Recording waiting, Accessibility waiting"; then pass "status prints the steps"; else bad "status output: $out"; fi
if has "$out" "K2 notices by itself when they do."; then pass "status says K2 notices"; else bad "status notice: $out"; fi

out="$(k2 enable)"
if has "$out" "Starting K2 Screen to ask macOS"; then pass "enable prints the first step"; else bad "enable output: $out"; fi

touch "$WORK/off"
if err="$(k2 ask 2>&1 >/dev/null)"; then
    bad "ask while off exited 0"
elif has "$err" "desktop_off" && has "$err" "nothing to ask"; then
    pass "ask while off says so"
else
    bad "off error: $err"
fi

if [ "$fail" -ne 0 ]; then
    echo "desktop_ask_cli: FAILED" >&2
    exit 1
fi
echo "desktop_ask_cli: all passed"
