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
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

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
#   /cli/settings/update     → 403 owner_only, no permission (stays exit 1);
#                              to the agent's passport while on, + permission
#                              {requestable:false}, as the daemon's
#                              send_response teaches it (k2 settings set)
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
        if path == "/cli/compute/nodes/policy/set":
            # A widening is held for the owner (`held: true`, a car_ id);
            # with cards on it teaches the card that brings it to chat (0.46.9).
            held = {"held": True, "request": {"id": "car_9f8e7d", "state": "pending", "node": "k2n1-m4",
                                               "summary": "change job limits on k2n1-m4: max_cpus=10"},
                    "hint": "held for the owner"}
            if on:
                held["permission"] = ask("compute.admin-approve", " request=car_9f8e7d")
            return self.reply(200, held)
        if path == "/cli/agent-access/set":
            # The switch's route refuses every passport (`forbidden`); the
            # daemon teaches compute.use only for the agent's own
            # `k2 compute access on` (0.46.9, Rosson 2026-11-05).
            err = {"error": {"code": "forbidden", "hint": "only the owner or an admin login may change agent access"}}
            if on:
                b = json.loads(body or "{}")
                if b.get("toggle") == "compute" and b.get("value") is True and not b.get("workspace"):
                    err["permission"] = ask("compute.use")
                else:
                    err["permission"] = {"requestable": False, "where": "a person does this, in Settings or from a terminal outside K2"}
            return self.reply(403, err)
        if path in ("/cli/workspace/msg", "/cli/inbox/compose") and on:
            # 0.46.16: `k2 msg` to an agent that isn't a connection teaches
            # connections.add; one the agent can't reach by any card
            # (`gated`) says it can't be requested. Both are form POSTs.
            # The sender's-name lookup before the send (`/cli/workspace/handle`)
            # falls to the last branch: a refusal it can't request.
            b = body + "&" + urlparse(self.path).query
            if "workspace=gamma" in b or "to=gamma" in b:
                return self.reply(403, {"ok": False, "error": {"code": "gated", "hint": "gamma takes no messages from agents"},
                                        "permission": {"requestable": False, "where": "a person does this, in Settings"}})
            return self.reply(403, {"ok": False, "error": {"code": "not_connected", "hint": "'beta' is not a local connection"},
                                    "permission": ask("connections.add", " to=beta")})
        if path in ("/cli/settings/update", "/cli/workspace/open"):
            err = {"error": {"code": "owner_only", "hint": "owner only"}}
            if on and path == "/cli/settings/update" and self.headers.get("Authorization") == "Bearer passport-teach":
                if json.loads(body or "{}") == {"ciPreview": True}:
                    # 0.46.17: the agent's own ciPreview true asks the owner.
                    err["permission"] = dict(ask("ci.preview"), approvers="owner")
                else:
                    err["permission"] = {"requestable": False, "where": "a person does this, from a terminal outside K2"}
            return self.reply(403, err)
        if path == "/cli/ci/run" and on:
            # 0.46.17: CI on K2 Compute is off (409 ci_preview_off).
            return self.reply(409, {"error": {"code": "ci_preview_off", "hint": "k2 ci is part of CI on K2 Compute, a preview that is off on this server"},
                                    "permission": dict(ask("ci.preview"), approvers="owner")})
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
<<<"$OUT" grep -q '^  permissions         Permission cards: ask a person' && ok || bad "k2 study: no permissions entry while on"
run agent study --json
<<<"$OUT" grep -q '"id":"permissions"' && ok || bad "k2 study --json: no permissions topic while on"
run agent study permissions
[ "$RC" = 0 ] && ok || bad "k2 study permissions: exit $RC ($ERR)"
<<<"$OUT" head -1 | grep -qx '# Permission cards (Beta)' && ok || bad "k2 study permissions: wrong page: $(printf '%s' "$OUT" | head -2)"
run agent study --help
<<<"$OUT" grep -qx '  permissions' && ok || bad "k2 study --help: no permissions topic while on"

# A refused verb ends with the Ask-with line, printed once.
run agent connections add billing
[ "$(last_err_line)" = 'Ask with: k2 permission request connections.add to=billing --why "<what you need it for>"' ] && ok \
    || bad "connections add: last stderr line is '$(last_err_line)'"
[ "$(printf '%s\n' "$ERR" | grep -c '^Ask with:')" = 1 ] && ok || bad "connections add: the closing line must print once"
[ "$RC" = 3 ] && ok || bad "connections add: exit $RC, want 3"

# 0.46.16 field report: `k2 msg` to an agent that isn't a connection got
# JSON naming connections.add, then "It can't be requested." The form
# transports never noted the send's own `permission` object, so the line
# came from (or went missing behind) another request. The send's line wins.
for form in "msg beta ping" "msg beta --inbox-silent --title Note --body body"; do
    # shellcheck disable=SC2086
    run agent $form
    [ "$RC" = 3 ] && ok || bad "$form: exit $RC, want 3 ($ERR)"
    [ "$(last_err_line)" = 'Ask with: k2 permission request connections.add to=beta --why "<what you need it for>"' ] && ok \
        || bad "$form: last stderr line is '$(last_err_line)'"
    [ "$(printf '%s\n' "$ERR" | grep -c '^Ask with:')" = 1 ] && ok || bad "$form: the closing line must print once"
    <<<"$ERR" grep -q "It can.t be requested" && bad "$form: 'It can't be requested.' beside the Ask-with line: $ERR" || ok
done
# A refusal no card opens still ends with "It can't be requested."
run agent msg gamma ping
[ "$RC" = 3 ] && ok || bad "msg gamma: exit $RC, want 3 ($ERR)"
[ "$(last_err_line)" = "It can't be requested." ] && ok || bad "msg gamma: last stderr line is '$(last_err_line)'"
<<<"$ERR" grep -q '^Ask with:' && bad "msg gamma: an Ask-with line on a refusal no card opens: $ERR" || ok

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
<<<"$OUT" grep -q '^held for the owner: w_abc123def456' && ok || bad "app install held: '$OUT'"
<<<"$OUT" grep -q 'this is your earlier request w_abc123def456 (asked 2 h ago)' && ok || bad "app install held: no reuse line: '$OUT'"
<<<"$OUT" grep -q '^  owner: no permission card went up; the request waits in Settings → App Approvals' && ok || bad "app install held: no owner line: '$OUT'"
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
<<<"$ERR" grep -q 'Ask with:\|It can.t be requested' && bad "workspace open: a closing line without a permission object" || ok
[ "$RC" = 1 ] && ok || bad "workspace open: exit $RC, want 1"

# 0.46.9: the agent's own `k2 compute access on` asks for compute.use ...
run agent compute access on
[ "$RC" = 3 ] && ok || bad "compute access on: exit $RC, want 3 ($ERR)"
[ "$(last_err_line)" = 'Ask with: k2 permission request compute.use --why "<what you need it for>"' ] && ok \
    || bad "compute access on: last stderr line is '$(last_err_line)'"
[ "$(printf '%s\n' "$ERR" | grep -c '^Ask with:')" = 1 ] && ok || bad "compute access on: the closing line must print once"
grep '^POST /cli/agent-access/set ' "$LOG" | grep -q '"toggle": "compute"' && ok || bad "compute access on: wrong request: $(cat "$LOG")"
grep '^POST /cli/agent-access/set ' "$LOG" | grep -q '"workspace"' && bad "compute access on: a K2 terminal names no workspace: $(cat "$LOG")" || ok
# ... while "Allow managing compute nodes" stays the owner's own act.
run agent compute manage on
[ "$RC" = 3 ] && ok || bad "compute manage on: exit $RC, want 3 ($ERR)"
[ "$(last_err_line)" = "It can't be requested." ] && ok || bad "compute manage on: last stderr line is '$(last_err_line)'"

# 0.46.9: a held node request (exit 2) says where the owner approves it,
# and ends with the Ask-with line that brings it to their chat.
run agent compute node policy k2n1-m4 set max_cpus=10 --reason "bigger builds"
[ "$RC" = 2 ] && ok || bad "node policy held: exit $RC, want 2 ($ERR)"
<<<"$ERR" grep -q '^held for the owner: car_9f8e7d$' && ok || bad "node policy held: '$ERR'"
<<<"$ERR" grep -q "^  to ask the owner in chat now, run the Ask with: line below: it puts this same request on a card.$" && ok || bad "node policy held: no chat line: '$ERR'"
<<<"$ERR" grep -q "don't run your command again" && ok || bad "node policy held: no don't-rerun line: '$ERR'"
[ "$(last_err_line)" = 'Ask with: k2 permission request compute.admin-approve request=car_9f8e7d --why "<what you need it for>"' ] && ok \
    || bad "node policy held: last stderr line is '$(last_err_line)'"

# Any other refusal to the agent: "It can't be requested." `k2 settings set`
# (0.46.9) writes /cli/settings/update; the owner's switch, exit 3, line once.
run agent settings set computePreview true
[ "$RC" = 3 ] && ok || bad "settings set: exit $RC, want 3 ($ERR)"
[ "$(last_err_line)" = "It can't be requested." ] && ok || bad "settings set: last stderr line is '$(last_err_line)'"
[ "$(printf '%s\n' "$ERR" | grep -c "It can't be requested.")" = 1 ] && ok || bad "settings set: the closing line must print once"
grep -q '^POST /cli/settings/update {"computePreview":true}$' "$LOG" && ok || bad "settings set: wrong request: $(cat "$LOG")"

# 0.46.17: the agent's own `k2 settings set ciPreview true`, and any k2 ci
# verb while CI on K2 Compute is off, end with the owner's ci.preview card.
CI_ASK='Ask with: k2 permission request ci.preview --why "<what you need it for>"'
run agent settings set ciPreview true
[ "$RC" = 3 ] && ok || bad "settings set ciPreview: exit $RC, want 3 ($ERR)"
[ "$(last_err_line)" = "$CI_ASK" ] && ok || bad "settings set ciPreview: last stderr line is '$(last_err_line)'"
[ "$(printf '%s\n' "$ERR" | grep -c '^Ask with:')" = 1 ] && ok || bad "settings set ciPreview: the closing line must print once"
<<<"$ERR" grep -q "ask the owner on a permission card" && ok || bad "settings set ciPreview: no card line: '$ERR'"
<<<"$ERR" grep -q "ask your human" && bad "settings set ciPreview: still says ask your human: '$ERR'" || ok
grep -q '^POST /cli/settings/update {"ciPreview":true}$' "$LOG" && ok || bad "settings set ciPreview: wrong request: $(cat "$LOG")"
run agent ci run
[ "$RC" = 3 ] && ok || bad "ci run off: exit $RC, want 3 ($ERR)"
<<<"$ERR" grep -q '^ci_preview_off: ' && ok || bad "ci run off: no code line: '$ERR'"
[ "$(last_err_line)" = "$CI_ASK" ] && ok || bad "ci run off: last stderr line is '$(last_err_line)'"
[ "$(printf '%s\n' "$ERR" | grep -c '^Ask with:')" = 1 ] && ok || bad "ci run off: the closing line must print once"

# The mail family (its own Python transport) ends the same way: an agent
# widening its own inbox access (M0) is the owner's, exit 3, line once.
run agent mail access grant ops@brand.test "$SANDBOX/ws" --level send
[ "$RC" = 3 ] && ok || bad "mail access grant: exit $RC, want 3 ($ERR)"
[ "$(last_err_line)" = "It can't be requested." ] && ok || bad "mail access grant: last stderr line is '$(last_err_line)'"
[ "$(printf '%s\n' "$ERR" | grep -c "It can't be requested.")" = 1 ] && ok || bad "mail access grant: the closing line must print once"
grep -q '^POST /cli/mail/access/grant ' "$LOG" && ok || bad "mail access grant: never reached the stub: $(cat "$LOG")"

# `k2 permission request` reaches the daemon and prints the ask.
run agent permission request compute.use --why "Run the test suite on k2n2-m1"
[ "$RC" = 0 ] && ok || bad "permission request: exit $RC ($ERR)"
<<<"$OUT" grep -q '^Asked: pr_7f3a2c (pending; expires in .*). Shown to: rosson (owner), maya (admin).$' && ok \
    || bad "permission request: '$OUT'"
grep -q '^POST /cli/permission/request {"action": "compute.use", "params": {}, "why": "Run the test suite on k2n2-m1"}$' "$LOG" && ok \
    || bad "permission request: wrong request: $(cat "$LOG")"
run agent permission request compute.use
[ "$RC" = 2 ] && ok || bad "permission request without --why: exit $RC, want 2"
# 0.46.16: --to names who is asked; it rides the request as `to`.
run agent permission request compute.use --why "Run the test suite" --to ada,owner
[ "$RC" = 0 ] && ok || bad "permission request --to: exit $RC ($ERR)"
grep -q '^POST /cli/permission/request {"action": "compute.use", "params": {}, "why": "Run the test suite", "to": "ada,owner"}$' "$LOG" && ok \
    || bad "permission request --to: wrong request: $(cat "$LOG")"
run agent permission request compute.use --why "x" --to
[ "$RC" = 2 ] && ok || bad "permission request --to with no value: exit $RC, want 2"
# 0.46.17: --assign is the same flag (k2 app's name for it), on every card.
: >"$LOG"
run agent permission request vault.send secret=MINE to=bob::heldbox --why "Bob needs it" --assign ada
[ "$RC" = 0 ] && ok || bad "permission request --assign: exit $RC ($ERR)"
grep -q '"to": "ada"}$' "$LOG" && ok || bad "permission request --assign rides as to: $(cat "$LOG")"
: >"$LOG"
run agent permission request compute.use --why "x" --assign=ada,owner
grep -q '"to": "ada,owner"}$' "$LOG" && ok || bad "permission request --assign=: $(cat "$LOG")"
run agent permission request compute.use --why "x" --assign
[ "$RC" = 2 ] && ok || bad "permission request --assign with no value: exit $RC, want 2"
# The field case: a login on the agent's own App, to one owner.
: >"$LOG"
run agent permission request app.login-grant op=add user=dana app=jobs role=viewer --why "Dana runs the jobs board" --assign olga
[ "$RC" = 0 ] && ok || bad "app.login-grant --assign: exit $RC ($ERR)"
grep -q '"action": "app.login-grant".*"to": "olga"}$' "$LOG" && ok || bad "app.login-grant --assign rides as to: $(cat "$LOG")"
# The owner's default, from a terminal outside K2: a settings write.
run person permission approvers ada,owner
grep -q '^POST /cli/settings/update {"permissionApprovers": \["ada", "owner"\]}$' "$LOG" && ok || bad "permission approvers: wrong request: $(cat "$LOG")"
run person permission approvers everyone
grep -q '^POST /cli/settings/update {"permissionApprovers": \[\]}$' "$LOG" && ok || bad "permission approvers everyone: wrong request: $(cat "$LOG")"
run agent permission approvers ada
[ "$RC" = 3 ] && ok || bad "permission approvers in a K2 terminal: exit $RC, want 3"

# People verbs are refused in a K2 terminal.
run agent permission pending
[ "$RC" = 3 ] && ok || bad "permission pending in a K2 terminal: exit $RC, want 3"

# From a terminal outside K2, the owner turns the switch on even while off.
echo off > "$MODE"
run person permission beta on
grep -q '^POST /cli/settings/update {"permissionCards":true}$' "$LOG" && ok || bad "permission beta on: wrong request: $(cat "$LOG")"
run person permission pending
<<<"$ERR" head -1 | grep -qx 'Unknown command: permission' && ok || bad "permission pending (person, off): '$ERR'"
echo on > "$MODE"

echo "== off (the daemon says no) =="
echo off > "$MODE"
run agent help
<<<"$OUT" grep -q 'permission request' && bad "k2 help: permission line while off" || ok
run agent permission request compute.use --why x
[ "$RC" = 1 ] && ok || bad "permission (off): exit $RC, want 1"
<<<"$ERR" head -1 | grep -qx 'Unknown command: permission' && ok || bad "permission (off): '$(printf '%s' "$ERR" | head -1)'"
run agent study permissions
[ "$RC" = 2 ] && ok || bad "study permissions (off): exit $RC, want 2"
run agent connections add billing
<<<"$ERR" grep -q 'Ask with:\|It can.t be requested' && bad "connections add (off): a closing line" || ok
run agent compute node policy k2n1-m4 set max_cpus=10
[ "$RC" = 2 ] && ok || bad "node policy held (off): exit $RC, want 2"
<<<"$ERR" grep -q "Ask with:\|It can.t be requested\|owner's chat" && bad "node policy held (off): a card line: $ERR" || ok
run agent compute access on
<<<"$ERR" grep -q 'Ask with:\|It can.t be requested' && bad "compute access on (off): a closing line" || ok
[ "$RC" = 3 ] && ok || bad "compute access on (off): exit $RC, want 3 (forbidden)"
<<<"$ERR" grep -q '"error": "forbidden"' && ok || bad "compute access on (off): '$ERR'"
run agent mail access grant ops@brand.test "$SANDBOX/ws" --level send
<<<"$ERR" grep -q 'Ask with:\|It can.t be requested' && bad "mail access grant (off): a closing line" || ok
[ "$RC" = 3 ] && ok || bad "mail access grant (off): exit $RC, want 3 (owner_only)"

echo "permission_teach: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
