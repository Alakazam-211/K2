#!/usr/bin/env bash
# App refusals point at the fix (prd-tauri-core-0467-v1 TC-18, TC-19), and
# `k2 explain` reads the registry with no daemon.
#
# A stub daemon on loopback answers three App routes with fixed refusals:
#   GET /cli/app/inspect   → 404 app_not_found      (exit 3)
#   GET /cli/app/errors    → 403 apps_manage_off    (exit 3)
#   GET /cli/app/status    → 400 bad_request        (exit 2)
#   GET /cli/compute/nodes → 403 owner_only         (another family: unchanged)
# For each App refusal, the plain form is the JSON line with `next` and
# `explain` added, then `fix: <next>`; the --json form is the same JSON line
# and no fix: line. Exit codes are what they were. `k2 compute` refusals
# carry neither key.
#
# k2 explain: a live id, a retired id, an alias, an unknown id (exit 4),
# --json, the list by area, a Markdown kit id (handed to k2 doc explain), and
# `k2 study app-errors`.
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, no
# real daemon, no ~/.k2, no keychain. Needs the generated refusal block in
# cli/k2 (cargo run -p k2-core --bin contract-gen).

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
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2rfx-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

ANSWERS = {
    "/cli/app/inspect": (404, "app_not_found", "no App 'nope' on this box (k2 app list)"),
    "/cli/app/errors": (403, "apps_manage_off", "\"Build and run Apps\" is off for this agent"),
    "/cli/app/status": (400, "bad_request", "app is required"),
    "/cli/compute/nodes": (403, "owner_only", "only the server's owner may do this"),
}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        st, code, hint = ANSWERS.get(path, (404, "not_found", "no such route"))
        body = json.dumps({"error": {"code": code, "hint": hint}}).encode()
        self.send_response(st)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    do_POST = do_GET

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

# run <out> <err> <args…> → rc; an agent in a K2 terminal over TCP.
run() {
    local out="$1" err="$2"; shift 2
    set +e
    K2_PORT="$PORT" K2_HOOK_TOKEN=passport-rfx "$K2" "$@" </dev/null >"$out" 2>"$err"
    local rc=$?
    set -e
    return $rc
}
rc_of() { local rc=0; run "$SANDBOX/out" "$SANDBOX/err" "$@" || rc=$?; echo "$rc"; }

echo "== App refusals: plain and --json =="
rc="$(rc_of app inspect nope)"
expect_eq "inspect: exit 3" "$rc" "3"
expect_eq "inspect: the JSON line, next and explain, then fix:" "$(cat "$SANDBOX/err")" \
'{"error": "app_not_found", "hint": "no App '"'"'nope'"'"' on this box (k2 app list)", "next": "List the Apps you can see: k2 app list", "explain": "k2 explain app_not_found"}
fix: List the Apps you can see: k2 app list'
rc="$(rc_of app inspect nope --json)"
expect_eq "inspect --json: exit 3" "$rc" "3"
expect_eq "inspect --json: the JSON line only" "$(cat "$SANDBOX/err")" \
'{"error": "app_not_found", "hint": "no App '"'"'nope'"'"' on this box (k2 app list)", "next": "List the Apps you can see: k2 app list", "explain": "k2 explain app_not_found"}'

rc="$(rc_of app errors jobs)"
expect_eq "errors: exit 3" "$rc" "3"
err="$(cat "$SANDBOX/err")"
fix="$(K2_PORT=1 "$K2" explain apps_manage_off --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["fix"])')"
expect_eq "errors: two lines" "$(printf '%s\n' "$err" | wc -l | tr -d ' ')" "2"
expect_eq "errors: the fix line is the registry's" "$(printf '%s\n' "$err" | sed -n 2p)" "fix: $fix"
expect_has "errors: explain names the id" "$err" '"explain": "k2 explain apps_manage_off"'
rc="$(rc_of app errors jobs --json)"
expect_eq "errors --json: exit 3, one line" "$rc:$(wc -l < "$SANDBOX/err" | tr -d ' ')" "3:1"

rc="$(rc_of app status jobs)"
expect_eq "status: a bad request exits 2" "$rc" "2"
expect_has "status: next" "$(cat "$SANDBOX/err")" '"explain": "k2 explain bad_request"'
expect_has "status: fix line" "$(cat "$SANDBOX/err")" 'fix: '

rc="$(rc_of compute nodes)"
expect_eq "compute nodes: exit 3" "$rc" "3"
expect_eq "compute nodes: another family is unchanged" "$(cat "$SANDBOX/err")" \
'{"error": "owner_only", "hint": "only the server'"'"'s owner may do this"}'

echo "== k2 explain (no daemon) =="
ex() { local rc=0; K2_PORT=1 "$K2" explain "$@" >"$SANDBOX/out" 2>"$SANDBOX/err" </dev/null || rc=$?; echo "$rc"; }
rc="$(ex app_not_found)"
expect_eq "explain live: exit 0" "$rc" "0"
expect_eq "explain live: the first line" "$(head -1 "$SANDBOX/out")" "app_not_found  (deploy · HTTP 404 · exit 3)"
expect_has "explain live: the fix" "$(cat "$SANDBOX/out")" "fix: List the Apps you can see: k2 app list"
expect_has "explain live: who" "$(cat "$SANDBOX/out")" "who fixes it: you (the caller)"
rc="$(ex STATIC_NOT_ON_RUNTIME)"
expect_eq "explain retired: exit 0" "$rc" "0"
expect_has "explain retired: says so" "$(cat "$SANDBOX/out")" "retired: this K2 no longer sends it"
rc="$(ex pairing_needs_k2_front)"
expect_eq "explain alias: the row it maps to" "$rc:$(head -1 "$SANDBOX/out" | cut -d' ' -f1)" "0:needs_k2_front"
rc="$(ex no_such_code)"
expect_eq "explain unknown: exit 4" "$rc" "4"
expect_has "explain unknown: says how to list" "$(cat "$SANDBOX/err")" "k2 explain lists them"
rc="$(ex apps_manage_off --json)"
expect_eq "explain --json: exit 0" "$rc" "0"
expect_eq "explain --json: the row's keys" "$(python3 -c 'import json,sys; print(",".join(sorted(json.load(open(sys.argv[1])).keys())))' "$SANDBOX/out")" \
    "aliases,area,askable,exit,fix,http,id,status,what,who"
expect_eq "explain --json: askable" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["askable"])' "$SANDBOX/out")" "True"
rc="$(ex)"
expect_eq "explain list: exit 0" "$rc" "0"
expect_has "explain list: by area" "$(cat "$SANDBOX/out")" "deploy (validate, build, deploy, rollback):"
expect_has "explain list: retired marked" "$(cat "$SANDBOX/out")" "static_not_on_runtime (retired)"
# A Markdown kit id goes to `k2 doc explain` now that this K2 has k2 doc
# (0.46.7 multiplayer): it reads the kit's registry from the daemon, so
# with none here it is daemon_unreachable, never the old "no k2 doc yet".
rc="$(ex MPC-E301)"
expect_eq "explain kit id: handed to k2 doc explain (no daemon here)" "$rc" "1"
expect_has "explain kit id: asked the daemon" "$(cat "$SANDBOX/err")" "daemon_unreachable"
if grep -q "no k2 doc yet" "$SANDBOX/err"; then bad "explain kit id: still says no k2 doc yet"; else ok "explain kit id: k2 doc is here"; fi
rc="$(ex a b)"
expect_eq "explain two ids: usage exit 2" "$rc" "2"

rc=0; K2_PORT=1 "$K2" study app-errors >"$SANDBOX/out" 2>&1 </dev/null || rc=$?
expect_eq "study app-errors: exit 0" "$rc" "0"
expect_has "study app-errors: the title" "$(head -1 "$SANDBOX/out")" "k2 study app-errors"
expect_has "study app-errors: a row" "$(cat "$SANDBOX/out")" "apps_manage_off"

echo
echo "app_refusal_fix: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
