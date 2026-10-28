#!/usr/bin/env bash
# k2 compute manage — "Allow managing compute nodes" (compute_manage, 0194)
# CLI shapes against a tiny stub daemon on loopback (python3 http.server).
# No real daemon, no vault, no keychain, no ~/.k2: HOME is a throwaway dir.
#
# Covers: `k2 compute manage on|off --workspace WS` posts agent-access/set
# {toggle: compute_manage, value, workspace} over TCP and renders; a bad
# action is a usage error (exit 2) with no request; a daemon refusal
# (owner_only) exits 3; inside a K2 session the managed verbs (node add,
# grant) send the session's passport and render the answer; the usage text
# and `k2 study compute` document the switch and what stays the owner's.

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

SANDBOX="$(mktemp -d /tmp/k2cm-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]

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
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        token = (parse_qs(u.query).get("token") or [""])[0]
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": body, "token": token}) + "\n")
        if u.path == "/cli/agent-access/set":
            if body.get("workspace") == "locked":
                return self.reply(403, {"error": {"code": "owner_only", "hint": "only the server's owner may turn this on or off"}})
            return self.reply(200, {"ok": True, "workspace": body.get("workspace"), "toggle": body.get("toggle"), "agentsCanManageCompute": body.get("value")})
        if u.path == "/cli/compute/nodes/enroll-code":
            name = body.get("name")
            return self.reply(200, {"name": name, "code": "ABCDE-FGHJK", "controllerFp": "0123", "enroll": "ABCDE-FGHJK.0123",
                                    "expiresAt": 1, "controllerUrl": "https://ctl.example.test",
                                    "install": {"linux": "sudo install-linux --name " + name, "macos": "sudo install-macos --name " + name, "enrollOnly": "k2-node enroll"},
                                    "confirm": "k2 compute node confirm %s <code shown on the node>" % name})
        if u.path == "/cli/compute/grants/set":
            return self.reply(200, {"ok": True, "node": body.get("node"), "workspace": body.get("workspace"),
                                    "grant": {"maxJobSecs": 7200, "maxParallel": 1, "maxQueued": 10, "maxDiskGb": 60},
                                    "agentsCanUseCompute": False, "note": "agents also need Allow compute nodes"})
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

last() { tail -1 "$LOG" | python3 -c "import json,sys; v=json.load(sys.stdin); print(json.dumps(v[sys.argv[1]], sort_keys=True) if not isinstance(v[sys.argv[1]], str) else v[sys.argv[1]])" "$1"; }

echo "== k2 compute manage on|off (owner) =="
out="$("$K2" compute manage on --workspace ops </dev/null)"
expect_has "on renders" "$out" "Allow managing compute nodes: on for ops"
expect_has "on says what stays the owner's" "$out" "preview and node remove stay yours"
expect_eq "on path" "$(last path)" "/cli/agent-access/set"
expect_eq "on body" "$(last body)" '{"toggle": "compute_manage", "value": true, "workspace": "ops"}'
expect_eq "sent with the owner token" "$(last token)" "owner-test-token"
out="$("$K2" compute manage off --workspace ops </dev/null)"
expect_eq "off renders" "$out" "Allow managing compute nodes: off for ops"
expect_eq "off body" "$(last body)" '{"toggle": "compute_manage", "value": false, "workspace": "ops"}'

echo "== usage and refusals =="
n_before="$(wc -l < "$LOG")"
set +e
"$K2" compute manage maybe --workspace ops </dev/null >/dev/null 2>&1; rc=$?
set -e
expect_eq "bad action exits 2" "$rc" "2"
expect_eq "no request for a usage error" "$(wc -l < "$LOG")" "$n_before"
set +e
err="$("$K2" compute manage on --workspace locked 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "daemon refusal exits 3" "$rc" "3"
expect_has "refusal code" "$err" "owner_only"

echo "== managed verbs from inside a K2 session send the passport =="
out="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-ops "$K2" compute node add mini-9 </dev/null)"
expect_has "node add renders the enroll code" "$out" "Enroll code for mini-9"
expect_eq "node add path" "$(last path)" "/cli/compute/nodes/enroll-code"
expect_eq "node add body" "$(last body)" '{"name": "mini-9"}'
expect_eq "node add carries the passport" "$(last token)" "passport-ops"
expect_has "node add says to keep the code off argv" "$out" "ps shows them to every user"
expect_has "node add points scripts at --json" "$out" "--json prints {code, expiresAt, name, controller}"
# --json (2026-10-28): one object a script can hand to k2 vault set as is.
out="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-ops "$K2" compute node add mini-9 --json </dev/null)"
expect_eq "node add --json shape" "$out" '{"code": "ABCDE-FGHJK.0123", "expiresAt": 1, "name": "mini-9", "controller": "https://ctl.example.test"}'
expect_eq "node add --json body is the same" "$(last body)" '{"name": "mini-9"}'
out="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-ops "$K2" compute grant mini-9 --workspace web </dev/null)"
expect_has "grant renders" "$out" "granted mini-9 to web"
expect_eq "grant body names the other workspace" "$(last body)" '{"node": "mini-9", "workspace": "web"}'
expect_eq "grant carries the passport" "$(last token)" "passport-ops"

echo "== docs =="
usage="$("$K2" compute --help 2>&1 || true)"
expect_has "usage lists k2 compute manage" "$usage" "k2 compute manage on|off --workspace WS"
expect_has "usage names the switch" "$usage" "Allow managing compute nodes"
study="$("$K2" study compute)"
expect_has "study has the section" "$study" "MANAGING THE MACHINES"
expect_has "study names the switch" "$study" "Allow managing compute nodes"
expect_has "study keeps remove with the owner" "$study" "k2 compute node remove"
expect_has "study: never over the tunnel" "$study" "never over LAN, the tunnel or an App"

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
