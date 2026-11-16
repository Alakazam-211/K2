#!/usr/bin/env bash
# k2 compute: flags `k2 --schema` and the help list as other names, and the
# people-only --workspace flags, checked by what reaches the daemon. A tiny
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

echo
echo "compute_flag_aliases_body: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
