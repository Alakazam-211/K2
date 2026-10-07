#!/usr/bin/env bash
# Loud harness for blind copies in the CLI: `k2 mail send --bcc`,
# `k2 mail draft --bcc`, and the owner's `k2 hostmail config
# --workspace <ws> --always-bcc <list>` ('' clears). cargo does not cover
# cmd_mail_* / _mail_py. Fail-loud: no skip-if-missing.
# Run with: bash tests/cli/mail_bcc.sh  (no live daemon — a tiny fake).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -f "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found" >&2; exit 1; }
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }

pass=0
fail=0

ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

assert_exit() {
    local label="$1" expected="$2" got="$3"
    if [ "$got" = "$expected" ]; then ok "$label (exit $got)"; else bad "$label expected exit $expected got $got"; fi
}

# assert_json <label> <json> <python-expr over d> — expr must be truthy.
assert_json() {
    local label="$1" hay="$2" expr="$3"
    if printf '%s' "$hay" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($expr) else 1)" 2>/dev/null; then
        ok "$label"
    else
        bad "$label — '$expr' false for: $hay"
    fi
}

WORKDIR="$(mktemp -d -t k2-mail-bcc-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

# Fake daemon: record each POST body at post<path-with-slashes-as-_>.json.
python3 - "$WORKDIR" <<'PY' &
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer

root = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def _ok(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n)
        path = self.path.split("?", 1)[0]
        name = "post" + path.replace("/", "_") + ".json"
        with open(os.path.join(root, name), "wb") as f:
            f.write(raw)
        if path == "/cli/mail/send":
            self._ok({"ok": True, "id": "out_fake00000001", "status": "pending_approval",
                      "queued": True, "hint": "queued for approval"})
        elif path == "/cli/mail/draft":
            self._ok({"ok": True, "folder": "Drafts", "address": "me@example.com",
                      "hint": "draft saved"})
        else:
            self._ok({"ok": True, "applied": {}})
    def do_GET(self):
        self._ok({"ok": True})

httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
FAKE_PID=$!
disown "$FAKE_PID" 2>/dev/null || true   # no "Terminated" noise at exit
trap 'kill $FAKE_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

for _ in $(seq 1 50); do
    [ -f "$WORKDIR/port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/port" ] || { echo "FAIL: fake daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"

K2=(env K2_PORT="$PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR")
SEND_POST="$WORKDIR/post_cli_mail_send.json"
DRAFT_POST="$WORKDIR/post_cli_mail_draft.json"
CFG_POST="$WORKDIR/post_cli_mail_config_set.json"

echo "== mail send --bcc (repeatable + comma-separated) =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail send cust@dest.example --subject Quote --body hi \
    --cc cc@dest.example --bcc boss@shop.example --bcc 'qa@shop.example, audit@shop.example' --json 2>&1)"
rc=$?
set -e
assert_exit "send --bcc" 0 "$rc"
post="$(cat "$SEND_POST" 2>/dev/null || echo '{}')"
assert_json "send POST bcc list" "$post" "d.get('bcc') == ['boss@shop.example','qa@shop.example','audit@shop.example']"
assert_json "send POST cc kept apart" "$post" "d.get('cc') == ['cc@dest.example']"
assert_json "send POST to" "$post" "d.get('to') == ['cust@dest.example']"

echo "== mail send without --bcc sends no bcc key =="
rm -f "$SEND_POST"
set +e
"${K2[@]}" "$K2_CLI" mail send cust@dest.example --subject Quote --body hi --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "send (no bcc)" 0 "$rc"
post="$(cat "$SEND_POST" 2>/dev/null || echo '{}')"
assert_json "send POST has no bcc" "$post" "'bcc' not in d and 'to' in d"

echo "== mail send --bcc needs a value =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail send cust@dest.example --subject s --body b --bcc 2>&1)"
rc=$?
set -e
assert_exit "--bcc without value" 2 "$rc"
printf '%s' "$out" | grep -q -- "--bcc requires an address" && ok "--bcc missing-value hint" || bad "--bcc hint missing in: $out"

echo "== mail draft --bcc (compose) =="
set +e
"${K2[@]}" "$K2_CLI" mail draft --to cust@dest.example --subject s --body t --bcc boss@shop.example --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "draft compose --bcc" 0 "$rc"
post="$(cat "$DRAFT_POST" 2>/dev/null || echo '{}')"
assert_json "draft POST bcc" "$post" "d.get('bcc') == ['boss@shop.example']"

echo "== mail draft <id> --bcc is refused (compose only) =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail draft m_abc --body t --bcc boss@shop.example 2>&1)"
rc=$?
set -e
assert_exit "reply draft --bcc" 2 "$rc"
printf '%s' "$out" | grep -q "compose drafts" && ok "reply-draft bcc hint" || bad "reply-draft hint missing in: $out"

echo "== hostmail config --always-bcc sets the policy =="
set +e
"${K2[@]}" "$K2_CLI" hostmail config --workspace shop-cs --always-bcc 'owner@shop.example,audit@shop.example' --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "config --always-bcc" 0 "$rc"
post="$(cat "$CFG_POST" 2>/dev/null || echo '{}')"
assert_json "config POST alwaysBcc" "$post" "d.get('alwaysBcc') == ['owner@shop.example','audit@shop.example']"
assert_json "config POST workspace" "$post" "d.get('workspace') == 'shop-cs'"

echo "== hostmail config --always-bcc '' clears it =="
rm -f "$CFG_POST"
set +e
"${K2[@]}" "$K2_CLI" hostmail config --workspace shop-cs --always-bcc '' --json >/dev/null 2>&1
rc=$?
set -e
assert_exit "config --always-bcc ''" 0 "$rc"
post="$(cat "$CFG_POST" 2>/dev/null || echo '{}')"
assert_json "config POST clears with []" "$post" "d.get('alwaysBcc') == [] and d.get('workspace') == 'shop-cs'"

echo "== hostmail config --always-bcc needs --workspace =="
rm -f "$CFG_POST"
set +e
out="$("${K2[@]}" "$K2_CLI" hostmail config --always-bcc owner@shop.example 2>&1)"
rc=$?
set -e
assert_exit "--always-bcc without --workspace" 2 "$rc"
printf '%s' "$out" | grep -q -- "--workspace" && ok "needs-workspace hint" || bad "needs-workspace hint missing in: $out"
[ ! -f "$CFG_POST" ] && ok "nothing POSTed" || bad "a POST went out without --workspace"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
