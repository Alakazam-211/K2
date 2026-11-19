#!/usr/bin/env bash
# The agent opt-in (0.46.17) in the verbs the k5 front answers itself:
# k2 help, k2 study send-feedback and k2 setup agent-reports.
#
#   * an agent (a session passport) while the server's settings say
#     agents:false: k2 help doesn't list send-feedback, and
#     k2 study send-feedback is one line (exit 0), in --json too;
#   * the signal is the switch (agents), not canSend or why: with a drain
#     hold in why, agents:false still hides it and agents:true shows it;
#   * one settings read per process;
#   * agents on, no daemon, or a refused read: the full help and page;
#   * a person outside K2 (the owner token): the full help and page, and no
#     settings read at all;
#   * k2 setup agent-reports: the same POST bodies as send-feedback
#     settings; --enabled and --agent-diagnostics aren't its flags.
#
# Hermetic: a throwaway HOME, every K2_*/K2SO_*/K5_* var unset, a python
# stub daemon on loopback that logs every request. No real daemon.
#
#   bash tests/cli/send_feedback_optin_native.sh      (fails loudly on any miss)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^(K2|K2SO|K5)_' | grep -v '^K5_CLI_UNDER_TEST=' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d /tmp/k2sfn-XXXXXX)"
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

# The stub. $MODE: what GET settings answers:
#   off       agents false, nothing held (why agents_off for an agent)
#   on        agents true
#   off-held  agents false, a drain hold in why (needs_connect)
#   on-held   agents true, a drain hold in why
#   refused   403 (the read fails)
# The permission probe answers off. Every request is logged: METHOD PATH BODY.
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

LOG, PORTF, MODE = sys.argv[1:4]
PARTS = [{"id": "computer", "label": "Computer size and free space"}]

def settings(mode):
    on = mode.startswith("on")
    v = {"enabled": True, "agents": on, "agentPlatform": False, "agentAttach": [], "agentDiagnostics": False,
         "agentScopes": ["cli"] if on else [], "agentsAsked": on, "agentsAskedAt": 1763538000000 if on else None,
         "agentsAskedBy": "owner" if on else None, "attachParts": PARTS, "rememberDiagnostics": False,
         "canEdit": False, "canSend": on, "endpoint": "https://feedback.example.test", "contactAddress": "",
         "pending": 0, "service": {}, "ok": True}
    if mode.endswith("-held"):
        v["canSend"] = False
        v["why"] = "needs_connect"
        v["message"] = "This server isn't paired with K2 Connect."
    elif not on:
        v["why"] = "agents_off"
        v["message"] = "Agents don't report K2 problems on this server: a person hasn't allowed it."
    return v

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
        if path == "/cli/permission/enabled":
            return self.reply(200, {"enabled": False})
        if path == "/cli/product-feedback/settings":
            if mode == "refused":
                return self.reply(403, {"ok": False, "error": "unauthorized", "message": "no"})
            return self.reply(200, settings(mode))
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
: > "$LOG"
echo off > "$MODE"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$MODE" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2" "$SANDBOX/ws"
echo "$PORT" > "$HOME/.k2/heartbeat.port"
echo owner-native > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $*" >&2; fail=$((fail + 1)); }

# run <who> <args…>: who = person (a terminal outside K2: the owner token
# on disk) | agent (a session passport over TCP) | agent-nodaemon (a
# passport, nothing listening). Sets OUT, ERR, RC; the log starts empty.
run() {
    local who="$1"; shift
    : > "$LOG"
    set +e
    case "$who" in
        person) "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"; RC=$? ;;
        agent) K2_PORT="$PORT" K2_HOOK_TOKEN=passport-native "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"; RC=$? ;;
        agent-nodaemon) K2_PORT=1 K2_HOOK_TOKEN=passport-native "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"; RC=$? ;;
    esac
    set -e
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
}
reqs() { awk -v p="$1" '$2 == p { print $1 " " substr($0, length($1) + length($2) + 3) }' "$LOG"; }
want_rc() { [ "$RC" = "$1" ] && ok || bad "$2: exit $RC, want $1 (stderr: $ERR)"; }
want_has() { case "$2" in *"$3"*) ok ;; *) bad "$1: missing $(printf %q "$3") in: $2" ;; esac; }
want_not() { case "$2" in *"$3"*) bad "$1: unexpected $(printf %q "$3") in: $2" ;; *) ok ;; esac; }
want_eq() { [ "$2" = "$3" ] && ok || bad "$1: got $(printf %q "$2"), want $(printf %q "$3")"; }
S=/cli/product-feedback/settings
# The help text without the K2 ITSELF block: its blank line, the heading
# and the three send-feedback lines.
minus_block() {
    python3 - "$1" <<'PY2'
import sys
lines = sys.argv[1].split("\n")
i = lines.index("K2 ITSELF")
assert lines[i - 1] == "" and lines[i + 1].startswith("  send-feedback "), lines[i - 1:i + 4]
del lines[i - 1:i + 4]
print("\n".join(lines))
PY2
}
OFF_LINE="Agents don't report K2 problems on this server: a person turns it on in Settings, then Feedback."
SF_BLOCK='K2 ITSELF
  send-feedback "<text>"                Tell Alakazam Labs (K2'"'"'s makers) about K2: a bug, an'
PAGE_HEAD="k2 study send-feedback — telling K2's makers about K2"

# The person's help: the reference every other view is held against.
run person help
want_rc 0 "person: help"
PERSON_HELP="$OUT"
want_has "person: help lists send-feedback" "$PERSON_HELP" "$SF_BLOCK"
want_eq "person: help asks no settings" "$(reqs $S)" ""
run person study send-feedback
want_rc 0 "person: study send-feedback"
PERSON_PAGE="$OUT"
want_eq "person: the full page" "$(printf '%s\n' "$PERSON_PAGE" | head -1)" "$PAGE_HEAD"
want_eq "person: study asks no settings" "$(reqs $S)" ""

echo "== an agent while agents are off =="
for mode in off off-held; do
    echo "$mode" > "$MODE"
    run agent help
    want_rc 0 "$mode: help"
    want_not "$mode: help doesn't list send-feedback" "$OUT" "send-feedback"
    want_not "$mode: help has no K2 ITSELF" "$OUT" "K2 ITSELF"
    want_eq "$mode: one settings read" "$(reqs $S)" "GET "
    # Exactly the person's help without the K2 ITSELF block (its blank line
    # and four lines): nothing else moves.
    want_eq "$mode: help is the person's minus the block" "$OUT" "$(minus_block "$PERSON_HELP")"
    run agent study send-feedback
    want_rc 0 "$mode: study send-feedback"
    want_eq "$mode: study is one line" "$OUT" "$OFF_LINE"
    want_eq "$mode: one settings read for study" "$(reqs $S)" "GET "
    run agent study send-feedback --json
    want_rc 0 "$mode: study --json"
    want_eq "$mode: study --json is the one line" "$OUT" "{\"id\":\"send-feedback\",\"body\":\"Agents don't report K2 problems on this server: a person turns it on in Settings, then Feedback.\"}"
done

echo "== an agent while agents are on, or when the read fails =="
for case_ in on on-held refused nodaemon; do
    who=agent
    if [ "$case_" = nodaemon ]; then who=agent-nodaemon; else echo "$case_" > "$MODE"; fi
    run "$who" help
    want_rc 0 "$case_: help"
    want_eq "$case_: help is the person's" "$OUT" "$PERSON_HELP"
    run "$who" study send-feedback
    want_rc 0 "$case_: study send-feedback"
    want_eq "$case_: the full page" "$OUT" "$PERSON_PAGE"
done

echo "== a person while agents are off =="
echo off > "$MODE"
run person help
want_eq "person, agents off: the same help" "$OUT" "$PERSON_HELP"
want_eq "person, agents off: no settings read" "$(reqs $S)" ""
run person study send-feedback
want_eq "person, agents off: the full page" "$OUT" "$PERSON_PAGE"

echo "== k2 setup agent-reports =="
echo on > "$MODE"
run person setup agent-reports --agents all --attach computer
want_rc 0 "setup agent-reports --agents all --attach computer"
want_eq "setup agent-reports: the version read, then one write" "$(reqs $S)" \
    "$(printf '%s\n%s' 'GET ' 'POST {"agents": true, "agentPlatform": true, "agentAttach": "computer"}')"
want_has "setup agent-reports: the read after" "$OUT" "  agents report:      problems with K2 commands"
run person setup agent-reports --agents off
want_eq "setup agent-reports --agents off" "$(reqs $S)" 'POST {"agents": false}'
run person setup agent-reports --agents cli --json
want_eq "setup agent-reports --agents cli" "$(reqs $S | tail -1)" 'POST {"agents": true, "agentPlatform": false}'
python3 - "$OUT" <<'PY' && ok || bad "setup agent-reports --json is not the daemon's answer: $OUT"
import json, sys
assert json.loads(sys.argv[1])["agentsAskedBy"] == "owner"
PY
run person setup agent-reports
want_rc 0 "setup agent-reports, no flags"
want_eq "setup agent-reports, no flags: one read" "$(reqs $S)" "GET "
want_has "setup agent-reports, no flags: who answered" "$OUT" "  answered:           by the owner"
for flags in "--enabled off" "--agent-diagnostics on" "--agents maybe"; do
    # shellcheck disable=SC2086
    run person setup agent-reports $flags
    want_rc 2 "setup agent-reports $flags"
    want_eq "setup agent-reports $flags asks nothing" "$(cat "$LOG")" ""
done

echo
echo "send_feedback_optin_native: $pass passed, $fail failed"
[ "$fail" = 0 ]
