#!/usr/bin/env bash
# A K2 Connect request and a browser open that may have reached K2 are
# sent once (0.46.15).
#
# 1. K2 Connect (_connect_curl_daemon, POST /cli/connect/proxy): the
#    daemon makes the control-plane call. The CLI tries the cell socket,
#    then loopback TCP, then (with a token in tunnel.json) the control
#    plane itself. For a request that isn't a GET:
#      socket -> TCP only after curl 7 (the connect failed; 75 when the
#        socket file is closed to this user is curl 7 here) or the
#        daemon's "not served on cell socket";
#      TCP -> control plane only after curl 7 on TCP (or a daemon too
#        old to have the route: its 404).
#    Any other failure (curl 28, 52, 56, a reply that isn't HTTP, 52 while
#    TMPDIR can't be written) ends with the one line
#      Error: K2's server didn't answer /cli/connect/proxy (<n> s); the request may have been applied. Check before running it again.
#    and exit 1, and nothing goes on. An empty 200 is an answer. GETs keep
#    both fallbacks.
#    Commands: k2 publish subdomain create acme --target ... (POST),
#    k2 publish subdomain rm skin (DELETE, the one label with no local
#    owner check, so it runs with no daemon on TCP), k2 publish subdomain
#    list (GET).
# 2. scripts/k2-open (POST /cli/browser/open-url): socket, then the owner
#    arm over TCP, then the system opener. An arm hands on only after
#    curl 7 or an error status (curl 22, the "not served" 404 among
#    them). Any other failure: one line on stderr, exit 1, no other arm.
#
# A stub daemon serves a Unix socket and loopback TCP (each with its own
# mode file) and a third listener stands in for the control plane. Each
# counts arrivals per route. Only the route under test misbehaves; every
# other route answers. Modes: ok, hang (curl 28), close (52), reset (56;
# Apple's curl says 52), empty (rc 0, empty 200), garbage (not HTTP),
# notserved (the daemon's 404). A second socket file is bound and never
# listens: connect fails (curl 7). A probe runs curl against each mode
# first, so each case is the exit code it names.
#
# The CLI's own time limit on the proxy call is 45 s; a curl wrapper first
# on PATH adds --max-time 2 (curl keeps the last one) so hang is quick.
#
# Hermetic: throwaway HOME, every K2_*/K2SO_* var unset, a python stub. No
# real daemon, ~/.k2 or keychain. Opens ports: run it on K2 Compute.
#
#   bash tests/cli/connect_open_no_resend.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
K2_OPEN="${K2_OPEN_UNDER_TEST:-$PROJECT_ROOT/scripts/k2-open}"
[ -x "$K2" ] || { echo "FAIL: $K2 not executable" >&2; exit 1; }
[ -f "$K2_OPEN" ] || { echo "FAIL: $K2_OPEN not found" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
REAL_CURL="$(command -v curl)" || { echo "FAIL: curl required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY BROWSER

# Short: a Unix socket path must stay under ~104 bytes.
SANDBOX="$(mktemp -d /tmp/k2cn-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
PIDS=()
cleanup() {
    local p
    for p in "${PIDS[@]:-}"; do
        [ -n "$p" ] && kill "$p" 2>/dev/null || true
    done
    chmod -R u+rwx "$SANDBOX" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, os, socket, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

LOG, PORTF, CPPORTF, MODE_UDS, MODE_TCP, SOCK, DEAD = sys.argv[1:8]
TARGETS = ("/cli/connect/proxy", "/cli/browser/open-url")
log_lock = threading.Lock()

def log(via, method, path):
    with log_lock, open(LOG, "a") as f:
        f.write("%s %s %s\n" % (via, method, path))

def http(status, reason, obj):
    body = json.dumps(obj).encode()
    return ("HTTP/1.1 %d %s\r\nContent-Type: application/json\r\nContent-Length: %d\r\n"
            "Connection: close\r\n\r\n" % (status, reason, len(body))).encode() + body

CP_ANSWERS = {
    "POST": {"host": "acme.box.k2.dev", "label": "acme", "target": "localhost:3000",
             "status": "active", "cert_status": "issued", "effective": "live"},
    "GET": {"subdomains": [{"host": "acme.box.k2.dev", "label": "acme", "target": "localhost:3000",
                            "tier": "pro", "status": "active"}]},
    "DELETE": {"removed": "skin.box.k2.dev"},
}

def answer(path, body):
    if path == "/cli/connect/proxy":
        req = json.loads(body or b"{}")
        return {"status": 200, "body": json.dumps(CP_ANSWERS[req["method"]])}
    if path.startswith("/cli/publish/subdomain/"):
        return {"success": True}
    return {"ok": True}

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
    return head.split(b"\r\n")[0].decode(), rest

def serve(conn, via, modef):
    mode = open(modef).read().strip()
    try:
        body = None
        if mode == "reset":
            time.sleep(0.3)
            line = conn.recv(65536, socket.MSG_PEEK).split(b"\r\n")[0].decode()
        else:
            got = read_request(conn)
            if got is None:
                return
            line, body = got
        method, target = line.split(" ")[0], line.split(" ")[1]
        path = urlsplit(target).path
        if mode == "reset" and path in TARGETS:
            log(via, method, path)
            return
        if body is None:
            line, body = read_request(conn)
        log(via, method, path)
        if path not in TARGETS or mode == "ok":
            if via == "uds" and path not in TARGETS:
                conn.sendall(http(404, "Not Found", {"error": "route not served on cell socket"}))
            else:
                conn.sendall(http(200, "OK", answer(path, body)))
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

def loop(s, via, modef):
    s.listen(16)
    while True:
        c, _ = s.accept()
        threading.Thread(target=serve, args=(c, via, modef), daemon=True).start()

u = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
u.bind(SOCK)
dead = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
dead.bind(DEAD)
t = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
t.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
t.bind(("127.0.0.1", 0))

# The control plane: counts and answers.
class CP(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        log("cp", self.command, urlsplit(self.path).path)
        out = json.dumps(CP_ANSWERS[self.command]).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(out)
    do_GET = do_POST = do_DELETE = do_PUT = handle_any

cp = ThreadingHTTPServer(("127.0.0.1", 0), CP)
cp.daemon_threads = True
threading.Thread(target=loop, args=(u, "uds", MODE_UDS), daemon=True).start()
threading.Thread(target=loop, args=(t, "tcp", MODE_TCP), daemon=True).start()
with open(CPPORTF, "w") as f:
    f.write(str(cp.server_address[1]))
with open(PORTF, "w") as f:
    f.write(str(t.getsockname()[1]))
cp.serve_forever()
PY

LOG="$SANDBOX/requests.log"
MODE_UDS="$SANDBOX/mode.uds"
MODE_TCP="$SANDBOX/mode.tcp"
SOCK="$SANDBOX/c.sock"
DEAD="$SANDBOX/d.sock"
: > "$LOG"
echo ok > "$MODE_UDS"
echo ok > "$MODE_TCP"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$SANDBOX/cpport" "$MODE_UDS" "$MODE_TCP" "$SOCK" "$DEAD" &
PIDS+=($!)
for _ in $(seq 1 100); do [ -s "$SANDBOX/port" ] && [ -s "$SANDBOX/cpport" ] && [ -S "$SOCK" ] && [ -S "$DEAD" ] && break; sleep 0.05; done
[ -s "$SANDBOX/port" ] && [ -s "$SANDBOX/cpport" ] && [ -S "$SOCK" ] && [ -S "$DEAD" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"
CPPORT="$(cat "$SANDBOX/cpport")"
# A loopback port nobody listens on: TCP connect fails (curl 7).
CLOSED_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"

# curl with a short time limit; the CLI's 45 s on the proxy call is the
# earlier --max-time, so this one wins.
mkdir -p "$SANDBOX/bin"
printf '#!/bin/sh\nexec "%s" "$@" --max-time 2\n' "$REAL_CURL" > "$SANDBOX/bin/curl"
chmod 755 "$SANDBOX/bin/curl"
# The system opener k2-open falls through to on Linux logs instead of
# opening: /usr/bin/open where a distro ships it (xdg-open under another
# name, which runs $BROWSER), else the next xdg-open on PATH (this one).
printf '#!/bin/sh\necho "opener $*" >> "%s"\n' "$LOG" > "$SANDBOX/bin/opener-log"
chmod 755 "$SANDBOX/bin/opener-log"
cp "$SANDBOX/bin/opener-log" "$SANDBOX/bin/xdg-open"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2" "$SANDBOX/ws/.k2"
# A token in tunnel.json: with no daemon, the CLI calls the control plane.
printf '{"token":"tunnel-test-token","subdomain":"box"}\n' > "$HOME/.k2/tunnel.json"
cd "$SANDBOX/ws"
RO_TMP="$SANDBOX/ro-tmp"
mkdir -p "$RO_TMP"

pass=0
fail=0
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $*" >&2; fail=$((fail + 1)); }

echo "== 0. probe: each mode is the curl exit it names =="
probe() { # probe <sock|tcp> <mode>
    local where="$1" mode="$2"
    : > "$LOG"
    PRC=0
    if [ "$where" = tcp ]; then
        echo "$mode" > "$MODE_TCP"
        PCODE="$("$REAL_CURL" -s -o "$SANDBOX/probe.body" -w '%{http_code}' --max-time 2 \
            -X POST --data-raw '{}' "http://127.0.0.1:${PORT}/cli/connect/proxy")" || PRC=$?
        echo ok > "$MODE_TCP"
    else
        echo "$mode" > "$MODE_UDS"
        PCODE="$("$REAL_CURL" -s -o "$SANDBOX/probe.body" -w '%{http_code}' --unix-socket "$where" --max-time 2 \
            -X POST --data-raw '{}' "http://localhost/cli/connect/proxy")" || PRC=$?
        echo ok > "$MODE_UDS"
    fi
}
probe "$DEAD" close;  [ "$PRC" = 7 ] && ok "dead socket: curl 7" || bad "dead socket: curl $PRC, want 7"
probe "$SOCK" hang;   [ "$PRC" = 28 ] && ok "socket hang: curl 28" || bad "socket hang: curl $PRC, want 28"
probe "$SOCK" close;  [ "$PRC" = 52 ] && ok "socket close: curl 52" || bad "socket close: curl $PRC, want 52"
probe "$SOCK" reset
case "$PRC" in 56|52) ok "socket reset: curl $PRC" ;; *) bad "socket reset: curl $PRC, want 56 (or 52 from Apple's curl)" ;; esac
probe "$SOCK" empty
[ "$PRC" = 0 ] && [ "$PCODE" = 200 ] && [ ! -s "$SANDBOX/probe.body" ] && ok "socket empty: curl 0, status 200, empty body" \
    || bad "socket empty: curl $PRC, status $PCODE"
probe "$SOCK" garbage
case "$PRC" in 0|7|22|28|52|56) bad "socket garbage: curl $PRC, want another failure" ;; *) ok "socket garbage: curl $PRC" ;; esac
probe "$SOCK" notserved
[ "$PRC" = 0 ] && [ "$PCODE" = 404 ] && ok "socket notserved: curl 0, status 404" || bad "socket notserved: curl $PRC, status $PCODE"
probe tcp hang;  [ "$PRC" = 28 ] && ok "TCP hang: curl 28" || bad "TCP hang: curl $PRC, want 28"
probe tcp close; [ "$PRC" = 52 ] && ok "TCP close: curl 52" || bad "TCP close: curl $PRC, want 52"
probe tcp reset
case "$PRC" in 56|52) ok "TCP reset: curl $PRC" ;; *) bad "TCP reset: curl $PRC, want 56 (or 52)" ;; esac
PRC=0
"$REAL_CURL" -s -o /dev/null --max-time 2 "http://127.0.0.1:${CLOSED_PORT}/" || PRC=$?
[ "$PRC" = 7 ] && ok "closed port: curl 7" || bad "closed port: curl $PRC, want 7"

# run <sock-path> <socket mode> <TCP mode> <k2 args…>: an agent in a K2
# terminal (passport, cell socket). Sets OUT, ERR, RC. RUN_TMPDIR, when
# set, is its TMPDIR; RUN_PORT, when set, its daemon port.
run() {
    local sock="$1" umode="$2" tmode="$3"; shift 3
    echo "$umode" > "$MODE_UDS"
    echo "$tmode" > "$MODE_TCP"
    : > "$LOG"
    set +e
    env PATH="$SANDBOX/bin:$PATH" TMPDIR="${RUN_TMPDIR:-${TMPDIR:-/tmp}}" \
        K2_HOST=127.0.0.1 K2_PORT="${RUN_PORT:-$PORT}" K2_HOOK_TOKEN=test-session.passport \
        K2_HOOK_SOCK="$sock" K2_PROJECT_PATH="$SANDBOX/ws" \
        K2_CONNECT_BASE="http://127.0.0.1:${CPPORT}" \
        "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    RC=$?
    set -e
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
}
count() { grep -c "^$1 $2 $3\$" "$LOG" || true; }
# want3 <label> <socket> <TCP> <control plane>: arrivals of the route
# under test (POST /cli/connect/proxy on the daemon, any /subdomains call
# on the control plane).
want3() {
    local label="$1" u t c got_u got_t got_c
    u="$2"; t="$3"; c="$4"
    got_u="$(count uds POST /cli/connect/proxy)"
    got_t="$(count tcp POST /cli/connect/proxy)"
    got_c="$(grep -cE '^cp [A-Z]+ /subdomains' "$LOG" || true)"
    if [ "$got_u/$got_t/$got_c" = "$u/$t/$c" ]; then
        ok "$label: socket $u, TCP $t, control plane $c"
    else
        bad "$label: socket $got_u, TCP $got_t, control plane $got_c, want $u, $t, $c ($(tr '\n' ';' < "$LOG"))"
    fi
}
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
want_rc() {
    [ "$RC" = "$2" ] && ok "$1: exit $2" || bad "$1: exit $RC, want $2 ($(printf %q "$ERR"))"
}

CREATE=(publish subdomain create acme --target localhost:3000)

echo "== 1. Connect proxy, a POST: the cell socket's cases =="
run "$DEAD" ok ok "${CREATE[@]}"
want3 "socket curl 7" 0 1 0
want_rc "socket curl 7" 0
case "$OUT" in *"Created acme.box.k2.dev"*) ok "socket curl 7: the TCP answer prints" ;; *) bad "socket curl 7: stdout $(printf %q "$OUT")" ;; esac
no_unanswered "socket curl 7"
for mode in hang close reset garbage; do
    run "$SOCK" "$mode" ok "${CREATE[@]}"
    want3 "socket $mode" 1 0 0
    want_rc "socket $mode" 1
    [ -z "$OUT" ] && ok "socket $mode: stdout empty" || bad "socket $mode: stdout $(printf %q "$OUT")"
    want_unanswered "socket $mode" /cli/connect/proxy
done
# A status line came back (200, empty body): the daemon answered. It is
# read (and refused as no reply it knows), and not sent again.
run "$SOCK" empty ok "${CREATE[@]}"
want3 "socket empty 200" 1 0 0
want_rc "socket empty 200" 1
no_unanswered "socket empty 200"
run "$SOCK" notserved ok "${CREATE[@]}"
want3 "socket notserved" 1 1 0
want_rc "socket notserved" 0
case "$OUT" in *"Created acme.box.k2.dev"*) ok "socket notserved: the TCP answer prints" ;; *) bad "socket notserved: stdout $(printf %q "$OUT")" ;; esac
no_unanswered "socket notserved"
# 75: the socket file closed to this user; the connect fails (curl 7).
chmod 000 "$SOCK"
run "$SOCK" close ok "${CREATE[@]}"
chmod 755 "$SOCK"
want3 "75 (socket file closed)" 0 1 0
want_rc "75 (socket file closed)" 0
no_unanswered "75 (socket file closed)"
# 76: no reply (curl 52) while TMPDIR can't be written. It went out.
chmod 555 "$RO_TMP"
[ ! -w "$RO_TMP" ] && ok "the read-only TMPDIR is not writable" || bad "the read-only TMPDIR is writable (running as root?)"
RUN_TMPDIR="$RO_TMP" run "$SOCK" close ok "${CREATE[@]}"
chmod 755 "$RO_TMP"
want3 "76 (TMPDIR read-only)" 1 0 0
want_rc "76 (TMPDIR read-only)" 1
want_unanswered "76 (TMPDIR read-only)" /cli/connect/proxy

echo "== 2. Connect proxy, a POST: loopback TCP's cases (the socket's connect failed) =="
for mode in hang close reset garbage; do
    run "$DEAD" ok "$mode" "${CREATE[@]}"
    want3 "TCP $mode" 0 1 0
    want_rc "TCP $mode" 1
    [ -z "$OUT" ] && ok "TCP $mode: stdout empty" || bad "TCP $mode: stdout $(printf %q "$OUT")"
    want_unanswered "TCP $mode" /cli/connect/proxy
done
run "$DEAD" ok empty "${CREATE[@]}"
want3 "TCP empty 200" 0 1 0
want_rc "TCP empty 200" 1
no_unanswered "TCP empty 200"
# A daemon too old to have the route answers 404: the CLI calls the
# control plane itself, as before.
run "$DEAD" ok notserved "${CREATE[@]}"
want3 "TCP 404 (a daemon without the route)" 0 1 1
want_rc "TCP 404 (a daemon without the route)" 0
no_unanswered "TCP 404 (a daemon without the route)"
# No daemon on TCP either (curl 7): the control plane. rm skin has no
# local owner check, so it gets that far with no daemon.
RUN_PORT="$CLOSED_PORT" run "$DEAD" ok ok publish subdomain rm skin
want3 "TCP curl 7" 0 0 1
case "$OUT" in *"Removed skin.box.k2.dev"*) ok "TCP curl 7: the control plane's answer prints" ;; *) bad "TCP curl 7: stdout $(printf %q "$OUT")" ;; esac
no_unanswered "TCP curl 7"

echo "== 3. Connect proxy, a DELETE (k2 publish subdomain rm) =="
run "$SOCK" close ok publish subdomain rm skin
want3 "socket close" 1 0 0
want_rc "socket close" 1
want_unanswered "socket close" /cli/connect/proxy
run "$DEAD" ok close publish subdomain rm skin
want3 "TCP close" 0 1 0
want_rc "TCP close" 1
want_unanswered "TCP close" /cli/connect/proxy

echo "== 4. Connect proxy, a GET (k2 publish subdomain list) keeps both fallbacks =="
run "$SOCK" close ok publish subdomain list
want3 "GET socket close" 1 1 0
want_rc "GET socket close" 0
case "$OUT" in *acme.box.k2.dev*) ok "GET socket close: the TCP answer prints" ;; *) bad "GET socket close: stdout $(printf %q "$OUT")" ;; esac
no_unanswered "GET socket close"
run "$DEAD" ok close publish subdomain list
want3 "GET TCP close" 0 1 1
want_rc "GET TCP close" 0
case "$OUT" in *acme.box.k2.dev*) ok "GET TCP close: the control plane's answer prints" ;; *) bad "GET TCP close: stdout $(printf %q "$OUT")" ;; esac
no_unanswered "GET TCP close"

# open <sock-path> <socket mode> <TCP mode>: the shim in a K2 terminal
# (passport, cell socket), the owner's port and token on disk.
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf 'owner-test-token' > "$HOME/.k2/heartbeat.token"
URL="https://example.com/a?b=c"
open_url() {
    local sock="$1" umode="$2" tmode="$3"
    echo "$umode" > "$MODE_UDS"
    echo "$tmode" > "$MODE_TCP"
    : > "$LOG"
    set +e
    env PATH="$SANDBOX/bin:$PATH" TMPDIR="${RUN_TMPDIR:-${TMPDIR:-/tmp}}" \
        K2_HOOK_TOKEN=test-session.passport K2_HOOK_SOCK="$sock" BROWSER="$SANDBOX/bin/opener-log" \
        /bin/sh "$K2_OPEN" "$URL" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    RC=$?
    set -e
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
}
# want_open <label> <socket> <TCP> <system opener>
want_open() {
    local label="$1" got_u got_t got_o
    got_u="$(count uds POST /cli/browser/open-url)"
    got_t="$(count tcp POST /cli/browser/open-url)"
    got_o="$(grep -c '^opener ' "$LOG" || true)"
    if [ "$got_u/$got_t/$got_o" = "$2/$3/$4" ]; then
        ok "$label: socket $2, TCP $3, system opener $4"
    else
        bad "$label: socket $got_u, TCP $got_t, system opener $got_o, want $2, $3, $4 ($(tr '\n' ';' < "$LOG"))"
    fi
}
OPEN_LINE="k2-open: K2's server didn't answer /cli/browser/open-url; the URL may have been opened. Check before opening it again."
want_open_line() {
    [ "$ERR" = "$OPEN_LINE" ] && ok "$1: the one line" || bad "$1: stderr $(printf %q "$ERR")"
}

echo "== 5. k2-open: the cell socket's cases =="
# On macOS the shim's last arm is Apple's /usr/bin/open, which would
# really open a browser: Linux only.
[ "$(uname -s)" = Linux ] && ok "Linux: the system opener logs" || bad "run this on Linux (macOS's /usr/bin/open opens a browser)"
open_url "$DEAD" ok ok
want_open "socket curl 7" 0 1 0
want_rc "socket curl 7" 0
for mode in hang close reset garbage; do
    open_url "$SOCK" "$mode" ok
    want_open "socket $mode" 1 0 0
    want_rc "socket $mode" 1
    want_open_line "socket $mode"
done
open_url "$SOCK" empty ok
want_open "socket empty 200" 1 0 0
want_rc "socket empty 200" 0
open_url "$SOCK" notserved ok
want_open "socket notserved" 1 1 0
want_rc "socket notserved" 0
chmod 000 "$SOCK"
open_url "$SOCK" close ok
chmod 755 "$SOCK"
want_open "75 (socket file closed)" 0 1 0
want_rc "75 (socket file closed)" 0
chmod 555 "$RO_TMP"
RUN_TMPDIR="$RO_TMP" open_url "$SOCK" close ok
chmod 755 "$RO_TMP"
want_open "76 (TMPDIR read-only)" 1 0 0
want_rc "76 (TMPDIR read-only)" 1
want_open_line "76 (TMPDIR read-only)"

echo "== 6. k2-open: the owner arm's cases (the socket's connect failed) =="
for mode in hang close garbage; do
    open_url "$DEAD" ok "$mode"
    want_open "TCP $mode" 0 1 0
    want_rc "TCP $mode" 1
    want_open_line "TCP $mode"
done
open_url "$DEAD" ok notserved
want_open "TCP 404" 0 1 1
want_rc "TCP 404" 0
printf '%s' "$CLOSED_PORT" > "$HOME/.k2/heartbeat.port"
open_url "$DEAD" ok ok
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
want_open "TCP curl 7" 0 0 1
want_rc "TCP curl 7" 0

echo
echo "connect_open_no_resend: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
