#!/usr/bin/env bash
# `k2 thread ask` options (0.46.17). The daemon splits `--options` (k2-core
# `split_ask_options`): commas inside (), [] and "…" stay in one label, and a
# comma that reads like prose ("Yes, node-2…") still splits but the answer
# carries a `hint`. The CLI sends `--options` exactly as typed, sends
# `--option` labels as a list (commas kept), and prints the daemon's hint as
# one stderr line, never a refusal. The help leads with `--option`.
#
# Hermetic: HOME is a throwaway dir, K2_*/K2SO_* unset, the daemon is a tiny
# python stub on loopback that records each body and answers with a hint
# when the list holds a prose-like comma. No real daemon, no ~/.k2, no
# keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

SANDBOX="$(mktemp -d -t k2-thread-ask-opts-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
    hermetic_cli_cleanup
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

HINT = ('an option looks like it continues after a comma ("Yes" / "node-2 (…)"); '
        'to keep commas inside an option, pass each one with --option "…"')

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        req = json.loads(self.rfile.read(n).decode() or "{}") if n else {}
        if u.path == "/cli/thread/ask":
            with open(sys.argv[2], "w") as f:
                json.dump(req, f)
            obj = {"ok": True, "id": "card-opt", "seq": 7, "kind": "choice", "status": "pending",
                   "prompt": req.get("prompt"), "options": [], "conversation_id": "c1", "addr": req.get("addr")}
            if "Yes, node-2" in (req.get("options") or ""):
                obj["hint"] = HINT
            status = 200
        else:
            status, obj = 404, {"ok": False, "error": "unexpected route"}
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" "$SANDBOX/last.json" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
export K2_PORT="$(cat "$SANDBOX/port")"
export K2_HOOK_TOKEN=test-token
mkdir -p "$SANDBOX/ws/.k2"
export K2_PROJECT_PATH="$SANDBOX/ws"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}
assert_contains() {
    if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3"))"; fi
}
assert_not_contains() {
    if grep -Fq -- "$3" <<<"$2"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi
}
sent() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); v=d.get(sys.argv[2]); print(json.dumps(v, ensure_ascii=False))' "$SANDBOX/last.json" "$1"; }
HINT_LINE='hint: an option looks like it continues after a comma ("Yes" / "node-2 (…)"); to keep commas inside an option, pass each one with --option "…"'

echo "== --options with commas inside parentheses: sent as typed, no hint =="
set +e
out="$("$K2_CLI" thread ask ledger "Which mini?" --options "node-2 (an M1, small),k2n1-m4,Wait" 2>"$SANDBOX/err")"; rc=$?
set -e
err="$(cat "$SANDBOX/err")"
assert_eq "parens: exit 0" "$rc" "0"
assert_eq "parens: card line" "$out" "card-opt  card #7"
assert_eq "parens: the list goes to the daemon as typed" "$(sent options)" '"node-2 (an M1, small),k2n1-m4,Wait"'
assert_eq "parens: no option list" "$(sent option)" "null"
assert_not_contains "parens: no hint" "$err" "hint: an option"

echo "== a prose-like comma: the card is up, one hint line on stderr =="
set +e
out="$("$K2_CLI" thread ask ledger "Which mini?" --options "Yes, node-2 (an M1; Sew keeps the M4 and node-3),No" 2>"$SANDBOX/err")"; rc=$?
set -e
err="$(cat "$SANDBOX/err")"
assert_eq "prose: exit 0 (never refused)" "$rc" "0"
assert_eq "prose: card line on stdout" "$out" "card-opt  card #7"
assert_contains "prose: the hint" "$err" "$HINT_LINE"
assert_eq "prose: exactly one hint line" "$(grep -c '^hint: an option' "$SANDBOX/err")" "1"
assert_eq "prose: sent as typed" "$(sent options)" '"Yes, node-2 (an M1; Sew keeps the M4 and node-3),No"'

echo "== --json keeps stdout JSON; the hint stays on stderr =="
set +e
out="$("$K2_CLI" thread ask ledger "Which mini?" --options "Yes, node-2 (an M1),No" --json 2>"$SANDBOX/err")"; rc=$?
set -e
err="$(cat "$SANDBOX/err")"
assert_eq "json: exit 0" "$rc" "0"
assert_eq "json: stdout parses, id kept" "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$out")" "card-opt"
assert_contains "json: hint on stderr" "$err" "$HINT_LINE"

echo "== plain a,b,c is unchanged =="
set +e
"$K2_CLI" thread ask ledger "Pick" --options "a,b,c" >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
assert_eq "plain: exit 0" "$rc" "0"
assert_eq "plain: sent as typed" "$(sent options)" '"a,b,c"'
assert_not_contains "plain: no hint" "$(cat "$SANDBOX/err")" "hint: an option"

echo "== --option keeps commas inside each label =="
set +e
"$K2_CLI" thread ask ledger "Which mini?" --option "Yes, node-2 (an M1; Sew keeps the M4 and node-3)" --option "No, wait" >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
assert_eq "option: exit 0" "$rc" "0"
assert_eq "option: labels sent as a list, commas kept" "$(sent option)" '["Yes, node-2 (an M1; Sew keeps the M4 and node-3)", "No, wait"]'
assert_eq "option: no options string" "$(sent options)" "null"
assert_not_contains "option: no hint" "$(cat "$SANDBOX/err")" "hint: an option"

echo "== mixing the two is still refused =="
set +e
"$K2_CLI" thread ask ledger "?" --option "Go" --options "a,b" >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
assert_eq "mix: exit 1" "$rc" "1"
assert_contains "mix: message" "$(cat "$SANDBOX/err")" "mix of --options and --option is not allowed"

echo "== help leads with --option =="
help_out="$("$K2_CLI" thread --help 2>&1)"
assert_contains "help: --option first on the ask line" "$help_out" 'k2 thread ask <addr> "prompt" --option "…" --option "…"'
assert_contains "help: commas inside are fine" "$help_out" 'repeatable, one per button; commas inside are fine'
assert_contains "help: --options is for short labels" "$help_out" '--options "a,b,c"  short labels without commas'
first_option="$(grep -n -- '--option "…"' <<<"$help_out" | head -1 | cut -d: -f1)"
first_options="$(grep -n -- '--options' <<<"$help_out" | head -1 | cut -d: -f1)"
if [ -n "$first_option" ] && [ -n "$first_options" ] && [ "$first_option" -lt "$first_options" ]; then
    ok "help: --option comes before --options"
else
    bad "help: --option comes before --options (lines $first_option / $first_options)"
fi

echo
echo "thread_ask_options: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
