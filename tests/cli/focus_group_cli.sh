#!/usr/bin/env bash
# `k2 focus-group list|create|rename|add|remove`, `k2 agent set
# --focus-group` and `k2 workspace list --json`'s focusGroup, against a stub
# daemon that records every request (route, JSON body, credential).
#
#   1. each verb sends exactly the route and body Settings → Workspaces
#      sends (GET list; POST create {name}, update {id, name}, assign
#      {projectId, focusGroupId}, null = no group);
#   2. a person (the owner token, outside K2) changes groups; an agent
#      (its passport, inside K2) lists them, and its change is refused:
#      exit 3, the code, and the "Ask with:" line the daemon's permission
#      object names, printed once;
#   3. workspace list --json passes each row's focusGroup through;
#   4. -h or --help anywhere on every form prints the focus-group help (or
#      agent set's) and sends nothing; after `--` a word is a name;
#   5. usage errors exit 2 and send nothing, a value flag as the last word
#      too; an unknown or ambiguous group exits 4.
#
# Hermetic: HOME is a temp dir, every K2_*/K5_* variable is unset, the
# daemon is a python stub on a free loopback port. Loud.
# Run with: bash tests/cli/focus_group_cli.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
TEXT="$SCRIPT_DIR/../../cli/text/help"

for v in $(env | grep -oE '^K[25](SO)?_[A-Z0-9_]+' || true); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
check_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
check_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

WORK="$(mktemp -d -t k2-focus-group-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-token-fg-test"
PASSPORT="passport-fg-test"
mkdir -p "$WORK/home/.k2" "$WORK/proj"
printf '%s\n' "$OWNER" >"$WORK/home/.k2/heartbeat.token"
chmod 600 "$WORK/home/.k2/heartbeat.token"
REQ="$WORK/reqs.jsonl"
: >"$REQ"

python3 - "$WORK" "$PASSPORT" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, passport = sys.argv[1], sys.argv[2]
GROUPS = [
    {"id": "g-infra", "name": "Infra", "color": None, "tabOrder": 0, "createdAt": 1},
    {"id": "g-ca", "name": "Client A", "color": None, "tabOrder": 1, "createdAt": 2},
    {"id": "g-cb", "name": "Client B", "color": None, "tabOrder": 2, "createdAt": 3},
]

def conf(q):
    name = os.path.basename(q.rstrip("/")) or q
    if name == "nope":
        return 404, {"error": {"code": "not_found", "hint": "no workspace matches 'nope'"}}
    return 200, {"ok": True, "id": "w-" + name, "name": name, "path": "/tmp/" + name,
                 "focusGroupId": "g-infra" if name.startswith("inside") else None}

def refused(path, body):
    # What the daemon's walls map adds for an agent with cards on.
    if path.endswith("/create"):
        err = {"code": "focus_group_needs_person", "hint": "Creating focus group is a person's step.", "name": body.get("name")}
        cmd = "k2 permission request focus-groups.create name='%s' --why \"<what you need it for>\"" % body.get("name")
    elif path.endswith("/update"):
        err = {"code": "focus_group_needs_person", "hint": "Renaming is a person's step.", "group": "Infra", "to": body.get("name")}
        cmd = "k2 permission request focus-groups.rename group=Infra to=%s --why \"<what you need it for>\"" % body.get("name")
    else:
        group = {"g-infra": "Infra"}.get(body.get("focusGroupId"), "none")
        target = body.get("projectId", "")[2:]
        err = {"code": "focus_group_needs_person", "hint": "Filling a group is a person's step.", "target": target, "group": group}
        cmd = "k2 permission request focus-groups.assign group=%s target=%s --why \"<what you need it for>\"" % (group, target)
    return 403, {"error": err, "permission": {"action": cmd.split()[3], "command": cmd, "approvers": "owner or admin"}}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _go(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        u = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(u.query)
        auth = self.headers.get("Authorization") or ""
        tok = auth[7:] if auth.startswith("Bearer ") else (q.get("token") or [""])[0]
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps({"m": self.command, "p": u.path, "body": raw, "who": "agent" if tok == passport else "owner" if tok else "none", "q": (q.get("q") or [""])[0]}) + "\n")
        p = u.path
        status, out = 200, {"ok": True}
        if p == "/cli/focus-groups/list":
            out = GROUPS
        elif p == "/cli/agent/conf":
            status, out = conf((q.get("q") or [""])[0])
        elif p == "/cli/agent/list":
            rows = [{"name": "sales", "id": "w-sales", "handle": "sales", "path": "/tmp/sales", "focusGroup": "Infra", "enabled": True},
                    {"name": "ops", "id": "w-ops", "handle": "ops", "path": "/tmp/ops", "focusGroup": None, "enabled": True}]
            if os.path.exists(os.path.join(work, "big")):
                rows += [{"name": "ws%05d" % i, "id": "w-%05d" % i, "handle": "ws%05d" % i, "path": "/tmp/ws%05d" % i,
                          "focusGroup": "Infra" if i % 2 else None, "enabled": True} for i in range(4000)]
            out = {"ok": True, "agents": rows}
        elif p in ("/cli/focus-groups/create", "/cli/focus-groups/update", "/cli/focus-groups/assign"):
            body = json.loads(raw or "{}")
            if tok == passport:
                status, out = refused(p, body)
            elif p.endswith("/create"):
                out = {"id": "g-new", "name": body.get("name"), "color": None, "tabOrder": 3, "createdAt": 4}
            elif p.endswith("/update"):
                out = {"id": body.get("id"), "name": body.get("name"), "color": None, "tabOrder": 0, "createdAt": 1}
            else:
                out = {"id": body.get("projectId"), "focusGroupId": body.get("focusGroupId")}
        else:
            status, out = 404, {"error": "route not found: " + p}
        b = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    do_GET = _go
    do_POST = _go

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$WORK/port" ] && break; sleep 0.05; done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
PORT="$(cat "$WORK/port")"

# A person: the owner token from HOME, outside K2.
person() { env -i HOME="$WORK/home" PATH="$PATH" K2_HOST=127.0.0.1 K2_PORT="$PORT" "$K2_CLI" "$@"; }
# An agent: its passport, inside K2 (a session socket that isn't there, so
# the CLI goes over TCP with the passport).
agent() {
    env -i HOME="$WORK/home" PATH="$PATH" K2_HOST=127.0.0.1 K2_PORT="$PORT" \
        K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$WORK/no-such.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
# run <who> <args...>: OUT, ERR, RC.
run() {
    : >"$REQ"
    set +e
    OUT="$("$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}
# The requests the last run sent: "<who> <method> <path> <body>", one a
# line. An agent's CLI first checks its passport (GET /cli/whoami, the
# session's check before any id tool, help included): not the command's,
# so it is left out here.
sent() { python3 -c '
import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l)
    if r["p"] == "/cli/whoami":
        continue
    print(r["who"], r["m"], r["p"], r["body"])' "$REQ"; }
# Only the changes (POSTs).
changes() { sent | awk '$2 == "POST"'; }
nothing_sent() { if [ -n "$(sent)" ]; then bad "$1: sent: $(sent | tr '\n' '|')"; else ok "$1: sent nothing"; fi; }

echo "== list =="
# covers: focus-group
run person focus-group
check_eq "bare noun: exit 0" "$RC" 0
check_has "bare noun lists names" "$OUT" "Infra"
check_eq "bare noun: one GET of the list" "$(sent)" "owner GET /cli/focus-groups/list "
# covers: focus-group --json
run person focus-group list --json
check_eq "--json: the groups" "$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(d["ok"], [g["name"] for g in d["focusGroups"]])' "$OUT")" "True ['Infra', 'Client A', 'Client B']"
run agent focus-group list
check_eq "an agent lists: exit 0" "$RC" 0
check_has "an agent sees the groups" "$OUT" "Client B"
check_eq "an agent's list rides its passport" "$(sent)" "agent GET /cli/focus-groups/list "

echo "== a person's changes: the exact routes and bodies =="
run person focus-group create "Client work"
check_eq "create: exit 0" "$RC" 0
check_eq "create: POST {name}" "$(changes)" 'owner POST /cli/focus-groups/create {"name": "Client work"}'
check_has "create: says so" "$OUT" "Created focus group Client work."
run person focus-group rename infra Ops
check_eq "rename: exit 0" "$RC" 0
check_eq "rename: POST {id, name} with the group's id" "$(changes)" 'owner POST /cli/focus-groups/update {"id": "g-infra", "name": "Ops"}'
check_has "rename: says so" "$OUT" "Renamed focus group Infra to Ops."
run person focus-group add "client a" sales
check_eq "add: exit 0" "$RC" 0
check_eq "add: POST {projectId, focusGroupId}" "$(changes)" 'owner POST /cli/focus-groups/assign {"projectId": "w-sales", "focusGroupId": "g-ca"}'
check_has "add: says so" "$OUT" "sales is in focus group Client A now."
run person focus-group add Infra inside-ops
check_eq "add: already there sends no change" "$(changes)" ""
check_has "add: already there says so" "$OUT" "inside-ops is in focus group Infra already."
run person focus-group remove Infra inside-ops
check_eq "remove: exit 0" "$RC" 0
check_eq "remove: POST focusGroupId null" "$(changes)" 'owner POST /cli/focus-groups/assign {"projectId": "w-inside-ops", "focusGroupId": null}'
run person focus-group remove Infra sales
check_eq "remove from a group it isn't in: exit 0" "$RC" 0
check_eq "remove from a group it isn't in: no change" "$(changes)" ""
check_has "remove from a group it isn't in: says so" "$OUT" "sales is not in focus group Infra; nothing changed."
run person focus-group create Lab --json
check_eq "create --json: the daemon's answer" "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$OUT")" "g-new"

echo "== the other names and the --json forms =="
# native: focus-groups
run person focus-groups
check_eq "focus-groups: exit 0" "$RC" 0
check_eq "focus-groups: one GET of the list" "$(sent)" "owner GET /cli/focus-groups/list "
check_has "focus-groups lists" "$OUT" "Client A"
# native: focus-group ls
run person focus-group ls
check_eq "ls: exit 0" "$RC" 0
check_has "ls lists" "$OUT" "Infra"
# native: focus-group rm
run person focus-group rm Infra inside-ops
check_eq "rm: POST focusGroupId null" "$(changes)" 'owner POST /cli/focus-groups/assign {"projectId": "w-inside-ops", "focusGroupId": null}'
# native: focus-group add --json
run person focus-group add "client a" sales --json
check_eq "add --json: the daemon's answer" "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["focusGroupId"])' "$OUT")" "g-ca"
run person focus-group add Infra inside-ops --json
check_eq "add --json, already there: unchanged" "$OUT" '{"ok":true,"changed":false}'
check_eq "add --json, already there: no change" "$(changes)" ""
# native: focus-group remove --json
run person focus-group remove Infra sales --json
check_eq "remove --json from a group it isn't in: unchanged" "$OUT" '{"ok":true,"changed":false}'
# native: focus-group rename --json
run person focus-group rename Infra Ops --json
check_eq "rename --json: the daemon's answer" "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["name"])' "$OUT")" "Ops"
run person focus-group add Nope sales --json
check_eq "an unknown group --json: exit 4" "$RC" 4
check_eq "an unknown group --json: the error" "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["error"]["code"])' "$ERR")" "not_found"

echo "== agent set --focus-group =="
# covers: agent set
run person agent set sales --focus-group Infra
check_eq "agent set: exit 0" "$RC" 0
check_eq "agent set: the assign" "$(changes)" 'owner POST /cli/focus-groups/assign {"projectId": "w-sales", "focusGroupId": "g-infra"}'
check_has "agent set: the step" "$OUT" "focus-group: Infra"
run person agent set inside-ops --focus-group none
check_eq "agent set none: null" "$(changes)" 'owner POST /cli/focus-groups/assign {"projectId": "w-inside-ops", "focusGroupId": null}'
run person agent set inside-ops --focus-group infra
check_eq "agent set: already there sends no change" "$(changes)" ""
check_has "agent set: skipped" "$OUT" "already Infra"
run person agent set sales --focus-group Infra --dry-run
check_eq "agent set --dry-run: no change" "$(changes)" ""
# covers: agent set --json
run person agent set sales --focus-group "client a" --json
check_eq "agent set --json: exit 0" "$RC" 0
check_eq "agent set --json: the step, done" "$(python3 -c 'import json,sys; a=json.loads(sys.argv[1])["actions"][0]; print(a["step"], a["group"], a["focusGroupId"], a["status"])' "$OUT")" "set-focus-group Client A g-ca done"

echo "== an agent's change is refused with the card to ask with =="
run agent focus-group create "Client work"
check_eq "create as an agent: exit 3" "$RC" 3
check_eq "create as an agent: one request, its passport" "$(changes)" 'agent POST /cli/focus-groups/create {"name": "Client work"}'
check_has "create as an agent: the code" "$ERR" "focus_group_needs_person"
check_has "create as an agent: the Ask line" "$ERR" "Ask with: k2 permission request focus-groups.create name='Client work'"
check_eq "the Ask line once" "$(grep -c '^Ask with: ' <<<"$ERR")" 1
run agent focus-group rename Infra Ops
check_eq "rename as an agent: exit 3" "$RC" 3
check_has "rename as an agent: the Ask line" "$ERR" "Ask with: k2 permission request focus-groups.rename group=Infra to=Ops"
run agent focus-group add Infra sales
check_eq "add as an agent: exit 3" "$RC" 3
check_has "add as an agent: the Ask line" "$ERR" "Ask with: k2 permission request focus-groups.assign group=Infra target=sales"
run agent focus-group remove Infra inside-ops
check_eq "remove as an agent: exit 3" "$RC" 3
check_has "remove as an agent: the Ask line" "$ERR" "Ask with: k2 permission request focus-groups.assign group=none target=inside-ops"
run agent agent set sales --focus-group Infra
check_eq "agent set as an agent: exit 3" "$RC" 3
check_eq "agent set as an agent: the assign rides its passport" "$(changes)" 'agent POST /cli/focus-groups/assign {"projectId": "w-sales", "focusGroupId": "g-infra"}'
check_has "agent set as an agent: the code" "$ERR" "focus_group_needs_person"
check_has "agent set as an agent: the Ask line" "$ERR" "Ask with: k2 permission request focus-groups.assign group=Infra target=sales"

echo "== workspace list --json =="
# covers: workspace
run person workspace list --json
check_eq "workspace list --json: exit 0" "$RC" 0
check_eq "each row names its group" "$(python3 -c 'import json,sys; print([(r["name"], r["focusGroup"]) for r in json.loads(sys.argv[1])["workspaces"]])' "$OUT")" "[('sales', 'Infra'), ('ops', None)]"
: >"$WORK/big"
run person workspace list --json
check_eq "a large list: exit 0" "$RC" 0
check_eq "a large list: every row, with its group" "$(python3 -c 'import json,sys; r=json.loads(sys.stdin.read())["workspaces"]; print(len(r), sum(1 for x in r if x["focusGroup"] == "Infra"))' <<<"$OUT")" "4002 2001"
[ "${#OUT}" -gt 262144 ] && ok "a large list is over 256 KiB" || bad "the large list is only ${#OUT} bytes"
rm -f "$WORK/big"

echo "== -h or --help anywhere prints help and sends nothing =="
REF="$(cli_page "$TEXT/focus-group.txt")"
SETREF="$(cli_page "$TEXT/agent/set.txt")"
for form in "focus-group -h" "focus-group --help" "focus-group list -h" "focus-group create X -h" "focus-group create -h X" \
    "focus-group rename Infra Ops --help" "focus-group add Infra sales -h" "focus-group remove Infra sales --help" \
    "focus-group bogus -h"; do
    for who in person agent; do
        # shellcheck disable=SC2086
        run "$who" $form
        check_eq "$who k2 $form: exit 0" "$RC" 0
        check_eq "$who k2 $form: the focus-group help" "$OUT" "$REF"
        check_eq "$who k2 $form: nothing on stderr" "$ERR" ""
        nothing_sent "$who k2 $form"
    done
done
for form in "agent set sales --focus-group Infra -h" "agent set sales -h --focus-group Infra" "agent set sales --focus-group --help"; do
    # shellcheck disable=SC2086
    run person $form
    check_eq "k2 $form: exit 0" "$RC" 0
    check_eq "k2 $form: agent set's help" "$OUT" "$SETREF"
    nothing_sent "k2 $form"
done
run person focus-group create -- -h
check_eq "after --, -h is a name: exit 0" "$RC" 0
[ "$RC" = 0 ] || echo "      stderr: $ERR" >&2
check_eq "after --, -h is a name: created" "$(changes)" 'owner POST /cli/focus-groups/create {"name": "-h"}'

echo "== usage errors and not found =="
for form in "focus-group create" "focus-group create a b" "focus-group rename Infra" "focus-group add Infra" \
    "focus-group remove Infra" "focus-group list extra" "focus-group bogus" "focus-group create X --bogus" \
    "agent set sales --focus-group"; do
    # shellcheck disable=SC2086
    run person $form
    check_eq "k2 $form: exit 2" "$RC" 2
    nothing_sent "k2 $form"
done
run person focus-group create "  "
check_eq "an empty name: exit 2" "$RC" 2
nothing_sent "an empty name"
run person focus-group add Nope sales
check_eq "an unknown group: exit 4" "$RC" 4
check_has "an unknown group: says so" "$ERR" "no focus group matches 'Nope'"
check_eq "an unknown group: no change" "$(changes)" ""
run person focus-group add Client sales
check_eq "an ambiguous group: exit 4" "$RC" 4
check_has "an ambiguous group: names both" "$ERR" "Client A, Client B"
run person focus-group add Infra nope
check_eq "an unknown workspace: exit 4" "$RC" 4
check_eq "an unknown workspace: no change" "$(changes)" ""
run person agent set sales --focus-group Nope
check_eq "agent set, an unknown group: exit 4" "$RC" 4
check_eq "agent set, an unknown group: no change" "$(changes)" ""

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
