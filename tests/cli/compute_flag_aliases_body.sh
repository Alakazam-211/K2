#!/usr/bin/env bash
# k2 compute: flags `k2 --schema` and the help list as other names, the
# people-only --workspace flags, and a few more once-hidden flags, checked
# by what reaches the daemon. A tiny
# stub daemon on loopback (python3 http.server) records every request and
# refuses it, so nothing runs. No real daemon, no ~/.k2, no keychain: HOME
# is a throwaway dir.
#
# Covers:
#   run --dry-run    sends the same request as run --plan
#   run --artifacts  sends the same request as run --artifact
#   --workspace WS   names that workspace on move, cancel (--workspace-all),
#                    prewarm, rightsize and logs --search; without it the
#                    CLI names the checkout's own workspace instead
#   test --partition hash|count   reaches nextest's --partition (default
#                    hash; only with --shards above 1); another value: exit 2
#   node admin list --limit N, node servers list --refresh: in the query
#   local status --json   prints the daemon's answer as it came
#   k2 --schema lists each of them on its verb

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_ne() { if [ "$2" != "$3" ]; then ok "$1"; else bad "$1 (both $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2alias-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def refuse(self):
        b = json.dumps({"error": {"code": "no_node_fits", "hint": "stub: nothing runs here"}}).encode()
        self.send_response(404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def record(self, entry):
        with open(LOG, "a") as f:
            f.write(json.dumps(entry, sort_keys=True) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        self.record({"method": "GET", "path": u.path, "query": {k: v[0] for k, v in parse_qs(u.query).items()}})
        if u.path == "/cli/compute/local/status":
            b = json.dumps({"installed": False, "elevation": "helper", "stub": "local-status"}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)
            return
        return self.refuse()
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        # A fresh id per submit (it makes a retried POST idempotent): not
        # part of what a flag asks for.
        body.pop("clientJobId", None)
        self.record({"method": "POST", "path": urlparse(self.path).path, "body": body})
        return self.refuse()

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
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

# k2_sent <args…>: run k2 (the stub refuses, so it exits non-zero) and print
# the one request it sent, as sorted JSON. Exactly one request, or a FAIL.
k2_sent() {
    local before after
    before="$(wc -l < "$LOG")"
    "$K2" "$@" >/dev/null 2>&1 || true
    after="$(wc -l < "$LOG")"
    if [ "$((after - before))" != "1" ]; then
        bad "k2 $* sent $((after - before)) requests, want 1"
        echo "{}"
        return 0
    fi
    tail -1 "$LOG"
}
# field <json> <python expression on v>
field() { python3 -c 'import json,sys; v=json.loads(sys.argv[1]); print(json.dumps(eval(sys.argv[2])))' "$1" "$2"; }

echo "== run --dry-run is run --plan =="
plan="$(k2_sent compute run --no-source --os linux --mem 2G --plan -- true)"
dry="$(k2_sent compute run --no-source --os linux --mem 2G --dry-run -- true)"
expect_eq "--plan asks for a plan" "$(field "$plan" 'v["path"], v["body"].get("plan")')" '["/cli/compute/run", true]'
expect_eq "--dry-run sends the same request" "$dry" "$plan"

echo "== run --artifacts is run --artifact =="
one="$(k2_sent compute run --no-source --mem 2G --artifact 'out/*.json' --artifact 'junit.xml' -- true)"
two="$(k2_sent compute run --no-source --mem 2G --artifacts 'out/*.json' --artifacts 'junit.xml' -- true)"
mix="$(k2_sent compute run --no-source --mem 2G --artifact 'out/*.json' --artifacts 'junit.xml' -- true)"
expect_eq "--artifact keeps both globs, in order" "$(field "$one" 'v["body"].get("artifacts")')" '["out/*.json", "junit.xml"]'
expect_eq "--artifacts sends the same request" "$two" "$one"
expect_eq "the two mixed: the same request" "$mix" "$one"

echo "== --workspace names the workspace =="
ws="$(k2_sent compute move --workspace-all --workspace acme-ws --auto)"
own="$(k2_sent compute move --workspace-all --auto)"
expect_eq "move --workspace-all --workspace" "$(field "$ws" 'v["path"], v["body"]["filter"]')" '["/cli/compute/jobs/move", {"scope": "workspace", "workspace": "acme-ws"}]'
expect_ne "move without --workspace names another" "$(field "$own" 'v["body"]["filter"].get("workspace")')" '"acme-ws"'

ws="$(k2_sent compute cancel --workspace-all --workspace acme-ws)"
own="$(k2_sent compute cancel --workspace-all)"
expect_eq "cancel --workspace-all --workspace" "$(field "$ws" 'v["path"], v["body"]["filter"]')" '["/cli/compute/jobs/cancel", {"scope": "workspace", "workspace": "acme-ws"}]'
expect_ne "cancel without --workspace names another" "$(field "$own" 'v["body"]["filter"].get("workspace")')" '"acme-ws"'

ws="$(k2_sent compute prewarm --needs rust@stable --workspace acme-ws)"
own="$(k2_sent compute prewarm --needs rust@stable)"
expect_eq "prewarm --workspace" "$(field "$ws" 'v["path"], v["body"].get("workspace")')" '["/cli/compute/prewarm", "acme-ws"]'
expect_ne "prewarm without --workspace names another" "$(field "$own" 'v["body"].get("workspace")')" '"acme-ws"'

ws="$(k2_sent compute rightsize --tag nightly --workspace acme-ws)"
own="$(k2_sent compute rightsize --tag nightly)"
expect_eq "rightsize --workspace" "$(field "$ws" 'v["path"], v["query"].get("workspace"), v["query"].get("tag")')" '["/cli/compute/rightsize", "acme-ws", "nightly"]'
expect_ne "rightsize without --workspace names another" "$(field "$own" 'v["query"].get("workspace")')" '"acme-ws"'

ws="$(k2_sent compute logs --search 'thread panicked' --workspace acme-ws)"
own="$(k2_sent compute logs --search 'thread panicked')"
expect_eq "logs --search --workspace" "$(field "$ws" 'v["path"], v["query"].get("workspace"), v["query"].get("search")')" '["/cli/compute/logs/search", "acme-ws", "thread panicked"]'
expect_ne "logs --search without --workspace names another" "$(field "$own" 'v["query"].get("workspace")')" '"acme-ws"'

echo "== test --partition picks how nextest splits the shards =="
# script <json>: the sh -c script compute test sends as the job's command.
script() { python3 -c 'import json,sys; print(json.loads(sys.argv[1])["body"]["argv"][2])' "$1"; }
has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3"))"; fi; }
hasnt() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }
count="$(k2_sent compute test --no-source --mem 2G --shards 2 --partition count -- -p k2-core)"
hash="$(k2_sent compute test --no-source --mem 2G --shards 2 --partition hash -- -p k2-core)"
dflt="$(k2_sent compute test --no-source --mem 2G --shards 2 -- -p k2-core)"
one="$(k2_sent compute test --no-source --mem 2G --partition count -- -p k2-core)"
expect_eq "two shards" "$(field "$count" 'v["path"], v["body"].get("shards")')" '["/cli/compute/run", 2]'
has "--partition count reaches nextest" "$(script "$count")" "--partition count:{shard}/{shards}"
has "--partition hash reaches nextest" "$(script "$hash")" "--partition hash:{shard}/{shards}"
has "no --partition: hash" "$(script "$dflt")" "--partition hash:{shard}/{shards}"
hasnt "one shard: no nextest --partition" "$(script "$one")" "--partition"
before="$(wc -l < "$LOG")"
set +e
"$K2" compute test --no-source --mem 2G --shards 2 --partition random -- -p x >/dev/null 2>"$SANDBOX/err"
rc=$?
set -e
expect_eq "--partition random exits 2" "$rc" "2"
has "... and says the values" "$(cat "$SANDBOX/err")" "usage: --partition hash|count"
expect_eq "... and sends nothing" "$(wc -l < "$LOG")" "$before"

echo "== node admin list --limit =="
lim="$(k2_sent compute node admin list --state all --limit 7)"
expect_eq "admin list --limit" "$(field "$lim" 'v["path"], v["query"].get("limit"), v["query"].get("state")')" '["/cli/compute/admin/requests", "7", "all"]'
nolim="$(k2_sent compute node admin list --state all)"
expect_eq "no --limit: no limit key (the daemon's 50)" "$(field "$nolim" 'v["query"].get("limit")')" 'null'

echo "== node servers list --refresh =="
ref="$(k2_sent compute node servers k2n12 list --refresh)"
expect_eq "servers list --refresh" "$(field "$ref" 'v["path"], v["query"].get("node"), v["query"].get("refresh")')" '["/cli/compute/nodes/servers", "k2n12", "1"]'
plain="$(k2_sent compute node servers k2n12)"
expect_eq "no --refresh: no refresh key" "$(field "$plain" 'v["query"].get("node"), v["query"].get("refresh")')" '["k2n12", null]'

echo "== local status --json =="
out="$("$K2" compute local status --json)"
expect_eq "local status --json prints the daemon's answer" "$(python3 -c 'import json,sys; v=json.loads(sys.argv[1]); print(v["stub"], v["installed"])' "$out")" "local-status False"
out="$("$K2" compute local status)"
has "local status without --json: text" "$out" "This computer is not a compute node."
hasnt "... not JSON" "$out" '"stub"'

echo "== --schema lists them =="
schema_flags() {
    "$K2" --schema | python3 -c '
import json, sys
c = {x["name"]: x for x in json.load(sys.stdin)["commands"]}
print(" ".join(sorted(f["name"] for f in c[sys.argv[1]]["flags"])))' "$1"
}
has_flag() { if grep -qw -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (no $3 in: $2)"; fi; }
run_flags="$(schema_flags 'compute run')"
has_flag "schema: run --dry-run" "$run_flags" "--dry-run"
has_flag "schema: run --artifacts" "$run_flags" "--artifacts"
for verb in move cancel prewarm rightsize logs; do
    has_flag "schema: $verb --workspace" "$(schema_flags "compute $verb")" "--workspace"
done
has_flag "schema: test --partition" "$(schema_flags 'compute test')" "--partition"
has_flag "schema: node admin list --limit" "$(schema_flags 'compute node admin list')" "--limit"
has_flag "schema: node servers --refresh" "$(schema_flags 'compute node servers')" "--refresh"
has_flag "schema: local status --json" "$(schema_flags 'compute local status')" "--json"

echo
echo "compute_flag_aliases_body: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
