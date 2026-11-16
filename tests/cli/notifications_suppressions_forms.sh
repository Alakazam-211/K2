#!/usr/bin/env bash
# `k2 notifications email suppressions` takes every form its usage shows:
#   suppressions [list] [<address>] [--limit N] [--json]
# Before, only `suppressions list <address>` worked: `suppressions <address>`
# and `suppressions --json` took their first word as the verb and printed
# the usage (exit 2).
#
# Each case runs the CLI against a stub daemon that logs the method, path,
# query (without the token) and body of every request, and checks the one
# request it sent and the exit code. add and rm are checked too, so the
# list forms can't change them. No real daemon, no real home.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

T="$(cd "$(mktemp -d -t k2-suppressions-XXXXXX)" && pwd -P)"
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

# One answer holds what each verb's reader looks for.
cat > "$T/stub.py" <<'PY'
import http.server, json, sys, urllib.parse
log, portfile = sys.argv[1], sys.argv[2]
BODY = json.dumps({"suppressions": [], "added": True, "scope": "box", "removed": 1}).encode()
class H(http.server.BaseHTTPRequestHandler):
    def _go(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        u = urllib.parse.urlsplit(self.path)
        q = [p for p in u.query.split("&") if p and not p.startswith("token=")]
        with open(log, "a") as f:
            f.write("%s %s?%s %s\n" % (self.command, u.path, "&".join(q), body))
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

# sup <args...>: run `k2 notifications email suppressions <args>`; sets RC,
# OUT, ERR and leaves requests.log.
sup() {
    : > "$T/requests.log"
    RC=0
    OUT="$(K2_PORT="$STUB_PORT" bash "$K2_CLI" notifications email suppressions "$@" 2>"$T/stderr")" || RC=$?
    ERR="$(cat "$T/stderr")"
}
# sent <case> <exact log line>: exit 0 and exactly that one request.
sent() {
    [ "$RC" = 0 ] || die "$1: exit $RC, wanted 0 ($ERR)"
    ok
    local got; got="$(cat "$T/requests.log")"
    [ "$got" = "$2" ] || die "$1: wanted the request
  $2
got
  $got"
    ok
}
# refused <case>: exit 2 with the usage on stderr and nothing sent.
refused() {
    [ "$RC" = 2 ] || die "$1: exit $RC, wanted 2 ($ERR)"
    ok
    [ ! -s "$T/requests.log" ] || die "$1: a request was sent"
    ok
    [ -n "$ERR" ] || die "$1: nothing on stderr"
    ok
}

LIST="GET /cli/notifications/email/suppressions"
ADDR="a%40acme.example"

# ── The list forms.
sup;                                     sent "bare"                 "$LIST? "
case "$OUT" in "No suppressed addresses.") ok ;; *) die "bare: printed: $OUT" ;; esac
sup list;                                sent "list"                 "$LIST? "
sup ls;                                  sent "ls"                   "$LIST? "
sup a@acme.example;                      sent "<address>"            "$LIST?address=$ADDR "
case "$OUT" in "No suppressed addresses matching that one.") ok ;; *) die "<address>: printed: $OUT" ;; esac
sup list a@acme.example;                 sent "list <address>"       "$LIST?address=$ADDR "
sup --json;                              sent "--json"               "$LIST? "
[ "$OUT" = '{"suppressions": [], "added": true, "scope": "box", "removed": 1}' ] || die "--json: printed: $OUT"
ok
sup --limit 5;                           sent "--limit"              "$LIST?limit=5 "
sup --json a@acme.example --limit 5;     sent "--json <address> --limit" "$LIST?address=$ADDR&limit=5 "
[ "${OUT:0:1}" = "{" ] || die "--json <address> --limit: not JSON: $OUT"
ok
sup --limit 5 list a@acme.example --json; sent "--limit list <address> --json" "$LIST?address=$ADDR&limit=5 "
[ "${OUT:0:1}" = "{" ] || die "--limit list <address> --json: not JSON: $OUT"
ok
sup a@acme.example --json;               sent "<address> --json"     "$LIST?address=$ADDR "
sup list --limit 2 --json;               sent "list --limit --json"  "$LIST?limit=2 "

# ── add and rm, as before.
ADD="POST /cli/notifications/email/suppressions/add? "
RM="POST /cli/notifications/email/suppressions/rm? "
sup add a@acme.example;                  sent "add"                  "$ADD"'{"address": "a@acme.example"}'
[ "$OUT" = "suppressed (box)" ] || die "add: printed: $OUT"
ok
sup add a@acme.example --sender s1;      sent "add --sender"         "$ADD"'{"address": "a@acme.example", "sender": "s1"}'
sup add --sender s1 a@acme.example;      sent "add --sender first"   "$ADD"'{"address": "a@acme.example", "sender": "s1"}'
sup rm a@acme.example;                   sent "rm"                   "$RM"'{"address": "a@acme.example"}'
[ "$OUT" = "removed 1 suppression(s)" ] || die "rm: printed: $OUT"
ok
sup remove a@acme.example --all;         sent "remove --all"         "$RM"'{"address": "a@acme.example", "all": true}'
sup rm a@acme.example --sender s1 --json; sent "rm --sender --json"  "$RM"'{"address": "a@acme.example", "sender": "s1"}'
[ "${OUT:0:1}" = "{" ] || die "rm --json: not JSON: $OUT"
ok

# add/rm/list are verbs wherever an address could stand first: never an
# address, so `add` with no address is still the usage error.
sup add;                                 refused "add with no address"
sup rm --all;                            refused "rm --all with no address"
sup rm;                                  refused "rm with no address"

# ── Still refused.
sup a@acme.example b@acme.example;       refused "two addresses"
sup list a@acme.example b@acme.example;  refused "list, two addresses"
sup --bogus;                             refused "unknown flag"
sup remvoe a@acme.example;               refused "a misspelt verb"
case "$ERR" in *"usage: k2 notifications email suppressions"*) ok ;; *) die "a misspelt verb: stderr was: $ERR" ;; esac

echo "notifications_suppressions_forms: $pass passed"
