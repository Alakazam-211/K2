#!/usr/bin/env bash
# `k2 inbox read` when the session's cell socket is not served (0.46.12).
#
# Field report: a session whose $K2_HOOK_SOCK file existed but had no
# listener (ptyd held it, the daemon never got the fd) printed
#   Error: the K2 daemon gave no usable answer: ''
# because `cli_send` ran `out="$(uds…)"; rc=$?` under `set -e`: the failed
# cell-socket call ended the command before the loopback TCP fallback.
#
# Cases (stub daemon + a bound, never-listening socket; no live K2):
#   1. dead cell socket, TCP answers  → the item prints, exit 0
#   2. dead cell socket, TCP empty    → a plain "no answer" error, exit 1
#   3. dead cell socket, no TCP       → the same plain error, exit 1
# Run with: bash tests/cli/inbox_read_cell_socket_fallback.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

[ -x "$K2" ] || { echo "FAIL: $K2 not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    if [ "$2" = "$3" ]; then echo "  PASS: $1"; pass=$((pass + 1));
    else echo "  FAIL: $1 (got=$(printf %q "$2") want=$(printf %q "$3"))" >&2; fail=$((fail + 1)); fi
}
assert_contains() {
    case "$2" in
        *"$3"*) echo "  PASS: $1"; pass=$((pass + 1)) ;;
        *) echo "  FAIL: $1 (missing $(printf %q "$3") in $(printf %q "$2"))" >&2; fail=$((fail + 1)) ;;
    esac
}
assert_absent() {
    case "$2" in
        *"$3"*) echo "  FAIL: $1 (unexpected $(printf %q "$3") in $(printf %q "$2"))" >&2; fail=$((fail + 1)) ;;
        *) echo "  PASS: $1"; pass=$((pass + 1)) ;;
    esac
}

# Short base: a Unix socket path is capped near 104 bytes on macOS.
WORKDIR="$(mktemp -d /tmp/k2ibf-XXXXXX)"
PIDS=()
cleanup() {
    local p
    for p in "${PIDS[@]:-}"; do
        [ -n "$p" ] && kill "$p" 2>/dev/null || true
    done
    rm -rf "$WORKDIR"
}
trap cleanup EXIT
mkdir -p "$WORKDIR/home" "$WORKDIR/project"

# A socket file nobody listens on: connect() is refused, like a cell socket
# ptyd held while the daemon never got the fd.
python3 - "$WORKDIR/c.sock" "$WORKDIR/dead.ready" <<'PY' &
import socket, sys, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(sys.argv[1])
open(sys.argv[2], "w").write("1")
time.sleep(600)
PY
PIDS+=($!)
disown "$!" 2>/dev/null || true

# Stub daemon: /cli/inbox/read answers the item, or an empty 200 for
# id=empty. Records each request's path + query.
python3 - "$WORKDIR" <<'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs
root = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        with open(os.path.join(root, "requests.log"), "a") as f:
            f.write(u.path + " " + json.dumps({k: v[0] for k, v in q.items()}) + "\n")
        if u.path == "/cli/inbox/read" and q.get("id", [""])[0] == "empty":
            body = b""
        elif u.path == "/cli/inbox/read":
            body = json.dumps({"id": q["id"][0], "content": "hello from the loopback port"}).encode()
        else:
            body = json.dumps({"ok": False, "error": {"code": "not_found", "hint": "stub " + u.path}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
httpd = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(root, "stub.port"), "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
PIDS+=($!)
disown "$!" 2>/dev/null || true

for _ in $(seq 1 100); do
    [ -f "$WORKDIR/stub.port" ] && [ -f "$WORKDIR/dead.ready" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/stub.port" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }
[ -S "$WORKDIR/c.sock" ] || { echo "FAIL: dead cell socket never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORKDIR/stub.port")"
CLOSED_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"

# The session's env: a passport and a cell socket (that nobody serves).
run_read() {
    local port="$1" id="$2"
    env -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOST -u K2_CELL -u K2_SECRETS_DIR \
        HOME="$WORKDIR/home" \
        K2_PORT="$port" \
        K2_HOOK_TOKEN="test-session.passport" \
        K2_HOOK_SOCK="$WORKDIR/c.sock" \
        K2SO_HOOK_SOCK="$WORKDIR/c.sock" \
        K2_PROJECT_PATH="$WORKDIR/project" \
        "$K2" inbox read "$id"
}

echo "== 1. dead cell socket: the read falls back to the loopback port =="
set +e
out="$(run_read "$STUB_PORT" "from-a-sidecar" 2>"$WORKDIR/1.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/1.err")"
assert_eq "exit 0" "$rc" "0"
assert_eq "the item's content prints" "$out" "hello from the loopback port"
assert_absent "no empty-answer error" "$err" "no usable answer"
reqs="$(cat "$WORKDIR/requests.log")"
assert_contains "the TCP request named the item" "$reqs" '"id": "from-a-sidecar"'
assert_contains "the TCP request carried the session passport" "$reqs" '"token": "test-session.passport"'

echo "== 2. dead cell socket, TCP answers nothing: a plain error =="
set +e
out="$(run_read "$STUB_PORT" "empty" 2>"$WORKDIR/2.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/2.err")"
assert_eq "exit 1" "$rc" "1"
assert_eq "stdout empty" "$out" ""
assert_contains "says nothing answered" "$err" "no answer from the K2 daemon"
assert_absent "not the opaque repr" "$err" "no usable answer: ''"

echo "== 3. dead cell socket, nothing on the port: the same plain error =="
set +e
out="$(run_read "$CLOSED_PORT" "anything" 2>"$WORKDIR/3.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/3.err")"
assert_eq "exit 1" "$rc" "1"
assert_eq "stdout empty" "$out" ""
assert_contains "says nothing answered" "$err" "no answer from the K2 daemon"
assert_absent "not the opaque repr" "$err" "no usable answer: ''"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
