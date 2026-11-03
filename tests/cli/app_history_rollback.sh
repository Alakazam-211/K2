#!/usr/bin/env bash
# 0.46.8: K2 keeps the Seeds of an App's last 5 deployments. `k2 app history`
# marks what you can still roll back to and names the ones K2 is keeping;
# `k2 app rollback --to <freed>` prints the daemon's refusal with the
# registry's fix and exits 3; an older daemon's answer prints as it did.
#
# A stub daemon on loopback answers:
#   GET  /cli/app/history?app=jobs   the marks, and what you can go back to
#   GET  /cli/app/history?app=none   nothing to go back to
#   GET  /cli/app/history?app=older  a daemon before 0.46.8 (no new fields)
#   POST /cli/app/rollback           409 rollback_seed_gone
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, no real
# daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; diff <(printf '%s\n' "$3") <(printf '%s\n' "$2") >&2 || true; fi; }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2ahr-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from urllib.parse import urlparse, parse_qs
from http.server import BaseHTTPRequestHandler, HTTPServer

def dep(seed, result, live, stored=None, rb=False, soon=False, cross=False):
    d = {"id": "d-" + seed[:4], "kind": "deploy", "fromSeed": None, "toSeed": seed * 16, "commitRef": "c0", "actor": "owner",
         "result": result, "startedAt": 1000, "liveAt": 1000 if live else None}
    if stored is not None:
        d.update(seedStored=stored, rollbackable=rb, freedSoon=soon, crossesMigration=cross)
    return d

NEW = {"keepDeployments": 5, "keepUnusedDays": 3, "auditVerified": True, "ok": True}
HIST = {
    "jobs": dict(NEW, app="jobs", liveSeed="aaaa" * 16, rollbackTo=[{"seed": "bbbb" * 16, "startedAt": 1000}, {"seed": "1111" * 16, "startedAt": 900}], deployments=[
        dep("dddd", "failed", False, True),
        dep("eeee", "rolled_back", True, True),
        dep("aaaa", "live", True, True),
        dep("bbbb", "superseded", True, True, rb=True),
        dep("2222", "superseded", True, True, cross=True),
        dep("1111", "superseded", True, True, rb=True),
        dep("3333", "superseded", True, True, rb=True, soon=True),
        dep("cccc", "superseded", True, False),
    ]),
    "none": dict(NEW, app="none", liveSeed="aaaa" * 16, rollbackTo=[], deployments=[dep("aaaa", "live", True, True)]),
    "older": {"ok": True, "app": "older", "liveSeed": "aaaa" * 16, "auditVerified": True,
              "deployments": [dep("aaaa", "live", True), dep("bbbb", "superseded", True)]},
}
HINT = "cccccccccccc last ran on jobs on 2026-10-02 and its Seed has been freed. You can go back to bbbbbbbbbbbb (2026-10-28): k2 app rollback jobs --to <seed>."

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def answer(self, st, v):
        body = json.dumps(v).encode()
        self.send_response(st)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        u = urlparse(self.path)
        if u.path == "/cli/app/history":
            self.answer(200, HIST[parse_qs(u.query)["app"][0]])
        else:
            self.answer(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        self.answer(409, {"error": {"code": "rollback_seed_gone", "hint": HINT}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

k() { K2_PORT="$PORT" K2_HOOK_TOKEN=passport-ahr "$K2" "$@" </dev/null; }

echo "== k2 app history: the marks and what you can go back to =="
out="$(k app history jobs)"
line() { grep -F -- " -> $1" <<<"$out"; }
expect_has "the kept one is marked" "$(line bbbbbbbbbbbb)" "can roll back"
expect_not "the kept one isn't freed soon" "$(line bbbbbbbbbbbb)" "freed soon"
expect_has "one past the last 5 says so" "$(line 333333333333)" "can roll back (freed soon)"
expect_has "across a migration says so" "$(line 222222222222)" "across a data migration: refused"
expect_not "across a migration isn't a way back" "$(line 222222222222)" "can roll back"
expect_not "a crashed deploy is no way back" "$(line eeeeeeeeeeee)" "can roll back"
expect_not "a crashed deploy whose Seed is stored isn't freed" "$(line eeeeeeeeeeee)" "Seed freed"
expect_has "the freed one is marked" "$(line cccccccccccc)" "Seed freed"
expect_has "the live one says live" "$(line aaaaaaaaaaaa)" "<- live"
expect_not "the live one isn't a rollback" "$(line aaaaaaaaaaaa)" "can roll back"
expect_not "a failed deploy never went live: no mark" "$(line dddddddddddd)" "can roll back"
expect_not "a failed deploy: not freed either" "$(line dddddddddddd)" "Seed freed"
expect_has "what you can go back to" "$out" "roll back to: bbbbbbbbbbbb ("
expect_has "each of them" "$out" ", 111111111111 ("
expect_not "never the one next to be freed" "$(grep -F 'roll back to:' <<<"$out")" "333333333333"
expect_has "the command" "$out" "k2 app rollback jobs --to <seed>"
expect_has "the rule" "$out" "K2 keeps the Seeds of the last 5 deployments of each App and frees older ones 3 days after they last ran."
expect_has "the audit line stays" "$out" "audit chain: verifies"

out="$(k app history none)"
expect_has "nothing to go back to" "$out" "roll back: nothing K2 keeps can be rolled back to"

out="$(k app history older)"
expect_not "an older daemon: no marks" "$out" "can roll back"
expect_not "an older daemon: no rule" "$out" "K2 keeps the Seeds"
expect_has "an older daemon: the rows still print" "$out" "<- live"

echo "== k2 app rollback --to a freed Seed =="
rc=0
k app rollback jobs --to cccccccccccc >"$SANDBOX/out" 2>"$SANDBOX/err" || rc=$?
expect_eq "exit 3" "$rc" "3"
err="$(cat "$SANDBOX/err")"
expect_has "the id" "$err" '"error": "rollback_seed_gone"'
expect_has "what you can go back to" "$err" "You can go back to bbbbbbbbbbbb"
expect_has "the registry's fix" "$err" "fix: Go back to a Seed that's still kept (k2 app history <app-id> marks them)"
expect_has "k2 explain" "$err" '"explain": "k2 explain rollback_seed_gone"'

echo "== The docs say 5 and 3, never 10 and 30 =="
for page in "study app-deploys" "study app-resources"; do
    # shellcheck disable=SC2086
    body="$(K2_PORT=1 "$K2" $page </dev/null)"
    expect_has "$page: 5" "$body" "last 5"
    expect_has "$page: counts what stayed live" "$body" "stayed"
    expect_has "$page: 3 days" "$body" "3 days"
    expect_not "$page: not the last 10" "$body" "last 10"
done
body="$(K2_PORT=1 "$K2" study app-resources </dev/null)"
expect_not "study app-resources: no 30 days on Seeds" "$body" "unused 30"
expect_has "study app-resources: the sweep and the guard" "$body" "unless the guard is off"
body="$(K2_PORT=1 "$K2" study app-deploys </dev/null)"
expect_has "study app-deploys: the sweep and the guard" "$body" "With the disk guard off the daily"

echo
echo "app_history_rollback: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
