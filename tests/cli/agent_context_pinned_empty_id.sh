#!/usr/bin/env bash
# `k2 agent context remove|off|move <token>` resolves the token to the right
# layer id when a pinned row has no id.
#
# Regression: the resolver's loop over pinned rows compared the row's id with
# the token and with the token's system alias. A pinned row with no id has id
# "", and the alias of any token outside the system names is "" too, so every
# token matched that row. The resolver then printed the typed token as the id
# and skipped the real id, path and label lookup: `remove docs/a.md` sent
# {"id":"docs/a.md"} instead of the layer's real id. Pinned rows with no id
# are now skipped.
#
# The stub's GET /cli/context/layers answer has a pinned row with no id, a
# pinned row with an id and a label, and an optional layer L1 at docs/a.md.
#   - remove, off and move on docs/a.md send {"id":"L1"};
#   - the label of a pinned row that has an id still resolves to that id;
#   - a token nothing matches exits 4 and sends no write.
#
# How: a python stub daemon on a free loopback port records every request.
# HOME is a temp dir, no real daemon, no network. Loud: every failure is
# named and the script exits 1.
# Run with: bash tests/cli/agent_context_pinned_empty_id.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-ctx-pinned-empty-id-XXXXXX)"
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
LAYERS = {
    "pinned": [
        {"label": "Role (persona)", "path": ".k2/agent/ROLE.md", "enabled": True},
        {"id": "pinned:tooling", "label": "Tooling footer", "enabled": True},
    ],
    "layers": [
        {"id": "L1", "path": "docs/a.md", "label": "Doc A", "enabled": True},
    ],
}
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
        if self.command == "GET" and path == "/cli/context/layers":
            return self._reply(LAYERS)
        if self.command == "POST" and path in (
            "/cli/context/remove", "/cli/context/set-enabled", "/cli/context/move"
        ):
            return self._reply({"ok": True})
        self.send_response(404)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"error":{"code":"not_found","hint":"stub: unexpected route"}}')
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

# run <args...>: clears the request log, then sets OUT, ERR and RC.
run() {
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# posted_field <route> <field>: the field of the one POST body sent to route.
# Prints nothing, and the caller fails, when there is not exactly one.
posted_field() {
    python3 - "$REQ_LOG" "$1" "$2" <<'PYEOF'
import json, sys
log, route, field = sys.argv[1], sys.argv[2], sys.argv[3]
bodies = []
with open(log) as f:
    for line in f:
        method, path, body = (line.rstrip("\n").split(" ", 2) + ["", ""])[:3]
        if method == "POST" and path == route:
            bodies.append(json.loads(body))
if len(bodies) == 1:
    v = bodies[0].get(field)
    print(json.dumps(v) if not isinstance(v, str) else v)
PYEOF
}

show_run() {
    {
        echo "      exit $RC"
        printf '%s\n' "$OUT" | sed 's/^/      out | /'
        printf '%s\n' "$ERR" | sed 's/^/      err | /'
        sed 's/^/      req | /' "$REQ_LOG"
    } >&2
}

expect_id() {
    local label="$1" route="$2" want="$3"
    if [ "$RC" -ne 0 ]; then
        bad "$label: exit $RC (want 0)"; show_run; return
    fi
    local got
    got="$(posted_field "$route" id)"
    if [ "$got" = "$want" ]; then
        ok "$label sends id $want"
    else
        bad "$label sent id '$got' to $route (want $want)"; show_run
    fi
}

echo "== remove, off and move resolve docs/a.md to L1 =="
run agent context remove docs/a.md
expect_id "remove docs/a.md" /cli/context/remove L1
[ "$OUT" = "removed layer L1" ] && ok "remove prints the real id" \
    || { bad "remove printed '$OUT' (want 'removed layer L1')"; show_run; }

run agent context off docs/a.md
expect_id "off docs/a.md" /cli/context/set-enabled L1
if [ "$RC" -eq 0 ]; then
    en="$(posted_field /cli/context/set-enabled enabled)"
    [ "$en" = "false" ] && ok "off sends enabled false" || { bad "off sent enabled '$en'"; show_run; }
fi

run agent context move docs/a.md top
expect_id "move docs/a.md top" /cli/context/move L1

echo "== a pinned row with an id still resolves by its label =="
run agent context off "Tooling footer"
expect_id "off 'Tooling footer'" /cli/context/set-enabled pinned:tooling

echo "== a token nothing matches is not found and writes nothing =="
run agent context remove docs/nope.md
if [ "$RC" -eq 4 ]; then
    ok "remove docs/nope.md: exit 4"
else
    bad "remove docs/nope.md: exit $RC (want 4)"; show_run
fi
case "$ERR" in
    *'"not_found"'*) ok "remove docs/nope.md names not_found" ;;
    *) bad "remove docs/nope.md: no not_found error"; show_run ;;
esac
if grep -q '^POST ' "$REQ_LOG"; then
    bad "remove docs/nope.md sent a write"; show_run
else
    ok "remove docs/nope.md sent no write"
fi

echo ""
echo "agent_context_pinned_empty_id: $pass passed, $fail failed"
[ "$fail" -eq 0 ] || exit 1
