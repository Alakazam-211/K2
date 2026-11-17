#!/usr/bin/env bash
# A POST over the cell socket is sent again over TCP only when the socket
# took none of it (0.46.15).
#
# cli_send and cli_send_json try the session's cell socket first. They go
# on to loopback TCP for a POST only when the connect failed (curl 7) or
# the daemon answered "not served on cell socket". Any other failure after
# the request may have gone out (curl 28, 52, 56, rc 0 with an empty body,
# status 000) is not resent: the command prints
#   Error: K2's server didn't answer <ep> (<n> s); the request may have been applied. Check before running it again.
# and exits 1. GETs still fall back.
#
# Commands (one route each, so the counts below are per route):
#   k2 inbox archive <id>           cli_send cli_post_form (POST)
#   k2 heartbeat unarchive <name>   cli_send cli_request_post (POST, piped)
#   k2 inbox read <id>              cli_send cli_request (GET)
#   k2 agent identity --color c     cli_send_json POST
#   k2 agent identity               cli_send_json GET
#
# A stub daemon serves loopback TCP and a Unix socket and counts each route
# per channel. The socket's behaviour per run comes from a mode file:
#   hang (curl 28), close (52), reset (56), empty (rc 0, empty body),
#   garbage (not HTTP: status 000), notserved (the daemon's 404).
# A second socket file is bound and never listens: connect fails (curl 7).
# The socket file closed to this user (EACCES, which the CLI names 75) still
# falls back; a failure while TMPDIR can't be written (76) does not.
# A probe runs curl itself against each mode first, so each case is known
# to be the exit code it names.
#
# Hermetic: throwaway HOME, every K2_*/K2SO_* var unset, a python stub. No
# real daemon, ~/.k2 or keychain. Opens ports: run it on K2 Compute.
#
#   bash tests/cli/cell_socket_post_no_resend.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
[ -x "$K2" ] || { echo "FAIL: $K2 not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

# Short: a Unix socket path must stay under ~104 bytes.
SANDBOX="$(mktemp -d /tmp/k2nr-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
PIDS=()
cleanup() {
    local p
    for p in "${PIDS[@]:-}"; do
        [ -n "$p" ] && kill "$p" 2>/dev/null || true
    done
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, os, socket, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

LOG, PORTF, MODEF, SOCK, DEAD = sys.argv[1:6]
TARGETS = ("/cli/inbox/archive", "/cli/inbox/read", "/cli/heartbeat/unarchive", "/cli/agent/identity")
log_lock = threading.Lock()

def log(via, method, path):
    with log_lock, open(LOG, "a") as f:
        f.write("%s %s %s\n" % (via, method, path))

def answer(path, method):
    if path == "/cli/inbox/read":
        return {"id": "item-1", "content": "from tcp"}
    if path == "/cli/heartbeat/unarchive":
        return {"success": True}
    if path == "/cli/agent/identity":
        d = {"handle": "scout", "displayName": "Scout", "color": "#22c55e"}
        if method == "POST":
            d["changed"] = ["color"]
        return d
    return {"ok": True}

def http(status, reason, obj):
    body = json.dumps(obj).encode()
    return ("HTTP/1.1 %d %s\r\nContent-Type: application/json\r\nContent-Length: %d\r\n"
            "Connection: close\r\n\r\n" % (status, reason, len(body))).encode() + body

def read_request(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(65536)
        if not chunk:
            return None
        data += chunk
    head, _, rest = data.partition(b"\r\n\r\n")
    n = 0
    for line in head.split(b"\r\n")[1:]:
        k, _, v = line.partition(b":")
        if k.strip().lower() == b"content-length":
            n = int(v.strip())
    while len(rest) < n:
        chunk = conn.recv(65536)
        if not chunk:
            break
        rest += chunk
    return head.split(b"\r\n")[0].decode()

def request_line_of(raw):
    return raw.split(b"\r\n")[0].decode()

def serve_uds(conn):
    mode = open(MODEF).read().strip()
    try:
        if mode == "reset":
            # Leave the request unread and close: the peer's read gets
            # ECONNRESET (curl 56).
            time.sleep(0.3)
            line = request_line_of(conn.recv(65536, socket.MSG_PEEK))
        else:
            line = read_request(conn)
            if line is None:
                return
        method, target = line.split(" ")[0], line.split(" ")[1]
        path = urlsplit(target).path
        log("uds", method, path)
        if path not in TARGETS:
            conn.sendall(http(404, "Not Found", {"error": "route not served on cell socket"}))
        elif mode == "reset":
            pass
        elif mode == "hang":
            time.sleep(4)
        elif mode == "close":
            pass
        elif mode == "empty":
            conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        elif mode == "garbage":
            conn.sendall(b"garbage\r\n\r\n")
        elif mode == "notserved":
            conn.sendall(http(404, "Not Found", {"error": "route not served on cell socket"}))
        else:
            raise SystemExit("unknown mode " + mode)
    except (BrokenPipeError, ConnectionResetError):
        pass
    finally:
        conn.close()

def uds_loop():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.bind(SOCK)
    s.listen(16)
    while True:
        c, _ = s.accept()
        threading.Thread(target=serve_uds, args=(c,), daemon=True).start()

# A socket file nobody listens on: connect() fails (curl 7).
dead = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
dead.bind(DEAD)

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        path = urlsplit(self.path).path
        log("tcp", self.command, path)
        body = json.dumps(answer(path, self.command)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    do_GET = handle_any
    do_POST = handle_any

threading.Thread(target=uds_loop, daemon=True).start()
srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
srv.daemon_threads = True
with open(PORTF, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
MODE="$SANDBOX/mode"
SOCK="$SANDBOX/c.sock"
DEAD="$SANDBOX/d.sock"
: > "$LOG"
echo close > "$MODE"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$MODE" "$SOCK" "$DEAD" &
PIDS+=($!)
for _ in $(seq 1 100); do [ -s "$SANDBOX/port" ] && [ -S "$SOCK" ] && [ -S "$DEAD" ] && break; sleep 0.05; done
[ -s "$SANDBOX/port" ] && [ -S "$SOCK" ] && [ -S "$DEAD" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2" "$SANDBOX/ws/.k2"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $*" >&2; fail=$((fail + 1)); }

echo "== 0. probe: each socket mode is the curl exit it names =="
probe() {
    local sock="$1" mode="$2"
    echo "$mode" > "$MODE"
    PRC=0
    PCODE="$(curl -s -o "$SANDBOX/probe.body" -w '%{http_code}' --unix-socket "$sock" --max-time 1 \
        -X POST --data-raw 'x=1' "http://localhost/cli/inbox/archive")" || PRC=$?
    PBODY="$(cat "$SANDBOX/probe.body" 2>/dev/null)" || PBODY=""
}
probe "$DEAD" close;   [ "$PRC" = 7 ] && ok "dead socket: curl 7" || bad "dead socket: curl $PRC, want 7"
probe "$SOCK" hang;    [ "$PRC" = 28 ] && ok "hang: curl 28" || bad "hang: curl $PRC, want 28"
probe "$SOCK" close;   [ "$PRC" = 52 ] && ok "close: curl 52" || bad "close: curl $PRC, want 52"
probe "$SOCK" reset;   [ "$PRC" = 56 ] && ok "reset: curl 56" || bad "reset: curl $PRC, want 56"
probe "$SOCK" empty
[ "$PRC" = 0 ] && [ "$PCODE" = 200 ] && [ -z "$PBODY" ] && ok "empty: curl 0, status 200, empty body" \
    || bad "empty: curl $PRC, status $PCODE, body '$PBODY'"
probe "$SOCK" garbage
case "$PRC" in 0|7|28|52|56) bad "garbage: curl $PRC, want another failure" ;; *) ok "garbage: curl $PRC" ;; esac
[ "$PCODE" = 000 ] && ok "garbage: status 000" || bad "garbage: status $PCODE, want 000"
probe "$SOCK" notserved
[ "$PRC" = 0 ] && [ "$PCODE" = 404 ] && ok "notserved: curl 0, status 404" || bad "notserved: curl $PRC, status $PCODE"

# run <sock-path> <mode> <k2 args…>: an agent in a K2 terminal (passport,
# cell socket). Sets OUT, ERR, RC. RUN_TMPDIR, when set, is its TMPDIR.
run() {
    local sock="$1" mode="$2"; shift 2
    echo "$mode" > "$MODE"
    : > "$LOG"
    set +e
    env TMPDIR="${RUN_TMPDIR:-${TMPDIR:-/tmp}}" \
        K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=test-session.passport \
        K2_HOOK_SOCK="$sock" K2_PROJECT_PATH="$SANDBOX/ws" \
        _K2_POST_MAX_TIME=1 _K2_SEND_MAX_TIME=1 \
        "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    RC=$?
    set -e
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
}
# count <channel> <method> <path>
count() { grep -c "^$1 $2 $3\$" "$LOG" || true; }
want_counts() {
    local label="$1" method="$2" path="$3" uds="$4" tcp="$5" got_u got_t
    got_u="$(count uds "$method" "$path")"
    got_t="$(count tcp "$method" "$path")"
    if [ "$got_u" = "$uds" ] && [ "$got_t" = "$tcp" ]; then
        ok "$label: socket $uds, TCP $tcp"
    else
        bad "$label: socket $got_u, TCP $got_t, want socket $uds, TCP $tcp ($(tr '\n' ';' < "$LOG"))"
    fi
}
# The one stderr line, nothing else.
want_unanswered() {
    local label="$1" ep="$2" re
    re="^Error: K2's server didn't answer ${ep} \([0-9]+ s\); the request may have been applied\. Check before running it again\.\$"
    if [ "$(printf '%s\n' "$ERR" | wc -l | tr -d ' ')" = 1 ] && printf '%s\n' "$ERR" | grep -Eq "$re"; then
        ok "$label: the one 'may have been applied' line"
    else
        bad "$label: stderr is not the one line: $(printf %q "$ERR")"
    fi
}
no_unanswered() {
    case "$ERR" in
        *"may have been applied"*) bad "$1: unexpected 'may have been applied': $(printf %q "$ERR")" ;;
        *) ok "$1: no 'may have been applied' line" ;;
    esac
}

echo "== 1. cli_send POST (k2 inbox archive) =="
run "$DEAD" close inbox archive item-1
want_counts "curl 7" POST /cli/inbox/archive 0 1
[ "$RC" = 0 ] && ok "curl 7: exit 0" || bad "curl 7: exit $RC ($ERR)"
no_unanswered "curl 7"
for mode in hang close reset empty garbage; do
    run "$SOCK" "$mode" inbox archive item-1
    want_counts "$mode" POST /cli/inbox/archive 1 0
    [ "$RC" = 1 ] && ok "$mode: exit 1" || bad "$mode: exit $RC, want 1"
    [ -z "$OUT" ] && ok "$mode: stdout empty" || bad "$mode: stdout $(printf %q "$OUT")"
    want_unanswered "$mode" /cli/inbox/archive
done
run "$SOCK" notserved inbox archive item-1
want_counts "notserved" POST /cli/inbox/archive 1 1
[ "$RC" = 0 ] && ok "notserved: exit 0" || bad "notserved: exit $RC ($ERR)"
no_unanswered "notserved"

echo "== 2. cli_send POST through cli_request_post, read by a pipe (k2 heartbeat unarchive) =="
run "$DEAD" close heartbeat unarchive hb-1
want_counts "curl 7" POST /cli/heartbeat/unarchive 0 1
[ "$RC" = 0 ] && ok "curl 7: exit 0" || bad "curl 7: exit $RC ($ERR)"
[ "$OUT" = "Unarchived heartbeat: hb-1" ] && ok "curl 7: the TCP answer prints" || bad "curl 7: stdout $(printf %q "$OUT")"
run "$SOCK" close heartbeat unarchive hb-1
want_counts "close" POST /cli/heartbeat/unarchive 1 0
[ "$RC" != 0 ] && ok "close: exit $RC (not 0)" || bad "close: exit 0"
want_unanswered "close" /cli/heartbeat/unarchive

echo "== 2b. 75 (the OS refused the connect) resends; 76 (TMPDIR unusable) never does =="
# A socket file this user may not open: connect() fails with EACCES, curl 7.
# The CLI's -v trace shows "Permission denied", which it names 75 (a local
# restriction; nothing was sent). The probe reads the same trace.
chmod 000 "$SOCK"
PRC=0
PERR="$(curl -sS -v -o /dev/null --unix-socket "$SOCK" --max-time 2 -X POST --data-raw x \
    "http://localhost/cli/heartbeat/unarchive" 2>&1)" || PRC=$?
if [ "$PRC" = 7 ] && [[ "$PERR" == *"Permission denied"* ]]; then
    ok "closed socket file: curl 7, Permission denied in the -v trace"
else
    bad "closed socket file: curl $PRC, $(printf %q "$PERR") (running as root?)"
fi
run "$SOCK" close heartbeat unarchive hb-1
chmod 755 "$SOCK"
want_counts "75" POST /cli/heartbeat/unarchive 0 1
[ "$RC" = 0 ] && ok "75: exit 0" || bad "75: exit $RC ($ERR)"
[ "$OUT" = "Unarchived heartbeat: hb-1" ] && ok "75: the TCP answer prints" || bad "75: stdout $(printf %q "$OUT")"
no_unanswered "75"
# The socket closes with no reply (curl 52) while TMPDIR can't be written:
# the CLI names that 76. The request went out, so it is not sent again.
RO_TMP="$SANDBOX/ro-tmp"
mkdir -p "$RO_TMP"
chmod 555 "$RO_TMP"
[ ! -w "$RO_TMP" ] && ok "the read-only TMPDIR is not writable" || bad "the read-only TMPDIR is writable (running as root?)"
RUN_TMPDIR="$RO_TMP" run "$SOCK" close heartbeat unarchive hb-1
chmod 755 "$RO_TMP"
want_counts "76" POST /cli/heartbeat/unarchive 1 0
[ "$RC" != 0 ] && ok "76: exit $RC (not 0)" || bad "76: exit 0"
want_unanswered "76" /cli/heartbeat/unarchive

echo "== 3. cli_send GET (k2 inbox read) still falls back =="
run "$SOCK" close inbox read item-1
want_counts "GET close" GET /cli/inbox/read 1 1
[ "$RC" = 0 ] && ok "GET close: exit 0" || bad "GET close: exit $RC ($ERR)"
[ "$OUT" = "from tcp" ] && ok "GET close: the TCP answer prints" || bad "GET close: stdout $(printf %q "$OUT")"
no_unanswered "GET close"

echo "== 4. cli_send_json POST (k2 agent identity --color) =="
run "$DEAD" close agent identity --color '#22c55e'
want_counts "curl 7" POST /cli/agent/identity 0 1
[ "$RC" = 0 ] && ok "curl 7: exit 0" || bad "curl 7: exit $RC ($ERR)"
no_unanswered "curl 7"
for mode in hang close reset garbage; do
    run "$SOCK" "$mode" agent identity --color '#22c55e'
    want_counts "$mode" POST /cli/agent/identity 1 0
    [ "$RC" = 1 ] && ok "$mode: exit 1" || bad "$mode: exit $RC, want 1"
    [ -z "$OUT" ] && ok "$mode: stdout empty" || bad "$mode: stdout $(printf %q "$OUT")"
    want_unanswered "$mode" /cli/agent/identity
done
# A status line came back (200, empty body): the daemon answered, so it is
# the caller's answer to read, and it is not sent again.
run "$SOCK" empty agent identity --color '#22c55e'
want_counts "empty" POST /cli/agent/identity 1 0
no_unanswered "empty"
run "$SOCK" notserved agent identity --color '#22c55e'
want_counts "notserved" POST /cli/agent/identity 1 1
[ "$RC" = 0 ] && ok "notserved: exit 0" || bad "notserved: exit $RC ($ERR)"
no_unanswered "notserved"

echo "== 5. cli_send_json GET (k2 agent identity) still falls back =="
run "$SOCK" close agent identity
want_counts "GET close" GET /cli/agent/identity 1 1
[ "$RC" = 0 ] && ok "GET close: exit 0" || bad "GET close: exit $RC ($ERR)"
case "$OUT" in "Agent scout"*) ok "GET close: the TCP answer prints" ;; *) bad "GET close: stdout $(printf %q "$OUT")" ;; esac
no_unanswered "GET close"

echo
echo "cell_socket_post_no_resend: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
