#!/usr/bin/env bash
# A port outside 1 to 65535 is refused before anything is sent.
#
# On Linux, glibc's getaddrinfo (behind Python's urllib and http.client)
# keeps only the low 16 bits of a port above 65535, so a request to
# P + 65536 lands on port P. Every row here runs against a recording
# listener on loopback port P and checks what reached it.
#
# Rows:
#   1. The platform: a plain Python request to P + 65536 reaches P on
#      Linux; on macOS the resolver refuses it. So on Linux the rows below
#      prove P + 65536 never reaches P.
#   2. The port setting (K2_PORT, and its K5_PORT and K2SO_PORT names): a
#      value that isn't a number from 1 to 65535 (above 65535, 0, a
#      leading zero, a sign, not a number) is refused: exit 2, stdout
#      empty, nothing reaches P, and one line on stderr naming the
#      variable and the value:
#        Error: K2_PORT must be a number from 1 to 65535 (got: <value>)
#      K2_PORT=P reaches P (the control).
#   3. The saved port (heartbeat.port, no setting): the same, with
#        Error: the port in <home>/.k2/heartbeat.port must be a number from 1 to 65535 (got: <value>)
#      heartbeat.port holding P reaches P (the control).
#   4. clone's server address (--host, `clone to <host>`, `clone from
#      <host>:<ws>`), its port read as urllib reads it (a bracketed IPv6
#      address, %-escapes decoded): exit 2, stdout empty, nothing reaches
#      P, and the one line
#        {"error": "usage", "hint": "the server's port must be a number from 1 to 65535 (got: <url>)"}
#      --host http://127.0.0.1:P reaches P (the control).
#
# The sending verb for rows 2 and 3 is `preset list`, which asks over
# Python's urllib (where the port is cut); `whoami` (curl) once each too.
#
# Hermetic: a throwaway HOME, every K2_*, K5_* and K2SO_* variable unset,
# no daemon. Opens a port: run it on K2 Compute.
#   bash tests/cli/port_range.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

for v in $(env | grep -E '^(K2|K5|K2SO)_' | cut -d= -f1); do unset "$v"; done
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY PYTHONPATH

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

W="$(mktemp -d -t k2-portrange-XXXXXX)"
W="$(cd "$W" && pwd -P)"
LPID=""
cleanup() {
    [ -n "$LPID" ] && kill "$LPID" 2>/dev/null || true
    rm -rf "$W"
}
trap cleanup EXIT

export HOME="$W/home"
KH="$HOME/.k2"
mkdir -p "$KH" "$W/ws" "$W/tmp"
printf 'owner-portrange-1111\n' >"$KH/heartbeat.token"
chmod 600 "$KH/heartbeat.token"

# The recording listener: one line per connection in $W/seen, written
# before it answers. A connection that sends "MARK" is logged as "mark"
# and answered "ok" (sync_listener); an HTTP request gets a 403
# {"error":"recorded"}; anything else is logged and closed.
cat >"$W/listener.py" <<'PY'
import os, socket, sys
w = sys.argv[1]
s = socket.socket()
s.bind(("127.0.0.1", 0))
s.listen(16)
with open(os.path.join(w, "port.tmp"), "w") as f:
    f.write(str(s.getsockname()[1]))
os.rename(os.path.join(w, "port.tmp"), os.path.join(w, "port"))
body = b'{"error":"recorded"}'
while True:
    c, _ = s.accept()
    c.settimeout(5)
    try:
        data = c.recv(4096)
    except OSError:
        data = b""
    if data.startswith(b"MARK"):
        line, reply = b"mark", b"ok"
    elif data[:1].isalpha():
        line = b"conn " + data.split(b"\r\n", 1)[0]
        reply = b"HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s" % (len(body), body)
    else:
        line, reply = b"conn not http: %d bytes" % len(data), b""
    with open(os.path.join(w, "seen"), "ab") as f:
        f.write(line + b"\n")
    try:
        c.sendall(reply)
    except OSError:
        pass
    c.close()
PY
: >"$W/seen"
python3 -I "$W/listener.py" "$W" >"$W/listener.log" 2>&1 &
LPID=$!
for _ in $(seq 1 50); do [ -s "$W/port" ] && break; sleep 0.1; done
[ -s "$W/port" ] || { echo "FAIL: the listener didn't start: $(head -c 300 "$W/listener.log")" >&2; exit 1; }
P="$(cat "$W/port")"
BIG=$((P + 65536))

# A loopback port with nothing on it: this server's port in the clone rows.
DEAD="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"

# sync_listener: a marker connection, answered once logged. The listener
# takes connections in the order they came, so once the marker is in
# $W/seen every connection made before it is there too.
sync_listener() {
    python3 -I -c '
import socket, sys
c = socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=10)
c.sendall(b"MARK")
if c.recv(16) != b"ok":
    sys.exit("the listener did not answer the marker")' "$P"
}

# conns: the connections that reached P since the last run.
conns() { grep -vc '^mark$' "$W/seen" || true; }

# run <stdin> [NAME=value ...] -- <args...>: the CLI in a clean
# environment from $W/ws. Sets RC; stdout and stderr in $W/out, $W/err.
run() {
    local stdin="$1" envs=()
    shift
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$W/seen"
    RC=0
    (cd "$W/ws" && printf '%s' "$stdin" | exec env -i PATH="$PATH" HOME="$HOME" TMPDIR="$W/tmp" \
        ${envs[@]+"${envs[@]}"} "$K2" "$@") >"$W/out" 2>"$W/err" || RC=$?
    sync_listener
}

# refused <label> <line>: exit 2, stdout empty, stderr the one line,
# nothing reached P.
refused() {
    local label="$1" want="$2" n
    if [ "$RC" = 2 ]; then ok "$label: exit 2"; else bad "$label: exit $RC, want 2 (stderr: $(head -c 300 "$W/err"))"; fi
    if printf '%s\n' "$want" | cmp -s - "$W/err"; then
        ok "$label: the refusal line"
    else
        bad "$label: stderr $(printf %q "$(head -c 400 "$W/err")"), want $(printf %q "$want")"
    fi
    if [ ! -s "$W/out" ]; then ok "$label: stdout empty"; else bad "$label: stdout $(head -c 200 "$W/out")"; fi
    n="$(conns)"
    if [ "$n" = 0 ]; then
        ok "$label: nothing reached port P"
    else
        bad "$label: $n connection(s) reached port P: $(grep -v '^mark$' "$W/seen" | head -c 300)"
    fi
}

# reached <label> <request start>: the control rows; at least one
# connection reached P, the first one this request.
reached() {
    local label="$1" want="$2" n first
    n="$(conns)"
    first="$(grep -v '^mark$' "$W/seen" | head -n 1)"
    case "$first" in
        "conn $want"*) ok "$label: reached port P ($n connection(s))" ;;
        *) bad "$label: want a '$want' at port P, saw $n: $(head -c 300 "$W/seen") (exit $RC, stderr: $(head -c 300 "$W/err"))" ;;
    esac
}

echo "== 1. the platform: a request to P + 65536 =="
: >"$W/seen"
prc=0
python3 -I -c '
import sys, urllib.request
urllib.request.urlopen("http://127.0.0.1:%s/platform" % sys.argv[1], timeout=5)' "$BIG" >"$W/out" 2>"$W/err" || prc=$?
sync_listener
os="$(uname -s)"
case "$os" in
    Linux)
        if grep -q '^conn GET /platform ' "$W/seen"; then
            ok "Linux: Python's request to P + 65536 reached P (the port is cut to 16 bits here)"
        else
            bad "Linux: Python's request to P + 65536 did not reach P (exit $prc: $(tail -n 1 "$W/err")); the rows below prove less here"
        fi
        ;;
    Darwin)
        if [ "$(conns)" = 0 ] && [ "$prc" != 0 ]; then
            ok "macOS: the resolver refuses P + 65536 (exit $prc), nothing reached P"
        else
            bad "macOS: Python's request to P + 65536 gave exit $prc and $(conns) connection(s) at P"
        fi
        ;;
    *) bad "an OS this test doesn't know: $os" ;;
esac

echo "== 2. the port setting =="
run "" K2_PORT="$P" -- preset list
reached "K2_PORT=P, preset list" "GET /cli/presets"
run "" K2_PORT="$BIG" -- preset list
refused "K2_PORT=P+65536, preset list" "Error: K2_PORT must be a number from 1 to 65535 (got: $BIG)"
run "" K2_PORT="$BIG" -- whoami
refused "K2_PORT=P+65536, whoami" "Error: K2_PORT must be a number from 1 to 65535 (got: $BIG)"
for v in 65536 0 "0$P" "+$P" -1 "$P " "${P}x" 99999999999999999999; do
    run "" K2_PORT="$v" -- preset list
    refused "K2_PORT=$(printf %q "$v")" "Error: K2_PORT must be a number from 1 to 65535 (got: $v)"
done
run "" K5_PORT="$BIG" K2_PORT="$P" -- preset list
refused "K5_PORT=P+65536 over K2_PORT=P" "Error: K5_PORT must be a number from 1 to 65535 (got: $BIG)"
run "" K2SO_PORT="$BIG" -- preset list
refused "K2SO_PORT=P+65536" "Error: K2SO_PORT must be a number from 1 to 65535 (got: $BIG)"
run "" K2_PORT=65535 -- preset list
if [ "$RC" != 2 ] && ! grep -q 'must be a number' "$W/err"; then ok "K2_PORT=65535 is a port (not refused)"; else bad "K2_PORT=65535 refused: exit $RC, $(head -c 300 "$W/err")"; fi

echo "== 3. the saved port (heartbeat.port) =="
printf '%s\n' "$P" >"$KH/heartbeat.port"
run "" -- preset list
reached "heartbeat.port=P, preset list" "GET /cli/presets"
printf '%s\n' "$BIG" >"$KH/heartbeat.port"
run "" -- preset list
refused "heartbeat.port=P+65536, preset list" "Error: the port in $KH/heartbeat.port must be a number from 1 to 65535 (got: $BIG)"
run "" -- whoami
refused "heartbeat.port=P+65536, whoami" "Error: the port in $KH/heartbeat.port must be a number from 1 to 65535 (got: $BIG)"
for v in 0 "0$P" abc; do
    printf '%s\n' "$v" >"$KH/heartbeat.port"
    run "" -- preset list
    refused "heartbeat.port=$v" "Error: the port in $KH/heartbeat.port must be a number from 1 to 65535 (got: $v)"
done
# The setting wins over the file, so the file isn't read or checked.
run "" K2_PORT="$P" -- preset list
reached "K2_PORT=P with heartbeat.port=abc" "GET /cli/presets"

echo "== 4. clone's server address =="
printf '%s\n' "$DEAD" >"$KH/heartbeat.port"
clone_hint() { printf '{"error": "usage", "hint": "the server'"'"'s port must be a number from 1 to 65535 (got: %s)"}' "$1"; }
run "tok-portrange" -- clone preview ws1 --host "http://127.0.0.1:$P" --token-stdin --json
reached "clone preview --host http://127.0.0.1:P" "POST /cli/clone/preview"
for url in "http://127.0.0.1:$BIG" "http://[::1]:$BIG" "https://127.0.0.1%3A$BIG" "https://localhost:0" "https://localhost:0$P" "http://127.0.0.1:+$P"; do
    run "tok-portrange" -- clone preview ws1 --host "$url" --token-stdin --json
    refused "clone preview --host $url" "$(clone_hint "$url")"
done
run "tok-portrange" -- clone preview ws1 --host "http://127.0.0.1:$BIG/" --token-stdin --json
refused "clone preview --host http://127.0.0.1:P+65536/" "$(clone_hint "http://127.0.0.1:$BIG")"
run "tok-portrange" -- clone preview ws1 --host "127.0.0.1:$BIG" --token-stdin --json
refused "clone preview --host 127.0.0.1:P+65536 (https)" "$(clone_hint "https://127.0.0.1:$BIG")"
run "tok-portrange" -- clone to "http://127.0.0.1:$BIG" ws1 --token-stdin --json
refused "clone to http://127.0.0.1:P+65536" "$(clone_hint "http://127.0.0.1:$BIG")"
run "tok-portrange" -- clone from "http://127.0.0.1:$BIG:ws1" --token-stdin --json
refused "clone from http://127.0.0.1:P+65536:ws1" "$(clone_hint "http://127.0.0.1:$BIG")"
# No port, or an empty one, is the scheme's own: not refused by this
# check (nothing listens on 443 here, so nothing reaches P either).
run "tok-portrange" -- clone preview ws1 --host "https://[::1]:" --token-stdin --json
if ! grep -q 'must be a number' "$W/err"; then ok "clone --host https://[::1]: (an empty port) is not refused as a port"; else bad "an empty port was refused: $(head -c 300 "$W/err")"; fi

echo
echo "port_range: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
