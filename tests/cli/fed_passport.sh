#!/usr/bin/env bash
# `k2 fed` read the owner token from ~/.k2/daemon.token itself, whatever
# passport the session carried. So from an agent session the pairing
# commands, `confirm`, `outbox` and `send` all went to the daemon as the
# server's owner.
#
# Inside a session (a passport is set):
#   - pair-start / pair-join / pair-finish / pair-verify, confirm, outbox
#     are refused (exit 3) before the token file is read, and nothing is
#     sent;
#   - send goes with the session's own passport (the daemon's send route
#     takes one and binds the message to that session's workspace).
# With no passport (a person's own terminal) nothing changes.
#
# A stub daemon logs every request line. No real daemon, no real home.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

T="$(cd "$(mktemp -d -t k2-fed-passport-XXXXXX)" && pwd -P)"
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
ok() { pass=$((pass + 1)); }

# The stub answers every request with one object that holds what each
# command's reader looks for, so no command fails for lack of an answer.
cat > "$T/stub.py" <<'PY'
import http.server, json, sys
log, portfile = sys.argv[1], sys.argv[2]
BODY = json.dumps({
    "ok": True, "offer": {"n": 1}, "answer": {"n": 2}, "reveal": {"n": 3},
    "sas": "1234", "peer_fingerprint": "ab" * 32, "peers": [], "queued": [], "dead": [],
}).encode()
class H(http.server.BaseHTTPRequestHandler):
    def _go(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        with open(log, "a") as f:
            f.write(self.command + " " + self.path + "\n")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)
    do_GET = do_POST = _go
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

OWNER="owner-token-in-daemon-token"
HB_OWNER="owner-token-in-heartbeat-token"
SESSION="sess-abc.passport1"
WS="$T/ws"
mkdir -p "$WS/.k2" "$HOME/.k2"
printf '%s\n' "$OWNER" > "$HOME/.k2/daemon.token"
printf '%s' "$HB_OWNER" > "$HOME/.k2/heartbeat.token"
printf '%s' "$STUB_PORT" > "$HOME/.k2/heartbeat.port"

# A pairing token of each kind, well formed, so pair-join/finish/verify
# get as far as the request when they are allowed to.
tok() { python3 -c 'import base64, json, sys; print(sys.argv[1] + base64.urlsafe_b64encode(json.dumps({"n": 1}).encode()).decode().rstrip("="))' "$1"; }
TOK1="$(tok k2pair1:)"; TOK2="$(tok k2pair2:)"; TOK3="$(tok k2pair3:)"

# fed <session|person> <args...>: run `k2 fed …`; sets RC, OUT, ERR and
# leaves requests.log.
LAST_WHO=""; LAST_ARGS=()
fed() {
    local who="$1"; shift
    LAST_WHO="$who"; LAST_ARGS=("$@")
    : > "$T/requests.log"
    local -a envs=(K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WS")
    [ "$who" = session ] && envs+=(K2_HOOK_TOKEN="$SESSION")
    RC=0
    OUT="$(env -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT \
        "${envs[@]}" bash "$K2_CLI" fed "$@" 2>"$T/stderr")" || RC=$?
    ERR="$(cat "$T/stderr")"
}
sent_nothing() { [ ! -s "$T/requests.log" ] || die "$1: a request was sent"; ok; }
never_sent() { if grep -q "token=${2}" "$T/requests.log"; then die "$1: sent ${2}"; fi; ok; }
sent_once() { # sent_once <case> <METHOD> <route> <token>
    local n; n="$(grep -c "^${2} ${3}?token=${4}\$" "$T/requests.log" || true)"
    [ "$n" = 1 ] || die "$1: wanted one ${2} ${3} with token=${4}, saw $n"
    [ "$(wc -l < "$T/requests.log" | tr -d ' ')" = 1 ] || die "$1: more than one request"
    ok
}
# The token file must not be opened at all in a session. The last command
# is run once more under `bash -x`, whose trace shows every command the
# shell ran. (Its requests land in the log after the checks on it.)
token_file_not_read() {
    local -a envs=(K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WS")
    [ "$LAST_WHO" = session ] && envs+=(K2_HOOK_TOKEN="$SESSION")
    env -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT \
        "${envs[@]}" bash -x "$K2_CLI" fed "${LAST_ARGS[@]}" >/dev/null 2>"$T/trace" || true
    grep -q '^+* *cmd_fed ' "$T/trace" || die "$1: the trace never reached cmd_fed"
    if grep -E '^\++ (cat|tr) ' "$T/trace" | grep -q 'daemon\.token'; then die "$1: read daemon.token"; fi
    ok
}

# ── Inside an agent session: the owner's commands are refused.
refused() { # refused <case> <reason in the first line> <args...>
    local name="$1" why="$2"; shift 2
    fed session "$@"
    [ "$RC" = 3 ] || die "$name: exit $RC, wanted 3 ($ERR)"
    ok
    [ -z "$OUT" ] || die "$name: printed on stdout: $OUT"
    ok
    local want="Error: k2 fed ${1} is the server owner's: ${why}.
This shell is an agent session (it carries a session passport), so k2 doesn't use the owner token here.
Ask the owner to run it from their own terminal."
    [ "$ERR" = "$want" ] || die "$name: stderr was: $ERR"
    ok
    sent_nothing "$name"
    token_file_not_read "$name"
}
refused "pair-start, session"  "it pairs this server with another one" pair-start --subdomain acme
refused "pair-join, session"   "it pairs this server with another one" pair-join "$TOK1"
refused "pair-finish, session" "it pairs this server with another one" pair-finish "$TOK2"
refused "pair-verify, session" "it pairs this server with another one" pair-verify "$TOK3"
refused "confirm, session"     "it trusts another server" confirm "$(printf 'ab%.0s' $(seq 1 32))" 1234
refused "outbox, session"      "it lists every workspace's queued messages" outbox
# Refused before the arguments are looked at: no usage line teaches the form.
refused "confirm with no arguments, session" "it trusts another server" confirm

# ── Inside an agent session: send goes as the session.
fed session send "acme::ws::agent" "hello"
[ "$RC" = 0 ] || die "send, session: exit $RC ($ERR)"
ok
sent_once "send, session" POST /cli/federation/send "$SESSION"
never_sent "send, session" "$OWNER"
never_sent "send, session" "$HB_OWNER"
token_file_not_read "send, session"

# ── A person's own terminal (no passport): as before, the owner token
#    from daemon.token.
fed person send "acme::ws::agent" "hello"
[ "$RC" = 0 ] || die "send, person: exit $RC ($ERR)"
ok
sent_once "send, person" POST /cli/federation/send "$OWNER"

fed person pair-start --subdomain acme
[ "$RC" = 0 ] || die "pair-start, person: exit $RC ($ERR)"
ok
sent_once "pair-start, person" POST /cli/federation/pair/v2/init "$OWNER"
case "$OUT" in *"k2pair1:"*) ok ;; *) die "pair-start, person: no token printed: $OUT" ;; esac

fed person pair-join "$TOK1";   sent_once "pair-join, person"   POST /cli/federation/pair/v2/respond "$OWNER"
fed person pair-finish "$TOK2"; sent_once "pair-finish, person" POST /cli/federation/pair/v2/reveal "$OWNER"
fed person pair-verify "$TOK3"; sent_once "pair-verify, person" POST /cli/federation/pair/v2/verify "$OWNER"
fed person confirm "$(printf 'ab%.0s' $(seq 1 32))" 1234
sent_once "confirm, person" POST /cli/federation/pair/confirm "$OWNER"
fed person outbox
sent_once "outbox, person" GET /cli/federation/outbox "$OWNER"

# A person with no owner token file is told so, as before.
mv "$HOME/.k2/daemon.token" "$T/daemon.token.away"
fed person confirm "$(printf 'ab%.0s' $(seq 1 32))" 1234
[ "$RC" = 1 ] || die "confirm, person, no token file: exit $RC"
case "$ERR" in *"k2 fed confirm: no owner token (need "*) ok ;; *) die "confirm, person, no token file: $ERR" ;; esac
sent_nothing "confirm, person, no token file"
mv "$T/daemon.token.away" "$HOME/.k2/daemon.token"

echo "fed_passport: $pass passed"
