#!/usr/bin/env bash
# k2 app requests: visitors' requests to an App, one row each (the
# forwarder's per-request log). The CLI is thin: it turns its flags into
# GET /cli/app/requests and prints the daemon's rows as a compact table.
#
# A stub daemon on loopback answers:
#   GET /cli/app/requests?app=jobs     three rows (an App answer, K2's 502, a
#                                      request the client cut), the query it got
#   GET /cli/app/requests?app=quiet    no rows
#   GET /cli/app/requests?app=nope     404 app_not_found
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, no real
# daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2arq-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from urllib.parse import urlparse, parse_qs
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = sys.argv[2]

def row(at, method, path, status, code=None, closed=None, bin=0, bout=0, ms=0):
    return {"atMs": at, "app": "jobs", "deployment": "d-1", "method": method, "path": path, "status": status,
            "k2Code": code, "bytesIn": bin, "bytesOut": bout, "headMs": ms, "totalMs": ms, "closedEarly": closed,
            "ws": None, "visitor": "0123456789abcdef"}

JOBS = {"ok": True, "app": "jobs", "order": "newest first", "dropped": 0, "keep": {"days": 7, "rowsPerApp": 200000},
        "totals": {"requests": 3, "ok": 1, "clientErrors": 0, "serverErrors": 1, "k2Answered": 1, "closedEarly": 1},
        "requests": [row(1700000003000, "POST", "/upload", 0, closed="client", bin=5 * 1024 * 1024, ms=900),
                     row(1700000002000, "POST", "/upload", 502, code="app_not_answering", bin=310, bout=180, ms=4),
                     row(1700000001000, "GET", "/", 200, bin=120, bout=2048, ms=12)]}
QUIET = {"ok": True, "app": "quiet", "order": "newest first", "dropped": 4, "keep": {"days": 7, "rowsPerApp": 200000},
         "totals": {"requests": 0}, "requests": []}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def answer(self, st, v):
        body = json.dumps(v).encode()
        self.send_response(st)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write("%s %s\n" % (u.path, u.query))
        if u.path != "/cli/app/requests":
            return self.answer(404, {"error": {"code": "not_found", "hint": u.path}})
        app = parse_qs(u.query)["app"][0]
        if app == "jobs":
            self.answer(200, JOBS)
        elif app == "quiet":
            self.answer(200, QUIET)
        else:
            self.answer(404, {"error": {"code": "app_not_found", "hint": "no App '%s' on this box (k2 app list)" % app}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" "$SANDBOX/queries" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

k() { K2_PORT="$PORT" K2_HOOK_TOKEN=passport-arq "$K2" "$@" </dev/null; }

echo "== k2 app requests: the table =="
out="$(k app requests jobs)"
expect_has "the totals line" "$out" "jobs: 3 requests (1 ok, 0 4xx, 1 5xx; 1 answered by K2, 1 closed early)"
expect_has "the header" "$out" "TIME"
expect_has "the columns" "$out" "METHOD   PATH"
expect_has "K2's own answer and its code" "$out" "502 K2 app_not_answering"
expect_has "no answer came, the client cut it" "$out" "- (client closed)"
expect_has "the App's answer" "$out" "GET      /      "
expect_has "bytes are short" "$out" "5.0M"
expect_has "the retention" "$out" "Kept 7 days, at most 200000 rows per App."
expect_not "never a visitor key in the table" "$out" "0123456789abcdef"

echo "== flags become the route's query =="
: > "$SANDBOX/queries"
k app requests jobs --since 1h --until 10m --status 5xx --path /upload --method POST --limit 50 >/dev/null
q="$(cat "$SANDBOX/queries")"
expect_has "the route" "$q" "/cli/app/requests "
for want in "app=jobs" "since=1h" "until=10m" "status=5xx" "method=POST" "limit=50"; do
    expect_has "sends $want" "$q" "$want"
done
# The %XX hex's case is the sender's: curl 8.5 and 8.7 write -G data's
# escapes in lower case, curl 8.23 and the k5 binary in upper case (the
# daemon decodes either; crates/k5-cli/src/transport/wire.rs).
expect_has "sends path_prefix=%2fupload" "$(printf '%s' "$q" | tr 'A-F' 'a-f')" "path_prefix=%2fupload"

echo "== --json is the daemon's answer =="
out="$(k app requests jobs --json)"
expect_eq "json rows" "$(python3 -c 'import json,sys; v=json.load(sys.stdin); print(len(v["requests"]), v["requests"][1]["k2Code"])' <<<"$out")" "3 app_not_answering"

echo "== nothing in the window =="
out="$(k app requests quiet)"
expect_has "says no request reached the App" "$out" "No visitor request reached this App in that window"
expect_has "says how far back rows go" "$out" "Rows are kept 7 days."
expect_has "says rows were dropped" "$out" "note: 4 row(s) were dropped"

echo "== refusals =="
rc=0
k app requests nope >"$SANDBOX/out" 2>"$SANDBOX/err" || rc=$?
expect_eq "an unknown App exits 3" "$rc" "3"
expect_has "the daemon's code" "$(cat "$SANDBOX/err")" "app_not_found"
rc=0
k app requests >"$SANDBOX/out" 2>"$SANDBOX/err" || rc=$?
expect_eq "no App is usage (exit 2)" "$rc" "2"
expect_has "the usage line" "$(cat "$SANDBOX/err")" "usage: k2 app requests <app-id>"
rc=0
k app requests jobs --bogus >"$SANDBOX/out" 2>"$SANDBOX/err" || rc=$?
expect_eq "an unknown flag exits 2" "$rc" "2"

echo "== help =="
out="$(k app door --help 2>&1 || true)"
expect_has "k2 app door --help lists it" "$out" "k2 app requests <app-id>"
out="$(k app --help 2>&1 || true)"
expect_has "k2 app --help lists it" "$out" "k2 app requests <app-id>"
out="$(HOME="$SANDBOX/home" "$K2" study app-deploys 2>&1 </dev/null)"
expect_has "the study page explains it" "$out" "DID A VISITOR'S REQUEST REACH THE APP? (k2 app requests)"
expect_has "the study page says what is never stored" "$out" "Never stored: query strings, header"

echo
echo "app_requests_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
