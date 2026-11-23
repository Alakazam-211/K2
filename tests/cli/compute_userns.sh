#!/usr/bin/env bash
# k2 compute run --userns and k2 compute grant --userns (user namespaces in
# jobs, 0.46.18): CLI shapes against a tiny stub daemon on loopback
# (python3 http.server). No real daemon, no ~/.k2, no keychain: HOME is a
# throwaway dir.
#
# Covers: --userns sends userns: true, no --userns sends no key;
# `k2 compute test --userns` passes it through; userns_not_granted and
# userns_owner_only exit 3 with the daemon's hint; grant --userns /
# --no-userns send allowUserns and the answer says "--userns jobs allowed";
# re-granting limits sends no allowUserns (keeps it); a tenant node's
# refusal (userns_owner_only) exits 3 naming the node; `get` prints the
# userns line only when the job asked; `--plan` prints it; `grants` shows
# it; the help, `k2 study compute` and the schema name the flag and codes.
# --no-addr-filter: run/test send noAddrFilter, grant --no-addr-filter /
# --addr-filter send allowNoAddrFilter, get prints "address filter: off".

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2userns-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]
OWNER_ONLY = "--userns is only for the node owner's own server: this server is a tenant of shared-node, so its jobs can't make user namespaces there"

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path == "/cli/compute/jobs/get":
            j = {"id": "abcd1234" * 4, "short": "abcd1234", "node": "own-node", "state": "done", "exitCode": 0, "argv": ["true"], "result": {}}
            if q.get("job", [""])[0].startswith("abcd"):
                j["userns"] = True
                j["noAddrFilter"] = True
            return self.reply(200, j)
        if u.path == "/cli/compute/grants":
            return self.reply(200, {"grants": [
                {"node": "own-node", "workspace": "builder", "maxJobSecs": 7200, "maxParallel": 1, "maxQueued": 10, "maxDiskGb": 60, "allowUserns": True, "grantedBy": "owner"},
                {"node": "plain", "workspace": "builder", "maxJobSecs": 7200, "maxParallel": 1, "maxQueued": 10, "maxDiskGb": 60, "grantedBy": "owner"}]})
        return self.reply(404, {"error": {"code": "not_found", "hint": self.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": body}) + "\n")
        if u.path == "/cli/compute/run":
            if body.get("plan"):
                return self.reply(200, {"node": "own-node", "startsNow": True, "online": True, "placement": {"node": "own-node", "why": "only one"},
                                        "source": {"checkout": "workspace", "commit": None}, "userns": True, "ranking": []})
            if body.get("node") == "shared-node":
                return self.reply(403, {"error": {"code": "userns_owner_only", "node": "shared-node", "hint": OWNER_ONLY}})
            if body.get("userns"):
                return self.reply(403, {"error": {"code": "userns_not_granted", "node": "own-node",
                                                  "hint": "--userns: workspace builder isn't granted user namespaces on 'own-node' (by default a job can't make them). The node's owner or an Admin grants it: k2 compute grant own-node --workspace builder --userns"}})
            return self.reply(404, {"error": {"code": "no_node_fits", "hint": "none"}})
        if u.path == "/cli/compute/grants/set":
            if body.get("node") == "shared-node" and body.get("allowUserns"):
                return self.reply(403, {"error": {"code": "userns_owner_only", "node": "shared-node", "hint": OWNER_ONLY}})
            g = {"maxJobSecs": 7200, "maxParallel": 1, "maxQueued": 10, "maxDiskGb": 60, "allowCi": False, "allowUserns": bool(body.get("allowUserns")), "allowNoAddrFilter": bool(body.get("allowNoAddrFilter"))}
            return self.reply(200, {"ok": True, "node": body.get("node"), "workspace": body.get("workspace"), "grant": g, "note": "ok", "resent": 0})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

last_key() { tail -1 "$LOG" | python3 -c 'import json,sys; v=json.load(sys.stdin); x=v["body"].get(sys.argv[1]); print(json.dumps(x))' "$1"; }

echo "== run --userns: the body and the refusals =="
set +e
err="$("$K2" compute run --no-source --workspace builder --userns -- true 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "--userns sends userns true" "$(last_key userns)" "true"
expect_eq "userns_not_granted exits 3" "$rc" "3"
expect_has "userns_not_granted code" "$err" "userns_not_granted"
expect_has "the hint names the owner's grant" "$err" "k2 compute grant own-node --workspace builder --userns"
"$K2" compute run --no-source --workspace builder -- true >/dev/null 2>&1 || true
expect_eq "no --userns: no userns key" "$(last_key userns)" "null"
set +e
err="$("$K2" compute run shared-node --no-source --workspace builder --userns -- true 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "a tenant node: userns_owner_only exits 3" "$rc" "3"
expect_has "userns_owner_only code" "$err" "userns_owner_only"
expect_has "it names the reason" "$err" "this server is a tenant of shared-node, so its jobs can't make user namespaces there"
"$K2" compute test --no-source --workspace builder --userns -- -p x >/dev/null 2>&1 || true
expect_eq "compute test passes --userns through" "$(last_key userns)" "true"

echo "== --no-addr-filter =="
"$K2" compute run --no-source --workspace builder --userns --network none --no-addr-filter -- true >/dev/null 2>&1 || true
expect_eq "--no-addr-filter sends noAddrFilter true" "$(last_key noAddrFilter)" "true"
expect_eq "... beside userns" "$(last_key userns)" "true"
"$K2" compute run --no-source --workspace builder --userns --network none -- true >/dev/null 2>&1 || true
expect_eq "no --no-addr-filter: no key" "$(last_key noAddrFilter)" "null"
"$K2" compute test --no-source --workspace builder --userns --network none --no-addr-filter -- -p x >/dev/null 2>&1 || true
expect_eq "compute test passes --no-addr-filter through" "$(last_key noAddrFilter)" "true"
out="$("$K2" compute grant own-node --workspace builder --userns --no-addr-filter)"
expect_eq "grant --no-addr-filter sends allowNoAddrFilter true" "$(last_key allowNoAddrFilter)" "true"
expect_has "the answer says so" "$out" "--userns jobs allowed, --no-addr-filter allowed"
"$K2" compute grant own-node --workspace builder --addr-filter >/dev/null
expect_eq "grant --addr-filter sends allowNoAddrFilter false" "$(last_key allowNoAddrFilter)" "false"
expect_eq "... and leaves --userns alone" "$(last_key allowUserns)" "null"
out="$("$K2" compute get abcd1234 2>&1)"
expect_has "get: address filter line" "$out" "address filter: off (--no-addr-filter, granted by the node's owner)"
expect_has "help: run --no-addr-filter" "$("$K2" compute --help)" "[--no-addr-filter]  with --userns and --network none only"
expect_has "study: no_addr_filter_invalid" "$("$K2" study compute)" "no_addr_filter_invalid"

echo "== --plan shows it =="
plan="$("$K2" compute run --no-source --workspace builder --userns --plan -- true 2>&1 || true)"
expect_has "--plan: userns line" "$plan" "userns: allowed (--userns, granted by the node's owner)"

echo "== grant --userns / --no-userns =="
out="$("$K2" compute grant own-node --workspace builder --userns)"
expect_eq "grant --userns sends allowUserns true" "$(last_key allowUserns)" "true"
expect_has "the answer says so" "$out" "--userns jobs allowed"
out="$("$K2" compute grant own-node --workspace builder --no-userns)"
expect_eq "grant --no-userns sends allowUserns false" "$(last_key allowUserns)" "false"
expect_not "no --userns in the answer" "$out" "--userns jobs allowed"
"$K2" compute grant own-node --workspace builder --max-parallel 2 >/dev/null
expect_eq "re-granting limits sends no allowUserns (keeps it)" "$(last_key allowUserns)" "null"
set +e
err="$("$K2" compute grant shared-node --workspace builder --userns 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "grant --userns on a tenant node exits 3" "$rc" "3"
expect_has "... userns_owner_only" "$err" "userns_owner_only"

echo "== get and grants show it =="
out="$("$K2" compute get abcd1234 2>&1)"
expect_has "get: userns line" "$out" "userns:   allowed (--userns, granted by the node's owner)"
out="$("$K2" compute get ffff0000 2>&1)"
expect_not "get: no line without the ask" "$out" "userns:"
out="$("$K2" compute grants)"
expect_has "grants: the userns grant" "$out" "60 GB disk  userns  by owner"
expect_has "grants: a plain grant unchanged" "$out" "60 GB disk  by owner"

echo "== help, study and schema =="
help="$("$K2" compute --help)"
expect_has "help: run --userns" "$help" "[--userns]     by default a job can't create user namespaces"
expect_has "help: only this node's own server's workspaces" "$help" "Only this node's own"
expect_has "help: grant --userns" "$help" "[--userns|--no-userns]"
study="$("$K2" study compute)"
expect_has "study: section" "$study" "USER NAMESPACES (--userns; k2-node 0.46.18 on the node)"
for c in userns_not_granted userns_owner_only userns_needs_update "unshare -U" "--workspace <w> --userns"; do
    expect_has "study: $c" "$study" "$c"
done
schema="$("$K2" --schema)"
expect_has "schema: run --userns" "$schema" '"name": "--userns"'
expect_has "schema: grant --no-userns" "$schema" '"name": "--no-userns"'

echo
echo "compute userns CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
