#!/usr/bin/env bash
# Agent Isolation M2 B4: `k2 agent isolation layout on|off|status`. Help and
# usage errors send nothing (exit 0 / 2); status prints the report; `on`
# answered vault_presence_required asks this computer's owner check over the
# vault socket and sends `on` again with its token; a refusal exits 3. A
# local stand-in daemon (TCP) and vault socket log each request. No real
# daemon, no ~/.k2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; CLI="$CLI_UNDER_TEST"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

STAND_DIR="$(mktemp -d -t k2-iso-layout-XXXXXX)"
PIDS=()
cleanup() {
    for p in ${PIDS[@]+"${PIDS[@]}"}; do kill "$p" 2>/dev/null || true; wait "$p" 2>/dev/null || true; done
    rm -rf "$STAND_DIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

cat >"$STAND_DIR/daemon.py" <<'PY'
import http.server, json, os, socketserver, sys, threading
log_path, port_path, sock_path = sys.argv[1], sys.argv[2], sys.argv[3]

def log(line):
    with open(log_path, "a") as f:
        f.write(line + "\n")

class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        b = json.loads(body) if body.startswith("{") else {}
        path = self.path.split("?")[0]
        log("%s %s %s" % (self.command, path, json.dumps(b, sort_keys=True)))
        status, out = 404, {"error": {"code": "not_found", "hint": path}}
        if path == "/cli/agent-isolation/layout":
            mode = b.get("mode")
            if mode == "status":
                status, out = 200, {"ok": True, "placing": True, "setting": "on", "whyOff": None,
                                    "layout": {"ready": True, "p1": "refused", "pidMax": 4194304,
                                               "scopes": [{"kind": "fenced"}, {"kind": "open"}], "survivors": []}}
            elif mode == "on" and b.get("confirmToken") == "tok-1":
                status, out = 200, {"ok": True, "changed": True, "placing": True,
                                    "appliesTo": "new sessions get a scope at their next start"}
            elif mode == "on":
                status, out = 403, {"error": {"code": "vault_presence_required", "hint": "run k2 agent isolation layout on yourself"},
                                    "confirm": {"action": "isolation-layout", "secretId": "layout:on"}}
            elif mode == "off":
                status, out = 409, {"error": {"code": "fenced_scopes_live", "hint": "1 fenced agent session(s) run in a scope"}}
        elif path == "/cli/vault/confirm/start":
            status, out = 200, {"methods": ["terminal"], "confirmText": "turn on a root-owned scope for every agent session on this server"}
        elif path == "/cli/vault/confirm/finish":
            ok = b.get("action") == "isolation-layout" and b.get("secretId") == "layout:on" and b.get("method") == "terminal"
            status, out = (200, {"token": "tok-1", "expiresIn": 60}) if ok else (400, {"error": {"code": "bad_action", "hint": "x"}})
        raw = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
    def address_string(self):
        return "local"

class UnixH(H):
    pass

class UnixServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

vault = UnixServer(sock_path, UnixH)
threading.Thread(target=vault.serve_forever, daemon=True).start()
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
open(port_path, "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
mkdir -p "$HOME/.k2"
python3 -I "$STAND_DIR/daemon.py" "$STAND_DIR/requests.log" "$STAND_DIR/port" "$HOME/.k2/vault.sock" &
PIDS+=($!)
for _ in $(seq 1 50); do [ -s "$STAND_DIR/port" ] && [ -S "$HOME/.k2/vault.sock" ] && break; sleep 0.1; done
[ -s "$STAND_DIR/port" ] || fail "the stand-in daemon did not start"
[ -S "$HOME/.k2/vault.sock" ] || fail "the stand-in vault socket did not start"
export K2_PORT="$(cat "$STAND_DIR/port")"
# The owner's terminal outside K2: no passport, the owner token on disk.
unset K2_HOOK_TOKEN
printf 'owner-token' >"$HOME/.k2/heartbeat.token"

run() {
    set +e
    OUT="$("$CLI" "$@" 2>"$STAND_DIR/stderr" </dev/null)"
    RC=$?
    ERR="$(cat "$STAND_DIR/stderr")"
    set -e
}

# ── help and usage send nothing ─────────────────────────────────────
: >"$STAND_DIR/requests.log"
run agent isolation layout --help
[ "$RC" -eq 0 ] || fail "layout --help exits 0, got $RC: $ERR"
grep -qF "k2 agent isolation layout on|off|status [--json]" <<<"$OUT" || fail "the help names layout: $OUT"
for args in "agent isolation layout" "agent isolation layout maybe" "agent isolation layout on off" "agent isolation layout on --frob"; do
    # shellcheck disable=SC2086
    run $args
    [ "$RC" -eq 2 ] || fail "k2 $args is a usage error (exit 2), got $RC: $ERR"
done
[ ! -s "$STAND_DIR/requests.log" ] || fail "help and usage sent requests: $(cat "$STAND_DIR/requests.log")"

# ── status ──────────────────────────────────────────────────────────
run agent isolation layout status
[ "$RC" -eq 0 ] || fail "status exits 0, got $RC: $ERR"
grep -q "^Agent scopes: on (the owner's setting: on)" <<<"$OUT" || fail "status line: $OUT"
grep -q "live scopes: 2 (1 fenced)" <<<"$OUT" || fail "live scopes: $OUT"
grep -q '^POST /cli/agent-isolation/layout {"mode": "status"}$' "$STAND_DIR/requests.log" || fail "status is a POST with mode status: $(cat "$STAND_DIR/requests.log")"
run agent isolation layout status --json
[ "$RC" -eq 0 ] && python3 -c 'import json,sys; v=json.loads(sys.argv[1]); assert v["placing"] is True, v' "$OUT" || fail "--json: $OUT"

# ── on: the owner check over the vault socket, then on again ────────
: >"$STAND_DIR/requests.log"
run agent isolation layout on
[ "$RC" -eq 0 ] || fail "on exits 0 after the owner check, got $RC: $OUT $ERR"
grep -q "^Agent scopes are on: new sessions get a scope" <<<"$OUT" || fail "on's line: $OUT"
want='POST /cli/agent-isolation/layout {"mode": "on"}
POST /cli/vault/confirm/start {"action": "isolation-layout", "secretId": "layout:on"}
POST /cli/vault/confirm/finish {"action": "isolation-layout", "method": "terminal", "secretId": "layout:on"}
POST /cli/agent-isolation/layout {"confirmToken": "tok-1", "mode": "on"}'
[ "$(cat "$STAND_DIR/requests.log")" = "$want" ] || fail "on's requests:
$(cat "$STAND_DIR/requests.log")"
grep -q "You are approving: turn on a root-owned scope" <<<"$ERR" || fail "says what is approved: $ERR"
grep -q "tok-1" <<<"$OUT$ERR" && fail "the token is never printed: $OUT $ERR"

# ── a refusal exits 3 with its code ─────────────────────────────────
run agent isolation layout off
[ "$RC" -eq 3 ] && grep -q "fenced_scopes_live" <<<"$ERR" || fail "off refused: exit 3, got $RC: $ERR"

# ── in a K2 session (a passport): no owner check is asked ───────────
: >"$STAND_DIR/requests.log"
K2_HOOK_TOKEN=passport run agent isolation layout on
[ "$RC" -eq 3 ] && grep -q "vault_presence_required" <<<"$ERR" || fail "a passport's on is refused, exit 3: $RC $ERR"
grep -q "confirm/" "$STAND_DIR/requests.log" && fail "a session never asks the owner check: $(cat "$STAND_DIR/requests.log")"

echo "PASS: agent_isolation_layout_cli"
