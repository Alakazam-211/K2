#!/usr/bin/env bash
# `k2 terminal spawn --sandbox` where this computer can't sandbox: K2's
# server answers 409 with one plain line and runs nothing; the CLI prints
# that line as `Error: …` on stderr, exits 1 and never says "Terminal
# spawned" (0.46.17). Before, the server ignored the flag, the session ran
# unsandboxed and the CLI printed "Terminal spawned" with exit 0.
#
# The daemon side (409, no session, nothing run) is pinned by the daemon
# tests `terminal_spawn_sandbox_refuses_409_and_runs_nothing` and
# `spawn_with_sandbox_true_refuses_409_and_spawns_nothing`; this row pins
# what the CLI sends and how it reports the refusal.
#
# How: a python stub daemon on a free loopback port answers
# POST /cli/terminal/spawn the way K2's server does: 409 with the refusal
# for sandbox=true, else 200 with "sandbox":"none". HOME is a temp dir, no
# real daemon, no network. Loud: every failure is named and the script
# exits 1.
# Run with: bash tests/cli/terminal_sandbox_refusal.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-terminal-sandbox-XXXXXX)"
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

LINE="this computer can't run a sandboxed session. Run it without --sandbox, knowing it runs as you."
REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"

python3 - "$WORK" "$LINE" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work, line = sys.argv[1], sys.argv[2]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _reply(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        u = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(u.query, keep_blank_values=True)
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s sandbox=%s\n" % (self.command, u.path, ",".join(q.get("sandbox", ["<none>"]))))
        if u.path == "/cli/terminal/spawn" and q.get("sandbox", ["false"])[0] not in ("", "false", "0", "no", "off"):
            self._reply(409, {"error": line, "code": "sandbox_unavailable"})
            return
        self._reply(200, {"success": True, "terminalId": "t1", "agentName": "agent-a", "sandbox": "none"})
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

run() {
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

echo "== --sandbox where this computer can't sandbox =="
run terminal spawn --command ls --sandbox
[ "$RC" -eq 1 ] && ok "exit 1" || bad "exit $RC (want 1)"
[ "$ERR" = "Error: $LINE" ] && ok "stderr is the one refusal line" \
    || { bad "stderr is not the refusal line"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
case "$OUT" in *"Terminal spawned"*) bad "printed Terminal spawned for a refused sandbox" ;; *) ok "no Terminal spawned line" ;; esac
[ "$(grep -c '' "$REQ_LOG")" -eq 1 ] && ok "one request" || { bad "not one request"; sed 's/^/      | /' "$REQ_LOG" >&2; }
grep -qx 'POST /cli/terminal/spawn sandbox=true' "$REQ_LOG" && ok "the request asks for the sandbox" \
    || { bad "the request does not carry sandbox=true"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo "== --no-sandbox is the plain spawn =="
run terminal spawn --command ls --no-sandbox
[ "$RC" -eq 0 ] && ok "exit 0" || bad "exit $RC (want 0): $ERR"
case "$OUT" in *"Terminal spawned: ls"*) ok "Terminal spawned" ;; *) bad "no Terminal spawned line: $OUT" ;; esac
grep -qx 'POST /cli/terminal/spawn sandbox=false' "$REQ_LOG" && ok "sends sandbox=false" \
    || { bad "--no-sandbox did not send sandbox=false"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo "== no flag sends no sandbox param =="
run terminal spawn --command ls
[ "$RC" -eq 0 ] && ok "exit 0" || bad "exit $RC (want 0): $ERR"
grep -qx 'POST /cli/terminal/spawn sandbox=<none>' "$REQ_LOG" && ok "no sandbox param" \
    || { bad "a plain spawn sent a sandbox param"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo ""
echo "terminal_sandbox_refusal: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
