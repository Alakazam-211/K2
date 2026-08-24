#!/usr/bin/env bash
# Loud harness for `k2 mail draft` compose + xor (prd-mail-draft-compose-v1 §5).
# cargo does not cover cmd_mail_draft / _mail_py. Fail-loud: no skip-if-missing.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -f "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found" >&2; exit 1; }
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }

pass=0
fail=0

assert_exit() {
    local label="$1" expected="$2" got="$3"
    if [ "$got" = "$expected" ]; then
        echo "  PASS: $label (exit $got)"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label expected exit $expected got $got" >&2
        fail=$((fail + 1))
    fi
}

assert_json_has() {
    local label="$1" hay="$2" key="$3"
    if printf '%s' "$hay" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if '$key' in d else 1)" 2>/dev/null; then
        echo "  PASS: $label has $key"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label missing $key in: $hay" >&2
        fail=$((fail + 1))
    fi
}

assert_json_absent() {
    local label="$1" hay="$2" key="$3"
    if printf '%s' "$hay" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if '$key' not in d else 1)" 2>/dev/null; then
        echo "  PASS: $label no $key"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label unexpected $key in: $hay" >&2
        fail=$((fail + 1))
    fi
}

assert_json_eq() {
    local label="$1" hay="$2" key="$3" want="$4"
    local got
    got="$(printf '%s' "$hay" | python3 -c "import json,sys; d=json.load(sys.stdin); v=d.get('$key'); print(v if not isinstance(v,list) else ','.join(v))" 2>/dev/null || true)"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label $key=$want"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label $key expected '$want' got '$got' in: $hay" >&2
        fail=$((fail + 1))
    fi
}

WORKDIR="$(mktemp -d -t k2-mail-draft-XXXXXX)"
POSTFILE="$WORKDIR/post.json"
trap 'rm -rf "$WORKDIR"' EXIT

# Tiny fake daemon: record POST /cli/mail/draft JSON, return ok.
python3 - "$WORKDIR" <<'PY' &
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer

root = sys.argv[1]
post_path = os.path.join(root, "post.json")

class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n)
        with open(post_path, "wb") as f:
            f.write(raw)
        body = json.dumps({
            "ok": True,
            "folder": "Drafts",
            "address": "me@example.com",
            "hint": "draft saved to 'Drafts' in me@example.com",
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"ok":true}')

httpd = HTTPServer(("127.0.0.1", 0), H)
port = httpd.server_address[1]
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(port))
httpd.serve_forever()
PY
FAKE_PID=$!
trap 'kill $FAKE_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

for _ in $(seq 1 50); do
    [ -f "$WORKDIR/port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/port" ] || { echo "FAIL: fake daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"

K2=(env K2_PORT="$PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR")

echo "== CLI compose POST (no id) =="
rm -f "$POSTFILE"
set +e
out="$("${K2[@]}" "$K2_CLI" mail draft --to a@b.example --subject s --body t --json 2>&1)"
rc=$?
set -e
assert_exit "compose --to/--subject/--body" 0 "$rc"
[ -f "$POSTFILE" ] || { echo "FAIL: no POST recorded" >&2; fail=$((fail + 1)); POSTFILE=/dev/null; }
post="$(cat "$POSTFILE" 2>/dev/null || echo '{}')"
assert_json_has "compose POST" "$post" "to"
assert_json_has "compose POST" "$post" "subject"
assert_json_has "compose POST" "$post" "body"
assert_json_absent "compose POST" "$post" "id"
assert_json_eq "compose POST to" "$post" "to" "a@b.example"
assert_json_eq "compose POST subject" "$post" "subject" "s"
assert_json_eq "compose POST body" "$post" "body" "t"

echo "== CLI reply POST (id + body) =="
rm -f "$POSTFILE"
set +e
out="$("${K2[@]}" "$K2_CLI" mail draft m_abc123 --body t --json 2>&1)"
rc=$?
set -e
assert_exit "reply <id> --body" 0 "$rc"
post="$(cat "$POSTFILE" 2>/dev/null || echo '{}')"
assert_json_has "reply POST" "$post" "id"
assert_json_has "reply POST" "$post" "body"
assert_json_absent "reply POST" "$post" "to"
assert_json_eq "reply POST id" "$post" "id" "m_abc123"

echo "== CLI xor id AND --to =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail draft m_abc --to a@b.example --subject s --body t 2>&1)"
rc=$?
set -e
assert_exit "id and --to" 2 "$rc"
printf '%s' "$out" | grep -q "not both" && { echo "  PASS: xor hint"; pass=$((pass + 1)); } || {
    echo "  FAIL: xor hint missing in: $out" >&2
    fail=$((fail + 1))
}

echo "== CLI compose missing --subject =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail draft --to a@b.example --body t 2>&1)"
rc=$?
set -e
assert_exit "compose missing --subject" 2 "$rc"
printf '%s' "$out" | grep -q "subject" && { echo "  PASS: missing subject hint"; pass=$((pass + 1)); } || {
    echo "  FAIL: missing subject hint in: $out" >&2
    fail=$((fail + 1))
}

echo "== CLI compose missing --body =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail draft --to a@b.example --subject s 2>&1)"
rc=$?
set -e
assert_exit "compose missing --body" 2 "$rc"
printf '%s' "$out" | grep -q "body" && { echo "  PASS: missing body hint"; pass=$((pass + 1)); } || {
    echo "  FAIL: missing body hint in: $out" >&2
    fail=$((fail + 1))
}

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
