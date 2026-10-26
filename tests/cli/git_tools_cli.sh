#!/usr/bin/env bash
# k2 git-tools — requests and rendering against a stub daemon (0.46.4,
# first run without git).
#
# - status/install/recheck hit the right routes with the right method,
#   `token=` percent-encoded in pure bash and `format=text` (the daemon
#   renders the lines; its wording is tested in Rust, git_tools::render_text);
#   `--json` asks for JSON and passes it through.
# - NO python3: on a fresh Mac /usr/bin/python3 is Apple's shim (it opens
#   the install dialog or prints an xcrun error). Every verb here runs with
#   a python3 first on PATH that exits 1 with the xcrun note and leaves a
#   mark; the mark must never appear.
# - A refusal (409 text) prints the daemon's lines on stderr, exit 1; a
#   router refusal (JSON 403) prints one plain line; no daemon → one line.
# Nothing is installed: the daemon is a stub (python3 runs only the stub
# server, from its absolute path, outside the CLI's PATH).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

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

# Apple's python3 shim on a Mac without the tools.
SHIMS="$WORK/shims"
PY_MARK="$WORK/python3-ran"
mkdir -p "$SHIMS"
cat >"$SHIMS/python3" <<SH
#!/bin/sh
: > '$PY_MARK'
echo 'xcode-select: note: No developer tools were found, requesting install.' >&2
exit 1
SH
chmod 755 "$SHIMS/python3"
cp "$SHIMS/python3" "$SHIMS/python"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

MODE_FILE, LOG_FILE = os.environ["MODE_FILE"], os.environ["LOG_FILE"]
MAC_TEXT = ("git        : not installed (needs Apple's command line tools)\n"
            "waiting    : site (/Users/me/site) git: waiting for Apple's command line tools\n"
            "  last try : Failed to initialize git: boom\n")
INSTALL_TEXT = "Apple's installer is open on this Mac's screen (it can be behind the K2 window).\n"
READY_TEXT = "git        : ready (git version 2.39.5, /usr/bin/git)\nfinished   : /Users/me/site\n"

class H(BaseHTTPRequestHandler):
    def _send(self, code, body, ctype):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _answer(self, method, body):
        u = urlsplit(self.path)
        with open(LOG_FILE, "a") as f:
            f.write("%s %s %s %s\n" % (method, u.path, u.query, body))
        q = parse_qs(u.query)
        text = q.get("format") == ["text"]
        mode = open(MODE_FILE).read().strip()
        if q.get("token") != ["t.k/+= z"]:
            return self._send(403, '{"error":"invalid or missing token"}', "application/json")
        if mode == "linux" and u.path == "/cli/git-tools/install":
            if text:
                return self._send(409, "k2 git-tools: K2 installs git only on macOS\n  sudo apt install git\n", "text/plain")
            return self._send(409, json.dumps({"error": "K2 installs git only on macOS", "code": "not_macos"}), "application/json")
        if not text:
            return self._send(200, json.dumps({"state": "not_installed", "needs": "apple_clt"}), "application/json")
        if u.path == "/cli/git-tools":
            return self._send(200, MAC_TEXT, "text/plain")
        if u.path == "/cli/git-tools/install":
            return self._send(200, INSTALL_TEXT, "text/plain")
        if u.path == "/cli/git-tools/recheck":
            return self._send(200, READY_TEXT, "text/plain")
        return self._send(404, '{"error":"route not found"}', "application/json")

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
    curl -s --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/git-tools" >/dev/null 2>&1 && break
    sleep 0.1
done
: >"$LOG_FILE"

# The CLI runs with Apple's python3 shim first on PATH, from a folder
# that is not a K2 workspace.
k2() { (cd "$WORK" && env PATH="$SHIMS:$PATH" HOME="$HOME" K2_PORT="$PORT" K2_HOOK_TOKEN='t.k/+= z' "$K2_CLI" "$@"); }
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

echo "== status =="
run git-tools
assert_eq "status: exit 0" "$rc" "0"
assert_eq "status: GET, token encoded, text" "$(last_request)" "GET /cli/git-tools token=t.k%2F%2B%3D%20z&format=text "
assert_eq "status: the daemon's lines verbatim" "$out" "$(printf "git        : not installed (needs Apple's command line tools)\nwaiting    : site (/Users/me/site) git: waiting for Apple's command line tools\n  last try : Failed to initialize git: boom")"
assert_eq "status: nothing on stderr" "$err" ""
run git-tools status --json
assert_eq "--json: exit 0" "$rc" "0"
assert_eq "--json: no format=text" "$(last_request)" "GET /cli/git-tools token=t.k%2F%2B%3D%20z "
assert_contains "--json: body verbatim" "$out" '"needs": "apple_clt"'
run git-tools --json
assert_contains "bare --json = status" "$out" '"state": "not_installed"'

echo "== install / recheck =="
run git-tools install
assert_eq "install: exit 0" "$rc" "0"
assert_eq "install: POST {}" "$(last_request)" "POST /cli/git-tools/install token=t.k%2F%2B%3D%20z&format=text {}"
assert_contains "install: the daemon's line" "$out" "Apple's installer is open on this Mac's screen"
run git-tools recheck
assert_eq "recheck: exit 0" "$rc" "0"
assert_eq "recheck: POST {}" "$(last_request)" "POST /cli/git-tools/recheck token=t.k%2F%2B%3D%20z&format=text {}"
assert_contains "recheck: finished" "$out" "finished   : /Users/me/site"

echo "== refusals =="
echo linux >"$MODE_FILE"
run git-tools install
assert_eq "linux install: exit 1" "$rc" "1"
assert_eq "linux install: the daemon's lines on stderr" "$err" "$(printf 'k2 git-tools: K2 installs git only on macOS\n  sudo apt install git')"
assert_eq "linux install: nothing on stdout" "$out" ""
run git-tools install --json
assert_eq "linux install --json: exit 1" "$rc" "1"
assert_contains "linux install --json: body" "$out" '"code": "not_macos"'
set +e
out="$(cd "$WORK" && env PATH="$SHIMS:$PATH" HOME="$HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=wrong "$K2_CLI" git-tools 2>"$WORK/err")"
rc=$?
set -e
err="$(cat "$WORK/err")"
assert_eq "bad token: exit 1" "$rc" "1"
assert_eq "bad token: one plain line" "$err" "k2 git-tools: error: invalid or missing token"

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

echo "== python3 =="
if [ -e "$PY_MARK" ]; then bad "k2 git-tools ran python3 (Apple's shim on a fresh Mac)"; else ok "k2 git-tools never ran python3"; fi

echo
echo "git_tools_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
