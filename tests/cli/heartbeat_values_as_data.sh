#!/usr/bin/env bash
# The heartbeat verbs hand the caller's values to Python as data, so each
# one comes back exactly as given.
#
# These commands print or match a value they were given (a heartbeat name,
# a rename's two names) in Python. The value reaches that
# Python in argv, never inside the program text. This test gives each one
# a value holding a double quote, three double quotes, a single quote,
# three single quotes, a backslash and a newline, and checks:
#   - k2 heartbeat rename A B prints "Renamed heartbeat: A -> B";
#   - k2 heartbeat use-pinned-session on|off N names N and the state;
#   - k2 heartbeat status N prints "No fire history for heartbeat "N" yet.".
# And k2 heartbeat add still sends the same schedule spec for each kind of
# schedule (its flags are checked before Python sees them).
#
# How: a python stub daemon on a free loopback port answers the routes and
# keeps every request. HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/heartbeat_values_as_data.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-hb-data-XXXXXX)"
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

echo "== k2 heartbeat rename =="
run heartbeat rename "$A" "$B"
want_out "rename" "Renamed heartbeat: $A -> $B"

echo "== k2 heartbeat use-pinned-session =="
run heartbeat use-pinned-session on "$A"
want_out "use-pinned-session on" "Pinned-session delivery enabled for heartbeat: $A"
run heartbeat use-pinned-session off "$B"
want_out "use-pinned-session off" "Pinned-session delivery disabled for heartbeat: $B"

echo "== k2 heartbeat status =="
printf '%s' "$A" | python3 -c 'import json, sys; print(json.dumps([{"name": sys.stdin.read()}]))' >"$WORK/hb_list.json"
run heartbeat status "$A"
want_out "status" "No fire history for heartbeat \"$A\" yet."

echo "== k2 heartbeat add: the spec is unchanged =="
# spec_of <want spec JSON> <want frequency> <add flags...>
spec_of() {
    local want="$1" freq="$2"; shift 2
    : >"$REQ_LOG"
    run heartbeat add --name hb1 "$@" --instructions "go"
    if python3 -c '
import json, sys, urllib.parse
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
w = [r for r in rows if r["path"] == "/cli/heartbeat/add"]
assert len(w) == 1, "want one add, got %d" % len(w)
q = urllib.parse.parse_qs(w[0]["query"] + "&" + w[0]["body"])
assert json.loads(q["spec"][0]) == json.loads(sys.argv[2]), q["spec"]
assert q["frequency"] == [sys.argv[3]], q["frequency"]
' "$REQ_LOG" "$want" "$freq" 2>"$WORK/perr"; then
        ok "add $*: spec $want"
    else
        bad "add $*: $(cat "$WORK/perr")"; cat "$REQ_LOG" >&2
    fi
}
spec_of '{"frequency": "daily", "time": "09:30"}' daily --daily --time 09:30
spec_of '{"frequency": "weekly", "time": "08:00", "days": ["mon", "fri"]}' weekly --weekly --time 08:00 --days mon,fri
spec_of '{"frequency": "monthly", "days_of_month": [1, 15]}' monthly --monthly --days 1,15
spec_of '{"frequency": "yearly", "days_of_month": [3], "months": ["jan", "feb"]}' yearly --yearly --days 3 --months jan,feb
spec_of '{"frequency": "hourly", "every_seconds": 7200, "start": "08:00", "end": "18:00"}' hourly --hourly --every 2 --unit hours --start 08:00 --end 18:00

echo ""
echo "heartbeat_values_as_data: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
