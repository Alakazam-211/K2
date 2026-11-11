#!/usr/bin/env bash
# k2 setup — the developer-tools step (`dev_tools`, 0.46.12) against a stub
# daemon: on a Mac it leads the list with its line and the git-tools verb,
# says when Apple's installer is open, and reads [x] once git runs; on
# Linux it prints the distro's install line; a done row prints no note;
# `k2 setup skip dev_tools` posts the row. Nothing is installed: the daemon
# is a stub. The CLI runs with a clean environment (`env -i`, a temp HOME),
# so it can only reach the stub.
# Run with: bash tests/cli/setup_dev_tools.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-setup-dev-tools-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK/home/.k2"
MODE_FILE="$WORK/mode"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

MODE_FILE, LOG_FILE = os.environ["MODE_FILE"], os.environ["LOG_FILE"]
REST = [
    {"id": "llm_sign_in", "state": "todo", "opens": "llm_sign_in"},
    {"id": "first_agent", "state": "todo", "opens": "add_workspace"},
]
DEV = {
    "mac": {"id": "dev_tools", "state": "todo", "opens": "dev_tools"},
    "installing": {"id": "dev_tools", "state": "todo", "opens": "dev_tools", "note": "installing"},
    "done": {"id": "dev_tools", "state": "done", "opens": "dev_tools"},
    "linux": {"id": "dev_tools", "state": "todo", "opens": None, "note": "sudo apt install git"},
    "skipped": {"id": "dev_tools", "state": "skipped", "opens": "dev_tools"},
}

class H(BaseHTTPRequestHandler):
    def _answer(self, method, body):
        u = urlsplit(self.path)
        with open(LOG_FILE, "a") as f:
            f.write("%s %s %s\n" % (method, u.path, body))
        mode = open(MODE_FILE).read().strip()
        rows = ([DEV[mode]] if mode in DEV else []) + REST
        data = json.dumps({"enabled": True, "hidden": False, "finished": False, "rows": rows}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._answer("GET", "")

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._answer("POST", self.rfile.read(n).decode())

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo mac >"$MODE_FILE"
: >"$LOG_FILE"
MODE_FILE="$MODE_FILE" LOG_FILE="$LOG_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -s --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/first-run/status" >/dev/null 2>&1 && break
    sleep 0.1
done
: >"$LOG_FILE"

k2() { (cd "$WORK" && env -i PATH="/usr/bin:/bin:$(dirname "$PY")" HOME="$WORK/home" K2_PORT="$PORT" K2_HOOK_TOKEN=stub-token "$K2_CLI" "$@"); }
run() {
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}

echo "== help =="
run setup --help
assert_eq "help: exit 0" "$rc" "0"
assert_contains "help: dev_tools is a step to skip" "$out" "skip one step: dev_tools, llm_sign_in"

echo "== macOS, git missing =="
run setup
assert_eq "mac: exit 0" "$rc" "0"
assert_eq "mac: first row, its line and the verb" "$(printf '%s\n' "$out" | sed -n 2p)" \
    "  [ ] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)"
assert_eq "mac: then sign-in" "$(printf '%s\n' "$out" | sed -n 3p)" "  [ ] Sign in to an AI tool"
assert_eq "mac: nothing on stderr" "$err" ""

echo "== macOS, installing =="
echo installing >"$MODE_FILE"
run setup
assert_contains "installing: the note" "$out" \
    "  [ ] Install developer tools (The Apple installer is open. Follow it, and this step finishes by itself.)"

echo "== macOS, done =="
echo done >"$MODE_FILE"
run setup
assert_contains "done: [x], no note" "$out" "$(printf '  [x] Install developer tools\n')"
assert_absent "done: no verb" "$out" "k2 git-tools install"
assert_contains "done: counted" "$out" "Setup: 1 of 3 done"

echo "== Linux, git missing =="
echo linux >"$MODE_FILE"
run setup
assert_contains "linux: the install line" "$out" "  [ ] Install developer tools (run: sudo apt install git)"
assert_absent "linux: no Apple" "$out" "Apple"

echo "== git runs: no row =="
echo none >"$MODE_FILE"
run setup
assert_absent "no row" "$out" "developer tools"

echo "== skip =="
echo skipped >"$MODE_FILE"
: >"$LOG_FILE"
run setup skip dev_tools
assert_eq "skip: exit 0" "$rc" "0"
assert_eq "skip: POST the row" "$(cat "$LOG_FILE")" 'POST /cli/first-run/skip {"row": "dev_tools"}'
assert_contains "skip: [-]" "$out" "  [-] Install developer tools"

echo
echo "setup_dev_tools: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
