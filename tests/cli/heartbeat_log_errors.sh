#!/usr/bin/env bash
# `k2 heartbeat log` prints the daemon's rows as a table, or "No heartbeat
# activity recorded yet." for an empty list. Any other answer is an error:
# one line on stderr naming it and a non-zero exit, 3 for a refusal that
# carries a permission card (the exit trap adds its Ask with: line, or the
# hint and "It can't be requested."), 1 for the rest. Before, its reader
# sent its stderr to /dev/null and fell back to `cat`, so a 500, a refusal
# or an answer that is not JSON printed the raw body, or nothing at all,
# and exited 0.
#
# A stub daemon answers /cli/heartbeat-log as the case says and logs every
# request. Every case runs; the test fails at the end naming each case
# that broke. No real daemon, no real home.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

T="$(cd "$(mktemp -d -t k2-hb-log-err-XXXXXX)" && pwd -P)"
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

# The stub answers /cli/heartbeat-log with answer.json: [status, body].
cat >"$T/stub.py" <<'PY'
import http.server, json, os, sys
log, answer, portfile = sys.argv[1], sys.argv[2], sys.argv[3]
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        with open(log, "a") as f:
            f.write("%s %s auth=%s\n" % (self.command, self.path, self.headers.get("Authorization") or ""))
        if self.path.split("?", 1)[0] == "/cli/heartbeat-log":
            with open(answer) as f:
                status, body = json.load(f)
        else:
            status, body = 404, '{"error":"not found"}'
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
echo '[200, "[]"]' >"$T/answer.json"
: >"$T/requests.log"
python3 -I "$T/stub.py" "$T/requests.log" "$T/answer.json" "$T/stub.port" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$T/stub.port" ] && break; sleep 0.05; done
if [ ! -s "$T/stub.port" ]; then echo "FAIL: the stub daemon did not start" >&2; exit 1; fi
STUB_PORT="$(cat "$T/stub.port")"
DEAD_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"

WS="$T/ws"
mkdir -p "$WS/.k2" "$HOME/.k2"
ASK='k2 permission request heartbeats.read --why "<what you need it for>"'

# answer <status> <body>: what the stub answers next.
answer() { python3 -c 'import json, sys; print(json.dumps([int(sys.argv[1]), sys.argv[2]]))' "$1" "$2" >"$T/answer.json"; }

# log [port] [args...]: the command end to end with the session's port and passport.
log() {
    local port="${1:-$STUB_PORT}"
    [ $# -gt 0 ] && shift
    : >"$T/requests.log"
    RC=0
    env K2_PROJECT_PATH="$WS" K2_PORT="$port" K2_HOOK_TOKEN="sess-hb-log.passport1" \
        "$K2_CLI" heartbeat log "$@" >"$T/out" 2>"$T/err" || RC=$?
}

# is_error <case> <exit> <stderr>: that exit, nothing on stdout, exactly
# that stderr.
is_error() {
    if [ "$RC" = "$2" ] && [ ! -s "$T/out" ] && [ "$(cat "$T/err")" = "$3" ]; then
        ok "$1"
    else
        bad "$1 (want exit $2 and stderr: $3)"
    fi
}

echo "the rows, as before"
# Two rows as the daemon writes them (k2_core::db::schema::HeartbeatFire,
# camelCase): one fired by a heartbeat, one skipped at project level.
answer 200 '[{"id":12,"projectId":"p1","agentName":"manager","scheduleName":"triage","firedAt":"2026-11-18T09:00:05.123Z","mode":"heartbeat","decision":"fired","reason":"inbox has 2 items","inboxPriority":"high","inboxCount":2,"durationMs":840,"actor":null},{"id":11,"projectId":"p1","agentName":null,"scheduleName":null,"firedAt":"2026-11-18T08:00:00Z","mode":"heartbeat","decision":"skipped_quiet_hours","reason":null,"inboxPriority":null,"inboxCount":null,"durationMs":null,"actor":null}]'
log "$STUB_PORT" --limit 5
{
    printf '%-22s %-20s %-22s %-8s %s\n' "Fired At" "Agent" "Decision" "Inbox" "Reason"
    printf '%0100d\n' 0 | tr 0 -
    printf '%-22s %-20s %-22s %-8s %s\n' "2026-11-18 09:00:05" "manager" "fired" "2 high" "inbox has 2 items"
    printf '%-22s %-20s %-22s %-8s %s\n' "2026-11-18 08:00:00" "(project)" "skipped_quiet_hours" "0" ""
} >"$T/want"
if [ "$RC" = 0 ] && [ ! -s "$T/err" ] && cmp -s "$T/out" "$T/want" &&
    grep -q '^GET /cli/heartbeat-log?project=[^ ]*&limit=5 auth=Bearer sess-hb-log.passport1$' "$T/requests.log"; then
    ok "two rows print as the table, with the limit asked for"
else
    diff "$T/want" "$T/out" >&2 || true
    bad "two rows print as the table, with the limit asked for"
fi
answer 200 '[]'
log
if [ "$RC" = 0 ] && [ ! -s "$T/err" ] && [ "$(cat "$T/out")" = "No heartbeat activity recorded yet." ]; then
    ok "an empty list"
else
    bad "an empty list"
fi

echo "errors say why"
answer 500 '{"error":"database is locked"}'
log
is_error "the daemon answers 500" 1 \
    "Error: k2 heartbeat log: K2's server answered /cli/heartbeat-log with an error: database is locked"
answer 400 '{"error":"Project not found: /nowhere"}'
log
is_error "the daemon answers 400" 1 \
    "Error: k2 heartbeat log: K2's server answered /cli/heartbeat-log with an error: Project not found: /nowhere"
answer 200 '<html>not json</html>'
log
is_error "the daemon answers 200 with something that is not JSON" 1 \
    "Error: k2 heartbeat log: K2's server answered /cli/heartbeat-log with something that is not JSON"
answer 200 '{"rows":[]}'
log
is_error "the daemon answers an object where the rows go" 1 \
    "Error: k2 heartbeat log: K2's server answered /cli/heartbeat-log with a dict, not a list of heartbeat rows"
answer 403 '{"error":"not allowed: read heartbeats","permission":{"command":"k2 permission request heartbeats.read --why \"<what you need it for>\""}}'
log
is_error "a refusal with a permission card" 3 \
    "Error: k2 heartbeat log: K2's server refused /cli/heartbeat-log: not allowed: read heartbeats"$'\n'"Ask with: $ASK"
# 0.46.18: with no card, the refusal's last line is how to send feedback.
NO_CARD_FEEDBACK='If you think this should be possible, tell the K2 team: k2 send-feedback "<what you were trying to do>"'
answer 403 '{"error":{"code":"people_only","hint":"only a person can do this"},"permission":{"requestable":false,"hint":"Ask your person to run it."}}'
log
is_error "a refusal that can't be requested" 3 \
    "Error: k2 heartbeat log: K2's server refused /cli/heartbeat-log: only a person can do this"$'\n'"Ask your person to run it."$'\n'"It can't be requested."$'\n'"$NO_CARD_FEEDBACK"
answer 403 '{"error":"forbidden"}'
log
is_error "a refusal with no permission card" 1 \
    "Error: k2 heartbeat log: K2's server answered /cli/heartbeat-log with an error: forbidden"
log "$DEAD_PORT"
is_error "nothing listens on the port" 1 \
    "Error: k2 heartbeat log: K2's server gave no answer on /cli/heartbeat-log; run k2 daemon status"

if [ "${#failed[@]}" -gt 0 ]; then
    echo "heartbeat_log_errors: ${#failed[@]} failed, $pass passed: ${failed[*]}" >&2
    exit 1
fi
echo "heartbeat_log_errors: $pass passed"
