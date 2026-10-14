#!/usr/bin/env bash
# K2 projects — the per-agent "Manage K2 projects" switch (0162, 0.45.6).
#
#   1. Catalog: `project` is an `id` tool, so inside a K2 terminal the CLI
#      never re-sources the owner token.
#   2. A fake cell (K2_HOOK_SOCK + K2_HOOK_TOKEN set, owner token on disk)
#      against a stub daemon: every `k2 project` request (reads and the
#      add-member mutation) carries the SCOPED token, never the owner
#      token; a daemon `agent_permission_off` refusal exits 1 with the JSON
#      error on stderr; `k2 agent set --project` sends the scoped token on
#      the membership step too.
#   3. Outside K2 (no hook socket, owner token on disk): `k2 project access
#      off --workspace W` posts agent-access/set {toggle: projects_manage,
#      value: false} with the owner token; usage errors exit 2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-project-access-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

# Never inherit a session's identity from the caller.
unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN || true

# ── 1. catalog ───────────────────────────────────────────────────────
echo "== catalog =="
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "project tool id" "$(_cli_tool_id_for_verb project)" "project"
assert_eq "project is an id tool" "$(_cli_tool_default_mode project)" "id"
SCOPED_TOKEN="scoped-x" TOKEN="owner-x" K2_HOOK_SOCK="/nope.sock"
assert_eq "membership token in a cell is the passport" "$(_k2_project_token)" "scoped-x"
unset K2_HOOK_SOCK
assert_eq "membership token outside a cell is \$TOKEN" "$(_k2_project_token)" "owner-x"
unset SCOPED_TOKEN TOKEN

# ── 2. fake cell → stub daemon ───────────────────────────────────────
OWNER="owner-disk-token-projects"
SCOPED="sess77.scoped-secret-projects"
HOME2="$WORK/home"
mkdir -p "$HOME2/.k2" "$WORK/proj"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
GROUP = {"ok": True, "id": "g-1", "name": "Atlas", "pocWorkspaceId": "w-1",
         "members": [{"workspaceId": "w-1", "name": "scout", "path": "/w/scout"}]}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {"method": method, "path": parsed.path,
               "token": (q.get("token") or [""])[0],
               "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        b = json.loads(body) if body else {}
        status, out = 200, {"ok": True}
        if parsed.path == "/cli/project-group/show":
            out = GROUP
        elif parsed.path == "/cli/project-group/add-member":
            if b.get("workspace") == "denied":
                status = 403
                out = {"ok": False, "error": {"code": "agent_permission_off",
                       "hint": "'Manage K2 projects' is off for scout"}}
            else:
                out = {"ok": True, "groupId": "g-1", "workspaceId": "w-1",
                       "alreadyMember": False, "becamePoc": False}
        elif parsed.path == "/cli/agent-access/set":
            out = {"ok": True, "workspace": b.get("workspace"), "toggle": b.get("toggle"),
                   "agentsCanManageProjects": b.get("value")}
        elif parsed.path == "/cli/agent/conf":
            out = {"ok": True, "path": "/w/scout", "name": "scout", "mode": "custom",
                   "enabled": True, "projects": []}
        elif parsed.path == "/cli/project-group/list":
            out = {"ok": True, "groups": [{"id": "g-1", "name": "Atlas", "memberCount": 1}]}
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

echo "== fake cell: k2 project sends the passport =="
reset
set +e
out="$(cell project add Atlas scout 2>"$WORK/err")"; rc=$?
set -e
assert_eq "add exit 0" "$rc" "0"
assert_contains "add output" "$out" "Added scout to Atlas"
assert_contains "add-member was posted" "$(reqs)" '"path": "/cli/project-group/add-member"'
python3 - "$WORK/reqs.jsonl" "$SCOPED" <<'PYEOF' && ok "every project request carries the scoped token" || bad "every project request carries the scoped token ($(reqs))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
assert rows, "no requests"
proj = [r for r in rows if r["path"].startswith("/cli/project-group/")]
assert proj, rows
assert all(r["token"] == sys.argv[2] for r in proj), proj
PYEOF
assert_absent "never the owner token" "$(reqs)" "$OWNER"

reset
set +e
out="$(cell project add Atlas denied 2>"$WORK/err")"; rc=$?
set -e
assert_eq "agent_permission_off exits 1" "$rc" "1"
assert_contains "stderr carries the code" "$(cat "$WORK/err")" '"code":"agent_permission_off"'
assert_absent "refused: never the owner token" "$(reqs)" "$OWNER"

reset
cell project show Atlas --json >/dev/null
assert_contains "show reads with the passport" "$(reqs)" "\"token\": \"$SCOPED\""
assert_absent "show: never the owner token" "$(reqs)" "$OWNER"

echo "== fake cell: k2 agent set --project rides the passport for membership =="
reset
set +e
cell agent set scout --add-project Atlas --json >/dev/null 2>"$WORK/err"
set -e
python3 - "$WORK/reqs.jsonl" "$SCOPED" <<'PYEOF' && ok "agent set membership step uses the passport" || bad "agent set membership step uses the passport ($(reqs); $(cat "$WORK/err"))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
adds = [r for r in rows if r["path"] == "/cli/project-group/add-member"]
assert adds, rows
assert all(r["token"] == sys.argv[2] for r in adds), adds
PYEOF

echo "== outside K2: k2 project access =="
reset
out="$(outside project access off --workspace scout)"
assert_contains "access off output" "$out" "Manage K2 projects is OFF for scout"
python3 - "$WORK/reqs.jsonl" "$OWNER" <<'PYEOF' && ok "access posts projects_manage=false with the owner token" || bad "access request ($(reqs))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
assert len(rows) == 1, rows
r = rows[0]
assert r["method"] == "POST" and r["path"] == "/cli/agent-access/set", r
assert r["token"] == sys.argv[2], r
assert json.loads(r["body"]) == {"workspace": "scout", "toggle": "projects_manage", "value": False}, r
PYEOF
reset
json_out="$(outside project access on --workspace scout --json)"
python3 -c 'import json,sys; v=json.loads(sys.argv[1]); assert v=={"ok":True,"workspace":"scout","agentsCanManageProjects":True}, v' "$json_out" \
    && ok "access on --json" || bad "access on --json ($json_out)"

set +e
outside project access >/dev/null 2>&1; rc=$?
set -e
assert_eq "access without on|off exits 2" "$rc" "2"
set +e
outside project access maybe >/dev/null 2>&1; rc=$?
set -e
assert_eq "access with a bad state exits 2" "$rc" "2"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
