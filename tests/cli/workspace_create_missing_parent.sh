#!/usr/bin/env bash
# k2 workspace create under a missing parent (field report 2026-10-16,
# fresh Debian 13 box): `k2 workspace create /home/k2/ai/hearth` with no
# ~/ai sent path=/hearth to the daemon — the CLI's `cd "$(dirname p)"`
# failed and the path collapsed to "/<basename>" — and the daemon's mkdir
# of /hearth failed with "Permission denied (os error 13)".
#
# 1. cli_abs_path keeps the full path when parents are missing.
# 2. `k2 workspace create` sends that full path to a stub daemon.
# No real daemon, never the real ~/.k2. The daemon half (mkdir -p, the
# "not writable" copy) is covered by k2-core lifecycle tests.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "  PASS: $*"; }

[ -x "$K2" ] || fail "$K2 not executable"
bash -n "$K2" || fail "bash -n cli/k2"

SANDBOX="$(mktemp -d -t k2-ws-create-parent-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

echo "== cli_abs_path =="
FN="$SANDBOX/fn.sh"
python3 - "$K2" "$FN" <<'PY'
import pathlib, re, sys
src = pathlib.Path(sys.argv[1]).read_text()
m = re.search(r'^cli_abs_path\(\) \{\n.*?^\}\n', src, re.M | re.S)
if not m:
    print("FAIL: cli_abs_path not found in cli/k2", file=sys.stderr)
    sys.exit(1)
pathlib.Path(sys.argv[2]).write_text(m.group(0))
PY
# shellcheck disable=SC1090
source "$FN"

mkdir -p "$SANDBOX/home/existing"
got="$(cli_abs_path "$SANDBOX/home/ai/team/hearth")"
[ "$got" = "$SANDBOX/home/ai/team/hearth" ] || fail "missing nested parent: got '$got'"
pass "missing nested parent keeps the full path"

got="$(cli_abs_path "$SANDBOX/home/existing")"
[ "$got" = "$SANDBOX/home/existing" ] || fail "existing dir: got '$got'"
pass "existing dir"

got="$(cli_abs_path "$SANDBOX/home/existing/")"
[ "$got" = "$SANDBOX/home/existing" ] || fail "trailing slash: got '$got'"
pass "trailing slash dropped"

got="$(cd "$SANDBOX/home" && cli_abs_path "ai/hearth")"
[ "$got" = "$SANDBOX/home/ai/hearth" ] || fail "relative missing parent: got '$got'"
pass "relative path with a missing parent"

got="$(cd "$SANDBOX/home/existing" && cli_abs_path "../new/x")"
[ "$got" = "$SANDBOX/home/new/x" ] || fail "dot-dot into missing: got '$got'"
pass "../ resolved through the existing part"

got="$(cli_abs_path "/nonexistent-k2-root-$$/ai/hearth")"
[ "$got" = "/nonexistent-k2-root-$$/ai/hearth" ] || fail "missing top-level: got '$got'"
pass "missing top-level dir"

echo "== k2 workspace create sends the full path (stub daemon) =="
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
LOG="$SANDBOX/requests.log"
cat >"$SANDBOX/stub.py" <<'PY'
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = os.environ["LOG"]

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        u = urllib.parse.urlsplit(self.path)
        q = urllib.parse.parse_qs(u.query)
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        if u.path != "/cli/workspace/create":
            self.send_response(404); self.end_headers(); return
        path = q.get("path", [""])[0]
        with open(LOG, "a") as f:
            f.write(path + "\n")
        body = json.dumps({"success": True, "path": path}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
PORT="$PORT" LOG="$LOG" python3 -I "$SANDBOX/stub.py" &
STUB_PID=$!
for _ in $(seq 1 50); do
    if python3 -c 'import socket,sys; socket.create_connection(("127.0.0.1",int(sys.argv[1])),0.2)' "$PORT" 2>/dev/null; then
        break
    fi
    sleep 0.1
done

mkdir -p "$SANDBOX/home/.k2"
TARGET="$SANDBOX/home/ai/hearth"
out="$(env -i PATH="$PATH" HOME="$SANDBOX/home" K2_HOST=127.0.0.1 K2_PORT="$PORT" \
    K2_HOOK_TOKEN=test-token "$K2" workspace create "$TARGET" --no-wiki 2>&1)" \
    || fail "k2 workspace create exited non-zero: $out"
[ -f "$LOG" ] || fail "stub daemon got no /cli/workspace/create request; CLI said: $out"
sent="$(cat "$LOG")"
[ "$sent" = "$TARGET" ] || fail "daemon got path='$sent', want '$TARGET'"
pass "daemon got the full path, not /hearth"
case "$out" in
    *'"success": true'*|*'"success":true'*) pass "CLI printed the success body" ;;
    *) fail "unexpected CLI output: $out" ;;
esac

echo "PASS: workspace_create_missing_parent"
