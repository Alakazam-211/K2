#!/usr/bin/env bash
# What `k2 workspace create|open|launch|profile|update|list` do with K2's
# server's answer. A false success is the bug this guards: an answer that
# is empty, not JSON, not an object or an error must never print as
# success with exit 0.
#
# Nothing is installed: the daemon is a stub that answers each route with
# what the case writes to its answers file, and logs each request. The CLI
# runs with a clean environment (`env -i`, a temp HOME holding the owner
# token), so it can only reach the stub.
# Run with: bash tests/cli/workspace_verb_answers.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (no $(printf %q "$3") in $(printf %q "$2"))" ;; esac; }
assert_not_contains() { case "$2" in *"$3"*) bad "$1 ($(printf %q "$3") in $(printf %q "$2"))" ;; *) ok "$1" ;; esac; }

WORK="$(mktemp -d -t k2-ws-answers-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-tok-8b2d4f"
LOG_FILE="$WORK/requests"
ANSWERS="$WORK/answers.json"

# answers.json: {"<path>": [status, "body"]}; any other path is a 404.
cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

LOG_FILE = os.environ["LOG_FILE"]
ANSWERS = os.environ["ANSWERS"]

class H(BaseHTTPRequestHandler):
    def _answer(self, method):
        u = urlsplit(self.path)
        with open(LOG_FILE, "a") as f:
            f.write(json.dumps({"method": method, "path": u.path, "query": u.query,
                                "auth": self.headers.get("Authorization")}) + "\n")
        with open(ANSWERS) as f:
            answers = json.load(f)
        code, body = answers.get(u.path, [404, '{"error":"not found"}'])
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._answer("GET")

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        self._answer("POST")

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

WS="$WORK/ws"
mkdir -p "$WS/.k2"
PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo '{}' >"$ANSWERS"
: >"$LOG_FILE"
LOG_FILE="$LOG_FILE" ANSWERS="$ANSWERS" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

mkdir -p "$WORK/home/.k2"
printf '%s' "$OWNER" >"$WORK/home/.k2/heartbeat.token"
chmod 600 "$WORK/home/.k2/heartbeat.token"
printf '%s' "$PORT" >"$WORK/home/.k2/heartbeat.port"

# answer <path> <status> <body> [<path> <status> <body>]...: what the stub says.
answer() {
    "$PY" -c 'import json, sys
a = sys.argv[2:]
json.dump({a[i]: [int(a[i + 1]), a[i + 2]] for i in range(0, len(a), 3)}, open(sys.argv[1], "w"))' "$ANSWERS" "$@"
}
# run <k2 args...>: the owner's own terminal, in the workspace.
run() {
    : >"$LOG_FILE"
    set +e
    out="$(cd "$WS" && env -i PATH="/usr/bin:/bin" HOME="$WORK/home" K2_PROJECT_PATH="$WS" "$K2_CLI" "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}

echo "== create/open: an answer that is no success is an error, exit 1 =="
for verb in create open; do
    route="/cli/workspace/$verb"
    target="$WORK/new-$verb"
    [ "$verb" = open ] && target="$WS"
    answer "$route" 200 ""
    run workspace "$verb" "$target"
    assert_eq "$verb: empty 200: exit 1" "$rc" "1"
    assert_eq "$verb: empty 200: nothing on stdout" "$out" ""
    assert_eq "$verb: empty 200: says so" "$err" "Error: K2's server gave an empty answer to $route"
    answer "$route" 200 "<html>oops</html>"
    run workspace "$verb" "$target"
    assert_eq "$verb: not JSON: exit 1" "$rc" "1"
    assert_eq "$verb: not JSON: nothing on stdout" "$out" ""
    assert_eq "$verb: not JSON: says so" "$err" "Error: K2's server answered $route with something that is not JSON: '<html>oops</html>'"
    answer "$route" 200 "[1, 2]"
    run workspace "$verb" "$target"
    assert_eq "$verb: JSON array: exit 1" "$rc" "1"
    assert_eq "$verb: JSON array: nothing on stdout" "$out" ""
    assert_eq "$verb: JSON array: says why" "$err" "Error: K2's server answered $route with JSON that is not an object: [1, 2]"
    answer "$route" 400 '{"error":"Directory already exists"}'
    run workspace "$verb" "$target"
    assert_eq "$verb: an error: exit 1" "$rc" "1"
    assert_eq "$verb: an error: on stderr" "$err" "Error: Directory already exists"
    answer "$route" 200 '{"success":true}'
    run workspace "$verb" "$target"
    assert_eq "$verb: success: exit 0" "$rc" "0"
    assert_eq "$verb: success: the answer" "$out" '{"success":true}'
done

echo "== launch/profile/update: an error goes to stderr with its exit and closing line =="
CARD='{"error":{"code":"owner_only","hint":"this needs the owner"},"permission":{"command":"k2 permission request ws-change"}}'
NAME_OK='{"display_name":"Acme","handle":"acme"}'
# "label|final route|lookup route|lookup answer|args" (args split on spaces).
CASES=(
    "launch|/cli/workspace/ensure-canonical-session|||workspace launch $WS"
    "profile|/cli/workspace/agent-display-name|||workspace profile"
    "update|/cli/agent/update|/cli/workspace/agent-display-name|$NAME_OK|workspace update --field role --value Lead"
)
for c in "${CASES[@]}"; do
    IFS='|' read -r name route lroute lbody args <<<"$c"
    lookup=()
    [ -n "$lroute" ] && lookup=("$lroute" 200 "$lbody")
    answer ${lookup[@]+"${lookup[@]}"} "$route" 400 '{"error":"workspace not found: nope"}'
    # shellcheck disable=SC2086
    run $args
    assert_eq "$name: an error: exit 1" "$rc" "1"
    assert_eq "$name: an error: nothing on stdout" "$out" ""
    assert_eq "$name: an error: on stderr" "$err" "Error: workspace not found: nope"
    answer ${lookup[@]+"${lookup[@]}"} "$route" 403 "$CARD"
    # shellcheck disable=SC2086
    run $args
    assert_eq "$name: a permission card: exit 3" "$rc" "3"
    assert_eq "$name: a permission card: nothing on stdout" "$out" ""
    assert_eq "$name: a permission card: the error, then the closing line" "$err" "Error: this needs the owner
Ask with: k2 permission request ws-change"
    answer ${lookup[@]+"${lookup[@]}"} "$route" 200 ""
    # shellcheck disable=SC2086
    run $args
    assert_eq "$name: empty 200: exit 1" "$rc" "1"
    assert_eq "$name: empty 200: says so" "$err" "Error: K2's server gave an empty answer to $route"
    answer ${lookup[@]+"${lookup[@]}"} "$route" 200 '{"success":true}'
    # shellcheck disable=SC2086
    run $args
    assert_eq "$name: success: exit 0" "$rc" "0"
    assert_eq "$name: success: the answer" "$out" '{"success":true}'
done

echo
echo "workspace_verb_answers: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
