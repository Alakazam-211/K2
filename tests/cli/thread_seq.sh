#!/usr/bin/env bash
# `k2 thread <addr> --seq N` (prd-thread-message-menu-v1): exactly message N,
# whole. A person's reply in the Thread starts `↩ #N <name>:`; this is how
# the agent reads message N. CLI only: the plain read as
# `before_seq=N+1&limit=1`, and anything but seq N there is "not here".
#
# Hermetic: HOME is a throwaway dir, K2_*/K2SO_* unset, the daemon is a tiny
# python stub on loopback serving a fixed Thread (seqs 3, 5, 6; 4 is gone).
# No real daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

SANDBOX="$(mktemp -d -t k2-thread-seq-XXXXXX)"
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
LONG = "word " * 200
ITEMS = [
    {"collection": "thread", "seq": 3, "id": "m3", "doc": {"id": "m3", "kind": "text", "from": "sales", "body": "Ship it?\nOr wait for QA."}},
    {"collection": "thread", "seq": 5, "id": "m5", "doc": {"id": "m5", "kind": "text", "from": "sales", "body": LONG.strip()}},
    {"collection": "thread", "seq": 6, "id": "m6", "doc": {"id": "m6", "kind": "text", "from": "owner", "via": "compose", "body": "↩ #5 sales:\n> word word\n\nok"}},
]

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_GET(self):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        # The read's own params (the CLI also sends project/token, and asks
        # /cli/workspace/handle who it is).
        if u.path == "/cli/thread":
            with open(LOG, "a") as f:
                read = {k: q[k] for k in ("addr", "before_seq", "limit", "since_seq", "after_seq") if k in q}
                f.write("%s %s\n" % (u.path, json.dumps(read, sort_keys=True)))
        if u.path != "/cli/thread":
            body = json.dumps({"ok": False, "error": "unexpected route"}).encode()
        else:
            items = ITEMS
            if "before_seq" in q:
                items = [i for i in items if i["seq"] < int(q["before_seq"])]
            limit = int(q.get("limit", "25"))
            more = limit and len(items) > limit
            if limit:
                items = items[-limit:]
            body = json.dumps({"ok": True, "collection": "thread", "addr": q.get("addr"), "items": items, "has_more": bool(more)}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

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
assert_contains "help names --seq" "$("$K2_CLI" thread --help 2>&1)" 'k2 thread <addr> --seq N'
study="$("$K2_CLI" study human 2>&1)"
assert_contains "study human: the reply marker" "$study" '`↩ #N <name>:`'
assert_contains "study human: the read" "$study" 'Read message N whole: `k2 thread <addr> --seq N`.'
assert_contains "schema: --seq flag" "$("$K2_CLI" --schema 2>&1)" '"name": "--seq"'

echo "== message N, whole =="
: > "$LOG"
out="$("$K2_CLI" thread sales --seq 3 2>&1)"
assert_eq "seq 3 printed whole" "$out" "$(printf '[3] from sales: Ship it?\nOr wait for QA.')"
assert_eq "one read: before_seq N+1, limit 1" "$(cat "$LOG")" '/cli/thread {"addr": "sales", "before_seq": "4", "limit": "1"}'
out="$("$K2_CLI" thread sales --seq 5 2>&1)"
assert_eq "a long message is not cut" "$out" "[5] from sales: $(python3 -c 'print(("word " * 200).strip())')"
out="$("$K2_CLI" thread sales --seq 05 2>&1)"
assert_contains "leading zero is message 5" "$out" "[5] from sales: word"

echo "== --json =="
out="$("$K2_CLI" thread sales --seq 6 --json 2>&1)"
assert_eq "json: just message 6" "$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print([i["seq"] for i in d["items"]], d["has_more"])' "$out")" "[6] False"

echo "== not here =="
set +e
out="$("$K2_CLI" thread sales --seq 4 2>&1)"; rc=$?
set -e
assert_eq "a gone message exits 1" "$rc" "1"
assert_eq "a gone message says so (never message 3)" "$out" "No message 4 on this Thread (k2 thread sales lists them as [N])."
set +e
out="$("$K2_CLI" thread sales --seq 4 --json 2>&1)"; rc=$?
set -e
assert_eq "json: a gone message exits 1 too" "$rc" "1"

echo "== usage errors (no daemon reached) =="
: > "$LOG"
for args in "--seq" "--seq 0" "--seq x" "--seq -2" "hi --seq 3" "--seq 3 --limit 2" "--seq 3 --all" "--seq 3 --before-seq 9" "--seq 3 --file a.md"; do
    set +e
    # shellcheck disable=SC2086
    out="$("$K2_CLI" thread sales $args 2>&1)"; rc=$?
    set -e
    if [ "$rc" -eq 1 ] && grep -q -- "--seq" <<<"$out"; then ok "refused: $args"; else bad "refused: $args (rc=$rc out=$out)"; fi
done
assert_eq "no request for a usage error" "$(cat "$LOG")" ""

echo
echo "thread_seq: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
