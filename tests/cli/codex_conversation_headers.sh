#!/usr/bin/env bash
# A Codex call's conversation headers on every daemon request, from the
# bash CLI and the k5 binary alike (cli-parity runs this on both).
#
# The shell's rule (_k2_codex_headers in cli/src/cli-tools-policy.sh):
# while CODEX_THREAD_ID is set and K2_CODEX_PASSPORT_BIND is not off,
# every request to the daemon carries X-K2-Conversation (CODEX_SESSION_ID)
# and X-K2-Thread (CODEX_THREAD_ID), each only when it is 1 to 128 of
# letters, digits, '-' and '_'. On the cell socket and over TCP alike;
# never on curl's argv; never to a server that is not the daemon.
#
# Covers, as a Codex tool call in a tab K2 started (K2_CODEX_TAB_ISOLATED
# names the passport's session):
#   1. permission request and permission status (a GET) on the cell socket
#      and over TCP; msg and thread (a write and a read, the read a GET)
#      on the cell socket and over TCP. Every request carries both
#      headers; neither id is ever in curl's argv.
#   2. No CODEX_ setting, and K2_CODEX_PASSPORT_BIND=0: no header.
#   3. A root id with a space: X-K2-Thread alone.
#   4. connect logout's POST to the account server (not the daemon): no
#      header.
#
# No daemon build; the daemon and the account server are a python stub.
# HOME is sandboxed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_absent() {
    local label="$1" hay="$2" needle="$3"
    if [[ "$hay" != *"$needle"* ]]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (found $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    fi
}

WORK="$(mktemp -d -t k2-codex-headers-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-aaaa"
SCOPED="sess42.scoped-secret-bbbb"
MARK="${SCOPED%%.*}"
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj/.k2"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
: >"$WORK/requests.log"

python3 - "$WORK" <<'PYEOF' &
import json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
ANSWERS = {
    "/cli/permission/enabled": {"enabled": True},
    "/boot-status": {"version": "0", "features": ["send-once"]},
    "/cli/permission/request": {"id": "pr_1"},
    "/cli/permission/status": {"status": "pending"},
    "/auth/v1/logout": {},
}
DEFAULT = {"ok": True, "success": True, "id": "m1", "seq": 4, "from": "me",
           "items": [], "has_more": False, "target_session_id": "s1", "attempts": 1}

class H(BaseHTTPRequestHandler):
    via = "tcp"

    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        if length:
            self.rfile.read(length)
        path = urllib.parse.urlparse(self.path).path
        rec = {
            "via": self.via,
            "method": method,
            "path": path,
            "conversation": self.headers.get_all("X-K2-Conversation") or [],
            "thread": self.headers.get_all("X-K2-Thread") or [],
        }
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        data = json.dumps(ANSWERS.get(path, DEFAULT)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

class UH(H):
    via = "sock"

    def address_string(self):
        return "cell-socket"

class US(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

us = US(os.path.join(work, "cell.sock"), UH)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && [ -S "$WORK/cell.sock" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
[ -S "$WORK/cell.sock" ] || { echo "FAIL: the stub's cell socket is missing" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
SOCK="$WORK/cell.sock"

# A curl on PATH that logs its argv, then runs the real one.
REAL_CURL="$(command -v curl)"
mkdir -p "$WORK/bin"
cat >"$WORK/bin/curl" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >>"$WORK/curl-argv.log"
exec "$REAL_CURL" "\$@"
EOF
chmod +x "$WORK/bin/curl"

CODEX_ENV=(CODEX_THREAD_ID=thread-1 CODEX_SESSION_ID=root-1 K2_CODEX_TAB_ISOLATED="$MARK")

# run NAME=VALUE... -- ARGS...: k2 with a passport, the stub's port and
# nothing else of K2's or Codex's from this shell.
run() {
    local -a envs=()
    while [ $# -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$WORK/requests.log"; : >"$WORK/curl-argv.log"
    set +e
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K5_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K5_HOOK_TOKEN \
        -u K2SO_PORT -u K5_PORT -u K2_PANE_ID -u K5_PANE_ID -u K2SO_PANE_ID \
        -u CODEX_THREAD_ID -u CODEX_SESSION_ID -u CODEX_HOME -u CODEX_SANDBOX \
        -u K2_CODEX_TAB_ISOLATED -u K2_CODEX_PASSPORT_BIND -u K2_CONNECT_DB_URL \
        K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_PROJECT_PATH="$WORK/proj" PATH="$WORK/bin:$PATH" \
        ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" >"$WORK/out" 2>"$WORK/err" </dev/null
    rc=$?
    set -e
}

# "<requests> <requests on via> <requests whose headers are not WANT>":
# WANT is "both", "none" or "thread".
summary() {
    python3 - "$WORK/requests.log" "$1" "$2" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
via, want = sys.argv[2], sys.argv[3]
expect = {
    "both": (["root-1"], ["thread-1"]),
    "none": ([], []),
    "thread": ([], ["thread-1"]),
}[want]
bad = [r for r in rows if (r["conversation"], r["thread"]) != expect]
print("%d %d %d" % (len(rows), sum(1 for r in rows if r["via"] == via), len(bad)))
PY
}

# check LABEL VIA WANT [all]: exit 0; at least one request; every request
# carries WANT; at least one on VIA (with `all`, every one); neither id on
# curl's argv.
check() {
    local label="$1" via="$2" want="$3" all="${4:-}" s n on bad
    assert_eq "$label: exit 0" "$rc" "0"
    [ "$rc" = 0 ] || sed 's/^/    err: /' "$WORK/err" >&2
    s="$(summary "$via" "$want")"
    read -r n on bad <<<"$s"
    if [ "$n" -ge 1 ] && [ "$bad" = 0 ]; then
        echo "  PASS: $label: every request ($n) carries $want"; pass=$((pass + 1))
    else
        echo "  FAIL: $label: requests=$n not $want=$bad ($(cat "$WORK/requests.log"))" >&2
        fail=$((fail + 1))
    fi
    if [ "$all" = all ]; then
        assert_eq "$label: every request on $via" "$on" "$n"
    elif [ "$on" -ge 1 ]; then
        echo "  PASS: $label: sent on $via"; pass=$((pass + 1))
    else
        echo "  FAIL: $label: nothing on $via ($(cat "$WORK/requests.log"))" >&2
        fail=$((fail + 1))
    fi
    assert_absent "$label: root id never in curl argv" "$(cat "$WORK/curl-argv.log")" "root-1"
    assert_absent "$label: thread id never in curl argv" "$(cat "$WORK/curl-argv.log")" "thread-1"
    assert_absent "$label: no X-K2- header in curl argv" "$(cat "$WORK/curl-argv.log")" "X-K2-"
}

echo "== both headers, cell socket =="
run "${CODEX_ENV[@]}" K2_HOOK_SOCK="$SOCK" -- permission request x.write --why w
check "permission request, socket" sock both all
run "${CODEX_ENV[@]}" K2_HOOK_SOCK="$SOCK" -- permission status pr_1
check "permission status (GET), socket" sock both all
# msg and thread ask about send-once over TCP, then send on the socket.
run "${CODEX_ENV[@]}" K2_HOOK_SOCK="$SOCK" -- msg cortana hello --from me
check "msg, socket" sock both
run "${CODEX_ENV[@]}" K2_HOOK_SOCK="$SOCK" -- thread bob hi --from me
check "thread write, socket" sock both
run "${CODEX_ENV[@]}" K2_HOOK_SOCK="$SOCK" -- thread bob --from me
check "thread read (GET), socket" sock both

echo "== both headers, TCP =="
run "${CODEX_ENV[@]}" -- permission request x.write --why w
check "permission request, TCP" tcp both all
run "${CODEX_ENV[@]}" -- permission status pr_1
check "permission status (GET), TCP" tcp both all
run "${CODEX_ENV[@]}" -- msg cortana hello --from me
check "msg, TCP" tcp both all
run "${CODEX_ENV[@]}" -- thread bob hi --from me
check "thread write, TCP" tcp both all
run "${CODEX_ENV[@]}" -- thread bob --from me
check "thread read (GET), TCP" tcp both all

echo "== no headers =="
run K2_HOOK_SOCK="$SOCK" -- permission request x.write --why w
check "no CODEX_ setting, socket" sock none all
run -- thread bob --from me
check "no CODEX_ setting, TCP" tcp none all
run "${CODEX_ENV[@]}" K2_CODEX_PASSPORT_BIND=0 K2_HOOK_SOCK="$SOCK" -- permission request x.write --why w
check "K2_CODEX_PASSPORT_BIND=0, socket" sock none all
run "${CODEX_ENV[@]}" K2_CODEX_PASSPORT_BIND=0 -- msg cortana hello --from me
check "K2_CODEX_PASSPORT_BIND=0, TCP" tcp none all

echo "== an id the shell won't send =="
run "${CODEX_ENV[@]}" CODEX_SESSION_ID="root 1" K2_HOOK_SOCK="$SOCK" -- permission request x.write --why w
check "a root id with a space, socket" sock thread all
run "${CODEX_ENV[@]}" CODEX_SESSION_ID="root 1" -- thread bob --from me
check "a root id with a space, TCP" tcp thread all

echo "== not the daemon =="
printf '%s' '{"access_token":"a.b.c","refresh_token":"r1","email":"me@acme.example"}' >"$HOME/.k2/connect-account.json"
run "${CODEX_ENV[@]}" K2_CONNECT_DB_URL="http://127.0.0.1:$STUB_PORT" -- connect logout
check "connect logout to the account server" tcp none all
assert_eq "connect logout: one request, the logout" \
    "$(python3 -c 'import json,sys; print(",".join(json.loads(l)["path"] for l in open(sys.argv[1]) if l.strip()))' "$WORK/requests.log")" \
    "/auth/v1/logout"

echo
echo "codex_conversation_headers: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
