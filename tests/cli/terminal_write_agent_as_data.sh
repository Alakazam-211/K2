#!/usr/bin/env bash
# `k2 terminal write <workspace> --agent <name>` hands the agent name to
# Python as data, so it picks the session of the agent named exactly that.
#
# resolve_write_session_id (k2 terminal write and k2 talk) matches --agent
# against the workspace's live sessions in Python. The name reaches that
# Python in argv, never inside the program text. This test gives it a name
# holding a double quote, three double quotes, a single quote, three single
# quotes, a backslash and a newline; the stub lists another agent and a
# near-miss name first, and the write must go to the session of the agent
# named exactly that. `--agent` with no word after it is a usage error,
# "Error: --agent takes a value", exit 2, nothing sent (before 0.46.17 it
# was sent as the message).
#
# How: a python stub daemon on a free loopback port answers the routes and
# keeps every request. HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/terminal_write_agent_as_data.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-tw-agent-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

REQ_LOG="$WORK/requests.jsonl"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"
echo '[]' >"$WORK/hb_list.json"
echo '[]' >"$WORK/sessions.json"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
def fixture(name):
    return json.load(open(work + "/" + name))
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "surrogateescape") if n else ""
        u = urllib.parse.urlparse(self.path)
        with open(work + "/requests.jsonl", "a") as f:
            f.write(json.dumps({"method": self.command, "path": u.path, "query": u.query, "body": body}) + "\n")
        if u.path == "/cli/heartbeat/list":
            out = fixture("hb_list.json")
        elif u.path == "/cli/heartbeat/status":
            out = []
        elif u.path == "/cli/workspace/resolve":
            out = {"path": "/stub/ws"}
        elif u.path == "/cli/sessions/list-for-workspace":
            out = fixture("sessions.json")
        else:
            out = {"success": True, "ok": True}
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

# run <args...>: sets OUT (stdout), ERR (stderr), RC.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}
# want_out <label> <want>: exit 0 and stdout exactly <want>.
want_out() {
    [ "$RC" -eq 0 ] && ok "$1: exit 0" || { bad "$1: exit $RC"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    if [ "$OUT" = "$2" ]; then
        ok "$1: prints the value exactly"
    else
        bad "$1: stdout $(printf %q "$OUT") (want $(printf %q "$2"))"
    fi
}

NL=$'\n'
A="ha\"b\"\"\"c'd'''e\\f${NL}g"
B="nb\"\"\"'''\\x${NL}y\"z"

echo "== k2 terminal write --agent =="
printf '%s' "$A" | python3 -c '
import json, sys
a = sys.stdin.read()
print(json.dumps([
    {"isV2": True, "agentName": "other", "sessionId": "s-other"},
    {"isV2": True, "agentName": a + "x", "sessionId": "s-near"},
    {"isV2": True, "agentName": a, "sessionId": "s-match"},
]))' >"$WORK/sessions.json"
: >"$REQ_LOG"
run terminal write acme --agent "$A" "hello"
if python3 -c '
import json, sys, urllib.parse
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
w = [r for r in rows if r["path"] == "/cli/terminal/write"]
assert len(w) == 1, "want one write, got %d" % len(w)
q = urllib.parse.parse_qs(w[0]["query"] + "&" + w[0]["body"])
assert q.get("id") == ["s-match"], q.get("id")
' "$REQ_LOG" 2>"$WORK/perr"; then
    ok "terminal write --agent: writes to the session of the agent named exactly that"
else
    bad "terminal write --agent: $(cat "$WORK/perr")"; cat "$REQ_LOG" >&2
fi

echo "== --agent with no value is a usage error, nothing sent =="
for form in "acme --agent" "s-1 --agent"; do
    : >"$REQ_LOG"
    # shellcheck disable=SC2086
    run terminal write $form
    [ "$RC" -eq 2 ] && ok "terminal write $form: exit 2" || bad "terminal write $form: exit $RC (want 2)"
    [ "$ERR" = "Error: --agent takes a value" ] && ok "terminal write $form: says --agent takes a value" \
        || { bad "terminal write $form: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "terminal write $form: nothing on stdout" \
        || { bad "terminal write $form: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    if [ -s "$REQ_LOG" ]; then
        bad "terminal write $form: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "terminal write $form: sent nothing"
    fi
done

echo ""
echo "terminal_write_agent_as_data: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
