#!/usr/bin/env bash
# k2 compute node pinned-only <node> on|off: CLI shapes against a tiny stub
# daemon on loopback (python3 http.server). No real daemon, no vault, no
# keychain, no ~/.k2: HOME is a throwaway dir.
#
# Covers: on and off post /cli/compute/nodes/pinned-only {node, on} with a
# JSON true or false and render the answer; "already" is said when nothing
# changed; a missing or wrong word is a usage error (exit 2) with no
# request; the daemon's owner_only refusal exits 3; `k2 compute nodes`
# marks a pinned-only node; the usage text and `k2 study compute` name the
# verb and say it stays the owner's.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
# A here-string, not a pipe: with pipefail, `printf … | grep -q` fails when
# grep finds its match early and closes the pipe on a long text (the
# usage), and that read as "missing" on Linux.
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2po-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]
STATE = {"mini-9": False}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def note(self, path, body):
        token = (parse_qs(urlparse(self.path).query).get("token") or [""])[0]
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": path, "body": body, "token": token}) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        self.note(u.path, {})
        if u.path == "/cli/compute/nodes":
            nodes = []
            for name in ("mini-9", "mini-10"):
                n = {"id": name, "name": name, "state": "active", "online": True, "labels": {"os": "macos", "arch": "arm64"},
                     "queued": 0, "running": 0, "lastOffer": None}
                if STATE.get(name):
                    n["pinnedOnly"] = True
                nodes.append(n)
            return self.reply(200, {"nodes": nodes})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        self.note(u.path, body)
        if u.path == "/cli/compute/nodes/pinned-only":
            node, on = body.get("node"), body.get("on")
            if node == "locked":
                return self.reply(403, {"error": {"code": "owner_only", "hint": "only the server's owner (or an admin, where noted) can do this"}})
            if not isinstance(on, bool):
                return self.reply(400, {"error": {"code": "bad_request", "hint": "on is true or false"}})
            changed = STATE.get(node, False) != on
            STATE[node] = on
            note = ("automatic placement no longer chooses this node; a job runs here only when it names the node. Jobs already queued or running here are not moved"
                    if on else "automatic placement may choose this node again")
            return self.reply(200, {"ok": True, "node": node, "pinnedOnly": on, "changed": changed, "note": note})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
PORT="$(cat "$SANDBOX/port")"

# Owner-style environment: port + token from a throwaway ~/.k2, no session.
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

last() { tail -1 "$LOG" | python3 -c "import json,sys; v=json.load(sys.stdin); print(json.dumps(v[sys.argv[1]], sort_keys=True) if not isinstance(v[sys.argv[1]], str) else v[sys.argv[1]])" "$1"; }

echo "== on and off =="
out="$("$K2" compute node pinned-only mini-9 on </dev/null)"
expect_has "on renders" "$out" "mini-9: pinned-only on. automatic placement no longer chooses this node"
expect_eq "on path" "$(last path)" "/cli/compute/nodes/pinned-only"
expect_eq "on body: a JSON true" "$(last body)" '{"node": "mini-9", "on": true}'
expect_eq "sent with the owner token" "$(last token)" "owner-test-token"
out="$("$K2" compute node pinned-only mini-9 on </dev/null)"
expect_has "on again says it already was" "$out" "mini-9: pinned-only on (it already was)."
out="$("$K2" compute nodes </dev/null)"
expect_has "the node list marks it" "$(grep 'mini-9 ' <<<"$out")" "(pinned only)"
if grep 'mini-10' <<<"$out" | grep -F '(pinned only)' >/dev/null; then bad "another node is marked too"; else ok "another node is not marked"; fi
out="$("$K2" compute node pinned-only mini-9 off </dev/null)"
expect_has "off renders" "$out" "mini-9: pinned-only off. automatic placement may choose this node again."
expect_eq "off body: a JSON false" "$(last body)" '{"node": "mini-9", "on": false}'
out="$("$K2" compute nodes </dev/null)"
if grep -Fq '(pinned only)' <<<"$out"; then bad "still marked after off"; else ok "the mark goes with off"; fi

echo "== usage and refusals =="
n_before="$(wc -l < "$LOG")"
for args in "mini-9" "mini-9 maybe" "mini-9 on extra" ""; do
    set +e
    # shellcheck disable=SC2086 # the words are separate arguments on purpose
    err="$("$K2" compute node pinned-only $args 2>&1 </dev/null >/dev/null)"; rc=$?
    set -e
    expect_eq "'$args' exits 2" "$rc" "2"
    expect_has "'$args' prints the usage" "$err" "usage: k2 compute node pinned-only <name> on|off"
done
expect_eq "no request for a usage error" "$(wc -l < "$LOG")" "$n_before"
set +e
err="$("$K2" compute node pinned-only locked on 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "the daemon's refusal exits 3" "$rc" "3"
expect_has "refusal code" "$err" "owner_only"

echo "== docs =="
usage="$("$K2" compute --help 2>&1 || true)"
expect_has "usage lists the verb" "$usage" "k2 compute node pinned-only <name> on|off"
expect_has "usage says who" "$usage" "(owner or Admin)"
study="$("$K2" study compute)"
expect_has "study keeps it with the owner" "$study" "k2 compute node pinned-only (a pinned-only node runs only jobs that"

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
