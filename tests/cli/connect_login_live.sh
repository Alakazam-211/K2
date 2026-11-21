#!/usr/bin/env bash
# `k2 connect login` prints "✓ live at <url>" and exits 0 only when the
# daemon's answer to /cli/tunnel/start says the tunnel started. No answer
# (the connection refused, or closed with no reply, or an empty body), an
# answer that isn't JSON, a JSON answer that isn't an object and an error
# answer each print one error line on stderr and exit 1, and the live line
# is printed in none of them.
#
# One stub server is the Connect account server and this server's daemon;
# each case starts its own, set to answer the tunnel start one way. The
# access token comes on stdin (--token-stdin), so nothing prompts.
#
# No daemon build; a throwaway HOME; never touches the real ~/.k2.
# Runs with the bash 3.2 of macOS as well.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

REAL_PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }

W="$(mktemp -d "${TMPDIR:-/tmp}/k2live-XXXXXX")"
STUB_PID=""
stop_stub() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
        STUB_PID=""
    fi
}
cleanup() { stop_stub; rm -rf "$W"; }
trap cleanup EXIT

# A throwaway HOME, outside any K2 session (test process only).
export HOME="$W/home"
mkdir -p "$HOME/.k2"
for p in HOOK_SOCK HOOK_TOKEN CELL PORT HOST REMOTE_TOKEN HOME CONNECT_DB_URL PROJECT_PATH; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done
printf 'local-test-token\n' >"$HOME/.k2/heartbeat.token"
cd "$W"

# start_stub MODE: the stub, answering /cli/tunnel/start by MODE:
#   ok      200 {"public_url": "https://acme.k2.dev", "running": true}
#   empty   200 with an empty body
#   drop    no reply at all: the connection closes (curl's empty reply)
#   refuse  the stub exits once it has answered /cli/tunnel/config, so
#           the start's connection is refused
#   html    200 with a body that isn't JSON
#   error   400 {"error": {"code": "frpc_missing", "hint": "frpc is not installed"}}
#   list    200 [] (JSON, not an object)
# Every request's path goes to $W/requests.log.
start_stub() {
    local mode="$1"
    rm -f "$W/port" "$W/port.tmp"
    : >"$W/requests.log"
    "$REAL_PY" -I - "$W" "$mode" <<'PYEOF' &
import http.server, json, os, sys
d, mode = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def _answer(self, status, body):
        if not isinstance(body, bytes):
            body = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        path = self.path.split("?")[0]
        with open(os.path.join(d, "requests.log"), "a") as f:
            f.write(path + "\n")
        if path == "/rest/v1/subdomains":
            return self._answer(200, [{"label": "acme", "status": "active", "tunnel_token": "k2tt_LIVE_TEST"}])
        if path == "/rest/v1/rpc/claim_subdomain":
            return self._answer(200, [{"claimed": True}])
        if path == "/cli/tunnel/config":
            self._answer(200, {"tokenSet": True, "subdomain": "acme"})
            if mode == "refuse":
                os._exit(0)
            return
        if path == "/cli/tunnel/start":
            if mode == "ok":
                return self._answer(200, {"public_url": "https://acme.k2.dev", "running": True})
            if mode == "empty":
                return self._answer(200, b"")
            if mode == "drop":
                self.close_connection = True
                return
            if mode == "html":
                return self._answer(200, b"<html>busy</html>")
            if mode == "error":
                return self._answer(400, {"error": {"code": "frpc_missing", "hint": "frpc is not installed"}})
            if mode == "list":
                return self._answer(200, b"[]")
        return self._answer(200, {"ok": True})
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(d, "port.tmp"), "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(os.path.join(d, "port.tmp"), os.path.join(d, "port"))
srv.serve_forever()
PYEOF
    STUB_PID=$!
    for _ in $(seq 1 50); do [ -s "$W/port" ] && break; sleep 0.1; done
    [ -s "$W/port" ] || { echo "FAIL: the stub server never came up ($mode)" >&2; exit 1; }
    PORT_NO="$(cat "$W/port")"
    printf '%s\n' "$PORT_NO" >"$HOME/.k2/heartbeat.port"
    export K2_CONNECT_DB_URL="http://127.0.0.1:$PORT_NO/"
}

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
expect_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}

# login MODE: one `connect login --subdomain acme --token-stdin` against a
# stub in MODE. Sets RC, OUT and ERR.
login() {
    start_stub "$1"
    rm -f "$HOME/.k2/connect-account.json"
    RC=0
    OUT="$(printf 'jwt-live-test\n' | "$K2_CLI" connect login --subdomain acme --token-stdin 2>"$W/stderr")" || RC=$?
    ERR="$(cat "$W/stderr")"
    stop_stub
}

# fails MODE LINE: the login exits 1, prints nothing on stdout, ends its
# stderr with LINE, never prints the live line, and asked the start once.
fails() {
    local mode="$1" want="$2"
    login "$mode"
    expect_eq "$mode: exit 1" "$RC" "1"
    expect_eq "$mode: nothing on stdout" "$OUT" ""
    expect_eq "$mode: the error line" "$(printf '%s\n' "$ERR" | tail -n 1)" "$want"
    case "$OUT$ERR" in *"live at"*) bad "$mode: the live line was printed" ;; *) ok "$mode: no live line" ;; esac
    expect_eq "$mode: the start was asked once" "$(grep -c '^/cli/tunnel/start$' "$W/requests.log" || true)" "1"
}

NO_ANSWER="Error starting tunnel: no answer from the K2 daemon on /cli/tunnel/start; check with: k2 connect status"

# covers: connect login
login ok
expect_eq "ok: exit 0" "$RC" "0"
expect_eq "ok: the live line" "$OUT" "✓ live at https://acme.k2.dev"
expect_eq "ok: stderr ends with the sign-in" "$(printf '%s\n' "$ERR" | tail -n 1)" "Signed in as (token)."
case "$ERR" in *Error*) bad "ok: an error on stderr: $ERR" ;; *) ok "ok: no error on stderr" ;; esac

# covers: connect login
fails empty "$NO_ANSWER"
# covers: connect login
fails drop "$NO_ANSWER"
# covers: connect login
fails html "Error: tunnel/start returned unparseable response: '<html>busy</html>'"
# covers: connect login
fails error "Error starting tunnel: frpc is not installed"
# covers: connect login
fails list "Error: tunnel/start failed: []"

# covers: connect login
# refuse: the stub is gone, so the start's request never reaches it.
login refuse
expect_eq "refuse: exit 1" "$RC" "1"
expect_eq "refuse: nothing on stdout" "$OUT" ""
expect_eq "refuse: the error line" "$(printf '%s\n' "$ERR" | tail -n 1)" "$NO_ANSWER"
case "$OUT$ERR" in *"live at"*) bad "refuse: the live line was printed" ;; *) ok "refuse: no live line" ;; esac
expect_eq "refuse: the config was the last request served" "$(tail -n 1 "$W/requests.log")" "/cli/tunnel/config"

echo "connect_login_live: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
