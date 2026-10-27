#!/usr/bin/env bash
# Garden access lists (prd-garden-access-v1 G19, G20, G24; slice A6): the
# CLI's copy, against a stub daemon. Garden access is the flow since 0.46.3
# (no switch); the CLI only checks that the server's K2 has its routes.
#
#   * `k2 study` always lists `garden-access` and prints its page (static
#     docs), whatever the daemon answers.
#   * Against a K2 older than 0.46.3 (no access routes) `garden new --from`
#     says that K2 is older and needs an update (exit 2).
#   * Retired verbs (k2 study garden-changes): `k2 zen widget claim` on a
#     daemon that answers 410 `retired` signs the version instead (exit 0)
#     with one notice line on stderr; with --json, stdout is only the sign
#     answer plus a "retired" object. `k2 zen proposals approve` answered
#     `retired` does nothing: the notice on stderr (text) or only
#     {"error":"retired",…} on stdout (--json), exit 1. A draining
#     `k2 zen proposals` list prints the notice and the list.
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

# run2 <expected rc> <args…>: like run, stdout in $SOUT and stderr in $SERR.
run2() {
    local want="$1" rc
    shift
    set +e
    SOUT="$(K2_PORT="$PORT" K2_HOOK_TOKEN=garden-access-cli "$K2" "$@" </dev/null 2>"$SANDBOX/stderr")"
    rc=$?
    set -e
    SERR="$(cat "$SANDBOX/stderr")"
    if [ "$rc" != "$want" ]; then
        bad "k2 $* exited $rc, want $want: $SOUT / $SERR"
        return 1
    fi
    return 0
}

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

# A daemon with Garden access writes ~/.k2/garden-access-on at boot, and
# the CLI asks nothing without it (an older K2); the stub's "on" carries
# the mark too.
routes() {
    printf '%s\n' "$1" > "$ROUTES"
    case "$1" in
        *'"/cli/zen/garden/access": [400'*) mkdir -p "$HOME/.k2" && : > "$HOME/.k2/garden-access-on" ;;
        *) rm -f "$HOME/.k2/garden-access-on" ;;
    esac
}

ACCESS_ON='"/cli/zen/garden/access": [400, {"ok": false, "error": "bad_request", "message": "garden/access needs garden=<id or name>"}]'

echo "== an older K2 (no access routes): the topic is still listed =="
routes '{}'
if run 0 study; then
    if printf '%s' "$OUT" | grep -q '^  garden-access       Who may edit a Garden'; then ok "study lists garden-access (older K2)"; else bad "study (older K2): $OUT"; fi
fi
if run 0 study garden-access; then
    if printf '%s' "$OUT" | grep -q '^k2 study garden-access'; then ok "study garden-access (older K2) prints the page"; else bad "older K2 page: $OUT"; fi
fi
printf 'schema = 1\n' > "$SANDBOX/page.toml"
if run 2 zen garden new Plans --from "$SANDBOX/page.toml"; then
    if [ "$OUT" = "Error: --from and --widgets need K2 0.46.3 or newer on this server; this server's K2 is older. Update K2 on that server." ]; then ok "garden new --from on an older K2"; else bad "garden new --from (older K2): $OUT"; fi
fi

echo "== Garden access: the topic and its page =="
routes "{$ACCESS_ON}"
if run 0 study; then
    if printf '%s' "$OUT" | grep -q '^  garden-access       Who may edit a Garden'; then ok "study lists garden-access"; else bad "study: $OUT"; fi
    if printf '%s' "$OUT" | grep -A1 '^  garden-data ' | grep -q '^  garden-access '; then ok "right after garden-data"; else bad "order: $OUT"; fi
fi
if run 0 study --json; then
    if printf '%s' "$OUT" | python3 -c 'import json,sys; t=[x["id"] for x in json.load(sys.stdin)["topics"]]; sys.exit(0 if t[t.index("garden-data") + 1]=="garden-access" else 1)'; then
        ok "study --json lists garden-access after garden-data"
    else
        bad "study --json: $OUT"
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

echo "== retired: claim signs instead, with a notice (G20, k2 study garden-changes) =="
RETIRED_CLAIM='"/cli/zen/widget/claim": [410, {"ok": false, "error": "retired", "message": "Keep as mine (It'"'"'s mine) is retired: Gardens use access lists now."}]'
SIGNED='"/cli/zen/widget/sign": [200, {"ok": true, "widget": "tally", "hash": "abcdef012345", "madeBy": {"kind": "owner"}, "stamped": true, "vouched": true}]'
routes "{$ACCESS_ON, $RETIRED_CLAIM, $SIGNED}"
CLAIM_NOTICE='Notice: `k2 zen widget claim` is retired (Gardens use access lists since 0.46.3) and is removed in 0.47.0. Use: k2 zen widget sign tally. See `k2 study garden-changes`.'
if run2 0 zen widget claim tally; then
    if [ "$SERR" = "$CLAIM_NOTICE" ]; then ok "claim: one notice line on stderr"; else bad "claim stderr: $SERR"; fi
    if [ "$SOUT" = "Signed as you: this version of tally runs with the Garden's data in every Garden." ]; then ok "claim: signed instead"; else bad "claim stdout: $SOUT"; fi
fi
if run2 0 zen widget claim tally --json; then
    if [ -z "$SERR" ]; then ok "claim --json: nothing on stderr"; else bad "claim --json stderr: $SERR"; fi
    if printf '%s' "$SOUT" | python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["stamped"] is True and d["widget"] == "tally", d
assert d["retired"] == {"verb": "k2 zen widget claim", "replacement": "k2 zen widget sign tally", "removedIn": "0.47.0", "see": "k2 study garden-changes"}, d
'; then ok "claim --json: the sign answer with a retired object, nothing else"; else bad "claim --json stdout: $SOUT"; fi
fi

echo "== retired: an agent's proposals approve does nothing (exit 1) =="
RETIRED_APPROVE='"/cli/zen/proposals/approve": [410, {"ok": false, "error": "retired", "message": "Approving Garden proposals from an agent is retired: Gardens use access lists now."}]'
routes "{$ACCESS_ON, $RETIRED_APPROVE}"
APPROVE_NOTICE="Notice: \`k2 zen proposals approve\` is retired (Gardens use access lists since 0.46.3) and is removed in 0.47.0. Use: k2 zen garden access <garden> (edit when you're on its list, else ask). See \`k2 study garden-changes\`."
if run2 1 zen proposals approve zp-1; then
    if [ "$SERR" = "$APPROVE_NOTICE" ]; then ok "approve: the notice on stderr"; else bad "approve stderr: $SERR"; fi
    if [ -z "$SOUT" ]; then ok "approve: nothing on stdout"; else bad "approve stdout: $SOUT"; fi
fi
if run2 1 zen proposals approve zp-1 --json; then
    if [ -z "$SERR" ]; then ok "approve --json: nothing on stderr"; else bad "approve --json stderr: $SERR"; fi
    if [ "$SOUT" = '{"error":"retired","verb":"k2 zen proposals approve","replacement":"k2 zen garden access <garden> (edit when you'"'"'re on its list, else ask)","removedIn":"0.47.0","see":"k2 study garden-changes"}' ]; then
        ok "approve --json: only the retired object"
    else
        bad "approve --json stdout: $SOUT"
    fi
fi

echo "== retired: proposals still drain, with the notice =="
routes "{$ACCESS_ON, \"/cli/zen/proposals\": [200, {\"ok\": true, \"proposals\": []}]}"
if run2 0 zen proposals; then
    case "$SERR" in 'Notice: `k2 zen proposals` is retired'*'removed in 0.47.0'*'k2 study garden-changes'*) ok "list: the notice" ;; *) bad "list stderr: $SERR" ;; esac
    if [ "$SOUT" = "No Garden proposals waiting." ]; then ok "list: still lists"; else bad "list stdout: $SOUT"; fi
fi
if run2 0 zen proposals --json; then
    if [ -z "$SERR" ] && printf '%s' "$SOUT" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["proposals"] == [] and d["retired"]["removedIn"] == "0.47.0", d'; then
        ok "list --json: the list with a retired object"
    else
        bad "list --json: $SOUT / $SERR"
    fi
fi
echo "== an older K2 (no access lists): proposals as before, no notice =="
routes '{"/cli/zen/proposals": [200, {"ok": true, "proposals": []}]}'
if run2 0 zen proposals; then
    if [ -z "$SERR" ] && [ "$SOUT" = "No Garden proposals waiting." ]; then ok "older K2: no notice"; else bad "older K2: $SOUT / $SERR"; fi
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

echo "== 0.46.5: a sign that can't cover a placed version asks the person (zen.sign) =="
routes "{$ACCESS_ON, \"/cli/zen/widget/sign\": [403, {\"ok\": false, \"error\": \"sign_needs_person\", \"widget\": \"tally\", \"version\": \"$(printf 'ab%.0s' $(seq 1 32))\", \"changes\": \"Since the last signed version, this widget now asks for thread:post.\", \"card\": \"c9\", \"message\": \"tally's version abababababab runs without the person's data in Sales board.\"}]}"
if run2 3 zen widget sign tally; then
    case "$SERR" in
        *"Error: tally's version abababababab runs without the person's data in Sales board."*"The card shows: Since the last signed version, this widget now asks for thread:post."*"Card: c9"*) ok "sign_needs_person: the message, the change and the card" ;;
        *) bad "sign_needs_person: $SOUT / $SERR" ;;
    esac
fi
routes "{$ACCESS_ON, \"/cli/zen/widget/sign\": [200, {\"ok\": true, \"widget\": \"tally\", \"version\": \"ab\", \"asked\": true, \"card\": \"c9\", \"changes\": \"Since it was signed, this widget now loads the library uplot@1.\"}]}"
if run2 0 zen widget sign tally --ask; then
    if [ "$SOUT" = "Asked the person to sign tally: K2 put a card on your Thread.
The card shows: Since it was signed, this widget now loads the library uplot@1." ]; then ok "--ask says it asked, with the change"; else bad "--ask: $SOUT / $SERR"; fi
fi
if run2 2 zen widget claim tally --ask; then
    case "$SERR" in *"--ask is for k2 zen widget sign"*) ok "--ask is sign's only" ;; *) bad "claim --ask: $SERR" ;; esac
fi
routes "{$ACCESS_ON, \"/cli/zen/widget/sign\": [200, {\"ok\": true, \"widget\": \"tally\", \"hash\": \"abcdef012345\", \"madeBy\": {\"kind\": \"owner\"}, \"stamped\": false, \"vouched\": false, \"review\": {\"changes\": \"Since it was signed, this widget now asks for presence:read.\", \"ask\": \"k2 zen widget sign tally --ask\"}}]}"
if run2 0 zen widget sign tally; then
    case "$SOUT" in *"Note: tally changed since it was signed: it now asks for presence:read. Ask the person to look: k2 zen widget sign tally --ask"*) ok "a carried difference offers the card" ;; *) bad "review note: $SOUT" ;; esac
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
