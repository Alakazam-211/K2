#!/usr/bin/env bash
# -h or --help anywhere on `k2 voice` prints the voice usage, exits 0 and
# sends nothing, even where a value flag would take it as its value. And a
# usage error is one line of valid JSON whatever words it quotes.
#
# Regression (0.46.17): cmd_voice read -h/--help only where a flag could
# start, so `k2 voice leave --call --help` left the call (--help was the
# call id) and `k2 voice join lounge --call -h` joined. Its usage errors
# put the word into JSON as it was, so a quote in a flag or a command
# broke the line.
#
# Every help form below prints exactly what `k2 voice --help` prints, on
# stdout, nothing on stderr, exits 0, and the stub daemon records no
# request at all. Then the plain `k2 voice leave --call c1` and `k2 voice
# join lounge` still send their POST, so the stub is known to see a
# request when one is sent.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/voice_help_no_request.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-voice-help-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _reply(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        self._reply({"ok": True})
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

# run <args...>: sets OUT (stdout), ERR (stderr), RC.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

: >"$REQ_LOG"
run voice --help
REF="$OUT"
[ "$RC" -eq 0 ] && ok "k2 voice --help: exit 0" || bad "k2 voice --help: exit $RC (want 0)"
case "$REF" in
    "Usage: k2 voice <command> [args] [--json]"*) ok "k2 voice --help prints the voice usage" ;;
    *) bad "k2 voice --help: no usage line"; printf '%s\n' "$OUT$ERR" | sed 's/^/      | /' >&2 ;;
esac
if [ -s "$REQ_LOG" ]; then
    bad "k2 voice --help sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
else
    ok "k2 voice --help sends nothing"
fi

# check <args...>
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$REF" ] && ok "$label: prints the voice usage" \
        || { bad "$label: stdout is not the voice usage"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
}

echo "== -h/--help as a value flag's value =="
check voice leave --call --help
check voice leave --call -h
check voice join lounge --call -h
check voice consent 77 yes --notice --help
check voice settings --require-recording -h
check voice settings --start-level --help
check voice recordings --room --help
check voice transcripts subscribe lounge ws --room -h
check voice transcript 5 --after -h
check voice transcript 5 --limit --help
check voice transcripts reads --recording -h

echo "== -h/--help anywhere else =="
check voice -h
check voice help
check voice -h join lounge
check voice join lounge -h
check voice leave --help
check voice set rooms 3 --now --help
check voice close lounge -h
check voice rename lounge New Name --help
check voice kick lounge ada -h
check voice repair --help

echo "== a usage error is one line of valid JSON =="
# usage_json <want-hint> <args...>
usage_json() {
    local want="$1"; shift
    local label="k2 voice $*"
    : >"$REQ_LOG"
    run voice "$@"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ -z "$OUT" ] && ok "$label: nothing on stdout" || bad "$label: stdout: $OUT"
    if got="$(printf '%s' "$ERR" | python3 -c '
import json, sys
lines = sys.stdin.read().splitlines()
assert len(lines) == 1, "want one line, got %d" % len(lines)
d = json.loads(lines[0])
assert d["error"] == "usage", d
print(d["hint"])
' 2>&1)"; then
        [ "$got" = "$want" ] && ok "$label: the hint is the words as typed" \
            || bad "$label: hint $(printf %q "$got") (want $(printf %q "$want"))"
    else
        bad "$label: stderr is not one line of JSON: $got"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
    fi
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
}
usage_json 'unknown command x"y (k2 voice --help)' 'x"y'
usage_json 'unknown command a\b (k2 voice --help)' 'a\b'
usage_json 'unknown flag --bad"q (k2 voice --help)' rooms '--bad"q'
usage_json 'unknown flag --x\ (k2 voice --help)' rooms '--x\'
usage_json 'k2 voice join <room> (k2 voice --help)' join
# A plain one is byte for byte what it was.
run voice frobnicate
[ "$ERR" = '{"error":"usage","hint":"unknown command frobnicate (k2 voice --help)"}' ] \
    && ok "k2 voice frobnicate: the usage line is unchanged" \
    || bad "k2 voice frobnicate: $(printf %q "$ERR")"

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"
run voice leave --call c1
if grep -q '^POST /cli/voice/session/leave .*"callId": *"c1"' "$REQ_LOG"; then
    ok "k2 voice leave --call c1 still sends its request"
else
    bad "k2 voice leave --call c1 sent no leave"; sed 's/^/      | /' "$REQ_LOG" >&2
fi
: >"$REQ_LOG"
run voice join lounge
if grep -q '^POST /cli/voice/join .*"room": *"lounge"' "$REQ_LOG"; then
    ok "k2 voice join lounge still sends its request"
else
    bad "k2 voice join lounge sent no join"; sed 's/^/      | /' "$REQ_LOG" >&2
fi

echo ""
echo "voice_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
