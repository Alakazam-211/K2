#!/usr/bin/env bash
# k2 compute run from a git worktree inside the workspace must name that
# checkout (repoDir) so the daemon runs the worktree's HEAD, not the main
# checkout's. A stub daemon captures the POST /cli/compute/run body.
#
# Pure checks. Fake TOKEN, sandbox HOME, temp repos. Never skip.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
expect_eq() {
    if [ "$2" = "$3" ]; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (got $(printf %q "$2") want $(printf %q "$3"))" >&2; fail=$((fail + 1)); fi
}

WORK="$(mktemp -d -t k2-compute-wt-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

g() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false -c init.defaultBranch=main "$@"; }
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1

WS="$WORK/ws"
mkdir -p "$WS"
g -C "$WS" init -q
echo one > "$WS/a.txt"
g -C "$WS" add .
g -C "$WS" commit -q -m one
WT="$WS/.claude/worktrees/x"
g -C "$WS" worktree add -q -b wt "$WT"
mkdir -p "$WT/sub"

cat > "$WORK/stub.py" <<'PYEOF'
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
out = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n)
        if "/cli/compute/run" in self.path:
            open(out + "/run.json", "wb").write(body)
        b = b'{"error":{"code":"stub","hint":"stub"}}'
        self.send_response(400)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    do_GET = do_POST
    def log_message(self, *a):
        pass
srv = HTTPServer(("127.0.0.1", 0), H)
open(out + "/stub.port", "w").write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
python3 "$WORK/stub.py" "$WORK" 2>"$WORK/stub.err" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] || { echo "FAIL: stub did not start: $(head -c 300 "$WORK/stub.err")" >&2; exit 1; }
PORT="$(cat "$WORK/stub.port")"

export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
echo "$PORT" > "$HOME/.k2/heartbeat.port"
echo "fake-token" > "$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"

run_from() {
    local dir="$1"; shift
    rm -f "$WORK/run.json"
    (
        cd "$dir"
        for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
        K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=fake-token K2_PROJECT_PATH="$WS" \
            "$K2_CLI" compute run --any "$@" -- true >/dev/null 2>&1 || true
    )
    [ -s "$WORK/run.json" ] || { echo "  FAIL: no run body captured from $dir" >&2; fail=$((fail + 1)); return 1; }
}
field() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))' "$WORK/run.json" "$1"; }
real() { python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1"; }

echo "== compute run from a worktree sub-folder =="
if run_from "$WT/sub"; then
    expect_eq "repoDir is the worktree" "$(real "$(field repoDir)")" "$(real "$WT")"
    expect_eq "cwd is relative to the worktree" "$(field cwd)" "sub"
fi

echo "== compute run from the main checkout =="
if run_from "$WS"; then
    expect_eq "repoDir is the main checkout" "$(real "$(field repoDir)")" "$(real "$WS")"
fi

echo "== compute run outside any repo =="
mkdir -p "$WORK/plain"
if run_from "$WORK/plain"; then
    expect_eq "no repoDir outside a repo" "$(field repoDir)" ""
fi

echo ""
echo "compute_run_worktree: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
