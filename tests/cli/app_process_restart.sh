#!/usr/bin/env bash
# Unwalled ("process") Apps across a daemon restart (smoke r6, 0.46.3).
#
# A sandbox daemon (temp HOME, K2_APPS=1 + K2_APPS_PROCESS=1, air-gapped)
# and this checkout's cli/k2. The restart is SIMULATED: the daemon is
# SIGKILLed and a fresh one starts on the same HOME. On a Linux box under
# systemd the App survives the same way only with K2's KillMode=process
# drop-in; that part is not exercised here (no systemd in a sandbox), and
# the script says so instead of skipping silently.
#
# The App prints a line per request and dies on a broken stdout (Node's
# default for EPIPE), so a pipe into a dead daemon shows at once.
#
#   A. Survivor: while the daemon is down the App still answers on its own
#      port and logs; the new daemon adopts it (same deployment, same pid),
#      checks it, and nothing is recorded as a crash.
#   B. A survivor whose logs went through a pipe (an older daemon's run
#      file): relaunched within seconds as a fresh deployment, the old pid
#      stopped, not a crash.
#   C. No survivor and the old daemon had marked it `restarting`: the new
#      daemon starts it within seconds, not a crash.
#
# Build first:  cargo build -p k2-daemon
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

echo "NOTE: systemd's KillMode=process is not exercised here: the restart is a SIGKILL of a sandbox daemon."

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_ne() { if [ "$2" != "$3" ]; then ok "$1"; else bad "$1 (both $(printf %q "$2"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2rst-XXXXXX)"
DHOME="$SANDBOX/home"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    for rf in "$DHOME"/.k2/apps/*/run/*.json; do
        [ -f "$rf" ] || continue
        pid="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$rf" 2>/dev/null || true)"
        [ -n "$pid" ] && kill -9 -- "-$pid" 2>/dev/null || true
    done
    for p in ${EXTRA_PIDS:-}; do kill -9 -- "-$p" 2>/dev/null || true; done
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -40 "$SANDBOX"/daemon*.log >&2 || true
    fi
    chmod -R u+w "$SANDBOX" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT
EXTRA_PIDS=""

mkdir -p "$DHOME" "$SANDBOX/tmp"
N=0
start_daemon() {
    N=$((N + 1))
    rm -f "$DHOME/.k2/heartbeat.port"
    HOME="$DHOME" TMPDIR="$SANDBOX/tmp" K2_APPS=1 K2_APPS_PROCESS=1 K2_AIRGAP=1 K2_APPS_WATCH_SECS=2 \
        K2_APPS_DEPLOY_TIMEOUT_SECS=15 K2_APPS_DRAIN_SECS=1 K2_APPS_STOP_GRACE_SECS=2 \
        "$DAEMON" > "$SANDBOX/daemon$N.log" 2>&1 &
    DAEMON_PID=$!
    for _ in $(seq 1 150); do
        [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
        sleep 0.2
    done
    [ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon $N wrote its port"; exit 1; }
    PORT="$(cat "$DHOME/.k2/heartbeat.port")"
    OWNER_TOKEN="$(cat "$DHOME/.k2/heartbeat.token")"
    for _ in $(seq 1 100); do
        curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
        sleep 0.2
    done
    ok "daemon $N up on $PORT"
}
kill_daemon() {
    kill -9 "$DAEMON_PID"
    wait "$DAEMON_PID" 2>/dev/null || true
    DAEMON_PID=""
}
k2() { HOME="$DHOME" "$K2" "$@"; }
jget() { python3 -c 'import json,sys
v = json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = v[int(k)] if isinstance(v, list) else (v or {}).get(k)
print("" if v is None else v)' "$1"; }
inspect() { k2 app inspect "$1" --json; }
alive() { kill -0 "$1" 2>/dev/null; }
get() { curl -s -m 3 "http://127.0.0.1:$1$2" || true; }
failures() { k2 app errors "$1" --since 1h --json | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["failures"]))'; }
wait_for() { # what secs cmd...
    local what="$1" secs="$2"; shift 2
    local end=$((SECONDS + secs))
    while [ "$SECONDS" -lt "$end" ]; do
        if "$@" >/dev/null 2>&1; then ok "$what"; return 0; fi
        sleep 0.3
    done
    bad "$what (not within ${secs}s)"
    return 1
}

start_daemon

echo "== workspace and App =="
WS="$SANDBOX/ws"
mkdir -p "$WS/v1"
git -C "$WS" -c init.defaultBranch=main init -q
git -C "$WS" -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q --allow-empty -m one
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "http://127.0.0.1:$PORT/cli/projects/add-from-path?token=$OWNER_TOKEN")"
expect_has "workspace registered" "$reg" "\"path\":\"$WS\""
cd "$WS"
export K2_PROJECT_PATH="$WS"
cat > "$WS/v1/manifest.json" <<'EOF'
{"schema":1,"kind":"app","id":"rst","name":"Restart","version":"1.0.0",
 "host":{"mode":"cmd","start":["python3","server.py"],"health":{"path":"/healthz"},"drainSecs":1}}
EOF
cat > "$WS/v1/server.py" <<'EOF'
import http.server, os, sys
def say(line):
    # Like Node on EPIPE: a broken stdout kills the App.
    try:
        print(line, flush=True)
    except BrokenPipeError:
        os._exit(3)
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        say("req %s" % self.path)
        b = ("ok %s\n" % os.getpid()).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
http.server.ThreadingHTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
EOF
expect_has "installed" "$(k2 app install rst --memory none --engine process)" "App rst installed"
expect_has "deployed" "$(k2 app deploy rst "$WS/v1")" "live on port"
FRONT="$(inspect rst | jget row.frontPort)"
DEP1="$(inspect rst | jget runtime.live.deployment)"
PID1="$(inspect rst | jget runtime.live.pid)"
APORT1="$(inspect rst | jget runtime.live.port)"
expect_has "front answers" "$(get "$FRONT" /hello)" "ok $PID1"
RUNF="$DHOME/.k2/apps/rst/run/$DEP1.json"
expect_eq "run file: the App holds its log (file sink)" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("log"))' "$RUNF")" "file"

echo "== A. the App survives a daemon restart and is adopted =="
kill_daemon
alive "$PID1" && ok "the App outlived the daemon" || bad "the App outlived the daemon"
out="$(get "$APORT1" /while-down)"
expect_has "it answers with the daemon gone (no dead pipe)" "$out" "ok $PID1"
LOG1="$DHOME/.k2/apps/rst/logs/$DEP1.log"
expect_has "and logs into its own file" "$(cat "$LOG1")" "req /while-down"
start_daemon
wait_for "adopted: the front answers again" 15 sh -c "curl -s -m 2 http://127.0.0.1:$FRONT/after | grep -q 'ok $PID1'"
expect_eq "same deployment" "$(inspect rst | jget runtime.live.deployment)" "$DEP1"
expect_eq "same pid" "$(inspect rst | jget runtime.live.pid)" "$PID1"
expect_eq "state running" "$(inspect rst | jget row.state)" "running"
sleep 3   # past the adopt check (probed every second)
expect_eq "still the same instance after its checks" "$(inspect rst | jget runtime.live.pid)" "$PID1"
expect_eq "no crash recorded" "$(failures rst)" "0"
expect_has "the new daemon's requests reach the log" "$(cat "$LOG1")" "req /after"

echo "== B. a survivor that logged through a pipe is relaunched =="
kill_daemon
python3 - "$RUNF" <<'EOF'
import json, sys
p = sys.argv[1]
v = json.load(open(p))
v.pop("log", None)   # an older daemon's run file: stdout was a pipe into it
open(p, "w").write(json.dumps(v))
EOF
start_daemon
wait_for "a fresh deployment is live within seconds" 20 sh -c "[ \"\$(HOME='$DHOME' '$K2' app inspect rst --json | python3 -c 'import json,sys; print(json.load(sys.stdin)[\"runtime\"][\"live\"][\"deployment\"])')\" != '$DEP1' ]"
DEP2="$(inspect rst | jget runtime.live.deployment)"
PID2="$(inspect rst | jget runtime.live.pid)"
expect_ne "new deployment" "$DEP2" "$DEP1"
expect_has "the front serves the new instance" "$(get "$FRONT" /b)" "ok $PID2"
wait_for "the old instance is stopped" 15 sh -c "! kill -0 $PID1 2>/dev/null"
expect_has "the restart says why" "$(inspect rst | jget lastDeployment.reason)" "log pipe went with the old K2 daemon"
expect_eq "no crash recorded" "$(failures rst)" "0"
expect_eq "state running" "$(inspect rst | jget row.state)" "running"

echo "== C. no survivor, left at restarting: started within seconds =="
kill_daemon
kill -9 -- "-$PID2" 2>/dev/null || kill -9 "$PID2"
DB="$DHOME/.k2/k2.db"
[ -f "$DB" ] || DB="$DHOME/.k2/k2so.db"
python3 - "$DB" <<'EOF'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute("UPDATE apps SET state = 'restarting', state_reason = 'exited' WHERE app_id = 'rst'")
c.commit()
EOF
t0=$SECONDS
start_daemon
wait_for "started again within seconds" 20 sh -c "curl -s -m 2 http://127.0.0.1:$FRONT/c | grep -q '^ok '"
echo "  (took $((SECONDS - t0)) s from the daemon start)"
PID3="$(inspect rst | jget runtime.live.pid)"
expect_ne "a new process" "$PID3" "$PID2"
expect_has "the restart says why" "$(inspect rst | jget lastDeployment.reason)" "the K2 daemon restarted"
expect_eq "state running" "$(inspect rst | jget row.state)" "running"
expect_eq "no crash recorded" "$(failures rst)" "0"

expect_has "uninstall" "$(k2 app uninstall rst)" "App rst uninstalled"

echo
echo "app_process_restart: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
