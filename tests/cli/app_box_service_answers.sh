#!/usr/bin/env bash
# What `k2 app box-service request` and `k2 app egress add` print for each
# answer the daemon gives (0.46.8). A stub daemon answers by App id:
#   newly    applied now, under the agent's own switch (nothing was allowed
#            before: this printed an empty "already allowed: ")
#   already  already allowed from before
#   failed   the agent's own apply failed (this printed "already allowed: "
#            and exited 0)
#   held     waits for the owner (exit 2)
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, no real
# daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; diff <(printf '%s\n' "$3") <(printf '%s\n' "$2") >&2 || true; fi; }

SANDBOX="$(mktemp -d /tmp/k2bsa-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

REQ = {"id": "w_1", "summary": "let App held reach sew-api (127.0.0.1:8470) on this box"}
SVC = [{"name": "sew-api", "hostPort": 8470, "appPort": 8470, "proto": "tcp"}]
ANSWERS = {
    "newly": {"ok": True, "status": "allowed", "request": REQ, "services": SVC, "alreadyAllowed": [],
              "result": {"note": "approved; it opens when the App starts"}, "hint": "opened under \"…and manage them end to end\""},
    "already": {"ok": True, "status": "allowed", "alreadyAllowed": ["sew-api"], "hint": "already approved for this App"},
    "failed": {"ok": False, "status": "failed", "request": REQ, "services": SVC, "alreadyAllowed": [],
               "result": {"code": "engine_unavailable", "hint": "the App's door didn't reload"}},
    "held": {"ok": True, "status": "held", "request": REQ, "alreadyAllowed": [], "hint": "held for the owner"},
    "hosts": {"ok": True, "status": "allowed", "request": REQ, "added": ["api.example.com"], "alreadyAllowed": ["old.example.com"],
              "result": {"note": "it applies to the App's next connection"}, "hint": "added"},
}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        app = json.loads(self.rfile.read(n) or b"{}").get("app", "")
        body = json.dumps(ANSWERS.get(app, {"error": {"code": "not_found", "hint": "no " + app}})).encode()
        self.send_response(200 if app in ANSWERS else 404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

# run <args…>: the agent's call; stdout+stderr in $out, the exit code in $rc.
run() {
    set +e
    out="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-bsa "$K2" "$@" </dev/null 2>&1)"
    rc=$?
    set -e
}

run app box-service request newly sew-api
expect_eq "applied now: exit 0" "$rc" "0"
expect_eq "applied now: names what it allowed" "$out" "allowed: sew-api
  approved; it opens when the App starts"

run app box-service request already sew-api
expect_eq "already allowed: exit 0" "$rc" "0"
expect_eq "already allowed: names it" "$out" "already allowed: sew-api"

run app box-service request failed sew-api
expect_eq "a failed apply is not a success: exit 3" "$rc" "3"
expect_eq "a failed apply says why" "$out" "not applied: the App's door didn't reload"

run app box-service request held sew-api
expect_eq "held: exit 2" "$rc" "2"
case "$out" in
    "held for the owner: w_1"*) ok "held: says so" ;;
    *) bad "held: $out" ;;
esac

run app egress add hosts api.example.com
expect_eq "egress applied now: exit 0" "$rc" "0"
expect_eq "egress applied now: names both" "$out" "allowed: api.example.com
already allowed: old.example.com
  it applies to the App's next connection"

echo
echo "app_box_service_answers: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
