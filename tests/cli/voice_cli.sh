#!/usr/bin/env bash
# k2 voice (voice rooms, prd-k2-voice-rooms-v1 §9.3; lane C) — the CLI over
# /cli/voice/*, against a stub daemon.
#
# Covers:
#   1. help and `k2 study voice-rooms` without a daemon; the tool catalog
#      (voice rides the locked hostchat id: a K2 terminal sends its
#      passport, never the owner token); the schema lists `voice`.
#   2. Request shapes: rooms/status GET; set rooms N [--now] →
#      /cli/voice/settings {count, now}; rename (multi-word names); close;
#      join {room}; leave {callId}; kick|mute|… → /cli/voice/moderate
#      {room, person, action}.
#   3. Summaries print people and the hosting line; --json prints raw.
#   4. Exit codes: usage errors exit 2 before any /cli/voice request;
#      refusals (403 agents_not_in_voice, 409 close_from_top) exit 3 with
#      the code on stderr.
# Fake values only. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

WORK="$(mktemp -d -t k2-voice-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

# ── 1. no daemon ─────────────────────────────────────────────────────
echo "== help + study + catalog (no daemon) =="
HOME1="$WORK/home1"
mkdir -p "$HOME1"
set +e
out="$(env HOME="$HOME1" K2_PORT=1 "$K2_CLI" voice --help 2>&1)"; rc=$?
set -e
assert_eq "voice --help exit 0" "$rc" "0"
assert_contains "help: set rooms" "$out" "k2 voice set rooms <0-10> [--now]"
assert_contains "help: owner token never joins" "$out" "never joins or moderates"
assert_contains "help: study page" "$out" "k2 study voice-rooms"
assert_contains "help: agents are read only" "$out" "Agents: read only"
out="$(env HOME="$HOME1" K2_PORT=1 "$K2_CLI" study voice-rooms 2>&1)"
assert_contains "study page title" "$out" "k2 study voice-rooms — Voice rooms on this server (preview)"
assert_contains "study page points App voice elsewhere" "$out" "k2 study chat-voice-rooms"
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "catalog: voice rides the hostchat tool id" "$(_cli_tool_id_for_verb voice)" "hostchat"
if _cli_tool_is_locked "$(_cli_tool_id_for_verb voice)"; then ok "catalog: voice is locked"; else bad "catalog: voice is locked"; fi
env HOME="$HOME1" "$K2_CLI" --schema | python3 -c '
import json, sys
names = {c["name"] for c in json.load(sys.stdin)["commands"]}
assert "voice" in names
' && ok "schema lists voice" || bad "schema lists voice"

# ── 2. stub daemon ───────────────────────────────────────────────────
echo "== stub daemon =="
OWNER="owner-disk-token-voice"
SCOPED="sess42.scoped-secret-voice"
HOME2="$WORK/home2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" "$SCOPED" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, scoped = sys.argv[1], sys.argv[2]
ROOMS = {"ok": True, "server": "box", "hosting": {"state": "ok", "line": "Voice rooms are on."}, "rev": 3, "canManage": True,
         "rooms": [{"id": "room-1", "slug": "room-1", "name": "Standup", "limit": 25, "closing": False, "unavailable": None,
                    "occupants": [{"user": "ada", "muted": True, "deafened": False}, {"user": "owner", "muted": False, "deafened": False}]},
                   {"id": "room-2", "slug": "room-2", "name": "Room 2", "limit": 25, "closing": True, "unavailable": None, "occupants": []}],
         "me": {"user": "ada", "inCall": True, "room": {"id": "room-1", "name": "Standup"}, "channelId": "901", "here": False, "elsewhere": True,
                "on": {"kind": "phone", "name": "Ada's iPhone"}, "mayBeLeaving": True, "presenceLagSecs": 45, "presence": "live"}}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {"method": method, "path": parsed.path, "token": (q.get("token") or [""])[0], "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        b = json.loads(body) if body else {}
        p = parsed.path
        status, out = 200, {"ok": True}
        if p == "/cli/voice/rooms":
            out = ROOMS
        elif p == "/cli/voice/status":
            out = {"ok": True, "hosting": {"state": "voice_off", "line": "Voice isn't set up: this server's Sew is too old for voice rooms.", "detail": "sew_too_old"},
                   "count": 1, "tier": {"state": "pro", "reading": "fresh"}, "space": {"spaceId": "7001", "state": "ready"}, "members": [], "rooms": [],
                   "holds": [], "lane": {"state": "not_applicable"}, "feed": {"state": "live"}}
        elif p == "/cli/voice/settings":
            out = {"ok": True, "count": b.get("count"), "closing": ["room-2"] if b.get("count", 9) < 2 else [], "closeNowAt": 1 if b.get("now") else None}
        elif p == "/cli/voice/rooms/rename":
            out = {"ok": True, "room": "room-1", "name": b.get("name")}
        elif p == "/cli/voice/rooms/close":
            status, out = 409, {"ok": False, "code": "close_from_top", "error": {"code": "close_from_top", "hint": "rooms close from the top"}}
        elif p == "/cli/voice/join":
            if rec["token"] == scoped:
                status, out = 403, {"ok": False, "code": "agents_not_in_voice", "error": {"code": "agents_not_in_voice", "hint": "agents don't join voice rooms"}}
            else:
                out = {"ok": True, "callId": "vc_1", "room": {"id": "room-1", "name": "Standup"}, "join": {"url": "wss://voice.box.k2.dev", "token": "m"}}
        elif p == "/cli/voice/session/leave":
            if b.get("callId") == "vc_old":
                out = {"ok": True, "left": True, "already": True, "why": "joined_elsewhere", "room": "room-1", "channelId": "901"}
            else:
                out = {"ok": True, "left": True, "already": False, "room": "room-1", "channelId": "901"}
        elif p == "/cli/voice/moderate":
            out = {"ok": True, "action": b.get("action"), "person": b.get("person"), "room": {"name": "Standup"}}
        self.send_response(status)
        data = json.dumps(out).encode()
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
STUB_PORT="$(cat "$WORK/stub.port")"

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@"; }
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
nreq() { if [ -f "$WORK/reqs.jsonl" ]; then grep -c '"path": "/cli/voice/' "$WORK/reqs.jsonl" || true; else echo 0; fi; }
req_where() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
rows = [r for r in rows if r["path"] == sys.argv[2]]
r = rows[-1]
print(r[sys.argv[3]] if sys.argv[3] != "body" else json.dumps(json.loads(r["body"] or "{}"), sort_keys=True))
' "$WORK/reqs.jsonl" "$1" "$2"
}
run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
mkdir -p "$WORK/proj"
cd "$WORK"

echo "== rooms + status =="
run owner voice rooms
assert_eq "rooms exit 0" "$rc" "0"
assert_contains "rooms: hosting line" "$out" "Voice rooms on box: on"
assert_contains "rooms: a room with its count" "$out" "Standup"
assert_contains "rooms: a muted person" "$out" "ada (muted)"
assert_contains "rooms: closing" "$out" "closing"
assert_contains "rooms: in a call on another device (phones, §6.4)" "$out" "you: in Standup from another device or session (if that app was closed, this can lag up to 45 s)"
assert_eq "rooms is a GET" "$(req_where /cli/voice/rooms method)" "GET"
run owner voice rooms --json
assert_eq "rooms --json is the raw body" "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["rev"])' "$out")" "3"
run owner voice status
assert_contains "status: hosting state and detail" "$out" "voice_off: Voice isn't set up"
assert_contains "status: detail" "$out" "(sew_too_old)"
assert_contains "status: the Pro gate reading" "$out" "(plan: pro, fresh reading)"

echo "== owner actions =="
run owner voice set rooms 1 --now
assert_eq "set rooms exit 0" "$rc" "0"
assert_eq "set rooms body" "$(req_where /cli/voice/settings body)" '{"count": 1, "now": true}'
assert_eq "set rooms is a POST" "$(req_where /cli/voice/settings method)" "POST"
assert_contains "set rooms: what closes" "$out" "room-2 (calls end in 10 s)"
run owner voice set rooms 3
assert_eq "set rooms without --now" "$(req_where /cli/voice/settings body)" '{"count": 3}'
run owner voice rename room-1 Daily standup
assert_eq "rename body (multi-word name)" "$(req_where /cli/voice/rooms/rename body)" '{"name": "Daily standup", "room": "room-1"}'
assert_contains "rename summary" "$out" "room-1 is now Daily standup"
run owner voice close room-1
assert_eq "close refused exits 3" "$rc" "3"
assert_contains "close: the code on stderr" "$err" "close_from_top"

echo "== join, leave, moderation =="
run owner voice join room-1
assert_eq "join body" "$(req_where /cli/voice/join body)" '{"room": "room-1"}'
assert_contains "join says the CLI has no audio" "$out" "The CLI has no audio"
run cell voice join room-1
assert_eq "agent join exits 3" "$rc" "3"
assert_contains "agent join: agents_not_in_voice" "$err" "agents_not_in_voice"
assert_eq "agent sends its passport, never the owner token" "$(req_where /cli/voice/join token)" "$SCOPED"
run owner voice leave --call vc_1
assert_eq "leave body" "$(req_where /cli/voice/session/leave body)" '{"callId": "vc_1"}'
assert_eq "leave says left" "$out" "left"
run owner voice leave --call vc_old
assert_eq "a leave of an ended call exits 0" "$rc" "0"
assert_eq "an ended call: already left, and why" "$out" "nothing to leave: that call already ended (you joined from another device or session; that call goes on) [joined_elsewhere]"
run owner voice kick room-1 ada
assert_eq "kick body" "$(req_where /cli/voice/moderate body)" '{"action": "kick", "person": "ada", "room": "room-1"}'
assert_contains "kick summary" "$out" "kick ada in Standup"
run owner voice deafen 1 owner
assert_eq "deafen body" "$(req_where /cli/voice/moderate body)" '{"action": "deafen", "person": "owner", "room": "1"}'

echo "== usage errors send nothing =="
before="$(nreq)"
for args in "set rooms 11" "set rooms x" "set seats 2" "rooms --now" "join" "kick room-1" "rename room-1" "frobnicate" "status extra"; do
    # shellcheck disable=SC2086
    run owner voice $args
    assert_eq "usage: k2 voice $args exits 2" "$rc" "2"
done
assert_eq "no /cli/voice request for a usage error" "$(nreq)" "$before"

echo
echo "voice_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
