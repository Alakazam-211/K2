#!/usr/bin/env bash
# K2 projects — `k2 project view` (prd-projects-threads-v1 §4.3, slice P2):
# print or set the view a project opens its agents in.
#
#   1. Help: `k2 project --help` lists `view`; `k2 project view --help`
#      prints the usage, the three views and the exit codes (no daemon).
#   2. Against a stub daemon: print (human and --json), set (human and
#      --json; the resolved id and the lowercased view are posted), a bad
#      view or split exits 2 BEFORE any request, unknown / ambiguous project
#      exits 4, a Member refusal exits 1 with code forbidden, an agent with
#      the switch off exits 1 and the error ends with the permission card's
#      "Ask with: …" line (or "It can't be requested."), and an older daemon
#      (no defaultView) is a clear error that never posts.
#   3. `--agent <name>` (0302): print one member's view (its own or the
#      project's), set it (the member's workspace id is posted), clear it
#      with `default`; a non-member exits 1 not_a_member without posting;
#      `default` without --agent exits 2 before any request.
#
# Hermetic: throwaway HOME, every K2_* session var unset, a python stub on
# loopback. No real daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if <<<"$2" grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-project-view-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

HOME2="$WORK/home"
mkdir -p "$HOME2/.k2" "$WORK/proj"
echo "owner-disk-token-view" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

# ── 1. help (no daemon) ──────────────────────────────────────────────
echo "== help =="
nodaemon() { env HOME="$HOME2" K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2_CLI" "$@"; }
pj_help="$(nodaemon project --help)"
assert_contains "project help lists view" "$pj_help" "view <project> [<view>]        Print or set the view the project's agents"
assert_contains "project help names the views" "$pj_help" "open in: terminal, thread or chatter"
assert_contains "project help names --agent" "$pj_help" "(--agent <name>: one agent's own view)"
assert_contains "project help: switch default" "$pj_help" "projects\" (on by default; Settings"
assert_contains "project help: switch sentence is one line" "$pj_help" "code forbidden. Each workspace has that per-agent switch, \"Let"
assert_contains "project help: view is admin" "$pj_help" "setting the view its agents open in (\`view\`) are for the owner, an admin,"
view_help="$(nodaemon project view --help)"
assert_contains "view help usage" "$view_help" "k2 project view <project> [terminal|thread|chatter] [--json]"
assert_contains "view help agent usage" "$view_help" "k2 project view <project> [terminal|thread|chatter|default] --agent <name> [--json]"
assert_contains "view help: default clears" "$view_help" "default      Only with --agent: clear the agent's own view"
assert_contains "view help: agent print example" "$view_help" "  ada in sales opens in: terminal (its own view)"
assert_contains "view help: agent follow example" "$view_help" "  bo in sales opens in: thread (the project's view)"
assert_contains "view help: agent set example" "$view_help" "  ada in sales now opens in: chatter"
assert_contains "view help: agent clear example" "$view_help" "  ada in sales now follows the project's view: thread"
assert_contains "view help default" "$view_help" "New
projects start on thread."
assert_contains "view help member" "$view_help" "A Member login gets exit 1 with code
forbidden"
assert_contains "view help no split" "$view_help" "Any other view (split, for one) is
  refused with exit 2, code usage, before the daemon is asked"
assert_contains "view help: switch falls back" "$view_help" "the agent then goes back to the project's default view."
assert_contains "view help: reading is open" "$view_help" "Anyone may read it; no permission is needed."
assert_contains "view help: switch default" "$view_help" "projects\" on (on by default)"
# The example lines in the help are the real output (checked below).
assert_contains "view help: print example" "$view_help" "  sales opens agents in: thread"
assert_contains "view help: set example" "$view_help" "  sales now opens agents in: terminal"
assert_contains "view help exit codes" "$view_help" "0 ok · 1 refused · 2 bad usage (an unknown view) · 4 project not found or ambiguous"
assert_eq "view -h is the same text" "$(nodaemon project view sales -h)" "$view_help"
schema_row="$(nodaemon --schema | python3 -c 'import json,sys; d=json.load(sys.stdin); print(json.dumps([c for c in d["commands"] if c["name"]=="project view"]))')"
assert_contains "schema lists project view" "$schema_row" '"usage": "k2 project view <project> [terminal|thread|chatter|default] [--agent <name>] [--json]"'
assert_contains "schema lists --agent" "$schema_row" '"name": "--agent", "arg": "name"'

# ── 2. stub daemon ───────────────────────────────────────────────────
python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
state = {"view": "thread", "own": {"w-ada": None, "w-bo": None}}
MEMBERS = [("w-ada", "ada", "/srv/agents/ada", "Ada"), ("w-bo", "bo", "/srv/agents/bo", None)]

def members_json():
    out = []
    for wid, name, path, agent_name in MEMBERS:
        own = state["own"][wid]
        out.append({"workspaceId": wid, "name": name, "path": path, "agentName": agent_name,
                    "createdAt": 1, "sessionAlive": False, "sessionId": None,
                    "defaultView": own or state["view"], "ownDefaultView": own})
    return out

def mode():
    try:
        return open(os.path.join(work, "mode")).read().strip()
    except OSError:
        return ""

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {"method": method, "path": parsed.path,
               "group": (q.get("group") or [""])[0],
               "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        b = json.loads(body) if body else {}
        m = mode()
        status, out = 200, {"ok": True}
        if parsed.path == "/cli/project-group/show":
            g = rec["group"]
            if g.lower() in ("sales", "g-1"):
                out = {"ok": True, "id": "g-1", "name": "sales", "pocWorkspaceId": None,
                       "members": members_json(), "defaultView": state["view"]}
                if m == "old":
                    del out["defaultView"]
                if m == "old-members":
                    for mm in out["members"]:
                        del mm["defaultView"]
                        del mm["ownDefaultView"]
            elif g == "s":
                status = 400
                out = {"ok": False, "error": {"code": "ambiguous_id",
                       "hint": "'s' matches several projects: sales, support"}}
            else:
                status = 404
                out = {"ok": False, "error": {"code": "not_found",
                       "hint": "no project named '%s'" % g}}
        elif parsed.path == "/cli/project-group/set-default-view":
            if m == "member":
                status = 403
                out = {"ok": False, "error": {"code": "forbidden",
                       "hint": "only the owner, an admin, or an agent with 'Manage K2 projects' can change the view a project opens its agents in (it changes it for everyone)"}}
            elif m == "agent-off":
                status = 403
                out = {"ok": False, "error": {"code": "agent_permission_off",
                       "hint": "'Manage K2 projects' is off for ada"},
                       "permission": {"requestable": True,
                                      "command": "k2 permission request projects.manage --why \"...\""}}
            elif m == "agent-cannot":
                status = 403
                out = {"ok": False, "error": {"code": "forbidden", "hint": "no"},
                       "permission": {"requestable": False, "where": "at the machine"}}
            elif b.get("view") not in ("terminal", "thread", "chatter"):
                status = 400
                out = {"ok": False, "error": {"code": "usage",
                       "hint": "view must be terminal, thread or chatter, got '%s'" % b.get("view")}}
            else:
                state["view"] = b["view"]
                out = {"ok": True, "id": "g-1", "name": "sales", "defaultView": state["view"]}
        elif parsed.path == "/cli/project-group/set-member-default-view":
            if m == "member":
                status = 403
                out = {"ok": False, "error": {"code": "forbidden",
                       "hint": "only the owner, an admin, or an agent with 'Manage K2 projects' can change the view an agent opens in on a project (it changes it for everyone)"}}
            elif "view" not in b or b["workspace"] not in state["own"]:
                status = 400
                out = {"ok": False, "error": {"code": "usage", "hint": "bad body"}}
            else:
                state["own"][b["workspace"]] = b["view"]
                out = {"ok": True, "groupId": "g-1", "workspaceId": b["workspace"],
                       "defaultView": b["view"] or state["view"], "ownDefaultView": b["view"]}
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

k2() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" \
        K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
reqs() { cat "$WORK/reqs.jsonl" 2>/dev/null || true; }
reset() { rm -f "$WORK/reqs.jsonl"; }
set_mode() { printf '%s' "$1" > "$WORK/mode"; }
run() { set +e; out="$(k2 "$@" 2>"$WORK/err")"; rc=$?; set -e; err="$(cat "$WORK/err")"; }

echo "== print =="
set_mode ""
reset
run project view sales
assert_eq "print exit 0" "$rc" "0"
assert_eq "print output" "$out" "sales opens agents in: thread"
assert_absent "print never posts" "$(reqs)" "set-default-view"
run project view sales --json
assert_eq "print --json exit 0" "$rc" "0"
assert_eq "print --json shape" "$out" '{"ok":true,"project":"sales","defaultView":"thread"}'

echo "== set =="
reset
run project view sales terminal
assert_eq "set exit 0" "$rc" "0"
assert_eq "set output" "$out" "sales now opens agents in: terminal"
python3 - "$WORK/reqs.jsonl" <<'PYEOF' && ok "set posts the resolved id and the view" || bad "set request ($(reqs))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
posts = [r for r in rows if r["path"] == "/cli/project-group/set-default-view"]
assert len(posts) == 1 and posts[0]["method"] == "POST", rows
assert json.loads(posts[0]["body"]) == {"group": "g-1", "view": "terminal"}, posts
PYEOF
run project view sales
assert_eq "print after set" "$out" "sales opens agents in: terminal"
reset
run project view sales Chatter --json
assert_eq "set --json exit 0" "$rc" "0"
assert_eq "set --json shape" "$out" '{"ok":true,"project":"sales","defaultView":"chatter"}'
assert_contains "a capitalised view is sent lowercase" "$(reqs)" '\"view\":\"chatter\"'
run project view sales thread
assert_eq "set back to thread" "$out" "sales now opens agents in: thread"

echo "== bad usage: exit 2, nothing sent =="
for args in "sales split" "sales Split" "sales grid" "sales thread extra" "" "sales --nope"; do
    reset
    # shellcheck disable=SC2086
    run project view $args
    assert_eq "'view $args' exits 2" "$rc" "2"
    assert_contains "'view $args' is a usage error" "$err" '"code":"usage"'
    assert_eq "'view $args' sends nothing" "$(reqs)" ""
done
run project view sales split
assert_contains "split is named" "$err" "split is not a project view"
run project view sales grid
assert_contains "bad view names the three" "$err" "view must be terminal, thread or chatter, not 'grid'"

echo "== not found / ambiguous: exit 4 =="
reset
run project view nowhere terminal
assert_eq "unknown project exits 4" "$rc" "4"
assert_contains "unknown project code" "$err" '"code":"not_found"'
assert_absent "unknown project never posts" "$(reqs)" "set-default-view"
run project view s
assert_eq "ambiguous prefix exits 4" "$rc" "4"
assert_contains "ambiguous code" "$err" '"code":"ambiguous"'

echo "== refusals: exit 1 =="
set_mode member
run project view sales terminal
assert_eq "member exits 1" "$rc" "1"
assert_contains "member code" "$err" '"code":"forbidden"'
assert_absent "a login refusal has no card line" "$err" "Ask with:"
assert_absent "a login refusal has no can't line" "$err" "It can't be requested."

set_mode agent-off
run project view sales terminal
assert_eq "switch off exits 1" "$rc" "1"
assert_contains "switch off code" "$err" '"code":"agent_permission_off"'
assert_eq "the error ends with the Ask with line" "$(printf '%s\n' "$err" | tail -n 1)" 'Ask with: k2 permission request projects.manage --why "..."'
assert_eq "the JSON error comes first" "$(printf '%s\n' "$err" | head -n 1 | cut -c1-10)" '{"error":{'
assert_eq "the Ask with line prints once" "$(printf '%s\n' "$err" | grep -c '^Ask with:')" "1"

set_mode agent-cannot
run project view sales terminal
assert_eq "not requestable exits 1" "$rc" "1"
assert_eq "the error ends with It can't be requested." "$(printf '%s\n' "$err" | tail -n 1)" "It can't be requested."

set_mode agent-off
run project view sales
assert_eq "reading still works with the switch off" "$out" "sales opens agents in: thread"
assert_absent "a read prints no card line" "$err" "Ask with:"

echo "== an older daemon (no defaultView) =="
set_mode old
reset
run project view sales terminal
assert_eq "older daemon exits 1" "$rc" "1"
assert_contains "older daemon code" "$err" '"code":"unsupported"'
assert_absent "older daemon never posts" "$(reqs)" "set-default-view"

echo "== --agent: one agent's own view =="
set_mode ""
reset
run project view sales --agent ada
assert_eq "agent print exit 0" "$rc" "0"
assert_eq "agent print follows the project" "$out" "Ada in sales opens in: thread (the project's view)"
assert_absent "agent print never posts" "$(reqs)" "set-member-default-view"
run project view sales --agent ada --json
assert_eq "agent print --json" "$out" '{"ok":true,"project":"sales","agent":"Ada","defaultView":"thread","ownDefaultView":null}'

reset
run project view sales Chatter --agent ADA
assert_eq "agent set exit 0" "$rc" "0"
assert_eq "agent set output" "$out" "Ada in sales now opens in: chatter"
python3 - "$WORK/reqs.jsonl" <<'PYEOF' && ok "agent set posts the project id, the member's workspace id and the view" || bad "agent set request ($(reqs))"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
posts = [r for r in rows if r["path"] == "/cli/project-group/set-member-default-view"]
assert len(posts) == 1 and posts[0]["method"] == "POST", rows
assert json.loads(posts[0]["body"]) == {"group": "g-1", "workspace": "w-ada", "view": "chatter"}, posts
assert not [r for r in rows if r["path"] == "/cli/project-group/set-default-view"], rows
PYEOF
run project view sales --agent /srv/agents/ada
assert_eq "agent by path: its own view" "$out" "Ada in sales opens in: chatter (its own view)"
run project view sales --agent bo
assert_eq "the other agent still follows" "$out" "bo in sales opens in: thread (the project's view)"
run project view sales
assert_eq "the project's view is unchanged" "$out" "sales opens agents in: thread"

reset
run project view sales default --agent w-ada --json
assert_eq "agent clear --json exit 0" "$rc" "0"
assert_eq "agent clear --json" "$out" '{"ok":true,"project":"sales","agent":"Ada","defaultView":"thread","ownDefaultView":null}'
assert_contains "clear posts view null" "$(reqs)" '\"view\":null'
run project view sales terminal --agent ada
run project view sales default --agent ada
assert_eq "agent clear output" "$out" "Ada in sales now follows the project's view: thread"

echo "== --agent: refusals =="
reset
run project view sales terminal --agent nobody
assert_eq "not a member exits 1" "$rc" "1"
assert_contains "not a member code" "$err" '"code":"not_a_member"'
assert_contains "not a member names it" "$err" "'nobody' is not a member of sales"
assert_absent "not a member never posts" "$(reqs)" "set-member-default-view"
for args in "sales default" "sales Default" "sales --agent" "sales split --agent ada"; do
    reset
    # shellcheck disable=SC2086
    run project view $args
    assert_eq "'view $args' exits 2" "$rc" "2"
    assert_contains "'view $args' is a usage error" "$err" '"code":"usage"'
    assert_eq "'view $args' sends nothing" "$(reqs)" ""
done
run project view sales default
assert_contains "default without --agent says why" "$err" "default clears one agent's own view, so it needs --agent <name>"
set_mode member
run project view sales terminal --agent ada
assert_eq "member login exits 1" "$rc" "1"
assert_contains "member login code" "$err" '"code":"forbidden"'
set_mode old-members
reset
run project view sales terminal --agent ada
assert_eq "older daemon (no member views) exits 1" "$rc" "1"
assert_contains "older daemon code" "$err" '"code":"unsupported"'
assert_absent "older daemon never posts" "$(reqs)" "set-member-default-view"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
