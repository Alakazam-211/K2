#!/usr/bin/env bash
# A secret another agent sent waits on K2's permission card (0.46.10,
# `vault.incoming.accept`), never on "go run a command" or "go find it in
# Settings":
#   * `k2 vault send --help` says K2 puts the card on the receiving agent's
#     Thread and gives the exact line to ask with;
#   * inside a K2 terminal, `k2 vault incoming accept|decline <id>` exits 3
#     at once (no daemon call) and points at the card, with that line;
#   * `k2 vault incoming` (against a stub daemon) says where each waiting
#     delivery's card is, or the line to ask with when there is none;
#   * `k2 study vault` names the card and the line.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d -t k2-vault-incoming-XXXXXX)"
STUB_PID=""
trap '[ -n "$STUB_PID" ] && { kill "$STUB_PID" 2>/dev/null; wait "$STUB_PID" 2>/dev/null || true; }; rm -rf "$WORK"' EXIT

ID="hd_0123456789abcdef0123456789abcdef"
ASK="k2 permission request vault.incoming.accept id=$ID --why \"<what you need it for>\""

# ── help, no daemon ─────────────────────────────────────────────────────
help="$(env HOME="$WORK" "$K2_CLI" vault send --help)" || fail "vault send --help"
for want in "K2 puts a permission card on your Thread" "Allow (accept)" "Deny (decline)" \
    "k2 permission request vault.incoming.accept id=<delivery-id> [as=NEW|replace=yes]" \
    "the card K2 puts on the receiving agent's Thread"; do
    printf '%s' "$help" | grep -Fq -- "$want" || fail "vault send --help lacks '$want': $help"
done
ok "k2 vault send --help: the card, Allow/Deny, and the line to ask with"

# ── an agent never accepts or declines; it is pointed at the card ──────
for verb in accept decline; do
    set +e
    err="$(env HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=agent-token "$K2_CLI" vault incoming "$verb" "$ID" 2>&1 >/dev/null)"; rc=$?
    set -e
    [ "$rc" = 3 ] || fail "incoming $verb from a K2 terminal: exit $rc ($err)"
    printf '%s' "$err" | grep -Fq "a person accepts or declines $ID on the card K2 put on your Thread" || fail "incoming $verb: $err"
    printf '%s' "$err" | grep -Fq -- "$ASK" || fail "incoming $verb lacks the ask line: $err"
    if printf '%s' "$err" | grep -Fq "Settings"; then fail "incoming $verb sends the agent to Settings: $err"; fi
done
ok "k2 vault incoming accept|decline in a K2 terminal: exit 3, the card and the ask line"

# ── k2 study vault ─────────────────────────────────────────────────────
study="$(env HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=offline "$K2_CLI" study vault)" || fail "study vault"
printf '%s' "$study" | grep -Fq "K2 puts a card on that agent's Thread for a person" || fail "study vault lacks the card: $study"
printf '%s' "$study" | grep -Fq "k2 permission request vault.incoming.accept" || fail "study vault lacks the ask line"
ok "k2 study vault: the card and the ask line"

# ── the list, against a stub daemon ───────────────────────────────────
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
cat >"$WORK/stub.py" <<'PY'
import http.server, json, sys
ID = sys.argv[2]
ITEMS = [
    {"deliveryId": ID, "status": "pending", "name": "STRIPE_KEY", "peer": "sales", "workspace": "ops", "createdAt": "2026-11-07T10:00:00Z",
     "card": {"request": ID, "addr": "ops", "card": "c-1", "expiresAt": 1}, "ask": "k2 permission request vault.incoming.accept id=" + ID + " --why \"<what you need it for>\"",
     "landsAs": {"delivery": "file", "ceiling": "env", "expiresAt": None}},
    {"deliveryId": "hd_nocard", "status": "pending", "name": "OTHER", "peer": "sales", "workspace": "ops", "createdAt": "2026-11-07T10:00:00Z",
     "card": None, "ask": "k2 permission request vault.incoming.accept id=hd_nocard --why \"<what you need it for>\"",
     "landsAs": {"delivery": "env", "ceiling": "env", "expiresAt": None}},
]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        out = {"items": ITEMS} if self.path.startswith("/cli/vault/incoming") else {}
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
python3 "$WORK/stub.py" "$PORT" "$ID" &
STUB_PID=$!
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break; sleep 0.05; done
out="$(env HOME="$WORK" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=stub-token K2_PROJECT_PATH="$WORK" "$K2_CLI" vault incoming)" || fail "incoming: $out"
printf '%s' "$out" | grep -F "STRIPE_KEY" | grep -Fq "(waiting for a person: the card on the Thread of ops)" || fail "incoming: the card line: $out"
printf '%s' "$out" | grep -F "OTHER" | grep -Fq "(no card yet: k2 permission request vault.incoming.accept id=hd_nocard" || fail "incoming: the ask line: $out"
if printf '%s' "$out" | grep -Fq "from their own terminal"; then fail "incoming still says 'from their own terminal': $out"; fi
ok "k2 vault incoming: where each card is, or the line to ask with"

echo "vault_incoming_card: $pass passed"
