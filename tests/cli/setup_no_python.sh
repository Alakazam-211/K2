#!/usr/bin/env bash
# k2 setup without python3: on a Mac without Apple's Command Line Tools
# /usr/bin/python3 is Apple's shim, which opens the installer instead of
# running, and that is the Mac the dev_tools step is for. Every k2 setup
# verb asks the daemon for its lines (`?format=text`) and prints them with
# bash and curl only; refusals (JSON) map to the same exit codes as before
# (2 usage, 3 refused, 4 no such Focus Group, 1 anything else). The POST bodies are built in bash,
# quotes and backslashes escaped. A python3 that fails loudly is first on
# PATH; the test fails if it ever runs.
#
# Nothing is installed: the daemon is a stub (python3 runs only the stub,
# from its full path). The CLI runs with a clean environment (`env -i`, a
# temp HOME), so it can only reach the stub.
# Run with: bash tests/cli/setup_no_python.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

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

MODE_FILE, LOG_FILE = os.environ["MODE_FILE"], os.environ["LOG_FILE"]
LIST = ("Setup: 1 of 3 done\n"
        "  [ ] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)\n"
        "  [x] Sign in to an AI tool\n"
        "  [-] Link your subdomain\n"
        "Next: meet K5, the agent that runs K2: k2 setup k5\n")
STATUS = {"enabled": True, "hidden": False, "finished": False,
          "rows": [{"id": "dev_tools", "state": "todo", "opens": "dev_tools"}], "offer": "meet_k5"}
MADE = "Made K5: K5 (k5) at /home/me/k2-agents/k5\nK5 is starting and will say hello on its Thread: k2 thread k5\n"

def refusal(code, hint):
    return json.dumps({"ok": False, "error": {"code": code, "hint": hint}}, separators=(",", ":"))

ERRORS = {
    "agent": (403, refusal("first_run_person_only", "the setup list belongs to a person: sign in with your own login (agents, app passes and API keys have none)"), "application/json"),
    "unknown": (400, refusal("unknown_row", '"nope" is not a row on your setup list (k2 setup shows them)'), "application/json"),
    "signed_out": (400, refusal("tool_not_signed_in", "grok isn't signed in on this server: sign in first"), "application/json"),
    "off": (404, refusal("k5_first_run_off", "K5 at first run is off on this server (the owner turns on k5FirstRun)"), "application/json"),
    "seat": (403, refusal("k5_session_only", "a shell tab is a person: ask K5 on its Thread to hire your Chief of Staff"), "application/json"),
    "role": (403, json.dumps({"error": "role_required", "message": "This server gives you Voice & Chat only."}, separators=(",", ":")), "application/json"),
    "bad_token": (403, '{"error":"invalid or missing token"}', "application/json"),
    "no_group": (400, refusal("unknown_focus_group", 'no focus group is "Nope": nothing was made (Settings \u2192 Workspaces lists them)'), "application/json"),
    "two_groups": (400, refusal("ambiguous_focus_group", 'more than one focus group is named "Ops": send its id'), "application/json"),
    "store": (500, refusal("store", "disk full\twhile saving \\ the list"), "application/json"),
    "garbage": (502, "<html>bad gateway</html>", "text/html"),
}

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
        if q.get("token") != ["t.k/+= z"]:
            return self._send(403, '{"error":"invalid or missing token"}', "application/json")
        if mode in ERRORS:
            return self._send(*ERRORS[mode])
        if q.get("format") != ["text"]:
            return self._send(200, json.dumps(STATUS, separators=(",", ":")), "application/json")
        if u.path in ("/cli/first-run/k5", "/cli/first-run/chief-of-staff"):
            return self._send(200, MADE, "text/plain; charset=utf-8")
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
MODE_FILE="$MODE_FILE" LOG_FILE="$LOG_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -s --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/first-run/status" >/dev/null 2>&1 && break
    sleep 0.1
done
: >"$LOG_FILE"

# The failing python3 is first on PATH; nothing else from this shell's env.
k2() { (cd "$WORK" && env -i PATH="$SHIMS:/usr/bin:/bin" HOME="$WORK/home" K2_PORT="$PORT" K2_HOOK_TOKEN='t.k/+= z' "$K2_CLI" "$@"); }
run() {
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
last_request() { tail -n 1 "$LOG_FILE"; }
Q='token=t.k%2F%2B%3D%20z&format=text'
LIST="$(printf '%s\n' \
    "Setup: 1 of 3 done" \
    "  [ ] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)" \
    "  [x] Sign in to an AI tool" \
    "  [-] Link your subdomain" \
    "Next: meet K5, the agent that runs K2: k2 setup k5")"

echo "== the list =="
run setup
assert_eq "setup: exit 0" "$rc" "0"
assert_eq "setup: GET, token encoded, text" "$(last_request)" "GET /cli/first-run/status $Q "
assert_eq "setup: the daemon's lines verbatim" "$out" "$LIST"
assert_eq "setup: nothing on stderr" "$err" ""

echo "== --json =="
run setup --json
assert_eq "--json: exit 0" "$rc" "0"
assert_eq "--json: no format=text" "$(last_request)" "GET /cli/first-run/status token=t.k%2F%2B%3D%20z "
assert_eq "--json: the body as sent" "$out" '{"enabled":true,"hidden":false,"finished":false,"rows":[{"id":"dev_tools","state":"todo","opens":"dev_tools"}],"offer":"meet_k5"}'

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

echo "== k5 and chief-of-staff =="
run setup k5
assert_eq "k5: exit 0" "$rc" "0"
assert_eq "k5: no flags, {}" "$(last_request)" "POST /cli/first-run/k5 $Q {}"
assert_eq "k5: the daemon's lines" "$out" "$(printf 'Made K5: K5 (k5) at /home/me/k2-agents/k5\nK5 is starting and will say hello on its Thread: k2 thread k5')"
run setup k5 --tool claude --name 'K "Five" \ 5' --handle k5
assert_eq "k5: the fields, escaped" "$(last_request)" "POST /cli/first-run/k5 $Q {\"tool\": \"claude\", \"name\": \"K \\\"Five\\\" \\\\ 5\", \"handle\": \"k5\"}"
run setup chief-of-staff --name Ada --about "$(printf 'runs "ops"\tand\nbooks')" --tool codex
assert_eq "chief-of-staff: exit 0" "$rc" "0"
assert_eq "chief-of-staff: about escaped" "$(tail -n 1 "$LOG_FILE")" "POST /cli/first-run/chief-of-staff $Q {\"tool\": \"codex\", \"name\": \"Ada\", \"about\": \"runs \\\"ops\\\"\\tand\\nbooks\"}"
run setup chief-of-staff --name Ada --about "" --tool codex
assert_eq "chief-of-staff: an empty about is sent" "$(last_request)" "POST /cli/first-run/chief-of-staff $Q {\"tool\": \"codex\", \"name\": \"Ada\", \"about\": \"\"}"
run setup k5 --focus-group "Ann's Ops"
assert_eq "k5 --focus-group: exit 0" "$rc" "0"
assert_eq "k5 --focus-group: focusGroup sent" "$(last_request)" "POST /cli/first-run/k5 $Q {\"focusGroup\": \"Ann's Ops\"}"
run setup k5 --tool claude --handle k5 --focus-group 'Bo"s \ crew'
assert_eq "k5 --focus-group: last, escaped" "$(last_request)" "POST /cli/first-run/k5 $Q {\"tool\": \"claude\", \"handle\": \"k5\", \"focusGroup\": \"Bo\\\"s \\\\ crew\"}"
run setup k5 --focus-group ""
assert_eq "k5 --focus-group empty: left out" "$(last_request)" "POST /cli/first-run/k5 $Q {}"
run setup chief-of-staff --name Ada --about "" --tool codex --focus-group 'fg_"1"'
assert_eq "chief-of-staff --focus-group: exit 0" "$rc" "0"
assert_eq "chief-of-staff --focus-group: after about, escaped" "$(last_request)" "POST /cli/first-run/chief-of-staff $Q {\"tool\": \"codex\", \"name\": \"Ada\", \"about\": \"\", \"focusGroup\": \"fg_\\\"1\\\"\"}"

echo "== usage (no daemon call) =="
: >"$LOG_FILE"
run setup skip
assert_eq "skip without a row: exit 2" "$rc" "2"
run setup chief-of-staff --name Ada
assert_eq "chief-of-staff without --about/--tool: exit 2" "$rc" "2"
run setup hide --tool claude
assert_eq "--tool off k5: exit 2" "$rc" "2"
run setup skip dev_tools --focus-group Ops
assert_eq "--focus-group off k5: exit 2" "$rc" "2"
run setup --bogus
assert_eq "unknown flag: exit 2" "$rc" "2"
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
refused signed_out 2 "Error: tool_not_signed_in: grok isn't signed in on this server: sign in first" k5 --tool grok
refused off 1 "Error: k5_first_run_off: K5 at first run is off on this server (the owner turns on k5FirstRun)" k5
refused seat 3 "Error: k5_session_only: a shell tab is a person: ask K5 on its Thread to hire your Chief of Staff" chief-of-staff --name A --about b --tool claude
refused role 3 "Error: role_required"
refused bad_token 3 "Error: invalid or missing token"
refused no_group 4 'Error: unknown_focus_group: no focus group is "Nope": nothing was made (Settings → Workspaces lists them)' k5 --focus-group Nope
refused two_groups 4 'Error: ambiguous_focus_group: more than one focus group is named "Ops": send its id' chief-of-staff --name A --about b --tool claude --focus-group Ops
refused store 1 "$(printf 'Error: store: disk full\twhile saving \\ the list')"
refused garbage 1 "Error: the daemon sent something k2 setup does not understand: <html>bad gateway</html>"
echo agent >"$MODE_FILE"
run setup --json
assert_eq "agent --json: exit 3" "$rc" "3"
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
