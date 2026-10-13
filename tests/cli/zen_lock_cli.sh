#!/usr/bin/env bash
# k2 zen and Garden Lock (prd-garden-lock-and-sync-v1 §5.2, §5.3, §6): the
# CLI side, against a tiny python stub daemon (the daemon side is covered by
# crates/k2-daemon/tests/zen_lock.rs).
#
# Covers:
#   1. Inside a K2 terminal (K2_HOOK_SOCK set), Garden writes (garden
#      rename/delete/template/reorder, reset, theme set/next) carry the
#      session passport, never the disk owner token.
#   2. A 423 garden_locked answer prints the daemon's message to stderr and
#      exits 3 (forbidden, stop and ask).
#   3. k2 zen garden lock|unlock sends the passport inside K2; the daemon's
#      403 owner_only exits 3 with its message.
#   4. Outside K2 the same verbs send the owner token (the person).
#   5. k2 zen validate --garden <locked> exits 3 with the lock message.
#   6. k2 zen garden list marks a locked Garden.
#
# No daemon build; HOME is sandboxed; nothing touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    if [ "$2" = "$3" ]; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (got=$(printf %q "$2") want=$(printf %q "$3"))" >&2; fail=$((fail + 1)); fi
}
assert_contains() {
    if [[ "$2" == *"$3"* ]]; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (missing $(printf %q "$3") in $(printf %q "$2"))" >&2; fail=$((fail + 1)); fi
}
assert_absent() {
    if [[ "$2" != *"$3"* ]]; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (found $(printf %q "$3"))" >&2; fail=$((fail + 1)); fi
}

WORK="$(mktemp -d -t k2-zen-lock-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-zenlock"
SCOPED="sess77.scoped-secret-zenlock"
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj" "$WORK/run"
echo "9999" >"$HOME/.k2/heartbeat.port"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
: >"$WORK/run/cell.sock"
: >"$WORK/requests.log"

python3 - "$WORK" "$OWNER" "$SCOPED" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, owner, scoped = sys.argv[1], sys.argv[2], sys.argv[3]
LOCKED = "g-locked01"
MSG = "“Diary” is locked. Agents can't change it. Ask the person to unlock it in Settings → Gardens."
WRITES = {"/cli/zen/garden/rename", "/cli/zen/garden/delete", "/cli/zen/garden/template",
          "/cli/zen/garden/reorder", "/cli/zen/reset", "/cli/zen/theme/set", "/cli/zen/theme/next"}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        tok = (q.get("token") or [""])[0]
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps({"method": method, "path": parsed.path, "token": tok, "body": raw.decode()}) + "\n")
        try:
            body = json.loads(raw) if raw.strip() else {}
        except Exception:
            body = {}
        status, out = 200, {"ok": True}
        p = parsed.path
        if p in WRITES and tok == scoped and body.get("garden") == LOCKED:
            status, out = 423, {"ok": False, "error": "garden_locked", "garden": LOCKED, "name": "Diary", "message": MSG}
        elif p == "/cli/zen/garden/lock" and tok != owner:
            status, out = 403, {"ok": False, "error": "owner_only",
                                "message": "Only the person can lock or unlock a Garden, in Settings → Gardens. Agents and K2 terminals can't, either way."}
        elif p == "/cli/zen/garden/lock":
            out = {"ok": True, "garden": {"id": body.get("garden"), "name": "Diary"}, "locked": body.get("locked"), "changed": True}
        elif p == "/cli/zen/gardens":
            out = {"ok": True, "setUp": True, "gardens": [
                {"index": 1, "id": "g-open0001", "name": "Garden 1", "template": "k2.texting@1", "locked": False},
                {"index": 2, "id": LOCKED, "name": "Diary", "template": "k2.diary@1", "locked": True}]}
        elif p == "/cli/zen/sync":
            out = {"ok": True, "gardens": []}
        elif p == "/cli/zen/validate":
            g = (q.get("garden") or [""])[0]
            out = {"ok": True, "errors": [], "warnings": [], "files": ["gardens/%s.toml" % g]}
            if g == LOCKED:
                out["locked"] = {"garden": LOCKED, "name": "Diary", "message": MSG}
        elif p == "/cli/zen/garden/rename":
            out = {"ok": True, "garden": {"id": body.get("garden"), "name": body.get("name")}, "changed": True}
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

clean_env() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u K2_HOOK_TOKEN -u K2_HOOK_SCOPED \
        -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX \
        K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORK/proj" "$@"
}
# Inside a K2 terminal: the cell's socket and passport are set.
cell() { clean_env K2_HOOK_SOCK="$WORK/run/cell.sock" K2_HOOK_TOKEN="$SCOPED" "$K2_CLI" "$@"; }
# A terminal outside K2: only the disk owner token.
outside() { clean_env "$K2_CLI" "$@"; }
last_token() { tail -n 1 "$WORK/requests.log" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["token"])'; }
last_path() { tail -n 1 "$WORK/requests.log" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["path"])'; }

run() {
    : >"$WORK/requests.log"
    set +e
    "$@" >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
}

echo "== inside a K2 terminal: Garden writes carry the passport =="
for args in "garden rename g-locked01 Journal" "garden delete g-locked01" "garden template g-locked01 blank" \
            "garden reorder g-locked01 1" "reset --garden g-locked01" "theme set paper --garden g-locked01" \
            "theme next --garden g-locked01"; do
    # shellcheck disable=SC2086
    run cell zen $args
    assert_eq "k2 zen $args: exit 3 on a locked Garden" "$rc" "3"
    assert_eq "k2 zen $args: sent the passport" "$(last_token)" "$SCOPED"
    assert_contains "k2 zen $args: the daemon's message" "$(cat "$WORK/err")" "is locked. Agents can't change it. Ask the person to unlock it in Settings"
    assert_absent "k2 zen $args: owner token never sent" "$(cat "$WORK/requests.log")" "$OWNER"
done

run cell zen garden rename g-open0001 Morning
assert_eq "an unlocked Garden: rename goes through (exit 0)" "$rc" "0"
assert_eq "an unlocked Garden: still the passport" "$(last_token)" "$SCOPED"

echo "== lock / unlock: the person only =="
for v in lock unlock; do
    run cell zen garden "$v" g-locked01
    assert_eq "agent k2 zen garden $v: exit 3" "$rc" "3"
    assert_eq "agent k2 zen garden $v: route" "$(last_path)" "/cli/zen/garden/lock"
    assert_eq "agent k2 zen garden $v: sent the passport" "$(last_token)" "$SCOPED"
    assert_contains "agent k2 zen garden $v: says who can" "$(cat "$WORK/err")" "Only the person can lock or unlock a Garden"
done
run outside zen garden lock g-locked01
assert_eq "person k2 zen garden lock: exit 0" "$rc" "0"
assert_eq "person k2 zen garden lock: owner token" "$(last_token)" "$OWNER"
assert_contains "person k2 zen garden lock: prints the state" "$(cat "$WORK/out")" "is locked: agents can't change it"
run outside zen garden unlock g-locked01 --json
assert_eq "person k2 zen garden unlock --json: exit 0" "$rc" "0"
assert_contains "unlock --json: body says locked false" "$(cat "$WORK/requests.log")" '\"locked\": false'

echo "== outside K2: the same writes are the person's =="
run outside zen garden rename g-open0001 Evening
assert_eq "outside: rename exit 0" "$rc" "0"
assert_eq "outside: owner token" "$(last_token)" "$OWNER"

echo "== validate and list =="
run cell zen validate --garden g-locked01
assert_eq "validate a locked Garden: exit 3" "$rc" "3"
assert_contains "validate: the lock message" "$(cat "$WORK/err")" "is locked. Agents can't change it"
run cell zen validate --garden g-open0001
assert_eq "validate an unlocked Garden: exit 0" "$rc" "0"
run cell zen garden list
assert_eq "garden list: exit 0" "$rc" "0"
assert_contains "garden list marks the locked Garden" "$(grep g-locked01 "$WORK/out")" "locked (agents can't change it)"
assert_absent "garden list: the unlocked Garden isn't marked" "$(grep g-open0001 "$WORK/out")" "locked"

echo "== help =="
run outside zen --help
assert_contains "help lists lock | unlock" "$(cat "$WORK/out")" "k2 zen garden lock | unlock <garden>"
assert_contains "help: the rule" "$(cat "$WORK/out")" "A locked Garden: don't edit it. Ask the person."

echo
echo "zen_lock_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
