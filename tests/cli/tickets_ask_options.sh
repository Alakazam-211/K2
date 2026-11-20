#!/usr/bin/env bash
# `k2 tickets ask` options (0.46.17). The CLI splits the choices itself and
# sends them as a list in POST /cli/feedback/create:
#   --options "a; b"   one string, split on ';' only; each label trimmed,
#                      empty ones skipped. A comma never splits.
#   --option "…"       repeatable, one label per button, commas kept.
# A value with a comma and no ';' stays one option, and the CLI prints one
# stderr note, exactly:
#   --options splits on ';'; this became one option. Use ';' between options, or repeat --option
# Mixing --options and --option is a usage error (exit 2). The help shows
# ';' and --option, never a comma list.
#
# Hermetic: HOME is a throwaway dir, K2_* unset, the daemon is a tiny python
# stub on loopback that records each create body. No real daemon, no ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORK="$(mktemp -d -t k2-tickets-options-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

cat > "$WORK/stub.py" <<'PY'
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
counter = {"n": 0}

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.path.split("?")[0] == "/cli/feedback/create":
            counter["n"] += 1
            with open(os.path.join(work, "req-%d.json" % counter["n"]), "wb") as f:
                f.write(body)
            status, resp = 200, {"ok": True, "id": "fb-stub-%08d" % counter["n"],
                                 "title": "t", "kind": "question", "priority": 3,
                                 "status": "waiting"}
        else:
            status, resp = 404, {"error": {"code": "not_found", "hint": "stub: unknown route"}}
        data = json.dumps(resp).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$WORK/stub.py" "$WORK" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/stub.port" ] && break; sleep 0.1; done
[ -s "$WORK/stub.port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/stub.port")"
export K2_HOOK_TOKEN=stub-token
export K2_PROJECT_PATH="$WORK"
cd "$WORK"

PASS=0
FAIL=0
ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
assert_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}
assert_contains() {
    if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3"))"; fi
}
assert_not_contains() {
    if grep -Fq -- "$3" <<<"$2"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi
}

NOTE="--options splits on ';'; this became one option. Use ';' between options, or repeat --option"
N=0
# ask <args...>: one tickets ask; exit in $rc, stderr in $err, the sent
# options (JSON, or null) in $got.
ask() {
    N=$((N + 1))
    set +e
    "$K2_CLI" tickets ask "case $N" "$@" --json >/dev/null 2>"$WORK/err"; rc=$?
    set -e
    err="$(cat "$WORK/err")"
    if [ -f "$WORK/req-$N.json" ]; then
        got="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])).get("options")))' "$WORK/req-$N.json")"
    else
        got="(no request)"
        N=$((N - 1))
    fi
}

echo "== an option containing commas, no ';': one option, and the note =="
ask --options "No, the forward clean-up is enough, Yes, rewrite the history too"
assert_eq "commas: exit 0 (never refused)" "$rc" "0"
assert_eq "commas: one option" "$got" '["No, the forward clean-up is enough, Yes, rewrite the history too"]'
assert_eq "commas: the note, exactly, alone on stderr" "$err" "$NOTE"

echo "== 'a; b' gives two, no note =="
ask --options "a; b"
assert_eq "a; b: exit 0" "$rc" "0"
assert_eq "a; b: two options" "$got" '["a", "b"]'
assert_eq "a; b: no note" "$err" ""
ask --options "No, the forward clean-up is enough; Yes, rewrite the history too"
assert_eq "semi: two options, commas kept" "$got" '["No, the forward clean-up is enough", "Yes, rewrite the history too"]'
assert_eq "semi: no note with a ';'" "$err" ""

echo "== trimmed; empty labels skipped =="
ask --options "  Go ; Stop  ;;"
assert_eq "trim: two options" "$got" '["Go", "Stop"]'
assert_eq "trim: no note" "$err" ""

echo "== no comma: one option, no note =="
ask --options "Done"
assert_eq "plain: one option" "$got" '["Done"]'
assert_eq "plain: no note" "$err" ""

echo "== repeated --option: each label whole, no note =="
ask --option "No, the forward clean-up is enough" --option "Yes, rewrite the history too"
assert_eq "option: exit 0" "$rc" "0"
assert_eq "option: two labels, commas kept" "$got" '["No, the forward clean-up is enough", "Yes, rewrite the history too"]'
assert_eq "option: no note" "$err" ""

echo "== mixing --options and --option is a usage error, nothing sent =="
ask --options "a; b" --option "c"
assert_eq "mix: exit 2" "$rc" "2"
assert_eq "mix: nothing sent" "$got" "(no request)"
assert_contains "mix: message" "$err" "mix of --options and --option is not allowed"
ask --option "c" --options "a; b"
assert_eq "mix (other order): exit 2" "$rc" "2"
assert_eq "mix (other order): nothing sent" "$got" "(no request)"

echo "== help shows ';' and --option, never a comma list =="
help_out="$("$K2_CLI" tickets ask --help 2>&1)"
assert_contains "help: --option" "$help_out" '--option <label>       One choice the human taps instead of typing; repeat it,'
assert_contains "help: --options splits on ';'" "$help_out" "--options <\"a; b; c\">  The choices as one string, split on ';' only"
assert_not_contains "help: no comma list" "$help_out" '--options "Yes,'
assert_not_contains "help: no comma escape" "$help_out" 'Escape a literal comma'

echo
echo "tickets_ask_options: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
