#!/usr/bin/env bash
# `k2 doc` sends a value the user typed as the plain text it is, and keeps
# every value the CLI builds itself (numbers, flags, the text, the bytes)
# exactly as before.
#
# Regression (0.46.17): the doc body builder read the markers "@int:N",
# "@true", "@false", "@text" and "@b64" in ANY value, typed ones included.
# `k2 doc version --name @int:x` died in a Python traceback and then still
# POSTed an empty body; `--name @int:5` named the version with the number
# 5, and `say --to @text` sent the message text as the recipient. Markers
# are only meant for values the CLI builds, so a typed value is now plain
# text.
#
# Also: a body the builder can't make (say text that is not UTF-8) is
# refused with one usage error and exit 2, and nothing is sent.
#
# How: a python stub daemon on a free loopback port records every request
# (method, path, body). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/doc_body_markers.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-doc-markers-XXXXXX)"
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
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body.rstrip("\n")))
        out = json.dumps({}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
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

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty.
run() {
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# sends <want-request-line> <args...>: exit 0, nothing on stderr, and the
# stub saw exactly that one request (method, route and body, byte for byte).
sends() {
    local want="$1"; shift
    local label="k2 $*"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" \
        || { bad "$label: exit $RC (want 0)"; printf '%s\n' "$OUT$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    if [ "$(cat "$REQ_LOG")" = "$want" ]; then
        ok "$label: sends $want"
    else
        bad "$label: the request is not: $want"; sed 's/^/      | /' "$REQ_LOG" >&2
    fi
}

D='{"workspace": "ws1", "path": "notes.md"'

echo "== a typed value that looks like a marker is plain text =="
sends "POST /cli/docs/name $D, \"name\": \"@int:x\"}" \
    doc version --name @int:x -w ws1 --json notes.md
sends "POST /cli/docs/name $D, \"name\": \"@int:5\"}" \
    doc version --name @int:5 -w ws1 --json notes.md
sends "POST /cli/docs/name $D, \"name\": \"@true\", \"version\": 3}" \
    doc version --name @true --version 3 -w ws1 --json notes.md
sends "POST /cli/docs/name $D, \"name\": \"@b64\"}" \
    doc version --name=@b64 -w ws1 --json notes.md
sends "POST /cli/docs/say $D, \"text\": \"hi\", \"to\": \"@text\", \"at\": \"@int:x\", \"time\": \"@false\", \"reply_to\": \"\"}" \
    doc say --to @text --at @int:x --time @false -w ws1 --json notes.md hi
sends "POST /cli/docs/share $D, \"with\": \"@int:1\", \"write\": false, \"history\": false, \"comment\": false}" \
    doc share --with @int:1 -w ws1 --json notes.md
sends "POST /cli/docs/claim $D, \"action\": \"take\", \"lines\": \"1,2\", \"note\": \"@true\", \"id\": \"@int:9\"}" \
    doc claim --lines 1,2 --note @true --id @int:9 -w ws1 --json notes.md
sends "POST /cli/docs/undo $D, \"steps\": 1, \"mode\": \"all\", \"forPerson\": \"@text\"}" \
    doc undo --for @text -w ws1 --json notes.md

echo "== the values the CLI builds keep their types, byte for byte =="
sends "POST /cli/docs/name $D, \"name\": \"v1\"}" \
    doc version --name v1 -w ws1 --json notes.md
sends "POST /cli/docs/restore $D, \"version\": 7}" \
    doc restore --version 7 -w ws1 --json notes.md
sends "POST /cli/docs/revert $D, \"change\": 12, \"partial\": true}" \
    doc revert 12 --partial -w ws1 --json notes.md
sends "POST /cli/docs/revert $D, \"change\": 12, \"partial\": false}" \
    doc revert 12 -w ws1 --json notes.md
sends "POST /cli/docs/undo $D, \"steps\": 2, \"mode\": \"partial\", \"forPerson\": \"\"}" \
    doc undo --steps 2 --partial -w ws1 --json notes.md
sends "POST /cli/docs/approve $D, \"change\": 5, \"state\": \"declined\"}" \
    doc decline 5 -w ws1 --json notes.md
sends "POST /cli/docs/share $D, \"with\": \"other\", \"write\": true, \"history\": true, \"comment\": true}" \
    doc share --with other --write --history --comment -w ws1 --json notes.md
sends "POST /cli/docs/claim $D, \"action\": \"take\", \"lines\": \"3,4\", \"ttlMs\": 30000, \"hold\": true}" \
    doc claim --lines 3,4 --ttl 30s --hold -w ws1 --json notes.md
sends "POST /cli/docs/lease $D, \"action\": \"acquire\", \"ttlSecs\": 300}" \
    doc lease --ttl 5m -w ws1 --json notes.md
sends "POST /cli/docs/comments/purge $D, \"thread\": \"ann-1\", \"n\": 3}" \
    doc purge-comments --thread ann-1 --n 3 -w ws1 --json notes.md
sends "POST /cli/docs/comments/purge $D, \"all\": true}" \
    doc purge-comments --all -w ws1 --json notes.md
sends "POST /cli/docs/say $D, \"text\": \"h\\u00e9llo there\", \"to\": \"\", \"at\": \"\", \"time\": \"\", \"reply_to\": \"\"}" \
    doc say -w ws1 --json notes.md "héllo" there
sends "POST /cli/docs/write $D, \"base\": \"live\", \"bytesB64\": \"aMOpbGxvCg==\"}" \
    doc write --base live --text $'h\xc3\xa9llo\n' -w ws1 --json notes.md

echo "== text the builder can't send is refused, nothing sent =="
run doc say -w ws1 notes.md $'caf\xe9'
[ "$RC" -eq 2 ] && ok "say with text that is not UTF-8: exit 2" \
    || bad "say with text that is not UTF-8: exit $RC (want 2)"
WANT_ERR='Error: the text is not valid UTF-8
{"error": {"code": "usage", "hint": "the text is not valid UTF-8"}}'
if [ "$ERR" = "$WANT_ERR" ]; then
    ok "say with text that is not UTF-8: one usage error, no traceback"
else
    bad "say with text that is not UTF-8: stderr is not the usage error"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi
[ -z "$OUT" ] && ok "say with text that is not UTF-8: nothing on stdout" \
    || { bad "say with text that is not UTF-8: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
if [ -s "$REQ_LOG" ]; then
    bad "say with text that is not UTF-8: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
else
    ok "say with text that is not UTF-8: sent nothing"
fi

echo ""
echo "doc_body_markers: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
