#!/usr/bin/env bash
# Permission cards (prd-agent-settings-cards-v1 rev 3.1 §3.1, §11; lane C):
# the CLI half of "on, the agent is taught" — the counterpart of
# permission_off_golden.sh ("off, today's bytes").
#
# With a daemon that says the family is on (`GET /cli/permission/enabled`
# → 200 {"enabled":true}) and refusals that carry the `permission` object:
#   * `k2 help` has the one permission line under ASK A HUMAN;
#   * `k2 study` lists `permissions`; `k2 study permissions` is the page;
#   * a refused verb's stderr ENDS with "Ask with: k2 permission request …"
#     (or a machine hint, then "It can't be requested."), printed once;
#   * `cli_print_or_fail` exits 3 for a teaching refusal, 1 otherwise;
#   * `k2 permission request …` reaches /cli/permission/request;
#   * people verbs are refused inside a K2 terminal.
# And with a daemon that says off (any non-200), every one of those is
# today's text: the unknown command, no help line, no closing line.
#
# Hermetic: throwaway HOME, every K2_*/K2SO_* var unset, a python stub
# daemon on loopback. No real daemon, ~/.k2 or keychain.
#
#   bash tests/cli/permission_teach.sh      (fails loudly on any miss)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d /tmp/k2pct-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

# The stub: MODE file "on" | "off". Routes:
#   /cli/permission/enabled  → 200 {"enabled":true} when on, else 405
#   /cli/permission/request  → 200 a pending request
#   /cli/connections         → 403 agents_create_connections_disabled + permission (Ask)
#   /cli/compute/admin/propose → 403 remote_admin_off + machine hint
#   /cli/project-group/create  → 403 agent_permission_off + permission
#   /cli/settings/update     → 403 owner_only, no permission (stays exit 1)
#   anything else            → 403 owner_only + permission {requestable:false}
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

LOG, MODE = sys.argv[1], sys.argv[3]

def ask(action, cmd_tail=""):
    return {"action": action, "params": {}, "approvers": "owner or admin",
            "command": "k2 permission request %s%s --why \"<what you need it for>\"" % (action, cmd_tail)}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def reply(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        path = urlparse(self.path).path
        with open(LOG, "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        on = open(MODE).read().strip() == "on"
        if path == "/cli/permission/enabled":
            if on:
                return self.reply(200, {"enabled": True})
            return self.reply(405, {"error": "method not allowed for this route"})
        if path == "/cli/permission/request":
            return self.reply(200, {"id": "pr_7f3a2c", "status": "pending", "expiresAt": 4102444800,
                                    "routedTo": [{"login": "rosson", "role": "owner"}, {"login": "maya", "role": "admin"}],
                                    "text": "Let billing run jobs on this server's compute nodes."})
        if path == "/cli/thread/post":
            # A success whose body still carries a `permission` object (what a
            # transport notes when a first hop was refused and a fallback then
            # succeeded, 0.46.4 field report): the command exits 0.
            return self.reply(200, {"ok": True, "id": "t_1", "seq": 1,
                                    "permission": {"requestable": False, "where": "a person does this"}})
        if path == "/cli/app/install" and on:
            # A held install that reused an earlier request; the card could
            # not go up, so the hold carries the Ask object (0.46.4 #9/#11).
            body = {"ok": True, "status": "held", "kind": "manage", "app": "heldapp",
                    "request": {"id": "w_abc123def456", "summary": "k2 app install heldapp"},
                    "hint": "held for the owner: they approve it in person.",
                    "reused": True, "askedAgo": "2 h",
                    "card": {"raised": False, "where": "no permission card went up; the request waits in Settings → App Approvals"},
                    "permission": ask("apps.approve", " request=w_abc123def456")}
            return self.reply(202, body)
        if path in ("/cli/settings/update", "/cli/workspace/open"):
            return self.reply(403, {"error": {"code": "owner_only", "hint": "owner only"}})
        err = {"error": {"code": "owner_only", "hint": "only the server's owner may do this"}}
        if on:
            if path == "/cli/connections" and "action=remove" in self.path:
                err = {"error": {"code": "remote_admin_off", "hint": "z3mbpz only takes admin steps from someone at that Mac."},
                       "permission": {"requestable": False, "where": "at the machine",
                                      "hint": "At z3mbpz, run: sudo k2-node admin level policy   (switch back after: sudo k2-node admin level off)"}}
            elif path == "/cli/connections":
                err = {"error": {"code": "agents_create_connections_disabled", "hint": "connections are off"}, "ok": False,
                       "permission": ask("connections.add", " to=billing")}
            elif path == "/cli/workspace/create":
                err = {"error": {"code": "agent_permission_off", "hint": "projects off"}, "ok": False,
                       "permission": ask("projects.manage")}
            elif path == "/cli/compute/admin/propose":
                err = {"error": {"code": "remote_admin_off", "hint": "z3mbpz only takes admin steps from someone at that Mac."},
                       "permission": {"requestable": False, "where": "at the machine",
                                      "hint": "At z3mbpz, run: sudo k2-node admin level policy   (switch back after: sudo k2-node admin level off)"}}
            else:
                err["permission"] = {"requestable": False, "where": "a person does this, in Settings or from a terminal outside K2"}
        self.reply(403, err)
    do_GET = handle_any
    do_POST = handle_any

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
MODE="$SANDBOX/mode"
: > "$LOG"
echo on > "$MODE"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$MODE" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
# A person in a terminal outside K2: the owner credential on disk, no passport.
PERSON_HOME="$SANDBOX/person"
mkdir -p "$PERSON_HOME/.k2"
echo "$PORT" > "$PERSON_HOME/.k2/heartbeat.port"
echo owner-teach > "$PERSON_HOME/.k2/heartbeat.token"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $*" >&2; fail=$((fail + 1)); }

# run <who> <args…>: who = agent (a K2 terminal: K2_HOOK_TOKEN) | person
# (a terminal outside K2: owner token, no passport). Sets OUT, ERR, RC.
run() {
    local who="$1"; shift
    : > "$LOG"
    set +e
    case "$who" in
        agent) K2_PORT="$PORT" K2_HOOK_TOKEN=passport-teach "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"; RC=$? ;;
        person) HOME="$PERSON_HOME" "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"; RC=$? ;;
    esac
    set -e
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
}

last_err_line() { printf '%s\n' "$ERR" | sed -e '/^$/d' | tail -1; }

echo "== on =="
run agent help
case "$OUT" in
    *'  permission request <action> --why "…"   Ask a person to allow what K2 refused you (k2 study permissions)'*) ok ;;
    *) bad "k2 help: no permission line while on" ;;
esac
[ "$RC" = 0 ] && ok || bad "k2 help: exit $RC"
[ "$(printf '%s\n' "$OUT" | grep -c 'permission request')" = 1 ] && ok || bad "k2 help: the line must appear once"

run agent study
printf '%s\n' "$OUT" | grep -q '^  permissions         Permission cards (Beta)' && ok || bad "k2 study: no permissions entry while on"
run agent study --json
printf '%s' "$OUT" | grep -q '"id":"permissions"' && ok || bad "k2 study --json: no permissions topic while on"
run agent study permissions
[ "$RC" = 0 ] && ok || bad "k2 study permissions: exit $RC ($ERR)"
printf '%s\n' "$OUT" | head -1 | grep -qx '# Permission cards (Beta)' && ok || bad "k2 study permissions: wrong page: $(printf '%s' "$OUT" | head -2)"
run agent study --help
printf '%s\n' "$OUT" | grep -qx '  permissions' && ok || bad "k2 study --help: no permissions topic while on"

# A refused verb ends with the Ask-with line, printed once.
run agent connections add billing
[ "$(last_err_line)" = 'Ask with: k2 permission request connections.add to=billing --why "<what you need it for>"' ] && ok \
    || bad "connections add: last stderr line is '$(last_err_line)'"
[ "$(printf '%s\n' "$ERR" | grep -c '^Ask with:')" = 1 ] && ok || bad "connections add: the closing line must print once"
[ "$RC" = 3 ] && ok || bad "connections add: exit $RC, want 3"

# A command that succeeds never ends with a closing line, even when a body
# on the way carried a `permission` object (0.46.4 field report: a
# successful `k2 thread` post printed "It can't be requested." at exit 0).
run agent thread billing "the report is ready"
[ "$RC" = 0 ] && ok || bad "thread post: exit $RC, want 0 ($ERR)"
printf '%s\n%s' "$OUT" "$ERR" | grep -q 'Ask with:\|It can.t be requested' && bad "thread post: a closing line on success: $ERR" || ok
grep -q '^POST /cli/thread/post ' "$LOG" && ok || bad "thread post: never reached the stub: $(cat "$LOG")"

# A held App install (exit 2) says it reused the earlier request, where the
# owner sees it, and ends with the Ask-with line the compute/App transport
# now notes (0.46.4 #9/#11).
run agent app install heldapp
[ "$RC" = 2 ] && ok || bad "app install held: exit $RC, want 2 ($ERR)"
printf '%s\n' "$OUT" | grep -q '^held for the owner: w_abc123def456' && ok || bad "app install held: '$OUT'"
printf '%s\n' "$OUT" | grep -q 'this is your earlier request w_abc123def456 (asked 2 h ago)' && ok || bad "app install held: no reuse line: '$OUT'"
printf '%s\n' "$OUT" | grep -q '^  owner: no permission card went up; the request waits in Settings → App Approvals' && ok || bad "app install held: no owner line: '$OUT'"
[ "$(last_err_line)" = 'Ask with: k2 permission request apps.approve request=w_abc123def456 --why "<what you need it for>"' ] && ok \
    || bad "app install held: last stderr line is '$(last_err_line)'"

# A machine-only refusal: the plain-text hint, then "It can't be requested."
run agent connections remove billing
tail2="$(printf '%s\n' "$ERR" | sed -e '/^$/d' | tail -2)"
[ "$tail2" = "At z3mbpz, run: sudo k2-node admin level policy   (switch back after: sudo k2-node admin level off)
It can't be requested." ] && ok || bad "machine hint: stderr ends with '$tail2'"

# cli_print_or_fail: a teaching refusal exits 3 …
run agent workspace create "$SANDBOX/new-ws"
[ "$(last_err_line)" = 'Ask with: k2 permission request projects.manage --why "<what you need it for>"' ] && ok \
    || bad "workspace create: last stderr line is '$(last_err_line)'"
[ "$RC" = 3 ] && ok || bad "workspace create: exit $RC, want 3"

# … and a refusal with no `permission` object keeps today's exit 1 and no line.
run agent workspace open "$SANDBOX/ws"
printf '%s' "$ERR" | grep -q 'Ask with:\|It can.t be requested' && bad "workspace open: a closing line without a permission object" || ok
[ "$RC" = 1 ] && ok || bad "workspace open: exit $RC, want 1"

# Any other refusal to the agent: "It can't be requested."
run agent settings set computePreview true
[ "$(last_err_line)" = "It can't be requested." ] && ok || bad "settings set: last stderr line is '$(last_err_line)'"

# `k2 permission request` reaches the daemon and prints the ask.
run agent permission request compute.use --why "Run the test suite on k2n2-m1"
[ "$RC" = 0 ] && ok || bad "permission request: exit $RC ($ERR)"
printf '%s\n' "$OUT" | grep -q '^Asked: pr_7f3a2c (pending; expires in .*). Shown to: rosson (owner), maya (admin).$' && ok \
    || bad "permission request: '$OUT'"
grep -q '^POST /cli/permission/request {"action": "compute.use", "params": {}, "why": "Run the test suite on k2n2-m1"}$' "$LOG" && ok \
    || bad "permission request: wrong request: $(cat "$LOG")"
run agent permission request compute.use
[ "$RC" = 2 ] && ok || bad "permission request without --why: exit $RC, want 2"

# People verbs are refused in a K2 terminal.
run agent permission pending
[ "$RC" = 3 ] && ok || bad "permission pending in a K2 terminal: exit $RC, want 3"

# From a terminal outside K2, the owner turns the switch on even while off.
echo off > "$MODE"
run person permission beta on
grep -q '^POST /cli/settings/update {"permissionCards":true}$' "$LOG" && ok || bad "permission beta on: wrong request: $(cat "$LOG")"
run person permission pending
printf '%s' "$ERR" | head -1 | grep -qx 'Unknown command: permission' && ok || bad "permission pending (person, off): '$ERR'"
echo on > "$MODE"

echo "== off (the daemon says no) =="
echo off > "$MODE"
run agent help
printf '%s' "$OUT" | grep -q 'permission request' && bad "k2 help: permission line while off" || ok
run agent permission request compute.use --why x
[ "$RC" = 1 ] && ok || bad "permission (off): exit $RC, want 1"
printf '%s' "$ERR" | head -1 | grep -qx 'Unknown command: permission' && ok || bad "permission (off): '$(printf '%s' "$ERR" | head -1)'"
run agent study permissions
[ "$RC" = 2 ] && ok || bad "study permissions (off): exit $RC, want 2"
run agent connections add billing
printf '%s' "$ERR" | grep -q 'Ask with:\|It can.t be requested' && bad "connections add (off): a closing line" || ok

echo "permission_teach: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
