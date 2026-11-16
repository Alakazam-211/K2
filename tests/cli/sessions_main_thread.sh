#!/usr/bin/env bash
# k2 sessions main set --thread (0.46.14): which Thread the main chat shows
# after another chat (a sidecar, a past chat) becomes the main chat.
#
#   1. No daemon: a bad --thread value is a usage error (exit 2).
#   2. A stub daemon: the POST body carries thread keep_main by default and
#      adopt_sidecar for --thread adopt-sidecar; the answer's other Thread
#      address is printed.

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
assert_contains() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

WORK="$(mktemp -d -t k2-sessions-main-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN || true

echo "== usage (no daemon) =="
mkdir -p "$WORK/home1"
set +e
out="$(env HOME="$WORK/home1" K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2_CLI" sessions main set abcdef12 --thread both 2>&1)"
rc=$?
set -e
assert_eq "bad --thread exits 2" "$rc" "2"
assert_contains "bad --thread names the choices" "$out" "--thread keep-main|adopt-sidecar"

set +e
out="$(env HOME="$WORK/home1" K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2_CLI" sessions --help 2>&1)"
set -e
assert_contains "help lists --thread" "$out" "k2 sessions main set <id> [--thread keep-main|adopt-sidecar] [--json]"

echo "== stub daemon =="
mkdir -p "$WORK/home2/.k2"
python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps({"method": method, "path": parsed.path, "body": body.decode("utf-8", "replace")}) + "\n")
        out = {}
        if parsed.path == "/cli/sessions/main/set":
            b = json.loads(body) if body else {}
            choice = b.get("thread")
            out = {"ok": True, "workspace": "k2", "previous": "11111111-aaaa", "title": "reviewer",
                   "main": {"conversationId": "22222222-bbbb", "cli": "claude", "state": "found"},
                   "restartWith": "k2 agent restart k2",
                   "thread": {"choice": choice, "conversationId": "22222222-bbbb",
                              "other": {"conversationId": "33333333-cccc", "address": "k2/4"}}}
        elif parsed.path == "/cli/sessions/main":
            out = {"ok": True, "workspace": "k2", "main": {"conversationId": "22222222-bbbb", "cli": "claude", "state": "found"}}
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
STUB_PORT="$(cat "$WORK/stub.port")"

run() {
    : > "$WORK/reqs.jsonl"
    env HOME="$WORK/home2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="person-token" \
        K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
set_body_thread() {
    python3 -c '
import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r["path"] == "/cli/sessions/main/set":
        print(json.loads(r["body"]).get("thread"))
        break
else:
    print("NO-SET-REQUEST")
' "$WORK/reqs.jsonl"
}

set +e
out="$(run sessions main set 22222222 2>&1)"; rc=$?
set -e
assert_eq "set exits 0" "$rc" "0"
assert_eq "default sends keep_main" "$(set_body_thread)" "keep_main"
assert_contains "keep: says the main Thread stayed" "$out" "Thread: the main chat kept its Thread."
assert_contains "keep: prints the other Thread's address" "$out" "kept at k2/4 (read it: k2 thread k2/4)"

set +e
out="$(run sessions main set 22222222 --thread adopt-sidecar 2>&1)"; rc=$?
set -e
assert_eq "adopt exits 0" "$rc" "0"
assert_eq "--thread adopt-sidecar sends adopt_sidecar" "$(set_body_thread)" "adopt_sidecar"
assert_contains "adopt: says the main chat shows the other Thread" "$out" "Thread: the main chat now shows this chat"
assert_contains "adopt: prints the old main Thread's address" "$out" "The old main Thread is kept at k2/4"

echo "sessions_main_thread: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
