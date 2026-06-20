#!/bin/bash
# Self-contained test for `k2 connect subdomain` (PRD k2-connect-e2e-encryption.md §7).
#
# Stands up a throwaway mock control plane on loopback, points the CLI at it
# via $K2_CONNECT_BASE, and exercises every subcommand:
#   * the EXACT HTTP method + path + body the CLI builds (the mock records and
#     asserts on the raw request);
#   * the documented error-code → message mapping (403 pro_required, 409
#     label_taken, 404 not_found, primary_undeletable, invalid_token, ...).
#
# Fail-loud: no try/catch swallowing, no skip-if-missing. Any mismatch is a
# hard FAIL with a non-zero exit. Needs python3 + curl (both already CLI deps).

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
K2_BIN="${HERE}/k2"

PASS=0
FAIL=0
ok()  { PASS=$((PASS+1)); echo "  ok   - $1"; }
bad() { FAIL=$((FAIL+1)); echo "  FAIL - $1"; }

# A fresh HOME with a tunnel.json holding a known token, so _connect_bearer_token
# finds it. Use .k2 (the post-0.40 home) so K2_HOME resolves there.
TEST_HOME="$(mktemp -d)"
mkdir -p "${TEST_HOME}/.k2"
cat > "${TEST_HOME}/.k2/tunnel.json" <<'JSON'
{ "token": "tok_test_123", "subdomain": "rosson" }
JSON
trap 'rm -rf "$TEST_HOME" "$REQ_FILE" 2>/dev/null; [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null' EXIT

REQ_FILE="$(mktemp)"

# Mock control plane: a tiny python HTTP server that records the request line +
# headers + body to $REQ_FILE and replies with a status/body chosen by a header
# the test sets via the URL path is awkward, so instead it keys off a file the
# test writes ($RESP_FILE: "<status>\n<json body>").
RESP_FILE="$(mktemp)"
echo $'200\n{}' > "$RESP_FILE"

PORT=0
start_mock() {
    REQ_FILE="$REQ_FILE" RESP_FILE="$RESP_FILE" python3 - "$REQ_FILE" "$RESP_FILE" <<'PY' &
import sys, http.server, socketserver, threading
req_file, resp_file = sys.argv[1], sys.argv[2]

class H(http.server.BaseHTTPRequestHandler):
    def _record_and_reply(self):
        length = int(self.headers.get("Content-Length", "0") or "0")
        body = self.rfile.read(length).decode("utf-8", "replace") if length else ""
        with open(req_file, "w") as f:
            f.write("%s %s\n" % (self.command, self.path))
            f.write("AUTH:%s\n" % self.headers.get("Authorization", ""))
            f.write(body)
        with open(resp_file) as f:
            raw = f.read()
        nl = raw.find("\n")
        code = int(raw[:nl].strip())
        rbody = raw[nl+1:]
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(rbody.encode())))
        self.end_headers()
        self.wfile.write(rbody.encode())
    def do_GET(self): self._record_and_reply()
    def do_POST(self): self._record_and_reply()
    def do_PUT(self): self._record_and_reply()
    def do_DELETE(self): self._record_and_reply()
    def log_message(self, *a): pass

srv = socketserver.TCPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
PY
    MOCK_PID=$!
}

# Start the mock and read the chosen port from its first stdout line.
exec 3< <(start_mock)
read -r PORT <&3
if [ -z "$PORT" ] || [ "$PORT" = "0" ]; then
    echo "could not start mock control plane" >&2; exit 1
fi
export K2_CONNECT_BASE="http://127.0.0.1:${PORT}"

run() { HOME="$TEST_HOME" "$K2_BIN" connect subdomain "$@" 2>&1; }
set_resp() { printf '%s\n%s' "$1" "$2" > "$RESP_FILE"; }
req_line() { head -1 "$REQ_FILE"; }
req_body() { tail -n +3 "$REQ_FILE"; }
req_auth() { sed -n '2p' "$REQ_FILE"; }

# ── create ───────────────────────────────────────────────────────────────
set_resp 200 '{"label":"staging","host":"staging.rosson.k2.dev","target":"localhost:3000","status":"active","cert_status":"covered","primary":false,"effective":"live"}'
out="$(run create staging --target localhost:3000)"; rc=$?
[ "$(req_line)" = "POST /subdomains" ] && ok "create -> POST /subdomains" || bad "create method/path: $(req_line)"
echo "$(req_body)" | grep -q '"label": "staging"' && ok "create body has label" || bad "create body label: $(req_body)"
echo "$(req_body)" | grep -q '"target": "localhost:3000"' && ok "create body has target" || bad "create body target: $(req_body)"
[ "$(req_auth)" = "AUTH:Bearer tok_test_123" ] && ok "create sends bearer token" || bad "create auth: $(req_auth)"
echo "$out" | grep -q "staging.rosson.k2.dev" && [ $rc -eq 0 ] && ok "create prints host (rc=0)" || bad "create output: $out (rc=$rc)"

# create — 403 pro_required -> FRIENDLY upsell (not a raw 403), points at
# `k2 connect upgrade`. Server may name the gated subdomain in `detail`.
set_resp 403 '{"error":"pro_required","detail":"staging.rosson.k2.dev"}'
out="$(run create staging --target localhost:3000)"; rc=$?
echo "$out" | grep -qi "needs the Pro plan" && [ $rc -ne 0 ] && ok "403 pro_required -> friendly upsell (rc!=0)" || bad "pro_required upsell: $out (rc=$rc)"
echo "$out" | grep -q "k2 connect upgrade" && ok "pro_required upsell names 'k2 connect upgrade'" || bad "pro_required missing upgrade hint: $out"
echo "$out" | grep -q "staging.rosson.k2.dev" && ok "pro_required upsell names the subdomain (from detail)" || bad "pro_required missing subdomain: $out"

# create — 409 label_taken
set_resp 409 '{"error":"label_taken"}'
out="$(run create staging --target localhost:3000)"; rc=$?
echo "$out" | grep -qi "already taken" && [ $rc -ne 0 ] && ok "409 label_taken mapped" || bad "label_taken map: $out"

# create — 400 bad_label
set_resp 400 '{"error":"bad_label"}'
out="$(run create 'BAD LABEL' --target localhost:3000)"; rc=$?
echo "$out" | grep -qi "Invalid label" && [ $rc -ne 0 ] && ok "400 bad_label mapped" || bad "bad_label map: $out"

# ── list ─────────────────────────────────────────────────────────────────
# Rows carry the per-row `tier` field (free|single|pro) the billing agent adds
# to GET /subdomains; list renders it as a PLAN column.
set_resp 200 '{"subdomains":[{"label":"rosson","host":"rosson.k2.dev","target":"daemon","status":"active","primary":true,"effective":"live","tier":"pro"},{"label":"staging","host":"staging.rosson.k2.dev","target":"localhost:3000","status":"active","primary":false,"effective":"live","tier":"single"}]}'
out="$(run list)"; rc=$?
[ "$(req_line)" = "GET /subdomains" ] && ok "list -> GET /subdomains" || bad "list method/path: $(req_line)"
echo "$out" | grep -q "rosson.k2.dev" && echo "$out" | grep -q "staging.rosson.k2.dev" && ok "list renders both hosts" || bad "list output: $out"
echo "$out" | grep -q "primary" && ok "list flags primary" || bad "list primary flag missing: $out"
echo "$out" | grep -q "PLAN" && ok "list has a PLAN column header" || bad "list missing PLAN header: $out"
echo "$out" | grep -q "Pro" && echo "$out" | grep -q "Single" && ok "list shows per-row tier (Pro/Single)" || bad "list tiers: $out"
# tunnel.json subdomain is "rosson" -> that row flagged connected
echo "$out" | grep "rosson.k2.dev" | grep -q "connected" && ok "list flags the connected row (tunnel.json subdomain)" || bad "list connected flag: $out"

# list — empty
set_resp 200 '{"subdomains":[]}'
out="$(run list)"; rc=$?
echo "$out" | grep -qi "No subdomains" && ok "list empty -> 'No subdomains'" || bad "list empty: $out"

# ── point ────────────────────────────────────────────────────────────────
set_resp 200 '{"label":"staging","host":"staging.rosson.k2.dev","target":"localhost:8080","status":"active","effective":"live"}'
out="$(run point staging --target localhost:8080)"; rc=$?
[ "$(req_line)" = "PUT /subdomains/staging" ] && ok "point -> PUT /subdomains/{label}" || bad "point method/path: $(req_line)"
echo "$(req_body)" | grep -q '"target": "localhost:8080"' && ok "point body has target" || bad "point body: $(req_body)"
echo "$out" | grep -q "localhost:8080" && [ $rc -eq 0 ] && ok "point prints new target" || bad "point output: $out"

# point — 404 not_found
set_resp 404 '{"error":"not_found"}'
out="$(run point ghost --target localhost:1)"; rc=$?
echo "$out" | grep -qi "No such subdomain" && [ $rc -ne 0 ] && ok "404 not_found mapped" || bad "not_found map: $out"

# ── rm ───────────────────────────────────────────────────────────────────
set_resp 200 '{"removed":"staging"}'
out="$(run rm staging)"; rc=$?
[ "$(req_line)" = "DELETE /subdomains/staging" ] && ok "rm -> DELETE /subdomains/{label}" || bad "rm method/path: $(req_line)"
echo "$out" | grep -q "Removed staging" && [ $rc -eq 0 ] && ok "rm prints removed" || bad "rm output: $out"

# rm — 403 primary_undeletable
set_resp 403 '{"error":"primary_undeletable"}'
out="$(run rm rosson)"; rc=$?
echo "$out" | grep -qi "primary subdomain cannot be removed" && [ $rc -ne 0 ] && ok "403 primary_undeletable mapped" || bad "primary_undeletable map: $out"

# any — 401 / invalid_token
set_resp 401 '{"error":"invalid_token"}'
out="$(run list)"; rc=$?
echo "$out" | grep -qi "Tunnel token rejected" && [ $rc -ne 0 ] && ok "invalid_token mapped" || bad "invalid_token map: $out"

# ── arg validation (no network) ──────────────────────────────────────────
out="$(run create staging)"; rc=$?
echo "$out" | grep -qi "Usage: k2 connect subdomain create" && [ $rc -ne 0 ] && ok "create without --target -> usage" || bad "create no-target: $out"

# ── connect status (plan display) ─────────────────────────────────────────
# `connect status` (not `subdomain`) — uses its own dispatch path.
runc() { HOME="$TEST_HOME" "$K2_BIN" connect "$@" 2>&1; }

# Pro plan on the connected (tunnel.json: rosson) subdomain.
set_resp 200 '{"subdomains":[{"label":"rosson","host":"rosson.k2.dev","tier":"pro","primary":true},{"label":"z3thon","host":"z3thon.k2.dev","tier":"free"}]}'
out="$(runc status)"; rc=$?
[ "$(req_line)" = "GET /subdomains" ] && ok "status -> GET /subdomains" || bad "status method/path: $(req_line)"
echo "$out" | grep -q "Connected: rosson.k2.dev" && ok "status names the connected host" || bad "status host: $out"
echo "$out" | grep -qi "Plan: Pro" && echo "$out" | grep -qi "nested subdomains enabled" && [ $rc -eq 0 ] && ok "status shows Pro plan" || bad "status pro: $out (rc=$rc)"

# Single plan -> upsell line.
set_resp 200 '{"subdomains":[{"label":"rosson","host":"rosson.k2.dev","tier":"single","primary":true}]}'
out="$(runc status)"; rc=$?
echo "$out" | grep -qi "Plan: Single" && echo "$out" | grep -qi "upgrade to Pro" && ok "status shows Single + upgrade prompt" || bad "status single: $out"
echo "$out" | grep -q "k2 connect upgrade" && ok "status single names 'k2 connect upgrade'" || bad "status single upgrade hint: $out"

# Free plan.
set_resp 200 '{"subdomains":[{"label":"rosson","host":"rosson.k2.dev","tier":"free","primary":true}]}'
out="$(runc status)"; rc=$?
echo "$out" | grep -qi "Plan: Free" && ok "status shows Free plan" || bad "status free: $out"

# Server-marked connected row wins over local label match.
set_resp 200 '{"subdomains":[{"label":"rosson","host":"rosson.k2.dev","tier":"free"},{"label":"other","host":"other.k2.dev","tier":"pro","connected":true}]}'
out="$(runc status)"; rc=$?
echo "$out" | grep -q "Connected: other.k2.dev" && echo "$out" | grep -qi "Plan: Pro" && ok "status honours server 'connected' flag" || bad "status connected-flag: $out"

# ── connect upgrade (checkout) ────────────────────────────────────────────
set_resp 200 '{"url":"https://checkout.stripe.com/c/pay/test_123"}'
out="$(runc upgrade)"; rc=$?
[ "$(req_line)" = "POST /billing/checkout" ] && ok "upgrade -> POST /billing/checkout" || bad "upgrade method/path: $(req_line)"
echo "$(req_body)" | grep -q '"subdomain": "rosson"' && ok "upgrade defaults subdomain to tunnel.json (rosson)" || bad "upgrade body subdomain: $(req_body)"
echo "$(req_body)" | grep -q '"plan": "pro"' && ok "upgrade requests plan=pro" || bad "upgrade body plan: $(req_body)"
echo "$out" | grep -q "checkout.stripe.com" && [ $rc -eq 0 ] && ok "upgrade prints checkout URL" || bad "upgrade output: $out (rc=$rc)"

# upgrade with explicit subdomain arg.
set_resp 200 '{"checkout_url":"https://checkout.stripe.com/c/pay/test_staging"}'
out="$(runc upgrade staging)"; rc=$?
echo "$(req_body)" | grep -q '"subdomain": "staging"' && ok "upgrade <subdomain> overrides default" || bad "upgrade explicit subdomain: $(req_body)"
echo "$out" | grep -q "test_staging" && ok "upgrade reads checkout_url alias" || bad "upgrade checkout_url alias: $out"

# ── base override default (sanity: env var is honoured) ──────────────────
echo "$K2_CONNECT_BASE" | grep -q "127.0.0.1" && ok "K2_CONNECT_BASE override honoured" || bad "base override"

echo
echo "connect-subdomain: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
