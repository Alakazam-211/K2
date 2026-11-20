#!/usr/bin/env bash
# `k2 msg <workspace> --signal <kind>` hands every value to Python as data,
# so the signal it sends carries each value exactly as given.
#
# cmd_signal builds the AgentSignal body in Python. Its kind, payload,
# sender name and target name reach that Python on stdin, never inside the
# program text. This test sends signals whose kind, payload, --from and
# target hold a double quote, three double quotes, a single quote, a
# backslash and a newline, and checks the body the stub daemon got: one
# JSON object whose fields are those values, byte for byte.
#
# How: a python stub daemon on a free loopback port keeps every request
# body. HOME is a temp dir, no real daemon, no network. Loud: every failure
# is named and the script exits 1.
# Run with: bash tests/cli/signal_values_as_data.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-signal-data-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

REQ_LOG="$WORK/requests.jsonl"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "surrogateescape") if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.jsonl", "a") as f:
            f.write(json.dumps({"method": self.command, "path": path, "body": body}) + "\n")
        out = b'{"ok": true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

# Each value holds ", """, ', a backslash and a newline.
NL=$'\n'
KIND="k\"i\"\"\"n'd\\x${NL}y"
TARGET="ta\"r\"\"\"g'e\\t${NL}z"
FROM="fr\"o\"\"\"m'\\n${NL}w"
RAW="pa\"y\"\"\"l'o\\a${NL}d"

# check_body <label> <python assertions on s (the signal) and E (the want values)>
check_body() {
    local label="$1" asserts="$2"
    local n
    n="$(python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
print(sum(1 for r in rows if r["method"] == "POST" and r["path"] == "/cli/awareness/publish"))
' "$REQ_LOG")"
    if [ "$n" != "1" ]; then
        bad "$label: want one POST /cli/awareness/publish, got $n"; cat "$REQ_LOG" >&2; return
    fi
    local err
    if err="$(printf '%s\0' "$KIND" "$TARGET" "$FROM" "$RAW" | python3 -c '
import json, sys
E = dict(zip(("kind", "target", "from", "raw"), sys.stdin.buffer.read().decode("utf-8", "surrogateescape").split("\0")))
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
r = [r for r in rows if r["path"] == "/cli/awareness/publish"][0]
s = json.loads(r["body"])
exec(sys.argv[2])
' "$REQ_LOG" "$asserts" 2>&1)"; then
        ok "$label"
    else
        bad "$label: $err"
        cat "$REQ_LOG" >&2
    fi
}

echo "== a custom kind, a text payload, --from and the target =="
: >"$REQ_LOG"
set +e
OUT="$("$K2" msg "$TARGET" --signal "$KIND" --payload "$RAW" --from "$FROM" </dev/null 2>"$WORK/err")"
RC=$?
set -e
[ "$RC" -eq 0 ] && ok "custom signal: exit 0" || { bad "custom signal: exit $RC"; cat "$WORK/err" >&2; }
check_body "custom signal: kind, payload, from and target arrive exactly" '
assert s["kind"] == {"kind": "custom", "data": {"kind": E["kind"], "payload": {"text": E["raw"]}}}, s["kind"]
assert s["from"]["name"] == E["from"], s["from"]
assert s["to"]["name"] == E["target"], s["to"]
assert s["delivery"] == "live", s["delivery"]
assert s["priority"] == "normal", s["priority"]
assert s["from"]["workspace"] == s["to"]["workspace"] != "", (s["from"], s["to"])
'

echo "== kind msg, a JSON payload whose text holds the same characters, --inbox =="
: >"$REQ_LOG"
JSON_PAYLOAD="$(printf '%s' "$RAW" | python3 -c 'import json, sys; print(json.dumps({"text": sys.stdin.read()}))')"
set +e
OUT="$("$K2" msg "$TARGET" --signal msg --payload "$JSON_PAYLOAD" --from "$FROM" --inbox-wake </dev/null 2>"$WORK/err")"
RC=$?
set -e
[ "$RC" -eq 0 ] && ok "msg signal: exit 0" || { bad "msg signal: exit $RC"; cat "$WORK/err" >&2; }
check_body "msg signal: the text, from and target arrive exactly, delivery inbox" '
assert s["kind"] == {"kind": "msg", "data": {"text": E["raw"]}}, s["kind"]
assert s["from"]["name"] == E["from"], s["from"]
assert s["to"]["name"] == E["target"], s["to"]
assert s["delivery"] == "inbox", s["delivery"]
'

echo ""
echo "signal_values_as_data: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
