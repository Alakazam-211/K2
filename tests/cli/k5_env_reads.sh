#!/usr/bin/env bash
# Settings once read under K2_ only now go through the env-name reader:
# K5_<NAME>, then K2_<NAME>, then K2SO_<NAME>; empty counts as unset, and
# each keeps its default.
#   1. Top-level reads, from bash's trace: HOST, K2_CONNECT_BASE, two of
#      the K2_TALK_* tunables, NO_OWNER_FALLBACK.
#   2. _feedback_detect_session (extracted, as feedback_detect_session.sh
#      does): SESSION_ID and CELL.
#   3. k2 whoami against a stub daemon: SESSION_ID, CELL, SIDECAR_NAME and
#      PRIMARY reach the query (token and project dropped; no token is logged).
# bash-only: reads a bash -x trace of cli/k2 and lifts a function out of it.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_BASH_SOURCE"
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', wanted '$3')"; fi; }

WORK="$(mktemp -d -t k2-k5-env-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -p "$WORK/home" "$WORK/proj"

# A bare env: nothing from the caller's session or settings.
bare() { env -i PATH="$PATH" HOME="$WORK/home" K2_PORT=1 K2_HOOK_TOKEN=test-token "$@"; }

echo "== 1. top-level reads (trace) =="
# set_to <var> <env…>: the last value _envn_into gave <var>.
set_to() {
    local var="$1" trace line; shift
    trace="$(bare "$@" bash -x "$K2_CLI" --version 2>&1 >/dev/null)" || { echo "--version failed"; return 0; }
    line="$(grep -E "^\\++ printf -v ${var} %s" <<<"$trace" | tail -1)" || { echo "no printf -v $var"; return 0; }
    printf '%s\n' "${line#*%s }"
}
# matrix <var> <NAME> <default>
matrix() {
    local var="$1" n="$2" def="$3"
    eq "$var: unset gives the default" "$(set_to "$var")" "$def"
    eq "$var: K2_$n" "$(set_to "$var" "K2_$n=two")" "two"
    eq "$var: K5_$n" "$(set_to "$var" "K5_$n=five")" "five"
    eq "$var: K5_ wins over K2_" "$(set_to "$var" "K5_$n=five" "K2_$n=two")" "five"
    eq "$var: an empty K5_ is unset" "$(set_to "$var" "K5_$n=" "K2_$n=two")" "two"
    eq "$var: all empty gives the default" "$(set_to "$var" "K5_$n=" "K2_$n=")" "$def"
    eq "$var: K2SO_ last" "$(set_to "$var" "K2SO_$n=old")" "old"
}
matrix HOST HOST 127.0.0.1
matrix K2_CONNECT_BASE CONNECT_BASE https://connect.k2.dev
matrix K2_TALK_SUBMIT_KEY TALK_SUBMIT_KEY 1
matrix K2_TALK_SCOUT_MAX TALK_SCOUT_MAX 20
# no_owner <env…>: what the prelude made of NO_OWNER_FALLBACK.
no_owner() {
    local trace
    trace="$(bare "$@" bash -x "$K2_CLI" --version 2>&1 >/dev/null)" || true
    grep -E '^\++ _k2_no_owner_fallback=' <<<"$trace" | tail -1 | sed 's/.*=//'
}
eq "NO_OWNER_FALLBACK unset: off" "$(no_owner)" ""
eq "K2_NO_OWNER_FALLBACK=1: on" "$(no_owner K2_NO_OWNER_FALLBACK=1)" "1"
eq "K5_NO_OWNER_FALLBACK=1: on" "$(no_owner K5_NO_OWNER_FALLBACK=1)" "1"
eq "K5_NO_OWNER_FALLBACK=0 wins over K2_=1: off" "$(no_owner K5_NO_OWNER_FALLBACK=0 K2_NO_OWNER_FALLBACK=1)" ""

echo "== 2. _feedback_detect_session =="
FN="$(awk '
    /^_feedback_detect_session\(\)/ { grab=1 }
    grab { print }
    grab && /^}/ { exit }
' "$K2_CLI")"
[ -n "$FN" ] || { echo "FAIL: could not extract _feedback_detect_session" >&2; exit 1; }
# detect <env…>: "kind|id"
detect() {
    env -i PATH="$PATH" "$@" FN="$FN" bash -c 'set -euo pipefail; eval "$FN"; _feedback_detect_session; printf "%s|%s" "$FB_SESSION_KIND" "$FB_SESSION_ID"'
}
eq "K5_CELL=sidecar, K5_SESSION_ID" "$(detect K5_CELL=sidecar K5_SESSION_ID=s5)" "sidecar|s5"
eq "K5_CELL wins over K2_CELL" "$(detect K5_CELL=sidecar K2_CELL=canonical K2_SESSION_ID=s2)" "sidecar|s2"
eq "empty K5_CELL is unset" "$(detect K5_CELL= K2_CELL=canonical K2_SESSION_ID=s2)" "canonical|s2"
eq "K5_SESSION_ID alone: canonical" "$(detect K5_SESSION_ID=s5)" "canonical|s5"
eq "K2 names only: as before" "$(detect K2_CELL=sidecar K2_SESSION_ID=s2)" "sidecar|s2"
eq "nothing: empty" "$(detect)" "|"

echo "== 3. k2 whoami sends the session's names =="
python3 - "$WORK" <<'PY' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(u.query).items() if k not in ("token", "project")}
        with open(os.path.join(work, "q.log"), "a") as f:
            f.write(u.path + " " + json.dumps(q, sort_keys=True) + "\n")
        data = b'{"ok":true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a): pass
srv = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(work, "port"), "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/port" ] && break; sleep 0.05; done
[ -s "$WORK/port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
PORT="$(cat "$WORK/port")"
# whoami_q <env…>: the query /cli/whoami got (token dropped).
whoami_q() {
    : >"$WORK/q.log"
    env -i PATH="$PATH" HOME="$WORK/home" K2_PORT="$PORT" K2_HOOK_TOKEN=sessA.passport \
        K2_PROJECT_PATH="$WORK/proj" "$@" "$K2_CLI" whoami >/dev/null 2>&1 || true
    grep '^/cli/whoami ' "$WORK/q.log" | tail -1 | sed 's|^/cli/whoami ||'
}
eq "K5_ names reach the query" \
    "$(whoami_q K5_SESSION_ID=s5 K5_CELL=sidecar K5_SIDECAR_NAME=rev K5_PRIMARY=acme)" \
    '{"cell": "sidecar", "primary": "acme", "session": "s5", "sidecar_name": "rev"}'
eq "K5_ wins over K2_, an empty K5_ does not" \
    "$(whoami_q K5_SESSION_ID=s5 K2_SESSION_ID=s2 K5_CELL= K2_CELL=canonical)" \
    '{"cell": "canonical", "session": "s5"}'
eq "K2_ names: as before" \
    "$(whoami_q K2_SESSION_ID=s2 K2_CELL=sidecar K2_SIDECAR_NAME=rev K2_PRIMARY=acme)" \
    '{"cell": "sidecar", "primary": "acme", "session": "s2", "sidecar_name": "rev"}'
eq "none: no session fields" "$(whoami_q)" '{}'

echo
echo "k5_env_reads: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
