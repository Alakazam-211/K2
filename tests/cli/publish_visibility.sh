#!/usr/bin/env bash
# `k2 publish visibility|open|sessions` and `k2 publish run --dev`
# (prd-publish-visibility-and-dev-mode-v1 PV-7, PV-10, PV-12, DM-1, DM-3, DM-6).
#   1. No daemon: help names the verbs; usage errors exit 2 before any request;
#      a PIN on the command line is refused.
#   2. Stub daemon: a held card exits 2; a refusal exits 3.
#   3. Real headless daemon (temp HOME) and a stand-in control plane:
#      run --dev, ps, a PIN from stdin, public, open, sessions, idle stop.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_lacks() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-publish-vis-XXXXXX)"
STUB_PID=""
CP_PID=""
DAEMON_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    [ -n "$CP_PID" ] && kill "$CP_PID" 2>/dev/null || true
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    [ -n "${DEV_PORT:-}" ] && pkill -f -- "seen_dev_${DEV_PORT}" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN || true

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }

# ── 1. no daemon ─────────────────────────────────────────────────────
echo "== help + usage (no daemon) =="
HOME1="$WORK/h1"
mkdir -p "$HOME1"
nod() { env HOME="$HOME1" K2_PORT=1 K2_HOOK_TOKEN=test-token K2_PROJECT_PATH="$HOME1" "$K2_CLI" "$@"; }
help_out="$(nod publish --help 2>&1)"
for word in "k2 publish visibility <name>" "k2 publish open <name>" "k2 publish sessions <name>" "--dev" "k2 study publish-dev" "k2 study publish-visibility" "skin_retired"; do
    assert_contains "help: $word" "$help_out" "$word"
done
run_usage() { # <label> <want-exit> <want-text> args...
    local label="$1" want="$2" text="$3"; shift 3
    local out rc=0
    out="$(nod "$@" 2>&1)" || rc=$?
    assert_eq "$label exits $want" "$rc" "$want"
    assert_contains "$label says" "$out" "$text"
}
run_usage "--pin refused" 2 "shell history" publish visibility docs pin --pin 123456
run_usage "--pin=x refused" 2 "shell history" publish visibility docs pin --pin=123456
run_usage "no name" 2 "usage: k2 publish visibility" publish visibility
run_usage "bad mode" 2 "private, pin or public" publish visibility docs secret
run_usage "bad pin-days" 2 "--pin-days" publish visibility docs pin --pin-days many
run_usage "open no name" 2 "usage: k2 publish open" publish open
run_usage "sessions no name" 2 "usage: k2 publish sessions" publish sessions
run_usage "dev + skin" 2 "needs --cmd" publish run web --skin --dev --port 5173
run_usage "dev + no-tunnel" 2 "--no-tunnel" publish run web --cmd "x" --dev --no-tunnel --port 5173
run_usage "idle without dev" 2 "go with --dev" publish run web --cmd "x" --idle 30m --port 5173
run_usage "public dev, no terminal" 2 "--yes" publish visibility web public --reason demo --dev-is-public </dev/null

study_vis="$(nod study publish-visibility)"
assert_contains "study: visibility title" "$study_vis" "k2 study publish-visibility"
[ "$(printf '%s\n' "$study_vis" | wc -l)" -le 80 ] && ok "study: visibility at most 80 lines" || bad "study: visibility over 80 lines"
study_dev="$(nod study publish-dev)"
assert_contains "study: dev title" "$study_dev" "k2 study publish-dev"
assert_contains "study: dev Vite" "$study_dev" "allowedHosts"
assert_contains "study: dev Next" "$study_dev" "allowedDevOrigins"
[ "$(printf '%s\n' "$study_dev" | wc -l)" -le 100 ] && ok "study: dev at most 100 lines" || bad "study: dev over 100 lines"

# ── 2. stub daemon: exit codes ───────────────────────────────────────
echo "== stub daemon: held exit 2, refused exit 3 =="
SPORT="$(free_port)"
cat >"$WORK/stub.py" <<'PY'
import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        if body.get("label") == "held":
            code, out = 403, {"status": "held", "code": "public_needs_person", "label": "held", "message": "K2 put a card on your Thread.", "request": "pr_1"}
        elif body.get("label") == "pinheld":
            assert body.get("check") is True and "pin" not in body, body
            code, out = 403, {"status": "held", "code": "pin_needs_person", "label": "pinheld", "message": "the person types the PIN into K2's own form."}
        else:
            code, out = 403, {"error": "an agent tests on the local port", "code": "person_only"}
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
rc=0; out="$(stub publish visibility held public --reason launch 2>&1)" || rc=$?
assert_eq "held public exits 2" "$rc" "2"
assert_contains "held public says the card" "$out" "card on your Thread"
# The agent's PIN attempt stops at the card: no PIN is read, so stdin stays unread.
rc=0; out="$(printf 'never-read\n' | stub publish visibility pinheld pin 2>&1)" || rc=$?
assert_eq "held PIN exits 2" "$rc" "2"
assert_contains "held PIN says the form" "$out" "K2's own form"
assert_lacks "held PIN echoes no PIN" "$out" "never-read"
rc=0; out="$(stub publish open docs 2>&1)" || rc=$?
assert_eq "person_only exits 3" "$rc" "3"
assert_contains "person_only says" "$out" "local port"
kill "$STUB_PID" 2>/dev/null || true
STUB_PID=""

# ── 3. real headless daemon ──────────────────────────────────────────
echo "== real daemon, real gate: run --dev, visibility, open, sessions, idle stop =="
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || { echo "FAIL: k2-daemon not built (cargo build -p k2-daemon)" >&2; exit 1; }

# A stand-in for the control plane: a Pro plan and a list of nested labels.
CP_PORT="$(free_port)"
cat >"$WORK/cp.py" <<'PY'
import http.server, json, sys
labels = {}
class H(http.server.BaseHTTPRequestHandler):
    def _send(self, code, out):
        data = json.dumps(out).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def _body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"{}")
    def do_GET(self):
        rows = [{"label": "rosson", "primary": True, "connected": True, "tier": "pro"}]
        rows += [{"label": k, "target": v} for k, v in labels.items()]
        self._send(200, {"subdomains": rows})
    def do_POST(self):
        b = self._body()
        labels[b["label"]] = b["target"]
        self._send(200, {"ok": True})
    def do_PUT(self):
        b = self._body()
        labels[self.path.rsplit("/", 1)[-1]] = b["target"]
        self._send(200, {"ok": True})
    def do_DELETE(self):
        labels.pop(self.path.rsplit("/", 1)[-1], None)
        self._send(200, {"ok": True})
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
python3 "$WORK/cp.py" "$CP_PORT" &
CP_PID=$!
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$CP_PORT/" >/dev/null 2>&1 && break; sleep 0.05; done

SB="$WORK/sb"
# Own TMPDIR / XDG_RUNTIME_DIR: the daemon's private runtime dir must never be
# the real one (2026-10-14 incident).
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run"
printf '#!/bin/sh\nexec /bin/cat\n' > "$SB/shim/claude"
chmod +x "$SB/shim/claude"
printf '{"token":"tok_test","subdomain":"rosson","e2e":true}' > "$SB/.k2/tunnel.json"
chmod 600 "$SB/.k2/tunnel.json"
# The frpc stand-in: the tunnel "connects" and the refresh loop starts.
mkdir -p "$SB/bin"
printf '#!/bin/sh\necho "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success"\nwhile :; do sleep 5; done\n' > "$SB/bin/frpc"
chmod +x "$SB/bin/frpc"
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
    HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    K2_PUBLISH_HELPER_EXIT_WITH_PARENT=1 K2_CONNECT_BASE="http://127.0.0.1:$CP_PORT" \
    K2_PUBLISH_DEV_IDLE_TICK_MS=200 K2_E2E_SELF_SIGNED=1 K2_TUNNEL_LEASE=off \
    K2_CERT_BROKER_URL=http://127.0.0.1:1 PATH="$SB/bin:$PATH" \
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
[ "$phase" = "ready" ] || { echo "FAIL: daemon never reached ready" >&2; tail -30 "$SB/daemon.log" >&2; exit 1; }

features="$(curl -s "http://127.0.0.1:$DPORT/boot-status?token=$DTOKEN")"
assert_contains "boot-status: publish-dev-v1" "$features" "publish-dev-v1"
assert_contains "boot-status: site-visibility-v1" "$features" "site-visibility-v1"

WSP="$SB/devws"
resp="$(curl -s -X POST "http://127.0.0.1:$DPORT/cli/workspace/create?token=$DTOKEN&path=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$WSP")" --data-raw '')"
assert_contains "workspace registered" "$resp" "devws"
[ -d "$WSP" ] || { echo "FAIL: workspace/create made no folder: $resp" >&2; exit 1; }
resp="$(curl -s "http://127.0.0.1:$DPORT/cli/publish/list?token=$DTOKEN&project=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$WSP")")"
assert_contains "the daemon knows the workspace" "$resp" "services"

real() {
    env HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" K2_HOOK_TOKEN="$DTOKEN" \
        K2_PROJECT_PATH="$WSP" "$K2_CLI" "$@"
}

# The routes are POST-only where they change something.
for route in open sessions/revoke; do
    code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DPORT/cli/publish/$route?token=$DTOKEN&label=web")"
    assert_eq "GET /cli/publish/$route is 405" "$code" "405"
done

DEV_PORT="$(free_port)"
# A stand-in for a dev server: the page says the live file's text, and an upgraded
# socket pushes when that file changes, as a hot-reload client expects.
cat >"$WSP/server.py" <<PY
# seen_dev_${DEV_PORT}
import http.server, os, socketserver, time
open("seen-env.txt", "w").write("\n".join("%s=%s" % kv for kv in sorted(os.environ.items())))
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_GET(self):
        open("seen-requests.txt", "a").write(self.path + " " + str(self.headers.get("Cookie")) + "\n")
        if self.headers.get("Upgrade", "").lower() == "websocket":
            self.send_response(101)
            self.send_header("Upgrade", "websocket")
            self.send_header("Connection", "Upgrade")
            self.end_headers()
            self.wfile.flush()
            seen = open("page.txt").read()
            while True:
                now = open("page.txt").read()
                if now != seen:
                    seen = now
                    self.wfile.write(("reload:" + now.strip() + "\n").encode())
                    self.wfile.flush()
                time.sleep(0.05)
        body = open("page.txt", "rb").read()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
S((os.environ["HOST"], int(os.environ["PORT"])), H).serve_forever()
PY
echo first > "$WSP/page.txt"
# New --skin sites are gone; the words point at what exists.
rc=0; out="$(real publish run fresh --skin --port "$(free_port)" 2>&1)" || rc=$?
assert_eq "new --skin site exits 3" "$rc" "3"
assert_contains "new --skin site says" "$out" "--skin no longer makes new sites"
assert_lacks "never offers what doesn't exist" "$out" "adopt-skin"

rc=0; out="$(real publish run web --cmd "python3 server.py" --port "$DEV_PORT" --dev --mem 2G --cpus 1 2>&1)" || rc=$?
assert_eq "run --dev exit 0" "$rc" "0"
assert_contains "run --dev warns" "$out" "source maps"
assert_contains "run --dev says the lane" "$out" "live folder, not walled"
assert_contains "run --dev URL" "$out" "https://web.rosson.k2.dev"
env_seen=""
for _ in $(seq 1 50); do
    [ -s "$WSP/seen-env.txt" ] && env_seen="$(cat "$WSP/seen-env.txt")" && break
    sleep 0.1
done
assert_contains "dev env: HOST" "$env_seen" "HOST=127.0.0.1"
assert_contains "dev env: PORT" "$env_seen" "PORT=$DEV_PORT"
assert_contains "dev env: public host" "$env_seen" "K2_DEV_PUBLIC_HOST=web.rosson.k2.dev"
assert_contains "dev env: public protocol" "$env_seen" "K2_DEV_PUBLIC_PROTOCOL=wss"

ps_out="$(real publish ps)"
assert_contains "ps VISIBILITY column" "$ps_out" "VISIBILITY"
assert_contains "ps private, dev" "$ps_out" "private, dev"
ps_long="$(real publish ps --long)"
assert_contains "ps --long caps" "$ps_long" "caps:"
case "$(uname -s)" in
    Darwin) assert_contains "ps --long: no caps on macOS" "$ps_long" "no caps (macOS has none)" ;;
esac

vis="$(real publish visibility web)"
assert_contains "visibility shows it" "$vis" "web: private, dev"
assert_contains "visibility says how to open it" "$vis" "k2 publish open web"

# A PIN from stdin: in, and never on the wire back.
rc=0; out="$(printf 'correct-horse\n' | real publish visibility web pin 2>&1)" || rc=$?
assert_eq "pin from stdin exit 0" "$rc" "0"
assert_contains "pin mode" "$out" "web: pin, dev"
assert_contains "pin warns about a dev server" "$out" "person with the PIN"
assert_lacks "pin never echoed" "$out" "correct-horse"
stored="$(python3 - "$SB" <<'PY'
import glob, sqlite3, sys, os
for name in ("k2.db", "k2so.db"):
    p = os.path.join(sys.argv[1], ".k2", name)
    if os.path.exists(p):
        print(sqlite3.connect(p).execute("select pin_hash from site_visibility where label='web'").fetchone()[0])
        break
PY
)"
assert_contains "pin stored hashed" "$stored" "\$argon2id\$"
assert_lacks "pin not stored plain" "$stored" "correct-horse"
rc=0; out="$(printf 'abc\n' | real publish visibility web pin 2>&1)" || rc=$?
assert_eq "short PIN exits 2" "$rc" "2"
assert_contains "short PIN says" "$out" "6 to 64"
rc=0; out="$(printf '12345678\n' | real publish visibility no-such-site pin 2>&1)" || rc=$?
assert_eq "unknown label exits 3" "$rc" "3"

# Public dev: loud, on purpose.
rc=0; out="$(real publish visibility web public --reason demo 2>&1)" || rc=$?
assert_eq "public dev needs the flag" "$rc" "2"
assert_contains "public dev says why" "$out" "--dev-is-public"
rc=0; out="$(real publish visibility web public --reason demo --dev-is-public --yes 2>&1 </dev/null)" || rc=$?
assert_eq "public dev with the flag exit 0" "$rc" "0"
assert_contains "public dev warns" "$out" "source maps"
rc=0; out="$(real publish visibility web public 2>&1)" || rc=$?
assert_eq "public needs a reason" "$rc" "2"

# Back to private, past the "is public" warning.
rc=0; out="$(real publish visibility web private 2>&1 </dev/null)" || rc=$?
assert_eq "public dev → private exit 0 (no parser warning for a dev site)" "$rc" "0"

# ── through the real gate ────────────────────────────────────────────
E2E="$(cat "$SB/.k2/tunnel-https.port")"
SITE="web.rosson.k2.dev"
visit() { # <path> [curl args...]: the whole answer, head first
    local path="$1"; shift
    curl -sS -k -i --max-time 30 --resolve "$SITE:$E2E:127.0.0.1" -H 'Accept: text/html' "$@" "https://$SITE:$E2E$path" | tr -d '\r'
}
cookie_of() { # <name>: from an answer on stdin
    grep -i "^set-cookie: $1=" | head -1 | sed 's/^[^:]*: //; s/;.*//'
}
seen_requests() { [ -f "$WSP/seen-requests.txt" ] && wc -l < "$WSP/seen-requests.txt" | tr -d ' ' || echo 0; }

# PV-2 / DM-2: no session, no bytes to the dev server; the answer says "dev".
before="$(seen_requests)"
r="$(visit /)"
assert_contains "private: K2's page" "$r" "HTTP/1.1 401"
assert_contains "private: says dev (DM-4)" "$r" "X-K2-Mode: dev"
assert_contains "private: noindex (DM-4)" "$r" "X-Robots-Tag: noindex"
assert_contains "private: 'Dev server' in the title" "$r" "Dev server"
r="$(visit /socket -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==')"
assert_contains "socket with no session: 401" "$r" "HTTP/1.1 401"
assert_eq "nothing reached the dev server" "$(seen_requests)" "$before"

# PV-5: one-time link → cookie → the site, with K2's cookie kept from it.
rc=0; out="$(real publish open web --no-browser --json 2>/dev/null)" || rc=$?
assert_eq "open exit 0" "$rc" "0"
assert_contains "open: a one-time link" "$out" "/_k2/site/open?code="
link_path="$(printf '%s' "$out" | python3 -c 'import json,sys,urllib.parse as u; p=u.urlparse(json.load(sys.stdin)["url"]); print(p.path+"?"+p.query)')"
r="$(visit "$link_path")"
assert_contains "the link redirects" "$r" "HTTP/1.1 30"
OWNER_COOKIE="$(printf '%s' "$r" | cookie_of __Host-k2_site)"
[ -n "$OWNER_COOKIE" ] && ok "the link sets the session cookie" || bad "no session cookie in: $r"
r="$(visit "$link_path")"
assert_lacks "a used link doesn't work twice" "$r" "HTTP/1.1 30"
r="$(visit / -H "Cookie: $OWNER_COOKIE")"
assert_contains "signed in: the site" "$r" "first"
assert_contains "signed in: still says dev" "$r" "X-K2-Mode: dev"
last="$(tail -1 "$WSP/seen-requests.txt")"
assert_lacks "K2's cookie never reaches the site" "$last" "__Host-k2_site"

# DM-1 / DM-2: an edit in the live folder reaches a hot-reload socket through the
# gate, for the owner now and for a friend with the PIN below.
cat >"$WORK/hmr.py" <<'PY'
import socket, ssl, sys
port, host, cookie, path = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
s = ctx.wrap_socket(socket.create_connection(("127.0.0.1", port), timeout=20), server_hostname=host)
s.sendall(("GET /socket HTTP/1.1\r\nHost: %s\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nCookie: %s\r\n\r\n" % (host, cookie)).encode())
buf = b""
while b"\r\n\r\n" not in buf:
    buf += s.recv(4096)
head = buf.decode(errors="replace")
print(head.split("\r\n")[0])
print("dev-header" if "x-k2-mode: dev" in head.lower() else "no-dev-header")
sys.stdout.flush()
text = open(path).read()
open(path, "w").write(text.replace("first", "second").replace("third", "fourth"))
rest = buf.split(b"\r\n\r\n", 1)[1]
while b"\n" not in rest:
    rest += s.recv(4096)
print(rest.decode(errors="replace").strip())
PY
hmr="$(python3 "$WORK/hmr.py" "$E2E" "$SITE" "$OWNER_COOKIE" "$WSP/page.txt" 2>&1)"
assert_contains "owner's socket: 101" "$hmr" "101"
assert_contains "owner's socket: says dev" "$hmr" "dev-header"
assert_contains "owner's socket: the edit arrives" "$hmr" "reload:second"
r="$(visit / -H "Cookie: $OWNER_COOKIE")"
assert_contains "the live folder, not a copy" "$r" "second"

# DM-3: pair programming. A PIN, a friend, the notice, hot reload, then cut off.
printf 'pair-programming\n' | real publish visibility web pin >/dev/null 2>&1
r="$(visit /)"
assert_contains "friend: K2's PIN page" "$r" "needs a PIN"
r="$(visit /_k2/site/pin -H "Origin: https://$SITE" --data 'pin=not-it&next=%2F')"
assert_contains "friend: wrong PIN" "$r" "HTTP/1.1 401"
r="$(visit /_k2/site/pin -H "Origin: https://$SITE" --data 'pin=pair-programming&next=%2F')"
FRIEND_COOKIE="$(printf '%s' "$r" | cookie_of __Host-k2_pin)"
[ -n "$FRIEND_COOKIE" ] && ok "friend: the PIN sets a cookie" || bad "no PIN cookie in: $r"
r="$(visit / -H "Cookie: $FRIEND_COOKIE")"
assert_contains "friend: the dev notice first (DM-4)" "$r" "This is a dev server for web"
r="$(visit '/_k2/site/continue?next=%2F' -H "Cookie: $FRIEND_COOKIE")"
assert_contains "friend: Continue" "$r" "HTTP/1.1 30"
r="$(visit / -H "Cookie: $FRIEND_COOKIE")"
assert_contains "friend: the site" "$r" "second"
echo third > "$WSP/page.txt"
hmr="$(python3 "$WORK/hmr.py" "$E2E" "$SITE" "$FRIEND_COOKIE" "$WSP/page.txt" 2>&1)"
assert_contains "friend's socket: the edit arrives" "$hmr" "reload:fourth"
real publish visibility web private >/dev/null 2>&1
r="$(visit / -H "Cookie: $FRIEND_COOKIE")"
assert_contains "back to private: the friend is cut off" "$r" "HTTP/1.1 401"
rc=0; out="$(real publish open web --path //evil.example --no-browser 2>&1)" || rc=$?
assert_eq "open refuses an off-site path" "$rc" "2"

sess="$(real publish sessions web)"
assert_contains "sessions: the owner's, from the link" "$sess" "owner-token"
out="$(real publish sessions web --revoke-all)"
assert_contains "revoke-all" "$out" "Ended 1 session"
assert_contains "sessions: none left" "$(real publish sessions web)" "Nobody is signed in"
r="$(visit / -H "Cookie: $OWNER_COOKIE")"
assert_contains "a revoked session is out" "$r" "HTTP/1.1 401"

# DM-6: with a short limit written straight to the row (the real minimum is 5
# minutes), it stops, says so, and a visitor who passes the gate starts it again.
python3 - "$SB" <<'PY'
import os, sqlite3, sys
db = sqlite3.connect(os.path.join(sys.argv[1], ".k2", "k2.db"), timeout=10)
db.execute("UPDATE site_visibility SET idle_secs = 2 WHERE label = 'web'")
db.commit()
PY
idle=""
for _ in $(seq 1 80); do
    idle="$(real publish ps)"
    printf '%s' "$idle" | grep -Fq "idle-stopped" && break
    sleep 0.2
done
assert_contains "ps says idle-stopped" "$idle" "idle-stopped"
r="$(visit /)"
assert_contains "a visitor who fails the gate gets K2's page" "$r" "HTTP/1.1 401"
sleep 1
assert_contains "and never wakes it" "$(real publish ps)" "idle-stopped"
# Owner's session from above was on a mode since changed: a fresh link.
out="$(real publish open web --no-browser --json 2>/dev/null)"
link_path="$(printf '%s' "$out" | python3 -c 'import json,sys,urllib.parse as u; p=u.urlparse(json.load(sys.stdin)["url"]); print(p.path+"?"+p.query)')"
r="$(visit "$link_path")"
OWNER_COOKIE="$(printf '%s' "$r" | cookie_of __Host-k2_site)"
r="$(visit / -H "Cookie: $OWNER_COOKIE")"
assert_contains "a visitor who passes: K2's starting page" "$r" "Starting the dev server"
woke=""
for _ in $(seq 1 100); do
    woke="$(visit / -H "Cookie: $OWNER_COOKIE")"
    printf '%s' "$woke" | grep -Fq "HTTP/1.1 200" && break
    sleep 0.1
done
assert_contains "and the site comes back" "$woke" "HTTP/1.1 200"
python3 - "$SB" <<'PY'
import os, sqlite3, sys
db = sqlite3.connect(os.path.join(sys.argv[1], ".k2", "k2.db"), timeout=10)
db.execute("UPDATE site_visibility SET idle_secs = 3600 WHERE label = 'web'")
db.commit()
PY
real publish stop web >/dev/null 2>&1
r="$(visit / -H "Cookie: $OWNER_COOKIE")"
assert_lacks "a site someone stopped doesn't wake" "$r" "Starting the dev server"
rc=0; out="$(real publish start web 2>&1)" || rc=$?
assert_eq "start exit 0" "$rc" "0"
assert_contains "start: running again" "$out" "running"

# Back to a normal service: --dev off, and it stays private.
real publish stop web >/dev/null 2>&1
rc=0; out="$(real publish run web --cmd "python3 server.py" --port "$DEV_PORT" 2>&1)" || rc=$?
assert_eq "plain re-run exit 0" "$rc" "0"
[ "$rc" = 0 ] || echo "    plain re-run said: $out" >&2
assert_lacks "plain re-run has no dev warning" "$out" "source maps"
assert_contains "ps: normal and private" "$(real publish ps)" "private"
assert_lacks "ps: no longer dev" "$(real publish ps)" "private, dev"

real publish stop web >/dev/null 2>&1 || true
real publish rm web >/dev/null 2>&1 || true

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
