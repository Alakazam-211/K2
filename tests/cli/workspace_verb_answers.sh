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

echo "== a lookup K2 never answers: the no-answer line, exit 1, no next request =="
# "label|lookup route|next route|args"
LOOKUPS=(
    "launch <name>|/cli/workspace/resolve|/cli/workspace/ensure-canonical-session|workspace launch acme"
    "update|/cli/workspace/agent-display-name|/cli/agent/update|workspace update --field role --value Lead"
)
for c in "${LOOKUPS[@]}"; do
    IFS='|' read -r name lroute next args <<<"$c"
    answer "$lroute" 500 "" "$next" 200 '{"success":true}'
    # shellcheck disable=SC2086
    run $args
    assert_eq "$name: no answer: exit 1" "$rc" "1"
    assert_eq "$name: no answer: nothing on stdout" "$out" ""
    assert_contains "$name: no answer: the no-answer line" "$err" "Error: K2's server gave an empty answer to $lroute after "
    assert_eq "$name: no answer: one line" "$(printf '%s\n' "$err" | wc -l | tr -d ' ')" "1"
    if grep -Fq "\"path\": \"$next\"" "$LOG_FILE"; then bad "$name: no answer: $next was still sent"; else ok "$name: no answer: $next not sent"; fi
done

echo "== nothing takes the connection: the no-answer line, exit 1 =="
CLOSED="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
mkdir -p "$WORK/closed-home/.k2"
printf '%s' "$OWNER" >"$WORK/closed-home/.k2/heartbeat.token"
printf '%s' "$CLOSED" >"$WORK/closed-home/.k2/heartbeat.port"
# "label|first route|args"
REFUSED_CASES=(
    "list|/cli/projects/list|workspace list"
    "list --json|/cli/agent/list|workspace list --json"
    "create|/cli/workspace/create|workspace create $WORK/never"
    "open|/cli/workspace/open|workspace open $WS"
    "launch <path>|/cli/workspace/ensure-canonical-session|workspace launch $WS"
    "launch <name>|/cli/workspace/resolve|workspace launch acme"
    "profile|/cli/workspace/agent-display-name|workspace profile"
    "update|/cli/workspace/agent-display-name|workspace update --field role --value Lead"
)
for c in "${REFUSED_CASES[@]}"; do
    IFS='|' read -r name route args <<<"$c"
    set +e
    # shellcheck disable=SC2086
    out="$(cd "$WS" && env -i PATH="/usr/bin:/bin" HOME="$WORK/closed-home" K2_PROJECT_PATH="$WS" "$K2_CLI" $args 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
    assert_eq "refused connect: $name: exit 1" "$rc" "1"
    assert_eq "refused connect: $name: nothing on stdout" "$out" ""
    assert_eq "refused connect: $name: says so" "$err" "Error: K2's server didn't answer $route: nothing took the connection at http://127.0.0.1:$CLOSED; run k2 daemon status"
done

echo "== launch <name>: a resolve answer with no folder is refused =="
ENSURE=/cli/workspace/ensure-canonical-session
for body in '{"path":null}' '{"path":""}' '{"workspace":"acme"}'; do
    answer /cli/workspace/resolve 200 "$body" "$ENSURE" 200 '{"success":true}'
    run workspace launch acme
    assert_eq "resolve $body: exit 1" "$rc" "1"
    assert_eq "resolve $body: nothing on stdout" "$out" ""
    assert_eq "resolve $body: says so" "$err" "Error: K2's server gave no folder for workspace 'acme' (/cli/workspace/resolve answered: $body)"
    if grep -Fq "\"path\": \"$ENSURE\"" "$LOG_FILE"; then bad "resolve $body: launched anyway"; else ok "resolve $body: nothing launched"; fi
done
answer /cli/workspace/resolve 400 '{"error":"workspace not found: acme"}' "$ENSURE" 200 '{"success":true}'
run workspace launch acme
assert_eq "resolve not found: exit 1" "$rc" "1"
assert_eq "resolve not found: the daemon's words" "$err" "Error: workspace not found: acme"
if grep -Fq "\"path\": \"$ENSURE\"" "$LOG_FILE"; then bad "resolve not found: launched anyway"; else ok "resolve not found: nothing launched"; fi
answer /cli/workspace/resolve 200 "{\"path\":\"$WS\"}" "$ENSURE" 200 '{"success":true}'
run workspace launch acme
assert_eq "resolve found: exit 0" "$rc" "0"
WS_Q="$("$PY" -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$WS")"
assert_contains "resolve found: launches that folder" "$(grep -F "\"path\": \"$ENSURE\"" "$LOG_FILE")" "project=$WS_Q"
assert_not_contains "resolve found: never the word None" "$(cat "$LOG_FILE")" "project=None"

echo "== launch/profile <path>: one project=, the target's =="
WS2="$WORK/ws2"
mkdir -p "$WS2/.k2"
# project_params <route>: the project= values that route was sent, one per line.
project_params() {
    "$PY" -c 'import json, sys
from urllib.parse import parse_qs
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r["path"] == sys.argv[2]:
        for v in parse_qs(r["query"], keep_blank_values=True).get("project", []):
            print(v)' "$LOG_FILE" "$1"
}
answer "$ENSURE" 200 '{"success":true}' /cli/workspace/agent-display-name 200 "$NAME_OK"
run workspace launch "$WS2"
assert_eq "launch <path>: exit 0" "$rc" "0"
assert_eq "launch <path>: one project=, the target" "$(project_params "$ENSURE")" "$WS2"
run workspace profile "$WS2"
assert_eq "profile <path>: exit 0" "$rc" "0"
assert_eq "profile <path>: one project=, the target" "$(project_params /cli/workspace/agent-display-name)" "$WS2"
run workspace profile
assert_eq "profile: one project=, this workspace" "$(project_params /cli/workspace/agent-display-name)" "$WS"
run workspace launch
assert_eq "launch: one project=, this workspace" "$(project_params "$ENSURE")" "$WS"

echo "== launch: flags first; --json honoured, --no-attach and unknown words refused =="
answer "$ENSURE" 200 '{"success":true,"session_id":"s-1"}'
run workspace launch --json
assert_eq "launch --json: exit 0" "$rc" "0"
assert_eq "launch --json: this workspace" "$(project_params "$ENSURE")" "$WS"
assert_eq "launch --json: K2's JSON answer" "$out" '{"success":true,"session_id":"s-1"}'
run workspace launch --json "$WS2"
assert_eq "launch --json <path>: the path" "$(project_params "$ENSURE")" "$WS2"
run workspace launch "$WS2" --json
assert_eq "launch <path> --json: the path" "$(project_params "$ENSURE")" "$WS2"
for c in "--no-attach|Error: --no-attach: k2 workspace launch never attaches a terminal" \
         "$WS2 --no-attach|Error: --no-attach: k2 workspace launch never attaches a terminal" \
         "--bogus|Unknown flag: --bogus" \
         "$WS $WS2|Unexpected extra argument: $WS2"; do
    args="${c%%|*}"; want="${c#*|}"
    # shellcheck disable=SC2086
    run workspace launch $args
    assert_eq "launch $args: exit 2" "$rc" "2"
    assert_contains "launch $args: says why" "$err" "$want"
    assert_eq "launch $args: no request" "$(wc -l <"$LOG_FILE" | tr -d ' ')" "0"
done

echo "== update: a flag with no value is a named usage error, exit 2 =="
USAGE="Usage: k2 workspace update --field <f> --value <v>"
for c in "--field|Error: --field needs a value
$USAGE" \
         "--field role --value|Error: --value needs a value
$USAGE" \
         "--value Lead --field|Error: --field needs a value
$USAGE" \
         "--value Lead|$USAGE" \
         "--field role --value Lead --bogus|Error: unknown argument: --bogus
$USAGE"; do
    args="${c%%|*}"; want="${c#*|}"
    # shellcheck disable=SC2086
    run workspace update $args
    assert_eq "update $args: exit 2" "$rc" "2"
    assert_eq "update $args: says why" "$err" "$want"
    assert_eq "update $args: no request" "$(wc -l <"$LOG_FILE" | tr -d ' ')" "0"
done

echo "== list: the table's errors go to stderr =="
answer /cli/projects/list 200 '{"error":"boom"}'
run workspace list
assert_eq "list error: exit 1" "$rc" "1"
assert_eq "list error: nothing on stdout" "$out" ""
assert_eq "list error: on stderr" "$err" "Error: boom"
answer /cli/projects/list 200 'garbage'
run workspace list
assert_eq "list unreadable: exit 1" "$rc" "1"
assert_eq "list unreadable: nothing on stdout" "$out" ""
assert_eq "list unreadable: on stderr" "$err" "Error: daemon returned an unparseable response: 'garbage'"
answer /cli/projects/list 200 "[{\"name\":\"acme\",\"agentMode\":\"custom\",\"path\":\"$WS\",\"agentEnabled\":true}]"
run workspace list
assert_eq "list: exit 0" "$rc" "0"
assert_contains "list: the row" "$out" "acme"

echo "== list --help/-h: the router's workspace help, before list runs =="
for args in "--help" "-h" "--json -h" "--running --help"; do
    # shellcheck disable=SC2086
    run workspace list $args
    assert_eq "list $args: exit 0" "$rc" "0"
    assert_contains "list $args: the workspace help" "$out" "k2 workspace"
    assert_eq "list $args: no request" "$(wc -l <"$LOG_FILE" | tr -d ' ')" "0"
done

echo "== agent update: a flag with no value is a named usage error, exit 2 =="
AUSAGE='Usage: k2 agent update --name <agent> --field <field> --value "..."'
AFIELD="  field: frontmatter key (role, type) or section name (Work Sources, Capabilities)"
for c in "--name|Error: --name needs a value
$AUSAGE" \
         "--name acme --field|Error: --field needs a value
$AUSAGE" \
         "--name acme --field role --value|Error: --value needs a value
$AUSAGE" \
         "--name acme --field role|$AUSAGE
$AFIELD"; do
    args="${c%%|*}"; want="${c#*|}"
    # shellcheck disable=SC2086
    run agent update $args
    assert_eq "agent update $args: exit 2" "$rc" "2"
    assert_eq "agent update $args: says why" "$err" "$want"
    assert_eq "agent update $args: no request" "$(wc -l <"$LOG_FILE" | tr -d ' ')" "0"
done
answer /cli/agent/update 200 '{"success":true,"content":"x"}'
run agent update --name acme --field role --value Lead
assert_eq "agent update: exit 0" "$rc" "0"
assert_eq "agent update: the success line" "$out" "Updated role for agent acme"
answer /cli/agent/update 400 '{"error":"Agent '"'"'acme'"'"' does not exist"}'
run agent update --name acme --field role --value Lead
assert_eq "agent update error: exit 1" "$rc" "1"
assert_eq "agent update error: nothing on stdout" "$out" ""
assert_eq "agent update error: on stderr" "$err" "Error: Agent 'acme' does not exist"

echo
echo "workspace_verb_answers: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
