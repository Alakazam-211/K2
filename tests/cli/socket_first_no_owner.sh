#!/usr/bin/env bash
# In a session, a socket-first sender's TCP fallback sends the session's
# passport, or nothing (0.46.17).
#
# The socket-first senders try the session's cell socket with the
# passport, then go on over loopback TCP: cli_send, cli_send_json,
# _compute_send, _sidecar_send, _doc_xfer, _app_rt_post_file and the
# k2-open shim. In a session (a passport, or the session socket marker
# K2_HOOK_SOCK), when the socket failed (the connect fails, curl 7, or its
# file is missing), the TCP request sends the session's passport. With no
# passport: one line, exit 1, nothing sent. Outside a session, and
# whenever the socket works, nothing changes.
#
# k2 send-feedback is stricter (_sf_session_token): in a session every one
# of its requests carries the passport, also when the socket says it
# doesn't serve the route, and a session with no passport gets its own one
# line, exit 3, and nothing sent, whatever the socket does (sections 3, 6
# and 8).
#
# Rows (the Open verbs are the ones whose TCP request carried the owner
# token before; the id verbs already sent the passport, and stay so):
#   k2 send-feedback settings --enabled off   _compute_send POST (Open)
#   k2 send-feedback list                     _compute_send GET (Open)
#   k2 send-feedback settings --agents cli    the opt-in: GET, then POST
#   k2 send-feedback "<text>" --diagnostics   POST diagnostics, POST send,
#                                             GET show
#   k2 send-feedback show|retry fb-1          _compute_send GET / POST
#   k2 compute retry j-1                      _compute_send POST (id)
#   k2 read acme                              cli_send GET (Open)
#   k2 settings set homeChat true             cli_send_json POST (id)
#   k2 root-helper status                     _sidecar_send GET (Open)
#   k2 llm tokens add-key claude work         _sidecar_send POST (id)
#   k2 doc cat notes.md                       _doc_xfer POST (id)
#   k2 app exec acme -- cat                   _app_rt_post_file POST (id)
#   scripts/k2-open <url>                     the shim's owner arm
#
# A stub daemon serves loopback TCP and a Unix socket (the live cell
# socket; a mode file makes it answer, or answer "not served") and logs
# each request's channel, method, path and Authorization header. A second
# socket file is bound and closed (connect fails, curl 7); a third path
# has no file. HOME is a temp dir whose ~/.k2/daemon.token holds a
# distinct owner value; never the real ~/.k2. Every run has a clean
# environment (env -i). Opens ports: run it on K2 Compute.
#
#   bash tests/cli/socket_first_no_owner.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
K2_OPEN="$PROJECT_ROOT/scripts/k2-open"
[ -f "$K2_OPEN" ] || { echo "FAIL: $K2_OPEN not found" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

# A Unix socket path must be short: the whole sandbox under /tmp.
T="$(mktemp -d /tmp/k2sf-XXXXXX)"
T="$(cd "$T" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$T"
}
trap cleanup EXIT

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

OWNER="owner-on-disk-3f9a61"
PASSPORT="sess-sf.passport8c1e"
H="$T/home"
WS="$T/ws"
LOG="$T/requests.log"
MODE="$T/mode"
LIVE_SOCK="$T/live.sock"
DEAD_SOCK="$T/dead.sock"
GONE_SOCK="$T/gone.sock"
mkdir -p "$H/.k2" "$WS/.k2" "$T/tmp"
# The CLI reads the owner token from daemon.token when heartbeat.token is
# missing; the k2-open rows add heartbeat.token below.
printf '%s' "$OWNER" >"$H/.k2/daemon.token"
printf '# notes\n' >"$WS/notes.md"
: >"$LOG"
echo ok >"$MODE"

cat >"$T/stub.py" <<'PY'
import json, os, socket, socketserver, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

LOG, MODE, PORTFILE, LIVE, DEAD, WS = sys.argv[1:7]
ANSWERS = {
    "/cli/whoami": {"ok": True, "workspace": "acme", "path": WS},
    "/cli/product-feedback/settings": {"enabled": False, "agents": True, "agentDiagnostics": False, "agentsAsked": False, "attachParts": [], "canSend": True, "endpoint": "stub"},
    "/cli/product-feedback/list": {"items": []},
    "/cli/product-feedback/diagnostics": {"draftId": "d-1", "sections": []},
    "/cli/product-feedback/send": {"id": "fb-1"},
    "/cli/product-feedback/show": {"item": {"id": "fb-1", "state": "sent", "category": "bug"}},
    "/cli/compute/jobs/retry": {"job": {"attempt": 2, "short": "j-1"}},
    "/cli/terminal/read": {"lines": ["a line"]},
    "/cli/settings/update": {"homeChat": True},
    "/cli/root-helper/status": {"installed": False},
}
lock = threading.Lock()

def handler(via):
    class H(BaseHTTPRequestHandler):
        def _send(self, code, obj):
            data = json.dumps(obj, separators=(",", ":")).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def _any(self, method):
            n = int(self.headers.get("Content-Length") or 0)
            if n:
                self.rfile.read(n)
            u = urlsplit(self.path)
            auth = self.headers.get("Authorization") or ""
            with lock, open(LOG, "a") as f:
                f.write(json.dumps({"via": via, "method": method, "path": u.path, "auth": auth}) + "\n")
            if via == "uds" and open(MODE).read().strip() == "notserved":
                return self._send(404, {"error": "route not served on cell socket"})
            return self._send(200, ANSWERS.get(u.path, {"ok": True}))

        def do_GET(self):
            self._any("GET")

        def do_POST(self):
            self._any("POST")

        def log_message(self, *_a):
            pass

        def address_string(self):
            return via
    return H

class TcpS(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True

class UdsS(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

uds = UdsS(LIVE, handler("uds"))
threading.Thread(target=uds.serve_forever, daemon=True).start()
# Bound, then closed: a connect to it fails (curl 7).
d = socket.socket(socket.AF_UNIX)
d.bind(DEAD)
d.close()
tcp = TcpS(("127.0.0.1", 0), handler("tcp"))
with open(PORTFILE + ".tmp", "w") as f:
    f.write(str(tcp.server_address[1]))
os.rename(PORTFILE + ".tmp", PORTFILE)
tcp.serve_forever()
PY
"$PY" -I "$T/stub.py" "$LOG" "$MODE" "$T/stub.port" "$LIVE_SOCK" "$DEAD_SOCK" "$WS" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$T/stub.port" ] && [ -S "$LIVE_SOCK" ] && [ -S "$DEAD_SOCK" ] && break; sleep 0.05; done
[ -s "$T/stub.port" ] && [ -S "$LIVE_SOCK" ] && [ -S "$DEAD_SOCK" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$T/stub.port")"
printf '%s' "$PORT" >"$H/.k2/heartbeat.port"
[ ! -e "$GONE_SOCK" ] || { echo "FAIL: $GONE_SOCK exists" >&2; exit 1; }

echo "== probe: the dead socket's connect fails with curl 7 =="
prc=0
curl -s -o /dev/null --unix-socket "$DEAD_SOCK" --max-time 2 "http://localhost/cli/whoami" || prc=$?
if [ "$prc" = 7 ]; then ok "dead socket: curl 7"; else bad "dead socket: curl $prc, want 7"; fi

STDIN_FILE=/dev/null
# run <sock or ""> <passport or ""> <program> args...: one run in a clean
# environment; RC, OUT and ERR hold the result, the log only its requests.
run() {
    local sock="$1" pp="$2"
    shift 2
    local envs=(PATH="$PATH" HOME="$H" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_PROJECT_PATH="$WS" TMPDIR="$T/tmp")
    [ -n "$sock" ] && envs+=(K2_HOOK_SOCK="$sock")
    [ -n "$pp" ] && envs+=(K2_HOOK_TOKEN="$pp")
    : >"$LOG"
    set +e
    (cd "$WS" && env -i "${envs[@]}" "$@" <"$STDIN_FILE" >"$T/out" 2>"$T/err")
    RC=$?
    set -e
    OUT="$(cat "$T/out")"
    ERR="$(cat "$T/err")"
}
k2() { local sock="$1" pp="$2"; shift 2; run "$sock" "$pp" "$K2_CLI" "$@"; }

# count <via|any> <method> <path> <auth>: requests in the log with all four.
count() {
    "$PY" -I - "$LOG" "$1" "$2" "$3" "$4" <<'PY'
import json, sys
log, via, method, path, auth = sys.argv[1:6]
n = 0
for line in open(log):
    r = json.loads(line)
    if (via == "any" or r["via"] == via) and r["method"] == method and r["path"] == path and r["auth"] == auth:
        n += 1
print(n)
PY
}
# with_auth <auth>: requests in the log, any channel, that carry that auth.
with_auth() {
    "$PY" -I - "$LOG" "$1" <<'PY'
import json, sys
log, auth = sys.argv[1:3]
print(sum(1 for line in open(log) if json.loads(line)["auth"] == auth))
PY
}
# on_via <via>: requests in the log on that channel.
on_via() {
    "$PY" -I - "$LOG" "$1" <<'PY'
import json, sys
log, via = sys.argv[1:3]
print(sum(1 for line in open(log) if json.loads(line)["via"] == via))
PY
}
requests() { wc -l <"$LOG" | tr -d ' '; }
show() { echo "    rc=$RC stderr=$(printf %q "$ERR") log=$(tr '\n' ';' <"$LOG")" >&2; }

P="Bearer $PASSPORT"
O="Bearer $OWNER"

# expect_tcp_passport <label> <method> <path>: the TCP fallback carried the
# passport, and no request in the run carried the owner token.
expect_tcp_passport() {
    local label="$1" method="$2" path="$3" n
    n="$(count tcp "$method" "$path" "$P")"
    if [ "$n" -ge 1 ]; then ok "$label: TCP $method $path carries the passport"; else bad "$label: no TCP $method $path with the passport"; show; fi
    n="$(with_auth "$O")"
    if [ "$n" = 0 ]; then ok "$label: no request carries the owner token"; else bad "$label: $n request(s) carried the owner token"; show; fi
}
# expect_tcp_owner <label> <method> <path>: TCP with the owner token, as before.
expect_tcp_owner() {
    local label="$1" method="$2" path="$3" n
    n="$(count tcp "$method" "$path" "$O")"
    if [ "$n" -ge 1 ]; then ok "$label: TCP $method $path carries the owner token, as before"; else bad "$label: no TCP $method $path with the owner token"; show; fi
}
# expect_socket_only <label> <method> <path>: the cell socket took it with
# the passport, and nothing went over TCP.
expect_socket_only() {
    local label="$1" method="$2" path="$3" n
    n="$(count uds "$method" "$path" "$P")"
    if [ "$n" -ge 1 ]; then ok "$label: the socket took $method $path with the passport"; else bad "$label: no $method $path on the socket"; show; fi
    n="$(on_via tcp)"
    if [ "$n" = 0 ]; then ok "$label: nothing over TCP"; else bad "$label: $n request(s) over TCP"; show; fi
}
LINE="Error: in a session, K2 doesn't act as the owner; this session has no passport, so nothing was sent."
SF_LINE="Error: in a session, k2 send-feedback doesn't act as this server's owner, and this session has no passport, so nothing was sent."
# expect_refused <label> [exit codes] [line]: the rule's one line on
# stderr, the exit code (1; a sender on the left of a pipe ends the command
# with 141, as _k2_end_command says) and no request at all.
expect_refused() {
    local label="$1" codes="${2:-1}" line="${3:-$LINE}" n
    case "|$codes|" in *"|$RC|"*) ok "$label: exit $RC" ;; *) bad "$label: exit $RC, want $codes"; show ;; esac
    if [ "$ERR" = "$line" ]; then ok "$label: the rule's one line"; else bad "$label: stderr is not the rule's one line"; show; fi
    n="$(requests)"
    if [ "$n" = 0 ]; then ok "$label: nothing sent"; else bad "$label: $n request(s) sent"; show; fi
}

echo "== 1. a session with a passport, the cell socket's connect fails (curl 7) =="
k2 "$DEAD_SOCK" "$PASSPORT" send-feedback settings --enabled off
expect_tcp_passport "send-feedback settings (_compute_send POST)" POST /cli/product-feedback/settings
k2 "$DEAD_SOCK" "$PASSPORT" send-feedback list
expect_tcp_passport "send-feedback list (_compute_send GET)" GET /cli/product-feedback/list
k2 "$DEAD_SOCK" "$PASSPORT" compute retry j-1
expect_tcp_passport "compute retry (_compute_send POST)" POST /cli/compute/jobs/retry
k2 "$DEAD_SOCK" "$PASSPORT" read acme
expect_tcp_passport "read (cli_send GET)" GET /cli/terminal/read
k2 "$DEAD_SOCK" "$PASSPORT" settings set homeChat true
expect_tcp_passport "settings set (cli_send_json POST)" POST /cli/settings/update
k2 "$DEAD_SOCK" "$PASSPORT" root-helper status
expect_tcp_passport "root-helper status (_sidecar_send GET)" GET /cli/root-helper/status
printf 'sk-stub-key\n' >"$T/key"
STDIN_FILE="$T/key" k2 "$DEAD_SOCK" "$PASSPORT" llm tokens add-key claude work
expect_tcp_passport "llm tokens add-key (_sidecar_send POST)" POST /cli/llm/accounts/add-key
k2 "$DEAD_SOCK" "$PASSPORT" doc cat "$WS/notes.md"
expect_tcp_passport "doc cat (_doc_xfer POST)" POST /cli/docs/tool
printf 'hi\n' >"$T/stdin"
STDIN_FILE="$T/stdin" k2 "$DEAD_SOCK" "$PASSPORT" app exec acme -- cat
expect_tcp_passport "app exec (_app_rt_post_file POST)" POST /cli/app/exec

echo "== 2. a session with a passport, no cell socket file =="
k2 "$GONE_SOCK" "$PASSPORT" send-feedback settings --enabled off
expect_tcp_passport "send-feedback settings, socket missing" POST /cli/product-feedback/settings
k2 "$GONE_SOCK" "$PASSPORT" read acme
expect_tcp_passport "read, socket missing" GET /cli/terminal/read
k2 "$GONE_SOCK" "$PASSPORT" root-helper status
expect_tcp_passport "root-helper status, socket missing" GET /cli/root-helper/status

echo "== 3. a session with no passport: one line, exit 1 (send-feedback: 3), nothing sent =="
k2 "$DEAD_SOCK" "" send-feedback settings --enabled off
expect_refused "send-feedback settings" 3 "$SF_LINE"
k2 "$DEAD_SOCK" "" send-feedback list
expect_refused "send-feedback list" 3 "$SF_LINE"
k2 "$DEAD_SOCK" "" root-helper status
expect_refused "root-helper status"
k2 "$GONE_SOCK" "" send-feedback settings --enabled off
expect_refused "send-feedback settings, socket missing" 3 "$SF_LINE"
# k2 read pipes its sender into a reader: _k2_end_command ends it with 141.
k2 "$DEAD_SOCK" "" read acme
expect_refused "read (a sender on the left of a pipe)" "1|141"

echo "== 4. outside a session (the owner's own terminal): unchanged =="
k2 "" "" send-feedback settings --enabled off
expect_tcp_owner "send-feedback settings" POST /cli/product-feedback/settings
k2 "" "" send-feedback list
expect_tcp_owner "send-feedback list" GET /cli/product-feedback/list
k2 "" "" send-feedback settings --agents cli
expect_tcp_owner "send-feedback opt-in, the read" GET /cli/product-feedback/settings
expect_tcp_owner "send-feedback opt-in, the change" POST /cli/product-feedback/settings
k2 "" "" send-feedback "x broke" --category bug --scope cli --command "k2 read acme" --diagnostics --yes
expect_tcp_owner "send-feedback send, diagnostics" POST /cli/product-feedback/diagnostics
expect_tcp_owner "send-feedback send" POST /cli/product-feedback/send
expect_tcp_owner "send-feedback send, show" GET /cli/product-feedback/show
k2 "" "" read acme
expect_tcp_owner "read" GET /cli/terminal/read
k2 "" "" root-helper status
expect_tcp_owner "root-helper status" GET /cli/root-helper/status

echo "== 5. a session whose cell socket works: unchanged =="
echo ok >"$MODE"
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback settings --enabled off
expect_socket_only "send-feedback settings" POST /cli/product-feedback/settings
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback list
expect_socket_only "send-feedback list" GET /cli/product-feedback/list
k2 "$LIVE_SOCK" "$PASSPORT" compute retry j-1
expect_socket_only "compute retry" POST /cli/compute/jobs/retry
k2 "$LIVE_SOCK" "$PASSPORT" read acme
expect_socket_only "read" GET /cli/terminal/read
k2 "$LIVE_SOCK" "$PASSPORT" settings set homeChat true
expect_socket_only "settings set" POST /cli/settings/update
k2 "$LIVE_SOCK" "$PASSPORT" root-helper status
expect_socket_only "root-helper status" GET /cli/root-helper/status

echo "== 6. the socket works and says it doesn't serve the route: TCP with \$TOKEN, as before; send-feedback with the passport =="
echo notserved >"$MODE"
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback settings --enabled off
if [ "$(count uds POST /cli/product-feedback/settings "$P")" = 1 ]; then ok "send-feedback settings: the socket was asked first"; else bad "send-feedback settings: the socket was not asked"; show; fi
expect_tcp_passport "send-feedback settings, not served on the socket" POST /cli/product-feedback/settings
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback settings
expect_tcp_passport "send-feedback settings read, not served on the socket" GET /cli/product-feedback/settings
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback settings --agents cli
expect_tcp_passport "send-feedback opt-in read, not served on the socket" GET /cli/product-feedback/settings
expect_tcp_passport "send-feedback opt-in change, not served on the socket" POST /cli/product-feedback/settings
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback "x broke" --category bug --scope cli --command "k2 read acme" --diagnostics --yes
[ "$RC" = 0 ] && ok "send-feedback send, not served on the socket: exit 0" || { bad "send-feedback send, not served on the socket: exit $RC"; show; }
expect_tcp_passport "send-feedback send diagnostics, not served on the socket" POST /cli/product-feedback/diagnostics
expect_tcp_passport "send-feedback send, not served on the socket" POST /cli/product-feedback/send
expect_tcp_passport "send-feedback send show, not served on the socket" GET /cli/product-feedback/show
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback list
expect_tcp_passport "send-feedback list, not served on the socket" GET /cli/product-feedback/list
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback show fb-1
expect_tcp_passport "send-feedback show, not served on the socket" GET /cli/product-feedback/show
k2 "$LIVE_SOCK" "$PASSPORT" send-feedback retry fb-1
expect_tcp_passport "send-feedback retry, not served on the socket" POST /cli/product-feedback/retry
k2 "$LIVE_SOCK" "$PASSPORT" read acme
if [ "$CLI_UNDER_TEST" != "$CLI_BASH_SOURCE" ]; then
    # The k5 binary answers read itself and never holds the owner token
    # beside a passport: scripts/cli-parity.sh's named difference
    # owner-token-with-passport (PRD 13.9 #4). Its TCP try carries the
    # passport, and nothing carries the owner token.
    expect_tcp_passport "read, not served on the socket (the k5 binary)" GET /cli/terminal/read
else
    expect_tcp_owner "read, not served on the socket" GET /cli/terminal/read
fi
echo ok >"$MODE"

echo "== 7. the k2-open shim (its owner arm reads heartbeat.token) =="
printf '%s' "$OWNER" >"$H/.k2/heartbeat.token"
run "$DEAD_SOCK" "$PASSPORT" /bin/sh "$K2_OPEN" "https://example.com/a"
expect_tcp_passport "k2-open, socket connect fails" POST /cli/browser/open-url
[ "$RC" = 0 ] && ok "k2-open, socket connect fails: exit 0" || { bad "k2-open, socket connect fails: exit $RC"; show; }
run "$DEAD_SOCK" "" /bin/sh "$K2_OPEN" "https://example.com/b"
expect_refused "k2-open, no passport"
run "" "" /bin/sh "$K2_OPEN" "https://example.com/c"
expect_tcp_owner "k2-open outside a session" POST /cli/browser/open-url
run "$LIVE_SOCK" "$PASSPORT" /bin/sh "$K2_OPEN" "https://example.com/d"
expect_socket_only "k2-open, the socket works" POST /cli/browser/open-url

echo "== 8. send-feedback in a session with no passport, the socket works or doesn't serve it: one line, exit 3, nothing sent =="
for m in ok notserved; do
    echo "$m" >"$MODE"
    k2 "$LIVE_SOCK" "" send-feedback settings
    expect_refused "send-feedback settings read ($m)" 3 "$SF_LINE"
    k2 "$LIVE_SOCK" "" send-feedback settings --agents cli
    expect_refused "send-feedback opt-in ($m)" 3 "$SF_LINE"
    k2 "$LIVE_SOCK" "" send-feedback "x broke" --category bug --scope cli --command "k2 read acme" --diagnostics --yes
    expect_refused "send-feedback send ($m)" 3 "$SF_LINE"
    k2 "$LIVE_SOCK" "" send-feedback list
    expect_refused "send-feedback list ($m)" 3 "$SF_LINE"
    k2 "$LIVE_SOCK" "" send-feedback retry fb-1
    expect_refused "send-feedback retry ($m)" 3 "$SF_LINE"
done
echo ok >"$MODE"
# Help still prints help in a session with no passport, and sends nothing.
k2 "$LIVE_SOCK" "" send-feedback list --help
if [ "$RC" = 0 ] && [ "$(requests)" = 0 ]; then ok "send-feedback list --help, no passport: help, nothing sent"; else bad "send-feedback list --help, no passport"; show; fi

echo
echo "socket_first_no_owner: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
