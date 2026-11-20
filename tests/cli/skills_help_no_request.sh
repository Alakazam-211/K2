#!/usr/bin/env bash
# -h or --help anywhere on `k2 skills` prints its help, exits 0 and sends
# nothing.
#
# Regression (0.46.17): `cmd_skills` skips words it does not know, so `k2 skills create <name> -h` created the skill, `remove <name> -h` removed it, `regenerate -h` regenerated every skill and `list -h` listed them.
#
# Every help form below prints exactly the reference help, on stdout,
# nothing on stderr, exits 0, and the stub daemon records no request at
# all. Then the plain forms still send their request, so the stub is known
# to see one when it is sent.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/skills_help_no_request.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-skills-help-XXXXXX)"
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

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty, so
# a form that reads stdin can't hang.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# ref <args...>: the reference help, printed with nothing sent.
ref() {
    : >"$REQ_LOG"
    run "$@"
    REF="$OUT"
    [ "$RC" -eq 0 ] && ok "k2 $*: exit 0" || bad "k2 $*: exit $RC (want 0)"
    [ -n "$REF" ] && ok "k2 $*: prints help" || bad "k2 $*: printed nothing"
    if [ -s "$REQ_LOG" ]; then
        bad "k2 $* sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "k2 $* sends nothing"
    fi
}

# check <args...>: prints exactly REF, nothing else, sends nothing.
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$REF" ] && ok "$label: prints the help" \
        || { bad "$label: stdout is not the help"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
}

echo "== k2 skills =="
ref skills --help
case "$REF" in "Usage: k2 skills <list|create|profile|remove|regenerate> [options]"*) ok "the skills usage" ;; *) bad "not the skills usage: $REF" ;; esac
check skills -h
check skills list -h
check skills create s1 -h
check skills create s1 --role --help
check skills create --template --help s1
check skills profile s1 -h
check skills remove s1 --help
check skills delete s1 -h
check skills regenerate -h
echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"; run skills create s1
grep -q '^POST /cli/agents/create ' "$REQ_LOG" && ok "k2 skills create s1 still sends its request" || { bad "create sent no request"; sed 's/^/      | /' "$REQ_LOG" >&2; }
: >"$REQ_LOG"; run skills list
grep -q '^GET /cli/agents/list ' "$REQ_LOG" && ok "k2 skills list still sends its request" || { bad "list sent no request"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo ""
echo "skills_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
