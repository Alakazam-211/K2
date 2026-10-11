#!/usr/bin/env bash
# Published services from before 0.45.4 keep the daemon's inherited
# environment in 0.45.x, with a notice that it ends in 0.46.0 (Rosson
# 2026-10-10). This checks the CLI shows the daemon's `envDeprecation`
# in `k2 publish ps` and on a single service, and that `k2 publish env`
# (k2 vault service) says so for a legacy service — against a stub daemon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN || true

WORK="$(mktemp -d -t k2-publish-envdep-XXXXXX)"
STUB_PID=""
trap '[ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null; rm -rf "$WORK"' EXIT
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
NOTICE="This service still inherits the K2 daemon's whole environment because it existed before 0.45.4. That ends in K2 0.46.0: switch with k2 publish env shop clean"
cat >"$WORK/stub.py" <<'PY'
import http.server, json, sys
NOTICE = sys.argv[2]
def svc(name, dep):
    return {"id": name + "-id", "projectId": "p", "name": name, "cmd": "node s.js", "cwd": ".",
            "port": 3000, "expose": "local", "desired": "running", "status": "running",
            "pid": 1, "url": None, "target": "127.0.0.1:3000", "error": None,
            "lastExitCode": None, "kind": "cmd", "skinRoot": "", "frameAncestors": "",
            "framePolicy": None, "frameWarning": None, "envDeprecation": dep}
class H(http.server.BaseHTTPRequestHandler):
    def reply(self, out):
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        if "/cli/vault/service" in self.path:
            self.reply({"service": "shop", "id": "shop-id", "mode": "legacy", "inheritedNames": ["AWS_SECRET"], "secrets": []})
        else:
            self.reply({"services": [svc("shop", NOTICE), svc("fresh", None)]})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        self.reply({"service": svc("shop", NOTICE)})
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
python3 "$WORK/stub.py" "$PORT" "$NOTICE" &
STUB_PID=$!
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break; sleep 0.05; done
k2s() { env HOME="$WORK" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=stub-token K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"; }

out="$(k2s publish ps 2>&1)" || fail "publish ps: $out"
printf '%s' "$out" | grep -Fq "warning: shop: deprecated: $NOTICE" || fail "ps lacks the shop notice: $out"
if printf '%s' "$out" | grep -q "warning: fresh"; then fail "a clean service must carry no notice: $out"; fi
ok "k2 publish ps: the legacy service carries the 0.46.0 notice, a clean one none"

out="$(k2s publish start shop 2>&1)" || fail "publish start: $out"
printf '%s' "$out" | grep -Fq "  deprecated: $NOTICE" || fail "start lacks the notice: $out"
ok "a single service's status line carries the notice"

out="$(k2s publish env shop 2>&1)" || fail "publish env: $out"
printf '%s' "$out" | grep -q "deprecated: inheriting the daemon environment ends in K2 0.46.0 — switch with: k2 publish env shop clean" \
	|| fail "publish env lacks the notice: $out"
ok "k2 publish env names the 0.46.0 end and the switch"

echo "PASS: publish env deprecation ($pass checks)"
