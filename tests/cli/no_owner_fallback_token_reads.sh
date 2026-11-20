#!/usr/bin/env bash
# K2_NO_OWNER_FALLBACK=1 (every program on a K2 virtual desktop) means k2
# never acts as this server's owner from a shell with no session passport.
# The connection guard enforces it for the token the prelude reads. Two
# commands read the owner token file themselves: heartbeat wake (its inline
# Python) and the Connect account commands (late discovery in
# _connect_require_daemon). Under the variable with no passport each
# refuses as the guard does (exit 3, the same message) and never reads the
# file. With a passport, or without the variable, nothing changes.
#
# A stub daemon logs every request with its Authorization header (the
# only place k2 puts a credential). HOME is a temp dir holding a known
# owner token in heartbeat.token and daemon.token; never the real ~/.k2.
# The commands run end to end, and the functions themselves (taken from the
# bash source) where the guard or the help page stops the command first,
# or where reaching the read needs the real account server (connect login
# with -h as a flag value skips the guard).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

T="$(cd "$(mktemp -d -t k2-no-owner-XXXXXX)" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$T"
}
trap cleanup EXIT

pass=0
die() {
    echo "FAIL: $*" >&2
    [ -f "$T/requests.log" ] && sed 's/^/  request: /' "$T/requests.log" >&2
    exit 1
}
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

OWNER="owner-token-on-disk-nofallback"
SESSION="sess-nofallback.passport1"
H="$T/home"
WS="$T/ws"
mkdir -p "$H/.k2" "$WS/.k2"
printf '%s' "$OWNER" >"$H/.k2/heartbeat.token"
printf '%s' "$OWNER" >"$H/.k2/daemon.token"

# The stub answers every request with an empty JSON list.
cat >"$T/stub.py" <<'PY'
import http.server, sys
log, portfile = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        with open(log, "a") as f:
            f.write("%s %s auth=%s\n" % (self.command, self.path, self.headers.get("Authorization") or ""))
        body = b"[]"
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
with open(portfile + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
import os
os.rename(portfile + ".tmp", portfile)
srv.serve_forever()
PY
python3 -I "$T/stub.py" "$T/requests.log" "$T/stub.port" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$T/stub.port" ] && break; sleep 0.05; done
[ -s "$T/stub.port" ] || die "the stub daemon did not start"
cp "$T/stub.port" "$H/.k2/heartbeat.port"

# run <env assignments...> -- <k2 args...>: a shell with no K2 identity
# (as on a virtual desktop) plus the given names. Sets OUT and RC.
run() {
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$T/requests.log"
    set +e
    OUT="$(env -i PATH="$PATH" HOME="$H" K2_PROJECT_PATH="$WS" ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" 2>&1)"
    RC=$?
    set -e
}

# The prelude exports _K2_PY_PROXY_RULE, and every embedded Python block
# that calls the daemon exec's it first. Without it the wake's Python stops
# before its first request and its "|| echo no" hides that, so fn() takes
# the definition from the bash source as the prelude leaves it.
PY_PROXY_RULE_DEF="$(awk -v q="'" '$0 == "_K2_PY_PROXY_RULE=" q {on=1} on {print} on && $0 == q {exit}' "$CLI_BASH_SOURCE")"
case "$PY_PROXY_RULE_DEF" in
    *"install_opener"*) ;;
    *) die "no _K2_PY_PROXY_RULE definition in $CLI_BASH_SOURCE" ;;
esac

# fn <names...> -- <env assignments...> -- <shell code>: define the named
# functions from the bash source and run the code with them, in a shell
# set up as the prelude leaves it: the k5 hand-over helpers' text
# (_K5_PY_HTTP, _K5_NOUN_RE, _k5_headers and _k5_py_headers, which every
# Python call to the daemon passes on, with the Codex header helpers
# _k5_py_headers calls) is always defined. Sets OUT and RC.
fn() {
    local names=(_k5_headers _k5_py_headers _cli_codex_bind_on _k2_codex_headers) envs=() n body
    local defs
    defs="$(sed -n "/^_K5_PY_HTTP='\$/,/^'\$/p; /^_K5_NOUN_RE=/p; /^_K2_CODEX_ID_RE=/p" "$CLI_BASH_SOURCE")"$'\n'
    case "$defs" in *"def k5_request"*"_K5_NOUN_RE="*|*"_K5_NOUN_RE="*"def k5_request"*) ;; *) die "no _K5_PY_HTTP or _K5_NOUN_RE in $CLI_BASH_SOURCE" ;; esac
    while [ "$1" != "--" ]; do names+=("$1"); shift; done
    shift
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    defs+="$PY_PROXY_RULE_DEF"$'\n'"export _K2_PY_PROXY_RULE"$'\n'
    for n in "${names[@]}"; do
        body="$(awk -v n="$n" '$0 ~ "^" n "\\(\\) \\{" {on=1} on {print} on && /^}/ {exit}' "$CLI_BASH_SOURCE")"
        [ -n "$body" ] || die "no function $n in $CLI_BASH_SOURCE"
        defs+="$body"$'\n'
    done
    : >"$T/requests.log"
    set +e
    OUT="$(env -i PATH="$PATH" HOME="$H" K2_HOME="$H/.k2" PROJECT="$WS" HOST=127.0.0.1 \
        DEFS="$defs" CODE="$1" ${envs[@]+"${envs[@]}"} \
        bash -c 'set -euo pipefail; eval "$DEFS"; eval "$CODE"' 2>&1)"
    RC=$?
    set -e
}

refused() { # refused <case>: the guard's exit and message, no owner token anywhere
    [ "$RC" = 3 ] || die "$1: exit $RC (want 3): $OUT"
    case "$OUT" in
        *"Error: k2 won't act as this server's owner here: no session passport (K2_NO_OWNER_FALLBACK=1, a K2 virtual desktop or k2 vault run)."*"not under vault run, or give the command the secret it needs."*) ;;
        *) die "$1: not the guard's message: $OUT" ;;
    esac
    case "$OUT" in *"$OWNER"*) die "$1: the owner token is in the output" ;; esac
    if grep -q "$OWNER" "$T/requests.log"; then die "$1: the owner token reached the daemon"; fi
    ok "$1"
}
sent() { # sent <case> <route> <token>: the route was called with that Bearer token
    local n
    n="$(awk -v route="$2" -v want=" auth=Bearer $3" '
        { split($2, p, "?") }
        p[1] == route && substr($0, length($0) - length(want) + 1) == want' \
        "$T/requests.log" | wc -l | tr -d ' ')"
    [ "$n" -ge 1 ] || die "$1: no $2 with Bearer $3"
    if grep -q "token=" "$T/requests.log"; then die "$1: a token in the query string"; fi
    ok "$1"
}
never_sent() { # never_sent <case> <token>
    if grep -q "$2" "$T/requests.log"; then die "$1: sent $2"; fi
    case "$OUT" in *"$2"*) die "$1: $2 is in the output" ;; esac
    ok "$1"
}

echo "heartbeat wake"
NOOP='{"status":"noop","reason":"no inbox work found"}'
run K2_NO_OWNER_FALLBACK=1 -- heartbeat wake
refused "wake, end to end, no passport"
fn _k2_refuse_owner_fallback cmd_heartbeat_wake -- _k2_no_owner_fallback=1 -- \
    'SCOPED_TOKEN=""; cmd_heartbeat_wake'
refused "wake function, no passport"
fn _k2_refuse_owner_fallback cmd_heartbeat_wake -- _k2_no_owner_fallback=1 K2_HOOK_TOKEN="$SESSION" -- \
    "SCOPED_TOKEN='$SESSION'; cmd_heartbeat_wake"
[ "$RC" = 0 ] && [ "$OUT" = "$NOOP" ] || die "wake function with a passport: exit $RC: $OUT"
sent "wake function, passport: agents list" /cli/agents/list "$SESSION"
sent "wake function, passport: workspace inbox" /cli/inbox/list "$SESSION"
never_sent "wake function, passport" "$OWNER"
run K2_NO_OWNER_FALLBACK=1 K2_HOOK_TOKEN="$SESSION" -- heartbeat wake
[ "$RC" = 0 ] && [ "$OUT" = "$NOOP" ] || die "wake with a passport: exit $RC: $OUT"
sent "wake, passport" /cli/agents/list "$SESSION"
never_sent "wake, passport" "$OWNER"
run -- heartbeat wake
[ "$RC" = 0 ] && [ "$OUT" = "$NOOP" ] || die "wake without the variable: exit $RC: $OUT"
sent "wake without the variable: the owner token as before" /cli/agents/list "$OWNER"

echo "connect account (_connect_require_daemon)"
REQ='PORT=""; TOKEN=""; DISK_OWNER_TOKEN=""; _connect_require_daemon; echo "TOKEN=$TOKEN"'
fn _k2_refuse_owner_fallback _k2_daemon_base _connect_require_daemon -- _k2_no_owner_fallback=1 -- "$REQ"
refused "connect, no passport"
fn _k2_refuse_owner_fallback _k2_daemon_base _connect_require_daemon -- _k2_no_owner_fallback=1 -- \
    "PORT=''; TOKEN='$SESSION'; DISK_OWNER_TOKEN=''; _connect_require_daemon; echo \"TOKEN=\$TOKEN\""
[ "$RC" = 0 ] && [ "$OUT" = "TOKEN=$SESSION" ] || die "connect with a passport: exit $RC: $OUT"
ok "connect, passport: used as is"
fn _k2_refuse_owner_fallback _k2_daemon_base _connect_require_daemon -- -- "$REQ"
[ "$RC" = 0 ] && [ "$OUT" = "TOKEN=$OWNER" ] || die "connect without the variable: exit $RC: $OUT"
ok "connect without the variable: reads heartbeat.token as before"
mv "$H/.k2/heartbeat.token" "$T/heartbeat.token.aside"
fn _k2_refuse_owner_fallback _k2_daemon_base _connect_require_daemon -- -- "$REQ"
mv "$T/heartbeat.token.aside" "$H/.k2/heartbeat.token"
[ "$RC" = 0 ] && [ "$OUT" = "TOKEN=$OWNER" ] || die "connect without the variable, daemon.token only: exit $RC: $OUT"
ok "connect without the variable: reads daemon.token as before"
run K2_NO_OWNER_FALLBACK=1 -- connect status
refused "connect status, end to end, no passport"

echo "no_owner_fallback_token_reads: $pass passed"
