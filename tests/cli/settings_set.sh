#!/usr/bin/env bash
# `k2 settings set <key> <true|false>` (0.46.9): the owner's server preview
# switches that K2's own hints name. The CLI decides only the allowlist and
# the value; the daemon (POST /cli/settings/update, OWNER_ONLY_KEYS) decides
# who may write. Checked here:
#   * each allowed key sends exactly {"<key>": true|false}; on/off work;
#   * an unknown key, a bad value or a wrong arg count exits 2 with the
#     allowed list, and sends nothing;
#   * a refusal (an agent passport, an Admin) exits 3 with the owner line;
#   * an answer that doesn't carry the new value exits 1;
#   * `k2 settings` and `k2 settings --mode` still do what they did.
#
# Hermetic: throwaway HOME, every K2_*/K2SO_* var unset, a python stub daemon
# on loopback that logs each request and answers by token. No real daemon,
# ~/.k2 or keychain.
#
#   bash tests/cli/settings_set.sh      (fails loudly on any miss)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d /tmp/k2sset-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

# The stub. By the request's token:
#   owner-tok  → 200 the merged settings (the body's keys over defaults)
#   stale-tok  → 200 the defaults only (the write didn't stick)
#   anything else → 403 owner_only (an agent passport, an Admin login)
# GET /cli/settings → 200 a workspace view; POST /cli/mode → 200 {mode}.
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]
DEFAULTS = {"voiceRoomsPreview": False, "voiceTunnelPreview": False, "computePreview": False, "boardFilePreview": False, "videoRoomsPreview": False}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def reply(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        u = urlparse(self.path)
        q = parse_qs(u.query)
        token = (q.get("token") or [""])[0]
        with open(LOG, "a") as f:
            f.write("%s %s %s\n" % (self.command, u.path, body))
        if u.path == "/cli/settings/update":
            if self.command != "POST":
                return self.reply(405, {"error": "method not allowed"})
            if token == "owner-tok":
                merged = dict(DEFAULTS)
                merged.update(json.loads(body))
                return self.reply(200, merged)
            if token == "stale-tok":
                return self.reply(200, dict(DEFAULTS))
            return self.reply(403, {"error": {"code": "owner_only", "hint": "only the server's owner may change this setting"}})
        if u.path == "/cli/settings":
            return self.reply(200, {"name": "ws", "mode": "custom", "heartbeatEnabled": False, "pinned": True})
        if u.path == "/cli/mode":
            return self.reply(200, {"mode": (q.get("set") or ["?"])[0]})
        return self.reply(404, {"error": "no such route in the stub"})
    do_GET = handle_any
    do_POST = handle_any

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
: > "$LOG"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

pass=0
ok() { pass=$((pass + 1)); }
bad() { echo "FAIL: $*" >&2; exit 1; }

# run <token> <k2 args…>: OUT, ERR, RC. The log is cleared first.
run() {
    local token="$1"; shift
    : > "$LOG"
    set +e
    K2_PORT="$PORT" K2_HOOK_TOKEN="$token" "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    RC=$?
    set -e
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
}

KEYS_LINE="voiceRoomsPreview, voiceTunnelPreview, computePreview, boardFilePreview, videoRoomsPreview"

# ── each allowed key, both values ─────────────────────────────────────────
for key in voiceRoomsPreview voiceTunnelPreview computePreview boardFilePreview videoRoomsPreview; do
    run owner-tok settings set "$key" true
    [ "$RC" = 0 ] || bad "$key true: exit $RC ($ERR)"
    [ "$(cat "$LOG")" = "POST /cli/settings/update {\"$key\":true}" ] || bad "$key true: wrong request: $(cat "$LOG")"
    printf '%s\n' "$OUT" | grep -qx "$key is now true on this server." || bad "$key true: stdout '$OUT'"
    printf '%s\n' "$OUT" | grep -q '^Next: ' || bad "$key true: no Next line: '$OUT'"
    ok

    run owner-tok settings set "$key" false
    [ "$RC" = 0 ] || bad "$key false: exit $RC ($ERR)"
    [ "$(cat "$LOG")" = "POST /cli/settings/update {\"$key\":false}" ] || bad "$key false: wrong request: $(cat "$LOG")"
    [ "$OUT" = "$key is now false on this server." ] || bad "$key false: stdout '$OUT'"
    ok
done

# on / off (any case) mean true / false.
run owner-tok settings set voiceRoomsPreview ON
[ "$RC" = 0 ] && [ "$(cat "$LOG")" = 'POST /cli/settings/update {"voiceRoomsPreview":true}' ] && ok || bad "ON: rc $RC log $(cat "$LOG")"
run owner-tok settings set voiceTunnelPreview off
[ "$RC" = 0 ] && [ "$(cat "$LOG")" = 'POST /cli/settings/update {"voiceTunnelPreview":false}' ] && ok || bad "off: rc $RC log $(cat "$LOG")"

# ── refused before any request: exit 2 with the list ──────────────────────
run owner-tok settings set apps true
[ "$RC" = 2 ] || bad "unknown key: exit $RC, want 2 ($ERR)"
[ "$ERR" = "Error: k2 settings set doesn't change 'apps'. It takes: $KEYS_LINE. Other settings are in the K2 app's Settings." ] || bad "unknown key: stderr '$ERR'"
[ ! -s "$LOG" ] && ok || bad "unknown key sent a request: $(cat "$LOG")"

run owner-tok settings set voice_rooms_preview true
[ "$RC" = 2 ] && [ ! -s "$LOG" ] && ok || bad "snake_case key: exit $RC, log $(cat "$LOG")"

run owner-tok settings set voiceRoomsPreview maybe
[ "$RC" = 2 ] || bad "bad value: exit $RC ($ERR)"
[ "$ERR" = "Error: voiceRoomsPreview takes true or false (on and off work too), not 'maybe'." ] || bad "bad value: stderr '$ERR'"
[ ! -s "$LOG" ] && ok || bad "bad value sent a request: $(cat "$LOG")"

for args in "" "voiceRoomsPreview" "voiceRoomsPreview true extra"; do
    # shellcheck disable=SC2086
    run owner-tok settings set $args
    [ "$RC" = 2 ] || bad "args '$args': exit $RC, want 2 ($ERR)"
    [ "$ERR" = "Error: usage: k2 settings set <key> <true|false>  (keys: $KEYS_LINE)" ] || bad "args '$args': stderr '$ERR'"
    [ ! -s "$LOG" ] && ok || bad "args '$args' sent a request: $(cat "$LOG")"
done

# ── the daemon refuses (an agent passport, an Admin): exit 3 ──────────────
run passport-agent settings set voiceRoomsPreview true
[ "$RC" = 3 ] || bad "refused: exit $RC, want 3 ($ERR)"
[ "$(cat "$LOG")" = 'POST /cli/settings/update {"voiceRoomsPreview":true}' ] || bad "refused: wrong request: $(cat "$LOG")"
printf '%s\n' "$ERR" | grep -qx "Error: owner_only: only the server's owner may change this setting" || bad "refused: no error line: '$ERR'"
printf '%s\n' "$ERR" | grep -qx "Server settings are the owner's: ask your human, or run it from a terminal outside a K2 agent session." || bad "refused: no owner line: '$ERR'"
[ -z "$OUT" ] && ok || bad "refused: stdout '$OUT'"

# ── a 200 that doesn't carry the new value: exit 1, not a false success ───
run stale-tok settings set computePreview true
[ "$RC" = 1 ] || bad "stale: exit $RC, want 1 ($ERR)"
printf '%s\n' "$ERR" | grep -q "computePreview is false, not true" && ok || bad "stale: stderr '$ERR'"

# ── no daemon: exit 1 ─────────────────────────────────────────────────────
set +e
K2_PORT=1 K2_HOOK_TOKEN=owner-tok "$K2" settings set voiceRoomsPreview true </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
RC=$?
set -e
[ "$RC" = 1 ] || bad "no daemon: exit $RC, want 1 ($(cat "$SANDBOX/err"))"
grep -q "no answer from the K2 daemon" "$SANDBOX/err" && ok || bad "no daemon: stderr '$(cat "$SANDBOX/err")'"

# ── help ──────────────────────────────────────────────────────────────────
run owner-tok settings set --help
[ "$RC" = 0 ] || bad "set --help: exit $RC"
for line in "k2 settings set <key> <true|false>" "voiceRoomsPreview" "voiceTunnelPreview" "computePreview" "boardFilePreview" "videoRoomsPreview"; do
    printf '%s\n' "$OUT" | grep -qF "$line" || bad "set --help: missing '$line'"
done
[ ! -s "$LOG" ] && ok || bad "set --help sent a request: $(cat "$LOG")"

# ── the old forms are unchanged ───────────────────────────────────────────
run owner-tok settings
[ "$RC" = 0 ] || bad "k2 settings: exit $RC ($ERR)"
grep -q '^GET /cli/settings ' "$LOG" || bad "k2 settings: wrong request: $(cat "$LOG")"
printf '%s\n' "$OUT" | grep -qx "  Mode:      custom" && ok || bad "k2 settings: stdout '$OUT'"

run owner-tok settings --mode manager
[ "$RC" = 0 ] || bad "--mode: exit $RC ($ERR)"
grep -q '^POST /cli/mode ' "$LOG" || bad "--mode: wrong request: $(cat "$LOG")"
[ "$OUT" = "Mode set to manager" ] && ok || bad "--mode: stdout '$OUT'"

echo "settings_set: $pass checks passed"
