#!/usr/bin/env bash
# App refusals point at the fix (prd-tauri-core-0467-v1 TC-18, TC-19), and
# `k2 explain` reads the registry with no daemon.
#
# A stub daemon on loopback answers three App routes with fixed refusals:
#   GET /cli/app/inspect   → 404 app_not_found      (exit 3)
#   GET /cli/app/errors    → 403 apps_manage_off    (exit 3)
#   GET /cli/app/status    → 400 bad_request        (exit 2)
#   GET /cli/compute/nodes → 403 owner_only         (another family: unchanged)
#   POST /cli/app/suspend  → 403 role_required      (the route floor's own shape,
#                                                    {"error": <code>, "message": …})
# For each App refusal, the plain form is the JSON line with `next` and
# `explain` added, then `fix: <next>`; the --json form is the same JSON line
# and no fix: line. Exit codes are what they were. `k2 compute` refusals
# carry neither key.
#
# k2 explain: a live id, a retired id, an alias, an unknown id (exit 4),
# --json, the list by area, a Markdown kit id, and `k2 study app-errors`.
#
# A Markdown kit id (MPC-*, MPCE-*) is the kit's explain, on its own read:
#   GET /cli/kit/explain?id=<id>[&json=1]
# with no path. It never depends on the Board files preview: the stub's
# board route answers as a daemon with Board files off (404 not_enabled),
# and k2 explain never asks it. Regression (0.46.17): it went to
# `k2 doc explain <id>`, which took the id as a doc's path, so a file named
# like the id in this folder was opened as the doc. A decoy file of that
# name sits in the folder; the stub logs every request with its body, and
# no request may name it.
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, no
# real daemon, no ~/.k2, no keychain. Needs the generated refusal block in
# cli/k2 (cargo run -p k2-core --bin contract-gen).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; diff <(printf '%s\n' "$3") <(printf '%s\n' "$2") >&2 || true; fi; }
expect_has() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

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
from urllib.parse import parse_qs, urlsplit

ANSWERS = {
    "/cli/app/inspect": (404, "app_not_found", "no App 'nope' on this box (k2 app list)"),
    "/cli/app/errors": (403, "apps_manage_off", "\"Build and run Apps\" is off for this agent"),
    "/cli/app/status": (400, "bad_request", "app is required"),
    "/cli/compute/nodes": (403, "owner_only", "only the server's owner may do this"),
}

# The kit's answers on GET /cli/kit/explain, by id.
KIT = {
    "MPC-E301": (200, {"exit": 0, "stdout": "MPC-E301 the kit's page\nwhat to do next\n"}),
    "mpce-E9": (422, {"id": "MPC-E110", "code": "unknown_id", "message": "no error id mpce-E9",
                      "next": "k2 explain lists every id", "exit": 4}),
}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        with open(sys.argv[2], "a") as f:
            f.write(json.dumps({"method": self.command, "target": self.path, "body": raw}) + "\n")
        st, code, hint = ANSWERS.get(path, (404, "not_found", "no such route"))
        body = json.dumps({"error": {"code": code, "hint": hint}}).encode()
        if path == "/cli/board/file/verb":
            # A daemon with Board files off.
            st = 404
            body = json.dumps({"ok": False, "error": {"code": "not_enabled", "hint": "Board files are a preview"}}).encode()
        if path == "/cli/kit/explain":
            q = parse_qs(urlsplit(self.path).query, keep_blank_values=True)
            st, v = KIT[q["id"][0]]
            if q.get("json") == ["1"] and st == 200:
                v = {"exit": 0, "stdout": json.dumps({"id": q["id"][0], "json": True}) + "\n"}
            body = json.dumps(v).encode()
        if path == "/cli/app/suspend":
            st = 403
            body = json.dumps({"error": "role_required", "required": "admin", "role": "member",
                               "message": "This needs an admin login on this server. You are signed in as member."}).encode()
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
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" "$SANDBOX/requests.jsonl" &
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

echo "== A login below the route's floor (a Member on k2 app suspend) =="
rc="$(rc_of app suspend jobs)"
expect_eq "suspend, Member: exit 3" "$rc" "3"
expect_eq "suspend, Member: says which role, who to ask, and the fix" "$(cat "$SANDBOX/err")" \
'{"error": "role_required", "hint": "This needs an admin login on this server. You are signed in as member.", "next": "Ask someone with that role to do it: an Admin or the owner.", "explain": "k2 explain role_required"}
fix: Ask someone with that role to do it: an Admin or the owner.'
rc=0; K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2" app suspend --help >"$SANDBOX/out" 2>&1 </dev/null || rc=$?
expect_eq "suspend --help: exit 0" "$rc" "0"
expect_has "suspend --help: who may" "$(cat "$SANDBOX/out")" "(the owner, an Admin or its own agent;"
expect_has "suspend --help: not a Member" "$(cat "$SANDBOX/out")" "not a Member); k2 app resume (owner)."
rc=0; K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2" app door >"$SANDBOX/out" 2>&1 </dev/null || rc=$?
expect_eq "app door: exit 0" "$rc" "0"
expect_has "app door: who may" "$(cat "$SANDBOX/out")" "Admin, on any App; an agent, on its own"
expect_has "app door: not a Member" "$(cat "$SANDBOX/out")" "workspace's Apps. Not a Member."

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
# A Markdown kit id: the daemon holds the kit's registry, so with none
# here it is daemon_unreachable, never the old "no k2 doc yet".
rc="$(ex MPC-E301)"
expect_eq "explain kit id: asks the daemon (none here)" "$rc" "1"
expect_has "explain kit id: asked the daemon" "$(cat "$SANDBOX/err")" "daemon_unreachable"
if grep -q "no k2 doc yet" "$SANDBOX/err"; then bad "explain kit id: still says no k2 doc yet"; else ok "explain kit id: k2 doc is here"; fi
rc="$(ex a b)"
expect_eq "explain two ids: usage exit 2" "$rc" "2"

echo "== k2 explain <kit id>: the kit's explain, never a doc path =="
# A decoy file named like each id, here where the command runs.
printf 'a decoy, never read\n' >"$SANDBOX/ws/MPC-E301"
printf 'a decoy, never read\n' >"$SANDBOX/ws/mpce-E9"
# kit_requests: each request as "<method> <target>[ <body>]".
kit_requests() {
    python3 -c '
import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l)
    print(" ".join(x for x in (r["method"], r["target"], r["body"]) if x))
' "$SANDBOX/requests.jsonl"
}
no_decoy() { # no_decoy <label>: no request names the decoy, a path or the board route
    local reqs; reqs="$(kit_requests)"
    if grep -qF -e decoy -e "$SANDBOX/ws" -e '"path"' -e 'path=' -e 'file=' -e /cli/docs/ -e /cli/board/ <<<"$reqs"; then
        bad "$1: a request names the decoy, a path, a docs or a board route: $reqs"
    else
        ok "$1: no request names the decoy, a path, a docs or a board route"
    fi
}
: >"$SANDBOX/requests.jsonl"
rc="$(rc_of explain MPC-E301)"
expect_eq "explain MPC-E301, Board files off: exit 0, the kit's page on stdout" "$rc:$(cat "$SANDBOX/out")" "0:MPC-E301 the kit's page
what to do next"
expect_eq "explain MPC-E301: nothing on stderr" "$(cat "$SANDBOX/err")" ""
expect_eq "explain MPC-E301: one request, the kit's explain with no path" "$(kit_requests)" \
    'GET /cli/kit/explain?id=MPC-E301'
no_decoy "explain MPC-E301"

: >"$SANDBOX/requests.jsonl"
rc="$(rc_of explain MPC-E301 --json)"
expect_eq "explain MPC-E301 --json: exit 0, the kit's JSON" "$rc:$(cat "$SANDBOX/out")" '0:{"id": "MPC-E301", "json": true}'
expect_eq "explain MPC-E301 --json: json=1, no path" "$(kit_requests)" \
    'GET /cli/kit/explain?id=MPC-E301&json=1'
no_decoy "explain MPC-E301 --json"

: >"$SANDBOX/requests.jsonl"
rc="$(rc_of explain mpce-E9)"
expect_eq "explain mpce-E9 (unknown): the kit's row, its exit" "$rc" "4"
expect_eq "explain mpce-E9: the row on stderr" "$(cat "$SANDBOX/err")" 'Error [unknown_id] MPC-E110: no error id mpce-E9
Next: k2 explain lists every id'
expect_eq "explain mpce-E9: sent as typed" "$(kit_requests)" \
    'GET /cli/kit/explain?id=mpce-E9'
no_decoy "explain mpce-E9"
expect_eq "the decoys are untouched" "$(cat "$SANDBOX/ws/MPC-E301" "$SANDBOX/ws/mpce-E9")" "a decoy, never read
a decoy, never read"

rc=0; K2_PORT=1 "$K2" study app-errors >"$SANDBOX/out" 2>&1 </dev/null || rc=$?
expect_eq "study app-errors: exit 0" "$rc" "0"
expect_has "study app-errors: the title" "$(head -1 "$SANDBOX/out")" "k2 study app-errors"
expect_has "study app-errors: a row" "$(cat "$SANDBOX/out")" "apps_manage_off"

echo
echo "app_refusal_fix: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
