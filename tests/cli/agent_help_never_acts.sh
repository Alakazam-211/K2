#!/usr/bin/env bash
# Help never acts on `k2 agent`: -h or --help anywhere before `--` prints
# the help on stdout, exits 0, and the stub daemon records no request.
#
# Regression (0.46.17), each found by the k5 agent lane:
#   - `k2 agent team unlink --help` sent an unlink for the link `--help`.
#   - `k2 agent instances show --help` asked for the instance `--help`.
#   - `k2 agent hire-root set --help` (or `set <dir> --help`) set the
#     hire root.
#   - `k2 agent update ... --help` ran the update: the loop skipped the
#     word, and a flag took it as its value.
#   - `k2 agent complete ... -h` ran the complete the same way.
#   - `k2 agent profile --help` asked for the profile of `--help`.
#
# The page: `team`, `instances` and `hire-root` print the hire verbs' help
# (cli/text/help/agent/instances.txt, which lists all three). `update`,
# `complete` and `profile` have no page of their own: they print the agent
# page (cli/text/help/agent.txt).
#
# Each help row: exactly the page on stdout, nothing on stderr, exit 0,
# nothing sent. Each verb's plain form then still sends, so the stub is
# known to see a request.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/agent_help_never_acts.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
TEXT="$SCRIPT_DIR/../../cli/text/help"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-agent-help-XXXXXX)"
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
ANSWERS = {
    "/cli/agent-teams/unlink": {"ok": True, "changed": True, "link": "l1"},
    "/cli/agent-instances": {"error": {"code": "not_found", "hint": "no hire hi_1"}},
    "/cli/agents/hire-root": {"ok": True, "root": "/srv/x"},
    "/cli/agent/update": {"success": True},
    "/cli/agent/complete": {"ok": True},
    "/cli/workspace/agent-display-name": {"display_name": "x"},
}
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        raw = json.dumps(ANSWERS.get(path, {})).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
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

show() { printf '%s\n' "$1" | sed 's/^/      | /' >&2; }

sent_nothing() {
    if [ -s "$REQ_LOG" ]; then
        bad "$1: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$1: sent nothing"
    fi
}

# page <file> <args...>: exactly cli/text/help/<file> on stdout, nothing
# on stderr, exit 0, nothing sent.
page() {
    local file="$1"; shift
    local label="k2 $*" want
    want="$(cli_page "$TEXT/$file")"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$want" ] && ok "$label: prints $file" \
        || { bad "$label: stdout is not $file"; show "$OUT"; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; show "$ERR"; }
    sent_nothing "$label"
}

# sends <code> <pattern> <args...>: exit <code>, and exactly one request,
# which matches <pattern> (grep -E).
sends() {
    local code="$1" pat="$2"; shift 2
    local label="k2 $*"
    run "$@"
    [ "$RC" -eq "$code" ] && ok "$label: exit $code" \
        || { bad "$label: exit $RC (want $code)"; show "$ERR"; }
    if [ "$(wc -l <"$REQ_LOG" | tr -d ' ')" = 1 ] && grep -Eq -- "$pat" "$REQ_LOG"; then
        ok "$label: one request, $pat"
    else
        bad "$label: not one request $pat:"; sed 's/^/      | /' "$REQ_LOG" >&2
    fi
}

echo "== agent team: help anywhere is the hire verbs' help =="
# native: agent team unlink --help
page agent/instances.txt agent team unlink --help
page agent/instances.txt agent team unlink -h
page agent/instances.txt agent team unlink l1 --help
page agent/instances.txt agent team unlink --help l1
page agent/instances.txt agent team -h unlink l1
page agent/instances.txt agent teams unlink -h
page agent/instances.txt agent team link t w --help
page agent/instances.txt agent team link t w --scope -h
page agent/instances.txt agent team served --workspace --help
echo "== agent team: the plain unlink still sends =="
sends 0 '^POST /cli/agent-teams/unlink .*"link": "l1"' agent team unlink l1

echo "== agent instances: help anywhere is the hire verbs' help =="
# native: agent instances show --help
page agent/instances.txt agent instances show --help
page agent/instances.txt agent instances show -h
page agent/instances.txt agent instances show hi_1 --help
page agent/instances.txt agent instances show hi_1 --json -h
page agent/instances.txt agent instance show -h
page agent/instances.txt agent instances retire hi_1 --force --help
page agent/instances.txt agent instances hire recipe.json --key -h
page agent/instances.txt agent instances --key --help
echo "== agent instances: the plain show still asks =="
sends 4 '^GET /cli/agent-instances ' agent instances show hi_1

echo "== agent hire-root: help anywhere is the hire verbs' help =="
# native: agent hire-root set --help
page agent/instances.txt agent hire-root set --help
page agent/instances.txt agent hire-root set -h
page agent/instances.txt agent hire-root set /srv/x --help
page agent/instances.txt agent hire-root set --help /srv/x
page agent/instances.txt agent hire-root -h
page agent/instances.txt agent hire-root --json --help
echo "== agent hire-root: the plain set still sends =="
sends 0 '^POST /cli/agents/hire-root .*"root": "/srv/x"' agent hire-root set /srv/x

echo "== agent update: help anywhere is the agent page =="
page agent.txt agent update --help
page agent.txt agent update -h qa --field f --value v
page agent.txt agent update qa --field f --value v --help
page agent.txt agent update qa --field f --value -h
page agent.txt agent update --name qa --field --help --value v
echo "== agent update: the plain update still sends =="
sends 0 '^POST /cli/agent/update ' agent update qa --field f --value v

echo "== agent complete: help anywhere is the agent page =="
page agent.txt agent complete -h
page agent.txt agent complete --help
page agent.txt agent complete --agent qa --file f.md -h
page agent.txt agent complete --agent qa --file --help
page agent.txt agent complete --agent -h --file f.md
echo "== agent complete: the plain complete still sends =="
sends 0 '^POST /cli/agent/complete ' agent complete --agent qa --file f.md

echo "== agent profile: help anywhere is the agent page =="
# native: agent profile --help
page agent.txt agent profile --help
page agent.txt agent profile -h
page agent.txt agent profile /srv/ws --help
page agent.txt agent profile -h /srv/ws
echo "== agent profile: the plain read still asks =="
sends 0 '^GET /cli/workspace/agent-display-name ' agent profile

echo ""
echo "agent_help_never_acts: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
