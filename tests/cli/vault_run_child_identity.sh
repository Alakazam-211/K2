#!/usr/bin/env bash
# `k2 vault run`: the command gets its secrets, not the agent's identity,
# and never the owner's. An agent's own shell is unchanged.
#
# The real run helper (k2-daemon --vault-run), the CLI under test, a stub
# daemon on TCP and on a Unix cell socket, and a FAKE K2 home holding an
# owner token (heartbeat.token and daemon.token). Never the real ~/.k2.
#
# Under vault run, the command:
#   - has the secret (KEY), and no session passport or other K2 token in its
#     environment, and K2_NO_OWNER_FALLBACK=1;
#   - runs k2 for one verb of every sender kind (whoami and the TCP GET,
#     POST, JSON, JSON on stdin and query POST senders, and the socket-first
#     cli_send, _k2_idem_send (form), cli_send_json, compute and sidecar
#     senders):
#     each one exits 3 with the no-owner refusal, and no request reaches the
#     stub from it, with the owner token or any other.
# In the agent's own shell (same env, not under vault run), the same verbs
# are never refused, and no request carries anything but the agent's
# passport or, for the TCP-only open verbs, what they send today (the
# COMPAT-58 owner inversion is unchanged by this).
#
# Build first: cargo build -p k2-daemon   (CARGO_TARGET_DIR honoured)
# Run with: bash tests/cli/vault_run_child_identity.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
for v in $(env | grep -E '^K(2|2SO|5)_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

# Short: a Unix socket path must stay under ~104 bytes.
WORK="$(mktemp -d /tmp/k2vrc-XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

H="$WORK/home"
mkdir -p "$H/.k2" "$WORK/proj/.k2"
OWNER="ownerSECRETtoken0123456789"
PASSPORT="sess42.agentPASSPORTsecret"
printf '%s\n' "$OWNER" >"$H/.k2/heartbeat.token"
printf '%s\n' "$OWNER" >"$H/.k2/daemon.token"
chmod 600 "$H/.k2/heartbeat.token" "$H/.k2/daemon.token"
SOCK="$WORK/cell.sock"
REQ="$WORK/requests.log"
: >"$REQ"
VALUE="k2test_VAULT_RUN_SECRET_42"

python3 - "$WORK" "$SOCK" "$DAEMON" "$VALUE" <<'PYEOF' &
import base64, http.server, json, os, socketserver, sys, threading
work, sock, helper, value = sys.argv[1:5]
def handler(chan):
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a): pass
        def address_string(self): return chan
        def _go(self):
            n = int(self.headers.get("Content-Length") or 0)
            if n: self.rfile.read(n)
            path = self.path.split("?")[0]
            auth = self.headers.get("Authorization") or "-"
            with open(work + "/requests.log", "a") as f:
                f.write("%s %s %s %s\n" % (chan, self.command, path, auth))
            if path == "/cli/vault/run-helper":
                out = {"helper": helper, "tokenFd": True}
            elif path == "/cli/vault/run":
                out = {"runId": "r1", "items": [{"name": "KEY", "env": "KEY", "how": "env",
                        "valueB64": base64.b64encode(value.encode()).decode()}]}
            else:
                out = {"ok": True}
            data = json.dumps(out).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(data)
        do_GET = _go
        do_POST = _go
    return H
class TS(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
class US(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    def get_request(self):
        req, _ = self.socket.accept()
        return req, ("unix", 0)
tcp = TS(("127.0.0.1", 0), handler("tcp"))
uds = US(sock, handler("uds"))
threading.Thread(target=uds.serve_forever, daemon=True).start()
with open(work + "/port.tmp", "w") as f: f.write(str(tcp.server_address[1]))
os.rename(work + "/port.tmp", work + "/port")
tcp.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$WORK/port" ] && [ -S "$SOCK" ] && break; sleep 0.05; done
[ -s "$WORK/port" ] && [ -S "$SOCK" ] || { echo "FAIL: the stub daemon never started" >&2; exit 1; }
printf '%s\n' "$(cat "$WORK/port")" >"$H/.k2/heartbeat.port"

# The agent's shell: a session (socket and passport), the fake home.
agent_env=(HOME="$H" K2_HOME="$H/.k2" K2_HOOK_SOCK="$SOCK" K2_HOOK_TOKEN="$PASSPORT" K2_PROJECT_PATH="$WORK/proj")

# One verb per sender kind: "label|args".
VERBS=(
    "whoami|whoami"
    "cli_request (TCP GET)|tunnel status"
    "cli_request_post (TCP POST)|connections remove acme"
    "cli_post_json (TCP JSON)|api on"
    "cli_post_json_stdin (TCP JSON on stdin)|api-key create --status --label mon"
    "cli_post (TCP query POST)|tunnel stop"
    "_k2_idem_send + cli_post_form (socket first, form)|msg acme hello"
    "cli_send (socket first)|heartbeat list"
    "cli_send_json (socket first)|permission status"
    "_compute_send (socket first)|compute nodes"
    "_sidecar_send (socket first)|vault list"
)

echo "== under vault run: the command's environment =="
: >"$REQ"
set +e
OUT="$(cd "$WORK/proj" && env -i PATH="$PATH" "${agent_env[@]}" "$K2" vault run --secret KEY --no-mask -- /usr/bin/env 2>"$WORK/err")"
RC=$?
set -e
[ "$RC" -eq 0 ] && ok "vault run -- env: exit 0" || { bad "vault run -- env: exit $RC"; sed 's/^/      | /' "$WORK/err" >&2; }
printf '%s\n' "$OUT" | grep -qx "KEY=$VALUE" && ok "the command has the secret" || bad "no KEY=<value> in the command's env"
printf '%s\n' "$OUT" | grep -qx "K2_NO_OWNER_FALLBACK=1" && ok "the command has K2_NO_OWNER_FALLBACK=1" || bad "no K2_NO_OWNER_FALLBACK=1"
case "$OUT" in *"$PASSPORT"*) bad "the passport is in the command's env" ;; *) ok "no passport in the command's env" ;; esac
case "$OUT" in *"$OWNER"*) bad "the owner token is in the command's env" ;; *) ok "no owner token in the command's env" ;; esac
if printf '%s\n' "$OUT" | grep -E '^(K2|K5|K2SO)_[A-Z0-9_]*(TOKEN|PASSPORT)[A-Z0-9_]*=' >/dev/null; then
    bad "a K2 token variable is in the command's env: $(printf '%s\n' "$OUT" | grep -E '^(K2|K5|K2SO)_[A-Z0-9_]*(TOKEN|PASSPORT)')"
else
    ok "no K2 token variable in the command's env"
fi
grep -q "^uds POST /cli/vault/run Bearer $PASSPORT\$" "$REQ" && ok "the helper asked for the secret with the agent's passport" \
    || { bad "the helper's request didn't carry the passport"; sed 's/^/      | /' "$REQ" >&2; }

echo "== under vault run: k2 from the command, every sender kind =="
for row in "${VERBS[@]}"; do
    label="${row%%|*}"; args="${row#*|}"
    : >"$REQ"
    set +e
    # shellcheck disable=SC2086 # the verb's words
    OUT="$(cd "$WORK/proj" && env -i PATH="$PATH" "${agent_env[@]}" PROBE_CLI="$K2" PROBE_ARGS="$args" \
        "$K2" vault run --secret KEY --no-mask -- /bin/sh -c '$PROBE_CLI $PROBE_ARGS' </dev/null 2>&1)"
    RC=$?
    set -e
    [ "$RC" -eq 3 ] && ok "$label: exit 3" || bad "$label: exit $RC (want 3): $OUT"
    case "$OUT" in
        *"won't act as this server's owner here"*"K2_NO_OWNER_FALLBACK=1"*) ok "$label: the no-owner refusal" ;;
        *) bad "$label: not the no-owner refusal: $OUT" ;;
    esac
    if grep -v -e ' /cli/vault/run-helper ' -e ' /cli/vault/run ' "$REQ" | grep -q .; then
        bad "$label: the command's k2 sent a request:"; sed 's/^/      | /' "$REQ" >&2
    else
        ok "$label: the command's k2 sent nothing"
    fi
    if grep -q "$OWNER" "$REQ"; then bad "$label: the owner token reached the daemon"; fi
done

echo "== the agent's own shell, not under vault run: unchanged =="
for row in "${VERBS[@]}"; do
    label="${row%%|*}"; args="${row#*|}"
    : >"$REQ"
    set +e
    # shellcheck disable=SC2086 # the verb's words
    OUT="$(cd "$WORK/proj" && env -i PATH="$PATH" "${agent_env[@]}" "$K2" $args </dev/null 2>&1)"
    RC=$?
    set -e
    case "$OUT" in
        *"won't act as this server's owner here"*) bad "$label: refused in the agent's own shell: $OUT" ;;
        *) [ "$RC" -ne 3 ] && ok "$label: not refused" || bad "$label: exit 3 in the agent's own shell: $OUT" ;;
    esac
    if grep -q . "$REQ"; then ok "$label: sent its request" ; else bad "$label: sent nothing in the agent's own shell"; fi
    # Socket-first senders carry the passport; nothing carries anything new.
    if grep -v -e " Bearer $PASSPORT\$" -e " Bearer $OWNER\$" -e ' -$' "$REQ" | grep -q .; then
        bad "$label: a request with an unexpected credential:"; sed 's/^/      | /' "$REQ" >&2
    fi
    if grep -q '^uds ' "$REQ" && grep '^uds ' "$REQ" | grep -qv " Bearer $PASSPORT\$"; then
        bad "$label: a cell-socket request without the passport:"; sed 's/^/      | /' "$REQ" >&2
    fi
done

echo ""
echo "vault_run_child_identity: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
