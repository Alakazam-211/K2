#!/usr/bin/env bash
# Security patch 0.44.4 — CLI side.
#
# Covers:
#   1. A state-changing verb (`k2 reserve`) is sent as POST, never GET (the
#      daemon answers GET with 405 on those routes now).
#   2. Against a pre-0.44.4 daemon (POST answered with the old top-level
#      `method not allowed for this route` 405) the CLI replays the same
#      request once as GET.
#   3. `k2 agent hire --db-access write` from inside an agent session
#      (K2_HOOK_SOCK + passport) is refused with exit 3 / owner_only and
#      names the setting; the daemon sees NO request.
#   4. The same hire with `--db-access off` is not refused by that check.
#   5. 0.46.12: the daemon's `hire_off` refusal ("Let this agent hire
#      agents" is off) exits 3 and ends with its "Ask with:" line.
#   6. 0.46.17: an agent's plain --dry-run asks the daemon's gate (a
#      path-less dry run): refused → FAILED/not run, exit 3, the Ask line
#      (none with cards off); allowed → would apply. A --template dry run
#      prints that the real hire needs the switch.
#
# No daemon build — a python stub records every request. HOME is sandboxed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}

WORK="$(mktemp -d -t k2-0444-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
OWNER="owner-disk-token-0444"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"

# Stub daemon. Mode file: "new" → POST ok, GET 405 "POST required";
# "old" → POST 405 "method not allowed for this route", GET ok.
echo new >"$WORK/mode"
python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def _handle(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write("%s %s\n" % (method, parsed.path))
        if parsed.path == "/cli/agents/hire":
            with open(os.path.join(work, "hire_bodies.log"), "a") as f:
                f.write(raw.decode("utf-8", "replace") + "\n")
        mode = open(os.path.join(work, "mode")).read().strip()
        if mode == "hireoff" and parsed.path == "/cli/agents/hire":
            code, body = 403, {"ok": False, "error": {"code": "hire_off", "hint": "Ask your human to turn on 'Let this agent hire agents'."},
                               "permission": {"action": "agents.hire", "command": "k2 permission request agents.hire --why \"<why>\""}}
        elif mode == "hireoff-nocards" and parsed.path == "/cli/agents/hire":
            code, body = 403, {"ok": False, "error": {"code": "hire_off", "hint": "Ask your human to turn on 'Let this agent hire agents'. Permission cards are off for you, so no card can go up."}}
        elif mode == "allow" and parsed.path == "/cli/agents/hire":
            code, body = 200, {"ok": True, "dryRun": True, "hire": "allowed"}
        elif mode == "tplswitch" and parsed.path == "/cli/agents/hire":
            code, body = 200, {"ok": True, "dryRun": True, "path": os.path.join(work, "tpl-agent"), "rows": [],
                               "hireSwitch": {"on": False, "code": "hire_off", "hint": "Ask your human.",
                                              "command": "k2 permission request agents.hire --why \"<why>\""}}
        elif mode == "new" and method == "GET":
            code, body = 405, {"error": "POST required"}
        elif mode == "old" and method == "POST":
            code, body = 405, {"error": "method not allowed for this route"}
        else:
            code, body = 200, {"success": True}
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._handle("GET")

    def do_POST(self):
        self._handle("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 50); do
    [ -f "$WORK/stub.port" ] && break
    sleep 0.05
done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

run_cli() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2_HOOK_SOCK -u K2_HOOK_TOKEN -u K2SO_PORT \
        K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"
}

# ── 1. state change goes out as POST ────────────────────────────────────
echo "== k2 reserve is a POST =="
: >"$WORK/requests.log"
set +e
out="$(run_cli reserve --agent tester src/a.rs 2>"$WORK/status.err")"
rc=$?
set -e
[ "$rc" = 0 ] || echo "  stderr: $(cat "$WORK/status.err")" >&2
assert_eq "k2 reserve exit 0" "$rc" "0"
assert_eq "k2 reserve → one POST /cli/reserve" "$(cat "$WORK/requests.log")" "POST /cli/reserve"
assert_eq "k2 reserve prints the daemon answer" "$out" '{"success": true}'

# ── 2. old daemon: POST 405 → one GET replay ────────────────────────────
echo "== pre-0.44.4 daemon fallback =="
echo old >"$WORK/mode"
: >"$WORK/requests.log"
set +e
out="$(run_cli reserve --agent tester src/b.rs 2>"$WORK/status_old.err")"
rc=$?
set -e
assert_eq "old daemon exit 0" "$rc" "0"
assert_eq "old daemon → POST then GET" "$(tr '\n' '|' <"$WORK/requests.log")" "POST /cli/reserve|GET /cli/reserve|"
assert_eq "old daemon answer printed" "$out" '{"success": true}'
echo new >"$WORK/mode"

# ── 3. agent passport cannot self-grant db access ───────────────────────
echo "== hire --db-access write from an agent session =="
FAKE_SOCK="$WORK/cell.sock"
python3 -c "import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])" "$FAKE_SOCK" 2>/dev/null || touch "$FAKE_SOCK"
: >"$WORK/requests.log"
set +e
env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT \
    K2_PORT="$STUB_PORT" \
    K2_HOOK_TOKEN="sess-0444.scoped-secret" \
    K2_HOOK_SOCK="$FAKE_SOCK" \
    K2_PROJECT_PATH="$WORK" \
    "$K2_CLI" agent hire "$WORK/newagent" --db-access write >/dev/null 2>"$WORK/hire.err"
rc=$?
set -e
assert_eq "self-grant exit 3" "$rc" "3"
code="$(python3 -c 'import json,sys; print(json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])["error"]["code"])' "$WORK/hire.err")"
assert_eq "self-grant error code owner_only" "$code" "owner_only"
if grep -q "db_agent_access" "$WORK/hire.err" && grep -q "Let this agent create databases" "$WORK/hire.err"; then
    echo "  PASS: refusal names the setting and where a human changes it"
    pass=$((pass + 1))
else
    echo "  FAIL: refusal text: $(cat "$WORK/hire.err")" >&2
    fail=$((fail + 1))
fi
assert_eq "self-grant never reached the daemon" "$(cat "$WORK/requests.log")" ""
if [ -e "$WORK/newagent" ]; then
    echo "  FAIL: refused hire still created the folder" >&2
    fail=$((fail + 1))
else
    echo "  PASS: refused hire created nothing"
    pass=$((pass + 1))
fi

# ── 4. --db-access off is not a grant ───────────────────────────────────
echo "== hire --db-access off from an agent session is not refused =="
set +e
env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT \
    K2_PORT="$STUB_PORT" \
    K2_HOOK_TOKEN="sess-0444.scoped-secret" \
    K2_HOOK_SOCK="$FAKE_SOCK" \
    K2_PROJECT_PATH="$WORK" \
    "$K2_CLI" agent hire "$WORK/offagent" --db-access off --dry-run --json >/dev/null 2>"$WORK/hire_off.err"
set -e
if grep -q "can't grant itself" "$WORK/hire_off.err"; then
    echo "  FAIL: --db-access off was refused as a grant: $(cat "$WORK/hire_off.err")" >&2
    fail=$((fail + 1))
else
    echo "  PASS: --db-access off passes the self-grant check"
    pass=$((pass + 1))
fi

# ── 5. hire_off: exit 3 and the Ask with: line ──────────────────────────
echo "== hire refused while Let this agent hire agents is off =="
echo hireoff >"$WORK/mode"
set +e
run_cli agent hire "$WORK/hireoff-agent" >/dev/null 2>"$WORK/hireoff.err"
rc=$?
set -e
echo new >"$WORK/mode"
assert_eq "hire_off exit 3" "$rc" "3"
if grep -q '"hire_off"' "$WORK/hireoff.err" && tail -n 1 "$WORK/hireoff.err" | grep -q '^Ask with: k2 permission request agents.hire'; then
    echo "  PASS: the refusal names hire_off and ends with its Ask with: line"
    pass=$((pass + 1))
else
    echo "  FAIL: hire_off stderr: $(cat "$WORK/hireoff.err")" >&2
    fail=$((fail + 1))
fi

# ── 6. an agent's plain --dry-run asks the daemon's gate ────────────────
run_agent() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT \
        K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="sess-0444.scoped-secret" K2_HOOK_SOCK="$FAKE_SOCK" \
        K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"
}
echo "== an agent's dry run while Let this agent hire agents is off =="
echo hireoff >"$WORK/mode"
: >"$WORK/hire_bodies.log"
set +e
run_agent agent hire "$WORK/dry-agent" --dry-run >"$WORK/dry.out" 2>"$WORK/dry.err"
rc=$?
set -e
echo new >"$WORK/mode"
assert_eq "dry run hire_off exit 3" "$rc" "3"
if grep -q '\[would apply\]' "$WORK/dry.out"; then
    echo "  FAIL: the dry run still says would apply: $(cat "$WORK/dry.out")" >&2
    fail=$((fail + 1))
else
    echo "  PASS: no step says would apply"
    pass=$((pass + 1))
fi
if grep -q '\[FAILED\] *create-dir: .*(hire_off: ' "$WORK/dry.out" && grep -q '\[not run\] .*(the hire would be refused)' "$WORK/dry.out"; then
    echo "  PASS: the plan shows the hire refused (hire_off) and the rest not run"
    pass=$((pass + 1))
else
    echo "  FAIL: dry run plan: $(cat "$WORK/dry.out")" >&2
    fail=$((fail + 1))
fi
if grep -q '"hire_off"' "$WORK/dry.err" && tail -n 1 "$WORK/dry.err" | grep -q '^Ask with: k2 permission request agents.hire'; then
    echo "  PASS: the dry run ends with the Ask with: line"
    pass=$((pass + 1))
else
    echo "  FAIL: dry run stderr: $(cat "$WORK/dry.err")" >&2
    fail=$((fail + 1))
fi
assert_eq "the gate probe is a path-less dry run" \
    "$(python3 -c 'import json,sys; b=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]; print(json.dumps(b))' "$WORK/hire_bodies.log")" \
    '[{"dryRun": true}]'
if [ -e "$WORK/dry-agent" ]; then
    echo "  FAIL: the dry run created the folder" >&2
    fail=$((fail + 1))
else
    echo "  PASS: the dry run created nothing"
    pass=$((pass + 1))
fi

echo "== the same dry run with permission cards off for the agent =="
echo hireoff-nocards >"$WORK/mode"
set +e
run_agent agent hire "$WORK/dry-agent" --dry-run >"$WORK/dry2.out" 2>"$WORK/dry2.err"
rc=$?
set -e
echo new >"$WORK/mode"
assert_eq "cards off: exit 3" "$rc" "3"
if ! grep -q '^Ask with:' "$WORK/dry2.err" && tail -n 1 "$WORK/dry2.err" | grep -q '"hire_off"' && grep -q 'Permission cards are off for you' "$WORK/dry2.err"; then
    echo "  PASS: no Ask with: line; the daemon's hint says cards are off"
    pass=$((pass + 1))
else
    echo "  FAIL: cards-off stderr: $(cat "$WORK/dry2.err")" >&2
    fail=$((fail + 1))
fi

echo "== an agent's dry run, the daemon lets it hire =="
echo allow >"$WORK/mode"
set +e
run_agent agent hire "$WORK/dry-agent" --dry-run >"$WORK/dry3.out" 2>"$WORK/dry3.err"
rc=$?
set -e
echo new >"$WORK/mode"
assert_eq "allowed dry run exit 0" "$rc" "0"
if grep -q '\[would apply\] *create-dir' "$WORK/dry3.out"; then
    echo "  PASS: allowed: the plan says would apply"
    pass=$((pass + 1))
else
    echo "  FAIL: allowed plan: $(cat "$WORK/dry3.out") $(cat "$WORK/dry3.err")" >&2
    fail=$((fail + 1))
fi

echo "== a template dry run says the real hire needs the switch =="
echo tplswitch >"$WORK/mode"
set +e
run_agent agent hire --template writer --name "Tpl" --dry-run >"$WORK/tpl.out" 2>"$WORK/tpl.err"
rc=$?
set -e
echo new >"$WORK/mode"
assert_eq "template dry run exit 0" "$rc" "0"
if grep -q "needs *'Let this agent hire agents': it is off, so the real hire is refused (hire_off)" "$WORK/tpl.out" \
    && grep -q 'ask with *k2 permission request agents.hire' "$WORK/tpl.out"; then
    echo "  PASS: the template plan names the switch and the card"
    pass=$((pass + 1))
else
    echo "  FAIL: template plan: $(cat "$WORK/tpl.out") $(cat "$WORK/tpl.err")" >&2
    fail=$((fail + 1))
fi

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
