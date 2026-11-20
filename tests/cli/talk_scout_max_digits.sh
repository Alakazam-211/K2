#!/usr/bin/env bash
# K2_TALK_SCOUT_MAX is used as a number only when it is plain digits; any
# other value falls back to the default cap (20), and nothing in it runs.
#
# Regression (0.46.17): talk_scout compared `[[ "$i" -ge
# "$K2_TALK_SCOUT_MAX" ]]`, and `[[ -ge ]]` evaluates its operands as
# arithmetic, so a value holding an array subscript with a command
# substitution ran that command. Now the value is checked for digits
# first.
#
# A stub daemon serves k2 talk <ws> --scout (resolve, the session list, a
# new screen on every read) and counts the arrow writes: a cap of N walks
# N-1 questions right and back, 2*(N-1) writes.
#   - a hostile cap runs nothing (its marker file never appears) and walks
#     the default 20 (38 writes), as with no value set;
#   - K2_TALK_SCOUT_MAX=3 still caps the walk (4 writes).
# Hermetic: temp HOME, a stub daemon, no network. Loud.
# Run with: bash tests/cli/talk_scout_max_digits.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-scout-max-XXXXXX)"
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

REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _reply(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        if path == "/cli/workspace/resolve":
            return self._reply({"path": "/stub/ws"})
        if path == "/cli/sessions/list-for-workspace":
            return self._reply([{"isV2": True, "agentName": "a", "sessionId": "s1"}])
        if path == "/cli/terminal/read":
            # A new screen every read, so the walk stops at the cap; after
            # 60 requests the screen stays put, so a walk with no working
            # cap still ends (the fallback stops on a screen that stopped
            # changing) instead of running forever.
            n = min(sum(1 for _ in open(work + "/requests.log")), 60)
            return self._reply({"lines": ["Question screen %d" % n]})
        self._reply({"ok": True})
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

export K2_TALK_NAV_SETTLE=0
# scout [env...]: sets OUT, ERR, RC and WRITES (arrow writes sent).
scout() {
    : >"$REQ_LOG"
    set +e
    OUT="$(env "$@" "$K2" talk acme --scout </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
    WRITES="$(grep -c '^POST /cli/terminal/write ' "$REQ_LOG" || true)"
}
MARK="$WORK/hostile-ran"
# A name that is set (i, the loop counter; PATH): under set -u an unset
# name stops bash before the subscript, a set one runs it.
scout K2_TALK_SCOUT_MAX="i[\$(touch $MARK)]"
[ ! -e "$MARK" ] && ok "a hostile cap runs nothing" || bad "a hostile cap ran its command"
[ "$RC" -eq 0 ] && ok "a hostile cap: exit 0" || bad "a hostile cap: exit $RC: $ERR"
[ "$WRITES" = 38 ] && ok "a hostile cap walks the default 20 (38 writes)" || bad "a hostile cap: $WRITES writes (want 38)"
scout K5_TALK_SCOUT_MAX="PATH[\$(touch $MARK)]"
[ ! -e "$MARK" ] && ok "a hostile cap under its K5_ name runs nothing" || bad "the K5_ name ran its command"
scout
[ "$WRITES" = 38 ] && ok "no cap set: the default 20 (38 writes)" || bad "no cap: $WRITES writes (want 38)"
scout K2_TALK_SCOUT_MAX=3
[ "$WRITES" = 4 ] && ok "K2_TALK_SCOUT_MAX=3 caps the walk (4 writes)" || bad "cap 3: $WRITES writes (want 4)"

echo ""
echo "talk_scout_max_digits: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
