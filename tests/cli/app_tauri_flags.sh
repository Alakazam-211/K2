#!/usr/bin/env bash
# 0.46.8, the Tauri-first App fixes: the CLI's own checks and what it sends.
# Hermetic: a throwaway HOME, every K2_* unset, and a tiny stub on loopback
# that answers each request with a fixed JSON and logs the body. The daemon
# side is covered by k2-core's app_templates tests and k2-daemon's route tests.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
SANDBOX="$(mktemp -d /tmp/k2tauri-XXXXXX)"
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2" "$SANDBOX/repo/src-tauri"
printf '{}\n' >"$SANDBOX/repo/src-tauri/tauri.conf.json"

LOG="$SANDBOX/requests.log"
ANSWER="$SANDBOX/answer.json"
printf '{"ok":true}' >"$ANSWER"
cat >"$SANDBOX/stub.py" <<'PY'
import http.server, sys
log, answer = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def _go(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        with open(log, "a") as f:
            f.write("%s %s %s\n" % (self.command, self.path.split("?")[0], body))
        out = open(answer, "rb").read()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    do_GET = do_POST = _go
    def log_message(self, *a):
        pass
s = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
PORT_FILE="$SANDBOX/port"
python3 "$SANDBOX/stub.py" "$LOG" "$ANSWER" >"$PORT_FILE" &
STUB=$!
trap 'kill "$STUB" 2>/dev/null || true; rm -rf "$SANDBOX"' EXIT
for _ in $(seq 1 50); do [ -s "$PORT_FILE" ] && break; sleep 0.1; done
[ -s "$PORT_FILE" ] || { echo "FAIL: the stub never started" >&2; exit 1; }
tr -d '\n' <"$PORT_FILE" >"$HOME/.k2/heartbeat.port"
printf 'offline-test-token' >"$HOME/.k2/heartbeat.token"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
run() { set +e; OUT="$(bash "$K2" "$@" 2>&1)"; RC=$?; set -e; }
expect() {
    local label="$1" rc="$2" needle="$3"; shift 3
    run "$@"
    if [ "$RC" = "$rc" ] && printf '%s' "$OUT" | grep -Fq -- "$needle"; then ok "$label"; else bad "$label (exit $RC, wanted $rc and '$needle'): $OUT"; fi
}
sent() { # the last request's line
    tail -1 "$LOG"
}
sent_has() {
    local label="$1" needle="$2"
    if sent | grep -Fq -- "$needle"; then ok "$label"; else bad "$label: wanted '$needle' in: $(sent)"; fi
}
nothing_sent() {
    local label="$1"
    if [ -s "$LOG" ]; then bad "$label: a call was made: $(cat "$LOG")"; else ok "$label"; fi
}

echo "== k2 app init: usage, no call =="
expect "help names --app" 0 "--app <app>" app init --help
expect "help names --voice" 0 "--voice" app init --help
expect "--app with static" 2 "go with --template tauri" app init --template static --app jobs "$SANDBOX/repo"
expect "--voice with node-api" 2 "go with --template tauri" app init --template node-api --voice "$SANDBOX/repo"
expect "--app with --name" 2 "--app or --name, not both" app init --template tauri --app jobs --name x "$SANDBOX/repo"
nothing_sent "none of those reached the daemon"

echo "== k2 app init: what it sends =="
printf '{"ok":true,"template":"tauri","dryRun":true,"files":[{"path":"src-tauri/Info.plist","action":"write","note":"new"},{"path":"a/AndroidManifest.xml","action":"snippet","note":"add","content":"<uses-permission />"}],"next":["k2 app updates token jobs --file <file>"],"notes":["a note"]}' >"$ANSWER"
expect "--app --voice --dry-run prints the plan" 0 "add by hand" app init --template tauri --app jobs --voice --dry-run "$SANDBOX/repo"
printf '%s' "$OUT" | grep -Fq "<uses-permission />" && ok "a snippet is shown" || bad "a snippet is shown: $OUT"
printf '%s' "$OUT" | grep -Fq "k2 app updates token jobs --file" && ok "Next: is shown" || bad "Next: is shown: $OUT"
sent_has "POST /cli/app/init" "POST /cli/app/init"
sent_has "the App goes" '"app": "jobs"'
sent_has "voice goes" '"voice": true'
sent_has "dry run goes" '"dryRun": true'
: >"$LOG"
run app init --template tauri "$SANDBOX/repo"
if sent | grep -q '"app"\|"voice"'; then bad "a plain run names neither: $(sent)"; else ok "a plain run names neither"; fi

echo "== k2 app login settings --max-devices =="
: >"$LOG"
expect "help is its own page" 0 "usage: k2 app login settings <app> [flags]" app login settings --help
expect "help names it" 0 "--max-devices <n>" app login settings --help
expect "k2 app login --help too" 0 "--max-devices <n>" app login --help
expect "not a number" 2 "whole number from 1 to 20" app login settings jobs --max-devices five
expect "too long to be one" 2 "whole number from 1 to 20" app login settings jobs --max-devices 100
nothing_sent "none of those reached the daemon"
printf '{"ok":true,"app":"jobs","maxDevices":5,"settings":{"inviteTtlSecs":259200,"resetTtlSecs":86400,"inviteTtlSet":false,"resetTtlSet":false}}' >"$ANSWER"
expect "shows the cap" 0 "devices per person: 5" app login settings jobs --max-devices 5
sent_has "sends a number" '"maxDevices": 5'
printf '{"ok":true,"app":"jobs","maxDevices":20,"settings":{"inviteTtlSecs":259200,"resetTtlSecs":86400,"inviteTtlSet":false,"resetTtlSet":false}}' >"$ANSWER"
expect "the default says so" 0 "devices per person: 20 (default)" app login settings jobs --default maxDevices
sent_has "--default clears it" '"maxDevices": null'

echo "== k2 app push set --register =="
: >"$LOG"
expect "help is its own page" 0 "k2 app push — notifications" app push --help
expect "help names it" 0 "--register server" app push --help
expect "k2 app pairing --help is its own page" 0 "k2 app pairing" app pairing --help
expect "a bad door" 2 "--register server|any" app push set jobs --register phone
nothing_sent "not sent"
printf '{"ok":true,"app":"svc","name":"jobs","state":"on","serverOnly":true,"devices":{"total":0,"guests":0,"apns":0,"fcm":0}}' >"$ANSWER"
expect "server" 0 "only through this App server" app push set jobs --register server
sent_has "sends serverOnly true" '"serverOnly": true'
expect "any" 0 "" app push set jobs --register any
sent_has "sends serverOnly false" '"serverOnly": false'

echo "== k2 hostchat home names the flag =="
printf '{"ok":true,"apps":[],"sew":{"appOrigin":true,"appOrigins":false}}' >"$ANSWER"
expect "every App origin no" 0 "every App origin no" hostchat home

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" = 0 ]
