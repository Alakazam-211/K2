#!/usr/bin/env bash
# 0.45.6 — `k2 msg agent::host --inbox-wake --title … --body …` (no file)
# asks the PEER to wake: the wake rides the compose call (`wake=true`)
# and the CLI never knocks /cli/workspace/msg on this box with the
# federated token (that always answered workspace_not_found — appa,
# 2026-10-11..13). A local target keeps the local knock.
#
# A python stub stands in for the local daemon and records every request.
# Sandbox HOME, fake port/token, never the production daemon. Loud: every
# check must pass; nothing is skipped.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

WORK="$(mktemp -d -t k2-fed-compose-wake-XXXXXX)"
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
mkdir -p "$HOME/.k2"

cat >"$WORK/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse
ROOT = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _send(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        self._send(200, {"ok": True})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        form = parse_qs(self.rfile.read(n).decode())
        path = urlparse(self.path).path
        with open(ROOT + "/requests.log", "a") as f:
            f.write(json.dumps({"path": path, "form": {k: v[0] for k, v in form.items()}}) + "\n")
        if path == "/cli/inbox/compose":
            target = form.get("workspace", [""])[0]
            out = {"id": "pkg-1", "title": form.get("title", [""])[0], "ok": True}
            if "::" in target:
                # What the local daemon relays back from the peer.
                out.update({"status": "sent", "mode": "inbox",
                            "coverPath": "/home/k2/ai/Chris/.k2/inbox/pkg-1.md"})
                if form.get("wake", ["false"])[0] == "true":
                    out["wake"] = {"success": True, "reason": None,
                                   "targetSessionId": "s-1", "attempts": 1}
            self._send(200, out)
        elif path == "/cli/workspace/msg":
            target = form.get("workspace", [""])[0]
            if "::" in target:
                self._send(200, {"success": False, "reason": "workspace_not_found",
                                 "hint": "stub: federated token on the local route",
                                 "attempts": 1})
            else:
                self._send(200, {"success": True, "target_session_id": "local-1",
                                 "woke": False, "attempts": 1})
        else:
            self._send(404, {"error": "not found"})
srv = HTTPServer(("127.0.0.1", 0), H)
open(ROOT + "/stub.port", "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 "$WORK/stub.py" "$WORK" >"$WORK/stub.out" 2>"$WORK/stub.err" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/stub.port" ] && break; sleep 0.1; done
[ -s "$WORK/stub.port" ] || fail "stub daemon never started: $(head -c 300 "$WORK/stub.err")"
STUB_PORT="$(cat "$WORK/stub.port")"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN \
        -u K2_PROJECT_PATH -u K2SO_PROJECT_PATH -u K2_CELL \
        HOME="$HOME" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="fake-token" \
        "$K2_CLI" "$@"
}

# Requests the stub saw, as "path wake mode workspace" lines.
seen() {
    python3 - "$WORK/requests.log" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    f = r["form"]
    print(r["path"], f.get("wake", "-"), f.get("mode", "-"), f.get("workspace", "-"))
PY
}

echo "== federated --inbox-wake --title/--body =="
: >"$WORK/requests.log"
set +e
run_k2 msg chris::claimchaser.k2.dev --inbox-wake --title "Numbers" --body "a to z" \
    >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
[ "$rc" = "0" ] || fail "exit $rc; out=$(cat "$WORK/out") err=$(cat "$WORK/err")"
ok "exit 0"
log="$(seen)"
printf '%s\n' "$log" | grep -Fxq "/cli/inbox/compose true wake chris::claimchaser.k2.dev" \
    || fail "compose must carry wake=true&mode=wake for the peer; saw: $log"
ok "compose carries wake=true, mode=wake"
if printf '%s\n' "$log" | grep -q "^/cli/workspace/msg "; then
    fail "must not knock /cli/workspace/msg on this box with a federated token; saw: $log"
fi
ok "no local /cli/workspace/msg knock"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["wake"]["success"] is True, d' \
    "$WORK/out" || fail "stdout must carry the peer wake.success=true: $(cat "$WORK/out")"
ok "wake.success true from the peer"
if grep -q "wake notify failed" "$WORK/err"; then
    fail "no wake failure note expected: $(cat "$WORK/err")"
fi
ok "no 'wake notify failed' note"

echo "== federated --inbox-silent --title =="
: >"$WORK/requests.log"
run_k2 msg chris::claimchaser.k2.dev --inbox-silent --title "Quiet" >"$WORK/out" 2>"$WORK/err" \
    || fail "silent exit non-zero: $(cat "$WORK/err")"
log="$(seen)"
printf '%s\n' "$log" | grep -Fxq "/cli/inbox/compose false silent chris::claimchaser.k2.dev" \
    || fail "silent compose must carry wake=false; saw: $log"
if printf '%s\n' "$log" | grep -q "^/cli/workspace/msg "; then
    fail "silent must not knock anything; saw: $log"
fi
ok "silent: wake=false, no knock"

echo "== local --inbox-wake --title (unchanged: compose then local knock) =="
: >"$WORK/requests.log"
run_k2 msg localws --inbox-wake --title "Local" >"$WORK/out" 2>"$WORK/err" \
    || fail "local exit non-zero: $(cat "$WORK/err")"
log="$(seen)"
printf '%s\n' "$log" | grep -q "^/cli/inbox/compose - - localws$" \
    || fail "local compose unchanged; saw: $log"
printf '%s\n' "$log" | grep -q "^/cli/workspace/msg true - localws$" \
    || fail "local target still knocks /cli/workspace/msg; saw: $log"
ok "local target keeps compose + local knock"

echo "OK: msg_inbox_federated_compose_wake ($pass checks)"
