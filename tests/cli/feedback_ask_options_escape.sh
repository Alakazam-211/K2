#!/usr/bin/env bash
# `k2 feedback ask --options` tokenization — escaped-comma support.
#
# Bug: the option string was split on EVERY comma, so a label containing a
# literal comma — `--options "Local 8B (slow, private),Hosted"` — shattered
# into bogus options ("Local 8B (slow" / "private)"). The fix splits on
# UNESCAPED commas only: `\,` inside an option yields a literal comma in the
# label; whitespace around options is still trimmed; empty tokens (trailing /
# double commas) are still skipped.
#
# This test exercises the REAL code path — `cli/k2 feedback ask` end-to-end
# against a stub daemon that records the POST /cli/feedback/create payload —
# and asserts the exact `options` array the CLI sends:
#   1. escaped comma      "Local 8B (slow\, private),Hosted endpoint (fast),Hybrid"
#   2. plain split        "Ship it,Hold,Needs changes"
#   3. trailing/double    "Go,Stop,,"
#   4. whitespace trim    "  Go , Stop  "
# Plus: the `--help` text documents the `\,` escape.
#
# No daemon build needed — the stub is a tiny python3 HTTP server. Never
# touches ~/.k2 (connection comes from K2_PORT/K2_HOOK_TOKEN env).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

WORK="$(mktemp -d -t k2-fb-options-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

# ── Stub daemon: records each create payload, replies ok ────────────────────
python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
counter = {"n": 0}

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.path.split("?")[0] == "/cli/feedback/create":
            counter["n"] += 1
            with open(os.path.join(work, "req-%d.json" % counter["n"]), "wb") as f:
                f.write(body)
            resp = {"ok": True, "id": "fb-stub-%08d" % counter["n"],
                    "title": "t", "kind": "question", "priority": 3,
                    "status": "waiting"}
        else:
            resp = {"error": {"code": "not_found", "hint": "stub: unknown route"}}
        data = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID"   # silence bash's "Terminated" notice when the trap kills it

for _ in $(seq 1 50); do
    [ -f "$WORK/stub.port" ] && break
    sleep 0.1
done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub daemon never wrote its port" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

PASS=0
FAIL=0

# run_case <n> <options-string> <expected-json-array>
run_case() {
    local n="$1" opts="$2" expected="$3"
    local out
    if ! out="$(cd "$WORK" && env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
            K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="stub-token" \
            K2_PROJECT_PATH="$WORK" \
            "$K2_CLI" feedback ask "case $n" --options "$opts" --json 2>&1)"; then
        echo "FAIL: case $n — CLI exited non-zero: $out" >&2
        FAIL=$((FAIL + 1))
        return
    fi
    local req="$WORK/req-$n.json"
    if [ ! -f "$req" ]; then
        echo "FAIL: case $n — stub recorded no create payload" >&2
        FAIL=$((FAIL + 1))
        return
    fi
    local got
    got="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])).get("options")))' "$req")"
    if [ "$got" = "$expected" ]; then
        echo "PASS: case $n — --options '$opts' → $got"
        PASS=$((PASS + 1))
    else
        echo "FAIL: case $n — --options '$opts'" >&2
        echo "      expected: $expected" >&2
        echo "      got:      $got" >&2
        FAIL=$((FAIL + 1))
    fi
}

# 1. Escaped comma → literal comma in the label (the original failing input).
run_case 1 'Local 8B (slow\, private),Hosted endpoint (fast),Hybrid' \
    '["Local 8B (slow, private)", "Hosted endpoint (fast)", "Hybrid"]'

# 2. Plain split is unchanged.
run_case 2 'Ship it,Hold,Needs changes' \
    '["Ship it", "Hold", "Needs changes"]'

# 3. Trailing / double commas do not produce empty options.
run_case 3 'Go,Stop,,' \
    '["Go", "Stop"]'

# 4. Whitespace around options is trimmed.
run_case 4 '  Go , Stop  ' \
    '["Go", "Stop"]'

# 5. Help documents the escape.
if env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="stub-token" \
        "$K2_CLI" feedback ask --help 2>/dev/null \
        | grep -qF 'Escape a literal comma inside an option with \,'; then
    echo "PASS: case 5 — help documents the \\, escape"
    PASS=$((PASS + 1))
else
    echo "FAIL: case 5 — help text missing the \\, escape line" >&2
    FAIL=$((FAIL + 1))
fi

echo
echo "feedback_ask_options_escape: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
