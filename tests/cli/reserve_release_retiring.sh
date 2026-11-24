#!/usr/bin/env bash
# `k2 reserve` / `k2 release` are retiring (nothing reads reservations any
# more). In this release they are hidden and refuse any argument that
# starts with `-` other than `--agent <name>`: before, `k2 reserve --help`
# reserved a path named `--help`. The refusal is a usage error (exit 2,
# the reason on stderr) and sends nothing. -h or --help anywhere prints a
# short page instead (exit 0, sends nothing; tests/cli/help_acts_10.sh
# checks each form). A call with paths still works.
#
# A stub daemon logs every request (method, path, query without the token
# and project). No real daemon, no real home.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

T="$(cd "$(mktemp -d -t k2-reserve-XXXXXX)" && pwd -P)"
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

cat > "$T/stub.py" <<'PY'
import http.server, json, sys, urllib.parse
log, portfile = sys.argv[1], sys.argv[2]
BODY = json.dumps({"success": True}).encode()
class H(http.server.BaseHTTPRequestHandler):
    def _go(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        u = urllib.parse.urlsplit(self.path)
        q = [p for p in u.query.split("&") if p and not p.startswith(("token=", "project="))]
        with open(log, "a") as f:
            f.write("%s %s?%s\n" % (self.command, u.path, "&".join(q)))
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
WS="$T/ws"
mkdir -p "$WS"

# k2_as <agent name or ""> <args...>: run cli/k2; sets RC, OUT, ERR and
# leaves requests.log. A name sets K2_AGENT_NAME (as a v1 host session
# does), so a refusal can't be the "--agent required" error.
k2_as() {
    local name="$1"; shift
    : > "$T/requests.log"
    local -a envs=(K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WS")
    [ -n "$name" ] && envs+=(K2_AGENT_NAME="$name")
    RC=0
    OUT="$(env -u K2_AGENT_NAME -u K2SO_AGENT_NAME "${envs[@]}" "$K2_CLI" "$@" 2>"$T/stderr")" || RC=$?
    ERR="$(cat "$T/stderr")"
}

# refused <case> <verb> <the flag named> <agent> <args...>
refused() {
    local name="$1" verb="$2" flag="$3" agent="$4"; shift 4
    k2_as "$agent" "$verb" "$@"
    [ "$RC" = 2 ] || die "$name: exit $RC, wanted 2 (stderr: $ERR)"
    ok
    [ ! -s "$T/requests.log" ] || die "$name: a request was sent"
    ok
    [ -z "$OUT" ] || die "$name: printed on stdout: $OUT"
    ok
    local want="k2 $verb: '$flag' is not a path. k2 $verb takes paths and --agent <name> only, and is being retired."
    [ "$ERR" = "$want" ] || die "$name: stderr was: $ERR"
    ok
}
# sent <case> <exact log line>
sent() {
    [ "$RC" = 0 ] || die "$1: exit $RC, wanted 0 (stderr: $ERR)"
    ok
    local got; got="$(cat "$T/requests.log")"
    [ "$got" = "$2" ] || die "$1: wanted the request
  $2
got
  $got"
    ok
    [ "$OUT" = '{"success": true}' ] || die "$1: printed: $OUT"
    ok
}

# ── Refused: any argument starting with `-` but --agent (and the help
# words, which print the page).
refused "reserve -x"                 reserve -x       tester -x
refused "release --foo"              release --foo    tester --foo
refused "reserve, a path then -x"    reserve -x       tester some/path -x
refused "release, --agent then --x"  release --x      "" --agent tester --x
refused "reserve, --agent=name"      reserve --agent=tester "" --agent=tester some/path
# Refused before the agent check: with no agent name it is still exit 2.
refused "reserve --nope, no agent"   reserve --nope   "" --nope
refused "release -x, no agent"       release -x       "" -x
# The help words print the page, exit 0, and send nothing.
helped() { # helped <case> <verb> <agent> <args...>
    local name="$1" verb="$2" agent="$3"; shift 3
    k2_as "$agent" "$verb" "$@"
    [ "$RC" = 0 ] || die "$name: exit $RC, wanted 0 (stderr: $ERR)"
    ok
    [ ! -s "$T/requests.log" ] || die "$name: a request was sent"
    ok
    [ -z "$ERR" ] || die "$name: stderr: $ERR"
    ok
    [ "$OUT" = "$(cli_page "$PROJECT_ROOT/cli/text/help/$verb.txt")" ] || die "$name: stdout was: $OUT"
    ok
}
helped "reserve --help"              reserve tester --help
helped "reserve -h"                  reserve tester -h
helped "release --help"              release tester --help
helped "release, --agent then --help" release "" --agent tester --help
helped "reserve --help, no agent"    reserve "" --help
helped "reserve --agent --help"      reserve "" --agent --help some/path
# --agent's value is a name, never a flag, and it must be there.
no_name() { # no_name <case> <verb> <args...>
    local name="$1" verb="$2"; shift 2
    k2_as "" "$verb" "$@"
    [ "$RC" = 2 ] || die "$name: exit $RC, wanted 2 (stderr: $ERR)"
    ok
    [ ! -s "$T/requests.log" ] || die "$name: a request was sent"
    ok
    [ "$ERR" = "k2 $verb: '--agent' needs a name." ] || die "$name: stderr was: $ERR"
    ok
}
no_name "reserve --agent --x"        reserve --agent --x some/path
no_name "reserve --agent, no name"   reserve some/path --agent
no_name "release --agent, no name"   release --agent

# ── Still working: paths (and --agent).
k2_as tester reserve some/path
sent "reserve some/path"             "POST /cli/reserve?agent=tester&paths=some%2Fpath"
k2_as "" reserve --agent tester a.rs b/c.rs
sent "reserve --agent, two paths"    "POST /cli/reserve?agent=tester&paths=a.rs%2Cb%2Fc.rs"
k2_as tester reserve some/path --agent other
sent "reserve, --agent after"        "POST /cli/reserve?agent=other&paths=some%2Fpath"
k2_as tester release
sent "release, no paths"             "POST /cli/release?agent=tester"
k2_as tester release some/path
sent "release some/path"             "POST /cli/release?agent=tester&paths=some%2Fpath"
k2_as "" release --agent tester some/path
sent "release --agent"               "POST /cli/release?agent=tester&paths=some%2Fpath"

# ── The agent name is escaped: one agent value, as typed. Raw, a '#' cut
# the query (and the paths), a '&' or '=' added a parameter and '%41'
# reached the daemon as 'A'.
# one_agent <case> <the agent value the daemon must parse>
one_agent() {
    python3 - "$T/requests.log" "$2" <<'PY' || die "$1: the daemon did not get one agent value"
import sys, urllib.parse
lines = [l for l in open(sys.argv[1]).read().split("\n") if l]
assert len(lines) == 1, lines
form = urllib.parse.parse_qs(lines[0].split("?", 1)[1], keep_blank_values=True)
assert form["agent"] == [sys.argv[2]], form
PY
    ok
}
k2_as "" reserve --agent 'a&b=c d%41' some/path
sent "reserve, a name with & = space %" "POST /cli/reserve?agent=a%26b%3Dc%20d%2541&paths=some%2Fpath"
one_agent "reserve, a name with & = space %" 'a&b=c d%41'
k2_as 'x#y' reserve some/path
sent "reserve, K2_AGENT_NAME with #"  "POST /cli/reserve?agent=x%23y&paths=some%2Fpath"
one_agent "reserve, K2_AGENT_NAME with #" 'x#y'
k2_as "" release --agent 'x&paths=y' some/path
sent "release, a name with & and ="   "POST /cli/release?agent=x%26paths%3Dy&paths=some%2Fpath"
one_agent "release, a name with & and =" 'x&paths=y'
k2_as 'a b+c%' release
sent "release, K2_AGENT_NAME with space + %" "POST /cli/release?agent=a%20b%2Bc%25"
one_agent "release, K2_AGENT_NAME with space + %" 'a b+c%'

# The errors that were there stay as they were.
k2_as "" reserve some/path
[ "$RC" = 1 ] || die "reserve with no agent: exit $RC"
ok
[ "$ERR" = "Error: --agent required (or set K2_AGENT_NAME)" ] || die "reserve with no agent: stderr was: $ERR"
ok
[ ! -s "$T/requests.log" ] || die "reserve with no agent: a request was sent"
ok
k2_as tester reserve
[ "$RC" = 1 ] || die "reserve with no path: exit $RC"
ok
[ "$ERR" = "Error: at least one path required" ] || die "reserve with no path: stderr was: $ERR"
ok

# ── Hidden: no help tier, study page or schema entry names them.
k2_as "" --help
case "$OUT$ERR" in *"k2 reserve"*|*"k2 release"*) die "k2 --help names reserve or release" ;; esac
ok
k2_as "" --schema
printf '%s' "$OUT" | python3 -c '
import json, sys
names = [c.get("name") or c.get("command") or "" for c in json.load(sys.stdin)["commands"]]
bad = [n for n in names if n.split()[:1] in (["reserve"], ["release"])]
assert not bad, bad
' || die "k2 --schema lists reserve or release"
ok

echo "reserve_release_retiring: $pass passed"
