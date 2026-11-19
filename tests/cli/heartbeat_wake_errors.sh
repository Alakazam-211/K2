#!/usr/bin/env bash
# `k2 heartbeat wake` prints its "noop" line only when the daemon answered
# and there is no inbox work: no agent with an inbox count above 0 and
# nothing at the root of the workspace inbox (/cli/inbox/list with
# folder=, answered as the daemon's InboxItem list). Any other outcome of
# the inbox check is an error: one line on stderr naming the reason and a
# non-zero exit, 3 for a refusal that carries a permission card (it ends
# with its Ask with: line or "It can't be requested."), 1 for the rest.
# Before, the check's Python sent its stderr to /dev/null and turned every
# failure into "noop", so a Python that stopped before its first request
# (no _K2_PY_PROXY_RULE), a daemon error or an answer that is not JSON all
# read as nothing to do.
#
# A stub daemon answers each route as the case says and logs every request.
# Every case runs; the test fails at the end naming each case that broke.
# No real daemon, no real home.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

T="$(cd "$(mktemp -d -t k2-hb-wake-err-XXXXXX)" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$T"
    hermetic_cli_cleanup
}
trap cleanup EXIT
pass=0
failed=()
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() {
    echo "  FAIL: $1" >&2
    echo "    exit: $RC" >&2
    sed 's/^/    stdout: /' "$T/out" >&2
    sed 's/^/    stderr: /' "$T/err" >&2
    sed 's/^/    request: /' "$T/requests.log" >&2
    failed+=("$1")
}

# The stub reads plan.json on every request: {"<route>": [status, body]}.
# A route the plan does not name answers 404.
cat >"$T/stub.py" <<'PY'
import http.server, json, os, sys
log, plan, portfile = sys.argv[1], sys.argv[2], sys.argv[3]
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        with open(log, "a") as f:
            f.write("%s %s auth=%s\n" % (self.command, self.path, self.headers.get("Authorization") or ""))
        with open(plan) as f:
            routes = json.load(f)
        status, body = routes.get(self.path.split("?", 1)[0], [404, '{"error":"not found"}'])
        body = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
with open(portfile + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(portfile + ".tmp", portfile)
srv.serve_forever()
PY
echo '{}' >"$T/plan.json"
: >"$T/requests.log"
python3 -I "$T/stub.py" "$T/requests.log" "$T/plan.json" "$T/stub.port" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$T/stub.port" ] && break; sleep 0.05; done
if [ ! -s "$T/stub.port" ]; then echo "FAIL: the stub daemon did not start" >&2; exit 1; fi
STUB_PORT="$(cat "$T/stub.port")"
DEAD_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"

SESSION="sess-wake-err.passport1"
WS="$T/ws"
mkdir -p "$WS/.k2" "$HOME/.k2"
NOOP='{"status":"noop","reason":"no inbox work found"}'
# One item as the daemon's /cli/inbox/list writes it (k2_core::inbox::
# InboxItem, camelCase): a message that arrived and nobody filed yet.
ITEM='[{"id":"review-the-plan","filename":"review-the-plan.md","folder":"","title":"Review the plan","priority":"normal","created":"2026-11-18T09:00:00Z","source":"msg","from":"acme-site","bodyPreview":"Please read the plan and say what is missing.","summary":"Read the plan"}]'
ASK='k2 permission request heartbeats.read --why "<what you need it for>"'

plan() { printf '%s\n' "$1" >"$T/plan.json"; }
# js <text>: text as a JSON string, for a body inside a plan.
js() { python3 -c 'import json, sys; print(json.dumps(sys.argv[1]))' "$1"; }

# wake [port]: the command end to end with the session's port and passport.
wake() {
    : >"$T/requests.log"
    RC=0
    env K2_PROJECT_PATH="$WS" K2_PORT="${1:-$STUB_PORT}" K2_HOOK_TOKEN="$SESSION" \
        "$K2_CLI" heartbeat wake >"$T/out" 2>"$T/err" || RC=$?
}

# wake_fn <export the rule: 0|1>: cmd_heartbeat_wake taken from the bash
# source, run on its own in a clean env, as a test harness or a caller
# that skips the prelude would. Only the prelude exports _K2_PY_PROXY_RULE.
RULE_DEF="$(awk -v q="'" '$0 == "_K2_PY_PROXY_RULE=" q {on=1} on {print} on && $0 == q {exit}' "$CLI_BASH_SOURCE")"
case "$RULE_DEF" in *install_opener*) ;; *) echo "FAIL: no _K2_PY_PROXY_RULE in $CLI_BASH_SOURCE" >&2; exit 1 ;; esac
WAKE_DEF="$(awk '$0 ~ "^cmd_heartbeat_wake\\(\\) \\{" {on=1} on {print} on && /^}/ {exit}' "$CLI_BASH_SOURCE")"
[ -n "$WAKE_DEF" ] || { echo "FAIL: no cmd_heartbeat_wake in $CLI_BASH_SOURCE" >&2; exit 1; }
wake_fn() {
    local defs="$WAKE_DEF"
    [ "$1" = 1 ] && defs="$RULE_DEF"$'\n'"export _K2_PY_PROXY_RULE"$'\n'"$defs"
    : >"$T/requests.log"
    RC=0
    env -i PATH="$PATH" HOME="$HOME" K2_HOME="$HOME/.k2" PROJECT="$WS" \
        K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SESSION" DEFS="$defs" \
        bash -c 'set -euo pipefail; eval "$DEFS"; cmd_heartbeat_wake' >"$T/out" 2>"$T/err" || RC=$?
}

gets() { # gets <route>: how many GETs of route the stub saw
    grep -cF "GET $1?project=" "$T/requests.log" || true
}
# root_asked: the inbox read asked for the root only (folder= present and
# empty), and the retired /cli/work/inbox was not asked.
root_asked() {
    [ "$(grep -c '^GET /cli/inbox/list?project=[^& ]*&folder= ' "$T/requests.log" || true)" = 1 ] &&
        [ "$(grep -cF '/cli/work/inbox' "$T/requests.log" || true)" = 0 ]
}

# is_noop <case>: exit 0, the noop line, nothing on stderr, each read once.
is_noop() {
    if [ "$RC" = 0 ] && [ "$(cat "$T/out")" = "$NOOP" ] && [ ! -s "$T/err" ] &&
        [ "$(gets /cli/agents/list)" = 1 ] && [ "$(gets /cli/inbox/list)" = 1 ] && root_asked; then
        ok "$1"
    else
        bad "$1"
    fi
}
# is_error <case> <exit> <first stderr line> [closing line]: that exit, no
# noop, that one line on stderr (plus the closing line when given), and
# /cli/agents/list asked at most once.
is_error() {
    local want_err="$3"
    [ $# -ge 4 ] && want_err+=$'\n'"$4"
    if [ "$RC" = "$2" ] && ! grep -qF '"status":"noop"' "$T/out" &&
        [ "$(cat "$T/err")" = "$want_err" ] && [ "$(gets /cli/agents/list)" -le 1 ]; then
        ok "$1"
    else
        bad "$1 (want exit $2 and stderr: $want_err)"
    fi
}

echo "nothing to do"
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [200, "[]"]}'
wake
is_noop "no agents, empty workspace inbox"
plan '{"/cli/agents/list": [200, "[{\"name\":\"a\",\"inboxCount\":0}]"], "/cli/inbox/list": [200, "[]"]}'
wake
is_noop "agents with empty inboxes, an empty workspace inbox"
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [200, "[]"]}'
wake_fn 1
is_noop "the function alone, with the proxy rule exported"

echo "work to do: the manager is told, as before"
RUNNING='"/cli/agents/running": [200, "[{\"terminalId\":\"agent-chat:p1:manager\",\"cwd\":\"/w\"}]"], "/cli/terminal/write": [200, "{\"ok\":true}"]'
# told <case>: the running manager got the triage message, once.
told() {
    if [ "$RC" = 0 ] && [ "$(tail -n 1 "$T/out")" = '{"ok":true}{"status":"notified","terminalId":"agent-chat:p1:manager"}' ] &&
        [ ! -s "$T/err" ] && [ "$(gets /cli/agents/list)" = 1 ] && [ "$(gets /cli/inbox/list)" = 1 ] && root_asked &&
        [ "$(gets /cli/agents/running)" = 1 ] &&
        [ "$(grep -cF 'POST /cli/terminal/write?' "$T/requests.log" || true)" = 1 ] &&
        grep -qF 'id=agent-chat%3Ap1%3Amanager&message=New%20work%20detected' "$T/requests.log"; then
        ok "$1"
    else
        bad "$1"
    fi
}
plan '{"/cli/agents/list": [200, "[{\"name\":\"a\",\"inboxCount\":2}]"], "/cli/inbox/list": [200, "[]"], '"$RUNNING"'}'
wake
told "an agent's inbox has work: the running manager gets the triage message"
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [200, '"$(js "$ITEM")"'], '"$RUNNING"'}'
wake
told "an item at the workspace inbox's root: the running manager gets the triage message"
# A k2.db from before migration 0039 (agent_sessions became
# workspace_sessions there) that names another terminal as the running
# manager: the wake never reads it, the daemon's live sessions decide.
python3 - "$HOME/.k2/k2.db" "$WS" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute("CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL)")
c.execute("CREATE TABLE agent_sessions (project_id TEXT, agent_name TEXT, terminal_id TEXT, status TEXT)")
c.execute("INSERT INTO projects VALUES ('p1', ?)", (sys.argv[2],))
c.execute("INSERT INTO agent_sessions VALUES ('p1', 'manager', 'agent-chat:p1:stale', 'running')")
c.commit()
PY
plan '{"/cli/agents/list": [200, "[{\"name\":\"a\",\"inboxCount\":1}]"], "/cli/inbox/list": [200, "[]"], '"$RUNNING"'}'
wake
rm -f "$HOME/.k2/k2.db"
told "a leftover agent_sessions row is never read: the live sessions name the manager"

echo "errors say why"
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [200, "[]"]}'
wake_fn 0
is_error "the function alone, no proxy rule" 1 \
    "Error: k2 heartbeat wake: _K2_PY_PROXY_RULE is not set (the k2 prelude exports it)"
plan '{"/cli/agents/list": [500, "{\"error\":\"database is locked\"}"], "/cli/inbox/list": [200, "[]"]}'
wake
is_error "the daemon answers 500" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/agents/list with HTTP 500: database is locked"
plan '{"/cli/agents/list": [200, "<html>not json</html>"], "/cli/inbox/list": [200, "[]"]}'
wake
is_error "the daemon answers 200 with something that is not JSON" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/agents/list with something that is not JSON"
plan '{"/cli/agents/list": [200, "{\"ok\":true}"], "/cli/inbox/list": [200, "[]"]}'
wake
is_error "the daemon answers an object where the agents list goes" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/agents/list with a dict, not a list of agents"
plan '{"/cli/agents/list": [403, "{\"error\":\"not allowed: read heartbeats\",\"permission\":{\"command\":\"k2 permission request heartbeats.read --why \\\"<what you need it for>\\\"\"}}"], "/cli/inbox/list": [200, "[]"]}'
wake
is_error "a refusal with a permission card" 3 \
    "Error: k2 heartbeat wake: K2's server refused /cli/agents/list with HTTP 403: not allowed: read heartbeats" \
    "Ask with: $ASK"
plan '{"/cli/agents/list": [403, "{\"error\":{\"code\":\"people_only\",\"hint\":\"only a person can do this\"},\"permission\":{\"requestable\":false,\"hint\":\"Ask your person to run it.\"}}"], "/cli/inbox/list": [200, "[]"]}'
wake
is_error "a refusal that can't be requested" 3 \
    "Error: k2 heartbeat wake: K2's server refused /cli/agents/list with HTTP 403: only a person can do this" \
    $'Ask your person to run it.\nIt can\'t be requested.'
plan '{"/cli/agents/list": [403, "{\"error\":\"forbidden\"}"], "/cli/inbox/list": [200, "[]"]}'
wake
is_error "a refusal with no permission card" 1 \
    "Error: k2 heartbeat wake: K2's server refused /cli/agents/list with HTTP 403: forbidden"
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [500, "{\"error\":\"inbox store unreadable\"}"]}'
wake
is_error "the workspace inbox answers 500" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/inbox/list with HTTP 500: inbox store unreadable"
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [410, "{\"error\":\"gone\"}"]}'
wake
is_error "a 410 on the workspace inbox is an error too (no retired-route allowance)" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/inbox/list with HTTP 410: gone"
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [200, "{\"items\":[]}"]}'
wake
is_error "the workspace inbox answers an object, not a list" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/inbox/list with a dict, not a list of inbox items"
wake "$DEAD_PORT"
case "$(cat "$T/err")" in
    "Error: k2 heartbeat wake: can't reach K2's server at http://127.0.0.1:$DEAD_PORT: "*"; run k2 daemon status")
        if [ "$RC" = 1 ] && [ "$(wc -l <"$T/err" | tr -d ' ')" = 1 ] && ! grep -qF '"status":"noop"' "$T/out"; then
            ok "nothing listens on the port"
        else
            bad "nothing listens on the port"
        fi ;;
    *) bad "nothing listens on the port" ;;
esac

if [ "${#failed[@]}" -gt 0 ]; then
    echo "heartbeat_wake_errors: ${#failed[@]} failed, $pass passed: ${failed[*]}" >&2
    exit 1
fi
echo "heartbeat_wake_errors: $pass passed"
