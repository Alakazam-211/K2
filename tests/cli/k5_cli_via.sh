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
#   K5_CLI_WOULD_REFUSE=1, with K5_CLI_VIA=rust: every daemon request also
#                       carries `X-K5-Would-Refuse: 1` (the front's passport
#                       rule, count-only). Any other value, or without
#                       VIA=rust: no request does. Never to the control
#                       plane.
# With K5_CLI_VIA=rust every daemon request also carries
#   X-K5-Cli-Noun: <the router's first word, as typed>, `other` when that
#                       word is not [a-z][a-z0-9-]{0,31};
#   X-K5-Owner-Source: disk, only when there is no passport and the call
#                       runs on the owner token read from disk.
# Without VIA none of the four goes anywhere.
#
# A python stub daemon (TCP, a cell socket and vault.sock) logs, per
# request, the transport, method, path, the four headers' values and a
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
            "refuse": self.headers.get_all("X-K5-Would-Refuse") or [],
            "noun": self.headers.get_all("X-K5-Cli-Noun") or [],
            "owner": self.headers.get_all("X-K5-Owner-Source") or [],
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
        -u K5_CLI_WOULD_REFUSE \
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
    python3 - "$WORK/requests.log" "$1" "$2" "${FIELD:-via}" <<'PY'
import json, sys
log, transport, path, field = sys.argv[1:5]
rows = [json.loads(l) for l in open(log)]
hits = [r for r in rows if r["transport"] == transport and r["path"] == path]
if not hits:
    sys.exit("no %s request to %s (log: %s)" % (transport, path, rows[-3:]))
for r in hits:
    print(",".join(r[field]) or "-")
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

# one <transport> <via setting> <want>: a GET and a form POST. With the
# header, the noun is the first word only (`heartbeat`, not `heartbeat
# list`); without it, no noun. A session has a passport: never
# X-K5-Owner-Source. The via setting is `unset` or `=<value>`.
one() {
    local transport="$1" via="$2" want="$3" pre=() get_noun=- post_noun=-
    case "$via" in
        unset) pre=() ;;
        =*) pre=(K5_CLI_VIA="${via#=}") ;;
    esac
    [ "$want" = - ] || { get_noun=heartbeat; post_noun=msg; }
    : >"$WORK/requests.log"
    "$transport" ${pre[@]+"${pre[@]}"} "$K2_CLI" heartbeat list --json >/dev/null 2>&1 || true
    expect "$transport GET, K5_CLI_VIA $via: $want" "$transport" /cli/heartbeat/list "$want"
    FIELD=noun expect "$transport GET, K5_CLI_VIA $via: X-K5-Cli-Noun $get_noun" "$transport" /cli/heartbeat/list "$get_noun"
    FIELD=owner expect "$transport GET, K5_CLI_VIA $via, a passport: no X-K5-Owner-Source" "$transport" /cli/heartbeat/list -
    : >"$WORK/requests.log"
    "$transport" ${pre[@]+"${pre[@]}"} "$K2_CLI" msg some-ws "hello" >/dev/null 2>&1 || true
    expect "$transport POST, K5_CLI_VIA $via: $want" "$transport" /cli/workspace/msg "$want"
    FIELD=noun expect "$transport POST, K5_CLI_VIA $via: X-K5-Cli-Noun $post_noun" "$transport" /cli/workspace/msg "$post_noun"
    FIELD=owner expect "$transport POST, K5_CLI_VIA $via, a passport: no X-K5-Owner-Source" "$transport" /cli/workspace/msg -
}

echo "== X-K5-Cli-Via =="
for t in tcp uds; do
    one "$t" "=rust" fallthrough
    one "$t" unset -
    one "$t" "=" -
    one "$t" "=RUST" -
    one "$t" "=bash" -
done

echo "== X-K5-Would-Refuse =="
# refuse <transport> <want refuse> <want via> <env…>: a GET and a form POST.
refuse() {
    local transport="$1" want="$2" via="$3"; shift 3
    local label="$transport, $*"
    : >"$WORK/requests.log"
    "$transport" "$@" "$K2_CLI" heartbeat list --json >/dev/null 2>&1 || true
    FIELD=refuse expect "GET $label: X-K5-Would-Refuse $want" "$transport" /cli/heartbeat/list "$want"
    expect "GET $label: X-K5-Cli-Via $via" "$transport" /cli/heartbeat/list "$via"
    : >"$WORK/requests.log"
    "$transport" "$@" "$K2_CLI" msg some-ws "hello" >/dev/null 2>&1 || true
    FIELD=refuse expect "POST $label: X-K5-Would-Refuse $want" "$transport" /cli/workspace/msg "$want"
}
for t in tcp uds; do
    refuse "$t" 1 fallthrough K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=1
    refuse "$t" - fallthrough K5_CLI_VIA=rust
    refuse "$t" - fallthrough K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=
    refuse "$t" - fallthrough K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=2
    refuse "$t" - fallthrough K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=yes
    refuse "$t" - fallthrough K5_CLI_VIA=rust "K5_CLI_WOULD_REFUSE= 1"
    refuse "$t" - fallthrough K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=11
    refuse "$t" - - K5_CLI_WOULD_REFUSE=1
    refuse "$t" - - K5_CLI_VIA=bash K5_CLI_WOULD_REFUSE=1
    refuse "$t" - - K5_CLI_VIA=RUST K5_CLI_WOULD_REFUSE=1
done
# The owner path (no passport): the front's case 3 rides the owner token.
: >"$WORK/requests.log"
clean_env K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=1 "$K2_CLI" heartbeat list --json >/dev/null 2>&1 || true
FIELD=refuse expect "owner token, VIA=rust and WOULD_REFUSE=1: X-K5-Would-Refuse 1" tcp /cli/heartbeat/list 1

echo "== X-K5-Owner-Source =="
# owner <want> <label> <env…>: a GET and a form POST, over TCP.
owner() {
    local want="$1" label="$2"; shift 2
    : >"$WORK/requests.log"
    clean_env "$@" "$K2_CLI" heartbeat list --json >/dev/null 2>&1 || true
    FIELD=owner expect "GET, $label: X-K5-Owner-Source $want" tcp /cli/heartbeat/list "$want"
    : >"$WORK/requests.log"
    clean_env "$@" "$K2_CLI" msg some-ws "hello" >/dev/null 2>&1 || true
    FIELD=owner expect "POST, $label: X-K5-Owner-Source $want" tcp /cli/workspace/msg "$want"
}
owner disk "no passport, the owner token from disk, VIA=rust" K5_CLI_VIA=rust
owner - "no passport, the owner token from disk, VIA unset"
owner - "the owner token from disk, VIA=RUST" K5_CLI_VIA=RUST
owner - "a passport, VIA=rust" K5_CLI_VIA=rust K2_HOOK_TOKEN=sessA.passport
# The same secret given as the session's token is not read from disk.
owner - "the owner's token from the environment, VIA=rust" K5_CLI_VIA=rust K2_HOOK_TOKEN=owner-test-token
: >"$WORK/requests.log"
clean_env K5_CLI_VIA=rust "$K2_CLI" heartbeat list --json >/dev/null 2>&1 || true
expect "owner from disk, VIA=rust: X-K5-Cli-Via fallthrough beside it" tcp /cli/heartbeat/list fallthrough
FIELD=noun expect "owner from disk, VIA=rust: X-K5-Cli-Noun heartbeat beside it" tcp /cli/heartbeat/list heartbeat
FIELD=refuse expect "owner from disk, VIA=rust: no X-K5-Would-Refuse without WOULD_REFUSE=1" tcp /cli/heartbeat/list -

echo "== X-K5-Cli-Noun: an alias is the word as typed =="
: >"$WORK/requests.log"
tcp K5_CLI_VIA=rust "$K2_CLI" tickets list --json >/dev/null 2>&1 || true
FIELD=noun expect "k2 tickets list: X-K5-Cli-Noun tickets" tcp /cli/feedback/list tickets
: >"$WORK/requests.log"
tcp K5_CLI_VIA=rust "$K2_CLI" feedback list --json >/dev/null 2>&1 || true
FIELD=noun expect "k2 feedback list: X-K5-Cli-Noun feedback" tcp /cli/feedback/list feedback

echo "== X-K5-Cli-Noun: a word the daemon would not take is sent as other =="
# _k5_headers on its own, from the bash source, for nouns no router arm
# reaches a daemon with.
LIB="$WORK/k5-headers.sh"
sed -n '/^# BEGIN_CLI_CURL_CFG$/,/^# END_CLI_CURL_CFG$/p' "$K2_CLI" >"$LIB"
grep -q '^_k5_headers() {' "$LIB" || { echo "FAIL: no _k5_headers in the curl block of $K2_CLI" >&2; exit 1; }
# noun_of <noun>: the X-K5-Cli-Noun value _k5_headers gives it.
noun_of() {
    bash -c 'set -euo pipefail; . "$1"; _K5_CLI_VIA=rust _K5_CLI_NOUN="$2"; _k5_headers
        for h in "${_K5_HDRS[@]}"; do case "$h" in "X-K5-Cli-Noun: "*) printf "%s
" "${h#X-K5-Cli-Noun: }" ;; esac; done' _ "$LIB" "$1"
}
for n in heartbeat a k9 app-update a-1 "$(printf 'a%.0s' $(seq 1 32))"; do
    got="$(noun_of "$n")"
    if [ "$got" = "$n" ]; then ok "noun '$n' is sent as is"; else bad "noun '$n' gave '$got'"; fi
done
for n in "" Heartbeat HEARTBEAT 9lives -h --schema "heart beat" "heartbeat list" a_b "a.b" "héllo" "$(printf 'a%.0s' $(seq 1 33))" "a"$'\n'"b" "msg;x"; do
    got="$(noun_of "$n")"
    if [ "$got" = other ]; then ok "noun '$n' is sent as other"; else bad "noun '$n' gave '$got', wanted other"; fi
done

echo "== the Connect control plane is not the daemon =="
printf '{"token":"k2c_test-connect","subdomain":"me"}' >"$WORK/home/.k2/tunnel.json"
chmod 600 "$WORK/home/.k2/tunnel.json"
: >"$WORK/requests.log"
clean_env K5_CLI_VIA=rust K2_CONNECT_BASE="http://127.0.0.1:${PORT}" "$K2_CLI" publish status --json >/dev/null 2>&1 || true
expect "the daemon's proxy route gets the header" tcp /cli/connect/proxy fallthrough
expect "the control plane does not" tcp /subdomains -
: >"$WORK/requests.log"
clean_env K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=1 K2_CONNECT_BASE="http://127.0.0.1:${PORT}" "$K2_CLI" publish status --json >/dev/null 2>&1 || true
FIELD=refuse expect "with WOULD_REFUSE=1 the daemon's proxy route gets X-K5-Would-Refuse" tcp /cli/connect/proxy 1
FIELD=refuse expect "the control plane never does" tcp /subdomains -
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
# An owner terminal: no session, so the vault calls go to vault.sock, and
# the call runs on the owner token from disk (X-K5-Owner-Source: disk).
owner_shim() { clean_env PATH="$WORK/shim:$PATH" "$@"; }
# site_run <tag> <stdin> <transport> <path> <argv match> <cmd…>: run once,
# keep the matching requests (the four headers split off, as
# via|noun|owner|refuse) and curl argvs.
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
        f.write("|".join(",".join(r.pop(k)) or "-" for k in ("via", "noun", "owner", "refuse")) + "\n")
with open(out + ".req", "w") as f:
    for r in hits:
        f.write(json.dumps(r, sort_keys=True) + "\n")
PY
    grep -F -- "$match" "$WORK/argv.log" >"$WORK/$tag.argv" || true
}
# site <label> <stdin> <transport> <path> <argv match> <max-time> <cmd…>
# (cmd is "$K2_CLI" <noun> …).
site() {
    local label="$1" input="$2" transport="$3" path="$4" match="$5" mt="$6"; shift 6
    local noun="$2" hdr_argv
    hdr_argv="$(printf '%q ' -H "X-K5-Cli-Via: fallthrough" -H "X-K5-Cli-Noun: $noun" -H "X-K5-Owner-Source: disk")"
    site_run off "$input" "$transport" "$path" "$match" "$@"
    site_run on "$input" "$transport" "$path" "$match" K5_CLI_VIA=rust "$@"
    if [ ! -s "$WORK/off.req" ] || [ ! -s "$WORK/off.argv" ]; then
        bad "$label: no request or no curl argv seen ($(tail -2 "$WORK/requests.log" | tr '\n' ' '))"
        return
    fi
    if [ -z "$(grep -vxF -- "-|-|-|-" "$WORK/off.via")" ]; then
        ok "$label: no header when VIA is unset"
    else
        bad "$label: a header without VIA ($(tr '\n' ' ' <"$WORK/off.via"))"
    fi
    if [ -s "$WORK/on.via" ] && [ -z "$(grep -vxF -- "fallthrough|$noun|disk|-" "$WORK/on.via")" ]; then
        ok "$label: X-K5-Cli-Via: fallthrough, X-K5-Cli-Noun: $noun and X-K5-Owner-Source: disk with VIA=rust"
    else
        bad "$label: with VIA=rust got '$(tr '\n' ' ' <"$WORK/on.via")', wanted fallthrough|$noun|disk|- on each"
    fi
    if cmp -s "$WORK/off.req" "$WORK/on.req"; then
        ok "$label: the request is otherwise the same"
    else
        bad "$label: the request changed ($(diff "$WORK/off.req" "$WORK/on.req" | tr '\n' ' '))"
    fi
    # Each VIA=rust argv holds the headers once, together and in order;
    # without them, it is the unset run's argv.
    local on_less n_hdr=0 l
    while IFS= read -r l; do
        [[ "$l" != *"$hdr_argv"* ]] || n_hdr=$((n_hdr + 1))
    done <"$WORK/on.argv"
    on_less="$(while IFS= read -r l; do printf '%s\n' "${l/"$hdr_argv"/}"; done <"$WORK/on.argv")"
    if [ "$n_hdr" = "$(wc -l <"$WORK/on.argv" | tr -d ' ')" ] && [ "$on_less" = "$(cat "$WORK/off.argv")" ]; then
        ok "$label: curl argv is the same but for the headers"
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
want_traced "WOULD_REFUSE unset: empty" _K5_WOULD_REFUSE "" K2_HOOK_TOKEN=x
want_traced "WOULD_REFUSE=1: 1" _K5_WOULD_REFUSE 1 K5_CLI_WOULD_REFUSE=1
want_traced "WOULD_REFUSE=yes: empty" _K5_WOULD_REFUSE "" K5_CLI_WOULD_REFUSE=yes

echo
echo "k5_cli_via: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
