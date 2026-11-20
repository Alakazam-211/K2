#!/usr/bin/env bash
# The CLI under macOS's /bin/bash 3.2 with an empty array under `set -u`.
#
# bash before 4.4 treats "${arr[@]}" of an empty array as an unbound
# variable under `set -u`, so these lines failed on a Mac (and passed on
# Linux's bash 5):
# - app email send with no --cc, --bcc, --reply-to, --header or --tag
#   ("ccs[@]: unbound variable", exit 2, nothing sent);
# - app push send to a room, or with no --data ("tos[@]", "datas[@]");
# - tickets assign --clear (users[@]: the line on stderr) and
#   tickets assign <id> --to "" (_parts[@]);
# - notifications email sender add, check, set and rm with no --reply-to
#   (rts[@]);
# - agent context with only --workspace (ctx_args[@]);
# - commit --merge with nothing else (rest[@]);
# - sessions set-label <id> with no label (label_parts[*]).
# The fix is the file's own idiom, ${arr[@]+"${arr[@]}"}.
#
# On macOS this runs cli/k2 with /bin/bash and checks it is 3.2. Elsewhere
# it runs the bash on PATH (a newer bash, where the lines always worked):
# the same checks, so the test proves the fix only on macOS, and says so.
# A stub daemon answers; no live K2. Fail loud; no skips.
# Run with: bash tests/cli/bash32_empty_arrays.sh
# bash-only: runs cli/k2 as source under a chosen bash; the parity gate skips it for the binary.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_BASH_SOURCE"

[ -f "$K2" ] || { echo "FAIL: $K2 not found" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

# Never inherit a live session's socket or identity.
for v in $(compgen -e | grep -E '^(K2|K5|K2SO)_' || true); do unset "$v"; done

if [ "$(uname -s)" = Darwin ]; then
    SH=/bin/bash
    ver="$("$SH" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
    [ "$ver" = 3.2 ] || { echo "FAIL: macOS's /bin/bash is $ver, want 3.2" >&2; exit 1; }
else
    SH=bash
    ver="$("$SH" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
fi
echo "== cli/k2 under $SH ($ver)"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORKDIR="$(mktemp -d -t k2-bash32-arrays-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT
mkdir -p "$WORKDIR/home" "$WORKDIR/ws"

# Stub daemon: logs each request as one JSON line in req.log and answers
# the routes these lines post to.
python3 - "$WORKDIR" <<'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

root = sys.argv[1]
SENDER = {"sender": {"id": "s1", "address": "news@acme.test", "path": "own", "state": "ready",
                     "owner": "ada", "fromName": "Acme"}}
ANSWERS = {
    "/cli/app/email/send": {"id": "em1", "status": "queued", "recipients": 1},
    "/cli/app/push/send": {"queued": 2, "sendId": "ps1"},
    "/cli/feedback/assign": {"id": "fb_12345678", "assignees": []},
    "/cli/notifications/email/sender/add": SENDER,
    "/cli/notifications/email/sender/set": SENDER,
    "/cli/commit-merge": {"ok": True, "message": "merged"},
}

class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def _answer(self, method):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode("utf-8", "replace")
        u = urlsplit(self.path)
        with open(os.path.join(root, "req.log"), "a") as f:
            f.write(json.dumps({"method": method, "path": u.path, "body": raw}) + "\n")
        if method == "POST" and u.path not in ANSWERS:
            obj, code = {"error": {"code": "not_found", "hint": u.path}}, 404
        else:
            obj, code = ANSWERS.get(u.path, {}), 200
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self._answer("GET")
    def do_POST(self):
        self._answer("POST")

httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
STUB_PID=$!
trap 'kill $STUB_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT
for _ in $(seq 1 100); do
    [ -s "$WORKDIR/port" ] && break
    sleep 0.05
done
[ -s "$WORKDIR/port" ] || { echo "FAIL: the stub daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"

# run <args...>: runs cli/k2 under $SH; sets rc, out and err.
run() {
    : >"$WORKDIR/req.log"
    rc=0
    (cd "$WORKDIR/ws" && env K2_PORT="$PORT" K2_HOOK_TOKEN=fake HOME="$WORKDIR/home" \
        "$SH" "$K2" "$@" >"$WORKDIR/out" 2>"$WORKDIR/err" </dev/null) || rc=$?
    out="$(cat "$WORKDIR/out")"
    err="$(cat "$WORKDIR/err")"
}

# check <label> <want rc> <stdout needle or ""> <stderr needle or "">
check() {
    local label="$1" want="$2" o="$3" e="$4"
    if <<<"$err" grep -q 'unbound variable'; then
        bad "$label: bash says unbound variable: $err"
        return
    fi
    [ "$rc" = "$want" ] && ok "$label: exit $rc" || bad "$label: exit $rc, want $want (stderr: $err)"
    if [ -n "$o" ]; then
        <<<"$out" grep -qF -- "$o" && ok "$label: prints '$o'" || bad "$label: stdout '$out' has no '$o'"
    fi
    if [ -n "$e" ]; then
        <<<"$err" grep -qF -- "$e" && ok "$label: says '$e'" || bad "$label: stderr '$err' has no '$e'"
    fi
}

# posted <label> <path> <python test on the JSON body b>: the one POST to
# <path>, and what its body holds.
posted() {
    local label="$1" path="$2" expr="$3"
    if python3 - "$WORKDIR/req.log" "$path" "$expr" <<'PY'; then ok "$label: posted $path"; else bad "$label: the POST to $path: $(cat "$WORKDIR/req.log")"; fi
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
hits = [r for r in rows if r["method"] == "POST" and r["path"] == sys.argv[2]]
if len(hits) != 1:
    sys.exit(1)
b = json.loads(hits[0]["body"])
sys.exit(0 if eval(sys.argv[3], {"b": b}) else 1)
PY
}

# nothing_sent <label>: no request at all.
nothing_sent() {
    [ ! -s "$WORKDIR/req.log" ] && ok "$1: sends nothing" || bad "$1: sent $(cat "$WORKDIR/req.log")"
}

echo "== app email send, no --cc, --bcc, --reply-to, --header or --tag"
run app email send acme --to ada@acme.test --subject Hello --text Hi
check "email send" 0 "email em1: queued (1 recipient)" ""
posted "email send" /cli/app/email/send \
    'b == {"app": "acme", "subject": "Hello", "to": ["ada@acme.test"], "text": "Hi"}'

echo "== app email send with every list"
run app email send acme --to a@acme.test --to b@acme.test --cc c@acme.test --bcc d@acme.test \
    --reply-to r@acme.test --header X-A=1 --tag t1 --subject S --text T
check "email send, all lists" 0 "email em1: queued" ""
posted "email send, all lists" /cli/app/email/send \
    'b["to"] == ["a@acme.test", "b@acme.test"] and b["cc"] == ["c@acme.test"] and b["bcc"] == ["d@acme.test"] and b["replyTo"] == ["r@acme.test"] and b["headers"] == {"X-A": "1"} and b["tags"] == ["t1"]'

echo "== app push send to a room, no --data"
run app push send acme --room lobby --title T --body B
check "push send --room" 0 "queued 2 device send(s), send ps1" ""
posted "push send --room" /cli/app/push/send \
    'b == {"app": "acme", "to": {"room": "lobby"}, "title": "T", "body": "B"}'

echo "== app push send to a guest with --data"
run app push send acme --to g1 --title T --body B --data k=v
check "push send --to --data" 0 "queued 2 device send(s)" ""
posted "push send --to --data" /cli/app/push/send \
    'b["to"] == {"guest": "g1"} and b["data"] == {"k": "v"}'

echo "== tickets assign --clear"
run tickets assign fb_12345678 --clear
check "assign --clear" 0 "Cleared assignees on fb_12345" ""
posted "assign --clear" /cli/feedback/assign 'b == {"id": "fb_12345678", "usernames": []}'

echo "== tickets assign --to with an empty list"
run tickets assign fb_12345678 --to ""
check "assign --to ''" 2 "" "assign requires at least one username"
nothing_sent "assign --to ''"

echo "== notifications email sender add, no --reply-to"
run notifications email sender add news@acme.test --name Acme
check "sender add" 0 "s1  news@acme.test  own  ready  ada (Acme)" ""
posted "sender add" /cli/notifications/email/sender/add \
    'b == {"address": "news@acme.test", "name": "Acme"}'

echo "== notifications email sender set, no --reply-to"
run notifications email sender set s1 --lock-name
check "sender set" 0 "s1  news@acme.test" ""
posted "sender set" /cli/notifications/email/sender/set 'b == {"sender": "s1", "nameLocked": True}'

echo "== agent context with only --workspace"
run agent context --workspace "$WORKDIR/ws"
check "agent context --workspace" 0 "k2 agent context" ""
nothing_sent "agent context --workspace"

echo "== commit --merge alone"
run commit --merge
check "commit --merge" 0 "Requesting AI Commit & Merge..." ""
[ "$(grep -c '"path": "/cli/commit-merge"' "$WORKDIR/req.log")" = 1 ] \
    && ok "commit --merge: posted /cli/commit-merge" || bad "commit --merge: $(cat "$WORKDIR/req.log")"

echo "== sessions set-label with no label"
run sessions set-label 0f0e
check "set-label, no label" 1 "" "Usage: k2 sessions set-label <session-id> <label> [--no-lock]"
nothing_sent "set-label, no label"

echo
echo "bash32_empty_arrays: $pass passed, $fail failed (under $SH $ver)"
[ "$ver" = 3.2 ] || echo "bash32_empty_arrays: bash $ver here, not 3.2: the fix is proven on macOS's /bin/bash only"
[ "$fail" -eq 0 ]
