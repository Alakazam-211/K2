#!/usr/bin/env bash
# In a session, `k2 agent hire` acts as the agent: every request carries the
# session's passport, never the owner token from disk (0.46.17).
#
# `agent hire` is an Open verb, so inside a K2 terminal the CLI's Open-verb
# rule reads the owner token from disk. In a session (the K2_HOOK_SOCK
# marker) the hire sends the passport on every request instead, and only to
# the routes a passport may use: the hire core (POST /cli/agents/hire, which
# also takes the name, the --agent preset as typed and catalog context),
# project membership, connections, the agent's own messages and the App
# handover. The daemon resolves the preset (id, label or command word), so
# a label goes as typed and a preset it doesn't have is its usage refusal,
# exit 2. A step only the owner's routes take (--mode other than custom,
# --enabled, --color, --focus-group, a --context path, --launch with no
# template, --context with a template) is refused before anything is sent,
# as is a session with no passport. With "Let this agent hire agents"
# off, the hire core's refusal ends with its card line. A refusal of the
# hire itself (here: a folder the agent didn't hire) ends the run cleanly:
# exit 3, nothing sent after it. Outside a session the owner token is sent
# to the same routes as before.
#
# Nothing is installed: the daemon is a stub that logs each request's
# method, path, query, body, Bearer header and transport, and answers a
# passport only on the routes the real daemon admits one on. The CLI runs
# with a clean environment (`env -i`, a temp HOME whose .k2 holds a
# readable owner token), so it can only reach the stub.
# Run with: bash tests/cli/agent_hire_session_passport.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (no $(printf %q "$3") in $(printf %q "$2"))" ;; esac; }
assert_lacks() { case "$2" in *"$3"*) bad "$1 ($(printf %q "$3") found in $(printf %q "$2"))" ;; *) ok "$1" ;; esac; }

WORK="$(mktemp -d -t k2-hire-passport-XXXXXX)"
STUB_PID=""
# A Unix socket path must be short (about 100 bytes): its own folder.
SOCK_DIR="$(mktemp -d /tmp/k2hp-XXXXXX)"
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK" "$SOCK_DIR"
}
trap cleanup EXIT

OWNER="owner-tok-5e7a10"
PASSPORT="sess77.passport3c9f"
PRESET_ID="b0a1c2d3-e4f5-6789-abcd-ef0123456002"
LOG_FILE="$WORK/requests"
MODE_FILE="$WORK/stub-mode"
SOCK="$SOCK_DIR/cell.sock"
EXISTING="$WORK/hired-by-someone-else"

cat >"$WORK/stub.py" <<'PY'
import json, os, socketserver, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

LOG_FILE = os.environ["LOG_FILE"]
MODE_FILE = os.environ["MODE_FILE"]
OWNER = os.environ["OWNER"]
PASSPORT = os.environ["PASSPORT"]
EXISTING = os.environ["EXISTING"]
REFUSED = '{"error":"Invalid or missing auth token"}'
# The hire's routes a passport is admitted on (session_token.rs
# is_agent_verb). Any other route answers a passport the daemon's 403.
AGENT_ROUTES = {"/cli/agents/hire", "/cli/connections", "/cli/workspace/msg", "/cli/app/handover",
                "/cli/project-group/show", "/cli/project-group/add-member"}
ASK = 'k2 permission request agents.hire --why "<what you need it for>"'

def mode():
    try:
        return open(MODE_FILE).read().strip()
    except OSError:
        return ""

def hire(who, body):
    if who == "agent" and mode() == "switch-off":
        return 403, {"ok": False, "error": {"code": "hire_off", "hint": "Ask your human to turn on 'Let this agent hire agents'."},
                     "permission": {"action": "agents.hire", "command": ASK}}
    if who == "agent" and body.get("path") == EXISTING:
        return 403, {"ok": False, "error": {"code": "not_your_hire", "hint": "that folder is a workspace this agent didn't hire"}}
    if body.get("preset") == "No Such Tool":
        return 400, {"ok": False, "error": {"code": "usage",
                     "hint": "preset No Such Tool no longer exists on this server; pick one of: claude (b0a1c2d3-e4f5-6789-abcd-ef0123456001)"}}
    if body.get("dryRun") and not body.get("template"):
        return 200, {"ok": True, "dryRun": True, "hire": "allowed"}
    if body.get("template"):
        launch = body.get("launch") is True
        return 200, {"ok": True, "path": body.get("path"), "id": "w_tpl", "changed": True, "created": True,
                     "rows": [{"step": "role", "status": "done", "target": "ROLE.md"}],
                     "template": {"id": "general", "version": "1"}, "permissions": {}, "questions": [],
                     "helloDelivered": launch, "onboardQueued": launch and bool(body.get("onboard"))}
    steps = [{"step": "create-dir", "status": "done"}, {"step": "register-workspace", "status": "done"},
             {"step": "write-persona", "status": "done"}]
    if body.get("name"):
        steps.append({"step": "set-name", "status": "done"})
    if body.get("preset"):
        steps.append({"step": "set-default-agent", "status": "done"})
    return 200, {"ok": True, "path": body.get("path"), "id": "w_new", "changed": True, "created": True, "steps": steps}

def answer(path, method, query, who, body):
    if path == "/cli/agents/hire" and method == "POST":
        return hire(who, body)
    if path == "/cli/project-group/show":
        return 200, {"ok": True, "id": "g_ops", "name": "Ops", "members": []}
    if path == "/cli/project-group/add-member":
        return 200, {"ok": True, "becamePoc": True}
    if path == "/cli/connections":
        if "action=resolve" in query:
            return 200, {"ok": True, "kind": "local", "path": "/srv/peer"}
        return 200, {"ok": True}
    if path == "/cli/workspace/msg":
        return 200, {"success": True}
    if path == "/cli/agent/conf":
        return 404, {"error": {"code": "not_found", "hint": "no agent there"}}
    if path == "/cli/workspace/set":
        return 200, {"ok": True, "actions": [{"field": "agent_mode", "status": "skipped"}]}
    if path in ("/cli/workspace/set-agent-display-name", "/cli/workspace/ensure-canonical-session"):
        return 200, {"success": True, "session_id": "s-1"}
    return 404, {"error": "route not found"}

def handler(via):
    class H(BaseHTTPRequestHandler):
        def _send(self, code, obj):
            data = (obj if isinstance(obj, str) else json.dumps(obj)).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def _answer(self, method, raw):
            u = urlsplit(self.path)
            auth = self.headers.get("Authorization")
            with open(LOG_FILE, "a") as f:
                f.write(json.dumps({"via": via, "method": method, "path": u.path, "query": u.query, "auth": auth,
                                    "body": raw.decode("utf-8", "replace")}) + "\n")
            if u.path == "/boot-status":
                return self._send(200, {"view": "full", "phase": "ready", "features": ["send-once"]})
            if auth == "Bearer " + PASSPORT:
                who = "agent"
            elif via == "tcp" and auth == "Bearer " + OWNER:
                who = "owner"
            else:
                return self._send(403, REFUSED)
            if who == "agent" and u.path not in AGENT_ROUTES:
                return self._send(403, REFUSED)
            try:
                body = json.loads(raw) if raw else {}
            except ValueError:
                body = {}
            code, obj = answer(u.path, method, u.query, who, body if isinstance(body, dict) else {})
            return self._send(code, obj)

        def do_GET(self):
            self._answer("GET", b"")

        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            self._answer("POST", self.rfile.read(n))

        def log_message(self, *_a):
            pass
    return H

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    def get_request(self):
        conn, _ = super().get_request()
        return conn, ("uds", 0)

class T(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True

u = U(os.environ["SOCK_PATH"], handler("uds"))
threading.Thread(target=u.serve_forever, daemon=True).start()
T(("127.0.0.1", int(os.environ["PORT"])), handler("tcp")).serve_forever()
PY

WS="$WORK/ws"
mkdir -p "$WS/.k2" "$EXISTING"
PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
: >"$LOG_FILE"
: >"$MODE_FILE"
LOG_FILE="$LOG_FILE" MODE_FILE="$MODE_FILE" PORT="$PORT" SOCK_PATH="$SOCK" OWNER="$OWNER" PASSPORT="$PASSPORT" \
    EXISTING="$EXISTING" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if [ -S "$SOCK" ] && curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/boot-status"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

# The owner token on disk, readable, as in a K2 terminal on the owner's box.
FAKE_HOME="$WORK/home"
mkdir -p "$FAKE_HOME/.k2"
printf '%s' "$OWNER" >"$FAKE_HOME/.k2/heartbeat.token"
chmod 600 "$FAKE_HOME/.k2/heartbeat.token"
printf '%s' "$PORT" >"$FAKE_HOME/.k2/heartbeat.port"

SESSION=(K2_HOOK_SOCK="$SOCK" K2_HOOK_TOKEN="$PASSPORT")

run() { # env words..., then --, then the k2 args
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$LOG_FILE"
    set +e
    out="$(cd "$WS" && env -i PATH="/usr/bin:/bin" HOME="$FAKE_HOME" K2_HOME="$FAKE_HOME/.k2" \
        K2_PROJECT_PATH="$WS" ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" 2>"$WORK/err" </dev/null)"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
# field <name>: that field of every logged /cli/ request, one per line.
field() {
    "$PY" -c 'import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r.get("path", "").startswith("/cli/"):
        print(r.get(sys.argv[2]))' "$LOG_FILE" "$1"
}
# body_of <path>: the JSON body of the first logged request to that path.
body_of() {
    "$PY" -c 'import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r.get("path") == sys.argv[2]:
        print(r.get("body")); break' "$LOG_FILE" "$1"
}
query_of() {
    "$PY" -c 'import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r.get("path") == sys.argv[2]:
        print(r.get("query")); break' "$LOG_FILE" "$1"
}
requests() { grep -c '"path": "/cli/' "$LOG_FILE" || true; }
owner_never_sent() { # label
    if grep -Fq -- "$OWNER" "$LOG_FILE"; then
        bad "$1: the owner token was sent ($(grep -F -- "$OWNER" "$LOG_FILE" | head -n 1))"
    else
        ok "$1: the owner token is in no header, query or body"
    fi
}
all_passport() { # label
    local a others
    others="$(field auth | grep -vxF "Bearer $PASSPORT" || true)"
    a="$(field auth | grep -cxF "Bearer $PASSPORT" || true)"
    if [ -z "$others" ] && [ "$a" -gt 0 ]; then
        ok "$1: every request ($a) carried the passport"
    else
        bad "$1: a request without the passport: $(printf %q "$others")"
    fi
}
OWNER_ROUTES="/cli/agent/conf /cli/presets/list /cli/focus-groups/list /cli/focus-groups/assign /cli/context/layers /cli/context/add /cli/projects/list /cli/projects/update /cli/workspace/set /cli/workspace/set-agent-display-name /cli/workspace/ensure-canonical-session"
no_owner_routes() { # label
    local p hit=""
    for p in $OWNER_ROUTES; do
        if field path | grep -qxF "$p"; then hit="$hit $p"; fi
    done
    if [ -z "$hit" ]; then ok "$1: no request to an owner-only route"; else bad "$1: owner-only routes asked:$hit"; fi
}

echo "== in a session: the whole hire rides the passport =="
run "${SESSION[@]}" -- agent hire "$WORK/scout" --name "Scout Two" --agent "$PRESET_ID" \
    --context wiki:hygiene --project Ops --connect peer --onboard "welcome aboard" --db-access off --json
assert_eq "session hire: exit 0 ($err)" "$rc" "0"
owner_never_sent "session hire"
all_passport "session hire"
no_owner_routes "session hire"
core="$(body_of /cli/agents/hire)"
assert_eq "session hire: the core gets the name" "$("$PY" -c 'import json,sys; print(json.loads(sys.argv[1]).get("name"))' "$core")" "Scout Two"
assert_eq "session hire: the core gets the exact preset id" "$("$PY" -c 'import json,sys; print(json.loads(sys.argv[1]).get("preset"))' "$core")" "$PRESET_ID"
assert_eq "session hire: the core gets the catalog context" "$("$PY" -c 'import json,sys; print(json.loads(sys.argv[1]).get("context"))' "$core")" "['wiki:hygiene']"
paths="$(field path | tr '\n' ' ')"
assert_contains "session hire: project membership asked" "$paths" "/cli/project-group/add-member"
assert_contains "session hire: the connection asked" "$(field query | tr '\n' ' ')" "action=add"
mq="$(query_of /cli/workspace/msg)"
assert_contains "session hire: the onboard message is the agent's own" "$mq" "from=ws"
assert_lacks "session hire: the onboard message is not from the owner" "$mq" "from=owner"
for s in set-name set-default-agent seed-context join-project connect onboard; do
    st="$("$PY" -c 'import json,sys; d=json.loads(sys.argv[1]); print(",".join(a["status"] for a in d["actions"] if a["step"]==sys.argv[2]))' "$out" "$s")"
    assert_eq "session hire: step $s" "$st" "done"
done
st="$("$PY" -c 'import json,sys; d=json.loads(sys.argv[1]); print([a["step"] for a in d["actions"] if a["step"] in ("set-mode","launch")])' "$out")"
assert_eq "session hire: no mode write and no launch route" "$st" "[]"

echo "== in a session: a template hire's launch and onboard are the hire core's =="
run "${SESSION[@]}" -- agent hire "$WORK/writer" --template worker --name Quill --launch --onboard "hi there" --json
assert_eq "template hire: exit 0 ($err)" "$rc" "0"
owner_never_sent "template hire"
all_passport "template hire"
no_owner_routes "template hire"
assert_eq "template hire: only the hire core asked" "$(field path | sort -u | tr '\n' ' ')" "/cli/agents/hire "
core="$(body_of /cli/agents/hire)"
assert_eq "template hire: launch and onboard in the core body" \
    "$("$PY" -c 'import json,sys; b=json.loads(sys.argv[1]); print(b.get("launch"), b.get("onboard"), b.get("name"))' "$core")" "True hi there Quill"
st="$("$PY" -c 'import json,sys; d=json.loads(sys.argv[1]); print(",".join("%s=%s" % (a["step"], a["status"]) for a in d["actions"] if a["step"] in ("launch","hello","onboard","set-name")))' "$out")"
assert_eq "template hire: steps" "$st" "set-name=done,launch=done,hello=done,onboard=done"

echo "== in a session: a dry run asks the hire core with the passport =="
run "${SESSION[@]}" -- agent hire "$WORK/dry" --name Dry --dry-run --json
assert_eq "dry run: exit 0 ($err)" "$rc" "0"
owner_never_sent "dry run"
all_passport "dry run"
assert_eq "dry run: only the hire core asked" "$(field path | sort -u | tr '\n' ' ')" "/cli/agents/hire "
assert_eq "dry run: the core is asked a dry run" "$(body_of /cli/agents/hire)" '{"dryRun": true}'

echo "== in a session: a person's steps are refused, nothing sent =="
REFUSALS=(
    "--mode manager|--mode manager"
    "--enabled false|--enabled"
    "--color #112233|--color"
    "--focus-group Ops|--focus-group"
    "--context ./notes/brief.md|--context ./notes/brief.md (a path)"
    "--launch|--launch (with no --template)"
    "--template worker --context wiki:hygiene|--context wiki:hygiene (with --template)"
)
for row in "${REFUSALS[@]}"; do
    flags="${row%%|*}"
    named="${row#*|}"
    # shellcheck disable=SC2086
    run "${SESSION[@]}" -- agent hire "$WORK/refused" $flags
    assert_eq "refused $flags: exit 3" "$rc" "3"
    assert_eq "refused $flags: no request" "$(requests)" "0"
    assert_contains "refused $flags: names the step" "$err" "these steps are a person's: $named. Nothing was sent."
    assert_contains "refused $flags: where a person does it" "$err" "Settings → Workspaces / Agents → the agent"
done

echo "== in a session: --agent goes to the hire core as typed =="
run "${SESSION[@]}" -- agent hire "$WORK/labelled" --agent "Cursor Agent" --json
assert_eq "preset label: exit 0 ($err)" "$rc" "0"
owner_never_sent "preset label"
all_passport "preset label"
no_owner_routes "preset label"
assert_eq "preset label: only the hire core asked" "$(field path | sort -u | tr '\n' ' ')" "/cli/agents/hire "
assert_eq "preset label: the core gets the label as typed" \
    "$("$PY" -c 'import json,sys; print(json.loads(sys.argv[1]).get("preset"))' "$(body_of /cli/agents/hire)")" "Cursor Agent"
st="$("$PY" -c 'import json,sys; d=json.loads(sys.argv[1]); print(",".join(a["status"] for a in d["actions"] if a["step"]=="set-default-agent"))' "$out")"
assert_eq "preset label: step set-default-agent" "$st" "done"
run "${SESSION[@]}" -- agent hire "$WORK/nosuch" --agent "No Such Tool" --onboard "hi"
assert_eq "unknown preset: exit 2, the daemon's usage" "$rc" "2"
owner_never_sent "unknown preset"
assert_eq "unknown preset: only the hire core asked, nothing after" "$(field path | sort -u | tr '\n' ' ')" "/cli/agents/hire "
assert_contains "unknown preset: the daemon's list" "$err" "preset No Such Tool no longer exists on this server; pick one of: claude ("

echo "== in a session with no passport: nothing sent =="
run K2_HOOK_SOCK="$SOCK" -- agent hire "$WORK/nopass" --name Nope
assert_eq "no passport: exit 3" "$rc" "3"
assert_eq "no passport: no request" "$(requests)" "0"
owner_never_sent "no passport"
assert_contains "no passport: says so" "$err" "this session has no passport, so nothing was sent."

echo "== in a session, hiring off: the card line =="
printf 'switch-off' >"$MODE_FILE"
run "${SESSION[@]}" -- agent hire "$WORK/off" --name Off
: >"$MODE_FILE"
assert_eq "hiring off: exit 3" "$rc" "3"
owner_never_sent "hiring off"
all_passport "hiring off"
assert_eq "hiring off: only the hire core asked" "$(field path | sort -u | tr '\n' ' ')" "/cli/agents/hire "
assert_contains "hiring off: the refusal" "$err" "hire_off"
assert_eq "hiring off: the card line, last" "$(printf '%s\n' "$err" | tail -n 1)" 'Ask with: k2 permission request agents.hire --why "<what you need it for>"'

echo "== in a session: the daemon refuses a folder the agent didn't hire =="
run "${SESSION[@]}" -- agent hire "$EXISTING" --name Taken --onboard "hello"
assert_eq "not its hire: exit 3" "$rc" "3"
owner_never_sent "not its hire"
all_passport "not its hire"
assert_eq "not its hire: nothing sent after the refusal" "$(field path | tr '\n' ' ')" "/cli/agents/hire "
assert_contains "not its hire: the daemon's words" "$err" "that folder is a workspace this agent didn't hire"
assert_contains "not its hire: the code" "$err" "not_your_hire"

echo "== outside a session: the owner token, as before =="
run -- agent hire "$WORK/owner-hire" --name Owned --json
assert_eq "owner hire: exit 0 ($err)" "$rc" "0"
assert_lacks "owner hire: no passport" "$(cat "$LOG_FILE")" "$PASSPORT"
others="$(field auth | grep -vxF "Bearer $OWNER" || true)"
assert_eq "owner hire: every request carried the owner token" "$others" ""
paths="$(field path | tr '\n' ' ')"
for p in /cli/agent/conf /cli/agents/hire /cli/workspace/set /cli/workspace/set-agent-display-name; do
    assert_contains "owner hire: asks $p" "$paths" "$p"
done

echo
echo "agent_hire_session_passport: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
