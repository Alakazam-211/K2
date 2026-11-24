#!/usr/bin/env bash
# 0.46.18: a federated `k2 msg <agent>::<host>` that could not be
# delivered is easy to spot. K2's daemon retries the other server briefly
# (crates/k2-daemon/src/federation_retry.rs); what reaches the CLI is the
# outcome, and the CLI shows it plainly:
#   - the roster step failed: exit non-zero, one line on stderr naming the
#     host, the step, the attempts and the last error, "Nothing was
#     delivered; send again", nothing on stdout, and no send is made;
#   - the send step left it queued: exit 0 (the outbox delivers it later),
#     and a QUEUED (not delivered yet) line with the attempts;
#   - a file package (--inbox-wake a.md b.md) whose roster failed: exit
#     non-zero, one line naming both files;
#   - a file package left queued: exit 0 and a QUEUED line naming both
#     files (it used to print only the raw answer, which read like a
#     delivery).
#
# No real daemon: a python stub on loopback answers as the daemon does.
# HOME is sandboxed. Never touches the real ~/.k2. Runs on the bash CLI and
# (scripts/cli-parity.sh) on the k5 binary.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    if [ "$2" = "$3" ]; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (got=$(printf %q "$2") want=$(printf %q "$3"))" >&2; fail=$((fail + 1)); fi
}

WORK="$(mktemp -d -t k2-msg-notdeliv-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/ws"
printf 'one\n' >"$WORK/ws/a.md"
printf 'two\n' >"$WORK/ws/b.md"

ROSTER_LINE='the roster step failed after 3 attempts (last error: GET https://rosson.k2.dev/cli/federation/roster: error sending request: operation timed out). Nothing was delivered; send again.'
SEND_WHY='the send step got no delivery after 3 attempts (last error: peer rejected inbound: HTTP 503)'
TRAY_LINE="not delivered to bob::rosson.k2.dev (2 files: a.md, b.md): $ROSTER_LINE"
TRAY_HINT="2 files: a.md, b.md for bob::rosson.k2.dev is not delivered yet: $SEND_WHY. K2 retries it from this server's outbox (next try in about 30 s); don't send it again."

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

MODEF, LOG = os.environ["MODEF"], os.environ["LOG"]

def mode():
    return open(MODEF).read().strip()

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _handle(self):
        p = urlsplit(self.path).path
        with open(LOG, "a") as f:
            f.write(p + "\n")
        m = mode()
        if p == "/cli/federation/peers":
            self._send(200, {"peers": [{"fingerprint": "fp1", "subdomain": "rosson", "label": "rosson", "trust": "trusted"}]})
        elif p == "/cli/federation/peer-roster":
            if m == "roster-fails":
                self._send(502, {"error": os.environ["ROSTER_LINE"], "step": "roster", "attempts": 3})
            else:
                self._send(200, {"peer": "fp1", "roster": {"agents": [{"agent": "bob", "workspace_id": "ws-b", "workspace_name": "Bob", "aliases": []}]}})
        elif p == "/cli/federation/send":
            self._send(200, {"status": "queued", "delivered": False, "reason": os.environ["SEND_WHY"], "step": "send", "attempts": 3, "msg_uuid": "u1", "peer": "fp1", "hint": "delivery deferred to retry: " + os.environ["SEND_WHY"]})
        elif p == "/cli/inbox/deliver":
            if m == "tray-roster-fails":
                self._send(502, {"error": os.environ["TRAY_LINE"], "step": "roster", "attempts": 3})
            else:
                self._send(200, {"status": "queued", "delivered": False, "reason": os.environ["SEND_WHY"], "msg_uuid": "u2", "peer": "fp1", "hint": os.environ["TRAY_HINT"]})
        else:
            self._send(200, {})

    def do_GET(self):
        self._handle()

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        self._handle()

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo "$PORT" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
: >"$WORK/log"
echo "roster-fails" >"$WORK/mode"
PORT="$PORT" MODEF="$WORK/mode" LOG="$WORK/log" ROSTER_LINE="$ROSTER_LINE" SEND_WHY="$SEND_WHY" \
    TRAY_LINE="$TRAY_LINE" TRAY_HINT="$TRAY_HINT" python3 "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 40); do
    if curl -s -o /dev/null --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/x"; then up=1; break; fi
    sleep 0.05
done
[ "$up" = "1" ] || { echo "FAIL: stub server did not start" >&2; exit 1; }

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2SO_PROJECT_PATH \
        -u K2_CELL -u K2_PRIMARY -u K2_SESSION_ID -u K2_PANE_ID -u K2_TAB_ID \
        HOME="$HOME" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="fake-token" \
        K2_PROJECT_PATH="$WORK/ws" \
        "$K2_CLI" "$@"
}

# run <mode> <args…>: run the CLI in that stub mode; $rc, $out, $err.
run() {
    echo "$1" >"$WORK/mode"; shift
    : >"$WORK/log"
    set +e
    (cd "$WORK/ws" && run_k2 "$@") >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
    out="$(cat "$WORK/out")"
    err="$(cat "$WORK/err")"
}

echo "== live text: the roster step failed =="
run roster-fails msg bob::rosson.k2.dev "part 2 of 3"
assert_eq "exit non-zero (2)" "$rc" "2"
assert_eq "one line on stderr" "$err" "could not reach rosson.k2.dev's roster: $ROSTER_LINE"
assert_eq "nothing on stdout" "$out" ""
assert_eq "no send was made" "$(grep -c '^/cli/federation/send$' "$WORK/log" || true)" "0"

echo "== live text: the send step left it queued =="
run send-queued msg bob::rosson.k2.dev "part 3 of 3"
assert_eq "exit 0 (the outbox has it)" "$rc" "0"
assert_eq "no delivered line" "$out" ""
assert_eq "the QUEUED line names the step and the attempts" \
    "$(printf '%s\n' "$err" | head -n 1)" \
    "QUEUED (not delivered yet) for bob::rosson.k2.dev: $SEND_WHY"

echo "== a two-file package: the roster step failed =="
run tray-roster-fails msg bob::rosson.k2.dev --inbox-wake a.md b.md
assert_eq "exit non-zero (1)" "$rc" "1"
assert_eq "nothing on stdout" "$out" ""
assert_eq "one line naming both files" "$err" \
    "$(python3 -c 'import json,sys; print(json.dumps({"error":{"code":"error","hint":sys.argv[1]}},ensure_ascii=False,separators=(",",":")))' "$TRAY_LINE")"

echo "== a two-file package left queued =="
run tray-queued msg bob::rosson.k2.dev --inbox-wake a.md b.md
assert_eq "exit 0 (the outbox has it)" "$rc" "0"
assert_eq "the QUEUED line names both files" "$err" "QUEUED (not delivered yet): $TRAY_HINT"

echo
echo "msg_federated_not_delivered: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
