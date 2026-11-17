#!/usr/bin/env bash
# `k2 heartbeat wake` asks the daemon whether there is inbox work from an
# inline Python block that picks its own port and token. It read only the
# K2SO_ names, so a session that carries K2_PORT / K2_HOOK_TOKEN (every
# session since 0.40) sent that check to the port on disk with the OWNER
# token from ~/.k2/heartbeat.token. It must send the session's passport.
#
# A stub daemon logs every request line. No real daemon, no real home.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

T="$(cd "$(mktemp -d -t k2-hb-wake-XXXXXX)" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$T"
    hermetic_cli_cleanup
}
trap cleanup EXIT
pass=0
die() { echo "FAIL: $*" >&2; [ -f "$T/requests.log" ] && sed 's/^/  request: /' "$T/requests.log" >&2; exit 1; }

# The stub answers every GET with an empty list (no agents, no inbox
# work), so the command prints its "noop" line and stops there.
cat > "$T/stub.py" <<'PY'
import http.server, sys
log, portfile = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(log, "a") as f:
            f.write(self.command + " " + self.path + "\n")
        body = b"[]"
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_POST = do_GET
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
with open(portfile, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 "$T/stub.py" "$T/requests.log" "$T/stub.port" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$T/stub.port" ] && break; sleep 0.05; done
[ -s "$T/stub.port" ] || die "the stub daemon did not start"
STUB_PORT="$(cat "$T/stub.port")"

# A port nothing listens on: where the files on disk point in the cases
# that must NOT use them.
DEAD_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"

OWNER="owner-token-on-disk"
SESSION="sess-abc.passport1"
OLDER="sess-old.passport2"
WS="$T/ws"
mkdir -p "$WS/.k2" "$HOME/.k2"
printf '%s' "$OWNER" > "$HOME/.k2/heartbeat.token"

# wake <port on disk> <env assignments...>: run the command with exactly
# these connection names set; prints nothing, leaves requests.log.
wake() {
    local disk_port="$1"; shift
    : > "$T/requests.log"
    printf '%s' "$disk_port" > "$HOME/.k2/heartbeat.port"
    local out rc=0
    out="$(env -u K2_PORT -u K2SO_PORT -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN \
        K2_PROJECT_PATH="$WS" "$@" bash "$K2_CLI" heartbeat wake 2>"$T/stderr")" || rc=$?
    [ "$rc" = 0 ] || die "heartbeat wake exited $rc ($*): $out $(cat "$T/stderr")"
    [ "$out" = '{"status":"noop","reason":"no inbox work found"}' ] || die "heartbeat wake printed ($*): $out"
}
# The inbox check is two requests, each with one token.
check_token() { # check_token <case> <wanted token>
    local n
    for route in /cli/agents/list /cli/work/inbox; do
        n="$(grep -c "^GET ${route}?token=${2}&project=" "$T/requests.log" || true)"
        [ "$n" = 1 ] || die "$1: wanted one GET $route with token=$2, saw $n"
    done
    pass=$((pass + 1))
}
never_sent() { # never_sent <case> <token>
    if grep -q "token=${2}" "$T/requests.log"; then die "$1: sent ${2}"; fi
    pass=$((pass + 1))
}

# 1. A session of today: K2_ names only. The check goes to the session's
#    port with the session's passport, and the owner token stays on disk.
wake "$DEAD_PORT" K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SESSION"
check_token "K2_ names only" "$SESSION"
never_sent "K2_ names only" "$OWNER"

# 2. The older names alone still work.
wake "$DEAD_PORT" K2SO_PORT="$STUB_PORT" K2SO_HOOK_TOKEN="$OLDER"
check_token "K2SO_ names only" "$OLDER"
never_sent "K2SO_ names only" "$OWNER"

# 3. Both set: the current name wins.
wake "$DEAD_PORT" K2_PORT="$STUB_PORT" K2SO_PORT="$DEAD_PORT" K2_HOOK_TOKEN="$SESSION" K2SO_HOOK_TOKEN="$OLDER"
check_token "both names" "$SESSION"
never_sent "both names" "$OLDER"
never_sent "both names" "$OWNER"

# 4. The current name set but empty counts as unset: the older one is used.
wake "$DEAD_PORT" K2_PORT="" K2SO_PORT="$STUB_PORT" K2_HOOK_TOKEN="" K2SO_HOOK_TOKEN="$OLDER"
check_token "K2_ names empty" "$OLDER"
never_sent "K2_ names empty" "$OWNER"

# 5. No session at all (a person's own terminal): the port and the owner
#    token on disk, as before.
wake "$STUB_PORT"
check_token "no session" "$OWNER"

echo "heartbeat_wake_passport: $pass passed"
