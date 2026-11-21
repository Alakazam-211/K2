#!/usr/bin/env bash
# `k2 hooks install` and `k2 hooks uninstall` show K2's error and fail.
#
# Regression (0.46.17): cmd_hooks_install's reader printed "Error: ..." on
# stderr and exited 1 for an answer with an `error` key, but the shell
# threw that stderr away (2>/dev/null) and its `|| echo "$raw"` printed
# the raw JSON instead, with exit 0. A refused install looked like a
# success.
#
# Now an answer with an `error` key prints one line on stderr, `Error:
# <the error>` (an error object gives its hint, else its code), nothing
# on stdout, and exits 1, as the other hooks errors do (`k2 hooks emit`'s
# daemon errors, `k2 heartbeat log`'s). A refusal that carries a
# `permission` object exits 3 and ends with its Ask-with line, as every
# refused command does. A lone surrogate in the error prints as \udXXX.
# Kept as before: an answer that is not a JSON object, or not UTF-8,
# prints as it came (exit 0), a report the reader can't lay out prints
# the lines it made and then the raw answer (exit 0), and --json prints
# the answer as it came.
#
# How: a python stub daemon on a free loopback port answers each request
# with the status and body the row puts in two files, and records every
# request. HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/hooks_install_error.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-hooks-install-error-XXXXXX)"
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
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, self.path, body))
        with open(work + "/reply.status") as f:
            status = int(f.read().strip())
        with open(work + "/reply.body", "rb") as f:
            out = f.read()
        self.send_response(status)
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

# reply <status> <body>: what the stub answers next.
reply() {
    printf '%s\n' "$1" >"$WORK/reply.status"
    printf '%s' "$2" >"$WORK/reply.body"
}

# row <label> <want rc> <want stdout> <want stderr> <args...>: one run,
# which must send exactly one request.
row() {
    local label="$1" rc="$2" out="$3" err="$4"; shift 4
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
    label="k2 $* ($label)"
    [ "$RC" -eq "$rc" ] && ok "$label: exit $rc" || bad "$label: exit $RC (want $rc)"
    [ "$OUT" = "$out" ] && ok "$label: stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ "$ERR" = "$err" ] && ok "$label: stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ "$(wc -l <"$REQ_LOG" | tr -d ' ')" = 1 ] && grep -q '^POST /cli/hooks/install ' "$REQ_LOG" \
        && ok "$label: one POST /cli/hooks/install" \
        || { bad "$label: requests"; sed 's/^/      | /' "$REQ_LOG" >&2; }
}

PERM='{"error":{"code":"owner_only","hint":"Only the owner can do that."},"permission":{"command":"k2 permission request --scope x.write --why \"...\"","requestable":true}}'

for verb in install uninstall; do
    echo "== $verb: an error answer is an Error: line and a failing exit =="
    reply 200 '{"error":"serialize: boom"}'
    row "an error string, 200" 1 "" "Error: serialize: boom" hooks "$verb"
    reply 500 '{"error":"installer worker: task panicked"}'
    row "an error string, 500" 1 "" "Error: installer worker: task panicked" hooks "$verb"
    reply 403 '{"error":{"code":"admin_only"}}'
    row "an error object with a code" 1 "" "Error: admin_only" hooks "$verb"
    reply 200 '{"error":"bad \ud800 name"}'
    row "an error with a lone surrogate" 1 "" 'Error: bad \ud800 name' hooks "$verb"
    reply 403 "$PERM"
    row "a refusal with a permission object" 3 "" \
        "$(printf '%s\n%s' "Error: Only the owner can do that." 'Ask with: k2 permission request --scope x.write --why "..."')" \
        hooks "$verb"

    echo "== $verb: kept as before =="
    reply 200 '{"skipped":"agentHooks off"}'
    row "skipped" 0 "Hooks not changed: agentHooks off." "" hooks "$verb"
    reply 200 '{"report":{"action":"install","script":{"state":"written","path":"/h/notify.sh"},"claudeVersion":"2.1","claudeEventSet":"full","clis":[{"cli":"claude","state":"injected","path":"/h/c.json"}]}}'
    row "a report" 0 "$(printf '%s\n%s\n%s' 'Hook install: notify.sh written (/h/notify.sh)' 'Claude 2.1: full event set' '  claude   injected           /h/c.json')" "" hooks "$verb"
    reply 200 '{"report":{"clis":[{"cli":null}]}}'
    row "a report it can't lay out: its lines, then raw" 0 \
        "$(printf '%s\n%s' 'Hook ?: notify.sh ? (?)' '{"report":{"clis":[{"cli":null}]}}')" "" hooks "$verb"
    reply 200 '"an error here"'
    row "a string: raw" 0 '"an error here"' "" hooks "$verb"
    reply 200 '["error"]'
    row "a list: raw" 0 '["error"]' "" hooks "$verb"
    reply 200 'nope'
    row "not JSON: raw" 0 'nope' "" hooks "$verb"
    reply 200 $'{"error":"x"}\xff'
    row "not UTF-8: raw" 0 $'{"error":"x"}\xff' "" hooks "$verb"
    reply 200 '{"error":"serialize: boom"}'
    row "--json prints the answer" 0 '{"error":"serialize: boom"}' "" hooks "$verb" --json
done

echo ""
echo "hooks_install_error: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
