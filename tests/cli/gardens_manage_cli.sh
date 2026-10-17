#!/usr/bin/env bash
# Agents run Gardens end to end (0.46.0, migration 0181): the CLI shapes of
# "Manage Gardens and data sources" against a tiny stub daemon on loopback.
# No real daemon, no vault, no keychain; HOME is a throwaway dir.
#
#   1. A fake cell (K2_HOOK_SOCK + K2_HOOK_TOKEN set, owner token on disk):
#      `k2 sources approve a b c --secret NAME` sends ONE batch-approve with
#      the secret on every item and no confirm token, with the passport;
#      `k2 sources secret` and `pause` send no confirm either; a daemon
#      person_only / secret_not_usable refusal exits 3; `k2 zen widget put`
#      and `garden put` carry the passport; a proposal answer prints
#      "Proposed"; `k2 zen proposals approve` posts with the passport.
#   2. Outside K2: `k2 sources access on|off --workspace W` posts
#      agent-access/set {toggle: gardens_manage} with the owner token; a 403
#      exits 3; usage errors exit 2.
#   3. Help and study pages say what agents may do with and without it.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-gardens-manage-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-gardens"
SCOPED="sess81.scoped-secret-gardens"
HOME2="$WORK/home"
mkdir -p "$HOME2/.k2" "$WORK/proj"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
H64 = "7" * 64
PENDING = {"sources": [
    {"id": "id-%s" % n, "name": n, "specHash": H64, "secret": None, "urlSecret": None,
     "approval": "one_click", "everySecs": 60, "targets": 1, "origins": ["https://example.com:443"], "madeBy": "agent:w-1"}
    for n in ("a-src", "b-src", "c-src", "denied-src")], "triggers": []}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        tok = (q.get("token") or [""])[0]
        auth = self.headers.get("Authorization") or ""
        if not tok and auth.startswith("Bearer "):
            tok = auth[7:]
        rec = {"method": method, "path": parsed.path, "token": tok, "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        b = json.loads(body) if body else {}
        status, out = 200, {"ok": True}
        p = parsed.path
        if p == "/cli/sources/pending":
            out = PENDING
        elif p == "/cli/sources/batch-approve":
            names = [i["name"] for i in b.get("items", [])]
            if "denied-src" in names:
                status, out = 403, {"error": {"code": "secret_not_usable", "hint": "server secret without vault_manage"}}
            else:
                out = {"approved": names, "triggersApproved": []}
        elif p == "/cli/sources/approve":
            if b.get("name") == "denied-src":
                status, out = 403, {"error": {"code": "person_only", "hint": "needs a person"}}
            else:
                out = {"name": b.get("name"), "status": "active", "secretBound": bool(b.get("secret"))}
        elif p == "/cli/sources/secret":
            out = {"name": b.get("name"), "secret": b.get("secret"), "secretBound": True}
        elif p == "/cli/sources/pause":
            out = {"name": b.get("name"), "status": "paused"}
        elif p == "/cli/agent-access/set":
            if b.get("workspace") == "refused":
                status, out = 403, {"error": {"code": "forbidden", "hint": "only the owner or an admin may change agent access"}}
            else:
                out = {"ok": True, "workspace": b.get("workspace"), "toggle": b.get("toggle"), "agentsCanManageGardens": b.get("value")}
        elif p == "/cli/zen/widget/put":
            out = {"ok": True, "proposed": True, "proposal": {"id": "zp-123", "kind": "widget", "target": b.get("widget")},
                   "message": "Proposed zp-123. A person approves it."}
        elif p == "/cli/zen/garden/put":
            out = {"ok": True, "garden": b.get("garden"), "file": "gardens/%s.toml" % b.get("garden"), "changed": True, "snapshot": "s1.toml"}
        elif p == "/cli/zen/proposals/approve":
            out = {"ok": True, "id": b.get("id"), "status": "approved"}
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
STUB_PORT="$(cat "$WORK/stub.port")"
[ -n "$STUB_PORT" ] || { echo "FAIL: stub daemon did not start" >&2; exit 1; }

cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
outside() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" \
        K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
reqs() { cat "$WORK/reqs.jsonl" 2>/dev/null || true; }
reset() { rm -f "$WORK/reqs.jsonl"; }

echo "== fake cell: one --secret for many sources, no confirm =="
reset
set +e
out="$(cell sources approve a-src b-src c-src --secret WS_KEY 2>"$WORK/err")"; rc=$?
set -e
assert_eq "multi approve exit 0" "$rc" "0"
assert_contains "multi approve output" "$out" "approved: a-src, b-src, c-src"
python3 - "$WORK/reqs.jsonl" "$SCOPED" <<'PYEOF' && ok "ONE batch-approve, secret on every item, no confirm, passport" || bad "batch request ($(reqs); $(cat "$WORK/err"))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
batch = [r for r in rows if r["path"] == "/cli/sources/batch-approve"]
assert len(batch) == 1, rows
b = json.loads(batch[0]["body"])
assert [i["name"] for i in b["items"]] == ["a-src", "b-src", "c-src"], b
assert all(i.get("secret") == "WS_KEY" for i in b["items"]), b
assert "confirmToken" not in b, b
assert all(r["token"] == sys.argv[2] for r in rows), rows
PYEOF
assert_absent "never the owner token" "$(reqs)" "$OWNER"

reset
set +e
cell sources approve a-src denied-src --secret SERVER_KEY >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "secret_not_usable exits 3" "$rc" "3"
assert_contains "secret_not_usable says why" "$(cat "$WORK/err")" "secret_not_usable"

reset
set +e
cell sources approve denied-src >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "person_only exits 3" "$rc" "3"
assert_contains "person_only names the switch" "$(cat "$WORK/err")" "Manage Gardens and data sources"

reset
out="$(cell sources secret a-src WS_KEY)"
assert_contains "secret output" "$out" "a-src: uses WS_KEY"
python3 - "$WORK/reqs.jsonl" <<'PYEOF' && ok "secret: one POST, no show lookup, no confirm" || bad "secret request ($(reqs))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"].startswith("/cli/sources/")]
assert [r["path"] for r in rows] == ["/cli/sources/secret"], rows
assert json.loads(rows[0]["body"]) == {"name": "a-src", "secret": "WS_KEY"}, rows
PYEOF

reset
out="$(cell sources pause a-src)"
assert_contains "pause output" "$out" "a-src: paused"
assert_contains "pause with the passport" "$(reqs)" "\"token\": \"$SCOPED\""

echo "== fake cell: k2 zen put and proposals =="
printf '<p>board</p>' > "$WORK/index.html"
reset
out="$(cell zen widget put board index.html --from "$WORK/index.html" --note "first cut")"
assert_contains "widget put proposes" "$out" "Proposed zp-123"
python3 - "$WORK/reqs.jsonl" "$SCOPED" <<'PYEOF' && ok "widget put carries the passport and the file" || bad "widget put ($(reqs))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
assert len(rows) == 1 and rows[0]["path"] == "/cli/zen/widget/put", rows
assert rows[0]["token"] == sys.argv[2], rows
assert json.loads(rows[0]["body"]) == {"widget": "board", "file": "index.html", "text": "<p>board</p>", "note": "first cut"}, rows
PYEOF
printf 'schema = 1\n' > "$WORK/page.toml"
reset
out="$(cell zen garden put g-1 --from "$WORK/page.toml")"
assert_contains "garden put wrote" "$out" "Wrote gardens/g-1.toml"
assert_contains "garden put keeps the old page" "$out" "k2 zen reset --to s1.toml"
assert_contains "garden put with the passport" "$(reqs)" "\"token\": \"$SCOPED\""
reset
out="$(cell zen proposals approve zp-123)"
assert_contains "approve output" "$out" "zp-123: approved"
assert_contains "approve with the passport" "$(reqs)" "\"token\": \"$SCOPED\""
set +e
cell zen widget put board >/dev/null 2>&1; rc=$?
set -e
assert_eq "widget put without --from exits 2" "$rc" "2"

echo "== outside K2: k2 sources access =="
reset
out="$(outside sources access on --workspace scout)"
assert_contains "access on output" "$out" "Manage Gardens and data sources is ON for scout"
python3 - "$WORK/reqs.jsonl" "$OWNER" <<'PYEOF' && ok "access posts gardens_manage=true with the owner token" || bad "access request ($(reqs))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
assert len(rows) == 1, rows
r = rows[0]
assert r["method"] == "POST" and r["path"] == "/cli/agent-access/set", r
assert r["token"] == sys.argv[2], r
assert json.loads(r["body"]) == {"workspace": "scout", "toggle": "gardens_manage", "value": True}, r
PYEOF
out="$(outside sources access off --workspace scout)"
assert_contains "access off output" "$out" "is OFF for scout"
set +e
outside sources access on --workspace refused >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "a refused flip exits 3" "$rc" "3"
assert_contains "refusal says an agent never can" "$(cat "$WORK/err")" "An agent never can"
set +e
outside sources access >/dev/null 2>&1; rc=$?
set -e
assert_eq "access without on|off exits 2" "$rc" "2"

echo "== help and study =="
help="$(outside sources --help)"
assert_contains "sources help names the switch" "$help" "MANAGE GARDENS AND DATA SOURCES"
assert_contains "sources help: one confirm for many" "$help" "ONE confirm"
zh="$(outside zen --help)"
assert_contains "zen help: put" "$zh" "k2 zen garden put <garden> --from"
assert_contains "zen help: proposals" "$zh" "k2 zen proposals"
st="$(outside study data-sources)"
assert_contains "study data-sources: the switch" "$st" "THE AGENT SWITCH"
assert_contains "study data-sources: without it" "$st" "Without the switch below"
sz="$(outside study zen)"
assert_contains "study zen: propose" "$sz" "WRITE THROUGH K2, OR PROPOSE"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
