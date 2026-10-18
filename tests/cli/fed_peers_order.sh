#!/usr/bin/env bash
# `k2 fed peers` lists paired servers A to Z (0.46.1): case-insensitive by
# the name shown (label, else subdomain), then host, the same order as
# Settings and the Connections picker. A 0.46.1+ daemon already sends this
# order (crates/k2-daemon/tests/federation_peer_rosters_integration.rs pins
# that end to end); here a stub daemon answers in PAIRING order, like an
# older server, and the CLI must still print A to Z.
# Hermetic: temp HOME, a stub daemon on loopback, no real ~/.k2. Loud.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORKDIR="$(mktemp -d -t k2-fed-order-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORKDIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

python3 - "$WORKDIR" <<'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

root = sys.argv[1]
# Pairing order, mixed case; one pin has no label (its subdomain shows).
PEERS = [
    {"fingerprint": "f" * 64, "label": "zulu", "subdomain": "zulu", "base_url": "", "trust": "trusted"},
    {"fingerprint": "b" * 64, "label": "Bravo", "subdomain": "bravo", "base_url": "", "trust": "trusted"},
    {"fingerprint": "c" * 64, "label": "", "subdomain": "charlie", "base_url": "", "trust": "pending"},
    {"fingerprint": "a" * 64, "label": "alpha", "subdomain": "alpha-two", "base_url": "", "trust": "trusted"},
    {"fingerprint": "d" * 64, "label": "Alpha", "subdomain": "alpha-one", "base_url": "", "trust": "trusted"},
]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def do_GET(self):
        if urlparse(self.path).path == "/cli/federation/peers":
            body = json.dumps({"peers": PEERS}).encode()
            code = 200
        else:
            body = json.dumps({"error": "not found"}).encode()
            code = 404
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

httpd = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(root, "stub.port"), "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do
    [ -s "$WORKDIR/stub.port" ] && break
    sleep 0.05
done
[ -s "$WORKDIR/stub.port" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }

echo "== k2 fed peers prints A to Z =="
set +e
out="$(K2_PORT="$(cat "$WORKDIR/stub.port")" "$K2_CLI" fed peers 2>"$WORKDIR/err")"
rc=$?
set -e
[ "$rc" = "0" ] && ok "exit 0" || bad "exit $rc: $(cat "$WORKDIR/err")"
hosts="$(printf '%s\n' "$out" | awk '/trust=/{print $1}' | tr '\n' ' ')"
# alpha-one before alpha-two: same name, the host breaks the tie.
want="alpha-one.k2.dev alpha-two.k2.dev bravo.k2.dev charlie.k2.dev zulu.k2.dev "
if [ "$hosts" = "$want" ]; then
    ok "order: $hosts"
else
    bad "order (got=$(printf %q "$hosts") want=$(printf %q "$want"))"
fi
case "$out" in
    *"Peers (5):"*) ok "count line" ;;
    *) bad "count line missing in $(printf %q "$out")" ;;
esac

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
