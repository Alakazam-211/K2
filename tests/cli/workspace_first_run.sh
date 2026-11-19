#!/usr/bin/env bash
# The k2 workspace verbs a first run uses, against a stub daemon: list
# (the table, --running, --json), create and open (how a folder becomes a
# workspace), launch, profile and update.
#
# list reads /cli/projects/list (the table) or /cli/agent/list (--json).
# create sends the absolute path (missing parents kept as typed), open a
# folder as `cd` names it; both print the daemon's answer, or its error
# as one "Error:" line with exit 1 (exit 3 for a refusal with a
# permission card). launch asks the daemon to resolve a name that is no
# folder, then POSTs /cli/workspace/ensure-canonical-session; update asks
# for the agent's name, then POSTs /cli/agent/update; both print the
# answer as it came. A POST with no answer is sent once: one line, exit 1.
#
# Nothing is installed and nothing real is reached: the daemon is a stub
# on loopback (python3 runs only the stub), the CLI runs with a clean
# environment (`env -i`, a temp HOME with a made-up owner token).
# Run with: bash tests/cli/workspace_first_run.sh

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

WORK="$(mktemp -d -t k2-ws-first-run-XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

K2H="$WORK/home/.k2"
WS="$WORK/ws"
mkdir -p "$K2H" "$WS/.k2" "$WS/sub" "$WORK/existing"
MODE_FILE="$WORK/mode"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qsl

MODE_FILE, LOG_FILE = os.environ["MODE_FILE"], os.environ["LOG_FILE"]
ROWS = ('[{"name":"acme","agentMode":"agent","path":"/srv/acme ws","agentEnabled":true},'
        '{"name":"globex","agentMode":null,"path":"/srv/globex","agentEnabled":false}]')
AGENTS = '{"ok":true,"agents":[{"name":"acme","enabled":true},{"name":"globex","enabled":false}]}'
ANSWERS = {
    # mode: {path: (status, body)}
    "ok": {"/cli/projects/list": (200, ROWS),
           "/cli/agent/list": (200, AGENTS),
           "/cli/workspace/create": (200, '{"ok":true}'),
           "/cli/workspace/open": (200, '{"ok":true}'),
           "/cli/workspace/resolve": (200, '{"path":"/srv/acme ws"}'),
           "/cli/workspace/ensure-canonical-session": (200, '{"ok":true,"action":"spawn"}'),
           "/cli/workspace/agent-display-name": (200, '{"display_name":"Acme"}'),
           "/cli/agent/update": (200, '{"ok":true}')},
    "none": {"/cli/projects/list": (200, "[]"),
             "/cli/workspace/agent-display-name": (200, '{"display_name":""}'),
             "/cli/agent/update": (200, '{"ok":true}')},
    "exists": {"/cli/workspace/create": (409, '{"error":"A workspace already exists at that path."}'),
               "/cli/workspace/open": (400, '{"error":{"code":"not_a_folder","hint":"That is not a folder."}}')},
    "card": {"/cli/workspace/create": (403, '{"error":{"code":"owner_only","hint":"Only the owner can add workspaces."},'
                                            '"permission":{"command":"k2 permission request --scope workspace.write --why \\"...\\"","requestable":true}}')},
}

class H(BaseHTTPRequestHandler):
    def _answer(self, method):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        u = urlsplit(self.path)
        q = dict(parse_qsl(u.query, keep_blank_values=True))
        with open(LOG_FILE, "a") as f:
            f.write("%s %s %s\n" % (method, u.path, " ".join("%s=%s" % (k, q[k]) for k in sorted(q) if k != "token")))
        mode = open(MODE_FILE).read().strip()
        if mode == "close":
            self.close_connection = True
            return
        status, body = ANSWERS.get(mode, {}).get(u.path, (404, '{"error":"no route"}'))
        data = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._answer("GET")

    def do_POST(self):
        self._answer("POST")

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo ok >"$MODE_FILE"
: >"$LOG_FILE"
MODE_FILE="$MODE_FILE" LOG_FILE="$LOG_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -s --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/projects/list" >/dev/null 2>&1 && break
    sleep 0.1
done

# The owner's terminal: a port and a made-up owner token in the K2 home.
printf '%s\n' "$PORT" >"$K2H/heartbeat.port"
printf 'owner-tok-workspace\n' >"$K2H/heartbeat.token"

k2() { (cd "$WS" && env -i PATH="/usr/bin:/bin:${PATH}" HOME="$WORK/home" TMPDIR="$WORK" "$K2_CLI" "$@"); }
run() {
    local mode="$1"; shift
    echo "$mode" >"$MODE_FILE"
    : >"$LOG_FILE"
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
requests() { tr '\n' ';' <"$LOG_FILE"; }

echo "== workspace list =="
run ok workspace list
assert_eq "list: exit 0" "$rc" "0"
assert_eq "list: one request" "$(requests)" "GET /cli/projects/list project=$WS;"
assert_eq "list: the table" "$out" "$(printf '%s\n' \
    "  acme                     [agent   ] /srv/acme ws" \
    "  globex                   [off     ] /srv/globex")"
run ok workspace list --running
assert_eq "list --running: only the enabled" "$out" "  acme                     [agent   ] /srv/acme ws"
run none workspace list
assert_eq "list: none" "$out" "No workspaces registered."
run ok workspace list --json
assert_eq "list --json: the agent rows" "$(requests)" "GET /cli/agent/list project=$WS;"
assert_eq "list --json: the JSON" "$out" '{"ok": true, "workspaces": [{"name": "acme", "enabled": true}, {"name": "globex", "enabled": false}]}'
run ok workspace list --json --running
assert_eq "list --json --running" "$out" '{"ok": true, "workspaces": [{"name": "acme", "enabled": true}]}'

echo "== workspace create / open =="
run ok workspace create "$WORK/existing/ai/hearth" --no-wiki
assert_eq "create: exit 0" "$rc" "0"
assert_eq "create: the full path, missing parents kept" "$(requests)" "POST /cli/workspace/create no_wiki=1 path=$WORK/existing/ai/hearth project=$WS;"
assert_eq "create: the answer" "$out" '{"ok":true}'
run ok workspace create ../new-ws
assert_eq "create: a relative path, absolute" "$(requests)" "POST /cli/workspace/create path=$WORK/new-ws project=$WS;"
run ok workspace open sub/
assert_eq "open: a folder as cd names it" "$(requests)" "POST /cli/workspace/open path=$WS/sub project=$WS;"
run exists workspace create "$WORK/existing"
assert_eq "create: the daemon's error, exit 1" "$rc" "1"
assert_eq "create: the error line" "$err" "Error: A workspace already exists at that path."
assert_eq "create: nothing on stdout" "$out" ""
run exists workspace open "$WORK/existing"
assert_eq "open: the hint, exit 1" "$rc:$err" "1:Error: That is not a folder."
run card workspace create "$WORK/existing"
assert_eq "create: a permission card, exit 3" "$rc" "3"
assert_eq "create: the error, then the closing line" "$err" "$(printf '%s\n' \
    "Error: Only the owner can add workspaces." \
    'Ask with: k2 permission request --scope workspace.write --why "..."')"
run ok workspace create
assert_eq "create: no path, exit 1" "$rc" "1"
assert_eq "create: the usage line" "$err" "Usage: k2 workspace create <path> [--no-wiki] [--no-agents-md] [--fanout]"
assert_eq "create: nothing sent" "$(requests)" ""
run ok workspace open a b
assert_eq "open: a second path" "$rc:$err" "1:Unexpected extra argument: b"
run close workspace create "$WORK/existing"
assert_eq "create: no answer, exit 1" "$rc" "1"
case "$err" in "Error: K2's server gave an empty answer to /cli/workspace/create after "*" s; run k2 daemon status") ok "create: no answer, the line" ;; *) bad "create: no answer, the line (got $(printf %q "$err"))" ;; esac
assert_eq "create: no answer, sent once" "$(requests)" "POST /cli/workspace/create path=$WORK/existing project=$WS;"

echo "== workspace launch / profile / update =="
run ok workspace launch
assert_eq "launch: the workspace, no lookup" "$(requests)" "POST /cli/workspace/ensure-canonical-session project=$WS;"
assert_eq "launch: the answer as it came" "$out" '{"ok":true,"action":"spawn"}'
run ok workspace launch acme
assert_eq "launch: a name, resolved first" "$(requests)" "GET /cli/workspace/resolve project=$WS q=acme;POST /cli/workspace/ensure-canonical-session project=/srv/acme ws;"
run close workspace launch
assert_eq "launch: no answer, exit 1" "$rc" "1"
assert_eq "launch: no answer, sent once" "$(requests)" "POST /cli/workspace/ensure-canonical-session project=$WS;"
run ok workspace profile
assert_eq "profile: the answer" "$rc:$out" '0:{"display_name":"Acme"}'
run ok workspace update --field role --value "frontend lead"
assert_eq "update: the name, then the update" "$(requests)" "GET /cli/workspace/agent-display-name project=$WS;POST /cli/agent/update agent=Acme field=role project=$WS value=frontend lead;"
run none workspace update --field role --value lead
assert_eq "update: no name, the folder's" "$(requests)" "GET /cli/workspace/agent-display-name project=$WS;POST /cli/agent/update agent=ws field=role project=$WS value=lead;"
run ok workspace update --field agent_mode --value off
assert_eq "update: a DB column, exit 2" "$rc" "2"
assert_eq "update: a DB column, nothing sent" "$(requests)" ""
run ok workspace update --field role
assert_eq "update: no value, the usage" "$rc:$err" "1:Usage: k2 workspace update --field <f> --value <v>"

echo
echo "workspace_first_run: $pass passed, $fail failed"
[ "$fail" = 0 ]
