#!/usr/bin/env bash
# k2-ptyd S5 (PRD prd-k2-ptyd-v1 P45, P47, P49): an agent's `k2` call made
# while the daemon restarts completes instead of failing, and a request
# that reached a handler is never sent twice.
#
# A python stub daemon answers from a per-path plan. Scenarios (each run
# through the real `cli/k2` against the stub):
#   1. refused, then up: nothing listens for ~1.5 s, then the stub binds
#      → exit 0, the request arrives exactly once;
#   2. two migrating 503s (the daemon's pre-ready answer), then 200
#      → exit 0, three arrivals;
#   3. a POST answered 500 → sent exactly once (never retried);
#   4. K2_PORT refused while ~/.k2/daemon.port names the live port (the
#      daemon came back on another port) → exit 0 on the new port;
#   5. nothing ever comes back → gives up within the retry bound;
#   6. migrating forever → at most 4 tries, then the 503 is returned.
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-restart-retry-XXXXXX)"
STUBS=()
cleanup() {
    local p
    for p in "${STUBS[@]+"${STUBS[@]}"}"; do kill "$p" 2>/dev/null || true; done
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-restart-retry-token"
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
echo "$OWNER" >"$HOME/.k2/daemon.token"
chmod 600 "$HOME/.k2/heartbeat.token" "$HOME/.k2/daemon.token"

cat >"$WORK/stub.py" <<'PYEOF'
import json, os, socketserver, sys, time, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, port, delay = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
lock = threading.Lock()

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        if length:
            self.rfile.read(length)
        path = urllib.parse.urlparse(self.path).path
        with lock:
            with open(os.path.join(work, "requests.log"), "a") as f:
                f.write(json.dumps({"method": method, "path": path}) + "\n")
            try:
                plan = json.load(open(os.path.join(work, "plan.json")))
            except Exception:
                plan = {}
            cpath = os.path.join(work, "counts.json")
            try:
                counts = json.load(open(cpath))
            except Exception:
                counts = {}
            n = counts.get(path, 0)
            counts[path] = n + 1
            json.dump(counts, open(cpath, "w"))
        steps = plan.get(path, [])
        # ["repeat", ""] as the last step repeats the step before it forever.
        if steps and steps[-1][0] == "repeat" and n >= len(steps) - 1:
            status, body = steps[-2]
        elif n < len(steps):
            status, body = steps[n]
        else:
            status, body = 200, json.dumps({"ok": True, "success": True, "heartbeats": [],
                                            "message": {"id": "m1", "subject": "back"}})
        if status == "drop":
            # A restart cut the connection: closed with no answer at all.
            self.close_connection = True
            return
        data = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self): self._go("GET")
    def do_POST(self): self._go("POST")
    def log_message(self, *a): pass

class TcpHTTP(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True
    allow_reuse_address = True

time.sleep(delay)
srv = TcpHTTP(("127.0.0.1", port), H)
open(os.path.join(work, "bound.%d" % port), "w").close()
srv.serve_forever()
PYEOF

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }
# start_stub <port> <delay-seconds>
start_stub() {
    python3 "$WORK/stub.py" "$WORK" "$1" "$2" &
    STUBS+=("$!")
}
wait_bound() { local i; for i in $(seq 1 100); do [ -e "$WORK/bound.$1" ] && return 0; sleep 0.05; done; echo "FAIL: stub on $1 never bound" >&2; exit 1; }
plan() { printf '%s' "$1" >"$WORK/plan.json"; rm -f "$WORK/counts.json"; : >"$WORK/requests.log"; }
arrivals() { grep -c "\"path\": \"$1\"" "$WORK/requests.log" || true; }

clean_env() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u K2_HOOK_TOKEN \
        -u K2_API_CELL -u K2SO_API_CELL -u K2_CELL -u K2_HOST -u K2_REMOTE_TOKEN \
        -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX -u K2_AIRGAP \
        K2_PROJECT_PATH="$WORK/proj" "$@"
}
k2_on() { local port="$1"; shift; clean_env K2_PORT="$port" "$K2_CLI" "$@"; }

MIGRATING='[503, "{\"state\":\"migrating\",\"error\":\"daemon is completing first-boot migrations\"}"]'

# ── 1. refused, then up ───────────────────────────────────────────────
echo "== refused, then the daemon is back =="
P1="$(free_port)"
plan '{}'
start_stub "$P1" 1.5
echo "$P1" >"$HOME/.k2/daemon.port"
rc=0; out="$(k2_on "$P1" heartbeat list --json 2>&1)" || rc=$?
if [ "$rc" -eq 0 ]; then ok "a refused call completes once the daemon is back (exit 0)"; else bad "refused then up: exit $rc: $out"; fi
n="$(arrivals /cli/heartbeat/list)"
if [ "$n" = 1 ]; then ok "the request arrived exactly once"; else bad "the request arrived $n times"; fi

# ── 2. migrating 503s, then ready ─────────────────────────────────────
echo "== migrating 503s, then ready =="
plan "{\"/cli/heartbeat/list\": [$MIGRATING, $MIGRATING]}"
rc=0; out="$(k2_on "$P1" heartbeat list --json 2>&1)" || rc=$?
if [ "$rc" -eq 0 ] && [[ "$out" != *migrating* ]]; then ok "two migrating 503s are retried to a 200"; else bad "migrating: exit $rc: $out"; fi
n="$(arrivals /cli/heartbeat/list)"
if [ "$n" = 3 ]; then ok "three arrivals (two 503s + the 200)"; else bad "expected 3 arrivals, got $n"; fi

# ── 3. a POST that got a 500 is never sent again ──────────────────────
echo "== POST answered 500 =="
plan '{"/cli/review/approve": [[500, "{\"error\":\"boom\"}"], [500, "{\"error\":\"boom\"}"]]}'
k2_on "$P1" review approve agentx branchx >/dev/null 2>&1 || true
n="$(arrivals /cli/review/approve)"
if [ "$n" = 1 ]; then ok "a POST answered 500 is sent exactly once"; else bad "a POST answered 500 was sent $n times"; fi

# ── 4. the daemon came back on another port ───────────────────────────
echo "== K2_PORT refused, daemon.port has moved =="
DEAD="$(free_port)"
plan '{}'
echo "$P1" >"$HOME/.k2/daemon.port"
rc=0; out="$(k2_on "$DEAD" heartbeat list --json 2>&1)" || rc=$?
if [ "$rc" -eq 0 ]; then ok "a refused K2_PORT falls back to ~/.k2/daemon.port (exit 0)"; else bad "port fallback: exit $rc: $out"; fi
n="$(arrivals /cli/heartbeat/list)"
if [ "$n" = 1 ]; then ok "the request arrived once, on the published port"; else bad "port fallback: $n arrivals"; fi

# ── 5. nothing comes back: bounded ────────────────────────────────────
echo "== the daemon never comes back =="
echo "$DEAD" >"$HOME/.k2/daemon.port"
t0=$(date +%s)
rc=0; k2_on "$DEAD" heartbeat list --json >/dev/null 2>&1 || rc=$?
t1=$(date +%s)
if [ "$rc" -ne 0 ]; then ok "an unreachable daemon still fails (exit $rc)"; else bad "an unreachable daemon returned exit 0"; fi
if [ $((t1 - t0)) -le 8 ]; then ok "it gives up within the retry bound ($((t1 - t0)) s)"; else bad "it took $((t1 - t0)) s to give up"; fi

# ── 6. migrating forever: bounded ─────────────────────────────────────
echo "== migrating forever =="
echo "$P1" >"$HOME/.k2/daemon.port"
plan "{\"/cli/heartbeat/list\": [$MIGRATING, [\"repeat\", \"\"]]}"
k2_on "$P1" heartbeat list --json >/dev/null 2>&1 || true
n="$(arrivals /cli/heartbeat/list)"
if [ "$n" = 4 ]; then ok "at most 4 tries on a daemon that stays migrating"; else bad "expected 4 tries, got $n"; fi

# ── 7. a long-poll cut by a restart is retried once (P48) ─────────────
echo "== mail wait: the connection drops once =="
plan '{"/cli/mail/wait": [["drop", ""]]}'
rc=0; out="$(k2_on "$P1" mail wait --json 2>&1)" || rc=$?
if [ "$rc" -eq 0 ] && [[ "$out" == *back* ]]; then ok "a dropped long-poll is retried and completes"; else bad "dropped long-poll: exit $rc: $out"; fi
n="$(arrivals /cli/mail/wait)"
if [ "$n" = 2 ]; then ok "the long-poll was sent twice (one retry)"; else bad "expected 2 long-poll sends, got $n"; fi
plan '{"/cli/mail/wait": [["drop", ""], ["drop", ""]]}'
rc=0; k2_on "$P1" mail wait --json >/dev/null 2>&1 || rc=$?
n="$(arrivals /cli/mail/wait)"
if [ "$rc" -ne 0 ] && [ "$n" = 2 ]; then ok "a long-poll is retried only once (exit $rc)"; else bad "two drops: exit $rc, $n sends"; fi

echo
echo "restart_window_retry: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
