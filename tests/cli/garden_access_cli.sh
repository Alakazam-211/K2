#!/usr/bin/env bash
# Garden access lists (prd-garden-access-v1 G19, G20, G24; slice A6): the
# CLI's switch-on copy, against a stub daemon.
#
#   * `k2 study` lists `garden-access` and prints its page only while the
#     daemon's gardenAccess switch is on (its routes answer); off (the
#     route is "unknown zen route") the topic doesn't exist, exactly as
#     0.46.3 (the byte-for-byte half is tests/cli/permission_off_golden.sh).
#   * `k2 zen widget claim` on a daemon that answers 410 `retired` prints
#     "Gardens use access lists now: `k2 zen garden access`" and exits 0.
#   * `k2 zen garden delete` refused with `garden_delete_person` and a card
#     prints "Asked the person to delete <name>" and exits 3.
#
# Hermetic: HOME is a throwaway dir, K2_* vars unset, the daemon is a tiny
# python stub on loopback whose answers per path come from a JSON file.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d /tmp/k2gac-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

ROUTES = sys.argv[1]

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        path = self.path.split("?", 1)[0]
        routes = json.load(open(ROUTES))
        status, body = routes.get(path, [404, {"error": "unknown zen route", "path": path}])
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(raw)
    do_GET = handle_any
    do_POST = handle_any

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

ROUTES="$SANDBOX/routes.json"
echo '{}' > "$ROUTES"
python3 -I "$SANDBOX/stub.py" "$ROUTES" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { pass=$((pass + 1)); echo "  ok: $1"; }
bad() { fail=$((fail + 1)); echo "  FAIL: $1" >&2; }

# run <expected rc> <args…>: K2 against the stub; stdout+stderr in $OUT.
run() {
    local want="$1" rc
    shift
    set +e
    OUT="$(K2_PORT="$PORT" K2_HOOK_TOKEN=garden-access-cli "$K2" "$@" </dev/null 2>&1)"
    rc=$?
    set -e
    if [ "$rc" != "$want" ]; then
        bad "k2 $* exited $rc, want $want: $OUT"
        return 1
    fi
    return 0
}

routes() { printf '%s\n' "$1" > "$ROUTES"; }

ACCESS_ON='"/cli/zen/garden/access": [400, {"ok": false, "error": "bad_request", "message": "garden/access needs garden=<id or name>"}]'

echo "== switch off: no garden-access topic =="
routes '{}'
if run 0 study; then
    if printf '%s' "$OUT" | grep -q 'garden-access'; then bad "study lists garden-access with the switch off"; else ok "study (off) has no garden-access"; fi
fi
if run 0 study --json; then
    if printf '%s' "$OUT" | grep -q 'garden-access'; then bad "study --json lists garden-access off"; else ok "study --json (off)"; fi
fi
if run 2 study garden-access; then
    if printf '%s' "$OUT" | grep -q "unknown study topic 'garden-access'"; then ok "study garden-access (off) is an unknown topic"; else bad "off: $OUT"; fi
fi

echo "== switch on: the topic and its page =="
routes "{$ACCESS_ON}"
if run 0 study; then
    if printf '%s' "$OUT" | grep -q '^  garden-access       Who may edit a Garden'; then ok "study lists garden-access (on)"; else bad "study (on): $OUT"; fi
    if printf '%s' "$OUT" | grep -A1 '^  garden-data ' | grep -q '^  garden-access '; then ok "right after garden-data"; else bad "order: $OUT"; fi
fi
if run 0 study --json; then
    if printf '%s' "$OUT" | python3 -c 'import json,sys; t=[x["id"] for x in json.load(sys.stdin)["topics"]]; sys.exit(0 if t[-1]=="garden-access" else 1)'; then
        ok "study --json (on) ends with garden-access"
    else
        bad "study --json (on): $OUT"
    fi
fi
if run 0 study garden-access; then
    for needle in "Edit a Garden" "k2 zen garden access <g> ask" "garden_access_needed" "garden_delete_person" "needs_editor" "Keep as mine"; do
        if printf '%s' "$OUT" | grep -qF "$needle"; then ok "page says $needle"; else bad "page lacks $needle"; fi
    done
fi
if run 0 study garden-access --json; then
    if printf '%s' "$OUT" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if d["id"]=="garden-access" and d["body"].startswith("k2 study garden-access") else 1)'; then
        ok "study garden-access --json"
    else
        bad "json page: $OUT"
    fi
fi

echo "== claim retired (G20) =="
routes "{$ACCESS_ON, \"/cli/zen/widget/claim\": [410, {\"ok\": false, \"error\": \"retired\", \"message\": \"Keep as mine (It's mine) is retired: Gardens use access lists now.\"}]}"
if run 0 zen widget claim tally; then
    if [ "$OUT" = 'Gardens use access lists now: `k2 zen garden access`' ]; then ok "claim prints the G20 line"; else bad "claim: $OUT"; fi
fi

echo "== sign with the switch on (G9) =="
routes "{$ACCESS_ON, \"/cli/zen/widget/sign\": [200, {\"ok\": true, \"widget\": \"tally\", \"hash\": \"abcdef012345\", \"madeBy\": {\"kind\": \"agent\", \"handle\": \"ana\"}, \"stamped\": true, \"vouched\": true}]}"
if run 0 zen widget sign tally; then
    if [ "$OUT" = "Signed as ana: this version of tally runs with the Garden's data in each Garden whose list has ana." ]; then ok "sign (on) words"; else bad "sign (on): $OUT"; fi
fi
routes "{$ACCESS_ON, \"/cli/zen/widget/sign\": [200, {\"ok\": true, \"widget\": \"tally\", \"hash\": \"abcdef012345\", \"madeBy\": {\"kind\": \"agent\", \"handle\": \"ana\"}, \"stamped\": false}]}"
if run 0 zen widget sign tally; then
    case "$OUT" in "Already signed by ana (the first signer"*) ok "sign (off answer) keeps today's words" ;; *) bad "sign (off answer): $OUT" ;; esac
fi

echo "== an agent's delete asks the person (G19) =="
routes "{$ACCESS_ON, \"/cli/zen/garden/delete\": [403, {\"ok\": false, \"error\": \"garden_delete_person\", \"garden\": \"g-1a2b3c4d\", \"name\": \"Sales board\", \"card\": {\"card\": {\"id\": \"c1\"}, \"request\": \"r1\"}, \"message\": \"Only the person deletes a Garden. K2 asked them to delete Sales board (a card on their Thread with you).\"}]}"
if run 3 zen garden delete "Sales board"; then
    if printf '%s' "$OUT" | grep -q '^Asked the person to delete Sales board$'; then ok "delete says it asked"; else bad "delete: $OUT"; fi
fi
routes "{$ACCESS_ON, \"/cli/zen/garden/delete\": [403, {\"ok\": false, \"error\": \"garden_delete_person\", \"garden\": \"g-1a2b3c4d\", \"name\": \"Sales board\", \"card\": {\"card\": null, \"asked\": true}, \"message\": \"Only the person deletes a Garden. Ask them to delete Sales board (Settings → Gardens), or wait for their answer to your last ask.\"}]}"
if run 3 zen garden delete "Sales board"; then
    if printf '%s' "$OUT" | grep -q 'Asked the person'; then bad "no card out, yet it says it asked: $OUT"; else ok "no card: the daemon's words only"; fi
fi

echo "garden_access_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
