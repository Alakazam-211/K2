#!/usr/bin/env bash
# `k2 workspace triage` in an agent's session lists only what K2 lets the
# session see: it is sent the way `k2 inbox list` is sent, over the
# session's cell socket first with the session's passport, and over TCP
# with the passport when the socket doesn't serve it. The owner token on
# disk is never sent. A session with no passport sends nothing (one line,
# exit 1). Outside a session the owner token is sent, as before.
#
# Nothing is installed: the daemon is a stub (a TCP port and a cell
# socket) that logs each request's transport, path, query and Bearer
# header. The CLI runs with a clean environment (`env -i`, a temp HOME
# whose ~/.k2/daemon.token holds an owner value of its own), so it can
# only reach the stub.
# Run with: bash tests/cli/workspace_triage_session.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (no $(printf %q "$3") in $(printf %q "$2"))" ;; esac; }
assert_lacks() { case "$2" in *"$3"*) bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))" ;; *) ok "$1" ;; esac; }

WORK="$(mktemp -d -t k2-triage-XXXXXX)"
STUB_PID=""
UDS_PID=""
# A Unix socket path must be short (about 100 bytes): its own folder.
SOCK_DIR="$(mktemp -d /tmp/k2tr-XXXXXX)"
cleanup() {
    for p in "$STUB_PID" "$UDS_PID"; do
        if [ -n "$p" ]; then
            kill "$p" 2>/dev/null || true
            wait "$p" 2>/dev/null || true
        fi
    done
    rm -rf "$WORK" "$SOCK_DIR"
}
trap cleanup EXIT

OWNER="owner-tok-9b42e1"
PASSPORT="sess23.passport7d0a"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import json, os, socketserver
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

LOG_FILE = os.environ["LOG_FILE"]
VIA = os.environ.get("VIA", "tcp")
# "items": the cell socket lists the inbox; "not-served": it answers as a
# daemon that doesn't serve the route there.
MODE = os.environ.get("MODE", "items")
ITEMS = json.dumps([{"id": "item-" + VIA, "folder": "", "priority": "normal", "title": "Listed over " + VIA}])

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        u = urlsplit(self.path)
        auth = self.headers.get("Authorization")
        with open(LOG_FILE, "a") as f:
            f.write(json.dumps({"via": VIA, "path": u.path, "query": u.query, "auth": auth}) + "\n")
        if u.path != "/cli/inbox/list":
            return self._send(404, '{"error":"no such route in this stub"}')
        if VIA == "uds" and MODE == "not-served":
            return self._send(404, '{"error":"route not served on cell socket"}')
        return self._send(200, ITEMS)

    def log_message(self, *_a):
        pass

if VIA == "uds":
    class U(socketserver.UnixStreamServer):
        def get_request(self):
            conn, _ = super().get_request()
            return conn, ("uds", 0)
    U(os.environ["SOCK_PATH"], H).serve_forever()
else:
    HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

# Two workspaces: the session's own, and another the CLI is pointed at.
mkdir -p "$WORK/ws/.k2" "$WORK/other/.k2"
WS="$WORK/ws"
OTHER="$WORK/other"
PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
: >"$LOG_FILE"
LOG_FILE="$LOG_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/health"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

# The owner token on disk, readable, as on a fresh headless server: only
# daemon.token, a value of its own that must never be sent from a session.
mkdir -p "$WORK/home/.k2"
printf '%s' "$OWNER" >"$WORK/home/.k2/daemon.token"
chmod 600 "$WORK/home/.k2/daemon.token"

LIVE_SOCK="$SOCK_DIR/cell.sock"
start_uds() { # MODE
    LOG_FILE="$LOG_FILE" VIA=uds MODE="$1" SOCK_PATH="$LIVE_SOCK" "$PY" "$WORK/stub.py" &
    UDS_PID=$!
    for _i in $(seq 1 50); do [ -S "$LIVE_SOCK" ] && break; sleep 0.1; done
    [ -S "$LIVE_SOCK" ] || { echo "FAIL: the cell socket stub did not start" >&2; exit 1; }
}
stop_uds() {
    kill "$UDS_PID" 2>/dev/null || true
    wait "$UDS_PID" 2>/dev/null || true
    UDS_PID=""
    rm -f "$LIVE_SOCK"
}

run() { # [cwd=<dir>] env words..., then --, then the k2 args
    local cwd="$WS" envs=()
    case "$1" in cwd=*) cwd="${1#cwd=}"; shift ;; esac
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$LOG_FILE"
    set +e
    out="$(cd "$cwd" && env -i PATH="/usr/bin:/bin" HOME="$WORK/home" K2_PORT="$PORT" ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
# field <name>: that field of every logged request, one per line, sorted
# and without repeats.
field() {
    "$PY" -c 'import json, sys
for line in open(sys.argv[1]):
    print(repr(json.loads(line).get(sys.argv[2])))' "$LOG_FILE" "$1" | sort -u
}
requests() { grep -c '"path": "/cli/' "$LOG_FILE" || true; }
owner_never_sent() { # label
    if grep -Fq -- "$OWNER" "$LOG_FILE"; then
        bad "$1: the owner token was sent ($(grep -F -- "$OWNER" "$LOG_FILE" | head -n 1))"
    else
        ok "$1: the owner token is in no header and no query"
    fi
}

echo "== in a session with a cell socket: the socket, with the passport =="
start_uds items
for names in K2 K5; do
    for where in "own folder|cwd=$WS" "another folder|cwd=$OTHER" "K2_PROJECT_PATH=other|${names}_PROJECT_PATH=$OTHER"; do
        label="$names, ${where%%|*}"
        # shellcheck disable=SC2086
        run ${where#*|} "${names}_HOOK_SOCK=$LIVE_SOCK" "${names}_HOOK_TOKEN=$PASSPORT" -- workspace triage
        assert_eq "$label: exit 0" "$rc" "0"
        assert_contains "$label: the socket's list" "$out" "Listed over uds"
        assert_eq "$label: one request" "$(requests)" "1"
        assert_eq "$label: by the cell socket" "$(field via)" "'uds'"
        assert_eq "$label: the passport as Bearer" "$(field auth)" "'Bearer $PASSPORT'"
        assert_lacks "$label: no project named on the socket" "$(field query)" "project="
        owner_never_sent "$label"
    done
done
stop_uds

echo "== a cell socket that doesn't serve it: TCP, still the passport =="
start_uds not-served
run K2_PROJECT_PATH="$OTHER" K2_HOOK_SOCK="$LIVE_SOCK" K2_HOOK_TOKEN="$PASSPORT" -- workspace triage
assert_eq "not served: exit 0" "$rc" "0"
assert_contains "not served: TCP's list" "$out" "Listed over tcp"
assert_eq "not served: socket, then TCP" "$(field via | tr '\n' ' ')" "'tcp' 'uds' "
assert_eq "not served: the passport on both" "$(field auth)" "'Bearer $PASSPORT'"
owner_never_sent "not served"
stop_uds

echo "== a session whose socket is gone: TCP, still the passport =="
run K2_HOOK_SOCK="$LIVE_SOCK" K2_HOOK_TOKEN="$PASSPORT" -- workspace triage
assert_eq "no socket: exit 0" "$rc" "0"
assert_eq "no socket: by TCP" "$(field via)" "'tcp'"
assert_eq "no socket: the passport" "$(field auth)" "'Bearer $PASSPORT'"
owner_never_sent "no socket"

echo "== a session with no passport: one line, exit 1, nothing sent =="
start_uds items
for names in K2 K5; do
    run "${names}_HOOK_SOCK=$LIVE_SOCK" -- workspace triage
    assert_eq "$names no passport: exit 1" "$rc" "1"
    assert_eq "$names no passport: nothing on stdout" "$out" ""
    assert_eq "$names no passport: one line on stderr" "$err" "Error: k2 workspace triage in an agent's session needs the session's passport, and this session has none."
    assert_eq "$names no passport: nothing sent" "$(requests)" "0"
done
stop_uds

echo "== the owner's own terminal (no session): the owner token, as before =="
run -- workspace triage
assert_eq "owner: exit 0" "$rc" "0"
assert_contains "owner: the list" "$out" "Listed over tcp"
assert_eq "owner: by TCP" "$(field via)" "'tcp'"
assert_eq "owner: the owner token as Bearer" "$(field auth)" "'Bearer $OWNER'"
assert_contains "owner: names this workspace" "$(field query)" "project="

echo
echo "workspace_triage_session: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
