#!/usr/bin/env bash
# k2 app domain / k2 tunnel status, custom domains Path B (0.46.8, PB-10),
# against a stub daemon: the CNAME and ALIAS lines, K2 Connect's answer in
# plain words, the 20-name meter, a refused agent's closing line asks for the
# broad App switch, and the visitor-IP line. Fake values only.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-path-b-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
MODE_FILE="$WORK/mode"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer

MODE_FILE, LOG_FILE = os.environ["MODE_FILE"], os.environ["LOG_FILE"]
PATH_B = {"available": True, "connect": True, "pro": True, "why": None, "used": 3, "cap": 20,
          "meter": "3 of 20 names through K2's relays", "upgrade": None}
ADVICE = {"path": "b", "pathA": True, "records": "shown", "pathB": PATH_B,
          "message": "Another web server holds port 443 on this server. Through K2 Connect (Pro) your domain points at K2's relays instead."}
def custom(host, first, relay="claimed"):
    return {
        "hostname": host, "app": "shop", "state": "waiting_dns", "words": "not served yet: waiting for the DNS records",
        "primary": False, "path": "connect", "lastError": None,
        "relay": {"state": relay, "error": None, "checkedAt": 1},
        "records": [first, {"type": "TXT", "name": "_k2-domain." + host, "value": "k2-verify=t0"}],
        "next": "Set these at your DNS host (DNS-only, no proxy), then run: k2 app domain check shop " + host + ".",
        "setup": ADVICE,
    }
SUB = custom("shop.example.com", {"type": "CNAME", "name": "shop.example.com", "value": "shop.box1.k2.dev"})
APEX = custom("example.com", {"type": "ALIAS", "name": "example.com", "value": "shop.box1.k2.dev",
                              "note": "an apex can't hold a CNAME: use ALIAS, ANAME or CNAME flattening at your DNS host"}, "verified")

class H(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _answer(self, method, body):
        path = self.path.split("?")[0]
        with open(LOG_FILE, "a") as f:
            f.write("%s %s %s\n" % (method, path, body))
        mode = open(MODE_FILE).read().strip()
        if mode == "agent":
            return self._send(403, {"error": {"code": "apps_manage_off", "hint": "\"Build and run Apps\" is off for this agent: ask your owner to turn it on"},
                                    "permission": {"action": "apps.build", "command": "k2 permission request apps.build --why \"<what you need it for>\""}})
        if path == "/cli/app/domains":
            if mode == "closed":
                return self._send(200, {"ok": True, "app": "shop", "own": {"label": "shop", "host": "shop.box1.k2.dev"}, "custom": [], "maxCustomNames": 100,
                                        "pathB": {"available": False, "why": "needs K2 Connect Pro: upgrade at https://k2.dev/dashboard"}})
            return self._send(200, {"ok": True, "app": "shop", "own": {"label": "shop", "host": "shop.box1.k2.dev"},
                                    "custom": [SUB, APEX], "front443": False, "pathB": PATH_B, "maxCustomNames": 100})
        if path == "/cli/app/domain/add":
            return self._send(200, {"ok": True, "app": "shop", "custom": APEX if "example.com\"" in body and "shop." not in body else SUB, "txt": None})
        if path == "/cli/app/domain/rm":
            return self._send(200, {"ok": True, "app": "shop", "removed": "shop.example.com", "relay": "released at K2 Connect",
                                    "note": "no longer served; restart the App so its origins drop it. released at K2 Connect; the tunnel restarts once to drop it (a few seconds' blip)"})
        if path == "/cli/tunnel/status":
            return self._send(200, {"running": True, "public_url": "https://box1.k2.dev",
                                    "visitorIps": {"state": "not_passed", "words": "Visitor IPs: not passed by the relay yet (per-visitor limits wait for it; box-wide and per-App limits apply)", "relayAsClient": 2}})
        return self._send(404, {"error": "route not found"})

    def do_GET(self):
        self._answer("GET", "")

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._answer("POST", self.rfile.read(n).decode())

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo ok >"$MODE_FILE"
: >"$LOG_FILE"
MODE_FILE="$MODE_FILE" LOG_FILE="$LOG_FILE" PORT="$PORT" python3 "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -sf --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/tunnel/status" >/dev/null 2>&1 && break
    sleep 0.1
done
: >"$LOG_FILE"

k2() { env HOME="$HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token "$K2_CLI" "$@"; }
rc=0; out=""; err=""
run() {
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}

echo "== add: the Path B records =="
run app domain add shop shop.example.com
assert_eq "add: exit 0" "$rc" "0"
assert_contains "add: CNAME to the App's name" "$out" "CNAME shop.example.com"
assert_contains "add: its target" "$out" "shop.box1.k2.dev"
assert_contains "add: TXT" "$out" "_k2-domain.shop.example.com"
assert_contains "add: says why" "$out" "Through K2 Connect (Pro)"
assert_contains "add: K2 Connect's state in words" "$out" "K2 Connect: claimed, waiting for its own check of the TXT"
assert_absent "add: never an A record" "$out" "    A "

echo "== an apex =="
run app domain add shop example.com
assert_contains "apex: ALIAS" "$out" "ALIAS example.com"
assert_contains "apex: how" "$out" "ANAME or CNAME flattening"
assert_contains "apex: state in words" "$out" "K2 Connect: verified: its relays route this name here"

echo "== domains: the meter =="
run app domains shop
assert_eq "domains: exit 0" "$rc" "0"
assert_contains "domains: which path DNS shows" "$out" "DNS shows: Path B, through K2 Connect"
assert_contains "domains: meter" "$out" "3 of 20 names through K2's relays"
assert_contains "domains: both caps" "$out" "100 custom names at most per box"
echo closed >"$MODE_FILE"
run app domains shop
assert_contains "domains, not Pro: says why" "$out" "not open here: needs K2 Connect Pro"
echo ok >"$MODE_FILE"

echo "== rm =="
run app domain rm shop shop.example.com
assert_contains "rm: released first" "$out" "released at K2 Connect"
assert_contains "rm: the blip" "$out" "a few seconds' blip"

echo "== an agent without the App switch =="
echo agent >"$MODE_FILE"
for verb in "domain add shop shop.example.com" "domain check shop shop.example.com" "domain rm shop shop.example.com"; do
    # shellcheck disable=SC2086
    run app $verb
    assert_eq "$verb: exit 3" "$rc" "3"
    assert_contains "$verb: the broad switch" "$err" "Ask with: k2 permission request apps.build"
    assert_absent "$verb: no web-server card" "$err" "It can't be requested"
done
echo ok >"$MODE_FILE"

echo "== tunnel status =="
run tunnel status
assert_contains "status: visitor IPs" "$out" "Visitor IPs: not passed by the relay yet"
assert_contains "status: a relay naming itself" "$out" "a relay named its own address 2 times"

echo "== stale help is gone =="
run app domain --help
assert_absent "help: no 0.46.7 promise" "$out" "Path B in 0.46.7"
assert_contains "help: the two ways" "$out" "TWO WAYS IN"
assert_contains "help: the cap" "$out" "At most 20 names per box"
assert_contains "help: who" "$out" "Build and run Apps"

echo
echo "app_domain_path_b: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
