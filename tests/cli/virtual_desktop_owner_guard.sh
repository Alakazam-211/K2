#!/usr/bin/env bash
# Virtual desktops (prd-linux-virtual-desktop-v1 LD-10): every program on a
# K2 virtual desktop runs with K2_NO_OWNER_FALLBACK=1. A `k2` there with no
# session passport must refuse (exit 3, saying why) instead of reading the
# owner token from ~/.k2 and acting as the owner.
#
# A stub daemon records every request. Control run first: without the guard
# the same call reaches the stub with the owner token, so the test can fail.
# Never touches the real ~/.k2 (HOME is a temp dir).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

WORK="$(mktemp -d -t k2-vd-guard-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-vd-guard"
mkdir -p "$WORK/home/.k2"
echo "$OWNER" >"$WORK/home/.k2/daemon.token"
echo "$OWNER" >"$WORK/home/.k2/heartbeat.token"

python3 -I -c '
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def _any(self):
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(self.path + " " + (self.headers.get("Authorization") or "") + "\n")
        data = json.dumps({"ok": True}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    do_GET = _any
    do_POST = _any
    def log_message(self, *a):
        pass
s = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(work, "port.tmp"), "w").write(str(s.server_port))
os.rename(os.path.join(work, "port.tmp"), os.path.join(work, "port"))
s.serve_forever()
' "$WORK" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -f "$WORK/port" ] && break; sleep 0.05; done
[ -f "$WORK/port" ] || { echo "FAIL: the stub daemon didn't start" >&2; exit 1; }
cp "$WORK/port" "$WORK/home/.k2/heartbeat.port"

# A shell with no K2 identity at all (as on a virtual desktop).
run_k2() {
    env -i PATH="$PATH" HOME="$WORK/home" "$@" "$K2_CLI" whoami
}

fail=0

# Control: no guard → the owner token goes out (proves the stub works).
set +e
run_k2 >/dev/null 2>&1
set -e
if grep -q "$OWNER" "$WORK/requests.log" 2>/dev/null; then
    echo "  PASS: control: without the guard k2 sends the owner token"
else
    echo "  FAIL: control: the stub saw no owner-token request" >&2
    fail=1
fi
rm -f "$WORK/requests.log"

# The guard: exit 3, says why, nothing reaches the daemon.
set +e
out="$(run_k2 K2_NO_OWNER_FALLBACK=1 2>&1)"
rc=$?
set -e
if [ "$rc" = "3" ]; then echo "  PASS: exit 3"; else echo "  FAIL: exit $rc (want 3): $out" >&2; fail=1; fi
case "$out" in
    *"won't act as this server's owner"*K2_NO_OWNER_FALLBACK*) echo "  PASS: says why" ;;
    *) echo "  FAIL: no reason in: $out" >&2; fail=1 ;;
esac
if [ -s "$WORK/requests.log" ]; then
    echo "  FAIL: a request reached the daemon: $(cat "$WORK/requests.log")" >&2
    fail=1
else
    echo "  PASS: nothing reached the daemon"
fi

# Pages that never reach the daemon still work under the guard.
set +e
env -i PATH="$PATH" HOME="$WORK/home" K2_NO_OWNER_FALLBACK=1 "$K2_CLI" study >/dev/null 2>&1
rc=$?
set -e
if [ "$rc" = "0" ]; then echo "  PASS: k2 study runs"; else echo "  FAIL: k2 study exit $rc" >&2; fail=1; fi

# A session passport is not the owner fallback: it is used as is.
set +e
env -i PATH="$PATH" HOME="$WORK/home" K2_NO_OWNER_FALLBACK=1 K2_HOOK_TOKEN=sess-passport "$K2_CLI" whoami >/dev/null 2>&1
set -e
if grep -q "$OWNER" "$WORK/requests.log" 2>/dev/null; then
    echo "  FAIL: the owner token went out alongside a passport" >&2
    fail=1
else
    echo "  PASS: with a passport, never the owner token"
fi

[ "$fail" = "0" ] || exit 1
echo "virtual_desktop_owner_guard: ok"
