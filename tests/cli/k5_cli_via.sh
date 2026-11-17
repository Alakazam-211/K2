#!/usr/bin/env bash
# The Rust front's two hand-over variables, read by the prelude.
#
#   K5_CLI_VIA=rust     every daemon request carries `X-K5-Cli-Via:
#                       fallthrough`, over TCP and the cell socket alike.
#                       Unset, empty or any other value: no request does.
#                       The Connect control plane is not the daemon: the
#                       CLI's own call to it never carries the header.
#                       The calls that once ran curl themselves (the
#                       stdin-body POST, swap-canonical, the migrate and
#                       vault calls on vault.sock) carry it too, and
#                       nothing else about them changes: same request, same
#                       curl argv but for the header.
#   K5_CLI_NOTICE_DONE  set non-empty: _K5_NOTICE_DONE=1, where the k2-to-k5
#                       notice will look. Empty or unset: it stays empty.
#
# A python stub daemon (TCP, a cell socket and vault.sock) logs, per
# request, the transport, method, path, the X-K5-Cli-Via values and a
# digest of the rest (headers it reads, body hash, whether a token came).
# No token values are logged. No live K2, HOME is a temp dir.
# bash-only: pins the bash side of the Rust hand-over, read from a bash -x trace.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_BASH_SOURCE"
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

# Short paths: sun_path is ~104 bytes on macOS, and vault.sock lives in
# $HOME/.k2.
WORK="$(mktemp -d /tmp/k2kv-XXXXXX)"
SOCK="/tmp/k2kv-$$.sock"
VSOCK="$WORK/home/.k2/vault.sock"
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

python3 - "$WORK" "$SOCK" "$VSOCK" <<'PY' &
import hashlib, json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work, sock_path, vsock_path = sys.argv[1], sys.argv[2], sys.argv[3]
class H(BaseHTTPRequestHandler):
    transport = "tcp"
    def _go(self):
        n = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(n) if n else b""
        u = urllib.parse.urlparse(self.path)
        rec = {
            "transport": self.transport,
            "method": self.command,
            "path": u.path,
            "via": self.headers.get_all("X-K5-Cli-Via") or [],
            "content_type": self.headers.get("Content-Type") or "",
            "tty": self.headers.get("X-K2-Tty") or "",
            "has_token": "token" in urllib.parse.parse_qs(u.query),
            "has_auth": bool(self.headers.get("Authorization")),
            "body_sha": hashlib.sha256(body).hexdigest(),
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
class V(H):
    transport = "vault"
    def address_string(self): return "vault"
class UnixHTTP(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
class TcpHTTP(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True
us = UnixHTTP(sock_path, U)
threading.Thread(target=us.serve_forever, daemon=True).start()
vs = UnixHTTP(vsock_path, V)
threading.Thread(target=vs.serve_forever, daemon=True).start()
srv = TcpHTTP(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && [ -S "$SOCK" ] && [ -S "$VSOCK" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] && [ -S "$SOCK" ] && [ -S "$VSOCK" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
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

echo "== the calls that once ran curl themselves =="
# A curl shim first on PATH logs each argv (bash %q, one call per line),
# then runs the real curl.
REAL_CURL="$(command -v curl)"
mkdir -p "$WORK/shim"
cat >"$WORK/shim/curl" <<SHIM
#!/usr/bin/env bash
printf '%q ' "\$@" >>'$WORK/argv.log'
echo >>'$WORK/argv.log'
exec '$REAL_CURL' "\$@"
SHIM
chmod +x "$WORK/shim/curl"
HDR_ARGV="$(printf '%q ' -H "X-K5-Cli-Via: fallthrough")"
# An owner terminal: no session, so the vault calls go to vault.sock.
owner_shim() { clean_env PATH="$WORK/shim:$PATH" "$@"; }
# site_run <tag> <stdin> <transport> <path> <argv match> <cmd…>: run once,
# keep the matching requests (via split off) and curl argvs.
site_run() {
    local tag="$1" input="$2" transport="$3" path="$4" match="$5"; shift 5
    : >"$WORK/requests.log"; : >"$WORK/argv.log"
    printf '%s' "$input" | owner_shim "$@" >/dev/null 2>&1 || true
    python3 - "$WORK/requests.log" "$transport" "$path" "$WORK/$tag" <<'PY'
import json, sys
log, transport, path, out = sys.argv[1:5]
rows = [json.loads(l) for l in open(log)]
hits = [r for r in rows if r["transport"] == transport and r["path"] == path]
with open(out + ".via", "w") as f:
    for r in hits:
        f.write((",".join(r.pop("via")) or "-") + "\n")
with open(out + ".req", "w") as f:
    for r in hits:
        f.write(json.dumps(r, sort_keys=True) + "\n")
PY
    grep -F -- "$match" "$WORK/argv.log" >"$WORK/$tag.argv" || true
}
# site <label> <stdin> <transport> <path> <argv match> <max-time> <cmd…>
site() {
    local label="$1" input="$2" transport="$3" path="$4" match="$5" mt="$6"; shift 6
    site_run off "$input" "$transport" "$path" "$match" "$@"
    site_run on "$input" "$transport" "$path" "$match" K5_CLI_VIA=rust "$@"
    if [ ! -s "$WORK/off.req" ] || [ ! -s "$WORK/off.argv" ]; then
        bad "$label: no request or no curl argv seen ($(tail -2 "$WORK/requests.log" | tr '\n' ' '))"
        return
    fi
    if [ -z "$(grep -vx -- - "$WORK/off.via")" ]; then
        ok "$label: no header when VIA is unset"
    else
        bad "$label: a header without VIA ($(tr '\n' ' ' <"$WORK/off.via"))"
    fi
    if [ -s "$WORK/on.via" ] && [ -z "$(grep -vx fallthrough "$WORK/on.via")" ]; then
        ok "$label: X-K5-Cli-Via: fallthrough with VIA=rust"
    else
        bad "$label: with VIA=rust got '$(tr '\n' ' ' <"$WORK/on.via")'"
    fi
    if cmp -s "$WORK/off.req" "$WORK/on.req"; then
        ok "$label: the request is otherwise the same"
    else
        bad "$label: the request changed ($(diff "$WORK/off.req" "$WORK/on.req" | tr '\n' ' '))"
    fi
    # Each VIA=rust argv holds the header once; without it, it is the
    # unset run's argv.
    local on_less n_hdr
    n_hdr="$(grep -cF -- "$HDR_ARGV" "$WORK/on.argv" || true)"
    on_less="$(while IFS= read -r l; do printf '%s\n' "${l/"$HDR_ARGV"/}"; done <"$WORK/on.argv")"
    if [ "$n_hdr" = "$(wc -l <"$WORK/on.argv" | tr -d ' ')" ] && [ "$on_less" = "$(cat "$WORK/off.argv")" ]; then
        ok "$label: curl argv is the same but for the header"
    else
        bad "$label: curl argv changed (off: $(cat "$WORK/off.argv") on: $(cat "$WORK/on.argv"))"
    fi
    if grep -qF -- "--max-time $mt " "$WORK/off.argv"; then
        ok "$label: still --max-time $mt"
    else
        bad "$label: --max-time $mt missing ($(cat "$WORK/off.argv"))"
    fi
}
site "cli_post_json_stdin (agent instances hire)" '{"team":"acme"}' tcp /cli/agent-instances/hire "-K /dev/fd/3" 30 \
    "$K2_CLI" agent instances hire -
site "workspace swap-canonical" "" tcp /cli/workspace/swap-canonical "--max-time 120" 120 \
    "$K2_CLI" workspace swap-canonical "$WORK/proj" --agent claude
site "migrate GET on vault.sock (migrate status)" "" vault /cli/migrate/secrets/status "/cli/migrate/secrets/status" 600 \
    "$K2_CLI" migrate status
site "migrate POST on vault.sock (import-secrets begin)" "" vault /cli/migrate/secrets/begin "/cli/migrate/secrets/begin" 600 \
    "$K2_CLI" migrate import-secrets begin
site "vault POST on vault.sock (recovery show-once's confirm)" "" vault /cli/vault/confirm/start "/cli/vault/confirm/start" 180 \
    "$K2_CLI" vault recovery show-once
site "vault GET on vault.sock (confirm-password set)" "" vault /cli/vault/status "/cli/vault/status" 180 \
    "$K2_CLI" vault confirm-password set

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
