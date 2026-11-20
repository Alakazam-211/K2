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

# The stub reads plan.json on every request: {"<route>": [status, body]},
# or {"<route>": {"seq": [[status, body], ...]}} for a route asked more than
# once: the nth request gets the nth answer, the last one repeats. Status 0
# closes the connection with no answer; ["slow", body] answers body after
# 6 s; ["down", secs] closes it and then listens on nothing for secs. A
# route the plan does not name answers 404.
# "#passport": {"route", "old", "new", "from", "again", "card"} plays a
# daemon that lost its passport store: from the route's "from"-th request
# on (0 = the first), a request that presents "old" is refused with the
# passport refusal, and the first refusal writes "new" into the session's
# secrets dir (the adoption finishing meanwhile). With "again", "new" is
# refused too ("card": with a permission card). Refused requests do not
# count toward "seq".
cat >"$T/stub.py" <<'PY'
import http.server, json, os, sys, time
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
        route = self.path.split("?", 1)[0]
        entry = routes.get(route, [404, '{"error":"not found"}'])
        counts = {}
        if os.path.exists(plan + ".counts"):
            with open(plan + ".counts") as f:
                counts = json.load(f)
        pp = routes.get("#passport")
        if pp and route == pp["route"]:
            n = counts.get("#" + route, 0)
            counts["#" + route] = n + 1
            with open(plan + ".counts", "w") as f:
                json.dump(counts, f)
            bearer = (self.headers.get("Authorization") or "").removeprefix("Bearer ")
            if n >= pp.get("from", 0) and (bearer == pp["old"] or (bearer == pp["new"] and pp.get("again"))):
                if not os.path.exists(pp["secrets"] + "/.k2-passport"):
                    with open(pp["secrets"] + "/.k2-passport", "w") as f:
                        f.write(pp["new"] + "\n")
                err = {"error": "Invalid or missing auth token"}
                if bearer == pp["new"] and pp.get("card"):
                    err["permission"] = {"command": 'k2 permission request heartbeats.read --why "<what you need it for>"'}
                entry = [403, json.dumps(err)]
        if isinstance(entry, dict):
            n = counts.get(route, 0)
            counts[route] = n + 1
            with open(plan + ".counts", "w") as f:
                json.dump(counts, f)
            entry = entry["seq"][min(n, len(entry["seq"]) - 1)]
        status, body = entry
        if status == "down":  # close unanswered, then stop listening for body seconds
            DOWN[0] = float(body)
            return
        if status == "slow":  # answer only after 6 s (the CLI waits 5)
            time.sleep(6)
            status, body = 200, body
        if status == 0:
            return
        body = body.encode()
        try:
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except OSError:
            pass
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
DOWN = [0.0]
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
port = srv.server_address[1]
with open(portfile + ".tmp", "w") as f:
    f.write(str(port))
os.rename(portfile + ".tmp", portfile)
while True:
    srv.handle_request()
    if DOWN[0]:
        # Nothing listens on the port meanwhile: a connect is refused.
        srv.server_close()
        time.sleep(DOWN[0])
        DOWN[0] = 0.0
        srv = http.server.HTTPServer(("127.0.0.1", port), H)
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
HEALED="sess-wake-err.passport2"
mkdir -p "$T/secrets"
WS="$T/ws"
mkdir -p "$WS/.k2" "$HOME/.k2"
NOOP='{"status":"noop","reason":"no inbox work found"}'
# One item as the daemon's /cli/inbox/list writes it (k2_core::inbox::
# InboxItem, camelCase): a message that arrived and nobody filed yet.
ITEM='[{"id":"review-the-plan","filename":"review-the-plan.md","folder":"","title":"Review the plan","priority":"normal","created":"2026-11-18T09:00:00Z","source":"msg","from":"acme-site","bodyPreview":"Please read the plan and say what is missing.","summary":"Read the plan"}]'
ASK='k2 permission request heartbeats.read --why "<what you need it for>"'

plan() { rm -f "$T/plan.json.counts" "$T/secrets/.k2-passport"; printf '%s\n' "$1" >"$T/plan.json"; }
# js <text>: text as a JSON string, for a body inside a plan.
js() { python3 -c 'import json, sys; print(json.dumps(sys.argv[1]))' "$1"; }

# wake [port]: the command end to end with the session's port and passport.
wake() {
    : >"$T/requests.log"
    RC=0
    env K2_PROJECT_PATH="$WS" K2_PORT="${1:-$STUB_PORT}" K2_HOOK_TOKEN="$SESSION" K2_SECRETS_DIR="$T/secrets" \
        "$K2_CLI" heartbeat wake >"$T/out" 2>"$T/err" || RC=$?
}

# wake_fn <export the rule: 0|1>: cmd_heartbeat_wake taken from the bash
# source, run on its own in a clean env, as a test harness or a caller
# that skips the prelude would. Only the prelude exports _K2_PY_PROXY_RULE.
RULE_DEF="$(awk -v q="'" '$0 == "_K2_PY_PROXY_RULE=" q {on=1} on {print} on && $0 == q {exit}' "$CLI_BASH_SOURCE")"
case "$RULE_DEF" in *install_opener*) ;; *) echo "FAIL: no _K2_PY_PROXY_RULE in $CLI_BASH_SOURCE" >&2; exit 1 ;; esac
WAKE_DEF="$(awk '$0 ~ "^cmd_heartbeat_wake\\(\\) \\{" {on=1} on {print} on && /^}/ {exit}' "$CLI_BASH_SOURCE")"
[ -n "$WAKE_DEF" ] || { echo "FAIL: no cmd_heartbeat_wake in $CLI_BASH_SOURCE" >&2; exit 1; }
# The function's python3 passes the k5 hand-over helpers on (_K5_PY_HTTP
# and $(_k5_py_headers): "cli: the embedded Python's urlopen calls carry
# the hand-over headers"), so a run of the function alone defines them
# too, from the same source, as the prelude leaves them (with the Codex
# conversation headers _k5_py_headers adds).
K5_DEF="$(sed -n "/^_K5_PY_HTTP='\$/,/^'\$/p; /^_K5_NOUN_RE=/p; /^_K2_CODEX_ID_RE=/p" "$CLI_BASH_SOURCE")"
for _k5f in _k5_headers _k5_py_headers _cli_codex_bind_on _k2_codex_headers; do
    K5_DEF="$K5_DEF"$'\n'"$(awk -v n="$_k5f() {" '$0 == n {on=1} on {print} on && /^}/ {exit}' "$CLI_BASH_SOURCE")"
done
case "$K5_DEF" in *"def k5_request"*) ;; *) echo "FAIL: no _K5_PY_HTTP in $CLI_BASH_SOURCE" >&2; exit 1 ;; esac
case "$K5_DEF" in *"_K5_NOUN_RE="*"_k5_headers() {"*"_k5_py_headers() {"*) ;; *) echo "FAIL: no _K5_NOUN_RE, _k5_headers or _k5_py_headers in $CLI_BASH_SOURCE" >&2; exit 1 ;; esac
WAKE_DEF="$K5_DEF"$'\n'"$WAKE_DEF"
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
# The write's answer as the daemon gives it (terminal_routes::handle_write).
WROTE='"/cli/terminal/write": [200, "{\"success\":true}"]'
SCAN='"/cli/agents/running": [200, "[{\"terminalId\":\"agent-chat:p1:manager\",\"cwd\":\"/w\"}]"]'
RUNNING="$SCAN, $WROTE"
# told <case>: the running manager got the triage message, once, and
# stdout is the notified line alone (the write's answer is not printed).
told() {
    if [ "$RC" = 0 ] && [ "$(cat "$T/out")" = '{"status":"notified","terminalId":"agent-chat:p1:manager"}' ] &&
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

echo "the manager scan and the launch poll: a failed read is an error, never asleep"
WORK='"/cli/agents/list": [200, "[{\"name\":\"a\",\"inboxCount\":1}]"], "/cli/inbox/list": [200, "[]"]'
BUSY='[500, "{\"error\":\"session map busy\"}"]'
posted() { # posted [route]: how many POSTs (of route) the stub saw
    grep -c "^POST ${1:-}" "$T/requests.log" || true
}
# err_like <case> <exit> <glob>: that exit, one stderr line matching glob, no noop.
err_like() {
    # shellcheck disable=SC2254
    case "$(cat "$T/err")" in
        $3) if [ "$RC" = "$2" ] && [ "$(wc -l <"$T/err" | tr -d ' ')" = 1 ] && ! grep -qF '"status":"noop"' "$T/out"; then
                ok "$1"
            else
                bad "$1 (want exit $2 and stderr like: $3)"
            fi ;;
        *) bad "$1 (want exit $2 and stderr like: $3)" ;;
    esac
}
# never_launched <case>: nothing was posted (no launch, no write).
never_launched() {
    if [ "$(posted)" != 0 ]; then
        bad "$1: it posted"
        return 1
    fi
}
# poll_stopped <case>: the launch was posted once, then nothing else, and
# no launched line.
poll_stopped() {
    if [ "$(posted)" != 1 ] || [ "$(posted /cli/agents/launch?)" != 1 ] || grep -qF '"status":"launched' "$T/out"; then
        bad "$1: want one POST (the launch) and no launched line"
        return 1
    fi
}
CLOSED="Error: k2 heartbeat wake: K2's server closed the connection on /cli/agents/running: *; run k2 daemon status"

plan '{'"$WORK"', "/cli/agents/running": '"$BUSY"'}'
wake
never_launched "the scan answers 500" && is_error "the scan answers 500" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/agents/running with HTTP 500: session map busy"
plan '{'"$WORK"', "/cli/agents/running": [200, "<html>not json</html>"]}'
wake
never_launched "the scan answers something that is not JSON" && is_error "the scan answers something that is not JSON" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/agents/running with something that is not JSON"
plan '{'"$WORK"', "/cli/agents/running": [0, ""]}'
wake
never_launched "the scan gets no answer" && err_like "the scan gets no answer" 1 "$CLOSED"
plan '{'"$WORK"', "/cli/agents/running": [200, "{\"sessions\":[]}"]}'
wake
never_launched "the scan answers an object" && is_error "the scan answers an object" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/agents/running with a dict, not a list of sessions"
plan '{'"$WORK"', "/cli/agents/running": [403, "{\"error\":\"not allowed: read sessions\",\"permission\":{\"command\":\"k2 permission request heartbeats.read --why \\\"<what you need it for>\\\"\"}}"]}'
wake
never_launched "the scan is refused with a card" && is_error "the scan is refused with a card" 3 \
    "Error: k2 heartbeat wake: K2's server refused /cli/agents/running with HTTP 403: not allowed: read sessions" \
    "Ask with: $ASK"

# The launch's answer as the daemon gives it (agents_routes::handle_agents_launch).
LAUNCHED='{"success":true,"terminalId":"7d3f9a52-1c4e-4b8a-9f21-5e6d7c8b9a01","agentName":"manager","pendingDrained":0,"note":"Agent session launched by daemon"}'
LAUNCH='"/cli/agents/launch": [200, '"$(js "$LAUNCHED")"'], '"$WROTE"
plan '{'"$WORK"', '"$LAUNCH"', "/cli/agents/running": {"seq": [[200, "[]"], '"$BUSY"']}}'
wake
poll_stopped "the launch poll answers 500" && is_error "the launch poll answers 500" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/agents/running with HTTP 500: session map busy"
# A poll that fails for good (a refusal, an error status, an answer that
# is not JSON) stops the wait at once.
plan '{'"$WORK"', '"$LAUNCH"', "/cli/agents/running": {"seq": [[200, "[]"], [403, "{\"error\":\"not allowed: read sessions\",\"permission\":{\"command\":\"k2 permission request heartbeats.read --why \\\"<what you need it for>\\\"\"}}"]]}}'
wake
if poll_stopped "the launch poll is refused"; then
    if [ "$(gets /cli/agents/running)" = 2 ]; then
        is_error "the launch poll is refused: no more polls" 3 \
        "Error: k2 heartbeat wake: K2's server refused /cli/agents/running with HTTP 403: not allowed: read sessions" "Ask with: $ASK"
    else
        bad "the launch poll is refused: no more polls: want 2 reads of /cli/agents/running"
    fi
fi
plan '{'"$WORK"', '"$LAUNCH"', "/cli/agents/running": {"seq": [[200, "[]"], [404, "{\"error\":\"not found\"}"]]}}'
wake
if poll_stopped "the launch poll answers 404"; then
    if [ "$(gets /cli/agents/running)" = 2 ]; then
        is_error "the launch poll answers 404: no more polls" 1 \
        "Error: k2 heartbeat wake: K2's server answered /cli/agents/running with HTTP 404: not found"
    else
        bad "the launch poll answers 404: no more polls: want 2 reads of /cli/agents/running"
    fi
fi
plan '{'"$WORK"', '"$LAUNCH"', "/cli/agents/running": {"seq": [[200, "[]"], [200, "<html>not json</html>"]]}}'
wake
if poll_stopped "the launch poll answers something that is not JSON"; then
    if [ "$(gets /cli/agents/running)" = 2 ]; then
        is_error "the launch poll answers something that is not JSON: no more polls" 1 \
        "Error: k2 heartbeat wake: K2's server answered /cli/agents/running with something that is not JSON"
    else
        bad "the launch poll answers something that is not JSON: no more polls: want 2 reads of /cli/agents/running"
    fi
fi
# A poll the daemon doesn't answer just now is tried again: a connection
# closed unanswered, nothing listening (the stub is down for 7 s), a
# timeout and the 503 of a daemon finishing its migrations, then the
# manager is up and told.
MIGRATING='[503, "{\"state\":\"migrating\",\"error\":\"daemon is completing first-boot migrations\"}"]'
plan '{'"$WORK"', '"$LAUNCH"', "/cli/agents/running": {"seq": [[200, "[]"], [0, ""], ["down", "7"], ["slow", "[]"], '"$MIGRATING"', [200, "[{\"terminalId\":\"agent-chat-manager\",\"cwd\":\"/w/.k2/agents/manager\"}]"]]}}'
wake
if [ "$RC" = 0 ] && [ ! -s "$T/err" ] &&
    [ "$(cat "$T/out")" = '{"status":"launched_and_notified","terminalId":"agent-chat-manager"}' ] &&
    [ "$(gets /cli/agents/running)" = 6 ] && [ "$(posted /cli/agents/launch?)" = 1 ] && [ "$(posted /cli/terminal/write?)" = 1 ]; then
    ok "the launch poll: no answer, refused, a timeout and a 503 while migrating are tried again"
else
    bad "the launch poll: no answer, refused, a timeout and a 503 while migrating are tried again"
fi
# Still not answering at the last poll: the wake fails with that reason.
plan '{'"$WORK"', '"$LAUNCH"', "/cli/agents/running": {"seq": [[200, "[]"], '"$MIGRATING"']}}'
wake
if poll_stopped "the launch poll never gets an answer"; then
    if [ "$(gets /cli/agents/running)" = 13 ]; then
        is_error "the launch poll never gets an answer: twelve polls, then the last reason" 1 \
        "Error: k2 heartbeat wake: the manager was launched, but K2 did not answer the last of 12 polls for its terminal (60 s): K2's server answered /cli/agents/running with HTTP 503: daemon is completing first-boot migrations; run k2 daemon status"
    else
        bad "the launch poll never gets an answer: twelve polls, then the last reason: want 13 reads of /cli/agents/running"
    fi
fi
plan '{'"$WORK"', '"$LAUNCH"', "/cli/agents/running": {"seq": [[200, "[]"], [200, "[{\"terminalId\":\"agent-chat-manager\",\"cwd\":\"/w/.k2/agents/manager\"}]"]]}}'
wake
if [ "$RC" = 0 ] && [ ! -s "$T/err" ] &&
    [ "$(cat "$T/out")" = '{"status":"launched_and_notified","terminalId":"agent-chat-manager"}' ] &&
    [ "$(gets /cli/agents/running)" = 2 ] && [ "$(posted /cli/agents/launch?)" = 1 ] &&
    [ "$(posted /cli/terminal/write?)" = 1 ] && grep -qF 'POST /cli/agents/launch?project=' "$T/requests.log" &&
    grep -qF '&agent=manager ' "$T/requests.log" && grep -qF 'id=agent-chat-manager&message=New%20work%20detected' "$T/requests.log"; then
    ok "no manager running: it is launched, then told once its terminal is up"
else
    bad "no manager running: it is launched, then told once its terminal is up"
fi

echo "the write and the launch: a failed one is an error, never notified or launched"
# not_told <case>: no notified or launched line on stdout.
not_told() {
    if grep -qF '"status":"notified"' "$T/out" || grep -qF '"status":"launched' "$T/out"; then
        bad "$1: it printed a notified or launched line"
        return 1
    fi
}
# <case>|<status>|<body, escaped for the plan>|<the line after "K2's server ">
for c in 'the write answers 500|500|{\"error\":\"pty write failed: broken pipe\"}|answered /cli/terminal/write with HTTP 500: pty write failed: broken pipe' \
    'the write answers 400 (no such session)|400|{\"error\":\"session not found (checked legacy + v2 maps)\"}|answered /cli/terminal/write with HTTP 400: session not found (checked legacy + v2 maps)' \
    'the write answers something that is not JSON|200|<html>not json</html>|answered /cli/terminal/write with something that is not JSON' \
    'the write answers 200 with an error in it|200|{\"error\":\"pty closed\"}|answered /cli/terminal/write with HTTP 200: pty closed'; do
    IFS='|' read -r name st body want <<<"$c"
    plan '{'"$WORK"', '"$SCAN"', "/cli/terminal/write": ['"$st"', "'"$body"'"]}'
    wake
    not_told "$name" && is_error "$name" 1 "Error: k2 heartbeat wake: K2's server $want"
done
plan '{'"$WORK"', '"$SCAN"', "/cli/terminal/write": [0, ""]}'
wake
not_told "the write gets no answer" && err_like "the write gets no answer" 1 \
    "Error: k2 heartbeat wake: K2's server closed the connection on /cli/terminal/write: *; run k2 daemon status"
plan '{'"$WORK"', '"$SCAN"', "/cli/terminal/write": [403, "{\"error\":\"not allowed: write to a terminal\",\"permission\":{\"command\":\"k2 permission request heartbeats.read --why \\\"<what you need it for>\\\"\"}}"]}'
wake
not_told "the write is refused with a card" && is_error "the write is refused with a card" 3 \
    "Error: k2 heartbeat wake: K2's server refused /cli/terminal/write with HTTP 403: not allowed: write to a terminal" \
    "Ask with: $ASK"
plan '{'"$WORK"', "/cli/agents/running": [200, "[]"], "/cli/agents/launch": [400, "{\"error\":\"build_launch failed: no manager agent\"}"]}'
wake
if not_told "the launch answers 400: no poll"; then
    if [ "$(gets /cli/agents/running)" = 1 ] && [ "$(posted)" = 1 ]; then
        is_error "the launch answers 400: no poll" 1 \
            "Error: k2 heartbeat wake: K2's server answered /cli/agents/launch with HTTP 400: build_launch failed: no manager agent"
    else
        bad "the launch answers 400: no poll (it polled or posted again)"
    fi
fi
plan '{'"$WORK"', "/cli/agents/running": [200, "[]"], "/cli/agents/launch": [0, ""]}'
wake
if not_told "the launch gets no answer: no poll"; then
    if [ "$(gets /cli/agents/running)" = 1 ]; then
        err_like "the launch gets no answer: no poll" 1 \
            "Error: k2 heartbeat wake: K2's server closed the connection on /cli/agents/launch: *; run k2 daemon status"
    else
        bad "the launch gets no answer: no poll (it polled)"
    fi
fi
plan '{'"$WORK"', "/cli/agents/launch": [200, '"$(js "$LAUNCHED")"'], "/cli/terminal/write": [500, "{\"error\":\"pty write failed: broken pipe\"}"], "/cli/agents/running": {"seq": [[200, "[]"], [200, "[{\"terminalId\":\"agent-chat-manager\",\"cwd\":\"/w/.k2/agents/manager\"}]"]]}}'
wake
not_told "launched, then the write fails" && is_error "launched, then the write fails" 1 \
    "Error: k2 heartbeat wake: K2's server answered /cli/terminal/write with HTTP 500: pty write failed: broken pipe"

echo "a stale passport heals once and retries, as the CLI's other senders do"
# pp <route> <from> [again] [card] [new]: the "#passport" plan entry.
pp() {
    printf '"#passport": {"route": "%s", "from": %s, "old": "%s", "new": "%s", "secrets": "%s", "again": %s, "card": %s}' \
        "$1" "$2" "$SESSION" "${5:-$HEALED}" "$T/secrets" "${3:-false}" "${4:-false}"
}
# auths <method> <route>: the passports that route was sent with, in order.
auths() {
    grep "^$1 $2?" "$T/requests.log" | sed 's/.* auth=Bearer //' | tr '\n' ' ' | sed 's/ $//'
}
# healed_ok <case> <method> <route> <want stdout tail>: exit 0, nothing on
# stderr, that tail line, and route sent twice: refused, then the retry
# with the new passport. Every request after the heal uses it.
healed_ok() {
    if [ "$RC" = 0 ] && [ ! -s "$T/err" ] && [ "$(cat "$T/out")" = "$4" ] &&
        [ "$(auths "$2" "$3")" = "$SESSION $HEALED" -o "$(auths "$2" "$3")" = "$SESSION $SESSION $HEALED" ] &&
        [ "$(awk '/auth=Bearer '"$HEALED"'$/ {seen = 1; next} seen && /auth=Bearer '"$SESSION"'$/ {bad = 1} END {print bad + 0}' "$T/requests.log")" = 0 ]; then
        ok "$1"
    else
        bad "$1"
    fi
}
TOLD='{"status":"notified","terminalId":"agent-chat:p1:manager"}'
UP='"/cli/agents/running": {"seq": [[200, "[]"], [200, "[{\"terminalId\":\"agent-chat-manager\",\"cwd\":\"/w/.k2/agents/manager\"}]"]]}'
UP_TOLD='{"status":"launched_and_notified","terminalId":"agent-chat-manager"}'

plan '{'"$WORK"', '"$RUNNING"', '"$(pp /cli/agents/running 0)"'}'
wake
healed_ok "the scan: refused, healed, retried once" GET /cli/agents/running "$TOLD"
plan '{'"$WORK"', '"$LAUNCH"', '"$UP"', '"$(pp /cli/agents/running 1)"'}'
wake
healed_ok "the poll: refused, healed, retried once" GET /cli/agents/running "$UP_TOLD"
[ "$(auths GET /cli/agents/running)" = "$SESSION $SESSION $HEALED" ] || bad "the poll: want the scan, the refused poll and its retry"
plan '{'"$WORK"', '"$LAUNCH"', '"$UP"', '"$(pp /cli/agents/launch 0)"'}'
wake
healed_ok "the launch: refused, healed, retried once" POST /cli/agents/launch "$UP_TOLD"
plan '{'"$WORK"', '"$RUNNING"', '"$(pp /cli/terminal/write 0)"'}'
wake
healed_ok "the write to the running manager: refused, healed, retried once" POST /cli/terminal/write "$TOLD"
plan '{'"$WORK"', '"$LAUNCH"', '"$UP"', '"$(pp /cli/terminal/write 0)"'}'
wake
healed_ok "the write after a launch: refused, healed, retried once" POST /cli/terminal/write "$UP_TOLD"

REFUSED="Error: k2 heartbeat wake: K2's server refused"
plan '{'"$WORK"', '"$RUNNING"', '"$(pp /cli/agents/running 0 true true)"'}'
wake
if [ "$(auths GET /cli/agents/running)" = "$SESSION $HEALED" ] && never_launched "the scan refused again after the heal"; then
    is_error "the scan refused again after the heal (a card)" 3 \
        "$REFUSED /cli/agents/running with HTTP 403: Invalid or missing auth token" "Ask with: $ASK"
else
    bad "the scan refused again after the heal (a card): want one retry and no post"
fi
plan '{'"$WORK"', '"$LAUNCH"', '"$UP"', '"$(pp /cli/agents/launch 0 true)"'}'
wake
if [ "$(auths POST /cli/agents/launch)" = "$SESSION $HEALED" ] && [ "$(gets /cli/agents/running)" = 1 ]; then
    is_error "the launch refused again after the heal: no poll" 1 \
        "$REFUSED /cli/agents/launch with HTTP 403: Invalid or missing auth token"
else
    bad "the launch refused again after the heal: want one retry and no poll"
fi
plan '{'"$WORK"', '"$RUNNING"', '"$(pp /cli/terminal/write 0 true)"'}'
wake
if [ "$(auths POST /cli/terminal/write)" = "$SESSION $HEALED" ] && not_told "the write refused again after the heal"; then
    is_error "the write refused again after the heal" 1 \
        "$REFUSED /cli/terminal/write with HTTP 403: Invalid or missing auth token"
else
    bad "the write refused again after the heal: want one retry and no notified line"
fi
# Send once: only a refusal is sent again. The retried write gets no
# answer; it may have gone out, so it is never sent a third time.
plan '{'"$WORK"', '"$SCAN"', "/cli/terminal/write": [0, ""], '"$(pp /cli/terminal/write 0)"'}'
wake
if [ "$(auths POST /cli/terminal/write)" = "$SESSION $HEALED" ] && not_told "a retried write that gets no answer"; then
    err_like "a retried write that gets no answer is not sent again" 1 \
        "Error: k2 heartbeat wake: K2's server closed the connection on /cli/terminal/write: *; run k2 daemon status"
else
    bad "a retried write that gets no answer is not sent again"
fi
plan '{'"$WORK"', '"$SCAN"', "/cli/terminal/write": [0, ""]}'
printf '%s\n' "$HEALED" >"$T/secrets/.k2-passport"
wake
if [ "$(auths POST /cli/terminal/write)" = "$HEALED" ] && not_told "a write with no answer"; then
    err_like "a write with no answer is not sent again, a new passport on disk or not" 1 \
        "Error: k2 heartbeat wake: K2's server closed the connection on /cli/terminal/write: *; run k2 daemon status"
else
    bad "a write with no answer is not sent again, a new passport on disk or not"
fi
plan '{'"$WORK"', '"$RUNNING"', '"$(pp /cli/agents/running 0 false false sess-other.passport9)"'}'
wake
if [ "$(auths GET /cli/agents/running)" = "$SESSION" ]; then
    is_error "another session's passport on disk is never used" 1 \
        "$REFUSED /cli/agents/running with HTTP 403: Invalid or missing auth token"
else
    bad "another session's passport on disk is never used"
fi

echo "paths with a quote and a space: the wake works"
# The workspace and the K2 home reach the wake's Python through the
# environment. Pasted into its source, a ' or " in either path ended the
# string and the Python never ran. Both paths here hold ', " and a space.
# No K2_PORT and no passport: the port and the owner token come from the
# K2 home, so the home path is used too.
QHOME="$T/it's a \"k2\" home"
QWS="$T/the \"plan\" it's in"
mkdir -p "$QHOME/.k2" "$QWS/.k2"
printf '%s' "$STUB_PORT" >"$QHOME/.k2/heartbeat.port"
printf '%s' "owner-token-quoted-home" >"$QHOME/.k2/heartbeat.token"
QPROJ="$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$QWS")"
# quoted_ok <case>: noop, both reads with the quoted workspace and the
# home's owner token.
quoted_ok() {
    if [ "$RC" = 0 ] && [ "$(cat "$T/out")" = "$NOOP" ] && [ ! -s "$T/err" ] &&
        grep -qxF "GET /cli/agents/list?project=$QPROJ auth=Bearer owner-token-quoted-home" "$T/requests.log" &&
        grep -qxF "GET /cli/inbox/list?project=$QPROJ&folder= auth=Bearer owner-token-quoted-home" "$T/requests.log"; then
        ok "$1"
    else
        bad "$1"
    fi
}
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [200, "[]"]}'
: >"$T/requests.log"
RC=0
env -i PATH="$PATH" HOME="$QHOME" K2_HOME="$QHOME/.k2" PROJECT="$QWS" \
    DEFS="$RULE_DEF"$'\n'"export _K2_PY_PROXY_RULE"$'\n'"$WAKE_DEF" \
    bash -c 'set -euo pipefail; eval "$DEFS"; cmd_heartbeat_wake' >"$T/out" 2>"$T/err" || RC=$?
quoted_ok "the function alone, quoted workspace and K2 home"
: >"$T/requests.log"
RC=0
env -u K2_PORT -u K2_HOOK_TOKEN HOME="$QHOME" K2_PROJECT_PATH="$QWS" \
    "$K2_CLI" heartbeat wake >"$T/out" 2>"$T/err" || RC=$?
quoted_ok "end to end, quoted workspace and HOME"
plan '{"/cli/agents/list": [200, "[]"], "/cli/inbox/list": [200, '"$(js "$ITEM")"'], '"$RUNNING"'}'
: >"$T/requests.log"
RC=0
env -u K2_PORT -u K2_HOOK_TOKEN HOME="$QHOME" K2_PROJECT_PATH="$QWS" \
    "$K2_CLI" heartbeat wake >"$T/out" 2>"$T/err" || RC=$?
if [ "$RC" = 0 ] && [ ! -s "$T/err" ] &&
    [ "$(cat "$T/out")" = '{"status":"notified","terminalId":"agent-chat:p1:manager"}' ] &&
    grep -qF "POST /cli/terminal/write?project=$QPROJ&id=agent-chat%3Ap1%3Amanager&message=" "$T/requests.log"; then
    ok "end to end, quoted paths: the manager is told"
else
    bad "end to end, quoted paths: the manager is told"
fi

if [ "${#failed[@]}" -gt 0 ]; then
    echo "heartbeat_wake_errors: ${#failed[@]} failed, $pass passed: ${failed[*]}" >&2
    exit 1
fi
echo "heartbeat_wake_errors: $pass passed"
