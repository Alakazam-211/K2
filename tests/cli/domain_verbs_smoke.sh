#!/usr/bin/env bash
# Custom domains Slice A — CLI verb smoke: catalog, usage exit 2, schema.
#
# No daemon build. HOME sandboxed. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if grep -Fq -- "$needle" <<<"$hay"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}

echo "== catalog =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "domain tool id" "$(_cli_tool_id_for_verb domain)" "dns"
assert_eq "cert tool id" "$(_cli_tool_id_for_verb cert)" "dns"
if _cli_tool_is_locked dns; then
    echo "  PASS: dns locked (covers domain/cert)"
    pass=$((pass + 1))
else
    echo "  FAIL: dns must be locked" >&2
    fail=$((fail + 1))
fi

# shellcheck disable=SC1090
eval "$(sed -n '/^_uds_eligible()/,/^}/p' "$K2_CLI")"
if _uds_eligible "/cli/domains"; then
    echo "  PASS: /cli/domains UDS-eligible"
    pass=$((pass + 1))
else
    echo "  FAIL: /cli/domains should be UDS-eligible" >&2
    fail=$((fail + 1))
fi
if _uds_eligible "/cli/certs"; then
    echo "  PASS: /cli/certs UDS-eligible"
    pass=$((pass + 1))
else
    echo "  FAIL: /cli/certs should be UDS-eligible" >&2
    fail=$((fail + 1))
fi

echo "== usage =="
set +e
out="$("$K2_CLI" domain add 2>&1)"
rc=$?
set -e
assert_eq "domain add missing apex exit" "$rc" "2"
assert_contains "domain add usage json" "$out" "usage"

set +e
out="$("$K2_CLI" domain name add 2>&1)"
rc=$?
set -e
assert_eq "domain name add missing hostname exit" "$rc" "2"
assert_contains "domain name add usage" "$out" "usage"

set +e
out="$("$K2_CLI" domain totally-bogus 2>&1)"
rc=$?
set -e
assert_eq "domain unknown subcommand exit" "$rc" "2"
assert_contains "domain unknown teaches hostmail distinction" "$out" "hostmail"

echo "== help =="
help="$("$K2_CLI" domain --help)"
assert_contains "domain help mentions attach" "$help" "attach"
assert_contains "domain help not hostmail" "$help" "hostmail"
help_name="$("$K2_CLI" domain name add --help)"
assert_contains "name add is dns_manage" "$help_name" "dns_manage"
assert_contains "name add then cert issue" "$help_name" "cert issue"

help_doc="$("$K2_CLI" hostmail doctor --help)"
assert_contains "doctor CLI POSTs" "$help_doc" "POST"
help="$("$K2_CLI" cert --help)"
assert_contains "cert help not hostmail cert" "$help" "hostmail"
assert_contains "cert help has issue" "$help" "issue"

set +e
out="$("$K2_CLI" cert issue 2>&1)"
rc=$?
set -e
assert_eq "cert issue missing hostname exit" "$rc" "2"
assert_contains "cert issue usage" "$out" "usage"

echo "== schema =="
schema="$("$K2_CLI" --schema)"
assert_contains "schema has domain add" "$schema" '"name": "domain add"'
assert_contains "schema has cert list" "$schema" '"name": "cert list"'

assert_contains "schema has domain refresh" "$schema" '"name": "domain refresh"'

# ── A8.1: pending nameservers / owned elsewhere / API missing ────────
echo "== A8.1 stub daemon =="
WORK="$(mktemp -d -t k2-domain-cli-XXXXXX)"
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
echo "1" >"$HOME/.k2/heartbeat.port"
echo "owner-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
PENDING = {
    "apex": "pending.example", "zoneId": "z-pend", "dnsWrite": False,
    "status": "pending_ns", "pendingNs": True,
    "nameservers": ["ns1.k2.dev", "ns2.k2.dev"], "created": True, "names": [],
}
ACTIVE = dict(PENDING, dnsWrite=True, status="active", pendingNs=False)

class H(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = urllib.parse.urlparse(self.path).path
        if path == "/cli/domains":
            self._send(200, {"ok": True, "domains": [PENDING]})
        else:
            self._send(404, {"ok": False, "error": {"code": "not_found", "hint": path}})

    def do_POST(self):
        path = urllib.parse.urlparse(self.path).path
        n = int(self.headers.get("Content-Length", 0) or 0)
        b = json.loads((self.rfile.read(n) if n else b"{}").decode() or "{}")
        with open(os.path.join(work, "posts.jsonl"), "a") as f:
            f.write(json.dumps({"path": path, "body": b}) + "\n")
        apex = b.get("apex", "")
        if path == "/cli/domains" and apex == "owned.example":
            self._send(409, {"ok": False, "error": {"code": "zone_owned_elsewhere",
                "hint": "This domain belongs to another k2.dev account."}})
        elif path == "/cli/domains" and apex == "missing.example":
            self._send(503, {"ok": False, "error": {"code": "bind_api_unavailable",
                "hint": "Domain linking isn't live on k2.dev yet (POST /api/dns/zones/bind answered HTTP 405 with an empty body)."}})
        elif path == "/cli/domains":
            self._send(200, {"ok": True, "domain": PENDING})
        elif path == "/cli/domains/refresh":
            self._send(200, {"ok": True, "checked": True, "domains": [ACTIVE], "domain": ACTIVE})
        else:
            self._send(404, {"ok": False, "error": {"code": "not_found", "hint": path}})

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 50); do
    [ -f "$WORK/stub.port" ] && break
    sleep 0.05
done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

run_cli() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u K2_HOOK_TOKEN \
        K2_PORT="$STUB_PORT" \
        K2_PROJECT_PATH="$WORK" \
        "$K2_CLI" "$@"
}

set +e
out="$(run_cli domain list 2>"$WORK/err_list")"
rc=$?
set -e
assert_eq "list exit 0" "$rc" "0"
assert_contains "list shows pending column" "$out" "pending"
assert_contains "list names the nameservers" "$out" "Pending: point nameservers to ns1.k2.dev, ns2.k2.dev"
assert_contains "list notes auto-add" "$out" "Auto-added to your k2.dev account"
assert_contains "list teaches refresh" "$out" "k2 domain refresh pending.example"

set +e
out="$(run_cli domain add pending.example 2>"$WORK/err_add")"
rc=$?
set -e
assert_eq "add pending exit 0" "$rc" "0"
assert_contains "add says pending" "$out" "pending nameservers"
assert_contains "add names the nameservers" "$out" "Pending: point nameservers to ns1.k2.dev, ns2.k2.dev"

set +e
out="$(run_cli domain add owned.example 2>"$WORK/err_owned")"
rc=$?
set -e
assert_eq "add owned elsewhere exit 1" "$rc" "1"
assert_contains "owned elsewhere code" "$(cat "$WORK/err_owned")" '"code":"zone_owned_elsewhere"'
assert_contains "owned elsewhere copy" "$(cat "$WORK/err_owned")" "belongs to another k2.dev account"

set +e
out="$(run_cli domain add missing.example 2>"$WORK/err_missing")"
rc=$?
set -e
assert_eq "add api missing exit 1" "$rc" "1"
assert_contains "api missing code" "$(cat "$WORK/err_missing")" '"code":"bind_api_unavailable"'
assert_contains "api missing copy" "$(cat "$WORK/err_missing")" "Domain linking isn't live on k2.dev yet"

set +e
out="$(run_cli domain refresh pending.example 2>"$WORK/err_refresh")"
rc=$?
set -e
assert_eq "refresh exit 0" "$rc" "0"
assert_contains "refresh shows active" "$out" "K2-hosted DNS (writable)"
assert_contains "refresh POSTs apex" "$(cat "$WORK/posts.jsonl")" '"path": "/cli/domains/refresh", "body": {"apex": "pending.example"}'

set +e
out="$(run_cli domain refresh --json 2>"$WORK/err_refresh_all")"
rc=$?
set -e
assert_eq "refresh all exit 0" "$rc" "0"
assert_contains "refresh all POSTs empty body" "$(cat "$WORK/posts.jsonl")" '"path": "/cli/domains/refresh", "body": {}'
assert_contains "refresh --json passes through" "$out" '"checked":true'

help_refresh="$("$K2_CLI" domain refresh --help)"
assert_contains "refresh help says check now" "$help_refresh" "Check now"

echo "== $pass passed, $fail failed =="
[ "$fail" -eq 0 ]
