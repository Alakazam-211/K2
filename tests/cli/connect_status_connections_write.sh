#!/usr/bin/env bash
# k2 connect status and k2 connections add / remove against a stub daemon.
#
# connect status reads the K2 home's connect-account.json and tunnel.json,
# asks the daemon whether it holds the tunnel token (only when tunnel.json
# has none) and how the tunnel is; with no port or no token it says there
# is no daemon and sends nothing. It never talks to the Connect control
# plane or the account service. connections add / remove POST
# /cli/connections once (a write that got no answer is not sent again),
# print what happened, and exit 3 with the gate's JSON when agents may not
# change connections.
#
# Nothing is installed and nothing real is reached: the daemon is a stub
# on loopback (python3 runs only the stub), the CLI runs with a clean
# environment (`env -i`, a temp HOME with a made-up owner token).
# Run with: bash tests/cli/connect_status_connections_write.sh

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

WORK="$(mktemp -d -t k2-connect-status-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

K2H="$WORK/home/.k2"
mkdir -p "$K2H" "$WORK/ws"
MODE_FILE="$WORK/mode"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

MODE_FILE, LOG_FILE = os.environ["MODE_FILE"], os.environ["LOG_FILE"]
ANSWERS = {
    # mode: {path: (status, body)}
    "held": {"/cli/tunnel/config": (200, '{"tokenSet":true}'),
             "/cli/tunnel/status": (200, '{"running":true,"public_url":"https://acme.k2.dev"}')},
    "not_held": {"/cli/tunnel/config": (200, '{"tokenSet":false}'),
                 "/cli/tunnel/status": (200, '{"running":false,"enabled":false}')},
    "vault": {"/cli/tunnel/config": (200, '{"tokenUnreadable":true}'),
              "/cli/tunnel/status": (200, '{"running":false,"released":true}')},
    "status_error": {"/cli/tunnel/config": (200, '{"tokenSet":true}'),
                     "/cli/tunnel/status": (403, '{"error":"owner only"}')},
    "added": {"/cli/connections": (200, '{"ok":true,"target":"beta"}')},
    "noop": {"/cli/connections": (200, '{"ok":true,"noop":true,"target":"beta","message":"already connected"}')},
    "bound": {"/cli/connections": (200, '{"ok":true,"target":"seoca::quahog.example","bound":true,"handle":"quahog-site","displayName":"Quahog Site","bindWarning":"slow peer"}')},
    "gated": {"/cli/connections": (403, '{"error":{"code":"agents_create_connections_disabled","hint":"no"}}')},
    "missing": {"/cli/connections": (404, '{"error":{"code":"not_found","hint":"no workspace named nope"}}')},
    "removed": {"/cli/connections": (200, '{"ok":true}')},
}

class H(BaseHTTPRequestHandler):
    def _answer(self, method):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        u = urlsplit(self.path)
        with open(LOG_FILE, "a") as f:
            f.write("%s %s %s\n" % (method, u.path, u.query))
        with open(LOG_FILE + ".auth", "a") as f:
            f.write("%s %s\n" % (u.path, self.headers.get("Authorization") or "-"))
        mode = open(MODE_FILE).read().strip()
        if mode == "close" or (mode == "close_status" and u.path == "/cli/tunnel/status"):
            self.close_connection = True
            return
        if mode == "close_status":
            mode = "held"
        if mode == "empty":
            status, body = 200, ""
        else:
            status, body = ANSWERS.get(mode, {}).get(u.path, (404, '{"error":"no route"}'))
        data = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._answer("GET")

    def do_POST(self):
        self._answer("POST")

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo held >"$MODE_FILE"
: >"$LOG_FILE"
MODE_FILE="$MODE_FILE" LOG_FILE="$LOG_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -s --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/tunnel/status" >/dev/null 2>&1 && break
    sleep 0.1
done

# The owner's terminal: a port and a made-up owner token in the K2 home.
printf '%s\n' "$PORT" >"$K2H/heartbeat.port"
printf 'owner-tok-connect\n' >"$K2H/heartbeat.token"

k2() { (cd "$WORK/ws" && env -i PATH="/usr/bin:/bin:${PATH}" HOME="$WORK/home" TMPDIR="$WORK" "$K2_CLI" "$@"); }
run() {
    local mode="$1"; shift
    echo "$mode" >"$MODE_FILE"
    : >"$LOG_FILE"
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
requests() { cut -d' ' -f1,2 "$LOG_FILE" | tr '\n' ';'; }

echo "== connect status =="
rm -f "$K2H/connect-account.json" "$K2H/tunnel.json"
run held connect status
assert_eq "nothing paired: exit 0" "$rc" "0"
assert_eq "nothing paired: the token, then the tunnel" "$(requests)" "GET /cli/tunnel/config;GET /cli/tunnel/status;"
assert_eq "nothing paired: the lines" "$out" "$(printf '%s\n' \
    "K2 Connect account" \
    "  Account session: no" \
    "  Paired subdomain: (none)" \
    "  Tunnel token:     yes" \
    "  Tunnel running:  yes" \
    "  Public URL:      https://acme.k2.dev")"
assert_eq "nothing paired: nothing on stderr" "$err" ""

printf '%s' '{"access_token":"a.b.c","refresh_token":"r","email":" me@acme.example "}' >"$K2H/connect-account.json"
printf '%s' '{"subdomain":"acme","token":"tok"}' >"$K2H/tunnel.json"
run not_held connect status
assert_eq "paired on disk: exit 0" "$rc" "0"
assert_eq "paired on disk: no token question" "$(requests)" "GET /cli/tunnel/status;"
assert_eq "paired on disk: the lines" "$out" "$(printf '%s\n' \
    "K2 Connect account" \
    "  Account session: yes  (me@acme.example)" \
    "  Paired subdomain: acme → https://acme.k2.dev" \
    "  Tunnel token:     yes" \
    "  Tunnel running:  no" \
    "  State:           DISABLED — re-enable with: k2 tunnel enable")"

printf '%s' '{"subdomain":"acme","token":""}' >"$K2H/tunnel.json"
run vault connect status
assert_eq "the vault can't be read: the line" "$(printf '%s\n' "$out" | sed -n '4p')" "  Tunnel token:     unknown (the K2 vault could not be read)"
assert_eq "released: the line" "$(printf '%s\n' "$out" | sed -n '6p')" "  State:           RELEASED — re-pair with: k2 connect login"
run status_error connect status
assert_eq "the tunnel's error: exit 0" "$rc" "0"
assert_eq "the tunnel's error: the line" "$(printf '%s\n' "$out" | tail -n 1)" "  Tunnel status:   (error: owner only)"

run held connect status now
assert_eq "a word after status: exit 2" "$rc" "2"
assert_eq "a word after status: usage" "$err" "Usage: k2 connect status"
assert_eq "a word after status: nothing sent" "$(requests)" ""

run close_status connect status
assert_eq "no answer to the status: exit 1" "$rc" "1"
assert_eq "no answer to the status: the four lines first" "$(printf '%s\n' "$out" | sed -n '1p;4p')" "$(printf 'K2 Connect account\n  Tunnel token:     yes')"
case "$err" in "Error: K2's server gave an empty answer to /cli/tunnel/status after "*" s; run k2 daemon status") ok "no answer to the status: the line" ;; *) bad "no answer to the status: the line (got $(printf %q "$err"))" ;; esac
assert_eq "no answer to the status: asked once" "$(requests)" "GET /cli/tunnel/config;GET /cli/tunnel/status;"

run close connect status
if [ "$rc" != 0 ]; then ok "no answer to the token question: fails"; else bad "no answer to the token question: exit 0"; fi
assert_eq "no answer to the token question: asked once, then stops" "$(requests)" "GET /cli/tunnel/config;"
assert_eq "no answer to the token question: nothing on stdout" "$out" ""

rm -f "$K2H/heartbeat.port"
run held connect status
assert_eq "no port: exit 0" "$rc" "0"
assert_eq "no port: nothing sent" "$(requests)" ""
assert_eq "no port: the last line" "$(printf '%s\n' "$out" | tail -n 1)" "  Tunnel status:   (daemon not running — start with: k2 daemon start)"
printf '%s\n' "$PORT" >"$K2H/daemon.port"
run held connect status
assert_eq "daemon.port: asked there" "$(requests)" "GET /cli/tunnel/config;GET /cli/tunnel/status;"
rm -f "$K2H/daemon.port"
printf '%s\n' "$PORT" >"$K2H/heartbeat.port"

echo "== connections add / remove =="
Q_TAIL='&action=add&target=beta'
run added connections add beta
assert_eq "add: exit 0" "$rc" "0"
assert_eq "add: the line" "$out" "Connected to beta"
assert_eq "add: one POST" "$(requests)" "POST /cli/connections;"
case "$(cat "$LOG_FILE")" in "POST /cli/connections project="*"$Q_TAIL") ok "add: the query" ;; *) bad "add: the query ($(cat "$LOG_FILE"))" ;; esac
assert_eq "add: the token rides the Bearer header" "$(tail -1 "$LOG_FILE.auth")" "/cli/connections Bearer owner-tok-connect"
run noop connections add beta
assert_eq "already connected" "$out" "Already connected to beta (already connected)"
run bound connections add 'seoca::quahog.example'
assert_eq "a remote add: the lines" "$out" "$(printf 'Connected to seoca::quahog.example\n  reaches quahog-site::quahog.example (Quahog Site)')"
assert_eq "a remote add: the warning" "$err" "warning: slow peer"
run gated connections add beta
assert_eq "the gate: exit 3" "$rc" "3"
assert_eq "the gate: JSON on stderr" "$err" '{"error": {"code": "agents_create_connections_disabled", "hint": "no — this agent isn'"'"'t allowed to create or remove connections — Owner can turn on: Settings → Network → Tunnel → Policies → \"Agents can add connections\" (or Settings → Workspaces / Agents → <workspace> → Connections → \"Let this agent add connections\")."}}'
assert_eq "the gate: nothing on stdout" "$out" ""
run missing connections remove nope
assert_eq "remove, not found: exit 1" "$rc" "1"
# 0.46.17: every error on stderr, stdout only the result lines.
assert_eq "remove, not found: the line" "$err" "Error: no workspace named nope"
assert_eq "remove, not found: nothing on stdout" "$out" ""
run removed connections remove beta
assert_eq "remove: exit 0" "$rc" "0"
assert_eq "remove: the line" "$out" "Disconnected."
case "$(cat "$LOG_FILE")" in *"&action=remove&target=beta") ok "remove: the query" ;; *) bad "remove: the query ($(cat "$LOG_FILE"))" ;; esac
run added connections add 'a b/é&c'
case "$(cat "$LOG_FILE")" in *"&action=add&target=a%20b%2F%C3%A9%26c") ok "add: the target escaped" ;; *) bad "add: the target escaped ($(cat "$LOG_FILE"))" ;; esac
run added connections add
assert_eq "add, no target: exit 1" "$rc" "1"
assert_eq "add, no target: usage" "$err" "Usage: k2 connections add <workspace-name>"
assert_eq "add, no target: nothing sent" "$(requests)" ""
run removed connections remove
assert_eq "remove, no target: usage" "$err" "Usage: k2 connections remove <workspace-name>"

echo "== a write is sent once =="
for verb in add remove; do
    run empty connections "$verb" beta
    assert_eq "$verb, an empty answer: exit 1" "$rc" "1"
    assert_eq "$verb, an empty answer: the reader's line" "$err" "Error: daemon returned an unparseable response: ''"
    assert_eq "$verb, an empty answer: nothing on stdout" "$out" ""
    assert_eq "$verb, an empty answer: one request" "$(requests)" "POST /cli/connections;"
    run close connections "$verb" beta
    if [ "$rc" != 0 ]; then ok "$verb, no answer: fails"; else bad "$verb, no answer: exit 0"; fi
    case "$err" in "Error: K2's server gave an empty answer to /cli/connections after "*" s; run k2 daemon status"*) ok "$verb, no answer: the line" ;; *) bad "$verb, no answer: the line (got $(printf %q "$err"))" ;; esac
    assert_eq "$verb, no answer: one request" "$(requests)" "POST /cli/connections;"
done

echo
echo "connect_status_connections_write: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
