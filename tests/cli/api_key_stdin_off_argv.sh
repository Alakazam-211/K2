#!/usr/bin/env bash
# KV34 — LLM / workspace API keys reach the daemon without riding any argv.
#
# `k2 workspace api-key <ws> --key-stdin` and `k2 api-key create --key-stdin`
# read the key from stdin and POST it in the JSON body via
# `curl --data-binary @-`. The old forms (key as an argument) still work
# this release but print a one-line deprecation warning.
#
# A `curl` shim first on PATH logs every argv it is given, then runs the
# real curl against a python stub daemon that records each request. Every
# scenario asserts the stub got the key in the body and the curl argv log
# never contains it.
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }
[ -x "$K2" ] || fail "$K2 not found/executable"
bash -n "$K2" || fail "bash -n cli/k2"

REAL_CURL="$(command -v curl)"
[ -n "$REAL_CURL" ] || fail "curl not found"

WORK="$(mktemp -d -t k2-apikey-argv-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

ARGV_LOG="$WORK/curl-argv.log"
REQ_LOG="$WORK/requests.log"
: >"$ARGV_LOG"
: >"$REQ_LOG"
mkdir -p "$WORK/shim" "$WORK/proj/.k2"

cat >"$WORK/shim/curl" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> '$ARGV_LOG'
exec '$REAL_CURL' "\$@"
EOF
chmod +x "$WORK/shim/curl"
export PATH="$WORK/shim:$PATH"
[ "$(command -v curl)" = "$WORK/shim/curl" ] || fail "curl shim not first on PATH"

# Stub daemon: answers resolve / api-key routes, records path + body.
python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _reply(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def _record(self, body):
        u = urllib.parse.urlparse(self.path)
        with open(work + "/requests.log", "a") as f:
            f.write(json.dumps({"method": self.command, "path": u.path,
                                "query": u.query, "body": body}) + "\n")
        return u.path
    def do_GET(self):
        path = self._record("")
        if path == "/cli/workspace/resolve":
            return self._reply({"path": "/stub/ws/path"})
        self._reply({"error": "unexpected GET " + path})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        path = self._record(body)
        if path == "/cli/workspace/api-key":
            key = json.loads(body).get("key", "")
            return self._reply({"keySet": bool(key)})
        if path == "/cli/api-keys/create":
            return self._reply({"id": "k1", "key": "k2sk_minted", "capabilities": {}})
        self._reply({"error": "unexpected POST " + path})
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || fail "stub daemon never wrote its port"
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

last_post_body() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
posts = [r for r in rows if r["method"] == "POST" and r["path"] == sys.argv[2]]
if not posts:
    sys.exit("no POST to " + sys.argv[2])
print(posts[-1]["body"])
' "$REQ_LOG" "$1"
}

json_field() {
    python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get(sys.argv[2], ""))' "$1" "$2"
}

assert_not_in_argv() {
    if grep -qF -- "$1" "$ARGV_LOG"; then
        fail "secret '$1' appeared on curl argv: $(cat "$ARGV_LOG")"
    fi
}

# 1. workspace api-key --key-stdin (piped, trailing newline).
err="$WORK/err1"
out="$(printf 'sk-ws-stdin-1111\n' | "$K2" workspace api-key myws --key-stdin 2>"$err")" \
    || fail "workspace api-key --key-stdin failed: $out $(cat "$err")"
[ "$out" = "Workspace API key set." ] || fail "unexpected output: $out"
body="$(last_post_body /cli/workspace/api-key)"
[ "$(json_field "$body" key)" = "sk-ws-stdin-1111" ] || fail "key not in body: $body"
[ "$(json_field "$body" project)" = "/stub/ws/path" ] || fail "project not resolved: $body"
grep -q "deprecated" "$err" && fail "stdin form must not warn: $(cat "$err")"
assert_not_in_argv "sk-ws-stdin-1111"

# 2. workspace api-key <ws> <key> — still works, warns once.
err="$WORK/err2"
out="$("$K2" workspace api-key myws sk-ws-argv-2222 2>"$err")" \
    || fail "deprecated argv form failed: $out $(cat "$err")"
body="$(last_post_body /cli/workspace/api-key)"
[ "$(json_field "$body" key)" = "sk-ws-argv-2222" ] || fail "key not in body: $body"
[ "$(grep -c 'deprecated' "$err")" = "1" ] || fail "argv form must warn exactly once: $(cat "$err")"
grep -q -- "--key-stdin" "$err" || fail "warning must point at --key-stdin: $(cat "$err")"
assert_not_in_argv "sk-ws-argv-2222"

# 3. workspace api-key <ws> — clears (empty key), no warning.
err="$WORK/err3"
out="$("$K2" workspace api-key myws 2>"$err" </dev/null)" || fail "clear failed: $out $(cat "$err")"
[ "$out" = "Workspace API key cleared." ] || fail "unexpected clear output: $out"
grep -q "deprecated" "$err" && fail "clear must not warn: $(cat "$err")"

# 4. --key-stdin with an empty stdin is refused (never silently clears).
set +e
printf '' | "$K2" workspace api-key myws --key-stdin >/dev/null 2>"$WORK/err4"
code=$?
set -e
[ "$code" = "2" ] || fail "empty --key-stdin must exit 2, got $code: $(cat "$WORK/err4")"

# 5. --key-stdin plus a key argument is refused.
set +e
printf 'x\n' | "$K2" workspace api-key myws sk-both-3333 --key-stdin >/dev/null 2>"$WORK/err5"
code=$?
set -e
[ "$code" = "2" ] || fail "--key-stdin plus a key must exit 2, got $code"

# 6. api-key create --key-stdin (no trailing newline).
err="$WORK/err6"
out="$(printf 'sk-llm-stdin-4444' | "$K2" api-key create --label lbl --workspaces '*' --key-stdin 2>"$err")" \
    || fail "api-key create --key-stdin failed: $out $(cat "$err")"
printf '%s' "$out" | grep -q "k2sk_minted" || fail "minted key not printed: $out"
body="$(last_post_body /cli/api-keys/create)"
[ "$(json_field "$body" anthropicKey)" = "sk-llm-stdin-4444" ] || fail "llm key not in body: $body"
[ "$(json_field "$body" label)" = "lbl" ] || fail "label not in body: $body"
grep -q "deprecated" "$err" && fail "stdin form must not warn: $(cat "$err")"
assert_not_in_argv "sk-llm-stdin-4444"

# 7. api-key create --llm-key K / --anthropic-key K — still work, warn.
for flag in --llm-key --anthropic-key; do
    err="$WORK/err7"
    out="$("$K2" api-key create --label lbl --workspaces '*' "$flag" "sk-llm-argv-5555$flag" 2>"$err")" \
        || fail "$flag form failed: $out $(cat "$err")"
    body="$(last_post_body /cli/api-keys/create)"
    [ "$(json_field "$body" anthropicKey)" = "sk-llm-argv-5555$flag" ] || fail "$flag key not in body: $body"
    [ "$(grep -c 'deprecated' "$err")" = "1" ] || fail "$flag must warn exactly once: $(cat "$err")"
    assert_not_in_argv "sk-llm-argv-5555$flag"
done

# 8. api-key create without a key: no anthropicKey field, no warning.
err="$WORK/err8"
out="$("$K2" api-key create --label nokey --workspaces '*' 2>"$err" </dev/null)" \
    || fail "keyless create failed: $out $(cat "$err")"
body="$(last_post_body /cli/api-keys/create)"
python3 -c 'import json,sys; b=json.loads(sys.argv[1]); sys.exit(0 if "anthropicKey" not in b else 1)' "$body" \
    || fail "keyless create must not send anthropicKey: $body"
grep -q "deprecated" "$err" && fail "keyless create must not warn: $(cat "$err")"

# 9. The key POSTs (body on stdin) keep the token off curl's argv too.
#    (The workspace-name resolve GET goes through the shared cli_request
#    transport, which is covered by its own test.)
posts="$(grep -F -- '--data-binary @-' "$ARGV_LOG" || true)"
[ "$(printf '%s\n' "$posts" | grep -c .)" -ge 7 ] || fail "expected the key POSTs in the argv log: $(cat "$ARGV_LOG")"
if printf '%s' "$posts" | grep -qF "owner-token-for-test"; then
    fail "token on curl argv for a key POST: $posts"
fi

echo "OK: API keys reach the daemon from stdin, never on curl argv"
