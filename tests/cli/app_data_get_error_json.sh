#!/usr/bin/env bash
# `k2 app data get`'s own error lines are valid JSON whatever the path.
#
# Regression (0.46.17): the data_changed and local_write lines put the
# App path and the --to path into the JSON as typed, so a `"` or `\` in
# either broke the line. They now go through _compute_json.
#
# Rows:
#   - data_changed: the stub answers the first piece with size 10 and the
#     second with size 11; the path holds `"` and `\`. Exit 3, one JSON
#     line on stderr that python3's json reads back to exactly the error
#     and the hint with the path as typed, and the part file is gone.
#   - local_write: --to names a folder that doesn't exist, with `"` and
#     `\` in it. Exit 3, the same check on the hint, and nothing sent.
#
# How: a python stub daemon on a free loopback port records every request.
# HOME is a temp dir, no real daemon, no network. Loud: every failure is
# named and the script exits 1.
# Run with: bash tests/cli/app_data_get_error_json.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-app-data-json-XXXXXX)"
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
mkdir -p "$WORK/proj/.k2" "$WORK/out"

python3 - "$WORK" <<'PYEOF' &
import base64, json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, u.path, u.query))
        off = int(urllib.parse.parse_qs(u.query).get("offset", ["0"])[0])
        # The file grows between the two pieces: size 10, then 11.
        out = {"data": base64.b64encode(b"hello").decode(), "size": 10 if off == 0 else 11,
               "mtime": 1, "eof": False, "offset": off, "length": 5}
        raw = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
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

run() {
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
}

show() { sed 's/^/      | /' "$1" >&2; }

# json_line <label> <error> <hint>: stderr holds exactly one line that
# starts with `{`, and json reads it to exactly {"error", "hint"}.
json_line() {
    local label="$1" err="$2" hint="$3"
    if K2_T_ERR="$err" K2_T_HINT="$hint" python3 -c '
import json, os, sys
lines = [l for l in open(sys.argv[1], encoding="utf-8").read().split("\n") if l.startswith("{")]
if len(lines) != 1:
    sys.exit("want one JSON line on stderr, got %d" % len(lines))
v = json.loads(lines[0])
want = {"error": os.environ["K2_T_ERR"], "hint": os.environ["K2_T_HINT"]}
if v != want:
    sys.exit("got %r, want %r" % (v, want))
' "$WORK/err"; then
        ok "$label: one JSON line, read back exactly"
    else
        bad "$label: the JSON line"; show "$WORK/err"
    fi
}

APP_PATH='var/say "hi"\back.txt'

echo "== data_changed: a quote and a backslash in the App path =="
# native: app data get
run app data get shop "$APP_PATH" --to "$WORK/out/got.txt"
[ "$RC" -eq 3 ] && ok "data_changed: exit 3" || { bad "data_changed: exit $RC (want 3)"; show "$WORK/err"; }
json_line "data_changed" data_changed "$APP_PATH changed while it was read (size 10 → 11): get it again"
[ -z "$OUT" ] && ok "data_changed: nothing on stdout" || { bad "data_changed: stdout"; printf '%s\n' "$OUT" >&2; }
[ "$(grep -c '^GET /cli/app/data/get ' "$REQ_LOG")" = 2 ] && ok "data_changed: two pieces asked" \
    || { bad "data_changed: not two pieces"; show "$REQ_LOG"; }
[ -z "$(ls -A "$WORK/out")" ] && ok "data_changed: no part file and no file left" \
    || { bad "data_changed: files left in --to's folder"; ls -la "$WORK/out" >&2; }

echo "== local_write: a quote and a backslash in --to =="
TO="$WORK/no such \"dir\"\\x/got.txt"
# native: app data get
run app data get shop var/a.txt --to "$TO"
[ "$RC" -eq 3 ] && ok "local_write: exit 3" || { bad "local_write: exit $RC (want 3)"; show "$WORK/err"; }
json_line "local_write" local_write "can't write $TO"
[ -z "$OUT" ] && ok "local_write: nothing on stdout" || { bad "local_write: stdout"; printf '%s\n' "$OUT" >&2; }
[ -s "$REQ_LOG" ] && { bad "local_write: sent a request"; show "$REQ_LOG"; } || ok "local_write: sent nothing"

echo ""
echo "app_data_get_error_json: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
