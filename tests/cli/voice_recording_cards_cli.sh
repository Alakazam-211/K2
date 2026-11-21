#!/usr/bin/env bash
# The agent's side of the 0.46.17 recording cards, against a stub daemon:
#
#   1. k2 voice settings --start-level <level> from a K2 terminal sends the
#      change once, gets recording_level_needs_owner with the card, and
#      ends with "Ask with: k2 permission request
#      voice.recording-start-level level=<level> …" (exit 3).
#   2. k2 hostchat recording policy set dm_enabled on|off the same, with
#      voice.recording-dm dm=on|off; never a vault confirm in a session.
#      Another key beside dm_enabled: the daemon's plain person_only, no
#      card line.
#   3. Help never acts: -h / --help on those lines sends nothing.
# The stub stands for the daemon's walls (the daemon's own tests pin which
# refusal gets which card); this pins that the CLI sends the right body and
# prints the line. Fake values only. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_lacks() { if <<<"$2" grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d /tmp/k2rc.XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}

OWNER="owner-disk-token-reccards"
SCOPED="sess55.scoped-secret-reccards"
H="$WORK/h"
mkdir -p "$H/.k2" "$WORK/proj"
echo "$OWNER" > "$H/.k2/heartbeat.token"
chmod 600 "$H/.k2/heartbeat.token"

python3 - "$WORK" "$SCOPED" "$H/.k2/vault.sock" <<'PYEOF' &
import json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, scoped, vsock = sys.argv[1], sys.argv[2], sys.argv[3]
WHY = ' --why "<what you need it for>"'

def card(action, values):
    cmd = "k2 permission request " + action + "".join(" %s=%s" % kv for kv in values) + WHY
    return {"requestable": True, "action": action, "approvers": "owner or admin", "command": cmd}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        auth = self.headers.get("Authorization") or ""
        token = auth[len("Bearer "):] if auth.startswith("Bearer ") else ""
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps({"method": method, "path": parsed.path, "token": token, "body": body.decode("utf-8", "replace")}) + "\n")
        b = json.loads(body) if body else {}
        p = parsed.path
        status, out = 404, {"error": "route not found"}
        if p == "/cli/voice/recording/settings" and method == "POST" and token == scoped:
            lvl = b.get("startLevel")
            if isinstance(lvl, str) and "required" not in b:
                status = 403
                out = {"ok": False, "code": "recording_level_needs_owner", "hint": "nothing changed",
                       "error": {"code": "recording_level_needs_owner", "hint": "nothing changed"},
                       "permission": card("voice.recording-start-level", [("level", lvl)])}
            else:
                status = 403
                out = {"ok": False, "code": "owner_or_admin_only", "hint": "x", "error": {"code": "owner_or_admin_only", "hint": "x"}}
        elif p == "/cli/sew/recording/policy" and method == "POST" and token == scoped:
            ch = b.get("changes") or {}
            status, out = 403, {"ok": False, "error": {"code": "person_only", "hint": "nothing changed: a person does this"}}
            if list(ch) == ["recording.dm_enabled"]:
                out["permission"] = card("voice.recording-dm", [("dm", "on" if ch["recording.dm_enabled"] else "off")])
        elif p.startswith("/cli/vault/"):
            status, out = 500, {"error": "no vault confirm in a session"}
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

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

class UH(H):
    def address_string(self):
        return "unix"

if os.path.exists(vsock):
    os.unlink(vsock)
us = U(vsock, UH)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && [ -S "$H/.k2/vault.sock" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] && [ -S "$H/.k2/vault.sock" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$WORK/stub.port")"
touch "$WORK/reqs.jsonl"

cell() {
    env HOME="$H" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@" </dev/null
}
nreq() { grep -cE '"path": "/cli/(voice|sew|vault)/' "$WORK/reqs.jsonl" || true; }
count_path() { python3 -c '
import json, sys
print(sum(1 for l in open(sys.argv[1]) if json.loads(l)["path"] == sys.argv[2]))' "$WORK/reqs.jsonl" "$1"; }
last_body() { python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if json.loads(l)["path"] == sys.argv[2]]
print(json.dumps(json.loads(rows[-1]["body"] or "{}"), sort_keys=True))' "$WORK/reqs.jsonl" "$1"; }
cd "$WORK"

echo "== start level =="
for level in none moderator member; do
    n0="$(count_path /cli/voice/recording/settings)"
    run cell voice settings --start-level "$level"
    assert_eq "voice settings --start-level $level from a K2 terminal → exit 3" "$rc" "3"
    assert_eq "$level: one POST" "$(( $(count_path /cli/voice/recording/settings) - n0 ))" "1"
    assert_eq "$level: the body" "$(last_body /cli/voice/recording/settings)" "{\"startLevel\": \"$level\"}"
    assert_contains "$level: its own code" "$err" "recording_level_needs_owner"
    assert_contains "$level: the Ask line" "$err" "Ask with: k2 permission request voice.recording-start-level level=$level --why"
done
run cell voice settings --require-recording on --start-level member
assert_eq "--require-recording with it → exit 3" "$rc" "3"
assert_contains "--require-recording: the shared refusal" "$err" "owner_or_admin_only"
assert_lacks "--require-recording: no card" "$err" "Ask with: k2 permission request voice.recording-start-level"

echo "== dm =="
for v in on off; do
    n0="$(count_path /cli/sew/recording/policy)"
    run cell hostchat recording policy set dm_enabled "$v"
    assert_eq "policy set dm_enabled $v from a K2 terminal → exit 3" "$rc" "3"
    assert_eq "dm $v: one POST, not resent" "$(( $(count_path /cli/sew/recording/policy) - n0 ))" "1"
    if [ "$v" = on ]; then b=true; else b=false; fi
    assert_eq "dm $v: the body" "$(last_body /cli/sew/recording/policy)" "{\"changes\": {\"recording.dm_enabled\": $b}}"
    assert_contains "dm $v: the Ask line" "$err" "Ask with: k2 permission request voice.recording-dm dm=$v --why"
done
run cell hostchat recording policy set dm_enabled on enabled on
assert_eq "dm_enabled and enabled → exit 3" "$rc" "3"
assert_contains "dm_enabled and enabled: person_only" "$err" "person_only"
assert_lacks "dm_enabled and enabled: no dm card" "$err" "voice.recording-dm"
assert_eq "no vault confirm in a session" "$(grep -c '"path": "/cli/vault/' "$WORK/reqs.jsonl" || true)" "0"

echo "== help never acts =="
for line in "voice settings --start-level member -h" "voice settings --help --start-level member" \
            "hostchat recording policy set dm_enabled on -h" "hostchat recording policy set dm_enabled on --help"; do
    before="$(nreq)"
    # shellcheck disable=SC2086
    run cell $line
    assert_eq "help: $line → exit 0, no request" "$rc/$(nreq)" "0/$before"
done

echo
echo "voice recording cards CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
