#!/usr/bin/env bash
# `k2 done` with a flag it doesn't take sends nothing.
#
# Regression: cmd_done skipped every word it didn't know, so
# `k2 done --help` (or a typo like `k2 done --blokced "x"`) marked the task
# done. Now --help/-h prints done's usage (exit 0) and any other unknown
# argument starting with - exits 2 with the reason on stderr; neither sends
# a request. Plain `k2 done`, its flags (--blocked, --agent) and a summary
# word (ignored, as before) still send POST /cli/done. In a session the API
# started (K2_API_CELL=1) done completes the session: --reason still goes
# out, an unknown flag exits 2 and sends nothing.
#
# On the session's cell socket with no passport, done sends no
# Authorization header at all. Regression (0.46.17): it sent an empty
# `Authorization: Bearer `. The stub's socket logs each request's raw head.
#
# Hermetic: temp HOME, a stub daemon on loopback that logs each request's
# path and form, no real ~/.k2. Loud.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORKDIR="$(mktemp -d -t k2-done-flags-XXXXXX)"
# Short: a Unix socket path must stay under ~104 bytes.
SOCKDIR="$(mktemp -d /tmp/k2dn-XXXXXX)"
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
from urllib.parse import parse_qs, urlparse

root, sock_path = sys.argv[1], sys.argv[2]
log = os.path.join(root, "requests.jsonl")
uds_log = os.path.join(root, "uds.jsonl")

# The cell socket: each request's raw head and body, then a 200.
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
        body = b'{"success": true}'
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
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        form = parse_qs(u.query)
        if n:
            form.update(parse_qs(self.rfile.read(n).decode()))
        for k in ("token", "project"):
            form.pop(k, None)
        with open(log, "a") as f:
            f.write(json.dumps({"path": u.path, "form": {k: v[0] for k, v in sorted(form.items())}}) + "\n")
        body = b'{"success": true}'
        self.send_response(200)
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
    [ -s "$WORKDIR/stub.port" ] && break
    sleep 0.05
done
[ -s "$WORKDIR/stub.port" ] && [ -S "$SOCK" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }
export K2_PORT="$(cat "$WORKDIR/stub.port")"
export K2_AGENT_NAME=tester

# run <env...> -- <k2 args...>: runs the CLI, sets RC, OUT, ERR and REQS
# (each request as "<path> <sorted form JSON>", one per line).
run() {
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : > "$WORKDIR/requests.jsonl"
    : > "$WORKDIR/uds.jsonl"
    RC=0
    env ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" >"$WORKDIR/out" 2>"$WORKDIR/err" || RC=$?
    OUT="$(cat "$WORKDIR/out")"
    ERR="$(cat "$WORKDIR/err")"
    REQS="$(python3 -c '
import json, sys
for l in open(sys.argv[1]):
    if l.strip():
        r = json.loads(l)
        print(r["path"], json.dumps(r["form"], sort_keys=True))
' "$WORKDIR/requests.jsonl")"
}

refused() {
    local label="$1"; shift
    run "$@"
    if [ "$RC" = 2 ] && [ -z "$REQS" ] && [ -n "$ERR" ]; then
        ok "$label: exit 2, reason on stderr, nothing sent"
    else
        bad "$label: rc=$RC requests=[$REQS] stderr=[$ERR]"
    fi
}

sends() {
    local label="$1" want="$2"; shift 2
    run "$@"
    if [ "$RC" = 0 ] && [ "$REQS" = "$want" ]; then
        ok "$label: sends $want"
    else
        bad "$label: rc=$RC requests=[$REQS] want [$want] stderr=[$ERR]"
    fi
}

echo "== an unknown flag sends nothing =="
run -- done --help
if [ "$RC" = 0 ] && [ -z "$REQS" ] && [[ "$OUT" == *"k2 done"* ]]; then
    ok "done --help: usage on stdout, exit 0, nothing sent"
else
    bad "done --help: rc=$RC requests=[$REQS] stdout=[$OUT]"
fi
run -- done -h
if [ "$RC" = 0 ] && [ -z "$REQS" ] && [[ "$OUT" == *"k2 done"* ]]; then
    ok "done -h: usage on stdout, exit 0, nothing sent"
else
    bad "done -h: rc=$RC requests=[$REQS] stdout=[$OUT]"
fi
refused "done -x" -- done -x
refused "done --bogus" -- done --bogus
refused "done --blocked x --bogus" -- done --blocked x --bogus
refused "done --agent (no name)" -- done --agent

echo "== done and its flags still send =="
sends "done" '/cli/done {"agent": "tester"}' -- done
sends "done --blocked" '/cli/done {"agent": "tester", "blocked": "waiting on review"}' -- done --blocked "waiting on review"
sends "done --agent" '/cli/done {"agent": "other"}' -- done --agent other
sends "done <summary>" '/cli/done {"agent": "tester"}' -- done "shipped the fix"

echo "== in a session the API started =="
CELL=(K2_API_CELL=1 K2_HOOK_TOKEN=s1.secret)
sends "API cell: done" '/cli/session/complete {}' "${CELL[@]}" -- done
sends "API cell: done --reason" '/cli/session/complete {"reason": "no answer needed"}' "${CELL[@]}" -- done --reason "no answer needed"
refused "API cell: done --bogus" "${CELL[@]}" -- done --bogus
refused "API cell: done -x" "${CELL[@]}" -- done -x

echo "== on the cell socket with no passport: no Authorization header =="
# The owner token on disk passes the connection gate; it is never sent on
# the session's socket, and with no passport nothing stands in for it.
mkdir -p "$HOME/.k2"
printf 'owner-secret-token\n' > "$HOME/.k2/heartbeat.token"
NOPASS=(-u K2_HOOK_TOKEN K2_API_CELL=1 K2_HOOK_SOCK="$SOCK")
for form in "" "--reason|no passport here"; do
    args=(done)
    label="API cell, no passport: done"
    if [ -n "$form" ]; then args+=("${form%%|*}" "${form#*|}"); label="$label --reason"; fi
    run "${NOPASS[@]}" -- "${args[@]}"
    heads="$(python3 -c '
import json, sys
for l in open(sys.argv[1]):
    if l.strip():
        r = json.loads(l)
        print(r["head"].replace("\r\n", "\n"))
        print("body=" + r["body"])
        print("--")
' "$WORKDIR/uds.jsonl")"
    want_body="body="
    [ -n "$form" ] && want_body="body=reason=no%20passport%20here"
    if [ "$RC" = 0 ] && [ -z "$REQS" ] && [ "$(grep -c '^POST /cli/session/complete HTTP/1.1$' <<<"$heads")" = 1 ] &&
        ! grep -qi '^authorization:' <<<"$heads" && ! grep -q 'owner-secret-token' <<<"$heads" &&
        grep -qx "$want_body" <<<"$heads"; then
        ok "$label: one POST on the socket, no Authorization header, the owner token never sent"
    else
        bad "$label: rc=$RC tcp=[$REQS] socket=[$heads] stderr=[$ERR]"
    fi
done
rm -f "$HOME/.k2/heartbeat.token"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
