#!/usr/bin/env bash
# Hermetic test for step 9c (hosted mail on the customer's domain) of
# scripts/provision-k2-server.sh. Cuts the 9c block out and runs it alone
# against a fake daemon on 127.0.0.1 (python http.server). No root, no
# network beyond loopback. The Hetzner metadata probe is shimmed off.
# Run with: bash tests/cli/provision_mail_setup.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROVISION="$ROOT/scripts/provision-k2-server.sh"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

echo "== static shape =="
bash -n "$PROVISION" || fail "bash -n"
line_of() { grep -n -- "$1" "$PROVISION" | head -n1 | cut -d: -f1; }
L_BAKE="$(line_of 'bake complete')"; L_9B="$(line_of '# ── 9b\.')"
L_9C="$(line_of '# ── 9c\.')"; L_10="$(line_of '# ── 10\. control-plane callback')"
[ "$L_BAKE" -lt "$L_9B" ] && [ "$L_9B" -lt "$L_9C" ] && [ "$L_9C" -lt "$L_10" ] || fail "9c must sit after 9b and the bake exit, before the callback"
ok "9c sits after 9b (and never runs in --bake), before the callback"
BLOCK="$(sed -n '/^# ── 9c\./,/^# ── 10\./p' "$PROVISION" | sed '$d')"
if printf '%s\n' "$BLOCK" | grep -Ev '^[[:space:]]*#' | grep -Eq '(^|[^_])die '; then fail "9c must never die"; fi
if printf '%s\n' "$BLOCK" | grep -Ev '^[[:space:]]*#' | grep -Eq 'curl[^|]*\$TOKEN|curl[^|]*MAIL_TOKEN'; then fail "the owner token must not ride curl argv"; fi
ok "9c never dies and keeps the owner token off argv"
grep -q '"mail":json.loads(os.environ\["MAIL_JSON"\])' "$PROVISION" || fail "callback must carry the mail block"
ok "callback carries the mail block"

TMP="$(mktemp -d)"; trap 'kill "${SRV_PID:-0}" 2>/dev/null || true; rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"
# Shim curl only for the Hetzner metadata probe (fails = not Hetzner).
printf '#!/bin/sh\nexit 7\n' > "$TMP/bin/curl"; chmod +x "$TMP/bin/curl"

# Fake daemon. MODE file picks the behaviour; requests are logged.
cat > "$TMP/fake.py" <<'PY'
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer
D = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode(); self.send_response(code)
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(b)))
        self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0)); body = self.rfile.read(n).decode()
        open(f"{D}/log", "a").write(f"POST {self.path} {body}\n")
        mode = open(f"{D}/mode").read().strip()
        if mode == "old": return self.reply(404, {"error": "not found"})
        if mode == "refuse": return self.reply(409, {"ok": False, "error": {"code": "airgap", "message": "air-gapped build"}})
        self.reply(200, {"ok": True, "setupId": "s1", "phase": "attach_zone"})
    def do_GET(self):
        open(f"{D}/log", "a").write(f"GET {self.path}\n")
        mode = open(f"{D}/mode").read().strip()
        c = int(open(f"{D}/n").read() if os.path.exists(f"{D}/n") else 0) + 1
        open(f"{D}/n", "w").write(str(c))
        phase = {"done": "done" if c >= 2 else "plant_records", "wait": "waiting_delegation",
                 "slow": "waiting_cert"}.get(mode, "error")
        self.reply(200, {"phase": phase, "hostname": "mail.agents.client.com", "lastError": None, "steps": []})
s = HTTPServer(("127.0.0.1", 0), H); open(f"{D}/port", "w").write(str(s.server_port)); s.serve_forever()
PY
echo done > "$TMP/mode"
python3 "$TMP/fake.py" "$TMP" & SRV_PID=$!
for _ in $(seq 50); do [ -s "$TMP/port" ] && break; sleep 0.1; done
PORT="$(cat "$TMP/port")"

run9c() {  # env assignments as args; prints MAIL_LINE / MAIL_JSON
	rm -f "$TMP/log" "$TMP/n"
	env -i PATH="$TMP/bin:/usr/bin:/bin" BASE="http://127.0.0.1:$PORT" TOKEN="secret-owner-token" \
		K2_MAIL_POLL_S=0 K2_MAIL_WAIT_S="${WAIT:-5}" "$@" bash -c '
		set -euo pipefail
		log() { echo "[provision] $*" >&2; }
		'"$BLOCK"'
		printf "LINE=%s\nJSON=%s\n" "$MAIL_LINE" "$MAIL_JSON"'
}
field() { printf '%s\n' "$1" | sed -n 's/^JSON=//p' | python3 -c "import json,sys;d=json.load(sys.stdin);print(d$2)"; }

echo "== skips =="
OUT="$(run9c K2_TUNNEL_TOKEN=k2c_x 2>/dev/null)"; printf '%s' "$OUT" | grep -q 'LINE=off (no mail domain)' || fail "no domain = off"
[ ! -e "$TMP/log" ] || fail "no domain must not call the daemon"; ok "no domain: off, daemon untouched"
OUT="$(run9c K2_MAIL_DOMAIN=agents.client.com K2_TUNNEL_TOKEN=k2c_x K2_MAIL_SKIP=1 2>/dev/null)"
printf '%s' "$OUT" | grep -q 'K2_MAIL_SKIP=1' && [ ! -e "$TMP/log" ] || fail "kill switch"; ok "K2_MAIL_SKIP=1"
OUT="$(run9c K2_MAIL_DOMAIN=agents.client.com 2>/dev/null)"
printf '%s' "$OUT" | grep -q 'per-customer setup only' && [ ! -e "$TMP/log" ] || fail "no tunnel token must skip"; ok "no customer tunnel token: skipped"
OUT="$(run9c K2_MAIL_DOMAIN=agents.client.com K2_TUNNEL_TOKEN=k2c_x K2_PROVIDER=Hetzner 2>/dev/null)"
printf '%s' "$OUT" | grep -q 'Hetzner' && [ ! -e "$TMP/log" ] || fail "Hetzner must skip"; ok "Hetzner: skipped"

echo "== runs =="
OUT="$(run9c K2_MAIL_DOMAIN=Agents.Client.com. K2_TUNNEL_TOKEN=k2c_x K2_PROVIDER=ovh 2>/dev/null)"
[ "$(field "$OUT" '["phase"]')" = "done" ] || fail "done path: $OUT"
grep -q '"domain": "agents.client.com", "portPlan": "auto"' "$TMP/log" || fail "POST body: $(cat "$TMP/log")"
grep -q 'GET /cli/mail/setup' "$TMP/log" || fail "must poll"
ok "POST {domain (normalised), portPlan:auto}, polls to done"
echo wait > "$TMP/mode"; OUT="$(run9c K2_MAIL_DOMAIN=agents.client.com K2_TUNNEL_TOKEN=k2c_x 2>/dev/null)"
[ "$(field "$OUT" '["phase"]')" = "waiting_delegation" ] || fail "waiting_delegation is terminal"; ok "stops at waiting_delegation"
echo slow > "$TMP/mode"; OUT="$(WAIT=1 run9c K2_MAIL_DOMAIN=agents.client.com K2_TUNNEL_TOKEN=k2c_x 2>/dev/null)"
[ "$(field "$OUT" '.get("timedOut")')" = "True" ] && [ "$(field "$OUT" '["phase"]')" = "waiting_cert" ] || fail "timeout: $OUT"
ok "times out on a non-terminal phase without failing"
echo old > "$TMP/mode"; OUT="$(run9c K2_MAIL_DOMAIN=agents.client.com K2_TUNNEL_TOKEN=k2c_x 2>/dev/null)"
[ "$(field "$OUT" '["phase"]')" = "unsupported" ] || fail "404 = unsupported"; ok "old daemon (404): unsupported, not a failure"
echo refuse > "$TMP/mode"; OUT="$(run9c K2_MAIL_DOMAIN=agents.client.com K2_TUNNEL_TOKEN=k2c_x 2>/dev/null)"
[ "$(field "$OUT" '["lastError"]["code"]')" = "airgap" ] || fail "refusal code: $OUT"; ok "daemon refusal: error with its code"
echo done > "$TMP/mode"
for bad in agents.k2.dev k2.dev 'bad domain' x; do
	OUT="$(run9c K2_MAIL_DOMAIN="$bad" K2_TUNNEL_TOKEN=k2c_x 2>/dev/null)"
	[ "$(field "$OUT" '["lastError"]["code"]')" = "invalid_domain" ] && [ ! -e "$TMP/log" ] || fail "must refuse '$bad'"
done
ok "refuses k2.dev and malformed names without calling the daemon"
kill "$SRV_PID"; OUT="$(run9c K2_MAIL_DOMAIN=agents.client.com K2_TUNNEL_TOKEN=k2c_x 2>/dev/null)"
[ "$(field "$OUT" '["phase"]')" = "error" ] || fail "daemon down must be error, not a crash"; ok "daemon unreachable: error, provisioning continues"
printf '%s' "$OUT" | grep -q secret-owner-token && fail "token leaked into output"; ok "token not in output"

echo; echo "PASS: provision mail setup ($pass checks)"
