#!/usr/bin/env bash
# `k2 thread <addr> --reply-to '#N' "text"` (thread-reply-to-v1, 0.46.18):
# an agent replies to message or card N of its Thread, so it can point its
# person at a card. The CLI sends `replyTo=N` on the post (a leading # taken
# off); the daemon writes the `↩ #N <from>:` line and the quote, stores
# `replyTo`, and refuses an N that isn't on that Thread (its refusal is
# printed as `reply_not_found: …`, exit 2). Usage errors never reach the
# daemon. The help, the human study page and the schema name it.
#
# Hermetic: HOME is a throwaway dir, K2_*/K2SO_* unset, the daemon is a tiny
# python stub on loopback that knows seqs 4 and 13664. No real daemon, no
# ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

SANDBOX="$(mktemp -d -t k2-thread-reply-XXXXXX)"
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
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]
KNOWN = {"4": ("m4", "text"), "13664": ("card-13664", "choice")}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def reply(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        u = urlparse(self.path)
        if u.path == "/boot-status":
            return self.reply(200, {"version": "0", "features": ["send-once", "thread-reply-to-v1"]})
        if u.path == "/cli/workspace/handle":
            return self.reply(200, {"ok": True, "handle": "ledger"})
        return self.reply(404, {"ok": False, "error": "unexpected route"})
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        form = {k: v[0] for k, v in parse_qs(self.rfile.read(n).decode()).items()} if n else {}
        if u.path != "/cli/thread/post":
            return self.reply(404, {"ok": False, "error": "unexpected route"})
        with open(LOG, "a") as f:
            f.write("%s\n" % json.dumps({k: form[k] for k in ("addr", "text", "replyTo") if k in form}, sort_keys=True))
        ref = form.get("replyTo")
        if ref is None:
            return self.reply(200, {"ok": True, "id": "p1", "seq": 20, "from": "ledger", "body": form.get("text")})
        if ref not in KNOWN:
            return self.reply(404, {"ok": False, "error": {"code": "reply_not_found",
                                    "hint": "no message or card #%s in this Thread (a read lists them as [N])" % ref}})
        rid, kind = KNOWN[ref]
        target = {"seq": int(ref)}
        if kind != "text":
            target["cardId"] = rid
        return self.reply(200, {"ok": True, "id": "p2", "seq": 21, "from": "ledger",
                                "body": "↩ #%s ledger:\n> Review it?\n\n  %s" % (ref, form.get("text")), "replyTo": target})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
: > "$LOG"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
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
    if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi
}

echo "== help, study, schema =="
help_out="$("$K2_CLI" thread --help 2>&1)"
assert_contains "help: the reply form" "$help_out" "k2 thread <addr> --reply-to '#N' \"text\""
assert_contains "help: point your person at a card" "$help_out" "To point your person at a card or message, reply to it:"
assert_contains "help: the example" "$help_out" "k2 thread <addr> --reply-to '#<card or seq>' '…'"
study="$("$K2_CLI" study human 2>&1)"
assert_contains "study human: the line" "$study" "To point your person at a card or message, reply to it:"
schema="$("$K2_CLI" --schema 2>&1)"
assert_contains "schema: --reply-to flag" "$schema" '"name": "--reply-to"'

echo "== a reply to a card, by #N =="
: > "$LOG"
set +e
out="$("$K2_CLI" thread ledger --from ledger --reply-to '#13664' "This is the card I meant." 2>/dev/null)"; rc=$?
set -e
assert_eq "card reply: exit 0" "$rc" "0"
assert_eq "card reply: the posted line" "$out" "p2  seq=21  from=ledger"
assert_eq "card reply: replyTo sent without the #" "$(cat "$LOG")" '{"addr": "ledger", "replyTo": "13664", "text": "This is the card I meant."}'

echo "== a reply to a message, by N (leading zeros fold), --json keeps the answer =="
: > "$LOG"
out="$("$K2_CLI" thread ledger --from ledger "ok" --reply-to 0004 --json 2>/dev/null)"
assert_eq "json: replyTo in the answer" "$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(json.dumps(d["replyTo"]))' "$out")" '{"seq": 4}'
assert_eq "message reply: replyTo=4" "$(cat "$LOG")" '{"addr": "ledger", "replyTo": "4", "text": "ok"}'

echo "== a post without --reply-to sends no replyTo =="
: > "$LOG"
"$K2_CLI" thread ledger --from ledger "plain" >/dev/null 2>&1
assert_eq "plain post: no replyTo" "$(cat "$LOG")" '{"addr": "ledger", "text": "plain"}'

echo "== an N that isn't on this Thread is refused, plainly =="
set +e
out="$("$K2_CLI" thread ledger --from ledger --reply-to 77 "hi" 2>&1 >/dev/null)"; rc=$?
set -e
assert_eq "not here: exit 2" "$rc" "2"
assert_eq "not here: the daemon's words" "$out" "reply_not_found: no message or card #77 in this Thread (a read lists them as [N])"

echo "== usage errors (no daemon reached) =="
: > "$LOG"
for args in "--reply-to" "x --reply-to 0" "x --reply-to #0" "x --reply-to ##4" "x --reply-to -1" "x --reply-to 1.5" "x --reply-to #" "--reply-to 4"; do
    set +e
    # shellcheck disable=SC2086
    out="$("$K2_CLI" thread ledger --from ledger $args 2>&1)"; rc=$?
    set -e
    if [ "$rc" -eq 1 ] && grep -q -- "--reply-to" <<<"$out"; then ok "refused: $args"; else bad "refused: $args (rc=$rc out=$out)"; fi
done
assert_eq "no request for a usage error" "$(cat "$LOG")" ""

echo
echo "thread_reply_to: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
