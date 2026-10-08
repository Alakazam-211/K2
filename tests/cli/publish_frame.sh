#!/usr/bin/env bash
# k2 publish frame + framing headers (prd-app-frame-ancestors-v1 §5 test 8).
#
#   1. No daemon: help lists the verb; usage errors exit 2 before any request.
#   2. Stub daemon: an owner_only refusal exits 3; a bad_frame_origin
#      refusal exits 2 (the CLI's exit-code mapping for this verb).
#   3. Real headless daemon (temp HOME): run a --skin App, ps shows FRAME,
#      --allow / --none / --reset change the live helper's headers, bad
#      origins exit 2, GET on the route is 405, the daemon's own responses
#      carry frame-ancestors 'self'.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

WORK="$(mktemp -d -t k2-publish-frame-XXXXXX)"
STUB_PID=""
DAEMON_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    [ -n "${GPORT:-}" ] && pkill -f -- "--listen 127.0.0.1:${GPORT}" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN || true

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }

# ── 1. no daemon ─────────────────────────────────────────────────────
echo "== help + usage (no daemon) =="
HOME1="$WORK/home1"
mkdir -p "$HOME1"
nod() { env HOME="$HOME1" K2_PORT=1 K2_HOOK_TOKEN=test-token K2_PROJECT_PATH="$HOME1" "$K2_CLI" "$@"; }
set +e
out="$(nod publish --help 2>&1)"; rc=$?
set -e
assert_eq "publish --help exit 0" "$rc" "0"
assert_contains "help: frame usage" "$out" "k2 publish frame <name> (--allow <origin> ... | --none | --reset) [--json]"
assert_contains "help: frame section" "$out" "--allow https://your.site"
for args in "frame" "frame agents" "frame agents --none --reset" "frame agents --allow" "frame agents --bogus" "frame agents --none extra"; do
    set +e
    # shellcheck disable=SC2086
    out="$(nod publish $args 2>&1)"; rc=$?
    set -e
    assert_eq "'publish $args' exits 2" "$rc" "2"
done

# ── 2. stub daemon: exit-code mapping ────────────────────────────────
echo "== stub daemon: owner_only exit 3, bad origin exit 2 =="
SPORT="$(free_port)"
cat >"$WORK/stub.py" <<'PY'
import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        if body.get("allow") == "https://owner.only":
            code, out = 403, {"ok": False, "error": {"code": "owner_only", "hint": "owner only"}}
        else:
            code, out = 400, {"ok": False, "error": {"code": "bad_frame_origin", "hint": "bad origin"}}
        data = json.dumps(out).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
python3 "$WORK/stub.py" "$SPORT" &
STUB_PID=$!
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$SPORT/" >/dev/null 2>&1 && break; sleep 0.05; done
stub() { env HOME="$HOME1" K2_HOST=127.0.0.1 K2_PORT="$SPORT" K2_HOOK_TOKEN=stub-token K2_PROJECT_PATH="$HOME1" "$K2_CLI" "$@"; }
set +e
out="$(stub publish frame agents --allow https://owner.only 2>&1)"; rc=$?
set -e
assert_eq "owner_only exits 3" "$rc" "3"
assert_contains "owner_only hint" "$out" "owner only"
set +e
out="$(stub publish frame agents --allow https://x.example 2>&1)"; rc=$?
set -e
assert_eq "bad_frame_origin exits 2" "$rc" "2"
kill "$STUB_PID" 2>/dev/null || true
STUB_PID=""

# ── 3. real headless daemon ──────────────────────────────────────────
echo "== real daemon: run --skin, frame allow/none/reset =="
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || { echo "FAIL: k2-daemon not built (cargo build -p k2-daemon)" >&2; exit 1; }

SB="$WORK/sb"
mkdir -p "$SB/.k2" "$SB/shim"
printf '#!/bin/sh\nexec /bin/cat\n' > "$SB/shim/claude"
chmod +x "$SB/shim/claude"
HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    K2_PUBLISH_HELPER_EXIT_WITH_PARENT=1 \
    "$DAEMON_BIN" >"$SB/daemon.log" 2>&1 &
DAEMON_PID=$!
disown "$DAEMON_PID" 2>/dev/null || true
for _ in $(seq 1 200); do
    [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ] && break
    sleep 0.1
done
[ -s "$SB/.k2/daemon.port" ] || { echo "FAIL: daemon never wrote daemon.port" >&2; tail -30 "$SB/daemon.log" >&2; exit 1; }
DPORT="$(cat "$SB/.k2/daemon.port")"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || { echo "FAIL: daemon never reached ready" >&2; exit 1; }

hdr() { curl -s -D - -o /dev/null "$@" | tr -d '\r'; }
boot="$(hdr "http://127.0.0.1:$DPORT/boot-status")"
assert_contains "daemon CSP 'self'" "$boot" "Content-Security-Policy: frame-ancestors 'self'"
assert_contains "daemon XFO" "$boot" "X-Frame-Options: SAMEORIGIN"

WSP="$SB/framews"
resp="$(curl -s -X POST "http://127.0.0.1:$DPORT/cli/workspace/create?token=$DTOKEN&path=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$WSP")" --data-raw '')"
assert_contains "workspace registered" "$resp" "framews"

real() {
    env HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" K2_HOOK_TOKEN="$DTOKEN" \
        K2_PROJECT_PATH="$WSP" "$K2_CLI" "$@"
}
GPORT="$(free_port)"
set +e
out="$(real publish run agents --skin --port "$GPORT" --no-tunnel 2>&1)"; rc=$?
set -e
assert_eq "run --skin exit 0" "$rc" "0"
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$GPORT/login" >/dev/null 2>&1 && break; sleep 0.1; done

APP="frame-ancestors 'self' tauri://localhost http://tauri.localhost"
login="$(hdr "http://127.0.0.1:$GPORT/login")"
assert_contains "App default CSP" "$login" "Content-Security-Policy: $APP"
assert_contains "App default XFO" "$login" "X-Frame-Options: SAMEORIGIN"
err404="$(hdr "http://127.0.0.1:$GPORT/nope")"
assert_contains "App 404 framed" "$err404" "Content-Security-Policy: $APP"
ps_out="$(real publish ps)"
assert_contains "ps FRAME column" "$ps_out" "FRAME"
assert_contains "ps default" "$ps_out" "default"

set +e
out="$(real publish frame agents --allow https://partner.example 2>&1)"; rc=$?
set -e
assert_eq "frame --allow exit 0" "$rc" "0"
assert_contains "frame --allow output" "$out" "frame-ancestors 'self' tauri://localhost http://tauri.localhost https://partner.example"
assert_contains "frame --allow restarted" "$out" "restarted"
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$GPORT/login" >/dev/null 2>&1 && break; sleep 0.1; done
login="$(hdr "http://127.0.0.1:$GPORT/login")"
assert_contains "allow-listed CSP live" "$login" "Content-Security-Policy: $APP https://partner.example"
assert_contains "ps shows extras" "$(real publish ps)" "default + https://partner.example"

for badorigin in '*' 'https://*.k2.dev' 'http://partner.example' 'https://a.example/x'; do
    set +e
    out="$(real publish frame agents --allow "$badorigin" 2>&1)"; rc=$?
    set -e
    assert_eq "bad origin $badorigin exits 2" "$rc" "2"
done

set +e
out="$(real publish frame agents --none --json 2>/dev/null)"; rc=$?
set -e
assert_eq "frame --none exit 0" "$rc" "0"
printf '%s' "$out" | python3 -c '
import json, sys
v = json.load(sys.stdin)
assert v["ok"] is True and v["restarted"] is True, v
assert v["service"]["framePolicy"] == "'"'"'none'"'"'", v
' && ok "--none --json" || bad "--none --json"
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$GPORT/login" >/dev/null 2>&1 && break; sleep 0.1; done
login="$(hdr "http://127.0.0.1:$GPORT/login")"
assert_contains "none CSP" "$login" "Content-Security-Policy: frame-ancestors 'none'"
assert_contains "none XFO" "$login" "X-Frame-Options: DENY"

code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DPORT/cli/publish/frame?token=$DTOKEN&name=agents&project=$WSP&reset=true")"
assert_eq "GET /cli/publish/frame is 405" "$code" "405"

set +e
out="$(real publish frame agents --reset 2>&1)"; rc=$?
set -e
assert_eq "frame --reset exit 0" "$rc" "0"
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$GPORT/login" >/dev/null 2>&1 && break; sleep 0.1; done
login="$(hdr "http://127.0.0.1:$GPORT/login")"
assert_contains "reset CSP" "$login" "Content-Security-Policy: $APP"
real publish rm agents >/dev/null 2>&1 || true

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
