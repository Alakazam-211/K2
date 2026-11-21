#!/usr/bin/env bash
# `k2 thread ask --meta` / `--meta-json` (0.46.17): a choice card's
# metadata map, for an App to read back.
#
# - `--meta key=value` (repeatable) sends a string; `--meta-json '{...}'`
#   sends numbers and booleans too. Both merge, keys sorted, into the body's
#   `meta` object; a key in both is refused.
# - The CLI checks the daemon's limits first, with the daemon's words
#   (k2-core overlay::card_meta): keys [a-z0-9_.-]{1,64}, at most 32 keys,
#   at most 4096 bytes of compact JSON, values strings, numbers or booleans.
#   Every refusal is exit 2 and sends nothing.
# - `--json` passes the daemon's answer (with `meta`) through, and
#   `k2 thread <addr> --json` reads it back. The text read prints no meta
#   line (a card shows its prompt only).
# - With no map the body is byte for byte what it was before.
# - The help says it is not for secrets; `-h` with `--meta` makes no request.
#
# Hermetic: HOME is a throwaway dir, K2_*/K2SO_* unset, the daemon is a tiny
# python stub on loopback that logs each request and records each ask's raw
# body. No real daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

SANDBOX="$(mktemp -d -t k2-thread-ask-meta-XXXXXX)"
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

PORT_FILE, BODY_FILE, LOG_FILE = sys.argv[1:4]
CARD = {"seq": 1, "id": "card-meta", "doc": {"id": "card-meta", "kind": "choice", "from": "sales",
        "created_at": 1, "body": "Ship?", "via": "card",
        "choice": {"prompt": "Ship?", "options": [{"label": "Go"}, {"label": "Stop"}], "allow_custom": False,
                   "status": "pending", "expiresAt": 2, "meta": {"order": "A-17", "qty": 3, "rush": True}}}}

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
        with open(LOG_FILE, "a") as f:
            f.write("GET %s\n" % u.path)
        if u.path == "/cli/thread":
            self.reply(200, {"ok": True, "collection": "thread", "addr": "ledger", "conversation_id": "c1",
                             "items": [CARD], "has_more": False})
        else:
            self.reply(404, {"ok": False, "error": "unexpected route"})
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        with open(LOG_FILE, "a") as f:
            f.write("POST %s\n" % u.path)
        if u.path != "/cli/thread/ask":
            self.reply(404, {"ok": False, "error": "unexpected route"})
            return
        with open(BODY_FILE, "wb") as f:
            f.write(raw)
        req = json.loads(raw.decode() or "{}")
        meta = req.get("meta")
        if isinstance(meta, dict) and "daemon.refuses" in meta:
            self.reply(400, {"ok": False, "error": {"code": "usage", "hint": "meta takes at most 32 keys"}})
            return
        obj = {"ok": True, "id": "card-meta", "seq": 7, "kind": "choice", "status": "pending",
               "prompt": req.get("prompt"), "options": [], "conversation_id": "c1", "addr": req.get("addr")}
        if meta is not None:
            obj["meta"] = meta
        self.reply(200, obj)

srv = HTTPServer(("127.0.0.1", 0), H)
with open(PORT_FILE, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

: > "$SANDBOX/log"
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" "$SANDBOX/body" "$SANDBOX/log" &
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
# The sent body's `meta` (compact, keys sorted), or `null` when it has none.
sent_meta() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps(d.get("meta"), sort_keys=True, separators=(",", ":")))' "$SANDBOX/body"; }
requests() { wc -l < "$SANDBOX/log" | tr -d ' '; }
ask() { # ask <args...>: thread ask, stdout to $out, stderr to $err, exit to $rc
    # The markdown hint prints on a workspace's 1st and every 10th write;
    # a count of 1 keeps it off this stderr.
    printf '1\n' > "$SANDBOX/ws/.k2/.thread-md-nudge"
    rm -f "$SANDBOX/body"
    set +e
    out="$("$K2_CLI" thread ask ledger "Ship?" --option Go --option Stop "$@" 2>"$SANDBOX/err")"; rc=$?
    set -e
    err="$(cat "$SANDBOX/err")"
}
refused() { # refused <label> <stderr line> <args...>: exit 2, that line, no request
    local label="$1" want="$2"
    shift 2
    local before
    before="$(requests)"
    ask "$@"
    assert_eq "$label: exit 2" "$rc" "2"
    assert_eq "$label: the refusal" "$err" "Error: $want"
    assert_eq "$label: nothing sent" "$(requests)" "$before"
}

echo "== --meta sends strings; '=' splits once =="
ask --meta order=A-17 --meta "note=a=b c"
assert_eq "set: exit 0" "$rc" "0"
assert_eq "set: the card line" "$out" "card-meta  card #7"
assert_eq "set: the body's meta" "$(sent_meta)" '{"note":"a=b c","order":"A-17"}'

echo "== --meta-json sends numbers and booleans =="
ask --meta-json '{"rush": true, "qty": 3, "weight.kg": 1.5, "who": "ada"}'
assert_eq "json: exit 0" "$rc" "0"
assert_eq "json: types kept" "$(sent_meta)" '{"qty":3,"rush":true,"weight.kg":1.5,"who":"ada"}'

echo "== both merge, keys sorted, in the body as sent =="
ask --meta-json '{"qty": 3}' --meta order=A-17 --idempotency-key key-merge-1
assert_eq "merge: exit 0" "$rc" "0"
assert_contains "merge: the body's bytes" "$(cat "$SANDBOX/body")" '"idempotency_key": "key-merge-1", "option": ["Go", "Stop"], "meta": {"order": "A-17", "qty": 3}}'

echo "== --json passes the answer's meta through =="
ask --meta order=A-17 --json
assert_eq "answer: exit 0" "$rc" "0"
assert_eq "answer: meta on stdout" "$(python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1])["meta"]))' "$out")" '{"order": "A-17"}'

echo "== k2 thread <addr> --json reads it back; the text read shows the prompt only =="
set +e
read_json="$("$K2_CLI" thread ledger --json 2>"$SANDBOX/err")"; rc=$?
set -e
assert_eq "read --json: exit 0" "$rc" "0"
assert_eq "read --json: the card's meta" "$(python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1])["items"][0]["doc"]["choice"]["meta"], sort_keys=True))' "$read_json")" '{"order": "A-17", "qty": 3, "rush": true}'
set +e
read_text="$("$K2_CLI" thread ledger 2>"$SANDBOX/err")"; rc=$?
set -e
assert_eq "read text: exit 0" "$rc" "0"
assert_eq "read text: no meta line" "$read_text" "[1] from sales: Ship?"

echo "== a daemon refusal is exit 2 with its words =="
ask --meta daemon.refuses=1
assert_eq "daemon refusal: exit 2" "$rc" "2"
assert_eq "daemon refusal: its words" "$err" "usage: meta takes at most 32 keys"

echo "== each limit is refused before anything is sent =="
NOT_AN_OBJECT="meta must be a JSON object of keys to strings, numbers or booleans"
refused "a key in both" 'meta key "order" is given twice' --meta-json '{"order": "B"}' --meta order=A
refused "a --meta key twice" 'meta key "order" is given twice' --meta order=A --meta order=B
refused "--meta without '='" "--meta needs key=value" --meta order
refused "--meta with no value" "--meta needs key=value" --meta
refused "--meta-json with no value" "--meta-json needs a JSON object" --meta-json
refused "--meta-json twice" "--meta-json is given twice" --meta-json '{}' --meta-json '{}'
refused "--meta-json not JSON" "$NOT_AN_OBJECT" --meta-json 'order=A'
refused "--meta-json a list" "$NOT_AN_OBJECT" --meta-json '[1, 2]'
refused "--meta-json NaN" "$NOT_AN_OBJECT" --meta-json '{"n": NaN}'
refused "--meta-json out of range" "$NOT_AN_OBJECT" --meta-json '{"n": 1e400}'
refused "a null value" 'meta value for "k" must be a string, number or boolean' --meta-json '{"k": null}'
refused "a list value" 'meta value for "k" must be a string, number or boolean' --meta-json '{"k": [1]}'
refused "an object value" 'meta value for "k" must be a string, number or boolean' --meta-json '{"k": {"a": 1}}'
refused "an upper-case key" 'meta key "Order" must match [a-z0-9_.-]{1,64}' --meta Order=A
refused "an empty key" 'meta key "" must match [a-z0-9_.-]{1,64}' --meta =A
long_key="$(printf 'k%.0s' $(seq 1 65))"
refused "a 65-byte key" "meta key \"$long_key\" must match [a-z0-9_.-]{1,64}" --meta "$long_key=A"
keys33="$(python3 -c 'import json; print(json.dumps({"k%d" % i: i for i in range(33)}))')"
refused "33 keys" "meta takes at most 32 keys" --meta-json "$keys33"
big="$(python3 -c 'print("v" * 4089)')"
refused "4097 bytes" "meta takes at most 4096 bytes of JSON" --meta "k=$big"

echo "== at the limits it is sent =="
keys32="$(python3 -c 'import json; print(json.dumps({"k%d" % i: i for i in range(32)}))')"
ask --meta-json "$keys32"
assert_eq "32 keys: exit 0" "$rc" "0"
ask --meta "$(printf 'k%.0s' $(seq 1 64))=A"
assert_eq "a 64-byte key: exit 0" "$rc" "0"
ask --meta "k=$(python3 -c 'print("v" * 4088)')"
assert_eq "4096 bytes: exit 0" "$rc" "0"
assert_eq "4096 bytes: sent whole" "$(python3 -c 'import json,sys; print(len(json.dumps(json.load(open(sys.argv[1]))["meta"], separators=(",", ":"))))' "$SANDBOX/body")" "4096"

echo "== no map: the body is what it was before =="
ask --idempotency-key key-plain-1
assert_eq "plain: exit 0" "$rc" "0"
want="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps({"addr": "ledger", "prompt": "Ship?", "from": d["from"], "idempotency_key": "key-plain-1", "option": ["Go", "Stop"]}))' "$SANDBOX/body")"
assert_eq "plain: the body's bytes" "$(cat "$SANDBOX/body")" "$want"
ask --idempotency-key key-plain-1 --meta-json '{}'
assert_eq "empty map: exit 0" "$rc" "0"
assert_eq "empty map: the same bytes" "$(cat "$SANDBOX/body")" "$want"

echo "== the help says it is not for secrets; -h makes no request =="
help_out="$("$K2_CLI" thread --help 2>&1)"
assert_contains "help: --meta" "$help_out" "[--meta key=value]..."
assert_contains "help: not for secrets" "$help_out" "NOT for secrets: whoever can see the card sees it."
before="$(requests)"
for h in -h --help; do
    set +e
    "$K2_CLI" thread ask ledger "Ship?" --option Go --meta order=A "$h" >/dev/null 2>&1; rc=$?
    set -e
    assert_eq "$h with --meta: exit 0" "$rc" "0"
done
assert_eq "-h/--help with --meta: no request" "$(requests)" "$before"

echo
echo "thread_ask_meta: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
