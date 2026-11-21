#!/usr/bin/env bash
# Agent Isolation M1: in a K2 session, every request that carries the
# session's passport goes over the session socket, never loopback TCP.
#
# The daemon refuses a passport bound to its agent's cgroup on TCP (403
# passport_needs_socket: a TCP connection names no process whose cgroup it
# could check). So a verb whose passport call still went to TCP would break
# for every agent on a box with the identity. This is the guard against
# that: a spread of agent verbs runs in a session against a stub daemon
# that logs each request's transport, and no passport may reach TCP. The
# CLI marks the requests it used to send over TCP (X-K2-Via-Tcp: 1) so the
# daemon's socket hands them to the same TCP route code.
#
# Bite: K2_VIA_TCP=1 (the CLI's own off-switch for this rule) sends those
# requests to TCP again, and the check fails.
#
# Nothing is installed; the CLI runs with `env -i` and a temp HOME.
# Run with: bash tests/cli/agent_isolation_passport_socket.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-iso-sock-XXXXXX)"
SOCK_DIR="$(mktemp -d /tmp/k2is-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK" "$SOCK_DIR"
}
trap cleanup EXIT

PASSPORT="sess91.passportfe12"
OWNER="owner-tok-77aa"
SOCK="$SOCK_DIR/cell.sock"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import json, os, socketserver, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit
LOG_FILE = os.environ["LOG_FILE"]

def handler(via):
    class H(BaseHTTPRequestHandler):
        def _answer(self, method, raw):
            u = urlsplit(self.path)
            with open(LOG_FILE, "a") as f:
                f.write(json.dumps({"via": via, "method": method, "path": u.path,
                                    "auth": self.headers.get("Authorization"),
                                    "viaTcp": self.headers.get("X-K2-Via-Tcp")}) + "\n")
            body = b'{"view":"full","phase":"ready","features":[]}' if u.path == "/boot-status" else b'{"ok":true}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def do_GET(self): self._answer("GET", b"")
        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            self._answer("POST", self.rfile.read(n))
        def log_message(self, *_a): pass
    return H

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    def get_request(self):
        conn, _ = super().get_request()
        return conn, ("uds", 0)

class T(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True

u = U(os.environ["SOCK_PATH"], handler("uds"))
threading.Thread(target=u.serve_forever, daemon=True).start()
T(("127.0.0.1", int(os.environ["PORT"])), handler("tcp")).serve_forever()
PY

PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
: >"$LOG_FILE"
LOG_FILE="$LOG_FILE" PORT="$PORT" SOCK_PATH="$SOCK" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if [ -S "$SOCK" ] && curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/boot-status"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

FAKE_HOME="$WORK/home"
WS="$WORK/ws"
mkdir -p "$FAKE_HOME/.k2" "$WS"
printf '%s' "$PORT" >"$FAKE_HOME/.k2/heartbeat.port"
printf '%s' "$PORT" >"$FAKE_HOME/.k2/daemon.port"
# A fenced agent can't read the owner token; an unfenced one can. Both cases.
printf '%s' "$OWNER" >"$FAKE_HOME/.k2/heartbeat.token"

# The verbs an agent runs most (and a few that only ever spoke TCP).
VERBS=(
    "whoami --json"
    "thread A hello"
    "inbox list"
    "heartbeat list"
    "connections"
    "compute nodes"
    "app list"
    "agent conf"
    "agent identity"
    "workspace list"
    "msg B hello"
    "capabilities"
    "sessions"
    "feedback list"
    "vault list"
    "mail inbox"
    "doc ls"
    "organization"
    "permission list"
    "postal status"
)

run_all() { # extra env words…
    : >"$LOG_FILE"
    local v
    for v in "${VERBS[@]}"; do
        # shellcheck disable=SC2086
        (cd "$WS" && env -i PATH="/usr/bin:/bin" HOME="$FAKE_HOME" K2_HOME="$FAKE_HOME/.k2" \
            K2_PROJECT_PATH="$WS" K2_HOOK_SOCK="$SOCK" K2_HOOK_TOKEN="$PASSPORT" K2_AGENT_CGROUP=a-ws1 "$@" \
            "$K2_CLI" $v >/dev/null 2>&1 </dev/null) || true
    done
}

passport_over_tcp() {
    "$PY" -c 'import json, sys
n = 0
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r["via"] == "tcp" and r.get("auth") == "Bearer " + sys.argv[2] and r["path"].startswith("/cli/"):
        n += 1
        print("    passport over TCP: %s %s" % (r["method"], r["path"]), file=sys.stderr)
print(n)' "$LOG_FILE" "$PASSPORT"
}
count() { # via auth-match viaTcp-flag
    "$PY" -c 'import json, sys
print(sum(1 for l in open(sys.argv[1]) for r in [json.loads(l)]
          if r["via"] == sys.argv[2] and r.get("auth") == "Bearer " + sys.argv[3] and (sys.argv[4] == "any" or (r.get("viaTcp") == "1") == (sys.argv[4] == "1"))))' \
        "$LOG_FILE" "$1" "$PASSPORT" "$2"
}

echo "== in a session: no passport request reaches TCP"
run_all
total="$(wc -l <"$LOG_FILE" | tr -d ' ')"
[ "$total" -gt 0 ] || bad "the verbs made no requests at all"
n="$(passport_over_tcp)"
if [ "$n" = 0 ]; then ok "0 passport requests over TCP ($total requests in all)"; else bad "$n passport request(s) went over TCP"; fi
marked="$(count uds 1)"
if [ "$marked" -gt 0 ]; then ok "$marked request(s) the CLI used to send over TCP went over the socket, marked X-K2-Via-Tcp"; else bad "no request carried X-K2-Via-Tcp (the reroute never ran)"; fi
native="$(count uds 0)"
if [ "$native" -gt 0 ]; then ok "$native native socket request(s) are not marked"; else bad "no native (unmarked) socket requests"; fi

echo "== bite: K2_VIA_TCP=1 sends them to TCP again"
run_all K2_VIA_TCP=1
n="$(passport_over_tcp 2>/dev/null)"
if [ "$n" -gt 0 ]; then ok "BITE: $n passport request(s) over TCP with the reroute off"; else bad "BITE: the reroute off still sent nothing over TCP (the check proves nothing)"; fi

echo "== a session without the identity mark (K2_AGENT_CGROUP unset): as before"
run_all K2_AGENT_CGROUP=
if [ "$(count uds 1)" = 0 ]; then ok "no request marked X-K2-Via-Tcp without the mark"; else bad "marked requests without the mark"; fi
n="$(passport_over_tcp 2>/dev/null)"
if [ "$n" -gt 0 ]; then ok "$n passport request(s) over TCP, as before 0.46.17"; else bad "without the mark nothing went over TCP"; fi

echo "== the socket is gone (connect refused): loopback TCP, as before"
: >"$LOG_FILE"
(cd "$WS" && env -i PATH="/usr/bin:/bin" HOME="$FAKE_HOME" K2_HOME="$FAKE_HOME/.k2" K2_PROJECT_PATH="$WS" \
    K2_HOOK_SOCK="$SOCK_DIR/gone.sock" K2_HOOK_TOKEN="$PASSPORT" K2_AGENT_CGROUP=a-ws1 \
    "$K2_CLI" workspace list >/dev/null 2>&1 </dev/null) || true
if [ "$(passport_over_tcp 2>/dev/null)" -gt 0 ]; then ok "a missing socket falls back to TCP (the passport went there)"; else bad "no TCP request when the socket is gone: $(cat "$LOG_FILE")"; fi

echo "== outside a session: the owner's own calls stay on TCP"
: >"$LOG_FILE"
(cd "$WS" && env -i PATH="/usr/bin:/bin" HOME="$FAKE_HOME" K2_HOME="$FAKE_HOME/.k2" K2_PROJECT_PATH="$WS" \
    "$K2_CLI" workspace list >/dev/null 2>&1 </dev/null) || true
if grep -q '"via": "tcp"' "$LOG_FILE" && ! grep -q '"via": "uds"' "$LOG_FILE"; then ok "no session: TCP only"; else bad "no session: $(cat "$LOG_FILE")"; fi

echo
echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
