#!/usr/bin/env bash
# `k2 publish run --probe-timeout <dur>` (found 2026-10-16: Hearth builds for
# minutes before it binds). The CLI validates the flag (90s, 10m, 1500ms,
# bare seconds or `default`; > 0, at most 15m) before any request, sends it
# as `probeTimeout`, and shows a service inside its grace as
# "starting (waiting for port N, 4m12s left)" in run, ps and status. A stub
# daemon records the bodies it got.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN || true

WORK="$(mktemp -d -t k2-publish-grace-XXXXXX)"
STUB_PID=""
trap '[ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null; rm -rf "$WORK"' EXIT
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
cat >"$WORK/stub.py" <<'PY2'
import http.server, json, sys
LOG = sys.argv[2]
def svc(name, pending):
    return {"id": name + "-id", "projectId": "p", "name": name, "cmd": "sh publish.sh", "cwd": ".",
            "port": 3047, "expose": "tunnel" if pending else "local", "desired": "running",
            "status": "starting", "pid": 7, "url": None, "target": "127.0.0.1:3047", "error": None,
            "lastExitCode": None, "kind": "cmd", "skinRoot": "", "frameAncestors": "",
            "framePolicy": None, "frameWarning": None, "envDeprecation": None,
            "envMode": "clean", "envRemovedIn": None,
            "probeTimeoutMs": 600000, "probeTimeoutEffectiveMs": 600000,
            "startingLeftMs": 252400, "hostnamePending": pending}
class H(http.server.BaseHTTPRequestHandler):
    def reply(self, out):
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        self.reply({"services": [svc("hearth", False)]})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) or b"{}"
        with open(LOG, "a") as f:
            f.write(self.path + " " + raw.decode() + "\n")
        b = json.loads(raw)
        name = b.get("name") or "hearth"
        self.reply(svc(name, name == "tun"))
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY2
: >"$WORK/posts"
python3 "$WORK/stub.py" "$PORT" "$WORK/posts" &
STUB_PID=$!
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break; sleep 0.05; done
k2s() { env HOME="$WORK" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=stub-token K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"; }
last_body() { tail -n 1 "$WORK/posts"; }
posts() { wc -l <"$WORK/posts" | tr -d ' '; }

# Bad values exit 2 before any request.
for bad in 16m 1h 0 0s soon 10x 1.5m "10m junk"; do
	before="$(posts)"
	set +e
	k2s publish run hearth --cmd "sh publish.sh" --port 3047 --no-tunnel --probe-timeout "$bad" >"$WORK/out" 2>"$WORK/err"
	rc=$?
	set -e
	[ "$rc" = 2 ] || fail "--probe-timeout $bad: want exit 2, got $rc ($(cat "$WORK/out" "$WORK/err"))"
	grep -q -- "--probe-timeout" "$WORK/err" || fail "--probe-timeout $bad: error must name the flag: $(cat "$WORK/err")"
	[ "$(posts)" = "$before" ] || fail "--probe-timeout $bad: must not reach the daemon"
done
grep -q "at most 15m" <(k2s publish run hearth --cmd x --port 3047 --probe-timeout 16m 2>&1 || true) \
	|| fail "16m must say the cap"
ok "bad --probe-timeout values exit 2 with no request (cap 15m)"

set +e
k2s publish run hearth --cmd x --port 3047 --probe-timeout >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
[ "$rc" = 2 ] || fail "--probe-timeout without a value: want 2, got $rc"
ok "--probe-timeout needs a value"

k2s publish run hearth --cmd "sh publish.sh" --port 3047 --no-tunnel --probe-timeout 10m >"$WORK/out" 2>"$WORK/err" \
	|| fail "run --probe-timeout 10m: $(cat "$WORK/out" "$WORK/err")"
python3 -c 'import json,sys; b=json.loads(sys.argv[1].split(" ",1)[1]); assert b["probeTimeout"]=="10m", b; assert b["noTunnel"] is True, b' "$(last_body)" \
	|| fail "body must carry probeTimeout=10m: $(last_body)"
grep -Fxq "Started hearth · starting (waiting for port 3047, 4m12s left) · port 3047" "$WORK/out" \
	|| fail "run output wording: $(cat "$WORK/out")"
ok "run sends probeTimeout and prints starting (waiting for port 3047, 4m12s left)"

k2s publish run hearth --cmd "sh publish.sh" --port 3047 --probe-timeout=90s --no-tunnel >/dev/null 2>&1 || fail "run --probe-timeout=90s"
python3 -c 'import json,sys; b=json.loads(sys.argv[1].split(" ",1)[1]); assert b["probeTimeout"]=="90s", b' "$(last_body)" \
	|| fail "--probe-timeout=90s form: $(last_body)"
k2s publish run hearth --cmd "sh publish.sh" --port 3047 --probe-timeout default --no-tunnel >/dev/null 2>&1 || fail "run default"
python3 -c 'import json,sys; b=json.loads(sys.argv[1].split(" ",1)[1]); assert b["probeTimeout"]=="default", b' "$(last_body)" \
	|| fail "default form: $(last_body)"
k2s publish run hearth --cmd "sh publish.sh" --port 3047 --no-tunnel >/dev/null 2>&1 || fail "run without the flag"
python3 -c 'import json,sys; b=json.loads(sys.argv[1].split(" ",1)[1]); assert "probeTimeout" not in b, b' "$(last_body)" \
	|| fail "no flag = no probeTimeout key (keep the stored value): $(last_body)"
ok "--probe-timeout=90s, default, and no flag (key absent)"

k2s publish run tun --cmd "sh publish.sh" --port 3047 --probe-timeout 10m >"$WORK/out" 2>"$WORK/err" || fail "tunnel run: $(cat "$WORK/err")"
grep -Fq "public URL is published once port 3047 answers" "$WORK/out" || fail "tunnel pending line: $(cat "$WORK/out")"
ok "tunnelled run inside its grace says the URL waits for the port"

out="$(k2s publish ps 2>&1)" || fail "ps: $out"
printf '%s\n' "$out" | grep -E '^hearth ' | grep -Fq "starting (waiting for port 3047, 4m12s left)" || fail "ps row: $out"
ok "ps shows the starting wording"

set +e
k2s publish status hearth --json >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
[ "$rc" = 2 ] || fail "status <starting>: want exit 2, got $rc ($(cat "$WORK/out" "$WORK/err"))"
python3 - "$WORK/out" <<'PY2' || fail "status --json shape: $(cat "$WORK/out")"
import json, sys
d = json.load(open(sys.argv[1]))
assert d["status"] == "starting", d
assert d["statusText"] == "starting (waiting for port 3047, 4m12s left)", d
assert d["probeTimeoutMs"] == 600000 and d["startingLeftMs"] == 252400, d
PY2
set +e
out="$(k2s publish status hearth 2>&1)"
set -e
printf '%s\n' "$out" | grep -Fxq "hearth: starting (waiting for port 3047, 4m12s left)" || fail "status line: $out"
ok "status <name>: exit 2, statusText and grace fields"

out="$(k2s publish --help 2>&1)" || true
printf '%s' "$out" | grep -Fq -- "--probe-timeout <dur>" || fail "publish help lacks the flag: $out"
ok "publish help documents --probe-timeout"

echo "PASS: publish probe timeout ($pass checks)"
