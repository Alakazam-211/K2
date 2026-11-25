#!/usr/bin/env bash
# This server's node labels (0.46.18): `k2 compute node label|labels` and
# `k2 compute run --label WORD --for VERSION`, CLI shapes against a tiny
# stub daemon on loopback (python3 http.server). No real daemon, no vault,
# no keychain, no ~/.k2: HOME is a throwaway dir.
#
# Covers: label add/remove post /cli/compute/nodes/labels/set {node,
# add|remove: [...]} and render the answer ("(no change)" when nothing
# changed); labels GETs /cli/compute/nodes/labels (?node= for one) and
# prints one line per node; bad words are a usage error (exit 2) with no
# request; the daemon's owner_only refusal exits 3; run sends --label WORD
# as serverLabels and --label k=v as labels, --for as forVersion, and
# leaves both keys out when not asked; an empty --label is a usage error;
# the usage text and `k2 study compute` name the verbs.

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
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2lb-cli-XXXXXX)"
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
LABELS = {"node-a": [], "node-b": []}
NOTE_SET = "this server's own labels: only its jobs (--label WORD) see them; jobs already queued or running are not moved"
NOTE_LIST = "these labels are this server's own: the node and other servers that use it never see them"

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
    def note(self, path, body, query):
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": path, "body": body, "query": query}) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items() if k != "token"}
        self.note(u.path, {}, q)
        if u.path == "/cli/compute/nodes/labels":
            rows = [{"node": n, "id": n, "labels": sorted(l)} for n, l in sorted(LABELS.items()) if q.get("node") in (None, n)]
            return self.reply(200, {"nodes": rows, "note": NOTE_LIST})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        self.note(u.path, body, {})
        if u.path == "/cli/compute/nodes/labels/set":
            node = body.get("node")
            if node == "locked":
                return self.reply(403, {"error": {"code": "owner_only", "hint": "only the server's owner (or an admin, where noted) can do this"}})
            have = set(LABELS.setdefault(node, []))
            before = set(have)
            have -= set(body.get("remove") or [])
            have |= set(body.get("add") or [])
            LABELS[node] = sorted(have)
            return self.reply(200, {"ok": True, "node": node, "labels": sorted(have), "changed": have != before, "note": NOTE_SET})
        if u.path == "/cli/compute/run":
            return self.reply(400, {"error": {"code": "bad_request", "hint": "stub: body logged"}})
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

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

last() { tail -1 "$LOG" | python3 -c "import json,sys; v=json.load(sys.stdin); x=v[sys.argv[1]]; print(x if isinstance(x, str) else json.dumps(x, sort_keys=True))" "$1"; }
# One key of the last request's body ("-" when absent).
last_key() { tail -1 "$LOG" | python3 -c "import json,sys; b=json.load(sys.stdin)['body']; print(json.dumps(b[sys.argv[1]], sort_keys=True) if sys.argv[1] in b else '-')" "$1"; }

echo "== label add / remove =="
# covers: compute node label
out="$("$K2" compute node label node-a add release for:0.46.18 </dev/null)"
expect_eq "add path" "$(last path)" "/cli/compute/nodes/labels/set"
expect_eq "add body" "$(last body)" '{"add": ["release", "for:0.46.18"], "node": "node-a"}'
expect_has "add renders" "$out" "node-a: labels for:0.46.18, release. this server's own labels"
out="$("$K2" compute node label node-a add release </dev/null)"
expect_has "again says no change" "$out" "node-a: labels for:0.46.18, release (no change)."
out="$("$K2" compute node label node-a remove for:0.46.18 </dev/null)"
expect_eq "remove body" "$(last body)" '{"node": "node-a", "remove": ["for:0.46.18"]}'
expect_has "remove renders" "$out" "node-a: labels release."

echo "== labels =="
# covers: compute node labels
out="$("$K2" compute node labels </dev/null)"
expect_eq "labels path" "$(last path)" "/cli/compute/nodes/labels"
expect_has "node-a line" "$out" "node-a: release"
expect_has "node-b line" "$out" "node-b: (no labels)"
expect_has "says they are this server's" "$out" "(these labels are this server's own"
"$K2" compute node labels node-b </dev/null >/dev/null
expect_eq "one node: ?node=" "$(last query)" '{"node": "node-b"}'
# covers: compute node labels --json
out="$("$K2" compute node labels --json </dev/null)"
expect_has "--json is the answer" "$out" '"nodes": ['

echo "== usage and refusals =="
n_before="$(wc -l < "$LOG")"
for args in "node-a" "node-a add" "node-a tag x" ""; do
    set +e
    # shellcheck disable=SC2086 # separate words on purpose
    err="$("$K2" compute node label $args 2>&1 </dev/null >/dev/null)"; rc=$?
    set -e
    expect_eq "label '$args' exits 2" "$rc" "2"
    expect_has "label '$args' prints the usage" "$err" "usage: k2 compute node label <node> add|remove <label>..."
done
set +e
err="$("$K2" compute node labels a b 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "labels with two nodes exits 2" "$rc" "2"
expect_eq "no request for a usage error" "$(wc -l < "$LOG")" "$n_before"
set +e
err="$("$K2" compute node label locked add x 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "the daemon's refusal exits 3" "$rc" "3"
expect_has "refusal code" "$err" "owner_only"

echo "== run --label WORD / --for =="
set +e
"$K2" compute run --label release --label os=linux --label for:0.46.18 --for 0.46.19 --mem 1G --expect 1m --no-source -- true </dev/null >/dev/null 2>&1
set -e
expect_eq "run path" "$(last path)" "/cli/compute/run"
expect_eq "words go to serverLabels" "$(last_key serverLabels)" '["release", "for:0.46.18"]'
expect_eq "k=v stays the node's own" "$(last_key labels)" '{"os": "linux"}'
expect_eq "--for is forVersion" "$(last_key forVersion)" '"0.46.19"'
set +e
"$K2" compute run --mem 1G --expect 1m --no-source -- true </dev/null >/dev/null 2>&1
set -e
expect_eq "no word: no serverLabels key" "$(last_key serverLabels)" "-"
expect_eq "no --for: no forVersion key" "$(last_key forVersion)" "-"
expect_eq "labels stays an empty object" "$(last_key labels)" "{}"
n_before="$(wc -l < "$LOG")"
set +e
err="$("$K2" compute run --label "" --mem 1G -- true 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "an empty --label exits 2" "$rc" "2"
expect_has "an empty --label says how" "$err" "usage: --label key=value | --label WORD"
expect_eq "no request for it" "$(wc -l < "$LOG")" "$n_before"

echo "== docs =="
usage="$("$K2" compute --help 2>&1 || true)"
expect_has "usage lists labels" "$usage" "k2 compute node labels [<node>] [--json]"
expect_has "usage lists label" "$usage" "k2 compute node label <node> add|remove <label>..."
expect_has "usage explains --label WORD" "$usage" "--label WORD (no =, repeatable)"
study="$("$K2" study compute)"
expect_has "study has the section" "$study" "THIS SERVER'S NODE LABELS (0.46.18)"

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
