#!/usr/bin/env bash
# Recording in Apps headless check (K2 PRD prd-k2-bobbin-in-apps-v1, Sew 0.9.0).
#
# A daemon-only run: no client, no webview, no real Sew. It starts a k2-daemon
# in a throwaway HOME, a small fake Sew mgmt socket (python3, Unix socket)
# that reports Sew 0.9.0's four recording flags, points the daemon's chat
# target at it, waits for one sweep, then drives the new routes over curl:
#
#   flags reach GET /cli/sew/recording/policies → the org policies Sew shows →
#   GET on each mutation is 405, POST on each read is 405 → the CUI/FCI lock and a
#   policy without a confirm are refused for the owner (owner_confirm_required)
#   → an App read without an App pass (app_pass_required) → the K5 switch
#   is refused (client_consent_missing) → an unknown App is refused.
#
#   scripts/recording-in-apps-headless.sh <path/to/k2-daemon>
#
# Linux only (the chat target is a Linux feature). Every K2_* variable is
# unset first; the real ~/.k2, the keychain and any running daemon are never
# touched. Fails loudly (exit 1) on the first wrong answer.

set -euo pipefail

BIN="${1:-}"
[ -x "$BIN" ] || { echo "usage: $0 <path/to/k2-daemon>" >&2; exit 2; }
[ "$(uname -s)" = Linux ] || { echo "Linux only (the chat target is a Linux box's)" >&2; exit 2; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/k2-rec-headless-XXXXXX")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME"
# The voice rooms preview, so /cli/voice/* answers (it is off by default).
export K2_VOICE_ROOMS=1
PID=""
FAKE_PID=""
cleanup() {
    for p in "$PID" "$FAKE_PID"; do
        if [ -n "$p" ]; then
            kill "$p" 2>/dev/null || true
            wait "$p" 2>/dev/null || true
        fi
    done
    if [ "${KEEP:-0}" != 1 ]; then rm -rf "$SANDBOX"; fi
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log (last 40 lines) ---" >&2
    tail -40 "$SANDBOX/daemon.log" >&2 || true
    echo "--- fake Sew log ---" >&2
    tail -40 "$SANDBOX/sew.log" >&2 || true
    exit 1
}
pass() { echo "PASS: $*"; }

# ── the fake Sew: status (0.9.0's flags), principals, policies, settings ──
SOCK="$SANDBOX/sew.sock"
TOKFILE="$SANDBOX/sew.token"
printf 'headless-mgmt-token\n' > "$TOKFILE"
cat > "$SANDBOX/fake_sew.py" <<'PY'
import json, os, socket, sys, threading
sock_path, log_path = sys.argv[1], sys.argv[2]
FLAGS = ["app_governed_spaces", "home_client", "app_origin", "app_origins", "k2_managed_avatar",
         "app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"]
policies = {"recording.enabled": False, "recording.dm_enabled": False, "recording.require_consent_to_join": False,
            "recording.required": "off", "recording.audio_max_days": 30,
            "identity.managed_fields": ["display_name", "title", "pronouns", "avatar"]}
log = open(log_path, "a", buffering=1)
def answer(c, status, body):
    b = json.dumps(body).encode()
    c.sendall(b"HTTP/1.1 %d X\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: %d\r\n\r\n" % (status, len(b)) + b)
def serve(c):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = c.recv(65536)
        if not chunk:
            return
        data += chunk
    head, _, rest = data.partition(b"\r\n\r\n")
    lines = head.decode().split("\r\n")
    method, target = lines[0].split(" ")[:2]
    hdrs = {l.split(":", 1)[0].lower(): l.split(":", 1)[1].strip() for l in lines[1:] if ":" in l}
    n = int(hdrs.get("content-length", "0"))
    while len(rest) < n:
        rest += c.recv(65536)
    path = target.split("?")[0]
    log.write(f"{method} {target} actor={hdrs.get('sew-actor')} {rest[:n].decode()}\n")
    if hdrs.get("authorization") != "Bearer headless-mgmt-token":
        return answer(c, 401, {"ok": False, "error": {"code": "unauthenticated", "hint": "token", "details": {}}})
    if path == "/mgmt/v1/status":
        return answer(c, 200, {"version": "0.9.0-fake", "features": FLAGS, "schema_version": 990011, "install_id": "4242",
                               "provider": "k2", "db": {}, "jobs": {}, "search": {}, "keys": {}, "sessions": 0, "sockets": 0})
    if path == "/mgmt/v1/principals":
        return answer(c, 200, {"items": [], "has_more": False})
    if path == "/mgmt/v1/policies" and method == "GET":
        return answer(c, 200, {"policies": policies})
    if path == "/mgmt/v1/policies" and method == "POST":
        policies.update(json.loads(rest[:n] or b"{}").get("changes", {}))
        return answer(c, 200, {"policies": policies})
    if method == "POST":
        return answer(c, 200, {"applied": True})
    return answer(c, 404, {"ok": False, "error": {"code": "not_found", "hint": "fake", "details": {"reason": "unknown_route"}}})
if os.path.exists(sock_path):
    os.unlink(sock_path)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(sock_path)
s.listen(16)
while True:
    conn, _ = s.accept()
    threading.Thread(target=lambda c=conn: (serve(c), c.close()), daemon=True).start()
PY
python3 -I "$SANDBOX/fake_sew.py" "$SOCK" "$SANDBOX/sew.log" &
FAKE_PID=$!
for _ in $(seq 1 50); do [ -S "$SOCK" ] && break; sleep 0.1; done
[ -S "$SOCK" ] || fail "the fake Sew never listened"

"$BIN" >"$SANDBOX/daemon.log" 2>&1 &
PID=$!
K2DIR=""
for _ in $(seq 1 150); do
    d="$HOME/.k2"
    if [ -s "$d/daemon.port" ] && { [ -s "$d/daemon.token" ] || [ -s "$d/heartbeat.token" ]; }; then K2DIR="$d"; break; fi
    sleep 0.2
done
[ -n "$K2DIR" ] || fail "the daemon never wrote its port and token"
PORT="$(tr -d '[:space:]' < "$K2DIR/daemon.port")"
TOKEN="$(cat "$K2DIR/daemon.token" 2>/dev/null || cat "$K2DIR/heartbeat.token")"
BASE="http://127.0.0.1:$PORT"
for _ in $(seq 1 150); do
    if curl -s "$BASE/boot-status" | grep -q '"ready"'; then break; fi
    sleep 0.2
done
curl -s "$BASE/boot-status" | grep -q '"ready"' || fail "the daemon never became ready"
curl -s "$BASE/boot-status?token=$TOKEN" | grep -q 'app-recordings-v1' || fail "boot-status lacks app-recordings-v1"
pass "headless daemon ready on $PORT with app-recordings-v1"

STATUS=""
BODY=""
call() {
    local method="$1" path="$2" body="${3:-}" out
    if [ "$method" = GET ]; then
        out="$(curl -s -G "$BASE$path" --data-urlencode "token=$TOKEN" ${body:+--data "$body"} -w $'\n%{http_code}')"
    else
        out="$(printf '%s' "$body" | curl -s -X POST "$BASE$path?token=$TOKEN" -H 'Content-Type: application/json' --data-binary @- -w $'\n%{http_code}')"
    fi
    STATUS="${out##*$'\n'}"
    BODY="${out%$'\n'*}"
}
field() { printf '%s' "$BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = v[int(k)] if isinstance(v, list) else (v or {}).get(k)
print("" if v is None else (json.dumps(v) if isinstance(v, (dict, list, bool)) else v))' "$1"; }
expect() { [ "$STATUS" = "$1" ] || fail "$2: HTTP $STATUS (want $1): $BODY"; pass "$2"; }
code_is() { [ "$(field error.code)" = "$1" ] || fail "$2: code $(field error.code) (want $1): $BODY"; pass "$2"; }

# Before the target: no Sew flags (Sew 0.8.x-like): inert.
call GET /cli/sew/recording/policies
expect 200 "policies read before any sweep"
[ "$(field flags.appRecording)" = false ] || fail "no flags before a sweep: $BODY"

call POST /cli/skin/grant-hooks/target "{\"kind\":\"chat\",\"enabled\":true,\"socket\":\"$SOCK\",\"tokenFile\":\"$TOKFILE\"}"
expect 200 "the chat target points at the fake Sew"
for _ in $(seq 1 100); do
    call GET /cli/sew/recording/policies
    [ "$(field flags.appRecording)" = true ] && break
    sleep 0.3
done
[ "$(field flags.appRecording)" = true ] || fail "the sweep never noted Sew 0.9.0's flags: $BODY"
[ "$(field flags.mgmtRecordingReads)" = true ] || fail "mgmt_recording_reads: $BODY"
pass "the sweep noted Sew 0.9.0's recording flags"
for _ in $(seq 1 50); do
    call GET /cli/sew/recording/policies
    printf '%s' "$BODY" | grep -q '"seen":"off"' && break
    sleep 0.3
done
printf '%s' "$BODY" | grep -q '"key":"recording.required"' || fail "recording.required listed: $BODY"
printf '%s' "$BODY" | grep -q '"seen":"off"' || fail "the org policy Sew shows was read: $BODY"
pass "the org recording policies Sew shows are read"

# Each method on its own path.
for p in /cli/app/recording/set /cli/sew/app-spaces/recording /cli/sew/app-spaces/recording-lock /cli/sew/recording/policy /cli/voice/recording/settings; do
    call GET "$p"
    expect 405 "GET $p is 405"
done
call POST /cli/app/recording '{"app":"x"}'
expect 405 "POST /cli/app/recording is 405"

# Person-only steps without a confirm: refused, nothing changes.
call POST /cli/sew/app-spaces/recording-lock '{"spaceId":"5101","lock":true}'
expect 403 "the lock without a confirm"
code_is owner_confirm_required "the lock needs the owner's fresh confirm"
call POST /cli/sew/recording/policy '{"changes":{"recording.required":"multi_person"}}'
expect 403 "a policy without a confirm"
code_is owner_confirm_required "a policy needs a fresh confirm"
call POST /cli/sew/recording/policy '{"changes":{"recording.required":"always"}}'
expect 400 "a bad policy value is refused before any confirm"
call GET /cli/sew/recording/policies
[ "$(field locks)" = "[]" ] || fail "no lock was stored: $BODY"

# The K5 switch waits for the desktop consent sheet.
call POST /cli/voice/recording/settings '{"required":true}'
expect 409 "the K5 require-recording switch"
code_is client_consent_missing "K5 required waits for K2's desktop consent sheet"

# An App's settings: an App that isn't here; its reads take an App pass.
call GET /cli/app/recording "app=nope"
[ "$STATUS" = 404 ] || [ "$STATUS" = 400 ] || fail "an unknown App: HTTP $STATUS: $BODY"
pass "an unknown App is refused ($STATUS)"
for p in /cli/sew/app/recordings /cli/sew/app/recording /cli/sew/app/recording/transcript /cli/sew/app/recordings/events; do
    call GET "$p" "id=71"
    expect 403 "GET $p with the owner token"
    code_is app_pass_required "$p takes the App's own pass"
done

# Nothing about recording reached Sew but the reads K2 does each sweep.
grep -q 'GET /mgmt/v1/policies' "$SANDBOX/sew.log" || fail "the sweep never read the policies"
if grep -E '^POST /mgmt/v1/(policies|spaces/.*/settings)' "$SANDBOX/sew.log" | grep -q 'recording'; then
    fail "K2 sent a recording write nobody asked for: $(grep recording "$SANDBOX/sew.log")"
fi
pass "no recording write reached Sew unasked"
echo "ALL PASSED"
