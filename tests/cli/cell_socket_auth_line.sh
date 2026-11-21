#!/usr/bin/env bash
# On the session's cell socket, the GET and the JSON POST helpers send an
# Authorization line only with a passport, never an empty Bearer, as the
# form POST helper (k2 done, tests/cli/done_flags.sh) does.
#
#   cli_request_uds (k2 read <workspace>, GET /cli/terminal/read):
#     no passport: one GET on the socket with no Authorization line, and
#     the owner token on disk never sent. Regression (0.46.17): it sent an
#     empty `Authorization: Bearer`.
#     with a passport: `Authorization: Bearer <passport>`, once.
#   cli_post_json_uds (k2 msg <ws> --signal, POST /cli/awareness/publish):
#     with a passport: `Authorization: Bearer <passport>`, once.
#     no passport: msg is a passport tool, so it stops with exit 3 before
#     any request (no verb reaches this helper without a passport today;
#     the helper's own guard is there for the next one).
#
# Hermetic: temp HOME, a stub daemon on loopback (it must get nothing) and
# a stub cell socket that logs each request's raw head. No real ~/.k2.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORKDIR="$(mktemp -d -t k2-cell-auth-XXXXXX)"
# Short: a Unix socket path must stay under ~104 bytes.
SOCKDIR="$(mktemp -d /tmp/k2ca-XXXXXX)"
SOCK="$SOCKDIR/cell.sock"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORKDIR" "$SOCKDIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

python3 - "$WORKDIR" "$SOCK" <<'PY' &
import json, os, socket, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer

root, sock_path = sys.argv[1], sys.argv[2]
tcp_log = os.path.join(root, "tcp.jsonl")
uds_log = os.path.join(root, "uds.jsonl")

# The cell socket: each request's raw head and body, then the route's answer.
def serve_uds(conn):
    try:
        data = b""
        while b"\r\n\r\n" not in data:
            chunk = conn.recv(65536)
            if not chunk:
                return
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
        with open(uds_log, "a") as f:
            f.write(json.dumps({"head": head.decode(), "body": rest.decode()}) + "\n")
        target = head.split(b"\r\n")[0].split(b" ")[1].split(b"?")[0]
        if target == b"/cli/terminal/read":
            body = b'{"lines": ["hello from acme"]}'
        else:
            body = b'{"success": true, "ok": true}'
        conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n" % len(body) + body)
    finally:
        conn.close()

def uds_loop():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.bind(sock_path)
    s.listen(8)
    while True:
        c, _ = s.accept()
        threading.Thread(target=serve_uds, args=(c,), daemon=True).start()

threading.Thread(target=uds_loop, daemon=True).start()

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def answer(self):
        with open(tcp_log, "a") as f:
            f.write(json.dumps({"method": self.command, "target": self.path}) + "\n")
        body = b'{"error": "the test sends nothing over TCP"}'
        self.send_response(500)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = answer
    do_POST = answer

httpd = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(root, "stub.port"), "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do
    [ -s "$WORKDIR/stub.port" ] && [ -S "$SOCK" ] && break
    sleep 0.05
done
[ -s "$WORKDIR/stub.port" ] && [ -S "$SOCK" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }
export K2_PORT="$(cat "$WORKDIR/stub.port")"
export K2_AGENT_NAME=tester
# The owner token on disk passes the connection gate; it must never be sent
# on the session's socket, and with no passport nothing stands in for it.
mkdir -p "$HOME/.k2"
printf 'owner-secret-token\n' > "$HOME/.k2/heartbeat.token"

# run <env...> -- <k2 args...>: sets RC, OUT, ERR, TCP (one request per
# line) and HEADS (each socket request's head, then body=<body>, then --).
run() {
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : > "$WORKDIR/tcp.jsonl"
    : > "$WORKDIR/uds.jsonl"
    RC=0
    env ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" >"$WORKDIR/out" 2>"$WORKDIR/err" </dev/null || RC=$?
    OUT="$(cat "$WORKDIR/out")"
    ERR="$(cat "$WORKDIR/err")"
    TCP="$(cat "$WORKDIR/tcp.jsonl")"
    HEADS="$(python3 -c '
import json, sys
for l in open(sys.argv[1]):
    if l.strip():
        r = json.loads(l)
        print(r["head"].replace("\r\n", "\n"))
        print("body=" + r["body"])
        print("--")
' "$WORKDIR/uds.jsonl")"
}

# head_of <request line prefix>: the head of the first socket request whose
# request line starts with it, one header per line, or nothing.
head_of() {
    python3 -c '
import json, sys
for l in open(sys.argv[1]):
    if l.strip():
        head = json.loads(l)["head"]
        if head.startswith(sys.argv[2]):
            print(head.replace("\r\n", "\n"))
            break
' "$WORKDIR/uds.jsonl" "$1"
}

NOPASS=(-u K2_HOOK_TOKEN -u K5_HOOK_TOKEN K2_HOOK_SOCK="$SOCK")
PASS=(-u K5_HOOK_TOKEN K2_HOOK_TOKEN=pp-acme.s3cret K2_HOOK_SOCK="$SOCK")

echo "== cli_request_uds: k2 read on the cell socket =="
run "${NOPASS[@]}" -- read acme --lines 5
if [ "$RC" = 0 ] && [ "$OUT" = "hello from acme" ] && [ -z "$TCP" ] &&
    [ "$(grep -c '^GET /cli/terminal/read?' <<<"$HEADS")" = 1 ] &&
    ! grep -qi '^authorization:' <<<"$HEADS" && ! grep -q 'owner-secret-token' <<<"$HEADS"; then
    ok "read, no passport: one GET on the socket, no Authorization line, the owner token never sent"
else
    bad "read, no passport: rc=$RC out=[$OUT] tcp=[$TCP] socket=[$HEADS] stderr=[$ERR]"
fi

run "${PASS[@]}" -- read acme --lines 5
h="$(head_of 'GET /cli/terminal/read?')"
if [ "$RC" = 0 ] && [ "$OUT" = "hello from acme" ] && [ -z "$TCP" ] &&
    [ "$(grep -ci '^authorization:' <<<"$h")" = 1 ] && grep -qx 'Authorization: Bearer pp-acme.s3cret' <<<"$h" &&
    ! grep -q 'owner-secret-token' <<<"$HEADS"; then
    ok "read, with a passport: the Bearer line, once, on the GET"
else
    bad "read, with a passport: rc=$RC out=[$OUT] tcp=[$TCP] head=[$h] socket=[$HEADS] stderr=[$ERR]"
fi

echo "== cli_post_json_uds: k2 msg --signal on the cell socket =="
run "${PASS[@]}" -- msg acme --signal status --payload '{"text":"hi"}'
h="$(head_of 'POST /cli/awareness/publish ')"
if [ "$RC" = 0 ] && [ -z "$TCP" ] && [ "$(grep -c '^POST /cli/awareness/publish ' <<<"$HEADS")" = 1 ] &&
    [ "$(grep -ci '^authorization:' <<<"$h")" = 1 ] && grep -qx 'Authorization: Bearer pp-acme.s3cret' <<<"$h" &&
    grep -qi '^content-type: application/json$' <<<"$h" && ! grep -q 'owner-secret-token' <<<"$HEADS"; then
    ok "msg --signal, with a passport: one JSON POST, the Bearer line once"
else
    bad "msg --signal, with a passport: rc=$RC tcp=[$TCP] head=[$h] socket=[$HEADS] stderr=[$ERR]"
fi

run "${NOPASS[@]}" -- msg acme --signal status --payload '{"text":"hi"}'
if [ "$RC" = 3 ] && [ -z "$TCP" ] && [ -z "$HEADS" ] && grep -q "requires session ID (passport)" <<<"$ERR"; then
    ok "msg --signal, no passport: exit 3, nothing sent (a passport tool)"
else
    bad "msg --signal, no passport: rc=$RC tcp=[$TCP] socket=[$HEADS] stderr=[$ERR]"
fi

echo
echo "cell_socket_auth_line: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
