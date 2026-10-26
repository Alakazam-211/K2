#!/usr/bin/env bash
# k2 git-tools — requests and rendering against a stub daemon (0.46.4,
# first run without git).
#
# status/install/recheck hit the right routes with the right method; the
# Mac state names Apple's command line tools, the install line and the SSH
# steps; Linux prints the package-manager line; a refusal (Linux install:
# 409 not_macos) prints ONE plain stderr line plus how to install, exit 1,
# never a traceback; --json passes the body through. Nothing is installed:
# the daemon is a stub.

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

WORK="$(mktemp -d -t k2-git-tools-cli-XXXXXX)"
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
MAC_HOW = [
    "touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress",
    "softwareupdate --list   # note the \"Command Line Tools for Xcode-NN.N\" label",
    "sudo softwareupdate -i \"Command Line Tools for Xcode-NN.N\" --verbose",
    "rm /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress",
]
WAITING = [{"projectId": "p1", "name": "site", "path": "/Users/me/site", "branch": "main",
            "note": "git: waiting for Apple's command line tools"}]

def mac(state="not_installed", **kw):
    d = {"os": "macos", "state": state, "needs": "apple_clt", "gitPath": None, "gitVersion": None,
         "canInstall": True, "installStartedAt": None, "howTo": MAC_HOW, "waiting": WAITING}
    d.update(kw)
    return d

READY = {"os": "macos", "state": "ready", "needs": None, "gitPath": "/usr/bin/git",
         "gitVersion": "git version 2.39.5", "canInstall": False, "installStartedAt": None,
         "howTo": [], "waiting": []}

class H(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        data = (obj if isinstance(obj, str) else json.dumps(obj)).encode()
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
        if mode == "linux":
            if path == "/cli/git-tools/install":
                return self._send(409, {"error": "K2 installs git only on macOS (Apple's command line tools); install it with your package manager",
                                        "code": "not_macos", "howTo": ["sudo apt install git"]})
            return self._send(200, {"os": "linux", "state": "not_installed", "needs": "git", "gitPath": None,
                                    "gitVersion": None, "canInstall": False, "installStartedAt": None,
                                    "howTo": ["sudo apt install git"], "waiting": []})
        if path == "/cli/git-tools":
            return self._send(200, READY if mode == "ready" else mac())
        if path == "/cli/git-tools/install":
            return self._send(200, mac("installing", started="started", installStartedAt=1))
        if path == "/cli/git-tools/recheck":
            return self._send(200, dict(READY, finished=["/Users/me/site"]))
        return self._send(404, '{"error":"route not found"}')

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
echo mac >"$MODE_FILE"
: >"$LOG_FILE"
MODE_FILE="$MODE_FILE" LOG_FILE="$LOG_FILE" PORT="$PORT" python3 "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -sf --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/git-tools" >/dev/null 2>&1 && break
    sleep 0.1
done
: >"$LOG_FILE"

k2() { env HOME="$HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token "$K2_CLI" "$@"; }
run() {
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
last_request() { tail -n 1 "$LOG_FILE"; }

echo "== help =="
run git-tools --help
assert_eq "help: exit 0" "$rc" "0"
assert_contains "help: install verb" "$out" "k2 git-tools install [--json]"
assert_absent "help: no daemon call" "$(cat "$LOG_FILE")" "/cli/git-tools"

echo "== status on a Mac without the tools =="
run git-tools
assert_eq "status: exit 0" "$rc" "0"
assert_contains "status: GET" "$(last_request)" "GET /cli/git-tools"
assert_contains "status: needs the tools" "$out" "git        : not installed (needs Apple's command line tools)"
assert_contains "status: waiting workspace" "$out" "waiting    : site (/Users/me/site) git: waiting for Apple's command line tools"
assert_contains "status: install line" "$out" "install    : k2 git-tools install"
assert_contains "status: SSH steps" "$out" 'sudo softwareupdate -i "Command Line Tools for Xcode-NN.N" --verbose'
run git-tools status --json
assert_eq "--json: exit 0" "$rc" "0"
assert_contains "--json: body verbatim" "$out" '"needs": "apple_clt"'
run git-tools --json
assert_contains "bare --json = status" "$out" '"state": "not_installed"'

echo "== install / recheck =="
run git-tools install
assert_eq "install: exit 0" "$rc" "0"
assert_eq "install: POST" "$(last_request)" 'POST /cli/git-tools/install {}'
assert_contains "install: points at the dialog" "$out" "Apple's installer is open on this Mac's screen (it can be behind the K2 window)."
assert_contains "install: Install, not Get Xcode" "$out" "Click Install, not Get Xcode (that opens the App Store for the full Xcode),"
assert_contains "install: licence and wait" "$out" "agree to Apple's licence, then wait for the download (about 5-15 min)."
assert_contains "install: installing" "$out" "git        : installing Apple's command line tools"
echo ready >"$MODE_FILE"
run git-tools recheck
assert_eq "recheck: exit 0" "$rc" "0"
assert_eq "recheck: POST" "$(last_request)" 'POST /cli/git-tools/recheck {}'
assert_contains "recheck: ready" "$out" "git        : ready (git version 2.39.5, /usr/bin/git)"
assert_contains "recheck: finished" "$out" "finished   : /Users/me/site"
assert_absent "recheck: no install line once ready" "$out" "install    :"

echo "== Linux =="
echo linux >"$MODE_FILE"
run git-tools
assert_eq "linux status: exit 0" "$rc" "0"
assert_contains "linux: not installed" "$out" "git        : not installed (needs git)"
assert_contains "linux: run it yourself" "$out" "install    : run this yourself:"
assert_contains "linux: apt line" "$out" "  sudo apt install git"
run git-tools install
assert_eq "linux install: exit 1" "$rc" "1"
assert_contains "linux install: one plain line" "$err" "k2 git-tools: error: K2 installs git only on macOS"
assert_contains "linux install: how to" "$err" "  sudo apt install git"
assert_absent "linux install: no traceback" "$err" "Traceback"

echo "== bad usage =="
run git-tools fix
assert_eq "unknown verb: exit 2" "$rc" "2"
run git-tools status --force
assert_eq "unknown flag: exit 2" "$rc" "2"

echo "== no daemon =="
kill "$STUB_PID" 2>/dev/null || true
wait "$STUB_PID" 2>/dev/null || true
STUB_PID=""
run git-tools
assert_eq "no daemon: exit 1" "$rc" "1"
assert_contains "no daemon: unreachable" "$err" "daemon unreachable"

echo
echo "git_tools_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
