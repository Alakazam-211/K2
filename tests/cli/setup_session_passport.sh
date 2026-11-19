#!/usr/bin/env bash
# k2 setup in an agent's session sends the session's passport, never the
# owner token. `setup` is an Open verb, so inside a K2 terminal the CLI's
# Open-verb rule reads the owner token from disk; k2 setup must not send
# it. In a session (the K2_HOOK_SOCK marker, any of its names) every setup
# verb sends the passport as its Bearer header, and nothing when the
# session has none; the owner token is never in a header or the query.
# In the owner's own terminal (no session) the owner token is sent, as
# before.
#
# Nothing is installed: the daemon is a stub that logs each request's
# method, path, query and headers. The CLI runs with a clean environment
# (`env -i`, a temp HOME holding a readable owner token), so it can only
# reach the stub.
# Run with: bash tests/cli/setup_session_passport.sh

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

WORK="$(mktemp -d -t k2-setup-passport-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-tok-9f3a1c"
PASSPORT="sess42.passport7b2e"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

LOG_FILE = os.environ["LOG_FILE"]
LIST = "Setup: 0 of 1 done\n  [ ] Install developer tools\n"
MADE = "Made Kessel: Kessel (kessel) at /home/me/k2-agents/kessel\n"

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
        auth = self.headers.get("Authorization")
        with open(LOG_FILE, "a") as f:
            f.write(json.dumps({
                "method": method, "path": u.path, "query": u.query,
                "auth": auth, "headers": [[k, v] for k, v in self.headers.items()],
                "body": body,
            }) + "\n")
        if not auth:
            return self._send(401, '{"error":"Invalid or missing auth token"}', "application/json")
        if "format=text" not in u.query:
            return self._send(200, '{"enabled":true,"rows":[]}', "application/json")
        if u.path in ("/cli/first-run/kessel", "/cli/first-run/chief-of-staff"):
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
: >"$LOG_FILE"
LOG_FILE="$LOG_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/cli/first-run/status"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

# The owner token on disk, readable, as in a K2 terminal on the owner's box.
mkdir -p "$WORK/home/.k2"
printf '%s' "$OWNER" >"$WORK/home/.k2/heartbeat.token"
chmod 600 "$WORK/home/.k2/heartbeat.token"
printf '%s' "$PORT" >"$WORK/home/.k2/heartbeat.port"
# The session marker names a socket; no socket is there (k2 setup uses TCP).
SOCK="$WORK/cell.sock"

run() { # env words..., then --, then the k2 args
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$LOG_FILE"
    set +e
    out="$(cd "$WORK" && env -i PATH="/usr/bin:/bin" HOME="$WORK/home" ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
# field <name>: that field of every logged request, one per line.
field() {
    "$PY" -c 'import json, sys
for line in open(sys.argv[1]):
    v = json.loads(line).get(sys.argv[2])
    print(repr(v))' "$LOG_FILE" "$1"
}
setup_requests() { grep -c '"path": "/cli/first-run/' "$LOG_FILE" || true; }
owner_never_sent() { # label
    if grep -Fq -- "$OWNER" "$LOG_FILE"; then
        bad "$1: the owner token was sent ($(grep -F -- "$OWNER" "$LOG_FILE" | head -n 1))"
    else
        ok "$1: the owner token is in no header and no query"
    fi
}

# Every setup verb, as "label|args" (args split on spaces).
VERBS=(
    "setup|setup"
    "setup --json|setup --json"
    "setup hide|setup hide"
    "setup show|setup show"
    "setup skip|setup skip dev_tools"
    "setup dismiss-offer|setup dismiss-offer"
    "setup kessel|setup kessel"
    "setup kessel --tool|setup kessel --tool claude --name Kay"
    "setup --json kessel|setup --json kessel"
    "setup chief-of-staff|setup chief-of-staff --name Ada --about ops --tool claude"
)

check_sends() { # case label, want auth repr, env words...
    local label="$1" want="$2"; shift 2
    local v name args
    for v in "${VERBS[@]}"; do
        name="${v%%|*}"; args="${v#*|}"
        # shellcheck disable=SC2086
        run "$@" -- $args
        assert_eq "$label: $name: exit 0" "$rc" "0"
        assert_eq "$label: $name: one request" "$(setup_requests)" "1"
        assert_eq "$label: $name: the Bearer header" "$(field auth)" "$want"
        case "$(field query)" in
            *token*) bad "$label: $name: a credential in the query: $(field query)" ;;
            *) ok "$label: $name: no credential in the query" ;;
        esac
        owner_never_sent "$label: $name"
    done
}

echo "== an agent's session, owner token readable on disk: the passport =="
check_sends "session" "'Bearer $PASSPORT'" K2_PORT="$PORT" K2_HOOK_SOCK="$SOCK" K2_HOOK_TOKEN="$PASSPORT"

echo "== the same under the K5_ names =="
check_sends "session (K5_)" "'Bearer $PASSPORT'" K5_PORT="$PORT" K5_HOOK_SOCK="$SOCK" K5_HOOK_TOKEN="$PASSPORT"

echo "== a session with no passport: nothing, never the owner token =="
for v in "${VERBS[@]}"; do
    name="${v%%|*}"; args="${v#*|}"
    # shellcheck disable=SC2086
    run K2_PORT="$PORT" K2_HOOK_SOCK="$SOCK" -- $args
    assert_eq "no passport: $name: exit 3" "$rc" "3"
    case "$name" in
        "setup chief-of-staff")
            # The locked agent-harness tool: refused before any request.
            assert_eq "no passport: $name: no request" "$(setup_requests)" "0"
            assert_eq "no passport: $name: says why" "$(printf '%s\n' "$err" | head -n 1)" "Error: tool 'agent-harness' requires session ID (passport)." ;;
        *)
            assert_eq "no passport: $name: one request" "$(setup_requests)" "1"
            assert_eq "no passport: $name: no Bearer header" "$(field auth)" "None"
            assert_eq "no passport: $name: the daemon's refusal" "$err" "Error: Invalid or missing auth token" ;;
    esac
    owner_never_sent "no passport: $name"
done

echo "== the owner's own terminal (no session): the owner token, as before =="
for v in "${VERBS[@]}"; do
    name="${v%%|*}"; args="${v#*|}"
    # shellcheck disable=SC2086
    run -- $args
    assert_eq "owner: $name: exit 0" "$rc" "0"
    assert_eq "owner: $name: one request" "$(setup_requests)" "1"
    assert_eq "owner: $name: the owner's Bearer header" "$(field auth)" "'Bearer $OWNER'"
    case "$(field query)" in
        *token*) bad "owner: $name: a credential in the query: $(field query)" ;;
        *) ok "owner: $name: no credential in the query" ;;
    esac
done

echo
echo "setup_session_passport: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
