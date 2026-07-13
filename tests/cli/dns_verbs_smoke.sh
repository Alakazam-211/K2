#!/usr/bin/env bash
# DNS K1 — CLI verb smoke: usage exit 2, gated exit 3, route wiring, UDS prefer.
#
# Covers:
#   1. Catalog: dns → tool id "dns", mode id, locked (already in
#      id_tool_auth_no_owner_inversion.sh for pure helpers; re-checked here).
#   2. Local usage errors exit 2 with JSON {"error":{"code":"usage",...}}
#      without requiring a live control plane (stub daemon returns nothing
#      useful — these fire before HTTP).
#   3. Stub daemon: access GET path + scoped token; gated list → exit 3
#      with Settings teaching text; record add POST body shape.
#   4. Schema mentions dns verbs.
#
# No daemon build. HOME sandboxed. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
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
    if printf '%s' "$hay" | grep -Fq "$needle"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}

# ── 1. Catalog helpers ────────────────────────────────────────────────
echo "== catalog =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "dns tool id" "$(_cli_tool_id_for_verb dns)" "dns"
assert_eq "dns mode id" "$(_cli_tool_default_mode dns)" "id"
if _cli_tool_is_locked dns; then
    echo "  PASS: dns locked"
    pass=$((pass + 1))
else
    echo "  FAIL: dns must be locked" >&2
    fail=$((fail + 1))
fi

# UDS eligibility
# shellcheck disable=SC1090
eval "$(sed -n '/^_uds_eligible()/,/^}/p' "$K2_CLI")"
if _uds_eligible "/cli/dns/access"; then
    echo "  PASS: /cli/dns/access UDS-eligible"
    pass=$((pass + 1))
else
    echo "  FAIL: /cli/dns/access should be UDS-eligible" >&2
    fail=$((fail + 1))
fi
if _uds_eligible "/cli/dns/records/add"; then
    echo "  PASS: /cli/dns/records/add UDS-eligible"
    pass=$((pass + 1))
else
    echo "  FAIL: /cli/dns/records/add should be UDS-eligible" >&2
    fail=$((fail + 1))
fi

# ── 2. Schema surface ─────────────────────────────────────────────────
echo "== schema =="
WORK="$(mktemp -d -t k2-dns-cli-XXXXXX)"
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

schema_out="$("$K2_CLI" --schema 2>/dev/null || true)"
assert_contains "schema has dns access" "$schema_out" '"name": "dns access"'
assert_contains "schema has dns record add" "$schema_out" '"name": "dns record add"'
assert_contains "schema mentions exit 3 / Settings" "$schema_out" 'Settings'

# ── 3. Usage errors (exit 2) via stub port ────────────────────────────
echo "== usage exit 2 =="
python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
log_path = os.path.join(work, "reqs.jsonl")

class H(BaseHTTPRequestHandler):
    def _handle(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {
            "method": method,
            "path": parsed.path,
            "token_query": (q.get("token") or [""])[0],
            "authorization": self.headers.get("Authorization") or "",
            "body": body.decode("utf-8", "replace"),
            "query": {k: v[0] if len(v) == 1 else v for k, v in q.items() if k != "token"},
        }
        with open(log_path, "a") as f:
            f.write(json.dumps(rec) + "\n")

        path = parsed.path
        # Gated list: 403 dns_manage_disabled (mail-style exit 3)
        if path == "/cli/dns/zones":
            data = json.dumps({
                "ok": False,
                "error": {
                    "code": "dns_manage_disabled",
                    "hint": "this agent isn't allowed to manage DNS — the owner can enable it in Settings → K2 Connect (Allow agents to manage DNS records) or Workspaces → (workspace) → Allow DNS manage",
                },
            }).encode()
            self.send_response(403)
        elif path == "/cli/dns/access":
            data = json.dumps({
                "ok": True,
                "allowed": True,
                "zones": [{"id": "z1", "domain": "example.com", "status": "active"}],
                "record_types": ["A", "AAAA", "CNAME", "TXT", "MX", "SRV", "CAA"],
                "workspace": {"id": "ws-1", "path": "/tmp/ws"},
            }).encode()
            self.send_response(200)
        elif path == "/cli/dns/records":
            data = json.dumps({
                "ok": True,
                "domain": (q.get("domain") or ["?"])[0],
                "records": [
                    {"id": "r1", "type": "A", "name": "www", "content": "203.0.113.10", "ttl": 60, "managed_by": "user"},
                ],
            }).encode()
            self.send_response(200)
        elif path == "/cli/dns/records/add":
            try:
                b = json.loads(body.decode() or "{}")
            except Exception:
                b = {}
            data = json.dumps({
                "ok": True,
                "record": {
                    "id": "r-new",
                    "type": b.get("type"),
                    "name": b.get("name"),
                    "content": b.get("value") or b.get("content"),
                },
                "propagation": "~10s",
            }).encode()
            self.send_response(201)
        elif path == "/cli/dns/records/remove":
            data = json.dumps({"ok": True, "id": json.loads(body or b"{}").get("id", "?")}).encode()
            self.send_response(200)
        elif path == "/cli/dns/verify":
            data = json.dumps({"ok": True, "status": "ok", "domain": json.loads(body or b"{}").get("domain")}).encode()
            self.send_response(200)
        else:
            data = json.dumps({"ok": False, "error": {"code": "not_found", "hint": path}}).encode()
            self.send_response(404)

        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._handle("GET")

    def do_POST(self):
        self._handle("POST")

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
SCOPED="sessdns.scoped-secret-cccc"

run_cli() {
    # shellcheck disable=SC2030,SC2031
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK \
        K2_PORT="$STUB_PORT" \
        K2_HOOK_TOKEN="$SCOPED" \
        K2_PROJECT_PATH="$WORK" \
        "$K2_CLI" "$@"
}

set +e
out="$(run_cli dns records 2>"$WORK/err_records_usage")"
rc=$?
set -e
assert_eq "records missing domain exit 2" "$rc" "2"
assert_contains "records usage code" "$(cat "$WORK/err_records_usage")" '"code":"usage"'

set +e
out="$(run_cli dns record add example.com A 2>"$WORK/err_add_usage")"
rc=$?
set -e
assert_eq "record add incomplete exit 2" "$rc" "2"
assert_contains "record add usage" "$(cat "$WORK/err_add_usage")" '"code":"usage"'

set +e
out="$(run_cli dns record remove 2>"$WORK/err_rm_usage")"
rc=$?
set -e
assert_eq "record remove missing id exit 2" "$rc" "2"

set +e
out="$(run_cli dns verify 2>"$WORK/err_verify_usage")"
rc=$?
set -e
assert_eq "verify missing domain exit 2" "$rc" "2"

set +e
out="$(run_cli dns totally-bogus 2>"$WORK/err_unknown")"
rc=$?
set -e
assert_eq "unknown subcommand exit 2" "$rc" "2"

# ── 4. Happy / gated routes ──────────────────────────────────────────
echo "== routes + exit 3 =="
: >"$WORK/reqs.jsonl"

set +e
out="$(run_cli dns access --json 2>"$WORK/err_access")"
rc=$?
set -e
assert_eq "access exit 0" "$rc" "0"
assert_contains "access allowed true" "$out" '"allowed":true'
assert_contains "access hit /cli/dns/access" "$(cat "$WORK/reqs.jsonl")" '"path": "/cli/dns/access"'
tok="$(python3 -c 'import json
for line in open("'"$WORK"'/reqs.jsonl"):
    r=json.loads(line)
    if r["path"]=="/cli/dns/access":
        print(r["token_query"]); break
')"
assert_eq "access uses scoped token (ID tool)" "$tok" "$SCOPED"

set +e
out="$(run_cli dns list 2>"$WORK/err_list")"
rc=$?
set -e
assert_eq "list gated exit 3" "$rc" "3"
assert_contains "list error code dns_manage_disabled" "$(cat "$WORK/err_list")" 'dns_manage_disabled'
assert_contains "list teaching mentions Settings" "$(cat "$WORK/err_list")" 'Settings'

set +e
out="$(run_cli dns records example.com --json 2>"$WORK/err_recs")"
rc=$?
set -e
assert_eq "records exit 0" "$rc" "0"
assert_contains "records domain param" "$(cat "$WORK/reqs.jsonl")" 'example.com'
assert_contains "records path" "$(cat "$WORK/reqs.jsonl")" '"path": "/cli/dns/records"'

set +e
out="$(run_cli dns record add example.com A www 203.0.113.10 --ttl 60 --json 2>"$WORK/err_add")"
rc=$?
set -e
assert_eq "record add exit 0" "$rc" "0"
assert_contains "record add POST" "$(cat "$WORK/reqs.jsonl")" '"method": "POST"'
assert_contains "record add path" "$(cat "$WORK/reqs.jsonl")" '"path": "/cli/dns/records/add"'
# Body shape: domain, type, name, value, ttl
add_body="$(python3 -c 'import json
for line in open("'"$WORK"'/reqs.jsonl"):
    r=json.loads(line)
    if r["path"]=="/cli/dns/records/add":
        print(r["body"]); break
')"
assert_contains "add body type A" "$add_body" '"type": "A"'
assert_contains "add body value" "$add_body" '203.0.113.10'
assert_contains "add body ttl 60" "$add_body" '"ttl": 60'

set +e
out="$(run_cli dns record remove r1 --json 2>"$WORK/err_rm")"
rc=$?
set -e
assert_eq "record remove exit 0" "$rc" "0"
assert_contains "remove path" "$(cat "$WORK/reqs.jsonl")" '"path": "/cli/dns/records/remove"'

set +e
out="$(run_cli dns verify example.com --json 2>"$WORK/err_ver")"
rc=$?
set -e
assert_eq "verify exit 0" "$rc" "0"
assert_contains "verify path" "$(cat "$WORK/reqs.jsonl")" '"path": "/cli/dns/verify"'

# Help does not need perfect network — still needs connection; stub is fine
set +e
out="$(run_cli dns --help 2>/dev/null)"
rc=$?
set -e
assert_eq "dns --help exit 0" "$rc" "0"
assert_contains "help mentions Settings → K2 Connect" "$out" 'Settings → K2 Connect'

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
