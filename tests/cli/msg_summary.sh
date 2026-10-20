#!/usr/bin/env bash
# 0.46.3: `k2 msg --summary` (plan-projects-threads-chatter-v1 "Summaries").
#   - live `k2 msg <ws> --summary "…" "text"` sends `summary=` with the text;
#   - a message over ~600 characters with no --summary prints ONE stderr
#     hint and is still sent (a hint, never an error); short ones or ones
#     with a summary print none;
#   - `--inbox-wake|--inbox-silent --summary` and `k2 inbox compose
#     --summary` send it too, and `k2 inbox list` shows an item's summary.
#
# No real daemon: a stub HTTP server records each request. HOME is
# sandboxed. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    if [ "$2" = "$3" ]; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (got=$(printf %q "$2") want=$(printf %q "$3"))" >&2; fail=$((fail + 1)); fi
}
assert_contains() {
    if printf '%s' "$2" | grep -Fq -- "$3"; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (missing $(printf %q "$3") in $(printf %q "$2"))" >&2; fail=$((fail + 1)); fi
}
assert_not_contains() {
    if printf '%s' "$2" | grep -Fq -- "$3"; then echo "  FAIL: $1 (found $(printf %q "$3"))" >&2; fail=$((fail + 1))
    else echo "  PASS: $1"; pass=$((pass + 1)); fi
}

WORK="$(mktemp -d -t k2-msg-summary-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/ws"
LOG="$WORK/requests.jsonl"
: >"$LOG"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

LOG = os.environ["LOG"]

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _handle(self, body_bytes=b""):
        u = urlsplit(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query, keep_blank_values=True).items()}
        if body_bytes:
            try:
                q.update(json.loads(body_bytes))
            except Exception:
                q.update({k: v[0] for k, v in parse_qs(body_bytes.decode(), keep_blank_values=True).items()})
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "q": q}) + "\n")
        if u.path == "/cli/workspace/msg":
            self._send(200, {"success": True, "target_session_id": "s", "attempts": 1, "reason": None, "hint": None, "deliveryId": "m_TEST000000"})
        elif u.path == "/cli/inbox/compose":
            self._send(200, {"id": "brief", "title": q.get("title"), "summary": q.get("summary")})
        elif u.path == "/cli/inbox/list":
            self._send(200, [
                {"id": "review-x", "folder": "", "priority": "normal", "title": "Review hold/x", "summary": "Need a review by 5pm"},
                {"id": "plain", "folder": "", "priority": "normal", "title": "Plain item"},
            ])
        else:
            self._send(200, {})

    def do_GET(self):
        self._handle()

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._handle(self.rfile.read(n))

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo "$PORT" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
LOG="$LOG" PORT="$PORT" python3 "$WORK/stub.py" &
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
last_for() {
    grep "\"path\": \"$1\"" "$LOG" | tail -n 1 | python3 -c 'import json,sys; d=json.loads(sys.stdin.read())["q"]; k=sys.argv[1]; print(d[k] if k in d else "<absent>")' "$2"
}

echo "== live msg --summary rides with the text =="
set +e
out="$(run_k2 msg peer --from tester --summary "Need a review of hold/x" "short body" 2>"$WORK/err")"
rc=$?
set -e
err="$(cat "$WORK/err")"
assert_eq "msg --summary exit" "$rc" "0"
assert_eq "summary param sent" "$(last_for /cli/workspace/msg summary)" "Need a review of hold/x"
assert_eq "text param unchanged" "$(last_for /cli/workspace/msg text)" "short body"
assert_not_contains "no hint with a summary" "$err" "hint:"

echo "== a long message without --summary: one hint, still sent =="
long="$(python3 -c 'print("This is a long status report. " * 30)')"
set +e
out="$(run_k2 msg peer --from tester "$long" 2>"$WORK/err")"
rc=$?
set -e
err="$(cat "$WORK/err")"
assert_eq "long msg exit (hint, not an error)" "$rc" "0"
assert_contains "long msg hint" "$err" "with no --summary. Add --summary"
assert_eq "one hint line" "$(printf '%s\n' "$err" | grep -c '^hint:')" "1"
assert_eq "long msg still sent" "$(last_for /cli/workspace/msg text)" "$long"
assert_eq "no summary param without the flag" "$(last_for /cli/workspace/msg summary)" "<absent>"

echo "== a long message WITH --summary: no hint =="
set +e
run_k2 msg peer --from tester --summary "Status: all green" "$long" >/dev/null 2>"$WORK/err"
set -e
assert_not_contains "no hint when summarized" "$(cat "$WORK/err")" "hint:"

echo "== a short message: no hint =="
set +e
run_k2 msg peer --from tester "ok" >/dev/null 2>"$WORK/err"
set -e
assert_not_contains "no hint on a short message" "$(cat "$WORK/err")" "hint:"

echo "== inbox compose --summary and msg --inbox-silent --summary =="
set +e
run_k2 inbox compose --title "Brief" --body "details" --summary "Read before standup" >/dev/null 2>&1
run_k2 msg peer --inbox-silent --title "Tray note" --body "body" --summary "FYI: new numbers" >/dev/null 2>&1
set -e
assert_eq "inbox compose sends summary" "$(grep '"title": "Brief"' "$LOG" | tail -n1 | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["q"].get("summary"))')" "Read before standup"
assert_eq "tray note sends summary" "$(grep '"title": "Tray note"' "$LOG" | tail -n1 | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["q"].get("summary"))')" "FYI: new numbers"

echo "== inbox list shows a summary =="
out="$(run_k2 inbox list 2>&1)"
assert_contains "list shows the summary" "$out" "summary: Need a review by 5pm"
assert_contains "list still shows titles" "$out" "Plain item"
assert_eq "one summary line (none for the plain item)" "$(printf '%s\n' "$out" | grep -c 'summary:')" "1"

echo
echo "msg_summary: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
