#!/usr/bin/env bash
# k2 compute prune | pin | unpin | retention (0.46.17): CLI shapes against
# a tiny stub daemon on loopback (python3 http.server). No real daemon, no
# vault, no keychain, no ~/.k2: HOME is a throwaway dir.
#
# Covers: prune posts /cli/compute/prune with {olderThan, dryRun} (dryRun a
# JSON bool, olderThan left out when not given) and prints the daemon's
# `text` lines; --json prints the whole answer; pin/unpin post {job};
# retention GETs, retention set posts {maxAge, cap}; a bad flag, a missing
# job or a bare `retention set` is a usage error (exit 2) with no request;
# the daemon's owner_only refusal exits 3; every call goes over TCP with the
# owner token as Bearer; status prints the store line; the usage text, the
# schema and `k2 study compute` name the verbs.

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
# A here-string, not a pipe (pipefail + an early grep match on a long text).
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2prune-cli-XXXXXX)"
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
    def note(self, method, path, body):
        auth = self.headers.get("Authorization") or ""
        token = auth[len("Bearer "):] if auth.startswith("Bearer ") else ""
        query_token = "token" in parse_qs(urlparse(self.path).query)
        with open(LOG, "a") as f:
            f.write(json.dumps({"method": method, "path": path, "body": body, "token": token, "query_token": query_token}) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        self.note("GET", u.path, {})
        if u.path == "/cli/compute/retention":
            return self.reply(200, {"bytes": 1, "policy": {"maxAgeSecs": 604800}, "text": ["compute store: 37.0 GB (artifacts …) · keep 7 d after a job ends · cap 20.0 GB (auto)"]})
        if u.path == "/cli/compute/nodes":
            return self.reply(200, {"nodes": [], "controllerMachine": {"name": "ctl", "diskFreeBytes": 1 << 35, "diskTotalBytes": 1 << 39,
                                    "store": {"line": "compute store: 2.0 GB (artifacts 1.5 GB in 3 folders, blobs 512 MB in 2 files) · keep 7 d after a job ends · cap 20.0 GB"}}})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        self.note("POST", u.path, body)
        if u.path == "/cli/compute/prune":
            if body.get("olderThan") == "locked":
                return self.reply(403, {"error": {"code": "owner_only", "hint": "only the server's owner (or an admin, where noted) can do this"}})
            verb = "would remove" if body.get("dryRun") else "removed"
            return self.reply(200, {"dryRun": bool(body.get("dryRun")), "removed": {"units": 2, "bytes": 3}, "text": ["%s 2 job(s)' files: 3 B" % verb, "kept: 1 pinned"]})
        if u.path in ("/cli/compute/pin", "/cli/compute/unpin"):
            if body.get("job") == "lockedjob":
                return self.reply(403, {"error": {"code": "owner_only", "hint": "only the server's owner (or an admin, where noted) can do this"}})
            return self.reply(200, {"job": body.get("job"), "pinned": u.path.endswith("/pin"), "text": ["%s %s" % (u.path.rsplit("/", 1)[1] + "ned", body.get("job"))]})
        if u.path == "/cli/compute/retention/set":
            return self.reply(200, {"policy": {}, "text": ["compute store retention: keep %s" % body.get("maxAge", "7d")]})
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

last() { tail -1 "$LOG" | python3 -c "import json,sys; v=json.load(sys.stdin); print(json.dumps(v[sys.argv[1]], sort_keys=True) if not isinstance(v[sys.argv[1]], str) else v[sys.argv[1]])" "$1"; }

echo "== prune =="
out="$("$K2" compute prune --dry-run </dev/null)"
expect_has "dry run prints the daemon's lines" "$out" "would remove 2 job(s)' files: 3 B"
expect_has "and the kept line" "$out" "kept: 1 pinned"
expect_eq "prune path" "$(last path)" "/cli/compute/prune"
expect_eq "dry run body: a JSON true, no olderThan" "$(last body)" '{"dryRun": true}'
expect_eq "sent with the owner token as Bearer" "$(last token)" "owner-test-token"
expect_eq "no token in the query" "$(last query_token)" "false"
out="$("$K2" compute prune --older-than 3d </dev/null)"
expect_has "real prune" "$out" "removed 2 job(s)' files"
expect_eq "real prune body" "$(last body)" '{"dryRun": false, "olderThan": "3d"}'
out="$("$K2" compute prune --json </dev/null)"
expect_eq "--json prints the whole answer" "$(python3 -c 'import json,sys; print(json.load(sys.stdin)["removed"]["units"])' <<<"$out")" "2"

echo "== pin, unpin, retention =="
out="$("$K2" compute pin abcd1234 </dev/null)"
expect_has "pin renders" "$out" "pinned abcd1234"
expect_eq "pin path" "$(last path)" "/cli/compute/pin"
expect_eq "pin body" "$(last body)" '{"job": "abcd1234"}'
out="$("$K2" compute unpin abcd1234 </dev/null)"
expect_eq "unpin path" "$(last path)" "/cli/compute/unpin"
out="$("$K2" compute retention </dev/null)"
expect_has "retention renders" "$out" "compute store: 37.0 GB"
expect_eq "retention is a GET" "$(last method)" "GET"
out="$("$K2" compute retention set --max-age 14d --cap 30G </dev/null)"
expect_has "retention set renders" "$out" "keep 14d"
expect_eq "retention set path" "$(last path)" "/cli/compute/retention/set"
expect_eq "retention set body" "$(last body)" '{"cap": "30G", "maxAge": "14d"}'
out="$("$K2" compute status </dev/null)"
expect_has "status prints the store line" "$out" "compute store: 2.0 GB (artifacts 1.5 GB in 3 folders"

echo "== usage and refusals =="
n_before="$(wc -l < "$LOG")"
for args in "prune --older-than" "prune --older-than soon" "prune --bogus" "pin" "pin a b" "unpin --bogus x" "retention set" "retention --max-age 3d" "retention set --what 1"; do
    set +e
    # shellcheck disable=SC2086 # the words are separate arguments on purpose
    err="$("$K2" compute $args 2>&1 </dev/null >/dev/null)"; rc=$?
    set -e
    expect_eq "'$args' exits 2" "$rc" "2"
    if [ -n "$err" ]; then ok "'$args' says what's wrong"; else bad "'$args' printed nothing"; fi
done
expect_eq "no request for a usage error" "$(wc -l < "$LOG")" "$n_before"
set +e
err="$("$K2" compute prune --older-than locked 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "a word the CLI can't read is still a usage error" "$rc" "2"
set +e
err="$("$K2" compute pin lockedjob 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "the daemon's refusal exits 3" "$rc" "3"
expect_has "refusal code" "$err" "owner_only"
out="$("$K2" compute prune --older-than 0 </dev/null)"
expect_eq "0 goes to the daemon" "$(last body)" '{"dryRun": false, "olderThan": "0"}'

echo "== docs =="
usage="$("$K2" compute --help 2>&1 || true)"
expect_has "usage lists prune" "$usage" "k2 compute prune [--older-than DURATION] [--dry-run] [--json]"
expect_has "usage lists pin" "$usage" "k2 compute pin <job> [--json]"
expect_has "usage lists retention set" "$usage" "k2 compute retention set [--max-age DURATION|default] [--cap SIZE|auto]"
study="$("$K2" study compute)"
expect_has "study has the store section" "$study" "THE STORE ON THIS SERVER (fetched files, blobs)"
expect_has "study names the codes" "$study" "node_reconnecting"
schema="$("$K2" --schema)"
for v in "compute prune" "compute pin" "compute unpin" "compute retention" "compute retention set"; do
    expect_has "schema has $v" "$schema" "\"name\": \"$v\""
done

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
