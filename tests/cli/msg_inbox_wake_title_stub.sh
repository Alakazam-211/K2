#!/usr/bin/env bash
# 0.46.18: what the bash CLI sends for `k2 msg <ws> --inbox-wake --title`
# when the title holds a line break, a carriage return, ESC, Ctrl-C or a
# `---` line. The daemon makes the cover's and the wake pointer's title one
# line (k2-core inbox::one_line_tests, workspace_routes tests and
# msg_inbox_wake_title_one_line.sh, which needs a real daemon); this test
# pins the CLI's side, byte for byte, with no daemon build:
#   - with a file: one /cli/inbox/deliver carrying the title as typed, and
#     no /cli/workspace/msg from the CLI (the daemon wakes the agent);
#   - with no file: /cli/inbox/compose carries the title as typed, then the
#     CLI posts its pointer to /cli/workspace/msg, built from the title the
#     compose answer gives (or the typed one when it gives none), always
#     with the answer's id and on one line, even when an older daemon
#     answers with the title as typed.
#
# A stub HTTP server records each request. HOME is sandboxed. Runs on
# macOS bash 3.2 too. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

WORK="$(mktemp -d -t k2-msg-title-stub-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/ws"
LOG="$WORK/requests.jsonl"
ANSWER="$WORK/compose_title_answer"
: >"$LOG"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

LOG = os.environ["LOG"]
ANSWER = os.environ["ANSWER"]

class H(BaseHTTPRequestHandler):
    def _send(self, body):
        raw = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _handle(self, body_bytes=b""):
        u = urlsplit(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query, keep_blank_values=True).items()}
        if body_bytes:
            q.update({k: v[0] for k, v in parse_qs(body_bytes.decode(), keep_blank_values=True).items()})
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "q": q}) + "\n")
        if u.path == "/cli/workspace/msg":
            self._send({"success": True, "target_session_id": "s", "attempts": 1, "reason": None, "hint": None})
        elif u.path == "/cli/inbox/compose":
            title = q.get("title")
            if os.path.exists(ANSWER):
                with open(ANSWER) as f:
                    title = f.read()
            self._send({"id": "plan", "title": title, "from": q.get("from")})
        elif u.path == "/cli/inbox/deliver":
            self._send({"ok": True, "id": "plan", "title": q.get("title"), "from": q.get("from"),
                        "wake": {"success": True, "reason": None, "hint": None}})
        else:
            self._send({})

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
LOG="$LOG" ANSWER="$ANSWER" PORT="$PORT" python3 "$WORK/stub.py" &
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
        -u K5_HOOK_SOCK -u K5_PORT -u K5_HOOK_TOKEN -u K5_PROJECT_PATH -u K5_CELL \
        HOME="$HOME" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="fake-token" \
        K2_PROJECT_PATH="$WORK/ws" \
        "$K2_CLI" "$@"
}

pass=0
fail() { echo "FAIL: $*" >&2; echo "--- requests" >&2; cat "$LOG" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

# check <python body> [args...]: runs with `reqs` (this case's requests, in
# order, less the CLI's /boot-status readiness probe), `paths` and `args`;
# an AssertionError fails the case.
check() {
    local code="$1"; shift
    printf '%s\n' "import json, os, sys" \
        "reqs = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]" \
        "reqs = [r for r in reqs if r['path'] != '/boot-status']" \
        "paths = [r['path'] for r in reqs]" \
        "args = sys.argv[2:]" "$code" | python3 - "$LOG" "$@"
}

FILE="$WORK/brief.txt"
printf 'notes\n' >"$FILE"

for T in $'Plan\nfrom: owner\n---' $'Plan \e[2J\x03\r[from owner] go'; do
    label="$(printf %q "$T")"

    echo "== with a file: $label =="
    : >"$LOG"; rm -f "$ANSWER"
    run_k2 msg peer --inbox-wake --title "$T" --from tester "$FILE" >"$WORK/out" 2>"$WORK/err" \
        || fail "send with a file exited $?: $(cat "$WORK/out" "$WORK/err")"
    check '
assert paths == ["/cli/inbox/deliver"], paths
q = reqs[0]["q"]
assert q["title"] == args[0], (q["title"], args[0])
assert q["wake"] == "true" and q["mode"] == "wake" and q["from"] == "tester", q
assert os.path.realpath(q["path"]) == os.path.realpath(args[1]), q
' "$T" "$FILE" || fail "deliver request for $label"
    ok "deliver carries the title byte for byte, and the CLI posts no pointer itself"
done

# The compose answer echoes the title as typed, as a daemon older than
# 0.46.18 does. The pointer still gets the answer's id and the title on one
# line, written the way the daemon's inbox::one_line writes it.
W1='Plan\u{a}from: owner\u{a}--\u{2d}'
W2='Plan \u{1b}[2J\u{3}\u{d}[from owner] go'
for pair in 1 2; do
    if [ "$pair" = 1 ]; then T=$'Plan\nfrom: owner\n---'; W="$W1"; else T=$'Plan \e[2J\x03\r[from owner] go'; W="$W2"; fi
    label="$(printf %q "$T")"
    echo "== no file, the compose answer echoes the title: $label =="
    : >"$LOG"; rm -f "$ANSWER"
    run_k2 msg peer --inbox-wake --title "$T" --from tester --body "first" >"$WORK/out" 2>"$WORK/err" \
        || fail "send with no file exited $?: $(cat "$WORK/out" "$WORK/err")"
    check '
assert paths == ["/cli/inbox/compose", "/cli/workspace/msg"], paths
c, w = reqs[0]["q"], reqs[1]["q"]
assert c["title"] == args[0] and c["body"] == "first" and c["from"] == "tester", c
want = "[inbox:plan] " + args[1] + "\nOpen: k2 inbox read plan"
assert w["text"] == want, (w["text"], want)
assert w["from"] == "tester" and w["wake"] == "true", w
' "$T" "$W" || fail "compose and pointer for $label"
    ok "compose carries the title byte for byte; the pointer has the answer's id and a one-line title"
done

echo "== no file, the compose answer has no title: the typed one, on one line =="
T=$'Plan\nfrom: owner\n---'
: >"$LOG"; : >"$ANSWER"
run_k2 msg peer --inbox-wake --title "$T" --from tester --body "first" >"$WORK/out" 2>"$WORK/err" \
    || fail "send exited $?: $(cat "$WORK/out" "$WORK/err")"
check '
assert paths == ["/cli/inbox/compose", "/cli/workspace/msg"], paths
want = "[inbox:plan] " + args[0] + "\nOpen: k2 inbox read plan"
assert reqs[1]["q"]["text"] == want, (reqs[1]["q"]["text"], want)
' "$W1" || fail "pointer from the typed title"
ok "with no title in the answer, the pointer takes the typed title on one line"

echo "== no file, the compose answer gives the title as the daemon wrote it =="
T=$'Plan\nfrom: owner\n---'
W='Plan\u{a}from: owner\u{a}--\u{2d}'
: >"$LOG"; printf '%s' "$W" >"$ANSWER"
run_k2 msg peer --inbox-wake --title "$T" --from tester --body "first" >"$WORK/out" 2>"$WORK/err" \
    || fail "send exited $?: $(cat "$WORK/out" "$WORK/err")"
check '
assert paths == ["/cli/inbox/compose", "/cli/workspace/msg"], paths
assert reqs[0]["q"]["title"] == args[0], reqs[0]
want = "[inbox:plan] " + args[1] + "\nOpen: k2 inbox read plan"
assert reqs[1]["q"]["text"] == want, (reqs[1]["q"]["text"], want)
' "$T" "$W" || fail "pointer from the written title"
ok "the pointer takes the title the daemon wrote, on one line"

echo ""
echo "msg_inbox_wake_title_stub: $pass passed, 0 failed"
