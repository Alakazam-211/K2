#!/usr/bin/env bash
# k2 app label take prints every note in the server's answer: guests
# copied, kept, skipped because their role is another App's, skipped
# because another workspace's agent added them, roles copied, and the
# closing note. The reader is Python inside a single-quoted shell block,
# so an apostrophe in it would end the quote and break the reader.
# Stub daemon only; a throwaway HOME; never touches the real ~/.k2.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_contains() {
    case "$2" in
        *"$3"*) echo "  PASS: $1"; pass=$((pass + 1)) ;;
        *) echo "  FAIL: $1 (missing $(printf %q "$3")) in: $2" >&2; fail=$((fail + 1)) ;;
    esac
}
assert_not_contains() {
    case "$2" in
        *"$3"*) echo "  FAIL: $1 (found $(printf %q "$3")) in: $2" >&2; fail=$((fail + 1)) ;;
        *) echo "  PASS: $1"; pass=$((pass + 1)) ;;
    esac
}
assert_eq() {
    if [ "$2" = "$3" ]; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (got=$(printf %q "$2") want=$(printf %q "$3"))" >&2; fail=$((fail + 1)); fi
}

SANDBOX="$(mktemp -d -t k2-app-label-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

export HOME="$SANDBOX"
mkdir -p "$HOME/.k2"
echo "owner-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
unset K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOOK_TOKEN K2SO_HOOK_TOKEN K2SO_PORT K5_HOOK_SOCK K5_HOOK_TOKEN || true

# The answer the daemon gives when it moves a --skin site onto an App
# (crates/k2-daemon/src/apps/skin_move.rs), with every list filled.
cat >"$SANDBOX/answer.json" <<'JSON'
{"ok": true, "app": "mvapp", "label": "agents", "frontPort": 47001, "site": true,
 "copied": {"added": ["ann", "bo"], "keptExisting": [{"user": "cy", "enabled": true}],
            "skippedOtherAppRole": ["dee"], "skippedOtherWorkspaceAgent": ["eli", "fay"],
            "rolesCopied": [{"from": "desk", "to": "mvapp-desk"}]},
 "notCopied": ["a login with no password"],
 "note": "The site keeps running without a public label."}
JSON

cat >"$SANDBOX/stub.py" <<'PY'
import os
from http.server import BaseHTTPRequestHandler, HTTPServer

answer = open(os.environ["ANSWER"], "rb").read()

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or "0")
        self.rfile.read(n)
        if self.path.split("?")[0] == "/cli/app/label/take":
            code, raw = 200, answer
        else:
            code, raw = 404, b'{"error":"no such route"}'
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"{}")

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
STUB_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
export K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT"
PORT="$STUB_PORT" ANSWER="$SANDBOX/answer.json" python3 "$SANDBOX/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -s --noproxy '*' --connect-timeout 1 -o /dev/null "http://127.0.0.1:${STUB_PORT}/"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon never answered" >&2; exit 1; }

echo "== k2 app label take prints every note =="
set +e
out="$("$K2_CLI" app label take mvapp agents 2>"$SANDBOX/err")"
rc=$?
set -e
err="$(cat "$SANDBOX/err")"
assert_eq "exit 0" "$rc" "0"
assert_eq "nothing on stderr" "$err" ""
assert_contains "the move" "$out" "agents now serves App mvapp (front port 47001)"
assert_contains "guests copied" "$out" "guests copied onto the App: 2 (ann, bo)"
assert_contains "kept" "$out" "already on the App (kept as they were): cy"
assert_contains "skipped: another App's role" "$out" "not copied (their role is another App’s): dee"
assert_contains "skipped: another workspace's agent" "$out" "not copied (another workspace’s agent added them): eli, fay"
assert_contains "roles copied" "$out" "role desk copied for the App as mvapp-desk"
assert_contains "not copied" "$out" "not copied: a login with no password"
assert_contains "closing note" "$out" "The site keeps running without a public label."
assert_not_contains "no Python error" "$out$err" "Traceback"
assert_not_contains "no syntax error" "$out$err" "SyntaxError"

echo
echo "app_label_take_notes: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
