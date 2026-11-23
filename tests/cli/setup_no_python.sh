#!/usr/bin/env bash
# k2 setup without python3: on a Mac without Apple's Command Line Tools
# /usr/bin/python3 is Apple's shim, which opens the installer instead of
# running, and that is the Mac the dev_tools step is for. Every k2 setup
# verb asks the daemon for its lines (`?format=text`) and prints them with
# bash and curl only; refusals (JSON) map to the same exit codes as before
# (2 usage, 3 refused, 4 no such Focus Group, 1 anything else). The POST bodies are built in bash,
# quotes and backslashes escaped. A write that got no answer is sent once.
# k2 setup agent-reports reads the feedback settings' JSON and prints it
# in bash too. A python3 that fails loudly is first on PATH; the test fails
# if it ever runs.
#
# Nothing is installed: the daemon is a stub (python3 runs only the stub,
# from its full path). The CLI runs with a clean environment (`env -i`, a
# temp HOME), so it can only reach the stub.
# Run with: bash tests/cli/setup_no_python.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

WORK="$(mktemp -d -t k2-setup-no-python-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK/home/.k2"
MODE_FILE="$WORK/mode"
LOG_FILE="$WORK/requests"
AUTH_FILE="$WORK/last-auth"

# A python3 (and python) that fails loudly.
SHIMS="$WORK/shims"
PY_MARK="$WORK/python3-ran"
mkdir -p "$SHIMS"
cat >"$SHIMS/python3" <<SH
#!/bin/sh
: > '$PY_MARK'
echo 'python3 was called' >&2
exit 1
SH
chmod 755 "$SHIMS/python3"
cp "$SHIMS/python3" "$SHIMS/python"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

MODE_FILE, LOG_FILE, AUTH_FILE = os.environ["MODE_FILE"], os.environ["LOG_FILE"], os.environ["AUTH_FILE"]
LIST = ("Setup: 1 of 3 done\n"
        "  [ ] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)\n"
        "  [x] Sign in to an AI tool\n"
        "  [-] Link your subdomain\n"
        "Next: meet Kessel, the agent that runs K2: k2 setup kessel\n")
STATUS = {"enabled": True, "hidden": False, "finished": False,
          "rows": [{"id": "dev_tools", "state": "todo", "opens": "dev_tools"}], "offer": "meet_kessel"}
MADE = "Made Kessel: Kessel (kessel) at /home/me/k2-agents/kessel\nKessel is starting and will say hello on its Thread: k2 thread kessel\n"

# k2 setup agent-reports (0.46.17): /cli/product-feedback/settings, compact
# JSON (json.dumps escapes the arrow as \u2192), nulls, the service's own
# message.
PARTS = [["computer", "Computer size and free space"], ["k2-state", "The names of settings that are on, and counts"],
         ["agent-tools", "Which AI tools are installed"], ["recent-errors", "Failure-looking lines from the last 24 hours"],
         ["log-tail", "The end of K2's log"]]
PF_SETTINGS = {"enabled": True, "agents": True, "agentPlatform": True, "agentAttach": ["computer", "log-tail"],
               "agentDiagnostics": True, "agentScopes": ["cli", "platform"], "agentsAsked": True,
               "agentsAskedAt": 1763538000000, "agentsAskedBy": "owner",
               "attachParts": [{"id": i, "label": l} for i, l in PARTS], "rememberDiagnostics": False,
               "canEdit": True, "canSend": True, "why": None, "message": None,
               "endpoint": "https://feedback.example.test", "contactAddress": "", "senderKind": "person",
               "pending": 2, "service": {"state": "ok", "code": None, "message": "K2's feedback service is reachable \u2192 ok.", "at": 1}}
PF_ERRORS = {
    "pf_people": (403, json.dumps({"ok": False, "error": "settings_people_only", "message": "Only a person signed in to this server changes these settings."}, separators=(",", ":")), "application/json"),
    "pf_bad_part": (400, json.dumps({"ok": False, "error": "bad_attach_part", "message": "nope isn't a part agents may attach."}, separators=(",", ":")), "application/json"),
}

def refusal(code, hint):
    return json.dumps({"ok": False, "error": {"code": code, "hint": hint}}, separators=(",", ":"))

ERRORS = {
    "agent": (403, refusal("first_run_person_only", "the setup list belongs to a person: sign in with your own login (agents, app passes and API keys have none)"), "application/json"),
    "unknown": (400, refusal("unknown_row", '"nope" is not a row on your setup list (k2 setup shows them)'), "application/json"),
    "signed_out": (400, refusal("tool_not_signed_in", "grok isn't signed in on this server: sign in first"), "application/json"),
    "off": (404, refusal("kessel_first_run_off", "Kessel at first run is off on this server (the owner turns on kesselFirstRun)"), "application/json"),
    "seat": (403, refusal("kessel_session_only", "a shell tab is a person: ask Kessel on its Thread to hire your Chief of Staff"), "application/json"),
    "handle_taken": (409, refusal("kessel_handle_taken", "the handles kessel and kessel5 are both taken by other agents: pass another with --handle (k2 setup kessel --handle <h>), or free one of them"), "application/json"),
    "role": (403, json.dumps({"error": "role_required", "message": "This server gives you Voice & Chat only."}, separators=(",", ":")), "application/json"),
    "bad_token": (403, '{"error":"invalid or missing token"}', "application/json"),
    "no_group": (400, refusal("unknown_focus_group", 'no focus group is "Nope": nothing was made (Settings \u2192 Workspaces lists them)'), "application/json"),
    "two_groups": (400, refusal("ambiguous_focus_group", 'more than one focus group is named "Ops": send its id'), "application/json"),
    "store": (500, refusal("store", "disk full\twhile saving \\ the list"), "application/json"),
    "garbage": (502, "<html>bad gateway</html>", "text/html"),
    "no_kessel": (404, refusal("no_kessel", "this server has no Kessel, so there is no welcome to send again"), "application/json"),
    "asked": (403, json.dumps({"ok": False, "error": {"code": "role_required", "hint": "only the owner or an admin can turn the setup list on, so they were asked on a ticket. Your setup starts again here when they say yes"}, "asked": {"ticketId": "t-1", "already": False}}, separators=(",", ":")), "application/json"),
}
WELCOME = "Kessel is starting its welcome again on its Thread: k2 thread kessel\n"

class H(BaseHTTPRequestHandler):
    def _send(self, code, body, ctype):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _answer(self, method, body):
        u = urlsplit(self.path)
        with open(LOG_FILE, "a") as f:
            f.write("%s %s %s %s\n" % (method, u.path, u.query, body))
        q = parse_qs(u.query)
        mode = open(MODE_FILE).read().strip()
        # The credential rides the Authorization header only, the raw token
        # exactly as given (nothing encoded).
        auth = self.headers.get("Authorization")
        with open(AUTH_FILE, "w") as f:
            f.write("%r\n" % (auth,))
        if auth != "Bearer t.k/+= z":
            return self._send(403, '{"error":"invalid or missing token"}', "application/json")
        if mode == "close":
            self.close_connection = True
            return
        if mode == "empty":
            return self._send(200, "", "text/plain")
        if mode == "migrating":
            return self._send(503, '{"state":"migrating"}', "application/json")
        if mode in ERRORS:
            return self._send(*ERRORS[mode])
        if u.path == "/cli/product-feedback/settings":
            if mode in PF_ERRORS and method == "POST":
                return self._send(*PF_ERRORS[mode])
            return self._send(200, json.dumps(PF_SETTINGS, separators=(",", ":")), "application/json")
        if q.get("format") != ["text"]:
            return self._send(200, json.dumps(STATUS, separators=(",", ":")), "application/json")
        if u.path in ("/cli/first-run/kessel", "/cli/first-run/chief-of-staff"):
            return self._send(200, MADE, "text/plain; charset=utf-8")
        if u.path == "/cli/first-run/kessel/welcome":
            return self._send(200, WELCOME, "text/plain; charset=utf-8")
        return self._send(200, LIST, "text/plain; charset=utf-8")

    def do_GET(self):
        self._answer("GET", "")

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._answer("POST", self.rfile.read(n).decode())

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo ok >"$MODE_FILE"
: >"$LOG_FILE"
MODE_FILE="$MODE_FILE" LOG_FILE="$LOG_FILE" AUTH_FILE="$AUTH_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -s --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/first-run/status" >/dev/null 2>&1 && break
    sleep 0.1
done
: >"$LOG_FILE"

# The failing python3 is first on PATH; nothing else from this shell's env.
k2() { (cd "$WORK" && env -i PATH="$SHIMS:/usr/bin:/bin" HOME="$WORK/home" TZ=UTC K2_PORT="$PORT" K2_HOOK_TOKEN='t.k/+= z' "$K2_CLI" "$@"); }
run() {
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
last_request() { tail -n 1 "$LOG_FILE"; }
Q='format=text'
LIST="$(printf '%s\n' \
    "Setup: 1 of 3 done" \
    "  [ ] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)" \
    "  [x] Sign in to an AI tool" \
    "  [-] Link your subdomain" \
    "Next: meet Kessel, the agent that runs K2: k2 setup kessel")"

echo "== the list =="
run setup
assert_eq "setup: exit 0" "$rc" "0"
assert_eq "setup: GET, no token in the query, text" "$(last_request)" "GET /cli/first-run/status $Q "
assert_eq "setup: the raw token in the Bearer header" "$(cat "$AUTH_FILE")" "'Bearer t.k/+= z'"
assert_eq "setup: the daemon's lines verbatim" "$out" "$LIST"
assert_eq "setup: nothing on stderr" "$err" ""

echo "== --json =="
run setup --json
assert_eq "--json: exit 0" "$rc" "0"
assert_eq "--json: no query at all" "$(last_request)" "GET /cli/first-run/status  "
assert_eq "--json: the raw token in the Bearer header" "$(cat "$AUTH_FILE")" "'Bearer t.k/+= z'"
assert_eq "--json: the body as sent" "$out" '{"enabled":true,"hidden":false,"finished":false,"rows":[{"id":"dev_tools","state":"todo","opens":"dev_tools"}],"offer":"meet_kessel"}'

echo "== verbs that print the list =="
run setup skip dev_tools
assert_eq "skip: exit 0" "$rc" "0"
assert_eq "skip: POST the row" "$(last_request)" "POST /cli/first-run/skip $Q {\"row\": \"dev_tools\"}"
assert_eq "skip: the list" "$out" "$LIST"
run setup skip 'od"d\row'
assert_eq "skip: the row escaped" "$(last_request)" "POST /cli/first-run/skip $Q {\"row\": \"od\\\"d\\\\row\"}"
for verb in hide show; do
    run setup "$verb"
    assert_eq "$verb: exit 0" "$rc" "0"
    assert_eq "$verb: POST {}" "$(last_request)" "POST /cli/first-run/$verb $Q {}"
    assert_eq "$verb: the list" "$out" "$LIST"
done
run setup dismiss-offer
assert_eq "dismiss-offer: exit 0" "$rc" "0"
assert_eq "dismiss-offer: POST {}" "$(last_request)" "POST /cli/first-run/offer/dismiss $Q {}"

echo "== restart and kessel-welcome (Settings -> Getting started) =="
run setup restart
assert_eq "restart: exit 0" "$rc" "0"
assert_eq "restart: POST {}" "$(last_request)" "POST /cli/first-run/restart $Q {}"
assert_eq "restart: the list" "$out" "$LIST"
run setup restart --kessel
assert_eq "restart --kessel: exit 0" "$rc" "0"
assert_eq "restart --kessel: kessel true" "$(last_request)" "POST /cli/first-run/restart $Q {\"kessel\": true}"
: >"$LOG_FILE"
run setup --kessel restart
assert_eq "--kessel before restart: exit 2" "$rc" "2"
assert_eq "--kessel before restart: says so" "$err" "k2 setup: --kessel goes with k2 setup restart"
run setup show --kessel
assert_eq "--kessel off restart: exit 2" "$rc" "2"
assert_eq "--kessel: nothing sent" "$(cat "$LOG_FILE")" ""
run setup kessel-welcome
assert_eq "kessel-welcome: exit 0" "$rc" "0"
assert_eq "kessel-welcome: POST {}" "$(last_request)" "POST /cli/first-run/kessel/welcome $Q {}"
assert_eq "kessel-welcome: the daemon's line" "$out" "Kessel is starting its welcome again on its Thread: k2 thread kessel"
run setup kessel-welcome --json
assert_eq "kessel-welcome --json: no format" "$(last_request)" "POST /cli/first-run/kessel/welcome  {}"
run setup help
case "$out" in *"k2 setup restart [--kessel]"*) ok "help: k2 setup restart" ;; *) bad "help: no k2 setup restart" ;; esac
case "$out" in *"k2 setup kessel-welcome"*) ok "help: k2 setup kessel-welcome" ;; *) bad "help: no k2 setup kessel-welcome" ;; esac

echo "== kessel and chief-of-staff =="
run setup kessel
assert_eq "kessel: exit 0" "$rc" "0"
assert_eq "kessel: no flags, {}" "$(last_request)" "POST /cli/first-run/kessel $Q {}"
assert_eq "kessel: the daemon's lines" "$out" "$(printf 'Made Kessel: Kessel (kessel) at /home/me/k2-agents/kessel\nKessel is starting and will say hello on its Thread: k2 thread kessel')"
run setup kessel --tool claude --name Kay --handle kay --focus-group Ops
assert_eq "kessel: every flag" "$(last_request)" "POST /cli/first-run/kessel $Q {\"tool\": \"claude\", \"name\": \"Kay\", \"handle\": \"kay\", \"focusGroup\": \"Ops\"}"
run setup kessel --tool claude --name 'K "Five" \ 5' --handle kessel
assert_eq "kessel: the fields, escaped" "$(last_request)" "POST /cli/first-run/kessel $Q {\"tool\": \"claude\", \"name\": \"K \\\"Five\\\" \\\\ 5\", \"handle\": \"kessel\"}"
run setup chief-of-staff --name Ada --about "$(printf 'runs "ops"\tand\nbooks')" --tool codex
assert_eq "chief-of-staff: exit 0" "$rc" "0"
assert_eq "chief-of-staff: about escaped" "$(tail -n 1 "$LOG_FILE")" "POST /cli/first-run/chief-of-staff $Q {\"tool\": \"codex\", \"name\": \"Ada\", \"about\": \"runs \\\"ops\\\"\\tand\\nbooks\"}"
run setup chief-of-staff --name Ada --about "" --tool codex
assert_eq "chief-of-staff: an empty about is sent" "$(last_request)" "POST /cli/first-run/chief-of-staff $Q {\"tool\": \"codex\", \"name\": \"Ada\", \"about\": \"\"}"
run setup kessel --focus-group "Ann's Ops"
assert_eq "kessel --focus-group: exit 0" "$rc" "0"
assert_eq "kessel --focus-group: focusGroup sent" "$(last_request)" "POST /cli/first-run/kessel $Q {\"focusGroup\": \"Ann's Ops\"}"
run setup kessel --tool claude --handle kessel --focus-group 'Bo"s \ crew'
assert_eq "kessel --focus-group: last, escaped" "$(last_request)" "POST /cli/first-run/kessel $Q {\"tool\": \"claude\", \"handle\": \"kessel\", \"focusGroup\": \"Bo\\\"s \\\\ crew\"}"
run setup kessel --focus-group ""
assert_eq "kessel --focus-group empty: left out" "$(last_request)" "POST /cli/first-run/kessel $Q {}"
run setup chief-of-staff --name Ada --about "" --tool codex --focus-group 'fg_"1"'
assert_eq "chief-of-staff --focus-group: exit 0" "$rc" "0"
assert_eq "chief-of-staff --focus-group: after about, escaped" "$(last_request)" "POST /cli/first-run/chief-of-staff $Q {\"tool\": \"codex\", \"name\": \"Ada\", \"about\": \"\", \"focusGroup\": \"fg_\\\"1\\\"\"}"

echo "== agent-reports (the feedback opt-in, 0.46.17) =="
PF=/cli/product-feedback/settings
run setup agent-reports
assert_eq "agent-reports: exit 0" "$rc" "0"
assert_eq "agent-reports: GET, no query" "$(last_request)" "GET $PF  "
assert_eq "agent-reports: the raw token in the Bearer header" "$(cat "$AUTH_FILE")" "'Bearer t.k/+= z'"
assert_eq "agent-reports: the read, in bash" "$out" "$(printf '%s\n' \
    "Send feedback to K2 on this server" \
    "  enabled:            on" \
    "  agents report:      anything about K2" \
    "  answered:           by the owner, 2025-11-19 07:40" \
    "  agents may attach:  (the versions always go)" \
    "    on   computer       Computer size and free space" \
    "    off  k2-state       The names of settings that are on, and counts" \
    "    off  agent-tools    Which AI tools are installed" \
    "    off  recent-errors  Failure-looking lines from the last 24 hours" \
    "    on   log-tail       The end of K2's log" \
    "  can send now:       yes" \
    "  service:            K2's feedback service is reachable → ok." \
    "  waiting to send:    2" \
    "  sends to:           https://feedback.example.test")"
assert_eq "agent-reports: nothing on stderr" "$err" ""
: >"$LOG_FILE"
run setup agent-reports --agents all --attach 'computer,log-tail'
assert_eq "agent-reports --agents all --attach: exit 0" "$rc" "0"
# Built outside "$(…)": bash 3.2 keeps the backslash of \" inside one.
want_pf="GET $PF  "$'\n'"POST $PF  "'{"agents": true, "agentPlatform": true, "agentAttach": "computer,log-tail"}'
assert_eq "agent-reports --agents all --attach: the version read, then one write" "$(cat "$LOG_FILE")" "$want_pf"
run setup agent-reports --agents off --json
assert_eq "agent-reports --agents off --json: one write" "$(last_request)" "POST $PF  {\"agents\": false}"
case "$out" in '{"enabled":true,'*) ok "agent-reports --json: the daemon's answer" ;; *) bad "agent-reports --json: $out" ;; esac
echo pf_people >"$MODE_FILE"
run setup agent-reports --agents off
assert_eq "agent-reports refused: exit 3" "$rc" "3"
assert_eq "agent-reports refused: the line" "$err" '{"error": "settings_people_only", "hint": "Only a person signed in to this server changes these settings."}'
echo pf_bad_part >"$MODE_FILE"
run setup agent-reports --attach nope
assert_eq "agent-reports bad part: exit 2" "$rc" "2"
assert_eq "agent-reports bad part: the line" "$err" "{\"error\": \"bad_attach_part\", \"hint\": \"nope isn't a part agents may attach.\"}"
echo ok >"$MODE_FILE"

echo "== usage (no daemon call) =="
: >"$LOG_FILE"
run setup skip
assert_eq "skip without a row: exit 2" "$rc" "2"
run setup chief-of-staff --name Ada
assert_eq "chief-of-staff without --about/--tool: exit 2" "$rc" "2"
run setup hide --tool claude
assert_eq "--tool off kessel: exit 2" "$rc" "2"
run setup skip dev_tools --focus-group Ops
assert_eq "--focus-group off kessel: exit 2" "$rc" "2"
run setup --bogus
assert_eq "unknown flag: exit 2" "$rc" "2"
run setup kessel --about x
assert_eq "kessel --about: exit 2" "$rc" "2"
assert_eq "kessel --about: says so" "$err" "k2 setup kessel: no --about"
run setup --help
case "$out" in *"k2 setup kessel [--tool claude|codex|grok] [--name N] [--handle H] [--focus-group G]"*) ok "help: k2 setup kessel" ;; *) bad "help: no k2 setup kessel" ;; esac
# The old agent's own forms ("meet K5", "setup k5", "K5 (k5)", the k5FirstRun
# switch and the meet_k5 offer), not the letters K5: the command and the
# product may be named k5 one day, and the help may say so.
case "$out" in *"setup k5"*|*"meet K5"*|*"K5 (k5)"*|*k5FirstRun*|*meet_k5*) bad "help: still names the old agent" ;; *) ok "help: says Kessel only" ;; esac
run setup k5
assert_eq "k5 is no verb: exit 2" "$rc" "2"
assert_eq "usage: nothing sent" "$(cat "$LOG_FILE")" ""

echo "== refusals =="
refused() { # mode, verb..., then: want rc, want stderr
    local mode="$1" want_rc="$2" want_err="$3"; shift 3
    echo "$mode" >"$MODE_FILE"
    run setup "$@"
    assert_eq "$mode: exit $want_rc" "$rc" "$want_rc"
    assert_eq "$mode: the error" "$err" "$want_err"
    assert_eq "$mode: nothing on stdout" "$out" ""
}
refused agent 3 "Error: first_run_person_only: the setup list belongs to a person: sign in with your own login (agents, app passes and API keys have none)"
refused unknown 2 'Error: unknown_row: "nope" is not a row on your setup list (k2 setup shows them)' skip nope
refused signed_out 2 "Error: tool_not_signed_in: grok isn't signed in on this server: sign in first" kessel --tool grok
refused off 1 "Error: kessel_first_run_off: Kessel at first run is off on this server (the owner turns on kesselFirstRun)" kessel
refused seat 3 "Error: kessel_session_only: a shell tab is a person: ask Kessel on its Thread to hire your Chief of Staff" chief-of-staff --name A --about b --tool claude
refused handle_taken 1 "Error: kessel_handle_taken: the handles kessel and kessel5 are both taken by other agents: pass another with --handle (k2 setup kessel --handle <h>), or free one of them" kessel
refused role 3 "Error: role_required"
refused bad_token 3 "Error: invalid or missing token"
refused no_group 4 'Error: unknown_focus_group: no focus group is "Nope": nothing was made (Settings → Workspaces lists them)' kessel --focus-group Nope
refused two_groups 4 'Error: ambiguous_focus_group: more than one focus group is named "Ops": send its id' chief-of-staff --name A --about b --tool claude --focus-group Ops
refused store 1 "$(printf 'Error: store: disk full\twhile saving \\ the list')"
refused garbage 1 "Error: the daemon sent something k2 setup does not understand: <html>bad gateway</html>"
refused no_kessel 4 "Error: no_kessel: this server has no Kessel, so there is no welcome to send again" kessel-welcome
refused asked 3 "Error: role_required: only the owner or an admin can turn the setup list on, so they were asked on a ticket. Your setup starts again here when they say yes" restart
echo agent >"$MODE_FILE"
run setup --json
assert_eq "agent --json: exit 3" "$rc" "3"
echo ok >"$MODE_FILE"

echo "== a write is sent once =="
# A POST that may have reached the daemon is never sent again: one request
# for an answer that never came, an empty one, or a 503 while migrating.
for mode in close empty migrating; do
    echo "$mode" >"$MODE_FILE"
    : >"$LOG_FILE"
    run setup skip dev_tools
    assert_eq "$mode: exit 1" "$rc" "1"
    assert_eq "$mode: one request" "$(grep -c . "$LOG_FILE")" "1"
    assert_eq "$mode: nothing on stdout" "$out" ""
done
echo close >"$MODE_FILE"
run setup skip dev_tools
assert_eq "close: the line" "$err" "Error: could not reach the K2 daemon (k2 setup)"
echo migrating >"$MODE_FILE"
run setup skip dev_tools
assert_eq "migrating: the answer" "$err" 'Error: {"state":"migrating"}'
# The list (a GET) is asked again while the daemon migrates: four tries.
: >"$LOG_FILE"
run setup
assert_eq "migrating list: exit 1" "$rc" "1"
assert_eq "migrating list: four requests" "$(grep -c . "$LOG_FILE")" "4"
echo ok >"$MODE_FILE"

echo "== no daemon =="
kill "$STUB_PID" 2>/dev/null || true
wait "$STUB_PID" 2>/dev/null || true
STUB_PID=""
run setup
assert_eq "no daemon: exit 1" "$rc" "1"
assert_eq "no daemon: one line" "$err" "Error: could not reach the K2 daemon (k2 setup)"

echo "== python3 =="
if [ -e "$PY_MARK" ]; then bad "k2 setup ran python3 (Apple's shim on a fresh Mac)"; else ok "k2 setup never ran python3"; fi

echo
echo "setup_no_python: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
