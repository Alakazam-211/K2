#!/usr/bin/env bash
# phone-web-off: `k2 users pair-phone | phones | unpair-phone` against a real
# sandbox daemon (temp HOME, never ~/.k2), plus the QR drawing against a
# one-shot stub that returns a known matrix.
#
# Needs a built daemon: cargo build -p k2-daemon (see _sandbox_daemon.sh).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

# shellcheck source=_sandbox_daemon.sh
source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start

fail=0
pass() { echo "  PASS: $1"; }
bad() { echo "  FAIL: $1" >&2; fail=1; }
contains() { if grep -Fq -- "$2" <<<"$3"; then pass "$1"; else bad "$1 (missing '$2' in: ${3:0:400})"; fi; }

k2() { HOME="$SANDBOX_HOME" K2SO_PORT="$K2SO_PORT" K2SO_TOKEN="$K2SO_TOKEN" "$K2" "$@"; }

printf 'password123\n' | k2 users add mobile --role member --password-stdin >/dev/null

# 1. The owner token with no linked login and no --user: refused, exit 3.
set +e
out="$(k2 users pair-phone 2>&1)"; rc=$?
set -e
[ "$rc" = 3 ] && pass "no linked login exits 3" || bad "no linked login exit $rc: $out"
contains "the hint says how to link a login" "Your account" "$out"
contains "the code is named" "owner_login_not_linked" "$out"

# 2. --user: a code, no link/QR on a box without a public host.
out="$(k2 users pair-phone --user mobile)"
contains "prints the code" "Pairing code for mobile: K2P-" "$out"
contains "no public host" "no public host" "$out"
json="$(k2 users pair-phone --user mobile --json)"
code="$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["ok"] is True and d["user"]=="mobile", d; print(d["code"])' "$json")"

# 3. Redeem it like the phone would, then list and unpair.
curl -sS -X POST "http://127.0.0.1:${K2SO_PORT}/cli/auth/device/pair" -H 'Content-Type: application/json' \
    --data-raw "{\"code\":\"${code}\",\"device\":{\"id\":\"cli-test-1\",\"name\":\"Bad\\u202Ename\"}}" >"$SANDBOX_HOME/pair.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["ok"] is True, d; assert d["device"]["name"]=="Badname", d' "$SANDBOX_HOME/pair.json" \
    && pass "the phone pairs; the name loses its bidi character" || bad "pair: $(cat "$SANDBOX_HOME/pair.json")"
out="$(k2 users phones)"
contains "phones lists it" "Badname" "$out"
contains "phones names the login" "mobile" "$out"
contains "phones names the minter" "owner-token" "$out"
id="$(k2 users phones --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["devices"][0]["id"])')"
out="$(k2 users unpair-phone "$id")"
contains "unpair-phone ends it" "Unpaired phone $id" "$out"
out="$(k2 users phones)"
contains "nothing left" "No paired phones." "$out"
set +e
out="$(k2 users unpair-phone "$id" 2>&1)"; rc=$?
set -e
[ "$rc" = 3 ] && pass "unpairing again exits 3" || bad "unpair again exit $rc: $out"
set +e
k2 users unpair-phone >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = 2 ] && pass "unpair-phone without an id is usage (2)" || bad "usage exit $rc"

# 4. The QR is drawn from the daemon's matrix (stub returns a 2x2 test
#    matrix: dark top-left and bottom-right).
stub_port_file="$SANDBOX_HOME/stub.port"
python3 - "$stub_port_file" <<'PY' &
import http.server, json, sys
body = json.dumps({"ok": True, "code": "K2P-AAAAA-BBBBB-CCCCC-DDDDD", "user": "mobile",
                   "expiresAt": "2026-10-24T00:10:00Z", "expiresInSecs": 600, "host": "rosson.k2.dev",
                   "pairUrl": "https://rosson.k2.dev/pair#K2P-AAAAA-BBBBB-CCCCC-DDDDD",
                   "qr": {"size": 2, "rows": ["10", "01"]}}).encode()
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        self.send_response(200); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
open(sys.argv[1], "w").write(str(s.server_address[1]))
s.handle_request()
PY
stub_pid=$!
for _ in $(seq 1 50); do [ -s "$stub_port_file" ] && break; sleep 0.1; done
out="$(HOME="$SANDBOX_HOME" K2SO_PORT="$(cat "$stub_port_file")" K2SO_TOKEN="$K2SO_TOKEN" "$K2" users pair-phone --user mobile)"
wait "$stub_pid" || true
contains "prints the link" "Link: https://rosson.k2.dev/pair#K2P-AAAAA-BBBBB-CCCCC-DDDDD" "$out"
# 2x2 + 2-module quiet zone = 6 wide, 3 text rows. Light = ink (dark
# terminal): row pair 0 (quiet) full; row pair 1 = rows 0,1 of the matrix.
want_mid="$(printf '\xe2\x96\x88\xe2\x96\x88\xe2\x96\x84\xe2\x96\x80\xe2\x96\x88\xe2\x96\x88')"
contains "the matrix is drawn with half blocks" "$want_mid" "$out"

[ "$fail" = 0 ] || { echo "users_pair_phone: FAILED" >&2; exit 1; }
echo "users_pair_phone: OK"
