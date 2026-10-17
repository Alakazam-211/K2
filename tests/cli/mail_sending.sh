#!/usr/bin/env bash
# Loud harness for `k2 mail sending <ws> [off|approval|on]` (Appa/Baden,
# macOS 0.45.4: the owner had no working way to turn agent-send on for a
# linked inbox). The daemon decides; these check the CLI:
# - show = GET /cli/mail/config?workspace=<ws>, prints the effective mode;
# - set  = POST /cli/mail/config/set {workspace, agentSend} (the same
#   route + owner gate as `k2 hostmail config --agent-send`), then shows;
# - a bad mode / missing workspace is usage (exit 2) and sends nothing;
# - an agent refusal (owner_only) exits 3 with the daemon's hint;
# - `k2 hostmail config --workspace <ws> --agent-send <m>` still posts the
#   same body.
# cargo does not cover cmd_mail_* / _mail_py. Fail-loud: no skips.
# Run with: bash tests/cli/mail_sending.sh  (fake daemon, no live one).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -f "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found" >&2; exit 1; }
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
bash -n "$K2_CLI" || { echo "FAIL: bash -n cli/k2" >&2; exit 1; }

# Never inherit a live session's socket or identity.
for v in $(compgen -e | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

assert_exit() {
    local label="$1" expected="$2" got="$3"
    if [ "$got" = "$expected" ]; then ok "$label (exit $got)"; else bad "$label expected exit $expected got $got"; fi
}

assert_json() {
    local label="$1" hay="$2" expr="$3"
    if printf '%s' "$hay" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($expr) else 1)" 2>/dev/null; then
        ok "$label"
    else
        bad "$label — '$expr' false for: $hay"
    fi
}

assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -qF -- "$needle"; then ok "$label"; else bad "$label — '$needle' missing in: $hay"; fi
}

WORKDIR="$(mktemp -d -t k2-mail-sending-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

# Fake daemon. Keeps the stored mode in STATE; records the last POST body
# at post.json and the last GET query at get.txt. CALLER=agent makes
# config/set answer like the workspace's own agent (owner_only).
python3 - "$WORKDIR" <<'PY' &
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

root = sys.argv[1]

def state():
    p = os.path.join(root, "STATE")
    return open(p).read().strip() if os.path.exists(p) else ""

class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def _send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n)
        path = urlsplit(self.path).path
        with open(os.path.join(root, "post.json"), "wb") as f:
            f.write(raw)
        if path != "/cli/mail/config/set":
            return self._send(404, {"ok": False, "error": {"code": "not_found", "hint": path}})
        caller = open(os.path.join(root, "CALLER")).read().strip()
        if caller == "agent":
            return self._send(403, {"ok": False, "error": {"code": "owner_only", "hint":
                "An agent can't loosen mail rules on its own sends: changing agentSend or "
                "always-BCC for this agent's own workspace needs the owner. Ask your human."}})
        b = json.loads(raw or b"{}")
        with open(os.path.join(root, "STATE"), "w") as f:
            f.write(b.get("agentSend", ""))
        self._send(200, {"ok": True, "applied": {"workspace": {"agentSend": b.get("agentSend")}}})
    def do_GET(self):
        u = urlsplit(self.path)
        q = parse_qs(u.query)
        with open(os.path.join(root, "get.txt"), "w") as f:
            f.write(u.path + "|" + ",".join(q.get("workspace", [])))
        if u.path != "/cli/mail/config":
            return self._send(404, {"ok": False, "error": {"code": "not_found", "hint": u.path}})
        ws = q.get("workspace", [""])[0]
        if ws == "nope":
            return self._send(404, {"ok": False, "error": {"code": "not_found",
                "hint": "no agent/workspace named 'nope'."}})
        st = state()
        self._send(200, {"ok": True, "supported": False, "agentSend": {"default": "off"},
                         "workspace": {"project": "/Users/baden/appa", "agentSend": {
                             "effective": st or "off", "override": st or None,
                             "default": "off", "modes": ["off", "approval", "on"]}}})

httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
FAKE_PID=$!
disown "$FAKE_PID" 2>/dev/null || true
trap 'kill $FAKE_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

for _ in $(seq 1 50); do
    [ -f "$WORKDIR/port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/port" ] || { echo "FAIL: fake daemon never bound" >&2; exit 1; }
PORT="$(cat "$WORKDIR/port")"
echo owner > "$WORKDIR/CALLER"

K2=(env K2_PORT="$PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR")

echo "== help =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail sending --help 2>&1)"
rc=$?
set -e
assert_exit "mail sending --help" 0 "$rc"
assert_contains "help names macOS" "$out" "every OS (macOS included)"
assert_contains "help names the Settings control" "$out" "Workspaces / Agents → <workspace> → Email sending"
set +e
out="$("${K2[@]}" "$K2_CLI" mail --help 2>&1)"
set -e
assert_contains "k2 mail --help lists sending" "$out" "sending <workspace> [off|approval|on]"

echo "== show (unset → server default) =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail sending appa 2>&1)"
rc=$?
set -e
assert_exit "mail sending appa" 0 "$rc"
assert_contains "show prints the mode" "$out" "agent email sending for appa: off (server default)"
assert_contains "off teaches the turn-on command" "$out" "k2 mail sending appa on"
[ "$(cat "$WORKDIR/get.txt")" = "/cli/mail/config|appa" ] && ok "GET carries ?workspace=appa" || bad "GET was: $(cat "$WORKDIR/get.txt")"
[ ! -f "$WORKDIR/post.json" ] && ok "show never POSTs" || bad "show POSTed: $(cat "$WORKDIR/post.json")"

echo "== set on =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail sending appa on 2>&1)"
rc=$?
set -e
assert_exit "mail sending appa on" 0 "$rc"
assert_json "POST body is {workspace, agentSend}" "$(cat "$WORKDIR/post.json")" "d == {'workspace': 'appa', 'agentSend': 'on'}"
assert_contains "set prints the new mode" "$out" "agent email sending for appa is now on (set for this workspace)"

echo "== set approval (linked note) + --json =="
set +e
out="$("${K2[@]}" "$K2_CLI" mail sending appa approval 2>&1)"
rc=$?
set -e
assert_exit "mail sending appa approval" 0 "$rc"
assert_contains "approval warns linked needs on" "$out" "a linked inbox (Gmail, Outlook, IMAP) needs 'on' to send"
set +e
out="$("${K2[@]}" "$K2_CLI" mail sending appa off --json 2>/dev/null)"
rc=$?
set -e
assert_exit "mail sending appa off --json" 0 "$rc"
assert_json "--json shape" "$out" "d['ok'] is True and d['agentSend'] == 'off' and d['override'] == 'off' and d['changed'] is True and d['workspace'] == 'appa'"

echo "== usage errors send nothing =="
rm -f "$WORKDIR/post.json"
set +e
err="$("${K2[@]}" "$K2_CLI" mail sending appa always 2>&1 >/dev/null)"
rc=$?
set -e
assert_exit "bad mode" 2 "$rc"
assert_json "bad mode code usage" "$err" "d['error']['code'] == 'usage' and 'off, approval, on' in d['error']['hint']"
set +e
"${K2[@]}" "$K2_CLI" mail sending >/dev/null 2>&1
rc=$?
set -e
assert_exit "missing workspace" 2 "$rc"
set +e
"${K2[@]}" "$K2_CLI" mail sending appa on extra >/dev/null 2>&1
rc=$?
set -e
assert_exit "extra argument" 2 "$rc"
[ ! -f "$WORKDIR/post.json" ] && ok "no POST for usage errors" || bad "usage error POSTed: $(cat "$WORKDIR/post.json")"

echo "== unknown workspace =="
set +e
err="$("${K2[@]}" "$K2_CLI" mail sending nope 2>&1 >/dev/null)"
rc=$?
set -e
assert_exit "unknown workspace" 1 "$rc"
assert_json "not_found on stderr" "$err" "d['error']['code'] == 'not_found'"

echo "== the workspace's own agent is refused (exit 3) =="
echo agent > "$WORKDIR/CALLER"
set +e
err="$("${K2[@]}" "$K2_CLI" mail sending appa on 2>&1 >/dev/null)"
rc=$?
set -e
assert_exit "agent sets its own sending" 3 "$rc"
assert_json "owner_only on stderr" "$err" "d['error']['code'] == 'owner_only'"
assert_json "hint names self-elevation" "$err" "\"can't loosen mail rules on its own sends\" in d['error']['hint']"
echo owner > "$WORKDIR/CALLER"

echo "== hostmail config --agent-send still posts the same body =="
rm -f "$WORKDIR/post.json"
set +e
"${K2[@]}" "$K2_CLI" hostmail config --workspace appa --agent-send on >/dev/null 2>&1
rc=$?
set -e
assert_exit "hostmail config --agent-send" 0 "$rc"
assert_json "hostmail config body" "$(cat "$WORKDIR/post.json")" "d == {'workspace': 'appa', 'agentSend': 'on'}"

echo
echo "mail_sending.sh: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
