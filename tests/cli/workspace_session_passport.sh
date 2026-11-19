#!/usr/bin/env bash
# The workspace verbs in an agent's session send the session's passport,
# never the owner token. `workspace` is an Open verb, so inside a K2
# terminal the CLI's Open-verb rule reads the owner token from disk; these
# verbs must not send it. In a session (the K2_HOOK_SOCK marker, any of its
# names) `k2 workspace list` (--running, --json), `create`, `open`,
# `launch`, `profile` and `update`, and their `k2 agent profile|update`
# twins, send the passport as their Bearer header on every request (the
# lookups too), and nothing when the session has none; the owner token is
# never in a header or the query. In the owner's own terminal (no session)
# the owner token is sent, as before.
#
# The daemon admits no passport on these routes, for any workspace (they
# are not agent verbs: the TCP catch-all and the cell socket both answer
# 403 "Invalid or missing auth token"). The stub answers the same, so an
# `update` aimed at another workspace is refused.
#
# Nothing is installed: the daemon is a stub that logs each request's
# method, path, query and Bearer header. The CLI runs with a clean
# environment (`env -i`, a temp HOME holding a readable owner token), so it
# can only reach the stub.
# Run with: bash tests/cli/workspace_session_passport.sh

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

WORK="$(mktemp -d -t k2-ws-passport-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-tok-5d1e7a"
PASSPORT="sess17.passport3c9f"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

LOG_FILE = os.environ["LOG_FILE"]
OWNER = os.environ["OWNER"]
PASSPORT = os.environ["PASSPORT"]
WS = os.environ["WS"]
REFUSED = '{"error":"Invalid or missing auth token"}'
OWNER_ANSWERS = {
    "/cli/projects/list": json.dumps([{"name": "acme", "agentMode": "custom", "path": WS, "agentEnabled": True}]),
    "/cli/agent/list": json.dumps({"ok": True, "agents": [{"name": "acme", "path": WS, "enabled": True}]}),
    "/cli/workspace/create": json.dumps({"success": True, "path": WS}),
    "/cli/workspace/open": json.dumps({"success": True, "path": WS}),
    "/cli/workspace/resolve": json.dumps({"path": WS}),
    "/cli/workspace/ensure-canonical-session": json.dumps({"success": True, "session_id": "s-1", "agent": "acme", "project_id": "p-1", "reused": False, "pending_drained": 0}),
    "/cli/workspace/agent-display-name": json.dumps({"display_name": "Acme", "handle": "acme"}),
    "/cli/agent/update": json.dumps({"success": True, "content": "updated"}),
}

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _answer(self, method):
        u = urlsplit(self.path)
        auth = self.headers.get("Authorization")
        with open(LOG_FILE, "a") as f:
            f.write(json.dumps({"method": method, "path": u.path, "query": u.query, "auth": auth}) + "\n")
        if auth == "Bearer " + OWNER and u.path in OWNER_ANSWERS:
            return self._send(200, OWNER_ANSWERS[u.path])
        # A passport: whoami (an agent verb) knows it; every workspace
        # route refuses it, whichever workspace it names (the daemon's
        # catch-all answer).
        if auth == "Bearer " + PASSPORT and u.path == "/cli/whoami":
            return self._send(200, json.dumps({"ok": True, "workspace": "acme"}))
        return self._send(403, REFUSED)

    def do_GET(self):
        self._answer("GET")

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        self._answer("POST")

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

mkdir -p "$WORK/ws/.k2" "$WORK/other/.k2"
WS="$WORK/ws"
PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
: >"$LOG_FILE"
LOG_FILE="$LOG_FILE" PORT="$PORT" OWNER="$OWNER" PASSPORT="$PASSPORT" WS="$WS" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/cli/whoami"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

# The owner token on disk, readable, as in a K2 terminal on the owner's box.
mkdir -p "$WORK/home/.k2"
printf '%s' "$OWNER" >"$WORK/home/.k2/heartbeat.token"
chmod 600 "$WORK/home/.k2/heartbeat.token"
printf '%s' "$PORT" >"$WORK/home/.k2/heartbeat.port"
# The session marker names a socket; no socket is there (these verbs use TCP).
SOCK="$WORK/cell.sock"

run() { # env words..., then --, then the k2 args
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$LOG_FILE"
    set +e
    out="$(cd "$WS" && env -i PATH="/usr/bin:/bin" HOME="$WORK/home" K2_PROJECT_PATH="$WS" ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
# field <name> [<path>]: that field of every logged request (or of those
# to that path), one per line.
field() {
    "$PY" -c 'import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if len(sys.argv) > 3 and r.get("path") != sys.argv[3]:
        continue
    print(repr(r.get(sys.argv[2])))' "$LOG_FILE" "$@"
}
requests() { grep -c '"path": "/cli/' "$LOG_FILE" || true; }
owner_never_sent() { # label
    if grep -Fq -- "$OWNER" "$LOG_FILE"; then
        bad "$1: the owner token was sent ($(grep -F -- "$OWNER" "$LOG_FILE" | head -n 1))"
    else
        ok "$1: the owner token is in no header and no query"
    fi
}
no_query_credential() { # label
    case "$(field query)" in
        *token*) bad "$1: a credential in the query: $(field query)" ;;
        *) ok "$1: no credential in the query" ;;
    esac
}

# Every verb, as "label|args" (args split on spaces).
VERBS=(
    "workspace list|workspace list"
    "workspace list --running|workspace list --running"
    "workspace list --json|workspace list --json"
    "workspace create|workspace create $WORK/newws"
    "workspace open|workspace open $WS"
    "workspace launch <path>|workspace launch $WS"
    "workspace launch <name>|workspace launch acme"
    "workspace profile|workspace profile"
    "workspace update|workspace update --field role --value Lead"
    "agent profile|agent profile"
    "agent update|agent update --name acme --field role --value Lead"
)

check_sends() { # case label, want auth repr, env words...
    local label="$1" want="$2"; shift 2
    local v name args
    for v in "${VERBS[@]}"; do
        name="${v%%|*}"; args="${v#*|}"
        # shellcheck disable=SC2086
        run "$@" -- $args
        if [ "$(requests)" -ge 1 ]; then ok "$label: $name: reached the daemon"; else bad "$label: $name: no request (rc=$rc err=$err)"; fi
        assert_eq "$label: $name: every request's Bearer header" "$(field auth | sort -u)" "$want"
        no_query_credential "$label: $name"
        owner_never_sent "$label: $name"
    done
}

echo "== an agent's session, owner token readable on disk: the passport =="
check_sends "session" "'Bearer $PASSPORT'" K2_PORT="$PORT" K2_HOOK_SOCK="$SOCK" K2_HOOK_TOKEN="$PASSPORT"

echo "== the same under the K5_ names =="
check_sends "session (K5_)" "'Bearer $PASSPORT'" K5_PORT="$PORT" K5_HOOK_SOCK="$SOCK" K5_HOOK_TOKEN="$PASSPORT"

echo "== a session with no passport: no credential, never the owner token =="
check_sends "no passport" "None" K2_PORT="$PORT" K2_HOOK_SOCK="$SOCK"

echo "== update aimed at another workspace: the passport, refused =="
run K2_PORT="$PORT" K2_HOOK_SOCK="$SOCK" K2_HOOK_TOKEN="$PASSPORT" K2_PROJECT_PATH="$WORK/other" -- workspace update --field role --value Lead
assert_eq "another workspace: the update's Bearer header" "$(field auth /cli/agent/update)" "'Bearer $PASSPORT'"
assert_contains "another workspace: the update names it" "$(field query /cli/agent/update)" "project=$("$PY" -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$WORK/other")"
assert_eq "another workspace: refused, exit 1" "$rc" "1"
assert_eq "another workspace: nothing on stdout" "$out" ""
assert_contains "another workspace: the daemon's refusal on stderr" "$err" "Error: Invalid or missing auth token"
owner_never_sent "another workspace"

echo "== the owner's own terminal (no session): the owner token, as before =="
for v in "${VERBS[@]}"; do
    name="${v%%|*}"; args="${v#*|}"
    # shellcheck disable=SC2086
    run -- $args
    assert_eq "owner: $name: exit 0" "$rc" "0"
    if [ "$(requests)" -ge 1 ]; then ok "owner: $name: reached the daemon"; else bad "owner: $name: no request (err=$err)"; fi
    assert_eq "owner: $name: every request's Bearer header" "$(field auth | sort -u)" "'Bearer $OWNER'"
    no_query_credential "owner: $name"
done

echo
echo "workspace_session_passport: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
