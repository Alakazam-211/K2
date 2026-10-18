#!/usr/bin/env bash
# `k2 sources secret <name>… NAME|--none` (0.46.1): what the REAL cli/k2
# sends, against a stub daemon that records each request. Inside a K2
# terminal (a passport) no confirm is asked for: the daemon decides
# (agent with the switch, else person_only).
#   1. several names + NAME → POST /cli/sources/secret {names, secret}.
#   2. several names + --none → {names, secret: null}.
#   3. one name keeps the old body {name, secret}.
#   4. usage: no secret, no names → exit 2, no request.
#   5. the daemon's person_only refusal exits 3.
#
# No daemon build; never touches ~/.k2 (env carries port + token).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORK="$(mktemp -d -t k2-sources-secret-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

work = sys.argv[1]
n = {"i": 0}

class H(BaseHTTPRequestHandler):
    def reply(self, obj, code=200):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        path = urlparse(self.path).path
        n["i"] += 1
        with open(os.path.join(work, "req-%03d.json" % n["i"]), "w") as f:
            json.dump({"path": path, "body": body}, f)
        if os.path.exists(os.path.join(work, "refuse")):
            return self.reply({"error": {"code": "person_only", "hint": "This needs a person"}}, 403)
        if path == "/cli/sources/secret":
            if "names" in body:
                return self.reply({"names": body["names"], "secret": body["secret"], "secretBound": body["secret"] is not None})
            return self.reply({"name": body["name"], "secret": body["secret"], "secretBound": body["secret"] is not None})
        self.reply({"error": {"code": "stub", "hint": path}}, 404)

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID"
for _ in $(seq 1 50); do [ -f "$WORK/stub.port" ] && break; sleep 0.1; done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/stub.port")"

PASS=0
FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
last_req() { ls -1 "$WORK"/req-*.json 2>/dev/null | sort | tail -1; }
req_count() { ls -1 "$WORK"/req-*.json 2>/dev/null | wc -l | tr -d ' '; }
body() { python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["body"], sort_keys=True))' "$(last_req)"; }

out="$("$K2_CLI" sources secret k2-ops-boxes k2-ops-billing k2-ops-edge K2_OPS_READ_TOKEN 2>&1)" || bad "1: exit non-zero: $out"
[ "$(body)" = '{"names": ["k2-ops-boxes", "k2-ops-billing", "k2-ops-edge"], "secret": "K2_OPS_READ_TOKEN"}' ] && ok "1: several names, one request" || bad "1: body $(body)"
grep -qF 'k2-ops-boxes, k2-ops-billing, k2-ops-edge: use K2_OPS_READ_TOKEN' <<<"$out" && ok "1: prints the list" || bad "1: out $out"

"$K2_CLI" sources secret a-src b-src --none >/dev/null 2>&1 || bad "2: exit non-zero"
[ "$(body)" = '{"names": ["a-src", "b-src"], "secret": null}' ] && ok "2: --none unbinds the list" || bad "2: body $(body)"

"$K2_CLI" sources secret a-src WS_KEY >/dev/null 2>&1 || bad "3: exit non-zero"
[ "$(body)" = '{"name": "a-src", "secret": "WS_KEY"}' ] && ok "3: one name keeps the old body" || bad "3: body $(body)"

before="$(req_count)"
for args in "a-src" "" "--none"; do
    set +e
    # shellcheck disable=SC2086 # split on purpose
    "$K2_CLI" sources secret $args >/dev/null 2>&1
    rc=$?
    set -e
    [ "$rc" = "2" ] && ok "4: '$args' exits 2" || bad "4: '$args' exit $rc"
done
[ "$(req_count)" = "$before" ] && ok "4: no request on a usage error" || bad "4: a request was sent"

touch "$WORK/refuse"
set +e
"$K2_CLI" sources secret a-src b-src K >/dev/null 2>&1
rc=$?
set -e
[ "$rc" = "3" ] && ok "5: person_only exits 3" || bad "5: exit $rc"

export K2_PORT=1
out="$("$K2_CLI" sources --help)"
grep -qF 'k2 sources secret <name>… NAME|--none' <<<"$out" && ok "help names several sources" || bad "help: $out"

echo
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
