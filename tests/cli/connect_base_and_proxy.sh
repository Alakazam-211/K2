#!/usr/bin/env bash
# Where a token may go, and what never uses a proxy (security fixes).
#
# 1. A K2_CONNECT_BASE with no http:// or https:// in front (curl would
#    guess http and send the tunnel token in clear) is refused before any
#    request, the daemon's included: exit 2 and the one line
#      Error: K2_CONNECT_BASE must start with https:// (got: <value>)
#    Verbs: publish status, publish subdomain list|create|point|rm, wiki
#    publish.
# 2. A token goes over plain http only to this computer, proxied or not.
#    Plain http:// (or no scheme, which curl takes as http) to a host that
#    isn't loopback AS WRITTEN (127.0.0.0/8 as four numbers, [::1], or the
#    name localhost; a name is never resolved, so x.localhost is not) is
#    refused before any request and before the token is looked up: exit 2
#    and
#      Error: plain http:// only to this computer; use https:// (got: <url>)
#    Sites: K2_CONNECT_BASE (the tunnel token), <ws>@<host> (talk, read,
#    remote-session, doc) and daemon restart --host (clone has its own
#    gate: the same rule, its own line and exit 3). Plain
#    http to loopback still works.
# 4. A daemon on another computer over plain http (K2_HOST is a name or an
#    address that isn't loopback as written) is allowed, with one stderr
#    line per run, printed before the command's first request, however
#    many requests it makes:
#      warning: k2 is sending this server's owner token to <host> over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https.
#    None for loopback, a literal Tailscale host (100.64.0.0/10,
#    fd7a:115c:a1e0::/48, *.ts.net; never resolved), https, or with
#    K2_PLAIN_HTTP_OK=1. A password flag there is refused before any
#    request (exit 2), K2_PLAIN_HTTP_OK=1 or not. x.localhost stands in
#    for another computer: curl sends it to loopback, where the stub is.
# 3. Local traffic never goes through a proxy: a control-plane call the CLI
#    makes itself (no daemon) to loopback gets --noproxy '*' like a daemon
#    call, with every proxy variable set. https to a remote control plane
#    keeps the person's proxy (the stub proxy gets its CONNECT), and so
#    does https to a name that only starts with 127 (127.evil.example).
#    The other forms of the rule: localhost in any case or with one
#    trailing dot, and [::1], go straight there; [::ffff:127.0.0.1] is not
#    this computer as written and keeps the proxy.
#
# A stub daemon (loopback TCP), a stub control plane and a stub proxy each
# log what reaches them. A second port is bound and closed: "no daemon"
# runs point K2_PORT at it, so the CLI falls back to calling the control
# plane itself with the token in tunnel.json. The proxy variables
# (http_proxy, HTTP_PROXY, https_proxy, HTTPS_PROXY, all_proxy, ALL_PROXY)
# point at the stub proxy in the rows that say so; no_proxy is unset.
#
# Hermetic: throwaway HOME, every K2_*/K5_*/K2SO_* var unset, no real
# daemon, ~/.k2 or keychain. Opens ports: run it on K2 Compute.
#
#   bash tests/cli/connect_base_and_proxy.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

for v in $(env | grep -E '^(K2|K5|K2SO)_' | cut -d= -f1); do unset "$v"; done
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-cbase-XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-cbase-1111"
TUNNEL="tunnel-cbase-2222"
REMOTE="remote-cbase-3333"
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"
printf '%s\n' "$OWNER" >"$HOME/.k2/heartbeat.token"
printf '%s\n' "$OWNER" >"$HOME/.k2/daemon.token"
printf '{"token": "%s", "subdomain": "acme"}\n' "$TUNNEL" >"$HOME/.k2/tunnel.json"
chmod 600 "$HOME/.k2/heartbeat.token" "$HOME/.k2/daemon.token" "$HOME/.k2/tunnel.json"
LOG="$WORK/log"
: >"$LOG"

cat >"$WORK/stub.py" <<'PY'
import json, os, socket, socketserver, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

work = sys.argv[1]
log_lock = threading.Lock()

def log(line):
    with log_lock:
        with open(os.path.join(work, "log"), "a") as f:
            f.write(line + "\n")

class H(BaseHTTPRequestHandler):
    who = "daemon"
    protocol_version = "HTTP/1.0"
    def _go(self, method):
        n = int(self.headers.get("Content-Length", 0) or 0)
        if n:
            self.rfile.read(n)
        path = self.path.split("?", 1)[0]
        log("%s %s %s %s" % (self.who, method, path, self.headers.get("Authorization") or "-"))
        if self.who == "cp":
            d = {"subdomains": []}
        elif path == "/cli/connect/proxy":
            d = {"status": 200, "body": json.dumps({"subdomains": []})}
        else:
            d = {"ok": True, "success": True, "items": [], "subdomains": []}
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

class CP(H):
    who = "cp"

class P(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(2)
        try:
            data = self.request.recv(4096)
        except OSError:
            data = b""
        log("proxy " + (data.split(b"\r\n", 1)[0].decode("latin-1") or "(no request line)"))
        try:
            self.request.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        except OSError:
            pass

class TS(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    allow_reuse_address = True

d = ThreadingHTTPServer(("127.0.0.1", 0), H)
c = ThreadingHTTPServer(("127.0.0.1", 0), CP)
p = TS(("127.0.0.1", 0), P)
s = socket.socket()
s.bind(("127.0.0.1", 0))
closed = s.getsockname()[1]
s.close()
for srv in (c, p):
    threading.Thread(target=srv.serve_forever, daemon=True).start()
with open(os.path.join(work, "ports.tmp"), "w") as f:
    f.write("%d %d %d %d\n" % (d.server_address[1], c.server_address[1], p.server_address[1], closed))
os.rename(os.path.join(work, "ports.tmp"), os.path.join(work, "ports"))
d.serve_forever()
PY
python3 "$WORK/stub.py" "$WORK" &
STUB_PID=$!
for _ in $(seq 1 100); do
    [ -s "$WORK/ports" ] && break
    sleep 0.05
done
[ -s "$WORK/ports" ] || { echo "FAIL: the stubs never bound" >&2; exit 1; }
read -r DPORT CPPORT PPORT CLOSED <"$WORK/ports"
PROXY="http://127.0.0.1:$PPORT"
CP="http://127.0.0.1:$CPPORT"

# The proxy stub logs what reaches it.
curl -s -o /dev/null --max-time 5 --noproxy '' -x "$PROXY" "http://127.0.0.1:${DPORT}/selftest" || true
if [ "$(grep -c '^proxy GET http://127.0.0.1' "$LOG" || true)" = 1 ]; then
    ok "the proxy stub logs what reaches it"
else
    bad "the proxy stub did not log its self test: $(tr '\n' ';' <"$LOG")"
fi

# run <daemon|nodaemon> <proxy|noproxy> <base or -> <k2 args…>: one verb,
# as the owner in an outside terminal. Sets RC, OUT, ERR; LOG holds what
# reached the stubs. Base "-" leaves K2_CONNECT_BASE unset.
run() {
    local d="$1" px="$2" base="$3" port
    shift 3
    port="$DPORT"
    [ "$d" = daemon ] || port="$CLOSED"
    local -a envs=(K2_PORT="$port" K2_PROJECT_PATH="$WORK/proj")
    [ "$base" = - ] || envs+=(K2_CONNECT_BASE="$base")
    if [ "$px" = proxy ]; then
        envs+=(http_proxy="$PROXY" HTTP_PROXY="$PROXY" https_proxy="$PROXY" HTTPS_PROXY="$PROXY"
            all_proxy="$PROXY" ALL_PROXY="$PROXY")
    fi
    envs+=(${RUN_ENV[@]+"${RUN_ENV[@]}"})
    : >"$LOG"
    RC=0
    env "${envs[@]}" "$K2" "$@" <"${RUN_STDIN:-/dev/null}" >"$WORK/out" 2>"$WORK/err" || RC=$?
    OUT="$(cat "$WORK/out")"
    ERR="$(cat "$WORK/err")"
}

# refused <label> <want stderr line>: exit 2, stderr is exactly that one
# line, and nothing reached any stub.
refused() {
    local label="$1" want="$2"
    if [ "$RC" = 2 ]; then ok "$label: exit 2"; else bad "$label: exit $RC, want 2 (stderr: $(printf %q "$ERR"))"; fi
    if [ "$ERR" = "$want" ]; then ok "$label: the one line"; else bad "$label: stderr $(printf %q "$ERR"), want $(printf %q "$want")"; fi
    if [ -s "$LOG" ]; then bad "$label: a request was sent: $(tr '\n' ';' <"$LOG")"; else ok "$label: nothing sent"; fi
}

# reached <label> <regex>: a line of LOG matches.
reached() {
    if grep -Eq "$2" "$LOG"; then ok "$1"; else bad "$1: no '$2' in $(tr '\n' ';' <"$LOG") (exit $RC, stderr: $(printf %q "$ERR"))"; fi
}
no_proxy_hit() {
    if grep -q '^proxy ' "$LOG"; then bad "$1: reached the proxy: $(grep '^proxy ' "$LOG" | tr '\n' ';')"; else ok "$1: nothing reached the proxy"; fi
}

echo "== 1. a K2_CONNECT_BASE with no scheme is refused before any request =="
NOS="cp.example.test"
WANT1="Error: K2_CONNECT_BASE must start with https:// (got: $NOS)"
run daemon proxy "$NOS" publish subdomain list --json
refused "subdomain list, daemon up" "$WANT1"
run daemon proxy "$NOS" publish status --json
refused "publish status, daemon up" "$WANT1"
run daemon proxy "$NOS" publish subdomain create acme --target localhost:3000
refused "subdomain create, daemon up" "$WANT1"
run daemon proxy "$NOS" publish subdomain point acme --target localhost:3000
refused "subdomain point, daemon up" "$WANT1"
run daemon proxy "$NOS" publish subdomain rm acme
refused "subdomain rm, daemon up" "$WANT1"
run daemon proxy "$NOS" wiki publish --json
refused "wiki publish, daemon up" "$WANT1"
run nodaemon proxy "$NOS" publish subdomain list --json
refused "subdomain list, no daemon (tunnel token on disk)" "$WANT1"
run daemon proxy "ftp://$NOS" publish subdomain list --json
refused "subdomain list, another scheme" "Error: K2_CONNECT_BASE must start with https:// (got: ftp://$NOS)"
# The default base and an https one pass the check; the daemon makes the call.
run daemon proxy - publish subdomain list --json
[ "$RC" = 0 ] && ok "default base: exit 0" || bad "default base: exit $RC (stderr: $(printf %q "$ERR"))"
reached "default base: the daemon made the call" "^daemon POST /cli/connect/proxy Bearer $OWNER\$"
run daemon proxy "HTTPS://$NOS" publish subdomain list --json
[ "$RC" = 0 ] && ok "HTTPS:// base: exit 0" || bad "HTTPS:// base: exit $RC (stderr: $(printf %q "$ERR"))"
reached "HTTPS:// base: the daemon made the call" "^daemon POST /cli/connect/proxy "

echo "== 2. a token goes over plain http only to this computer =="
printf '%s\n' "$REMOTE" >"$WORK/remote.tok"
RH="http://k2-remote.example.test"
WANT2="Error: plain http:// only to this computer; use https:// (got: $RH)"
run nodaemon proxy "$RH" publish subdomain list --json
refused "control plane over plain http, no daemon, proxy set" "$WANT2"
run nodaemon noproxy "$RH" publish subdomain list --json
refused "control plane over plain http, no daemon, no proxy" "$WANT2"
run daemon proxy "$RH" publish status --json
refused "control plane over plain http, daemon up" "$WANT2"
run daemon proxy "$RH" wiki publish --json
refused "wiki publish, control plane over plain http" "$WANT2"
RUN_STDIN="$WORK/remote.tok" run daemon proxy - talk "acme@$RH" --read-only --token-stdin
refused "talk acme@<plain http host>" "$WANT2"
RUN_STDIN="$WORK/remote.tok" run daemon proxy - read "acme@$RH" --token-stdin
refused "read acme@<plain http host>" "$WANT2"
# clone has its own gate and line (exit 3), from its own security pass.
RUN_STDIN="$WORK/remote.tok" run daemon proxy - clone preview acme --host "$RH" --token-stdin
[ "$RC" = 3 ] && ok "clone preview --host <plain http host>: exit 3" || bad "clone preview --host <plain http host>: exit $RC, want 3 (stderr: $(printf %q "$ERR"))"
case "$ERR" in *"isn't https, so K2 won't send your token or password there"*) ok "clone preview --host <plain http host>: its own line" ;; *) bad "clone preview --host <plain http host>: stderr $(printf %q "$ERR")" ;; esac
if [ -s "$LOG" ]; then bad "clone preview --host <plain http host>: a request was sent: $(tr '\n' ';' <"$LOG")"; else ok "clone preview --host <plain http host>: nothing sent"; fi
run daemon proxy - daemon restart --host "$RH"
refused "daemon restart --host <plain http host>" "$WANT2"
run daemon proxy - daemon restart --host "k2-remote.example.test"
refused "daemon restart --host <host with no scheme>" "Error: plain http:// only to this computer; use https:// (got: k2-remote.example.test)"
# Loopback is judged as written, never by resolving a name.
for h in "http://x.localhost:$DPORT" "http://127.0.0.1.evil.example:$DPORT" "http://127.1:$DPORT"; do
    run daemon proxy - daemon restart --host "$h"
    refused "daemon restart --host $h" "Error: plain http:// only to this computer; use https:// (got: $h)"
done
run nodaemon noproxy "http://x.localhost:$CPPORT" publish subdomain list --json
refused "control plane on http://x.localhost" "Error: plain http:// only to this computer; use https:// (got: http://x.localhost:$CPPORT)"
RUN_STDIN="$WORK/remote.tok" run daemon proxy - talk "acme@http://x.localhost:$DPORT" --read-only --token-stdin
refused "talk acme@http://x.localhost" "Error: plain http:// only to this computer; use https:// (got: http://x.localhost:$DPORT)"
RUN_STDIN="$WORK/remote.tok" run daemon proxy - remote logout --host "http://x.localhost:$DPORT" --token-stdin
refused "remote logout --host http://x.localhost (its own line)" "k2 remote logout: plain http:// only to this computer; use https://"
# Plain http to this computer still carries the token.
run nodaemon noproxy "$CP" publish subdomain list --json
[ "$RC" = 0 ] && ok "control plane on loopback http: exit 0" || bad "control plane on loopback http: exit $RC (stderr: $(printf %q "$ERR"))"
reached "control plane on loopback http: the call went with the tunnel token" "^cp GET /subdomains Bearer $TUNNEL\$"
run nodaemon noproxy "http://localhost:$CPPORT" publish subdomain list --json
reached "control plane on http://localhost: the call went" "^cp GET /subdomains Bearer $TUNNEL\$"
RUN_STDIN="$WORK/remote.tok" run daemon proxy - talk "acme@http://127.0.0.1:$DPORT" --read-only --token-stdin
reached "talk acme@<loopback http>: the remote token reached it" "^daemon [A-Z]+ /cli/[^ ]+ Bearer $REMOTE\$"
run daemon proxy - daemon restart --host "http://127.0.0.1:$DPORT"
reached "daemon restart --host <loopback http>: sent" "^daemon POST /cli/daemon/restart Bearer $OWNER\$"
run daemon proxy - daemon restart --host "http://localhost:$DPORT"
reached "daemon restart --host http://localhost: sent" "^daemon POST /cli/daemon/restart Bearer $OWNER\$"
run daemon proxy - daemon restart --host "http://[::1]:$CLOSED"
if [ "$ERR" != "${ERR#Error: plain http}" ]; then bad "daemon restart --host http://[::1]: refused: $(printf %q "$ERR")"; else ok "daemon restart --host http://[::1]: not refused (exit $RC)"; fi

echo "== 3. local traffic never goes through a proxy =="
run nodaemon proxy "$CP" publish subdomain list --json
[ "$RC" = 0 ] && ok "control plane on 127.0.0.1, proxy set: exit 0" || bad "control plane on 127.0.0.1, proxy set: exit $RC (stderr: $(printf %q "$ERR"))"
reached "control plane on 127.0.0.1, proxy set: the call went straight there" "^cp GET /subdomains Bearer $TUNNEL\$"
no_proxy_hit "control plane on 127.0.0.1"
run nodaemon proxy "http://localhost:$CPPORT" publish subdomain list --json
reached "control plane on localhost, proxy set: the call went straight there" "^cp GET /subdomains Bearer $TUNNEL\$"
no_proxy_hit "control plane on localhost"
run nodaemon proxy "https://k2-remote.example.test" publish subdomain list --json
reached "control plane on remote https: the person's proxy got the CONNECT" "^proxy CONNECT k2-remote.example.test:443 "
# A name that starts with 127 is a name, not loopback: https to it keeps
# the person's proxy.
run nodaemon proxy "https://127.evil.example" publish subdomain list --json
reached "control plane on https://127.evil.example: the person's proxy got the CONNECT" "^proxy CONNECT 127.evil.example:443 "
# The rule's other loopback forms, as _k2_direct_url reads them: the name
# localhost in any case and with one trailing dot, and [::1], go straight
# there (a port nobody listens on); an IPv4-mapped address is not this
# computer as written, so it keeps the person's proxy.
for h in "localhost:$CLOSED" "LOCALHOST:$CLOSED" "localhost.:$CLOSED" "[::1]:$CLOSED"; do
    run nodaemon proxy "https://$h" publish subdomain list --json
    no_proxy_hit "control plane on https://$h"
done
run nodaemon proxy "https://[::ffff:127.0.0.1]:$CLOSED" publish subdomain list --json
reached "control plane on https://[::ffff:127.0.0.1]: the person's proxy got the CONNECT" "^proxy CONNECT \[::ffff:(127\.0\.0\.1|7f00:1)\]:$CLOSED "

echo "== 4. K2_HOST on another computer over plain http: one warning per run =="
mkdir -p "$WORK/bin"
REAL_CURL="$(command -v curl)"
# Unroutable or unresolvable hosts below: curl gives up quickly (the last
# --connect-timeout and --max-time win).
printf '#!/bin/sh\nexec "%s" "$@" --connect-timeout 2 --max-time 3\n' "$REAL_CURL" >"$WORK/bin/curl"
chmod +x "$WORK/bin/curl"
warn_line() { printf "warning: k2 is sending this server's owner token to %s over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https." "$1"; }
# warned <label> <host> <count>: the warning for <host> is on stderr <count> times.
warned() {
    local n
    n="$(printf '%s\n' "$ERR" | grep -cxF "$(warn_line "$2")" || true)"
    [ "$n" = "$3" ] && ok "$1: the warning $3 time(s)" || bad "$1: the warning $n time(s), want $3 (stderr: $(printf %q "$ERR"))"
}
no_warning() {
    case "$ERR" in
        *"over plain http, unencrypted"*) bad "$1: warned: $(printf %q "$ERR")" ;;
        *) ok "$1: no warning" ;;
    esac
}
# curl must send x.localhost to loopback here, or the rows below mean nothing.
: >"$LOG"
"$REAL_CURL" -s -o /dev/null --max-time 5 --noproxy '*' "http://x.localhost:$DPORT/probe" || true
if grep -q '^daemon GET /probe ' "$LOG"; then ok "curl sends x.localhost to loopback"; else bad "curl did not send x.localhost to the stub: $(tr '\n' ';' <"$LOG")"; fi
RUN_ENV=(K2_HOST=x.localhost)
run daemon noproxy - publish subdomain create acme --target localhost:3000
n="$(grep -c "^daemon .* Bearer $OWNER\$" "$LOG" || true)"
[ "$n" -ge 2 ] && ok "K2_HOST=x.localhost: the command made $n daemon requests" || bad "K2_HOST=x.localhost: $n daemon requests, want 2 or more ($(tr '\n' ';' <"$LOG"); stderr: $(printf %q "$ERR"))"
warned "K2_HOST=x.localhost, several requests" x.localhost 1
run daemon noproxy - heartbeat list --json
reached "K2_HOST=x.localhost: heartbeat list sent" "^daemon [A-Z]+ /cli/"
warned "K2_HOST=x.localhost, heartbeat list" x.localhost 1
run daemon noproxy - preset list --json
warned "K2_HOST=x.localhost, a Python verb" x.localhost 1
run daemon noproxy - --help
no_warning "K2_HOST=x.localhost, --help"
[ ! -s "$LOG" ] && ok "--help sent nothing" || bad "--help sent: $(tr '\n' ';' <"$LOG")"
RUN_ENV=(K2_HOST=x.localhost K2_PLAIN_HTTP_OK=1)
run daemon noproxy - heartbeat list --json
reached "K2_PLAIN_HTTP_OK=1: heartbeat list sent" "^daemon [A-Z]+ /cli/"
no_warning "K2_PLAIN_HTTP_OK=1"
for h in 127.0.0.1 localhost; do
    RUN_ENV=(K2_HOST="$h")
    run daemon noproxy - heartbeat list --json
    reached "K2_HOST=$h: heartbeat list sent" "^daemon [A-Z]+ /cli/"
    no_warning "K2_HOST=$h"
done
for h in 100.64.0.1 100.127.255.254 fd7a:115c:a1e0::1 box.tail1234.ts.net; do
    RUN_ENV=(K2_HOST="$h" PATH="$WORK/bin:$PATH")
    run daemon noproxy - heartbeat list --json
    no_warning "K2_HOST=$h (Tailscale, as written)"
done
for h in x.ts.net.evil.com 100.128.0.1 192.0.2.10; do
    RUN_ENV=(K2_HOST="$h" PATH="$WORK/bin:$PATH")
    run daemon noproxy - heartbeat list --json
    warned "K2_HOST=$h" "$h" 1
done
RUN_ENV=(K2_HOST=https://k2-lan.example.test)
run daemon proxy - heartbeat list --json
reached "K2_HOST=https://k2-lan.example.test: through the person's proxy" "^proxy CONNECT k2-lan.example.test:$DPORT "
no_warning "K2_HOST=https://k2-lan.example.test"
printf 'pw-cbase-4444\n' >"$WORK/pw"
WANTPW="Error: a password never goes over plain http:// to another computer; use K2_HOST=https://<name> (got: http://x.localhost:$DPORT)"
RUN_ENV=(K2_HOST=x.localhost)
RUN_STDIN="$WORK/pw" run daemon noproxy - users add bob --password-stdin
refused "users add --password-stdin, K2_HOST=x.localhost" "$WANTPW"
RUN_ENV=(K2_HOST=x.localhost K2_PLAIN_HTTP_OK=1)
RUN_STDIN="$WORK/pw" run daemon noproxy - users add bob --password-stdin
refused "users add --password-stdin, K2_PLAIN_HTTP_OK=1" "$WANTPW"
RUN_ENV=(K2_HOST=box.tail1234.ts.net)
RUN_STDIN="$WORK/pw" run daemon noproxy - users add bob --password-stdin
refused "users add --password-stdin, a tailnet host" "Error: a password never goes over plain http:// to another computer; use K2_HOST=https://<name> (got: http://box.tail1234.ts.net:$DPORT)"
RUN_ENV=()
RUN_STDIN="$WORK/pw" run daemon noproxy - users add bob --password-stdin
reached "K2_HOST unset: users add --password-stdin is sent" "^daemon POST /cli/"

echo
echo "connect_base_and_proxy: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
