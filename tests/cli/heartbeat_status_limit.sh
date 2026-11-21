#!/usr/bin/env bash
# `k2 heartbeat status <name> --limit N` (and -n N): N is digits, or a
# usage error.
#
# Regression (0.46.17): cmd_heartbeat_status took the next word as the
# limit, whatever it was, and put it in the query as typed. `--limit` as
# the last word stopped on bash's own "$2: unbound variable" (exit 1),
# `--limit 5x` and `--limit --json` were sent, and `--limit '5&x=1'` added
# a field of its own to the query.
#
# Now a --limit or -n with no word after it, or with a word that is not
# digits only, prints "Error: <flag> needs a number" on stderr, nothing on
# stdout, exits 2 (as hooks status does) and sends nothing: not the
# heartbeat list it reads first either. A good limit is URL-encoded in the
# query. The default (10) and the last of several are as before.
#
# How: a python stub daemon on a free loopback port records every request
# with its query, and answers the list with one heartbeat (hb1) and the
# history with none. HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/heartbeat_status_limit.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-heartbeat-status-limit-XXXXXX)"
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

REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        u = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(u.query, keep_blank_values=True)
        keys = sorted(k for k in q if k != "project")
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, u.path,
                    "&".join("%s=%s" % (k, ",".join(q[k])) for k in keys)))
        out = json.dumps([{"name": "hb1"}] if u.path == "/cli/heartbeat/list" else []).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
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

run() {
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# refused <flag> <args...>: the usage error, exit 2, nothing sent.
refused() {
    local flag="$1"; shift
    local label="k2 $*"
    run "$@"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ "$ERR" = "Error: $flag needs a number" ] && ok "$label: names $flag" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
}

# sent <query> <args...>: exit 0, the list, then one GET
# /cli/heartbeat/status with that query (project aside).
sent() {
    local want="$1"; shift
    local label="k2 $*"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || { bad "$label: exit $RC (want 0)"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ "$(cat "$REQ_LOG")" = "$(printf 'GET /cli/heartbeat/list \nGET /cli/heartbeat/status %s' "$want")" ] && ok "$label: sends $want" \
        || { bad "$label: requests"; sed 's/^/      | /' "$REQ_LOG" >&2; }
    [ "$OUT" = 'No fire history for heartbeat "hb1" yet.' ] && ok "$label: stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
}

echo "== a missing limit is a usage error =="
refused --limit heartbeat status hb1 --limit
refused -n heartbeat status hb1 -n
refused --limit heartbeat status --limit

echo "== a limit that is not digits only is a usage error =="
refused --limit heartbeat status hb1 --limit 5x
refused --limit heartbeat status hb1 --limit -3
refused --limit heartbeat status hb1 --limit ""
refused --limit heartbeat status hb1 --limit "5 6"
refused --limit heartbeat status hb1 --limit "5&x=1"
refused --limit heartbeat status hb1 --limit --json
refused -n heartbeat status hb1 -n 1.5
refused --limit heartbeat status hb1 --limit 5 --limit x

echo "== a good limit is sent =="
sent "limit=10&name=hb1" heartbeat status hb1
sent "limit=5&name=hb1" heartbeat status hb1 --limit 5
sent "limit=7&name=hb1" heartbeat status --limit 7 hb1
sent "limit=7&name=hb1" heartbeat status hb1 -n 5 --limit 7
sent "limit=007&name=hb1" heartbeat status hb1 -n 007

echo ""
echo "heartbeat_status_limit: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
