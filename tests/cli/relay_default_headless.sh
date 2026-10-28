#!/usr/bin/env bash
# 0.46.6 relay default (PRD relay-default-and-ptr-guard test 25), headless:
# k2-daemon from this worktree's target/ under a throwaway HOME, every
# K2_* var unset, the relay-list endpoint a local fake that answers 401
# (a token k2.dev doesn't list) and records each request's headers.
#
#   1. A 0.46.5 tunnel.json (server_addr = the old baked IP) is dialed as
#      relay.k2.dev: /cli/tunnel/status relay_list.relays and .replaced,
#      and `k2 tunnel status` prints the replace note. tunnel.json is not
#      rewritten.
#   2. The fetch carries X-K2-Relays / X-K2-Relay-Source (the 401 too).
#   3. relay_default "legacy" in tunnel.json, then K2_RELAY_DEFAULT=legacy
#      at daemon start, both restore the old IP; status says so.
#
# The daemon looks relay.k2.dev up in DNS (a read, like every box); the
# tunnel never starts (auto_start false), so no relay is dialed.
#
# Build first (worktree target/, never main's): cargo build -p k2-daemon
# Never touches ~/.k2, the keychain or a running daemon. Fails loudly.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

FAKE_DIR="$(mktemp -d -t k2-relay-fake-XXXXXX)"
cat >"$FAKE_DIR/fake.py" <<'PYEOF'
import http.server, json, sys
log = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(log, "a") as f:
            f.write(json.dumps({"path": self.path, "headers": {k.lower(): v for k, v in self.headers.items()}}) + "\n")
        body = b'{"code":"tunnel_only"}'
        self.send_response(401)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
with open(log + ".port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
python3 "$FAKE_DIR/fake.py" "$FAKE_DIR/requests.jsonl" &
FAKE_PID=$!
for _ in $(seq 1 50); do [ -s "$FAKE_DIR/requests.jsonl.port" ] && break; sleep 0.1; done
[ -s "$FAKE_DIR/requests.jsonl.port" ] || { echo "FAIL: the fake relay-list server never started" >&2; exit 1; }
export K2_RELAY_LIST_URL="http://127.0.0.1:$(cat "$FAKE_DIR/requests.jsonl.port")/api/tunnel/relays"
export K2_TUNNEL_LEASE=off

# shellcheck source=_sandbox_daemon.sh
source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; kill "$FAKE_PID" 2>/dev/null || true; rm -rf "$FAKE_DIR"' EXIT

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $2, want $3)"; fi; }
contains() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $3 in: ${2:0:400})"; fi; }
status_field() {
    curl -sS "http://127.0.0.1:${K2SO_PORT}/cli/tunnel/status?token=${K2SO_TOKEN}" \
        | python3 -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."): v = v.get(k) if isinstance(v, dict) else None
print(json.dumps(v))' "$1"
}
cli_status() { HOME="$SANDBOX_HOME" "$PROJECT_ROOT/cli/k2" tunnel status; }
TUNNEL_JSON="$SANDBOX_HOME/.k2/tunnel.json"
write_tunnel_json() { # $1 = extra JSON members (may be empty)
    printf '{\n  "server_addr": "178.156.232.105",\n  "server_port": 7000,\n  "token": "k2c_headless_relay_test",\n  "auto_start": false,\n  "e2e": false%s\n}\n' "$1" >"$TUNNEL_JSON"
    chmod 0600 "$TUNNEL_JSON"
}
wait_requests() { # $1 = how many fetches must have arrived
    for _ in $(seq 1 100); do
        [ "$(wc -l <"$FAKE_DIR/requests.jsonl" 2>/dev/null || echo 0)" -ge "$1" ] && return 0
        sleep 0.2
    done
    bad "the daemon never fetched the relay list ($1 expected)"
    return 1
}

echo "== a 0.46.5 tunnel.json is dialed as relay.k2.dev"
write_tunnel_json ""
sandbox_daemon_restart daemon-2.log
wait_requests 1
# Boot may re-save the file once (the vault takes the token: tokenHome);
# from here on nothing may touch it, and the old address stays in it.
before="$(cksum <"$TUNNEL_JSON")"
expect "relays" "$(status_field relay_list.relays)" '["relay.k2.dev:7000"]'
expect "replaced" "$(status_field relay_list.replaced)" '["178.156.232.105:7000"]'
expect "source" "$(status_field relay_list.source)" '"default"'
expect "the 401 is a notice" "$(status_field relay_list.last_error)" 'null'
out="$(cli_status)"
contains "k2 tunnel status" "$out" "Relays:     relay.k2.dev:7000 [default; replaces the old built-in 178.156.232.105:7000]"
expect "tunnel.json keeps the old address" \
    "$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("server_addr"), "relays" in d, "relay.k2.dev" in open(sys.argv[1]).read())' "$TUNNEL_JSON")" \
    "178.156.232.105 False False"
sleep 2
status_field relay_list.relays >/dev/null
expect "status reads never rewrite tunnel.json" "$(cksum <"$TUNNEL_JSON")" "$before"

echo "== the fetch carries the readiness headers (on a 401)"
req="$(tail -1 "$FAKE_DIR/requests.jsonl")"
hdr() { python3 -c 'import json,sys; print(json.loads(sys.argv[1])["headers"].get(sys.argv[2], ""))' "$req" "$1"; }
expect "X-K2-Relays" "$(hdr x-k2-relays)" "relay.k2.dev:7000"
expect "X-K2-Relay-Source" "$(hdr x-k2-relay-source)" "default"
contains "User-Agent" "$(hdr user-agent)" "k2-daemon/"

echo "== tunnel.json relay_default legacy restores the old IP (no restart)"
write_tunnel_json ',
  "relay_default": "legacy"'
expect "relays (file switch)" "$(status_field relay_list.relays)" '["178.156.232.105:7000"]'
expect "replaced (file switch)" "$(status_field relay_list.replaced)" 'null'
expect "relay_default (file switch)" "$(status_field relay_list.relay_default)" '"legacy"'
contains "k2 tunnel status (file switch)" "$(cli_status)" "Relay default: legacy"

echo "== K2_RELAY_DEFAULT=legacy at daemon start restores the old IP"
write_tunnel_json ""
export K2_RELAY_DEFAULT=legacy
sandbox_daemon_restart daemon-3.log
unset K2_RELAY_DEFAULT
wait_requests 2
expect "relays (env switch)" "$(status_field relay_list.relays)" '["178.156.232.105:7000"]'
expect "relay_default (env switch)" "$(status_field relay_list.relay_default)" '"legacy"'
req="$(tail -1 "$FAKE_DIR/requests.jsonl")"
expect "X-K2-Relays (env switch)" "$(hdr x-k2-relays)" "178.156.232.105:7000"

echo
echo "relay_default_headless: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
