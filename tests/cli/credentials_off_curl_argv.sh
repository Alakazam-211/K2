#!/usr/bin/env bash
# 0.45.1 — credentials never ride curl's argv.
#
# A process's argv is readable by every OS user (`ps`). The k2 CLI, the
# agent hook script's sibling the browser shim (scripts/k2-open) and every
# curl they run must keep the owner token, a session passport, the Connect
# bearer, passwords and API keys OFF curl's argv, while the daemon still
# receives them exactly as before (`?token=` query, Bearer header, body).
#
# How: a `curl` shim first on PATH logs every argv it is given and then
# runs the real curl against a python stub daemon (TCP + a real Unix
# socket). Each scenario asserts the stub got the credential and the argv
# log never contains it.
#
# Covers every CLI transport helper: cli_request, cli_request_post,
# cli_post, cli_post_json, cli_post_form, the UDS arms, _sidecar_send
# (GET + POST, TCP + UDS), respond/done, fed outbox, daemon restart
# --host, _connect_curl, the stale-passport probe, and the k2-open shim.
# Also round-trips tricky bodies through the curl config quoting.
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
K2_OPEN="$PROJECT_ROOT/scripts/k2-open"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
[ -f "$K2_OPEN" ] || { echo "FAIL: $K2_OPEN not found" >&2; exit 1; }

REAL_CURL="$(command -v curl)"
[ -n "$REAL_CURL" ] || { echo "FAIL: curl not found" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-cred-argv-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-argv-secret-1111"
SCOPED="sessA.scoped-argv-secret-2222"
CONNECT="k2c_connect-argv-secret-3333"
PASSWORD="hunter2-argv-secret-4444"
APIKEY="sk-argv-secret-5555"
SECRETS=("$OWNER" "$SCOPED" "$CONNECT" "$PASSWORD" "$APIKEY" "argv-secret")

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj" "$WORK/shim" "$WORK/bodies"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
echo "$OWNER" >"$HOME/.k2/daemon.token"
chmod 600 "$HOME/.k2/heartbeat.token" "$HOME/.k2/daemon.token"
: >"$WORK/requests.log"
ARGV_LOG="$WORK/curl-argv.log"
: >"$ARGV_LOG"
# A short socket path: sun_path is ~104 bytes on macOS.
SOCK="/tmp/k2ca-$$.sock"
rm -f "$SOCK"

# ── curl shim: log argv, then the real curl (stdin passes through) ────
cat >"$WORK/shim/curl" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> '$ARGV_LOG'
exec '$REAL_CURL' "\$@"
EOF
chmod +x "$WORK/shim/curl"
export PATH="$WORK/shim:$PATH"
[ "$(command -v curl)" = "$WORK/shim/curl" ] || { echo "FAIL: curl shim not first on PATH" >&2; exit 1; }

# ── stub daemon: TCP + Unix socket, records every request ─────────────
python3 - "$WORK" "$SOCK" <<'PYEOF' &
import json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, sock_path = sys.argv[1], sys.argv[2]

class H(BaseHTTPRequestHandler):
    transport = "tcp"
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {
            "transport": self.transport,
            "method": method,
            "path": parsed.path,
            "token_query": (q.get("token") or [""])[0],
            "authorization": self.headers.get("Authorization") or "",
            "body": body.decode("utf-8", "replace"),
        }
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        if parsed.path == "/echo":
            data = body
            ctype = "application/octet-stream"
        else:
            data = json.dumps({"ok": True, "success": True, "heartbeats": [],
                               "subdomains": [], "messages": [], "pending": [],
                               "dead": [], "tools": []}).encode()
            ctype = "application/json"
        self.send_response(200)
        self.send_header("Content-Type", ctype)
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
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && [ -S "$SOCK" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] && [ -S "$SOCK" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
PORT="$(cat "$WORK/stub.port")"
echo "$PORT" >"$HOME/.k2/heartbeat.port"

# Requests matching path / transport / where the credential arrived.
seen() { # seen <path> <transport> <field> <value>
    python3 - "$WORK/requests.log" "$@" <<'PY'
import json, sys
log, path, transport, field, value = sys.argv[1:6]
for line in open(log):
    r = json.loads(line)
    if r["path"] == path and r["transport"] == transport and value in r[field]:
        sys.exit(0)
sys.exit(1)
PY
}
expect_seen() { # expect_seen <label> <path> <transport> <field> <value>
    local label="$1"; shift
    if seen "$@"; then ok "$label"; else bad "$label (stub log: $(tail -3 "$WORK/requests.log"))"; fi
}

clean_env() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u K2_HOOK_TOKEN \
        -u K2_API_CELL -u K2SO_API_CELL -u K2_CELL -u K2_HOST -u K2_REMOTE_TOKEN \
        -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX -u K2_AIRGAP \
        K2_PROJECT_PATH="$WORK/proj" "$@"
}
# External owner terminal: TCP + owner token from ~/.k2.
owner() { clean_env K2_PORT="$PORT" "$K2_CLI" "$@"; }
# Scoped host session with no socket: TCP + passport.
scoped_tcp() { clean_env K2_PORT="$PORT" K2_HOOK_TOKEN="$SCOPED" "$K2_CLI" "$@"; }
# Cell with a live per-cell socket: UDS + passport.
cell() { clean_env K2_PORT="$PORT" K2_HOOK_TOKEN="$SCOPED" K2_HOOK_SOCK="$SOCK" "$K2_CLI" "$@"; }

# ── 1. config quoting round-trips through the real curl ───────────────
echo "== curl config quoting round-trip =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_CURL_CFG/,/^# END_CLI_CURL_CFG/p' "$K2_CLI")"
roundtrip() { # roundtrip <label> <value>
    local label="$1" value="$2" _K2_CFG=""
    printf '%s' "$value" >"$WORK/bodies/want"
    _k2_cfg_add data-raw "$value"
    _k2_curl "$_K2_CFG" -s -X POST "http://127.0.0.1:${PORT}/echo" -o "$WORK/bodies/got"
    if cmp -s "$WORK/bodies/want" "$WORK/bodies/got"; then
        ok "round-trip: $label"
    else
        bad "round-trip: $label (want $(od -c "$WORK/bodies/want" | head -2 | tr -s ' '), got $(od -c "$WORK/bodies/got" | head -2 | tr -s ' '))"
    fi
}
roundtrip "plain" "token=abc&project=%2Fx"
roundtrip "empty" ""
roundtrip "quotes and backslashes" 'a "quoted" \ back\\slash \" \n literal'
roundtrip "newlines (lead, inner, trailing)" $'\n\nfirst\nsecond\n\n'
roundtrip "CRLF and tab" $'crlf\r\nline\r\n\tend\t'
roundtrip "leading @ stays literal" "@/etc/passwd"
roundtrip "leading # stays literal" "# not a comment"
roundtrip "unicode" "ünïcødé ✓ 漢字"
roundtrip "json" '{"username":"bob","password":"p\"w\\d","note":"a\nb"}'
big="$(python3 -c 'import json; print(json.dumps({"rows": [{"i": i, "s": "q\"b\\\\n\n%d" % i} for i in range(6000)]}, indent=1))')"
t0=$(date +%s)
roundtrip "big multi-line json (~$(( ${#big} / 1024 )) KB)" "$big"
t1=$(date +%s)
if [ $((t1 - t0)) -le 10 ]; then ok "big body quoting is fast ($((t1 - t0))s)"; else bad "big body quoting took $((t1 - t0))s"; fi
: >"$ARGV_LOG"
: >"$WORK/requests.log"

# ── 2. external owner terminal (TCP, owner token) ─────────────────────
echo "== owner over TCP =="
owner heartbeat list --json >/dev/null 2>&1 || true
expect_seen "cli_request GET carries ?token=" /cli/heartbeat/list tcp token_query "$OWNER"
owner review approve agentx branchx >/dev/null 2>&1 || true
expect_seen "cli_request_post carries ?token=" /cli/review/approve tcp token_query "$OWNER"
owner tunnel stop >/dev/null 2>&1 || true
expect_seen "cli_post carries ?token=" /cli/tunnel/stop tcp token_query "$OWNER"
owner msg some-ws "hello there" >/dev/null 2>&1 || true
expect_seen "cli_post_form carries ?token=" /cli/workspace/msg tcp token_query "$OWNER"
printf '%s\n' "$PASSWORD" | owner users add bob --password-stdin >/dev/null 2>&1 || true
expect_seen "cli_post_json carries ?token=" /cli/users/add tcp token_query "$OWNER"
expect_seen "cli_post_json body carries the password" /cli/users/add tcp body "$PASSWORD"
printf '%s\n' "$APIKEY" | owner llm tokens add-key claude work >/dev/null 2>&1 || true
expect_seen "_sidecar_send POST carries ?token=" /cli/llm/accounts/add-key tcp token_query "$OWNER"
expect_seen "_sidecar_send POST body carries the API key" /cli/llm/accounts/add-key tcp body "$APIKEY"
owner llm tokens list >/dev/null 2>&1 || true
expect_seen "_sidecar_send GET carries ?token=" /cli/llm/accounts/list tcp token_query "$OWNER"
owner fed outbox >/dev/null 2>&1 || true
expect_seen "fed outbox carries ?token=" /cli/federation/outbox tcp token_query "$OWNER"
owner daemon restart --host "http://127.0.0.1:${PORT}" >/dev/null 2>&1 || true
expect_seen "daemon restart --host carries ?token=" /cli/daemon/restart tcp token_query "$OWNER"
printf '{"token":"%s","subdomain":"me"}' "$CONNECT" >"$HOME/.k2/tunnel.json"
chmod 600 "$HOME/.k2/tunnel.json"
clean_env K2_PORT="$PORT" K2_CONNECT_BASE="http://127.0.0.1:${PORT}" "$K2_CLI" publish status --json >/dev/null 2>&1 || true
expect_seen "_connect_curl carries the Connect bearer" /subdomains tcp authorization "Bearer $CONNECT"

# ── 3. scoped session without a socket (TCP, passport) ────────────────
echo "== scoped session over TCP =="
scoped_tcp respond "a response line" >/dev/null 2>&1 || true
expect_seen "respond carries the passport header" /cli/respond tcp authorization "Bearer $SCOPED"
clean_env K2_PORT="$PORT" K2_HOOK_TOKEN="$SCOPED" K2_API_CELL=1 "$K2_CLI" done --reason finished >/dev/null 2>&1 || true
expect_seen "done carries the passport header" /cli/session/complete tcp authorization "Bearer $SCOPED"

# ── 4. cell with a live socket (UDS, passport) ────────────────────────
echo "== cell over the per-cell socket =="
cell heartbeat list --json >/dev/null 2>&1 || true
expect_seen "cli_request_uds carries the passport" /cli/heartbeat/list uds authorization "Bearer $SCOPED"
cell heartbeat enable nightly >/dev/null 2>&1 || true
expect_seen "cli_request_post_uds carries the passport" /cli/heartbeat/enable uds authorization "Bearer $SCOPED"
cell msg some-ws "hi from a cell" >/dev/null 2>&1 || true
expect_seen "cli_post_form_uds carries the passport" /cli/workspace/msg uds authorization "Bearer $SCOPED"
cell thread secret k2/x --name db-pass >/dev/null 2>&1 || true
expect_seen "cli_post_json_uds carries the passport" /cli/thread/secret uds authorization "Bearer $SCOPED"
printf '%s\n' "$APIKEY" | cell llm tokens add-key claude work >/dev/null 2>&1 || true
expect_seen "_sidecar_send UDS carries the passport" /cli/llm/accounts/add-key uds authorization "Bearer $SCOPED"
expect_seen "_sidecar_send UDS body carries the API key" /cli/llm/accounts/add-key uds body "$APIKEY"
cell llm tokens list >/dev/null 2>&1 || true
expect_seen "_sidecar_send UDS GET carries the passport" /cli/llm/accounts/list uds authorization "Bearer $SCOPED"

# ── 5. stale-passport probe (sourced; the e2e path needs a Codex tree) ─
echo "== stale-passport probe =="
eval "$(sed -n '/^# BEGIN_CLI_STALE_PASSPORT/,/^# END_CLI_STALE_PASSPORT/p' "$K2_CLI")"
SCOPED_TOKEN="$SCOPED" BASE_URL="http://127.0.0.1:${PORT}" _cli_scoped_token_rejected || true
expect_seen "whoami probe carries the passport" /cli/whoami tcp authorization "Bearer $SCOPED"

# ── 6. browser shim (scripts/k2-open) ─────────────────────────────────
echo "== k2-open shim =="
clean_env K2_HOOK_TOKEN="$SCOPED" K2_HOOK_SOCK="$SOCK" /bin/sh "$K2_OPEN" "https://example.com/a?b=c" >/dev/null 2>&1 || true
expect_seen "k2-open socket arm carries the passport" /cli/browser/open-url uds authorization "Bearer $SCOPED"
clean_env /bin/sh "$K2_OPEN" "https://example.com/owner" >/dev/null 2>&1 || true
expect_seen "k2-open owner arm carries ?token=" /cli/browser/open-url tcp token_query "$OWNER"

# ── 7. nothing secret ever reached curl's argv ────────────────────────
echo "== curl argv =="
n_curls="$(wc -l <"$ARGV_LOG" | tr -d ' ')"
if [ "$n_curls" -ge 20 ]; then ok "the shim saw $n_curls curl runs"; else bad "only $n_curls curl runs logged"; fi
leaked=0
for s in "${SECRETS[@]}"; do
    if grep -qF -- "$s" "$ARGV_LOG"; then
        bad "secret on curl argv: $s ($(grep -F -- "$s" "$ARGV_LOG" | head -2))"
        leaked=1
    fi
done
[ "$leaked" = 0 ] && ok "no token, passport, bearer, password or API key on any curl argv"
if grep -qi "authorization" "$ARGV_LOG"; then
    bad "an Authorization header on curl argv: $(grep -i authorization "$ARGV_LOG" | head -2)"
else
    ok "no Authorization header on any curl argv"
fi
if grep -q "token=" "$ARGV_LOG"; then
    bad "a token= query on curl argv: $(grep "token=" "$ARGV_LOG" | head -2)"
else
    ok "no token= on any curl argv"
fi

echo
echo "credentials_off_curl_argv: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
