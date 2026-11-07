#!/usr/bin/env bash
# k2 compute roster|lanes|seats|trust|owners and k2 ci signer (0.46.10,
# prd-ci-on-compute-for-clients-v1 C1 + C3a): the thin CLI over the
# compute_trust routes. A python stub daemon on loopback records every
# request and answers like the daemon; the daemon side (the roster key, the
# person gates, the two-server seat flow, signer pairing) is
# crates/k2-daemon/src/compute_trust/tests.rs.
#
# Covers: the help an agent reads with no daemon, usage errors (exit 2,
# nothing sent), each filing verb's request body (as an agent passport, the
# way an agent files), and the person-only verbs refusing inside a K2
# session (exit 3, nothing sent: no confirm prompt, no request).
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3"))"; fi; }
eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }

WORK="$(mktemp -d -t k2-trust-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

echo "== no daemon: help =="
help="$("$K2" compute --help)"
has "help: roster set" "$help" "k2 compute roster set <node> [--host H]"
has "help: attest" "$help" "--attest \"what you checked\""
has "help: seats request" "$help" "k2 compute seats request <owner-host>"
has "help: seats offer" "$help" "k2 compute seats offer <request> --nodes a,b,c"
has "help: a person decides" "$help" "a person (owner or Admin, from a"
has "help: consent" "$help" "k2 compute owners consent <owner> on|off"
cihelp="$("$K2" ci --help)"
has "ci help: signer add" "$cihelp" "k2 ci signer add <CODE.FP> --hello pair.json"
has "ci help: SAS here only" "$cihelp" "SAS here only"

# ── the stub daemon ──────────────────────────────────────────────────
REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"
python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def record(self, body):
        u = urllib.parse.urlparse(self.path)
        with open(work + "/requests.log", "a") as f:
            f.write(json.dumps({"method": self.command, "path": u.path, "query": u.query, "body": body}) + "\n")
        return u.path
    def do_GET(self):
        n = int(self.headers.get("Content-Length") or 0)
        path = self.record(self.rfile.read(n).decode() if n else "")
        if path == "/cli/compute/roster":
            return self.reply({"latest": {"seq": 3, "keyFingerprint": "a" * 64, "issuedAt": 1, "entries": [
                {"name": "n1", "status": "active", "taintBasis": "owner_attested", "host": "h1", "nodeFp": "b" * 64}]},
                "stale": False, "changed": False, "entries": [], "nodes": [], "pending": [{"id": "ctr_1", "summary": "list n2"}]})
        if path == "/cli/compute/seats":
            return self.reply({"offered": [], "held": [{"id": "client-ci", "ownerId": "owner.k2.dev", "lane": "ci-standard", "state": "active", "nodes": ["n1", "n2"], "note": None}]})
        if path == "/cli/compute/trust/requests":
            return self.reply({"requests": [{"id": "ctr_1", "state": "pending", "kind": "roster", "summary": "list n2"}], "canApprove": False})
        if path == "/cli/compute/lanes":
            return self.reply({"lanes": [{"name": "ci-standard", "summary": "CI tier on", "approvedBy": "owner"}]})
        if path == "/cli/compute/owners":
            return self.reply({"self": None, "trusted": [{"ownerId": "owner.k2.dev", "keyFp": "c" * 64, "secretsConsent": True, "latest": {"seq": 3}}]})
        self.reply({"error": {"code": "not_found", "hint": "GET " + path}}, 404)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        path = self.record(body)
        v = json.loads(body or "{}")
        if path == "/cli/compute/roster/propose" and v.get("node") == "tainted":
            return self.reply({"error": {"code": "node_key_tainted", "hint": "re-key it under the wall"}}, 409)
        if path in ("/cli/compute/roster/propose", "/cli/compute/lanes/set", "/cli/compute/seats/request", "/cli/compute/seats/offer"):
            return self.reply({"held": True, "request": {"id": "ctr_9", "summary": "x"}})
        if path in ("/cli/compute/owners/consent", "/cli/compute/roster/publish", "/cli/compute/trust/withdraw"):
            return self.reply({"ok": True})
        self.reply({"error": {"code": "not_found", "hint": "POST " + path}}, 404)
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/port")"
# An agent's passport (a K2 session): the filing verbs work, people's don't.
export K2_HOOK_TOKEN="agent-passport-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

last() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == sys.argv[2]]
if not rows:
    sys.exit("no request to " + sys.argv[2])
r = rows[-1]
print(r["body"] if r["method"] == "POST" else r["query"])' "$REQ_LOG" "$1"
}
field() { python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1]).get(sys.argv[2])))' "$1" "$2"; }

rc() {
    local label="$1" want="$2" got
    shift 2
    set +e
    "$K2" "$@" >"$WORK/out" 2>"$WORK/err"
    got=$?
    set -e
    eq "$label exits $want" "$got" "$want"
}

echo "== usage errors send nothing =="
before="$(wc -l <"$REQ_LOG")"
rc "roster bogus" 2 compute roster frob
rc "roster set without node" 2 compute roster set
rc "roster set bad flag" 2 compute roster set n1 --bogus x
rc "seats offer without nodes" 2 compute seats offer ctr_1
rc "seats request without owner" 2 compute seats request
rc "consent maybe" 2 compute owners consent owner.k2.dev maybe
rc "trust deny without id" 2 compute trust deny
rc "signer add without hello" 2 ci signer add ABCDE-FGHJK.0123456789abcdef
eq "nothing reached the daemon" "$(wc -l <"$REQ_LOG")" "$before"

echo "== a person's verbs refuse inside a K2 session (nothing sent) =="
before="$(wc -l <"$REQ_LOG")"
rc "trust approve in a session" 3 compute trust approve ctr_1
has "person_only" "$(cat "$WORK/err")" "person_only"
rc "roster key in a session" 3 compute roster key
rc "consent on in a session" 3 compute owners consent owner.k2.dev on
echo '{"hello":{}}' >"$WORK/pair.json"
rc "signer add in a session" 3 ci signer add ABCDE-FGHJK.0123456789abcdef --hello "$WORK/pair.json"
rc "signer reveal in a session" 3 ci signer reveal csp_1 --reveal "$WORK/pair.json"
eq "no request for any of them" "$(wc -l <"$REQ_LOG")" "$before"

echo "== filing verbs (an agent) =="
rc "roster set" 0 compute roster set n2 --host box-2 --provider vin --admin-reach fleet-ssh,k2web --attest "installed with the wall"
b="$(last /cli/compute/roster/propose)"
eq "op set" "$(field "$b" op)" '"set"'
eq "node" "$(field "$b" node)" '"n2"'
eq "host" "$(field "$b" host)" '"box-2"'
eq "attest" "$(field "$b" attest)" '"installed with the wall"'
eq "adminReach as given" "$(field "$b" adminReach)" '"fleet-ssh,k2web"'
rc "roster revoke" 0 compute roster revoke n1 --reason compromised
b="$(last /cli/compute/roster/propose)"
eq "op revoke" "$(field "$b" op)" '"revoke"'
eq "reason" "$(field "$b" reason)" '"compromised"'
rc "roster set tainted" 3 compute roster set tainted
has "tainted code" "$(cat "$WORK/err")" "node_key_tainted"
rc "roster publish --resign" 0 compute roster publish --resign
eq "resign true" "$(field "$(last /cli/compute/roster/publish)" resign)" "true"
rc "roster show" 0 compute roster
has "show: seq and key" "$(cat "$WORK/out")" "node roster seq 3 (key aaaaaaaaaaaaaaaa…)"
has "show: waiting" "$(cat "$WORK/out")" "waiting: ctr_1  list n2"
rc "lanes set" 0 compute lanes set ci-standard --from ci-standard max_ci_running=3 isolation=job_uid other_tenants_max_job_secs=1800
b="$(last /cli/compute/lanes/set)"
eq "lane name" "$(field "$b" name)" '"ci-standard"'
eq "from" "$(field "$b" from)" '"ci-standard"'
eq "set" "$(field "$b" set)" '{"max_ci_running": 3, "isolation": ["job_uid"], "other_tenants_max_job_secs": 1800}'
rc "seats request" 0 compute seats request owner.k2.dev --lane ci-standard --no-secrets --why "release builds"
b="$(last /cli/compute/seats/request)"
eq "owner" "$(field "$b" owner)" '"owner.k2.dev"'
eq "secrets off" "$(field "$b" secrets)" "false"
eq "why" "$(field "$b" why)" '"release builds"'
rc "seats offer" 0 compute seats offer ctr_7 --nodes n1,n2,n3
b="$(last /cli/compute/seats/offer)"
eq "request" "$(field "$b" request)" '"ctr_7"'
eq "nodes" "$(field "$b" nodes)" '"n1,n2,n3"'
rc "seats list" 0 compute seats
has "held seat row" "$(cat "$WORK/out")" "held    client-ci"
rc "consent off (an agent may narrow)" 0 compute owners consent owner.k2.dev off
eq "on false" "$(field "$(last /cli/compute/owners/consent)" on)" "false"
rc "owners" 0 compute owners
has "owners row" "$(cat "$WORK/out")" "owner.k2.dev: key cccccccc"
rc "trust list" 0 compute trust
has "trust row" "$(cat "$WORK/out")" "ctr_1  pending"

echo
echo "compute_trust_cli: $pass passed, $fail failed"
[ "$fail" = 0 ]
