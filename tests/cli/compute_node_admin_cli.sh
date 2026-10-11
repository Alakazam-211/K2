#!/usr/bin/env bash
# k2 compute node admin — CLI shapes (prd-k2-node-admin-v1 §13.2) against a
# tiny stub daemon on loopback (python3 http.server). No real daemon, no
# vault, no keychain, no ~/.k2: HOME is a throwaway dir.
#
# Covers: propose sends {node, kind, verb, args, reason} with typed values
# and exits 2 with the request id (held); the PRD shape `node admin <node>
# <verb>` is propose; `script --file` carries the file text; policy set
# narrows (exit 0) or is held (exit 2); approve inside a K2 session is
# refused (exit 3) before any network call; deny / list / health / operate
# render; a daemon refusal exits 3.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2na-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]
REQ = {"id": "car_0123456789abcdef", "state": "pending", "summary": "reboot mini-1 in 60s", "node": "mini-1", "reason": "stuck"}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def record(self, body):
        with open(LOG, "a") as f:
            f.write(json.dumps({"method": self.command, "path": urlparse(self.path).path, "body": body}) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        self.record(None)
        if u.path == "/cli/compute/admin/requests":
            return self.reply(200, {"requests": [REQ], "canApprove": False})
        if u.path == "/cli/compute/nodes/health":
            return self.reply(200, {"node": "mini-1", "online": True, "nodeVersion": "0.45.5", "remoteAdmin": "full", "pendingApprovals": 1,
                                    "facts": {"uptime_secs": 90000, "disk_encryption": "on", "sleep": {"autorestart": 1}, "disks": [{"name": "root", "path": "/", "total_bytes": 500 << 30, "free_bytes": 9 << 30}]},
                                    "update": {"state": "healthy", "version": "0.45.5"}, "alertAfterSecs": 600, "alertsTo": "it"})
        if u.path == "/cli/compute/nodes/policy":
            return self.reply(200, {"node": "mini-1", "online": True, "policy": {"mem_cap_gb": 8}, "remoteAdmin": "full"})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        self.record(body)
        if u.path == "/cli/compute/admin/propose":
            if body.get("node") == "ghost":
                return self.reply(404, {"error": {"code": "node_not_found", "hint": "no compute node 'ghost'"}})
            return self.reply(200, {"held": True, "request": REQ})
        if u.path == "/cli/compute/nodes/policy/set":
            if body["set"].get("mem_cap_gb", 0) < 8:
                return self.reply(200, {"ok": True, "node": "mini-1", "changed": True, "class": "operate", "note": "narrowing sent"})
            return self.reply(200, {"held": True, "request": dict(REQ, summary="change job limits on mini-1: mem_cap_gb=10")})
        if u.path == "/cli/compute/admin/deny":
            return self.reply(200, {"ok": True, "request": dict(REQ, state="denied")})
        if u.path == "/cli/agent-access/set":
            return self.reply(200, {"ok": True, "workspace": body.get("workspace"), "toggle": body.get("toggle"), "agentsCanOperateCompute": body.get("value")})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
PORT="$(cat "$SANDBOX/port")"

# Owner-style environment: port + token from a throwaway ~/.k2, no session.
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

last_body() { tail -1 "$LOG" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["body"], sort_keys=True))'; }
last_path() { tail -1 "$LOG" | python3 -c 'import json,sys; print(json.load(sys.stdin)["path"])'; }

echo "== propose (held → exit 2) =="
set +e
err="$("$K2" compute node admin propose mini-1 power.reboot in_secs=60 --reason stuck 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "propose exits 2 (held)" "$rc" "2"
expect_has "propose prints the request id" "$err" "car_0123456789abcdef"
expect_eq "propose path" "$(last_path)" "/cli/compute/admin/propose"
expect_eq "propose body (typed args)" "$(last_body)" '{"args": {"in_secs": 60}, "kind": "command", "node": "mini-1", "reason": "stuck", "verb": "power.reboot"}'

echo "== PRD shape + script --file =="
printf '#!/bin/sh\ndf -h\n' > "$SANDBOX/fix.sh"
set +e
"$K2" compute node admin mini-1 script --file "$SANDBOX/fix.sh" --interpreter /bin/bash --timeout 5m --reason "disk" </dev/null >/dev/null 2>&1; rc=$?
set -e
expect_eq "script propose exits 2" "$rc" "2"
expect_eq "script body carries the file" "$(last_body)" '{"args": {"interpreter": "/bin/bash", "text": "#!/bin/sh\ndf -h\n", "timeout_secs": 300}, "kind": "command", "node": "mini-1", "reason": "disk", "verb": "script"}'
set +e
"$K2" compute node admin propose mini-1 pkg.install names=htop,jq </dev/null >/dev/null 2>&1
set -e
expect_eq "names=a,b becomes a list" "$(last_body)" '{"args": {"names": ["htop", "jq"]}, "kind": "command", "node": "mini-1", "verb": "pkg.install"}'

echo "== policy =="
out="$("$K2" compute node policy mini-1 set mem_cap_gb=6 </dev/null)"
expect_has "narrowing applies" "$out" "narrowing sent"
expect_eq "policy body" "$(last_body)" '{"node": "mini-1", "set": {"mem_cap_gb": 6}}'
set +e
"$K2" compute node policy mini-1 set mem_cap_gb=10 --reason bigger </dev/null >/dev/null 2>&1; rc=$?
set -e
expect_eq "widening is held (exit 2)" "$rc" "2"
out="$("$K2" compute node policy mini-1 show)"
expect_has "policy show" "$out" "mem_cap_gb"

echo "== list / health / deny / operate =="
out="$("$K2" compute node admin list)"
expect_has "list shows the request" "$out" "reboot mini-1 in 60s"
out="$("$K2" compute node health mini-1)"
expect_has "health shows FileVault" "$out" "FileVault     on"
expect_has "health shows autorestart" "$out" "autorestart   on"
expect_has "health shows alerts" "$out" "offline after 10m, to it"
out="$("$K2" compute node admin deny car_0123456789abcdef --why "not now")"
expect_has "deny" "$out" "denied car_0123456789abcdef"
expect_eq "deny body" "$(last_body)" '{"request": "car_0123456789abcdef", "why": "not now"}'
out="$("$K2" compute operate on --workspace it)"
expect_has "operate switch" "$out" "operate compute nodes: on for it"
expect_eq "operate body" "$(last_body)" '{"toggle": "compute_operate", "value": true, "workspace": "it"}'

echo "== refusals =="
set +e
err="$("$K2" compute node admin propose ghost power.reboot 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "daemon refusal exits 3" "$rc" "3"
expect_has "refusal code" "$err" "node_not_found"
set +e
"$K2" compute node admin propose mini-1 </dev/null >/dev/null 2>&1; rc=$?
set -e
expect_eq "usage error exits 2" "$rc" "2"

echo "== approve inside a K2 session is refused before any network call =="
n_before="$(wc -l < "$LOG")"
set +e
err="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-test "$K2" compute node admin approve car_0123456789abcdef 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "approve in session exits 3" "$rc" "3"
expect_has "approve in session says owner_only" "$err" "owner_only"
expect_eq "no request reached the daemon" "$(wc -l < "$LOG")" "$n_before"

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
