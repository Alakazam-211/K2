#!/usr/bin/env bash
# The proxy rule gives the same answer in every helper that holds it.
#
# Loopback (127.0.0.1, ::1, localhost) and plain http never use a proxy;
# HTTPS to a remote K2 may use the person's proxy. The rule lives in
# k2_client::proxy_rule::direct (Rust), _k2_direct_url (the CLI's curl
# calls) and _K2_PY_PROXY_RULE's k2_direct (the CLI's embedded Python).
# Each row of tests/fixtures/proxy_rule/table.txt goes through the bash and
# the Python helper here; the Rust helper's test reads the same table.
#
# Then the Python helper's opener, in a child with every proxy variable
# set to a listening stub: a request to a remote https host asks the stub
# for a CONNECT tunnel; requests to 127.0.0.1 and to localhost never reach
# it. The stubs listen on 127.0.0.1 only, the one loopback address every
# system has (macOS has no 127.0.0.2, and a Linux host may have IPv6 off);
# the table above already holds the rule's answer for 127.0.0.2 and [::1].
#
# bash-only: reads the bash CLI's helpers out of cli/k2.
# Run with: bash tests/cli/proxy_rule_parity.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
TABLE="$SCRIPT_DIR/../fixtures/proxy_rule/table.txt"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

# The two helpers, as cli/k2 defines them.
eval "$(sed -n '/^_k2_direct_url() {/,/^}/p' "$CLI_BASH_SOURCE")"
eval "$(sed -n '/^_K2_PY_PROXY_RULE=/,/^export _K2_PY_PROXY_RULE/p' "$CLI_BASH_SOURCE")"
declare -F _k2_direct_url >/dev/null || { echo "FAIL: _k2_direct_url not found in cli/k2" >&2; exit 1; }
[ -n "${_K2_PY_PROXY_RULE:-}" ] || { echo "FAIL: _K2_PY_PROXY_RULE not found in cli/k2" >&2; exit 1; }

echo "== every row: bash and Python agree with the table =="
rows=0
while read -r want url; do
    case "$want" in ''|'#'*) continue ;; direct|proxy) ;; *) bad "bad row: $want $url"; continue ;; esac
    rows=$((rows + 1))
    if _k2_direct_url "$url"; then b=direct; else b=proxy; fi
    p="$(python3 -c 'import os, sys
exec(os.environ["_K2_PY_PROXY_RULE"])
print("direct" if k2_direct(sys.argv[1]) else "proxy")' "$url")"
    if [ "$b" = "$want" ] && [ "$p" = "$want" ]; then
        ok "$url: $want"
    else
        bad "$url: want $want, bash $b, python $p"
    fi
done <"$TABLE"
[ "$rows" -ge 19 ] && ok "the table has $rows rows" || bad "the table lost rows: $rows"

echo "== the Python opener with every proxy variable set =="
WORK="$(mktemp -d -t k2-proxy-rule-XXXXXX)"
STUB_PID=""
cleanup() { [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
python3 - "$WORK" <<'PYEOF' &
import os, socketserver, sys, threading
work = sys.argv[1]
class P(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(2)
        try:
            data = self.request.recv(4096)
        except OSError:
            data = b""
        with open(os.path.join(work, "proxy.log"), "a") as f:
            f.write(data.split(b"\r\n", 1)[0].decode("latin-1") + "\n")
        try:
            self.request.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        except OSError:
            pass
class D(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(2)
        try:
            data = self.request.recv(4096)
        except OSError:
            data = b""
        with open(os.path.join(work, "server.log"), "a") as f:
            f.write(data.split(b"\r\n", 1)[0].decode("latin-1") + "\n")
        self.request.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
class T(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    allow_reuse_address = True
p = T(("127.0.0.1", 0), P)
d1 = T(("127.0.0.1", 0), D)
for s in (p, d1):
    threading.Thread(target=s.serve_forever, daemon=True).start()
with open(os.path.join(work, "ports.tmp"), "w") as f:
    f.write("%d %d\n" % (p.server_address[1], d1.server_address[1]))
os.rename(os.path.join(work, "ports.tmp"), os.path.join(work, "ports"))
threading.Event().wait()
PYEOF
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$WORK/ports" ] && break; sleep 0.05; done
[ -s "$WORK/ports" ] || { echo "FAIL: the stubs never bound" >&2; exit 1; }
read -r PPORT DPORT <"$WORK/ports"
PROXY="http://127.0.0.1:$PPORT"
: >"$WORK/proxy.log"; : >"$WORK/server.log"

fetch() { # fetch <url>: one urlopen through the helper's opener; prints status or error
    env -u no_proxy -u NO_PROXY http_proxy="$PROXY" HTTP_PROXY="$PROXY" https_proxy="$PROXY" \
        HTTPS_PROXY="$PROXY" ALL_PROXY="$PROXY" all_proxy="$PROXY" python3 -c 'import os, sys, urllib.request
exec(os.environ["_K2_PY_PROXY_RULE"])
try:
    print(urllib.request.urlopen(sys.argv[1], timeout=5).status)
except Exception as e:
    print("error " + type(e).__name__)' "$1"
}

out="$(fetch "https://k2-remote.example.test/cli/x")"
if grep -qx "CONNECT k2-remote.example.test:443 HTTP/1.1" "$WORK/proxy.log"; then
    ok "remote https asked the proxy for a tunnel ($out)"
else
    bad "remote https did not go through the proxy ($out; proxy log: $(tr '\n' ';' <"$WORK/proxy.log"))"
fi
for url in "http://127.0.0.1:$DPORT/x" "http://localhost:$DPORT/x"; do
    : >"$WORK/proxy.log"
    out="$(fetch "$url")"
    if [ -s "$WORK/proxy.log" ]; then
        bad "$url reached the proxy stub: $(tr '\n' ';' <"$WORK/proxy.log")"
    elif [ "$out" = 200 ]; then
        ok "$url skipped the proxy and reached the server"
    else
        bad "$url: no answer from the server ($out)"
    fi
done
[ "$(grep -c '^GET /x' "$WORK/server.log")" = 2 ] && ok "the server saw both loopback requests" || bad "server log: $(tr '\n' ';' <"$WORK/server.log")"

echo
echo "proxy_rule_parity: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
