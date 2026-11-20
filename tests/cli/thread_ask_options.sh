#!/usr/bin/env bash
# `k2 thread ask` options (0.46.17). A card's `--options` value is one
# string, split on ';' only (the daemon splits it, k2-core
# `split_ask_options`); a comma never splits. A value with a comma and no
# ';' stays one option, and the CLI prints one stderr note, exactly:
#   --options splits on ';'; this became one option. Use ';' between options, or repeat --option
# The CLI sends `--options` exactly as typed and `--option` labels as a list
# (commas kept). The daemon's answer carries the same note as `hint`; the
# CLI prints it once, never twice. The help leads with `--option`.
#
# Hermetic: HOME is a throwaway dir, K2_*/K2SO_* unset, the daemon is a tiny
# python stub on loopback that records each body and, like the daemon,
# answers with the note as `hint` for a value with commas and no ';'. No
# real daemon, no ~/.k2, no keychain.

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

NOTE = "--options splits on ';'; this became one option. Use ';' between options, or repeat --option"

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
            raw = req.get("options") or ""
            if "," in raw and ";" not in raw and not raw.strip().startswith("["):
                obj["hint"] = NOTE
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
NOTE="--options splits on ';'; this became one option. Use ';' between options, or repeat --option"
ask() { # ask <args...>: run thread ask, stdout to $out, stderr to $SANDBOX/err, exit to $rc
    set +e
    out="$("$K2_CLI" thread ask ledger "Rewrite the history?" "$@" 2>"$SANDBOX/err")"; rc=$?
    set -e
    err="$(cat "$SANDBOX/err")"
}
note_count() { grep -cxF -- "$NOTE" "$SANDBOX/err" || true; }

echo "== an option containing commas, no ';': one option, sent as typed, the note once =="
ask --options "No, the forward clean-up is enough, Yes, rewrite the history too"
assert_eq "commas: exit 0 (never refused)" "$rc" "0"
assert_eq "commas: card line on stdout" "$out" "card-opt  card #7"
assert_eq "commas: sent as typed (the daemon keeps it one option)" "$(sent options)" '"No, the forward clean-up is enough, Yes, rewrite the history too"'
assert_eq "commas: the note, exactly once (the daemon's hint is not printed again)" "$(note_count)" "1"
assert_eq "commas: nothing else on stderr" "$err" "$NOTE"

echo "== 'a; b' goes as typed, no note =="
ask --options "No, the forward clean-up is enough; Yes, rewrite the history too"
assert_eq "semi: exit 0" "$rc" "0"
assert_eq "semi: sent as typed" "$(sent options)" '"No, the forward clean-up is enough; Yes, rewrite the history too"'
assert_eq "semi: no note with a ';'" "$err" ""
ask --options "a; b"
assert_eq "a; b: sent as typed" "$(sent options)" '"a; b"'
assert_eq "a; b: no note" "$err" ""

echo "== no comma: no note =="
ask --options "Done"
assert_eq "plain: exit 0" "$rc" "0"
assert_eq "plain: sent as typed" "$(sent options)" '"Done"'
assert_eq "plain: no note" "$err" ""

echo "== a JSON list (read whole by the daemon) gets no note =="
ask --options '["Yes, now","No"]'
assert_eq "jsonlist: exit 0" "$rc" "0"
assert_eq "jsonlist: no note" "$err" ""

echo "== --json keeps stdout JSON; the note stays on stderr =="
ask --options "Yes, now" --json
assert_eq "json: exit 0" "$rc" "0"
assert_eq "json: stdout parses, id kept" "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$out")" "card-opt"
assert_eq "json: the note on stderr" "$err" "$NOTE"

echo "== repeated --option keeps commas inside each label, no note =="
ask --option "No, the forward clean-up is enough" --option "Yes, rewrite the history too"
assert_eq "option: exit 0" "$rc" "0"
assert_eq "option: labels sent as a list, commas kept" "$(sent option)" '["No, the forward clean-up is enough", "Yes, rewrite the history too"]'
assert_eq "option: no options string" "$(sent options)" "null"
assert_eq "option: no note" "$err" ""

echo "== mixing the two is still refused, with no note =="
ask --option "Go" --options "a,b"
assert_eq "mix: exit 1" "$rc" "1"
assert_contains "mix: message" "$err" "mix of --options and --option is not allowed"
assert_eq "mix: no note" "$(note_count)" "0"

echo "== help leads with --option and shows ';' =="
help_out="$("$K2_CLI" thread --help 2>&1)"
assert_contains "help: --option first on the ask line" "$help_out" 'k2 thread ask <addr> "prompt" --option "…" --option "…"'
assert_contains "help: commas inside are fine" "$help_out" 'repeatable, one per button; commas inside are fine'
assert_contains "help: --options splits on ';'" "$help_out" '--options "a; b; c"  one string, split on '"';'"' only'
assert_not_contains "help: no comma list" "$help_out" '--options "a,b'
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
