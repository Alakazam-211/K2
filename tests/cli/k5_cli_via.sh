#!/usr/bin/env bash
# The Rust front's two hand-over variables, read by the prelude.
#
#   K5_CLI_VIA=rust     every daemon request carries `X-K5-Cli-Via:
#                       fallthrough`, over TCP and the cell socket alike.
#                       Unset, empty or any other value: no request does.
#                       The Connect control plane is not the daemon: the
#                       CLI's own call to it never carries the header.
#   K5_CLI_NOTICE_DONE  set non-empty: _K5_NOTICE_DONE=1, where the k2-to-k5
#                       notice will look. Empty or unset: it stays empty.
#
# A python stub daemon (TCP + a real Unix socket) logs, per request, the
# transport, the path and the X-K5-Cli-Via values it got. Nothing else is
# logged (no tokens). No live K2, HOME is a temp dir.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-k5-via-XXXXXX)"
# A short socket path: sun_path is ~104 bytes on macOS.
SOCK="/tmp/k2kv-$$.sock"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK" "$SOCK"
}
trap cleanup EXIT
rm -f "$SOCK"
mkdir -p "$WORK/home/.k2" "$WORK/proj"
echo "owner-test-token" >"$WORK/home/.k2/heartbeat.token"
chmod 600 "$WORK/home/.k2/heartbeat.token"
: >"$WORK/requests.log"

python3 - "$WORK" "$SOCK" <<'PY' &
import json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work, sock_path = sys.argv[1], sys.argv[2]
class H(BaseHTTPRequestHandler):
    transport = "tcp"
    def _go(self):
        n = int(self.headers.get("Content-Length", 0) or 0)
        if n:
            self.rfile.read(n)
        rec = {
            "transport": self.transport,
            "path": urllib.parse.urlparse(self.path).path,
            "via": self.headers.get_all("X-K5-Cli-Via") or [],
        }
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        if rec["path"] == "/cli/connect/proxy":
            # A daemon without the proxy route: the CLI then calls the
            # control plane (K2_CONNECT_BASE, this same stub) itself.
            data = b'{"error":"not found"}'
            self.send_response(404)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        data = json.dumps({"ok": True, "success": True, "heartbeats": []}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self): self._go()
    def do_POST(self): self._go()
    def log_message(self, *a): pass
class U(H):
    transport = "uds"
    def address_string(self): return "uds"
class UnixHTTP(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
class TcpHTTP(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True
us = UnixHTTP(sock_path, U)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = TcpHTTP(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && [ -S "$SOCK" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] && [ -S "$SOCK" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
PORT="$(cat "$WORK/stub.port")"

# Never inherit a session, or a K5_ name, from the caller.
clean_env() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u K2_HOOK_TOKEN \
        -u K5_HOOK_SOCK -u K5_HOOK_TOKEN -u K5_PORT -u K5_PROJECT_PATH \
        -u K5_CLI_VIA -u K5_CLI_NOTICE_DONE -u K5_CLI_CALLED_AS \
        -u K2_API_CELL -u K2SO_API_CELL -u K2_CELL -u K2_HOST -u K2_REMOTE_TOKEN \
        -u K2_SECRETS_DIR -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX -u K2_AIRGAP \
        HOME="$WORK/home" K2_PORT="$PORT" K2_PROJECT_PATH="$WORK/proj" "$@"
}
# A session with no cell socket (TCP) and one with a live socket (UDS).
tcp() { clean_env K2_HOOK_TOKEN="sessA.passport" "$@"; }
uds() { clean_env K2_HOOK_TOKEN="sessA.passport" K2_HOOK_SOCK="$SOCK" "$@"; }

# vias <transport> <path>: the X-K5-Cli-Via values of every matching
# request, one request per line (`-` for none). Fails when there is none.
vias() {
    python3 - "$WORK/requests.log" "$1" "$2" <<'PY'
import json, sys
log, transport, path = sys.argv[1:4]
rows = [json.loads(l) for l in open(log)]
hits = [r for r in rows if r["transport"] == transport and r["path"] == path]
if not hits:
    sys.exit("no %s request to %s (log: %s)" % (transport, path, rows[-3:]))
for r in hits:
    print(",".join(r["via"]) or "-")
PY
}
# expect <label> <transport> <path> <want>: every matching request's via.
expect() {
    local label="$1" got
    if ! got="$(vias "$2" "$3" 2>&1)"; then bad "$label ($got)"; return; fi
    if [ -n "$got" ] && [ -z "$(grep -vxF -- "$4" <<<"$got")" ]; then
        ok "$label"
    else
        bad "$label (got $(tr '\n' ' ' <<<"$got"), wanted $4 on each)"
    fi
}

# one <transport> <via setting> <want>: a GET and a form POST.
# The via setting is `unset` or `=<value>`.
one() {
    local transport="$1" via="$2" want="$3" pre=()
    case "$via" in
        unset) pre=() ;;
        =*) pre=(K5_CLI_VIA="${via#=}") ;;
    esac
    : >"$WORK/requests.log"
    "$transport" ${pre[@]+"${pre[@]}"} "$K2_CLI" heartbeat list --json >/dev/null 2>&1 || true
    expect "$transport GET, K5_CLI_VIA $via: $want" "$transport" /cli/heartbeat/list "$want"
    : >"$WORK/requests.log"
    "$transport" ${pre[@]+"${pre[@]}"} "$K2_CLI" msg some-ws "hello" >/dev/null 2>&1 || true
    expect "$transport POST, K5_CLI_VIA $via: $want" "$transport" /cli/workspace/msg "$want"
}

echo "== X-K5-Cli-Via =="
for t in tcp uds; do
    one "$t" "=rust" fallthrough
    one "$t" unset -
    one "$t" "=" -
    one "$t" "=RUST" -
    one "$t" "=bash" -
done

echo "== the Connect control plane is not the daemon =="
printf '{"token":"k2c_test-connect","subdomain":"me"}' >"$WORK/home/.k2/tunnel.json"
chmod 600 "$WORK/home/.k2/tunnel.json"
: >"$WORK/requests.log"
clean_env K5_CLI_VIA=rust K2_CONNECT_BASE="http://127.0.0.1:${PORT}" "$K2_CLI" publish status --json >/dev/null 2>&1 || true
expect "the daemon's proxy route gets the header" tcp /cli/connect/proxy fallthrough
expect "the control plane does not" tcp /subdomains -
rm -f "$WORK/home/.k2/tunnel.json"

echo "== _K5_NOTICE_DONE and _K5_CLI_VIA, from bash's trace =="
# traced <var> <env…>: the value the prelude gave <var>.
traced() {
    local var="$1" trace line; shift
    trace="$(clean_env "$@" bash -x "$K2_CLI" --version 2>&1 >/dev/null)" || { echo "--version failed"; return 1; }
    line="$(grep -E "^\\++ ${var}=" <<<"$trace" | tail -1)" || { echo "no $var line"; return 1; }
    printf '%s\n' "${line#*=}"
}
want_traced() { # want_traced <label> <var> <want> <env…>
    local label="$1" var="$2" want="$3" got; shift 3
    if got="$(traced "$var" "$@")" && [ "$got" = "$want" ]; then ok "$label"; else bad "$label (got '$got', wanted '$want')"; fi
}
want_traced "NOTICE_DONE unset: empty" _K5_NOTICE_DONE "" K2_HOOK_TOKEN=x
want_traced "NOTICE_DONE empty: empty" _K5_NOTICE_DONE "" K5_CLI_NOTICE_DONE=
want_traced "NOTICE_DONE=1: 1" _K5_NOTICE_DONE 1 K5_CLI_NOTICE_DONE=1
want_traced "NOTICE_DONE=yes: 1" _K5_NOTICE_DONE 1 K5_CLI_NOTICE_DONE=yes
want_traced "VIA unset: empty" _K5_CLI_VIA "" K2_HOOK_TOKEN=x
want_traced "VIA=rust: rust" _K5_CLI_VIA rust K5_CLI_VIA=rust
want_traced "VIA=other: empty" _K5_CLI_VIA "" K5_CLI_VIA=other

echo
echo "k5_cli_via: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
