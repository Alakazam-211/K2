#!/usr/bin/env bash
# `k2 hostmail status` must PRINT a status/systemd disagreement, not die on it.
#
# GET /cli/mail/status keeps the envelope `ok: true` and carries the verdict
# in `consistent`. fb449bc1 briefly put the verdict in `ok`, and the generic
# `call()` treated ok=false as a malformed response ("unexpected daemon
# response") — exactly when there was something to report. Stub daemon only;
# no live K2. Run with: bash tests/cli/hostmail_status_consistent.sh
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

assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  PASS: $label contains '$needle'"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label missing '$needle' in: $hay" >&2
        fail=$((fail + 1))
    fi
}

assert_absent() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  FAIL: $label unexpected '$needle' in: $hay" >&2
        fail=$((fail + 1))
    else
        echo "  PASS: $label no '$needle'"
        pass=$((pass + 1))
    fi
}

WORKDIR="$(mktemp -d -t k2-hostmail-status-XXXXXX)"
STUB_PID=""
trap '[ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null; rm -rf "$WORKDIR"' EXIT

# Stub daemon: /cli/mail/status answers with $WORKDIR/status.json (re-read per
# request so each case just rewrites the file); every request path is logged.
python3 - "$WORKDIR" <<'PY' &
import os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

root = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def _send(self, code, body):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        with open(os.path.join(root, "requests.log"), "a") as f:
            f.write(self.path + "\n")
        if self.path.split("?")[0] == "/cli/mail/status":
            with open(os.path.join(root, "status.json")) as f:
                self._send(200, f.read())
        else:
            self._send(404, '{"error":{"code":"not_found","hint":"stub"}}')
    do_POST = do_GET

httpd = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(root, "stub.port"), "w") as f:
    f.write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
STUB_PID=$!

for _ in $(seq 1 50); do
    [ -f "$WORKDIR/stub.port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/stub.port" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORKDIR/stub.port")"

K2_STUB=(env K2_PORT="$STUB_PORT" K2SO_PORT="$STUB_PORT" K2_HOOK_TOKEN=fake \
    K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR")

run_status() {
    set +e
    out="$("${K2_STUB[@]}" "$K2_CLI" hostmail status "$@" 2>&1)"
    rc=$?
    set -e
}

LAST_ERR="systemd reports the stalwart unit is 'inactive'"

echo "== consistent:false → state + warning + lastError, exit 0 =="
cat >"$WORKDIR/status.json" <<JSON
{"ok":true,"consistent":false,"supported":true,"state":"stopped","version":"0.16.10","pinnedVersion":"0.16.10","hostname":"mail.lztek.io","portPlan":"tls-alpn","enableProgress":null,"lastError":"$LAST_ERR","health":null}
JSON
run_status
assert_exit "inconsistent status exit" 0 "$rc"
assert_contains "inconsistent state" "$out" "server   : stopped"
assert_contains "inconsistent warning" "$out" "WARNING"
assert_contains "inconsistent warning names disagreement" "$out" "status and systemd disagree"
assert_contains "inconsistent lastError" "$out" "$LAST_ERR"
assert_contains "inconsistent ground truth" "$out" "systemctl is-active stalwart"
assert_contains "hostname still printed" "$out" "hostname : mail.lztek.io"
assert_absent "not unexpected daemon response" "$out" "unexpected daemon response"
if ! grep -q '^/cli/mail/status' "$WORKDIR/requests.log"; then
    echo "  FAIL: CLI never called GET /cli/mail/status: $(cat "$WORKDIR/requests.log")" >&2
    fail=$((fail + 1))
else
    pass=$((pass + 1))
fi

echo "== consistent:false --json → fields passed through, exit 0 =="
run_status --json
assert_exit "inconsistent json exit" 0 "$rc"
set +e
parsed="$(printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print("ok=%r consistent=%r state=%s lastError=%s" % (d["ok"], d["consistent"], d["state"], d["lastError"]))
' 2>&1)"
prc=$?
set -e
assert_exit "json output parses with ok/consistent/state/lastError" 0 "$prc"
assert_contains "json ok" "$parsed" "ok=True"
assert_contains "json consistent" "$parsed" "consistent=False"
assert_contains "json state" "$parsed" "state=stopped"
assert_contains "json lastError" "$parsed" "lastError=$LAST_ERR"

echo "== consistent:true → no warning, exit 0 =="
cat >"$WORKDIR/status.json" <<'JSON'
{"ok":true,"consistent":true,"supported":true,"state":"disabled","version":"0.16.10","pinnedVersion":"0.16.10","hostname":"mail.lztek.io","portPlan":"tls-alpn","enableProgress":null,"lastError":null,"health":null}
JSON
run_status
assert_exit "consistent status exit" 0 "$rc"
assert_contains "consistent state" "$out" "server   : disabled"
assert_absent "consistent no warning" "$out" "WARNING"

echo "== fb449bc1-shaped daemon (ok:false carries the verdict) → still printed =="
cat >"$WORKDIR/status.json" <<JSON
{"ok":false,"supported":true,"state":"stopped","version":"0.16.10","pinnedVersion":"0.16.10","hostname":"mail.lztek.io","portPlan":null,"enableProgress":null,"lastError":"$LAST_ERR","health":null}
JSON
run_status
assert_exit "legacy ok:false status exit" 0 "$rc"
assert_contains "legacy state" "$out" "server   : stopped"
assert_contains "legacy warning" "$out" "status and systemd disagree"

echo "== real daemon errors still fail =="
cat >"$WORKDIR/status.json" <<'JSON'
{"error":{"code":"forbidden","hint":"requires owner/admin — ask your human"}}
JSON
run_status
if [ "$rc" -eq 0 ]; then
    echo "  FAIL: error envelope must not exit 0: $out" >&2
    fail=$((fail + 1))
else
    pass=$((pass + 1))
fi
assert_contains "error envelope surfaced" "$out" "requires owner/admin"

cat >"$WORKDIR/status.json" <<'JSON'
{"ok":false}
JSON
run_status
if [ "$rc" -eq 0 ]; then
    echo "  FAIL: a status body with no state must not exit 0: $out" >&2
    fail=$((fail + 1))
else
    pass=$((pass + 1))
fi
assert_contains "stateless body is unexpected" "$out" "unexpected daemon response"

echo "== help + study explain consistent / systemd / disable =="
# `k2 hostmail <anything> --help` prints the group help (asserted below), and
# `k2 mail status` has moved, so the per-verb page is read from its heredoc.
help_out="$(sed -n '/^cmd_help_mail_status() {$/,/^}$/p' "$K2_CLI")"
[ -n "$help_out" ] || { echo "FAIL: cmd_help_mail_status() not found in $K2_CLI" >&2; exit 1; }
assert_contains "status help consistent" "$help_out" "consistent: false"
assert_contains "status help systemd" "$help_out" "systemctl is-active stalwart"
assert_contains "status help disable" "$help_out" "until \`k2 hostmail enable\`"
group_help="$("$K2_CLI" hostmail --help)"
assert_contains "hostmail help consistent" "$group_help" "consistent:false"
assert_contains "hostmail help systemd" "$group_help" "systemctl is-active stalwart"
assert_contains "hostmail help disable" "$group_help" "Host-wide inbound down"
study_out="$("$K2_CLI" study mail)"
assert_contains "study consistent" "$study_out" "consistent: false"
assert_contains "study systemd" "$study_out" "systemctl is-active stalwart"
assert_contains "study disable host-wide" "$study_out" "DISABLE IS HOST-WIDE"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
