#!/usr/bin/env bash
# _cli_curl sends a timed-out request again only when the connect never
# completed (k2-ptyd S5 P47), and never once the request reached the
# daemon (the 0.46.15 send-once rule).
#
# It used to read curl's -v trace for "Connected to". Newer curl words
# that line differently, so a request that reached the daemon and then
# timed out looked like a connect timeout and was sent again: a POST such
# as `k2 done` arrived twice. The test now is curl's own -w number
# (%{time_connect} is 0 when no connect completed), and a POST whose
# connect state is unknown is never sent again.
#
# Runs _cli_curl from cli/k2 (its HTTP helper and curl config parts)
# against local listeners, with a curl shim on PATH that counts each run:
#   1. a TCP server that accepts and never answers: POST and GET are sent
#      once, and the server sees exactly one request;
#   2. the same over a Unix socket (the cell socket's path);
#   3. a connect that times out (a listener whose backlog is full, or on
#      macOS a documentation address nothing answers): GET and POST are
#      tried $_K2_CURL_TRIES times;
#   4. a -w the helper can't extend (-w@file), so the connect state is
#      unknown: a POST is sent once, a GET still tries again;
#   5. a normal answer: the caller's bytes are unchanged (no marker);
#   6. the decision itself (_cli_connect_none) on fixed traces.
# No daemon, no real ~/.k2 (HOME is a temp dir). The only packets that
# leave the machine are unanswered connects to an RFC 5737 address, and
# only where a loopback connect can't be made to time out (macOS).
# Run with: bash tests/cli/curl_connect_retry.sh
# bash-only: runs functions from the bash CLI's source.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
REAL_CURL="$(command -v curl)" || { echo "FAIL: curl required" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }

# Short base: a Unix socket path is capped near 104 bytes on macOS.
W="$(mktemp -d /tmp/k2ccr-XXXXXX)"
STUB=""
cleanup() {
    [ -n "$STUB" ] && kill "$STUB" 2>/dev/null || true
    rm -rf "$W"
}
trap cleanup EXIT
mkdir -p "$W/home/.k2" "$W/bin"

# The functions under test: the curl config block and the HTTP helper.
LIB="$W/lib.sh"
# The prelude's two names first, as the CLI sets them: the curl block's
# hint fill reads both.
grep -E '^_K2_(CLI|PRODUCT)_NAME=' "$CLI_BASH_SOURCE" >"$LIB"
sed -n '/^# BEGIN_CLI_CURL_CFG$/,/^# END_CLI_CURL_CFG$/p' "$CLI_BASH_SOURCE" >>"$LIB"
sed -n '/^# HTTP helper$/,/^# BEGIN_CLI_NO_ANSWER$/p' "$CLI_BASH_SOURCE" >>"$LIB"
for f in _k2_curl _cli_curl _cli_transport_failure _cli_connect_none; do
    grep -q "^$f() {" "$LIB" || { echo "FAIL: no $f in $CLI_BASH_SOURCE" >&2; exit 1; }
done

# The shim: one line per curl run, then the real curl.
cat >"$W/bin/curl" <<EOF
#!/bin/sh
echo run >>"$W/curl.runs"
exec "$REAL_CURL" "\$@"
EOF
chmod +x "$W/bin/curl"

# Listeners. "stall": reads one request, logs its path, never answers
# (except /ok, a normal 200). "full": a backlog of 0 filled by
# connections nobody accepts, so a new connect is never completed.
python3 - "$W" <<'PY' &
import os, socket, sys, threading, time
w = sys.argv[1]
lock = threading.Lock()
held = []

def serve(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(65536)
        if not chunk:
            return
        data += chunk
    line = data.split(b"\r\n", 1)[0].decode("latin-1")
    parts = line.split(" ")
    path = parts[1].split("?", 1)[0] if len(parts) > 1 else "?"
    with lock:
        with open(os.path.join(w, "arrivals.log"), "a") as f:
            f.write(parts[0] + " " + path + "\n")
    if path == "/ok":
        conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
        conn.close()
        return
    held.append(conn)

def accept_loop(s):
    while True:
        c, _ = s.accept()
        threading.Thread(target=serve, args=(c,), daemon=True).start()

tcp = socket.socket()
tcp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
tcp.bind(("127.0.0.1", 0))
tcp.listen(16)
uds = socket.socket(socket.AF_UNIX)
uds.bind(os.path.join(w, "s.sock"))
uds.listen(16)
full = socket.socket()
full.bind(("127.0.0.1", 0))
full.listen(0)
fp = full.getsockname()[1]
fill = []
for _ in range(6):
    c = socket.socket()
    c.setblocking(False)
    try:
        c.connect(("127.0.0.1", fp))
    except (BlockingIOError, OSError):
        pass
    fill.append(c)
time.sleep(0.5)
for s in (tcp, uds):
    threading.Thread(target=accept_loop, args=(s,), daemon=True).start()
with open(os.path.join(w, "ports.tmp"), "w") as f:
    f.write("%d %d\n" % (tcp.getsockname()[1], fp))
os.rename(os.path.join(w, "ports.tmp"), os.path.join(w, "ports"))
while True:
    time.sleep(60)
PY
STUB=$!
for _ in $(seq 1 100); do [ -s "$W/ports" ] && break; sleep 0.05; done
[ -s "$W/ports" ] || { echo "FAIL: the listeners never started" >&2; exit 1; }
read -r SP FP <"$W/ports"

# The connect timeout must be real here: curl times out (28) and says no
# connect completed. Without that, rows 3 and 4 would prove nothing.
# Linux drops a new connect to the full backlog; macOS completes it, so
# there it is a documentation address nothing answers (RFC 5737).
CT=""
for target in "127.0.0.1:$FP" "192.0.2.1:9" "198.51.100.1:9" "203.0.113.1:9"; do
    pre_rc=0
    pre="$("$REAL_CURL" -s -o /dev/null --connect-timeout 1 --max-time 3 -w '%{time_connect}' "http://$target/x")" || pre_rc=$?
    if [ "$pre_rc" = 28 ] && [ -n "$pre" ] && [ "$(printf '%s' "$pre" | tr -d '0.,')" = "" ]; then
        CT="$target"
        break
    fi
    echo "  note: $target gives no connect timeout here (curl exit $pre_rc, time_connect $pre)"
done
[ -n "$CT" ] || { echo "FAIL: no address on this machine gives a connect timeout" >&2; exit 1; }
echo "  connect timeouts go to $CT"

# cc ARGS…: _cli_curl as the request helpers call it, in a clean shell.
# Sets $rc, $out (stdout), $runs (curl runs), $arr (requests that arrived).
cc() {
    : >"$W/curl.runs"
    : >"$W/arrivals.log"
    rc=0
    out="$(env -i PATH="$W/bin:$PATH" HOME="$W/home" TMPDIR="$W" LC_ALL=C \
        bash -c 'set -euo pipefail; . "$1"; HOST=127.0.0.1; PORT="$2"; BASE_URL="http://${HOST}:${PORT}"; shift 2; _cli_curl --k2-cfg "" "$@"' \
        _ "$LIB" "$SP" "$@" 2>"$W/cc.err")" || rc=$?
    # A held request is logged as it arrives; let a late write land.
    sleep 0.2
    runs="$(grep -c . "$W/curl.runs" || true)"
    arr="$(grep -c . "$W/arrivals.log" || true)"
}
WS=$'\n__k2http:%{http_code}'
TRIES="$(sed -n 's/^_K2_CURL_TRIES=\([0-9][0-9]*\)$/\1/p' "$LIB")"
[ -n "$TRIES" ] && [ "$TRIES" -gt 1 ] || { echo "FAIL: no _K2_CURL_TRIES in $CLI_BASH_SOURCE" >&2; exit 1; }

echo "== 1. a TCP server that accepts and never answers =="
cc -sS -v -X POST -w "$WS" --connect-timeout 1 --max-time 2 --data-raw "" "http://127.0.0.1:$SP/cli/done"
assert_eq "POST: curl's timeout is the answer" "$rc" "28"
assert_eq "POST: curl ran once" "$runs" "1"
assert_eq "POST: exactly one request reached the server" "$arr" "1"
cc -sS -v -G -w "$WS" --connect-timeout 1 --max-time 2 "http://127.0.0.1:$SP/cli/read"
assert_eq "GET: curl's timeout is the answer" "$rc" "28"
assert_eq "GET: curl ran once" "$runs" "1"
assert_eq "GET: exactly one request reached the server" "$arr" "1"

echo "== 2. a Unix socket that accepts and never answers =="
cc -sS -v -X POST -w "$WS" --unix-socket "$W/s.sock" --connect-timeout 1 --max-time 2 --data-raw "" "http://localhost/cli/done"
assert_eq "POST: curl's timeout is the answer" "$rc" "28"
assert_eq "POST: curl ran once" "$runs" "1"
assert_eq "POST: exactly one request reached the socket" "$arr" "1"

echo "== 3. a connect that times out =="
cc -sS -v -G -w "$WS" --connect-timeout 1 --max-time 3 "http://$CT/cli/read"
assert_eq "GET: exit 28 after the tries" "$rc" "28"
assert_eq "GET: tried $TRIES times" "$runs" "$TRIES"
cc -sS -v -X POST -w "$WS" --connect-timeout 1 --max-time 3 --data-raw "" "http://$CT/cli/done"
assert_eq "POST: exit 28 after the tries" "$rc" "28"
assert_eq "POST: tried $TRIES times (it never connected)" "$runs" "$TRIES"

echo "== 4. the connect state is unknown (a -w the helper can't extend) =="
printf '%s' "$WS" >"$W/wfmt"
cc -sS -v -X POST "-w@$W/wfmt" --connect-timeout 1 --max-time 3 --data-raw "" "http://$CT/cli/done"
assert_eq "POST: exit 28" "$rc" "28"
assert_eq "POST: sent once, never again" "$runs" "1"
cc -sS -v -G "-w@$W/wfmt" --connect-timeout 1 --max-time 3 "http://$CT/cli/read"
assert_eq "GET: exit 28 after the tries" "$rc" "28"
assert_eq "GET: tried $TRIES times" "$runs" "$TRIES"

echo "== 5. a normal answer keeps its bytes =="
cc -sS -v -X POST -w "$WS" --connect-timeout 1 --max-time 3 --data-raw "" "http://127.0.0.1:$SP/ok"
assert_eq "exit 0" "$rc" "0"
assert_eq "stdout is the body and the caller's -w, nothing more" "$out" "{}"$'\n__k2http:200'
cc -sS -X POST --connect-timeout 1 --max-time 3 --data-raw "" "http://127.0.0.1:$SP/ok"
assert_eq "no -w: stdout is the body alone" "$out" "{}"

echo "== 6. the decision on fixed traces =="
none() {
    if bash -c '. "$1"; _cli_connect_none "$2" "$3"' _ "$LIB" "$1" "$2"; then echo yes; else echo no; fi
}
M=$'\n__k2conn:'
assert_eq "marker 0: never connected" "$(none "* Trying 127.0.0.1:9...${M}0.000000" 0)" "yes"
assert_eq "marker with a comma (a locale): 0 still counts" "$(none "* Trying${M}0,000000" 0)" "yes"
assert_eq "marker above 0: connected" "$(none "* Trying${M}0.000143" 0)" "no"
assert_eq "the last marker decides (an earlier refused try wrote its own)" "$(none "*${M}0.000000${M}0.000210" 0)" "no"
assert_eq "a request line in the trace: never again, whatever the marker" "$(none $'* x\n> POST /cli/done HTTP/1.1'"${M}0.000000" 0)" "no"
assert_eq "a marker that is not a number: unknown, a POST stays sent" "$(none "*${M}" 0)" "no"
assert_eq "no marker, a POST: never again" "$(none "* Trying 127.0.0.1:9..." 0)" "no"
assert_eq "no marker, a GET with no connect line: again" "$(none "* Trying 127.0.0.1:9..." 1)" "yes"
assert_eq "no marker, a GET after 'Connected to': not again" "$(none "* Connected to h (127.0.0.1) port 9" 1)" "no"
assert_eq "no marker, a GET after 'Established connection to': not again" "$(none "* Established connection to h (127.0.0.1 port 9) from 127.0.0.1 port 5" 1)" "no"

echo
echo "curl_connect_retry: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
