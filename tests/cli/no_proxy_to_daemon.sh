#!/usr/bin/env bash
# The CLI never sends a request to the daemon through a proxy, and never
# puts a credential in a request's query string.
#
# A stub proxy listens and logs every connection it gets. The proxy
# variables (http_proxy, HTTP_PROXY, https_proxy, HTTPS_PROXY, ALL_PROXY,
# all_proxy) all point at it and no_proxy is unset. A stub daemon (TCP and
# a real Unix socket, the cell socket) records every request. A broad set
# of verbs runs: at least one through each sender (cli_request,
# cli_request_post, cli_post, cli_post_form, cli_post_json,
# cli_post_json_stdin, cli_send_json, _compute_send, _sidecar_send, the
# cell socket arms) and every embedded Python block that calls the daemon.
#
# Each case asserts the daemon stub got the request; at the end the proxy
# stub must have got nothing, and no request the daemon stub saw may carry
# token= in its query (the credential rides Authorization: Bearer).
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-noproxy-XXXXXX)"
STUB_PID=""
PROXY_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    [ -n "${PROXY_PID:-}" ] && kill "$PROXY_PID" 2>/dev/null || true
    rm -rf "$WORK"
    rm -f "${SOCK:-}"
}
trap cleanup EXIT

OWNER="owner-noproxy-1111"
SCOPED="sessA.scoped-noproxy-2222"

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
echo "$OWNER" >"$HOME/.k2/daemon.token"
chmod 600 "$HOME/.k2/heartbeat.token" "$HOME/.k2/daemon.token"
: >"$WORK/requests.log"
: >"$WORK/proxy.log"
# A short socket path: sun_path is ~104 bytes on macOS.
SOCK="/tmp/k2np-$$.sock"
rm -f "$SOCK"

# ── stub proxy: logs every connection, answers 502 ────────────────────
python3 - "$WORK" <<'PYEOF' &
import os, socketserver, sys
work = sys.argv[1]
class P(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(2)
        try:
            data = self.request.recv(4096)
        except OSError:
            data = b""
        line = data.split(b"\r\n", 1)[0].decode("latin-1") or "(no request line)"
        with open(os.path.join(work, "proxy.log"), "a") as f:
            f.write(line + "\n")
        try:
            self.request.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        except OSError:
            pass
class S(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    allow_reuse_address = True
srv = S(("127.0.0.1", 0), P)
with open(os.path.join(work, "proxy.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
PROXY_PID=$!
disown "$PROXY_PID" 2>/dev/null || true

# ── stub daemon: TCP + Unix socket, records every request ─────────────
python3 - "$WORK" "$SOCK" <<'PYEOF' &
import json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, sock_path = sys.argv[1], sys.argv[2]
proj = os.path.join(work, "proj")

class H(BaseHTTPRequestHandler):
    transport = "tcp"
    protocol_version = "HTTP/1.0"
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        rec = {
            "transport": self.transport,
            "method": method,
            "path": parsed.path,
            "query": parsed.query,
            "authorization": self.headers.get("Authorization") or "",
        }
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        if parsed.path == "/cli/agent/conf":
            d = {"ok": True, "path": proj, "name": "agentx", "id": "agentx",
                 "harness": "claude", "enabled": True, "projects": []}
        else:
            d = {"ok": True, "success": True, "id": "abc12345", "status": "waiting",
                 "title": "t", "items": [], "heartbeats": [], "messages": [],
                 "pending": [], "comments": [], "nodes": [], "jobs": [],
                 "accounts": [], "inboxes": [], "calendars": [], "presets": [],
                 "layers": [], "sessions": [], "tools": [], "subdomains": []}
        data = json.dumps(d).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self): self._go("GET")
    def do_POST(self): self._go("POST")
    def do_PUT(self): self._go("PUT")
    def do_DELETE(self): self._go("DELETE")
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
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true

for _ in $(seq 1 100); do
    [ -s "$WORK/stub.port" ] && [ -S "$SOCK" ] && [ -s "$WORK/proxy.port" ] && break
    sleep 0.05
done
[ -s "$WORK/stub.port" ] && [ -S "$SOCK" ] && [ -s "$WORK/proxy.port" ] || { echo "FAIL: a stub never bound" >&2; exit 1; }
PORT="$(cat "$WORK/stub.port")"
PROXY="http://127.0.0.1:$(cat "$WORK/proxy.port")"
echo "$PORT" >"$HOME/.k2/heartbeat.port"

# The proxy stub answers: a request sent to it would be seen there.
curl -s -o /dev/null --max-time 5 --noproxy '' -x "$PROXY" "http://127.0.0.1:${PORT}/selftest" || true
if [ "$(wc -l <"$WORK/proxy.log" | tr -d ' ')" = 1 ]; then ok "the proxy stub logs what reaches it"; else bad "the proxy stub did not log its self test"; fi
: >"$WORK/proxy.log"

clean_env() {
    env -u K5_HOOK_SOCK -u K5_HOOK_TOKEN -u K5_PORT -u K5_HOST \
        -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u K2_HOOK_TOKEN \
        -u K2_API_CELL -u K2SO_API_CELL -u K2_CELL -u K2_HOST -u K2_REMOTE_TOKEN \
        -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX -u K2_AIRGAP \
        -u no_proxy -u NO_PROXY \
        http_proxy="$PROXY" HTTP_PROXY="$PROXY" https_proxy="$PROXY" HTTPS_PROXY="$PROXY" \
        ALL_PROXY="$PROXY" all_proxy="$PROXY" \
        K2_PROJECT_PATH="$WORK/proj" "$@"
}
# External owner terminal: TCP + owner token from ~/.k2.
owner() { clean_env K2_PORT="$PORT" "$K2_CLI" "$@"; }
# The same, with the loopback host spelled localhost.
owner_localhost() { clean_env K2_PORT="$PORT" K2_HOST=localhost "$K2_CLI" "$@"; }
# Scoped API session with no socket: TCP + passport.
scoped_tcp() { clean_env K2_PORT="$PORT" K2_HOOK_TOKEN="$SCOPED" K2_API_CELL=1 "$K2_CLI" "$@"; }
# Cell with a live per-cell socket: UDS + passport.
cell() { clean_env K2_PORT="$PORT" K2_HOOK_TOKEN="$SCOPED" K2_HOOK_SOCK="$SOCK" "$K2_CLI" "$@"; }

# A verb that waits on something the stub never sends ends after 60 s.
# The runners are shell functions, so the limit runs them in a child bash.
LIMIT=(bash -c '"$@"' _)
command -v timeout >/dev/null 2>&1 && LIMIT=(timeout 60 bash -c '"$@"' _)
export -f clean_env owner owner_localhost scoped_tcp cell
export WORK PORT PROXY SOCK SCOPED K2_CLI

# case_run <label> <stdin> <runner> <args…>: run one verb; it must reach
# the daemon stub at least once. Prints the paths it asked for.
case_run() {
    local label="$1" input="$2"; shift 2
    local before after rc=0
    before="$(wc -l <"$WORK/requests.log" | tr -d ' ')"
    printf '%s' "$input" | "${LIMIT[@]}" "$@" >"$WORK/out.txt" 2>"$WORK/err.txt" || rc=$?
    after="$(wc -l <"$WORK/requests.log" | tr -d ' ')"
    if [ "$after" -gt "$before" ]; then
        ok "$label reached the daemon stub ($(tail -n +"$((before + 1))" "$WORK/requests.log" | python3 -c '
import json, sys
print(" ".join(sorted({"%s:%s" % (json.loads(l)["transport"], json.loads(l)["path"]) for l in sys.stdin})))'))"
    else
        bad "$label never reached the daemon stub (rc=$rc; stderr: $(tail -3 "$WORK/err.txt" | tr '\n' ' '))"
    fi
}

echo "== owner over TCP (127.0.0.1) =="
case_run "cli_request (heartbeat list)" "" owner heartbeat list --json
case_run "cli_request_post (review approve)" "" owner review approve agentx branchx
case_run "cli_post (tunnel stop)" "" owner tunnel stop
case_run "cli_post_form (msg)" "" owner msg some-ws "hello there"
case_run "cli_post_json (users add)" "pw-noproxy-1"$'\n' owner users add bob --password-stdin
case_run "cli_post_json_stdin (api-key create)" "" owner api-key create --status --label mon
case_run "cli_send_json (whoami)" "" owner whoami
case_run "inbox" "" owner inbox
case_run "read" "" owner read some-ws
case_run "sessions" "" owner sessions
case_run "thread" "" owner thread k2 "a thread line"
case_run "_sidecar_send (llm tokens list)" "" owner llm tokens list
case_run "_compute_send (compute nodes)" "" owner compute nodes
case_run "fed outbox" "" owner fed outbox
case_run "daemon restart --host" "" owner daemon restart --host "http://127.0.0.1:${PORT}"
case_run "daemon survive status" "" owner daemon survive status
case_run "git-tools status" "" owner git-tools status
case_run "setup" "" owner setup
case_run "tickets list" "" owner tickets list
case_run "tickets show" "" owner tickets show abc12345
case_run "tickets show --json" "" owner tickets show abc12345 --json
case_run "publish status (connect proxy)" "" owner publish status --json
case_run "workspace swap-canonical" "" owner workspace swap-canonical "$WORK/proj" --agent claude

echo "== owner, Python blocks =="
case_run "python: agent context list" "" owner agent context list
case_run "python: agent set" "" owner agent set agentx --enabled false
case_run "python: agent retire" "" owner agent retire agentx --dry-run
case_run "python: agent restart" "" owner agent restart agentx
case_run "python: agent hire" "" owner agent hire "$WORK/hired" --name hired --dry-run
case_run "python: calendar list" "" owner calendar list --json
case_run "python: clone preview" "" owner clone preview some-ws
case_run "python: db status" "" owner db status --json
case_run "python: tickets ask" "" owner tickets ask "A question?"
case_run "python: mail inboxes" "" owner mail inboxes --json
case_run "python: mail reader inboxes" "" owner mail reader inboxes --json
case_run "python: preset list" "" owner preset list --json
case_run "python: project access" "" owner project access on --json
case_run "python: heartbeat wake check" "" owner heartbeat wake

echo "== owner over TCP (localhost) =="
case_run "cli_request on localhost" "" owner_localhost heartbeat list --json
case_run "python on localhost: preset list" "" owner_localhost preset list --json

echo "== scoped session over TCP =="
case_run "respond" "" scoped_tcp respond "a response line"
case_run "done" "" scoped_tcp done --reason finished

echo "== cell over the per-cell socket =="
case_run "cell: heartbeat list" "" cell heartbeat list --json
case_run "cell: msg" "" cell msg some-ws "hi from a cell"
case_run "cell: whoami" "" cell whoami
case_run "cell: compute nodes" "" cell compute nodes
case_run "cell: llm tokens list" "" cell llm tokens list

echo "== what reached the proxy =="
if [ -s "$WORK/proxy.log" ]; then
    bad "requests reached the proxy stub: $(sort "$WORK/proxy.log" | uniq -c | head -20 | tr '\n' ';')"
else
    ok "no request reached the proxy stub"
fi

echo "== credentials stay out of the query =="
n="$(wc -l <"$WORK/requests.log" | tr -d ' ')"
if [ "$n" -ge 40 ]; then ok "the daemon stub saw $n requests"; else bad "the daemon stub saw only $n requests"; fi
if python3 - "$WORK/requests.log" <<'PY'
import json, sys, urllib.parse
bad = []
for line in open(sys.argv[1]):
    r = json.loads(line)
    if "token" in urllib.parse.parse_qs(r["query"], keep_blank_values=True):
        bad.append("%s %s" % (r["transport"], r["path"]))
if bad:
    print("token= in the query: " + ", ".join(sorted(set(bad))[:20]))
    sys.exit(1)
PY
then ok "no request carried token= in its query"; else bad "a credential rode the query string (above)"; fi
if python3 - "$WORK/requests.log" "$OWNER" "$SCOPED" <<'PY'
import json, sys
log, owner, scoped = sys.argv[1:4]
missing = []
for line in open(log):
    r = json.loads(line)
    if r["path"] in ("/selftest", "/boot-status"):
        continue
    if r["authorization"] not in ("Bearer " + owner, "Bearer " + scoped):
        missing.append("%s %s" % (r["transport"], r["path"]))
if missing:
    print("no Bearer credential: " + ", ".join(sorted(set(missing))[:20]))
    sys.exit(1)
PY
then ok "every request carried its credential as Authorization: Bearer"; else bad "a request carried no Bearer credential (above)"; fi

echo
echo "no_proxy_to_daemon: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
