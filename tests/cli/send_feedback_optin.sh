#!/usr/bin/env bash
# k2 send-feedback and the agent opt-in (0.46.17): the flags, the bodies they
# send, the read, and how the daemon's refusals map to exit codes.
#
#   * settings --agents off|cli|all (on = cli), --attach, --agent-diagnostics,
#     --enabled: each one's exact POST body; --attach with
#     --agent-diagnostics is a usage error before any request;
#   * a daemon older than 0.46.17 (no agentsAsked or attachParts): --agents
#     cli|all and --attach say it needs the update, exit 1, no POST; on and
#     off still work, with the old bodies;
#   * the plain read: what agents report, who answered and when (or "not
#     asked yet"), each part on or off; --json is the daemon's answer;
#   * send --scope cli|platform and --command: the body; --scope cli without
#     --command, a --command with a new line and a bad --scope are usage
#     errors (exit 2) before any request;
#   * the daemon's refusals: agents_off and scope_off exit 3 with the closing
#     line; command_required, bad_command and bad_scope exit 2;
#   * list and show print a row's scope; a row without one is as before;
#   * the product name this part prints is the brand file's.
#
# The verbs here are bash in the k5 front too (send-feedback falls through).
# help, study and setup agent-reports, which k5 answers itself, are in
# send_feedback_optin_native.sh.
#
# Hermetic: a throwaway HOME, every K2_*/K2SO_*/K5_* var unset, a python
# stub daemon on loopback that logs every request. No real daemon.
#
#   bash tests/cli/send_feedback_optin.sh      (fails loudly on any miss)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^(K2|K2SO|K5)_' | grep -v '^K5_CLI_UNDER_TEST=' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d /tmp/k2sfo-XXXXXX)"
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

# The stub. $MODE: the settings the GET answers (new-off, new-on, old).
# $REFUSE: what POST send answers (ok, or a refusal code). Every request is
# logged as one line: METHOD PATH BODY.
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

LOG, PORTF, MODE, REFUSE = sys.argv[1:5]
PARTS = [
    {"id": "computer", "label": "Computer size and free space"},
    {"id": "k2-state", "label": "The names of settings that are on, and counts"},
    {"id": "agent-tools", "label": "Which AI tools are installed"},
    {"id": "recent-errors", "label": "Failure-looking lines from the last 24 hours"},
    {"id": "log-tail", "label": "The end of K2's log"},
]
COMMON = {"rememberDiagnostics": False, "canEdit": True, "canSend": True,
          "endpoint": "https://feedback.example.test", "contactAddress": "", "pending": 0,
          "service": {}, "ok": True}

def settings(mode):
    if mode == "old":
        v = {"enabled": True, "agents": True, "agentDiagnostics": False}
    elif mode == "new-on":
        v = {"enabled": True, "agents": True, "agentPlatform": True, "agentAttach": ["computer", "log-tail"],
             "agentDiagnostics": True, "agentScopes": ["cli", "platform"], "agentsAsked": True,
             "agentsAskedAt": 1763538000000, "agentsAskedBy": "owner", "attachParts": PARTS}
    else:
        v = {"enabled": True, "agents": False, "agentPlatform": False, "agentAttach": [],
             "agentDiagnostics": False, "agentScopes": [], "agentsAsked": False,
             "agentsAskedAt": None, "agentsAskedBy": None, "attachParts": PARTS}
    v.update(COMMON)
    return v

CANT = {"requestable": False, "where": "a person does this, in Settings"}
REFUSALS = {
    "agents_off": (403, {"ok": False, "error": "agents_off", "message": "Agents don't report K2 problems on this server: a person hasn't allowed it.", "permission": CANT}),
    "scope_off": (403, {"ok": False, "error": "scope_off", "message": "On this server agents report only K2 CLI problems (--scope cli, with the command you ran).", "permission": CANT}),
    "command_required": (400, {"ok": False, "error": "command_required", "message": "A cli report names the command you ran."}),
    "bad_command": (400, {"ok": False, "error": "bad_command", "message": "command is the one line you ran, at most 1000 bytes."}),
    "bad_scope": (400, {"ok": False, "error": "bad_scope", "message": "scope is cli or platform."}),
}

ROW = {"id": "pf_aaaaaaaaaaaaaaaaaaaaaa", "state": "sent", "category": "bug", "text": "help says --json",
       "createdAt": 1763538000000, "sentAt": 1763538001000, "remoteId": "r1", "sender": "agent in acme",
       "senderKind": "agent", "client": "cli", "mine": True, "scope": "cli"}
ROW2 = dict(ROW, id="pf_bbbbbbbbbbbbbbbbbbbbbb", text="a person's note", senderKind="person", scope=None)

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
        mode = open(MODE).read().strip()
        if path == "/cli/product-feedback/settings":
            return self.reply(200, settings(mode))
        if path == "/cli/product-feedback/send":
            r = open(REFUSE).read().strip()
            if r in REFUSALS:
                return self.reply(*REFUSALS[r])
            return self.reply(200, {"ok": True, "id": ROW["id"], "state": "queued"})
        if path == "/cli/product-feedback/show":
            q = urlparse(self.path).query
            return self.reply(200, {"ok": True, "item": ROW2 if ROW2["id"] in q else ROW})
        if path == "/cli/product-feedback/list":
            return self.reply(200, {"ok": True, "items": [ROW, ROW2], "all": False})
        return self.reply(404, {"ok": False, "error": "not_found", "message": "no such route in the stub: " + path})
    do_GET = handle_any
    do_POST = handle_any

srv = HTTPServer(("127.0.0.1", 0), H)
with open(PORTF, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
MODE="$SANDBOX/mode"
REFUSE="$SANDBOX/refuse"
: > "$LOG"
echo new-off > "$MODE"
echo ok > "$REFUSE"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$MODE" "$REFUSE" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

# A person in a terminal outside K2: the owner credential on disk.
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2" "$SANDBOX/ws"
echo "$PORT" > "$HOME/.k2/heartbeat.port"
echo owner-optin > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $*" >&2; fail=$((fail + 1)); }

# run <who> <args…>: who = person (owner token) | agent (a session passport
# over TCP). Sets OUT, ERR, RC; the request log starts empty.
run() {
    local who="$1"; shift
    : > "$LOG"
    set +e
    case "$who" in
        person) TZ=UTC "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"; RC=$? ;;
        agent) TZ=UTC K2_PORT="$PORT" K2_HOOK_TOKEN=passport-optin "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"; RC=$? ;;
    esac
    set -e
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
}

# The requests the last run made to one path, as "METHOD BODY" lines.
reqs() { awk -v p="$1" '$2 == p { print $1 " " substr($0, length($1) + length($2) + 3) }' "$LOG"; }
want_rc() { [ "$RC" = "$1" ] && ok || bad "$2: exit $RC, want $1 (stderr: $ERR)"; }
want_has() { case "$2" in *"$3"*) ok ;; *) bad "$1: missing $(printf %q "$3") in: $2" ;; esac; }
want_not() { case "$2" in *"$3"*) bad "$1: unexpected $(printf %q "$3") in: $2" ;; *) ok ;; esac; }
want_eq() { [ "$2" = "$3" ] && ok || bad "$1: got $(printf %q "$2"), want $(printf %q "$3")"; }
no_requests() { [ ! -s "$LOG" ] && ok || bad "$1: made requests: $(cat "$LOG")"; }
last_err_line() { printf '%s\n' "$ERR" | sed -e '/^$/d' | tail -1; }
S=/cli/product-feedback/settings

echo "== settings: each flag's body =="
run person send-feedback settings --agents off
want_rc 0 "--agents off"
want_eq "--agents off: one POST, agents false, nothing else" "$(reqs $S)" 'POST {"agents": false}'
run person send-feedback settings --agents cli
want_rc 0 "--agents cli"
want_eq "--agents cli: the version read, then agents on and platform off" "$(reqs $S)" "$(printf '%s\n%s' 'GET ' 'POST {"agents": true, "agentPlatform": false}')"
run person send-feedback settings --agents all
want_eq "--agents all: both on" "$(reqs $S | tail -1)" 'POST {"agents": true, "agentPlatform": true}'
run person send-feedback settings --agents on
want_eq "--agents on is cli on a new server" "$(reqs $S | tail -1)" 'POST {"agents": true, "agentPlatform": false}'
run person send-feedback settings --attach computer,log-tail
want_rc 0 "--attach ids"
want_eq "--attach ids: as typed" "$(reqs $S | tail -1)" 'POST {"agentAttach": "computer,log-tail"}'
run person send-feedback settings --attach none
want_eq "--attach none" "$(reqs $S | tail -1)" 'POST {"agentAttach": "none"}'
run person send-feedback settings --agents cli --attach all
want_eq "--agents cli --attach all: one write" "$(reqs $S | tail -1)" 'POST {"agents": true, "agentPlatform": false, "agentAttach": "all"}'
run person send-feedback settings --agent-diagnostics on
want_eq "--agent-diagnostics on: the older switch, no version read" "$(reqs $S)" 'POST {"agentDiagnostics": true}'
run person send-feedback settings --enabled off
want_eq "--enabled off" "$(reqs $S)" 'POST {"enabled": false}'

echo "== settings: usage mistakes ask nothing =="
run person send-feedback settings --attach computer --agent-diagnostics on
want_rc 2 "--attach with --agent-diagnostics"
want_has "--attach with --agent-diagnostics says not both" "$ERR" "not both"
no_requests "--attach with --agent-diagnostics"
run person send-feedback settings --agents sometimes
want_rc 2 "--agents sometimes"
want_has "--agents sometimes names the words" "$ERR" "off, cli or all"
no_requests "--agents sometimes"
run person send-feedback settings --attach ""
want_rc 2 "--attach with nothing"
no_requests "--attach with nothing"

echo "== settings: an older server =="
echo old > "$MODE"
for flags in "--agents cli" "--agents all" "--attach computer"; do
    # shellcheck disable=SC2086
    run person send-feedback settings $flags
    want_rc 1 "old server: $flags"
    want_eq "old server: $flags says it needs the update, in one line" "$ERR" \
        "This server's K2 is older: --agents cli|all and --attach need its update (k2 update). Nothing was changed."
    want_eq "old server: $flags reads the version and writes nothing" "$(reqs $S)" 'GET '
done
run person send-feedback settings --agents on
want_rc 0 "old server: --agents on"
want_eq "old server: --agents on is the old body" "$(reqs $S | tail -1)" 'POST {"agents": true}'
run person send-feedback settings --agents off
want_eq "old server: --agents off" "$(reqs $S)" 'POST {"agents": false}'
run person send-feedback settings
want_has "old server: the read keeps the old lines" "$OUT" "  agents:             on"
want_has "old server: the read keeps the old lines" "$OUT" "  agent diagnostics:  off"
want_not "old server: no opt-in lines" "$OUT" "answered:"

echo "== settings: the read =="
echo new-off > "$MODE"
run person send-feedback settings
want_rc 0 "read, not asked"
want_eq "read, not asked: the agent lines" "$(printf '%s\n' "$OUT" | sed -n '2,10p')" "  enabled:            on
  agents report:      nothing (off)
  answered:           not asked yet
  agents may attach:  (the versions always go)
    off  computer       Computer size and free space
    off  k2-state       The names of settings that are on, and counts
    off  agent-tools    Which AI tools are installed
    off  recent-errors  Failure-looking lines from the last 24 hours
    off  log-tail       The end of K2's log"
echo new-on > "$MODE"
run person send-feedback settings
want_has "read, answered: anything" "$OUT" "  agents report:      anything about K2"
want_has "read, answered: who and when" "$OUT" "  answered:           by the owner, 2025-11-19 07:40"
want_has "read, answered: a part on" "$OUT" "    on   computer       Computer size and free space"
want_has "read, answered: a part off" "$OUT" "    off  k2-state       The names of settings that are on, and counts"
want_has "read, answered: the last part on" "$OUT" "    on   log-tail       The end of K2's log"
run person send-feedback settings --json
want_rc 0 "read --json"
python3 - "$OUT" <<'PY' && ok || bad "settings --json is not the daemon's answer: $OUT"
import json, sys
v = json.loads(sys.argv[1])
assert v["agentsAskedBy"] == "owner" and v["agentAttach"] == ["computer", "log-tail"] and len(v["attachParts"]) == 5, v
PY

echo "== send: --scope and --command =="
run agent send-feedback "help says --json, the command refuses it" --scope cli --command "k2 msg list --json"
want_rc 0 "send --scope cli --command"
python3 - "$(reqs /cli/product-feedback/send)" <<'PY' && ok || bad "send body: $(reqs /cli/product-feedback/send)"
import json, sys
line = sys.argv[1]
assert line.startswith("POST "), line
b = json.loads(line[5:])
assert b["scope"] == "cli" and b["command"] == "k2 msg list --json" and b["text"].startswith("help says"), b
PY
run agent send-feedback "the Garden is slow" --scope platform
python3 - "$(reqs /cli/product-feedback/send)" <<'PY' && ok || bad "send --scope platform body: $(reqs /cli/product-feedback/send)"
import json, sys
b = json.loads(sys.argv[1][5:])
assert b["scope"] == "platform" and "command" not in b, b
PY
run person send-feedback "a person's note"
python3 - "$(reqs /cli/product-feedback/send)" <<'PY' && ok || bad "a person's send has no scope: $(reqs /cli/product-feedback/send)"
import json, sys
b = json.loads(sys.argv[1][5:])
assert "scope" not in b and "command" not in b, b
PY

echo "== send: usage mistakes ask nothing =="
run agent send-feedback "x" --scope cli
want_rc 2 "--scope cli without --command"
want_has "--scope cli without --command says so" "$ERR" "--command"
no_requests "--scope cli without --command"
run agent send-feedback "x" --scope cli --command "k2 a
k2 b"
want_rc 2 "--command with a new line"
want_has "--command with a new line says one line" "$ERR" "one line"
no_requests "--command with a new line"
run agent send-feedback "x" --scope everything
want_rc 2 "--scope everything"
no_requests "--scope everything"
run agent send-feedback "x" --scope cli --command "   "
want_rc 2 "--command blank"
no_requests "--command blank"

echo "== send: the daemon's refusals =="
for code in agents_off scope_off; do
    echo "$code" > "$REFUSE"
    run agent send-feedback "x" --scope platform
    want_rc 3 "$code"
    want_has "$code: the code on stderr" "$ERR" "\"error\": \"$code\""
    want_eq "$code: the closing line last" "$(last_err_line)" "It can't be requested."
done
for code in command_required bad_command bad_scope; do
    echo "$code" > "$REFUSE"
    run agent send-feedback "x" --scope cli --command "k2 msg list"
    want_rc 2 "$code"
    want_has "$code: the code on stderr" "$ERR" "\"error\": \"$code\""
done
echo ok > "$REFUSE"

echo "== list and show: the scope =="
run agent send-feedback list
want_rc 0 "list"
want_has "list: a row with a scope says it" "$OUT" "help says --json  (scope cli)"
row2="$(printf '%s\n' "$OUT" | grep pf_bbbb)"
want_not "list: a row without a scope is as before" "$row2" "scope"
run agent send-feedback show pf_aaaaaaaaaaaaaaaaaaaaaa
want_has "show: the scope line" "$OUT" "  scope:    cli"
run agent send-feedback show pf_bbbbbbbbbbbbbbbbbbbbbb
want_not "show: no scope line without one" "$OUT" "scope:"

echo "== the product name is the brand file's =="
brand="$(sed -n 's/^product = "\(.*\)"$/\1/p' "$PROJECT_ROOT/brand/k2/brand.toml")"
cli_name="$(sed -n 's/^_SF_PRODUCT="\(.*\)"$/\1/p' "$PROJECT_ROOT/cli/src/send-feedback.sh")"
[ -n "$brand" ] && ok || bad "no product name in brand/k2/brand.toml"
want_eq "_SF_PRODUCT is brand/k2/brand.toml's product" "$cli_name" "$brand"

echo
echo "send_feedback_optin: $pass passed, $fail failed"
[ "$fail" = 0 ]
